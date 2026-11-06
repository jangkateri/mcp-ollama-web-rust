//! The MCP server itself: a JSON-RPC 2.0 dispatcher.
//!
//! Everything protocol-facing lives in [`McpServer::handle_message`], which is a
//! pure async function of one message plus the injected Ollama client, so the
//! whole surface is testable in-process without pipes. The stdio plumbing lives
//! in [`transport`](crate::transport).

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use serde_json::{Map, Value, json};
use tokio_util::sync::CancellationToken;

use crate::error::OllamaErrorCode;
use crate::logging::Logger;
use crate::ollama::WebClient;
use crate::protocol::{
    Classified, RequestId, classify_message, error_response, negotiate_protocol_version,
    result_response,
};
use crate::tools::{CallToolError, call_tool, error_tool_result, tool_definitions};

pub use crate::protocol::error_code as ErrorCode;

pub const SERVER_NAME: &str = "mcp-ollama-web-rust";
pub const SERVER_TITLE: &str = "Ollama Web Search and Fetch (Rust)";
/// Kept in step with `Cargo.toml` by `CARGO_PKG_VERSION`, so the wire version
/// never drifts from the release.
pub const SERVER_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Text published to clients that support server instructions. Harnesses such
/// as DeepSeek Harness place this in the system prompt, so it stays short.
pub const SERVER_INSTRUCTIONS: &str = "web_search and web_fetch reach the public web through Ollama's web search — a local daemon or the \
     hosted API, whichever this server is pointed at. Prefer web_search for current facts, then web_fetch \
     a promising result when its snippet is not enough.";

/// One MCP server instance bound to an Ollama client.
///
/// Reuse one instance per connection; it tracks the negotiated protocol version
/// and the in-flight calls that `notifications/cancelled` can abort.
pub struct McpServer<C: WebClient + ?Sized> {
    client: Arc<C>,
    logger: Logger,
    protocol_version: Mutex<Option<String>>,
    initialized: AtomicBool,
    in_flight: Mutex<HashMap<RequestId, CancellationToken>>,
    instructions: String,
}

impl<C: WebClient + ?Sized> McpServer<C> {
    pub fn new(client: Arc<C>, logger: Logger) -> Self {
        Self::with_instructions(client, logger, SERVER_INSTRUCTIONS)
    }

    /// Pass an empty string to publish no instructions.
    pub fn with_instructions(
        client: Arc<C>,
        logger: Logger,
        instructions: impl Into<String>,
    ) -> Self {
        Self {
            client,
            logger,
            protocol_version: Mutex::new(None),
            initialized: AtomicBool::new(false),
            in_flight: Mutex::new(HashMap::new()),
            instructions: instructions.into(),
        }
    }

    /// The protocol revision negotiated by the last `initialize`, if any.
    pub fn protocol_version(&self) -> Option<String> {
        self.protocol_version
            .lock()
            .expect("protocol version mutex poisoned")
            .clone()
    }

    /// Whether the client has completed the `initialize` handshake.
    pub fn initialized(&self) -> bool {
        self.initialized.load(Ordering::Relaxed)
    }

    /// Number of tool calls currently running; exposed for tests and diagnostics.
    pub fn in_flight_count(&self) -> usize {
        self.in_flight
            .lock()
            .expect("in-flight mutex poisoned")
            .len()
    }

    /// Abort every running tool call; used when the transport goes away.
    pub fn abort_all(&self) {
        let mut in_flight = self.in_flight.lock().expect("in-flight mutex poisoned");
        for token in in_flight.values() {
            token.cancel();
        }
        in_flight.clear();
    }

