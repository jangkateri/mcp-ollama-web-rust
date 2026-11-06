//! End-to-end tests for the two advertised MCP tools.
//!
//! Each test launches the real server binary, speaks newline-delimited JSON-RPC
//! over its stdin/stdout, and points `OLLAMA_HOST` at a local scripted HTTP
//! server. Nothing here needs the network, a daemon, or a key.

mod support;

use serde_json::{Value, json};
use support::{MockOllama, MockReply, response_for, run_session};

fn initialize_and(call: Value) -> Vec<Value> {
    vec![
        json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-11-25",
                "clientInfo": { "name": "tools-e2e" }
            }
        }),
        json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }),
        call,
    ]
}

fn assert_success(response: &Value) {
    assert!(
        response.get("error").is_none(),
        "expected a result, got {response}"
    );
}

#[test]
fn web_search_returns_normalized_results_and_structured_content() {
    let mock = MockOllama::start(vec![(
        "/api/experimental/web_search",
        MockReply::Json(
            200,
            json!({
                "results": [
                    {
                        "title": "  Ollama   Home  ",
                        "url": "https://ollama.com/",
                        "content": "  Run   open\nmodels "
                    },
                    {
                        "title": "Duplicate",
                        "url": "https://ollama.com/",
                        "content": "duplicate URL"
                    },
                    {
                        "title": "No URL",
                        "url": "",
                        "content": "unusable"
                    },
                    {
                        "title": "Docs",
                        "url": "https://docs.ollama.com/",
                        "content": "API docs"
                    }
                ]
            }),
        ),
    )]);

    let (responses, stderr) = run_session(
        mock.base_url(),
        &initialize_and(json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": {
                "name": "web_search",
                "arguments": { "query": "  ollama  ", "max_results": 3 }
            }
        })),
    );
    assert!(stderr.contains("initialize from tools-e2e"));

    let response = response_for(&responses, 2);
    assert_success(response);
    let result = &response["result"];
    assert_eq!(result["structuredContent"]["query"], "ollama");
    assert_eq!(
        result["structuredContent"]["results"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(result["structuredContent"]["dropped"], 2);
    assert_eq!(
        result["structuredContent"]["results"][0]["title"],
        "Ollama Home"
    );
    assert_eq!(
        result["structuredContent"]["results"][0]["snippet"],
        "Run open models"
    );

    let text = result["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("2 results for \"ollama\" (Ollama web search):"));
    assert!(text.contains("2 unusable duplicate or malformed entries omitted."));

    let requests = mock.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].method, "POST");
    assert_eq!(requests[0].path, "/api/experimental/web_search");
    assert_eq!(requests[0].body["query"], "ollama");
    assert_eq!(requests[0].body["max_results"], 3);
}

#[test]
fn web_fetch_returns_truncated_content_and_links() {
    let mock = MockOllama::start(vec![(
        "/api/experimental/web_fetch",
        MockReply::Json(
            200,
            json!({
                "title": "  Example   Page ",
                "content": "abcdefghij",
                "links": ["https://a.example/", "", "https://b.example/"]
            }),
        ),
    )]);

    let (responses, _) = run_session(
        mock.base_url(),
        &initialize_and(json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": {
                "name": "web_fetch",
                "arguments": {
                    "url": "https://example.com/page",
                    "max_chars": 5
                }
            }
        })),
    );

    let response = response_for(&responses, 2);
    assert_success(response);
    let result = &response["result"];
    assert_eq!(
        result["structuredContent"]["url"],
        "https://example.com/page"
    );
    assert_eq!(result["structuredContent"]["title"], "Example Page");
    assert_eq!(result["structuredContent"]["content"], "abcde");
    assert_eq!(result["structuredContent"]["truncated"], true);
    assert_eq!(result["structuredContent"]["total_chars"], 10);
    assert_eq!(
        result["structuredContent"]["links"],
        json!(["https://a.example/", "https://b.example/"])
    );

    let text = result["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("# Example Page"));
    assert!(text.contains("10 characters extracted"));
    assert!(text.contains(
        "[truncated: showing the first 5 of 10 characters. Call web_fetch again with a larger max_chars"
    ));

    let requests = mock.requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].path, "/api/experimental/web_fetch");
    assert_eq!(requests[0].body["url"], "https://example.com/page");
}