    /// Handle one decoded JSON-RPC message.
    ///
    /// Returns the response to send, or `None` for notifications and
    /// unsolicited responses.
    pub async fn handle_message(&self, message: &Value) -> Option<Value> {
        match classify_message(message) {
            Classified::Invalid { id, reason } => {
                self.logger.warn(format!("invalid message: {reason}"));
                Some(error_response(
                    id,
                    ErrorCode::INVALID_REQUEST,
                    format!("Invalid JSON-RPC message: {reason}"),
                ))
            }
            Classified::Response { id } => {
                // This server never sends requests, so a response is unsolicited.
                if let Some(id) = id {
                    self.logger
                        .debug(format!("ignoring unsolicited response {id}"));
                }
                None
            }
            Classified::Notification { method, params } => {
                self.handle_notification(&method, &params);
                None
            }
            Classified::Request { id, method, params } => {
                Some(self.handle_request(id, &method, &params).await)
            }
        }
    }

    /// Parse and handle one raw line of the stdio transport.
    pub async fn handle_line(&self, line: &str) -> Option<Value> {
        match serde_json::from_str::<Value>(line) {
            Ok(message) => self.handle_message(&message).await,
            Err(error) => {
                self.logger.warn(format!("unparseable message: {error}"));
                Some(error_response(
                    None,
                    ErrorCode::PARSE_ERROR,
                    format!("Parse error: {error}"),
                ))
            }
        }
    }

    fn handle_notification(&self, method: &str, params: &Value) {
        match method {
            "notifications/initialized" => {
                self.initialized.store(true, Ordering::Relaxed);
                self.logger.debug("client completed initialization");
            }
            "notifications/cancelled" => {
                let Some(request_id) = params
                    .get("requestId")
                    .and_then(|value| serde_json::from_value::<RequestId>(value.clone()).ok())
                else {
                    return;
                };
                let in_flight = self.in_flight.lock().expect("in-flight mutex poisoned");
                if let Some(token) = in_flight.get(&request_id) {
                    self.logger
                        .debug(format!("cancelling request {request_id}"));
                    token.cancel();
                }
            }
            other => self.logger.debug(format!("ignoring notification {other}")),
        }
    }

    async fn handle_request(&self, id: RequestId, method: &str, params: &Value) -> Value {
        match method {
            "initialize" => self.initialize(id, params),
            "ping" => result_response(Some(id), json!({})),
            "tools/list" => result_response(Some(id), json!({ "tools": tool_definitions() })),
            "tools/call" => self.tools_call(id, params).await,
            // Deprecated, but some clients still send it; accepting keeps them quiet.
            "logging/setLevel" => result_response(Some(id), json!({})),
            other => {
                self.logger.debug(format!("unsupported method {other}"));
                // `server/discover` deliberately lands here: a 2026-era client
                // uses Method Not Found as the signal to fall back to `initialize`.
                error_response(
                    Some(id),
                    ErrorCode::METHOD_NOT_FOUND,
                    format!(
                        "Method not found: {other}. This server implements initialize, ping, tools/list, \
                         tools/call and logging/setLevel."
                    ),
                )
            }
        }
    }

    fn initialize(&self, id: RequestId, params: &Value) -> Value {
        let requested = params.get("protocolVersion").and_then(Value::as_str);
        let negotiated = negotiate_protocol_version(requested);

        let mut result = json!({
            "protocolVersion": negotiated,
            "capabilities": { "tools": { "listChanged": false } },
            "serverInfo": {
                "name": SERVER_NAME,
                "title": SERVER_TITLE,
                "version": SERVER_VERSION,
            },
        });
        if !self.instructions.trim().is_empty() {
            result["instructions"] = json!(self.instructions);
        }

        *self
            .protocol_version
            .lock()
            .expect("protocol version mutex poisoned") = Some(negotiated.to_owned());

        let client_name = params
            .get("clientInfo")
            .and_then(|info| info.get("name"))
            .and_then(Value::as_str)
            .unwrap_or("unknown client");
        self.logger.info(format!(
            "initialize from {client_name} (requested {}, using {negotiated})",
            requested.unwrap_or("none")
        ));

        result_response(Some(id), result)
    }

    async fn tools_call(&self, id: RequestId, params: &Value) -> Value {
        let Some(name) = params.get("name").and_then(Value::as_str) else {
            return error_response(
                Some(id),
                ErrorCode::INVALID_PARAMS,
                "tools/call requires a string \"name\".",
            );
        };
        if name.is_empty() {
            return error_response(
                Some(id),
                ErrorCode::INVALID_PARAMS,
                "tools/call requires a string \"name\".",
            );
        }

        let arguments = match params.get("arguments") {
            None | Some(Value::Null) => Map::new(),
            Some(Value::Object(map)) => map.clone(),
            Some(_) => {
                return error_response(
                    Some(id),
                    ErrorCode::INVALID_PARAMS,
                    "tools/call \"arguments\" must be a JSON object.",
                );
            }
        };

        let cancel = CancellationToken::new();
        self.in_flight
            .lock()
            .expect("in-flight mutex poisoned")
            .insert(id.clone(), cancel.clone());

        let started = std::time::Instant::now();
        let outcome = call_tool(&*self.client, name, &arguments, cancel).await;

        let result = match outcome {
            Ok(result) => {
                self.logger.debug(format!(
                    "tools/call {name} completed in {} ms",
                    started.elapsed().as_millis()
                ));
                result_response(Some(id.clone()), result)
            }
            Err(CallToolError::Argument(error)) => {
                self.logger.warn(format!("tools/call rejected: {error}"));
                error_response(
                    Some(id.clone()),
                    ErrorCode::INVALID_PARAMS,
                    error.to_string(),
                )
            }
            Err(CallToolError::Unknown(error)) => {
                self.logger.warn(format!("tools/call rejected: {error}"));
                error_response(
                    Some(id.clone()),
                    ErrorCode::INVALID_PARAMS,
                    error.to_string(),
                )
            }
            Err(CallToolError::Ollama(error)) => {
                self.logger.warn(format!(
                    "tools/call {name} failed: {}{}",
                    error.message,
                    if error.code == OllamaErrorCode::Aborted {
                        " (aborted)"
                    } else {
                        ""
                    }
                ));
                // Execution failure: hand the model an isError result it can
                // read and react to rather than a bare protocol error.
                result_response(Some(id.clone()), error_tool_result(&error))
            }
        };

        self.in_flight
            .lock()
            .expect("in-flight mutex poisoned")
            .remove(&id);
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::future::Future;
    use std::pin::Pin;

    use tokio_util::sync::CancellationToken;

    use crate::error::OllamaError;
    use crate::ollama::{FetchRequest, FetchResponse, SearchHit, SearchRequest, SearchResponse};

    struct StubClient;

    impl WebClient for StubClient {
        fn search<'a>(
            &'a self,
            request: SearchRequest,
            _cancel: CancellationToken,
        ) -> Pin<Box<dyn Future<Output = Result<SearchResponse, OllamaError>> + Send + 'a>>
        {
            Box::pin(async move {
                Ok(SearchResponse {
                    query: request.query,
                    results: vec![SearchHit {
                        title: "Example".to_owned(),
                        url: "https://example.com/".to_owned(),
                        snippet: "A snippet".to_owned(),
                    }],
                    dropped: 0,
                })
            })
        }

        fn fetch<'a>(
            &'a self,
            request: FetchRequest,
            _cancel: CancellationToken,
        ) -> Pin<Box<dyn Future<Output = Result<FetchResponse, OllamaError>> + Send + 'a>> {
            Box::pin(async move {
                Ok(FetchResponse {
                    url: request.url,
                    title: "Example".to_owned(),
                    content: "Hello".to_owned(),
                    links: vec![],
                    truncated: false,
                    total_chars: 5,
                })
            })
        }
    }

    struct FailingClient;

    impl WebClient for FailingClient {
        fn search<'a>(
            &'a self,
            _request: SearchRequest,
            _cancel: CancellationToken,
        ) -> Pin<Box<dyn Future<Output = Result<SearchResponse, OllamaError>> + Send + 'a>>
        {
            Box::pin(async {
                Err(OllamaError::new(
                    "Cannot reach Ollama at http://127.0.0.1:11434 (search): refused",
                    OllamaErrorCode::Unreachable,
                ))
            })
        }

        fn fetch<'a>(
            &'a self,
            _request: FetchRequest,
            _cancel: CancellationToken,
        ) -> Pin<Box<dyn Future<Output = Result<FetchResponse, OllamaError>> + Send + 'a>> {
            Box::pin(async {
                Err(OllamaError::new(
                    "Cannot reach Ollama",
                    OllamaErrorCode::Unreachable,
                ))
            })
        }
    }

    /// A client whose calls only finish when the server cancels them.
    struct HangingClient;

    impl WebClient for HangingClient {
        fn search<'a>(
            &'a self,
            _request: SearchRequest,
            cancel: CancellationToken,
        ) -> Pin<Box<dyn Future<Output = Result<SearchResponse, OllamaError>> + Send + 'a>>
        {
            Box::pin(async move {
                cancel.cancelled().await;
                Err(OllamaError::aborted("search"))
            })
        }

        fn fetch<'a>(
            &'a self,
            _request: FetchRequest,
            cancel: CancellationToken,
        ) -> Pin<Box<dyn Future<Output = Result<FetchResponse, OllamaError>> + Send + 'a>> {
            Box::pin(async move {
                cancel.cancelled().await;
                Err(OllamaError::aborted("fetch"))
            })
        }
    }

    fn server() -> McpServer<StubClient> {
        McpServer::new(Arc::new(StubClient), Logger::new(false))
    }

    async fn handle(server: &McpServer<StubClient>, message: Value) -> Value {
        server
            .handle_message(&message)
            .await
            .expect("request should produce a response")
    }

    #[tokio::test]
    async fn initialize_negotiates_and_publishes_instructions() {
        let server = server();
        let response = handle(
            &server,
            json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": {
                    "protocolVersion": "2025-06-18",
                    "clientInfo": { "name": "test-client" }
                }
            }),
        )
        .await;

        assert_eq!(response["result"]["protocolVersion"], "2025-06-18");
        assert_eq!(response["result"]["serverInfo"]["name"], SERVER_NAME);
        assert!(response["result"]["instructions"].is_string());
        assert_eq!(server.protocol_version().as_deref(), Some("2025-06-18"));
    }

    #[tokio::test]
    async fn discover_is_method_not_found_so_modern_clients_fall_back() {
        let server = server();
        let response = handle(
            &server,
            json!({"jsonrpc":"2.0","id":2,"method":"server/discover","params":{}}),
        )
        .await;
        assert_eq!(response["error"]["code"], -32601);
    }

    #[tokio::test]
    async fn ping_and_tools_list_work_before_initialize() {
        let server = server();
        let ping = handle(
            &server,
            json!({"jsonrpc":"2.0","id":1,"method":"ping","params":{}}),
        )
        .await;
        assert_eq!(ping["result"], json!({}));

        let list = handle(
            &server,
            json!({"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}),
        )
        .await;
        assert_eq!(list["result"]["tools"].as_array().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn tools_call_returns_text_and_structured_content() {
        let server = server();
        let response = handle(
            &server,
            json!({
                "jsonrpc":"2.0","id":3,"method":"tools/call",
                "params":{"name":"web_search","arguments":{"query":"rust"}}
            }),
        )
        .await;

        assert_eq!(response["result"]["isError"], Value::Null);
        assert_eq!(
            response["result"]["structuredContent"]["results"][0]["url"],
            "https://example.com/"
        );
        assert!(
            response["result"]["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains("1 result for \"rust\"")
        );
        assert_eq!(server.in_flight_count(), 0);
    }

    #[tokio::test]
    async fn tools_call_fetch_returns_text_and_structured_content() {
        let server = server();
        let response = handle(
            &server,
            json!({
                "jsonrpc":"2.0","id":7,"method":"tools/call",
                "params":{"name":"web_fetch","arguments":{"url":"https://example.com/page"}}
            }),
        )
        .await;

        assert_eq!(response["result"]["isError"], Value::Null);
        assert_eq!(response["result"]["structuredContent"]["content"], "Hello");
        assert_eq!(response["result"]["structuredContent"]["total_chars"], 5);
        let text = response["result"]["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("# Example"));
        assert!(text.contains("https://example.com/page"));
        assert!(text.contains("5 characters extracted"));
        assert_eq!(server.in_flight_count(), 0);
    }

    #[tokio::test]
    async fn argument_faults_are_jsonrpc_errors() {
        let server = server();
        let response = handle(
            &server,
            json!({
                "jsonrpc":"2.0","id":4,"method":"tools/call",
                "params":{"name":"web_search","arguments":{"query":"x","max_results":99}}
            }),
        )
        .await;
        assert_eq!(response["error"]["code"], -32602);
        assert!(
            response["error"]["message"]
                .as_str()
                .unwrap()
                .contains("between 1 and 10")
        );
    }

    #[tokio::test]
    async fn unknown_tools_are_invalid_params() {
        let server = server();
        let response = handle(
            &server,
            json!({
                "jsonrpc":"2.0","id":5,"method":"tools/call",
                "params":{"name":"nope","arguments":{}}
            }),
        )
        .await;
        assert_eq!(response["error"]["code"], -32602);
        assert!(
            response["error"]["message"]
                .as_str()
                .unwrap()
                .contains("web_search")
        );
    }

    #[tokio::test]
    async fn execution_failures_become_iserror_tool_results() {
        let server = McpServer::new(Arc::new(FailingClient), Logger::new(false));
        let response = server
            .handle_message(&json!({
                "jsonrpc":"2.0","id":6,"method":"tools/call",
                "params":{"name":"web_search","arguments":{"query":"x"}}
            }))
            .await
            .unwrap();
        assert_eq!(response["result"]["isError"], true);
        assert!(
            response["result"]["content"][0]["text"]
                .as_str()
                .unwrap()
                .starts_with("Ollama is not reachable")
        );
    }

    #[tokio::test]
    async fn notifications_are_silent_and_mark_initialization() {
        let server = server();
        assert!(
            server
                .handle_message(&json!({
                    "jsonrpc":"2.0","method":"notifications/initialized"
                }))
                .await
                .is_none()
        );
        assert!(server.initialized());
    }

    #[tokio::test]
    async fn cancellation_notification_aborts_an_in_flight_call() {
        use std::time::Duration;

        let server = Arc::new(McpServer::new(Arc::new(HangingClient), Logger::new(false)));
        let task = {
            let server = Arc::clone(&server);
            tokio::spawn(async move {
                server
                    .handle_message(&json!({
                        "jsonrpc":"2.0","id":8,"method":"tools/call",
                        "params":{"name":"web_search","arguments":{"query":"slow"}}
                    }))
                    .await
            })
        };

        for _ in 0..200 {
            if server.in_flight_count() == 1 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert_eq!(server.in_flight_count(), 1, "the call should be in flight");

        let notification = json!({
            "jsonrpc":"2.0",
            "method":"notifications/cancelled",
            "params":{"requestId":8}
        });
        assert!(server.handle_message(&notification).await.is_none());

        let response = tokio::time::timeout(Duration::from_secs(2), task)
            .await
            .expect("cancelled call finishes")
            .expect("task joins")
            .expect("tools/call returns a response");
        assert_eq!(response["result"]["isError"], true);
        assert!(
            response["result"]["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains("Request cancelled")
        );
        assert_eq!(server.in_flight_count(), 0);
    }

    #[tokio::test]
    async fn parse_errors_are_reported_with_a_null_id() {
        let server = server();
        let response = server.handle_line("{not json").await.unwrap();
        assert_eq!(response["error"]["code"], -32700);
        assert!(response["id"].is_null());
    }
}