#[test]
fn search_falls_back_to_the_stable_endpoint_after_a_router_miss() {
    let mock = MockOllama::start(vec![
        (
            "/api/experimental/web_search",
            MockReply::Text(404, "404 page not found"),
        ),
        (
            "/api/web_search",
            MockReply::Json(
                200,
                json!({
                    "results": [{
                        "title": "Stable",
                        "url": "https://stable.example/",
                        "content": "found via the stable route"
                    }]
                }),
            ),
        ),
    ]);

    let (responses, _) = run_session(
        mock.base_url(),
        &initialize_and(json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": {
                "name": "web_search",
                "arguments": { "query": "fallback" }
            }
        })),
    );

    let response = response_for(&responses, 2);
    assert_success(response);
    let results = response["result"]["structuredContent"]["results"]
        .as_array()
        .unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0]["url"], "https://stable.example/");

    let requests = mock.requests();
    let paths: Vec<&str> = requests
        .iter()
        .map(|request| request.path.as_str())
        .collect();
    assert_eq!(
        paths,
        vec!["/api/experimental/web_search", "/api/web_search"]
    );
}

#[test]
fn fetch_404_from_an_existing_route_is_not_retried_as_a_missing_endpoint() {
    let mock = MockOllama::start(vec![
        (
            "/api/experimental/web_fetch",
            MockReply::Json(404, json!({ "error": "not found" })),
        ),
        (
            "/api/web_fetch",
            MockReply::Json(
                200,
                json!({ "title": "Should not be used", "content": "wrong route" }),
            ),
        ),
    ]);

    let (responses, _) = run_session(
        mock.base_url(),
        &initialize_and(json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": {
                "name": "web_fetch",
                "arguments": { "url": "https://example.com/missing" }
            }
        })),
    );

    let response = response_for(&responses, 2);
    assert_success(response);
    let result = &response["result"];
    assert_eq!(result["isError"], true);
    let text = result["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("Could not retrieve https://example.com/missing (HTTP 404)."));
    assert!(text.contains("not an OLLAMA_HOST problem"));

    let requests = mock.requests();
    assert_eq!(requests.len(), 1, "a route-exists 404 must not be retried");
    assert_eq!(requests[0].path, "/api/experimental/web_fetch");
}

#[test]
fn unauthorized_is_reported_as_an_actionable_tool_error() {
    let mock = MockOllama::start(vec![(
        "/api/experimental/web_search",
        MockReply::Json(401, json!({ "error": "Unauthorized" })),
    )]);

    let (responses, _) = run_session(
        mock.base_url(),
        &initialize_and(json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": {
                "name": "web_search",
                "arguments": { "query": "auth" }
            }
        })),
    );

    let response = response_for(&responses, 2);
    assert_success(response);
    let result = &response["result"];
    assert_eq!(result["isError"], true);
    let text = result["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("requires an API key (HTTP 401: Unauthorized)"));
    assert!(text.contains("OLLAMA_API_KEY"));
    assert!(text.contains("point OLLAMA_HOST at https://ollama.com"));

    assert_eq!(mock.requests().len(), 1);
}

#[test]
fn invalid_tool_arguments_are_jsonrpc_errors_and_never_reach_ollama() {
    let mock = MockOllama::start(vec![]);

    let (responses, _) = run_session(
        mock.base_url(),
        &[
            json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": { "protocolVersion": "2025-11-25" }
            }),
            json!({
                "jsonrpc": "2.0",
                "id": 2,
                "method": "tools/call",
                "params": {
                    "name": "web_search",
                    "arguments": { "query": "x", "max_results": 99 }
                }
            }),
            json!({
                "jsonrpc": "2.0",
                "id": 3,
                "method": "tools/call",
                "params": { "name": "web_fetch", "arguments": {} }
            }),
            json!({
                "jsonrpc": "2.0",
                "id": 4,
                "method": "tools/call",
                "params": { "name": "not_a_tool", "arguments": {} }
            }),
        ],
    );

    let too_many = response_for(&responses, 2);
    assert_eq!(too_many["error"]["code"], -32602);
    assert!(
        too_many["error"]["message"]
            .as_str()
            .unwrap()
            .contains("between 1 and 10")
    );

    let missing_url = response_for(&responses, 3);
    assert_eq!(missing_url["error"]["code"], -32602);
    assert!(
        missing_url["error"]["message"]
            .as_str()
            .unwrap()
            .contains("\"url\" is required")
    );

    let unknown = response_for(&responses, 4);
    assert_eq!(unknown["error"]["code"], -32602);
    assert!(
        unknown["error"]["message"]
            .as_str()
            .unwrap()
            .contains("web_search, web_fetch")
    );

    assert!(
        mock.requests().is_empty(),
        "argument faults must be rejected before any HTTP call"
    );
}

#[test]
fn non_http_fetch_urls_are_rejected_before_any_http_call() {
    let mock = MockOllama::start(vec![]);

    let (responses, _) = run_session(
        mock.base_url(),
        &initialize_and(json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": {
                "name": "web_fetch",
                "arguments": { "url": "ftp://example.com/file" }
            }
        })),
    );

    let response = response_for(&responses, 2);
    assert_success(response);
    let result = &response["result"];
    assert_eq!(result["isError"], true);
    assert!(
        result["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("Only http and https URLs can be fetched")
    );
    assert!(mock.requests().is_empty());
}
