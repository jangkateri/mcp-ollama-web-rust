//! HTTP client for Ollama's local daemon and hosted web search/fetch APIs.
//!
//! Both targets accept the same requests and answer the same shapes:
//!
//! ```text
//! POST /api/web_search   { query, max_results } -> { results: [{ title, url, content }] }
//! POST /api/web_fetch    { url }                -> { title, content, links }
//! ```
//!
//! Older daemons publish the `/api/experimental/` spellings, so each call falls
//! through to them when the first spelling turns out not to exist. A 404 is not
//! automatically a missing endpoint: see [`is_endpoint_missing`].

use std::collections::HashSet;
use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use reqwest::{StatusCode, header};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;
use url::Url;

use super::host::{
    CLOUD_HOST, CLOUD_KEY_URL, DEFAULT_HOST, is_ollama_cloud, normalize_api_key, normalize_host,
};
use super::types::{FetchRequest, FetchResponse, SearchHit, SearchRequest, SearchResponse};
use super::{
    CLOUD_FETCH_ENDPOINTS, CLOUD_SEARCH_ENDPOINTS, FETCH_ENDPOINTS, MAX_SEARCH_RESULTS,
    SEARCH_ENDPOINTS, WebClient,
};
use crate::error::{OllamaError, OllamaErrorCode};

/// Collapse runs of whitespace so snippets and page text stay readable.
pub fn collapse_whitespace(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Take at most `limit` characters, counting Unicode scalar values.
fn truncate_chars(value: &str, limit: usize) -> String {
    value.chars().take(limit).collect()
}

/// Whether a `404` body says *the endpoint does not exist here*, as opposed to
/// *the endpoint exists and could not retrieve what was asked of it*.
///
/// Measured against a daemon and the hosted API:
///
/// ```text
/// POST /api/web_fetch          -> 404 "404 page not found"        (no such route)
/// POST /api/experimental/...   -> 404 {"error": "not found"}      (route exists, page not)
/// ```
///
/// A plain-text body is a router miss, and so is a JSON body that names the path
/// it could not route. A JSON body saying only `not found` came from a route
/// that exists; retrying would replace a true report with a false claim about a
/// missing endpoint. An HTML body is neither and stays a `BAD_RESPONSE`.
fn is_endpoint_missing(text: &str, path: &str) -> bool {
    let trimmed = text.trim();
    if trimmed.starts_with('<') {
        return false;
    }
    if trimmed.starts_with('{') {
        return trimmed.contains(path);
    }
    !trimmed.is_empty()
}

fn unauthorized_message(status: u16, detail: &str, host: &str, authenticated: bool) -> String {
    let cloud = is_ollama_cloud(host);
    let who = if cloud {
        "The Ollama cloud API".to_owned()
    } else {
        format!("Ollama at {host}")
    };
    let reason = if authenticated {
        format!("{who} rejected the API key (HTTP {status}: {detail}).")
    } else {
        format!("{who} requires an API key (HTTP {status}: {detail}).")
    };
    let hint = if authenticated {
        "Check the OLLAMA_API_KEY value, or the OLLAMA_API_KEY entry of the credentials document named \
         by OLLAMA_CREDENTIALS_FILE."
            .to_owned()
    } else if cloud {
        format!(
            "Set OLLAMA_API_KEY, or put OLLAMA_API_KEY: <value> under refs: in a credentials document \
             and name it with OLLAMA_CREDENTIALS_FILE (create one at {CLOUD_KEY_URL})."
        )
    } else {
        format!(
            "Set OLLAMA_API_KEY, or put OLLAMA_API_KEY: <value> under refs: in a credentials document \
             and name it with OLLAMA_CREDENTIALS_FILE, or point OLLAMA_HOST at {CLOUD_HOST} for the \
             hosted API."
        )
    };
    format!("{reason} {hint}")
}

fn error_detail(parsed: &Value, text: &str) -> String {
    if let Some(detail) = parsed.get("error").and_then(Value::as_str) {
        return detail.to_owned();
    }
    if let Some(detail) = parsed.get("message").and_then(Value::as_str) {
        return detail.to_owned();
    }
    let trimmed = text.trim();
    if trimmed.is_empty() {
        "no response body".to_owned()
    } else {
        truncate_chars(trimmed, 240)
    }
}

fn value_type(value: Option<&Value>) -> &'static str {
    match value {
        None | Some(Value::Null) => "nothing",
        Some(Value::Bool(_)) => "a boolean",
        Some(Value::Number(_)) => "a number",
        Some(Value::String(_)) => "a string",
        Some(Value::Array(_)) => "an array",
        Some(Value::Object(_)) => "an object",
    }
}

/// Ollama-backed web search and fetch.
#[derive(Debug)]
pub struct OllamaClient {
    host: String,
    api_key: Option<String>,
    http: reqwest::Client,
    search_endpoints: &'static [&'static str],
    fetch_endpoints: &'static [&'static str],
}

impl OllamaClient {
    /// Build a client for a daemon or the hosted API.
    ///
    /// `host` is normalized first; `timeout_ms` of `0` disables the per-request
    /// ceiling. A blank, `undefined`, or `null` API key is treated as absent.
    pub fn new(
        host: Option<&str>,
        api_key: Option<String>,
        timeout_ms: u64,
    ) -> Result<Self, OllamaError> {
        let host = normalize_host(host)?;
        let api_key = normalize_api_key(api_key.as_deref());
        let cloud = is_ollama_cloud(&host);

        let timeout = (timeout_ms > 0).then(|| Duration::from_millis(timeout_ms));
        let mut builder = reqwest::Client::builder().redirect(reqwest::redirect::Policy::none());
        if let Some(limit) = timeout {
            builder = builder.timeout(limit);
        }
        let http = builder.build().map_err(|error| {
            OllamaError::new(
                format!("Cannot build the HTTP client: {error}"),
                OllamaErrorCode::Unreachable,
            )
        })?;

        Ok(Self {
            host,
            api_key,
            http,
            search_endpoints: if cloud {
                CLOUD_SEARCH_ENDPOINTS
            } else {
                SEARCH_ENDPOINTS
            },
            fetch_endpoints: if cloud {
                CLOUD_FETCH_ENDPOINTS
            } else {
                FETCH_ENDPOINTS
            },
        })
    }

    /// Build a client with the defaults: local daemon, 90 s ceiling.
    pub fn local() -> Result<Self, OllamaError> {
        Self::new(Some(DEFAULT_HOST), None, 90_000)
    }

    /// The normalized base URL.
    pub fn host(&self) -> &str {
        &self.host
    }

    /// Whether this client talks to Ollama's hosted API rather than a daemon.
    pub fn is_cloud(&self) -> bool {
        is_ollama_cloud(&self.host)
    }

    /// Whether a credential is configured. Never exposes the key itself.
    pub fn authenticated(&self) -> bool {
        self.api_key.is_some()
    }

    /// A log-safe description of the target, e.g. `https://ollama.com (api key)`.
    pub fn describe_target(&self) -> String {
        format!(
            "{} ({})",
            self.host,
            if self.api_key.is_some() {
                "api key"
            } else {
                "no api key"
            }
        )
    }

    /// Run a web search through Ollama.
    pub async fn search(
        &self,
        request: SearchRequest,
        cancel: CancellationToken,
    ) -> Result<SearchResponse, OllamaError> {
        let query = request.query.trim().to_owned();
        if query.is_empty() {
            return Err(OllamaError::invalid_argument("A search query is required"));
        }
        let wanted = request.max_results.clamp(1, MAX_SEARCH_RESULTS);

        let payload = self
            .post_first_ok(
                self.search_endpoints,
                &json!({ "query": query, "max_results": wanted }),
                "search",
                &cancel,
            )
            .await?;

        let Some(items) = payload.get("results").and_then(Value::as_array) else {
            return Err(OllamaError::new(
                format!(
                    "Ollama search returned no \"results\" array (got {})",
                    value_type(payload.get("results"))
                ),
                OllamaErrorCode::BadResponse,
            ));
        };

        let mut results = Vec::new();
        let mut seen = HashSet::new();
        let mut dropped = 0usize;
        for item in items {
            let url = item
                .get("url")
                .and_then(Value::as_str)
                .map(str::trim)
                .unwrap_or_default();
            if url.is_empty() || !seen.insert(url.to_owned()) {
                dropped += 1;
                continue;
            }
            results.push(SearchHit {
                title: item
                    .get("title")
                    .and_then(Value::as_str)
                    .map(collapse_whitespace)
                    .unwrap_or_default(),
                url: url.to_owned(),
                snippet: item
                    .get("content")
                    .and_then(Value::as_str)
                    .map(collapse_whitespace)
                    .unwrap_or_default(),
            });
        }

        Ok(SearchResponse {
            query,
            results,
            dropped,
        })
    }

    /// Fetch one page through Ollama and return its extracted text.
    pub async fn fetch(
        &self,
        request: FetchRequest,
        cancel: CancellationToken,
    ) -> Result<FetchResponse, OllamaError> {
        let target = request.url.trim().to_owned();
        if target.is_empty() {
            return Err(OllamaError::invalid_argument("A URL is required"));
        }
        let parsed = Url::parse(&target)
            .map_err(|_| OllamaError::invalid_argument(format!("Not a valid URL: {target:?}")))?;
        if parsed.scheme() != "http" && parsed.scheme() != "https" {
            return Err(OllamaError::invalid_argument(format!(
                "Only http and https URLs can be fetched, got {:?}",
                parsed.scheme()
            )));
        }

        let payload = match self
            .post_first_ok(
                self.fetch_endpoints,
                &json!({ "url": target }),
                "fetch",
                &cancel,
            )
            .await
        {
            Ok(payload) => payload,
            Err(error)
                if error.code == OllamaErrorCode::HttpError
                    && error.status == Some(404)
                    && !error.endpoint_missing =>
            {
                return Err(OllamaError::new(
                    format!(
                        "Could not retrieve {target} (HTTP 404). Ollama's fetcher reached the endpoint but \
                         could not get that page: the site may refuse automated fetches, the page may not \
                         exist, or its domain may not resolve. The fetch endpoint is present, so this is \
                         not an OLLAMA_HOST problem."
                    ),
                    OllamaErrorCode::HttpError,
                )
                .with_status(404));
            }
            Err(error) => return Err(error),
        };

        let Some(full) = payload.get("content").and_then(Value::as_str) else {
            return Err(OllamaError::new(
                format!(
                    "Ollama fetch returned no page content (got {})",
                    value_type(payload.get("content"))
                ),
                OllamaErrorCode::BadResponse,
            ));
        };

        let total_chars = full.chars().count();
        let limit = request.max_chars.filter(|limit| *limit > 0);
        let truncated = limit.is_some_and(|limit| total_chars > limit);
        let content = match limit {
            Some(limit) if truncated => truncate_chars(full, limit),
            _ => full.to_owned(),
        };

        let links = payload
            .get("links")
            .and_then(Value::as_array)
            .map(|links| {
                links
                    .iter()
                    .filter_map(Value::as_str)
                    .filter(|link| !link.is_empty())
                    .take(50)
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default();

        Ok(FetchResponse {
            url: target,
            title: payload
                .get("title")
                .and_then(Value::as_str)
                .map(collapse_whitespace)
                .unwrap_or_default(),
            content,
            links,
            truncated,
            total_chars,
        })
    }

    /// POST a JSON body to the first endpoint that does not answer "no route".
    async fn post_first_ok(
        &self,
        paths: &'static [&'static str],
        body: &Value,
        kind: &str,
        cancel: &CancellationToken,
    ) -> Result<Value, OllamaError> {
        let mut last_miss = String::new();

        for path in paths {
            let url = format!("{}{}", self.host, path);
            let mut request = self
                .http
                .post(&url)
                .header(header::ACCEPT, "application/json")
                .header(header::CONTENT_TYPE, "application/json")
                .json(body);
            if let Some(key) = self.api_key.as_deref() {
                request = request.bearer_auth(key);
            }

            let send = request.send();
            tokio::pin!(send);
            let response = tokio::select! {
                _ = cancel.cancelled() => return Err(OllamaError::aborted(kind)),
                result = &mut send => match result {
                    Ok(response) => response,
                    Err(error) => return Err(self.transport_error(error, kind, cancel)),
                },
            };

            let status = response.status();
            let read = response.text();
            tokio::pin!(read);
            let text = tokio::select! {
                _ = cancel.cancelled() => return Err(OllamaError::aborted(kind)),
                result = &mut read => match result {
                    Ok(text) => text,
                    Err(error) => return Err(self.transport_error(error, kind, cancel)),
                },
            };

            match self.read_json(&text, status, path, kind) {
                Ok(payload) => return Ok(payload),
                Err(error)
                    if error.code == OllamaErrorCode::HttpError
                        && error.status == Some(404)
                        && error.endpoint_missing =>
                {
                    last_miss = (*path).to_owned();
                }
                Err(error) => return Err(error),
            }
        }

        Err(OllamaError::new(
            format!(
                "Ollama has no {kind} endpoint on {} (tried {}; last miss: {last_miss}). {}",
                self.host,
                paths.join(", "),
                if self.is_cloud() {
                    format!(
                        "The hosted API is expected to serve /api/{}; check the URL.",
                        if kind == "search" {
                            "web_search"
                        } else {
                            "web_fetch"
                        }
                    )
                } else {
                    format!(
                        "Update the daemon, or check that OLLAMA_HOST points at an Ollama instance with \
                         web search enabled, or point it at {CLOUD_HOST} for the hosted API."
                    )
                }
            ),
            OllamaErrorCode::NoEndpoint,
        ))
    }

    /// Parse one JSON response, mapping failures onto typed errors.
    fn read_json(
        &self,
        text: &str,
        status: StatusCode,
        path: &str,
        kind: &str,
    ) -> Result<Value, OllamaError> {
        let absent = status == StatusCode::NOT_FOUND && is_endpoint_missing(text, path);

        let parsed = if text.trim().is_empty() {
            json!({})
        } else {
            match serde_json::from_str::<Value>(text) {
                Ok(value) => value,
                Err(_) => {
                    if absent {
                        return Err(OllamaError::new(
                            format!("Ollama has no endpoint at {path} (HTTP 404)"),
                            OllamaErrorCode::HttpError,
                        )
                        .with_status(404)
                        .as_endpoint_missing());
                    }
                    return Err(OllamaError::new(
                        format!(
                            "Ollama {kind} returned non-JSON (HTTP {}) from {path}: {}",
                            status.as_u16(),
                            truncate_chars(text, 240)
                        ),
                        OllamaErrorCode::BadResponse,
                    )
                    .with_status(status.as_u16()));
                }
            }
        };

        if !status.is_success() {
            let detail = error_detail(&parsed, text);
            if status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN {
                return Err(OllamaError::new(
                    unauthorized_message(
                        status.as_u16(),
                        &detail,
                        &self.host,
                        self.api_key.is_some(),
                    ),
                    OllamaErrorCode::Unauthorized,
                )
                .with_status(status.as_u16()));
            }
            let mut error = OllamaError::new(
                format!("Ollama {kind} failed (HTTP {}): {detail}", status.as_u16()),
                OllamaErrorCode::HttpError,
            )
            .with_status(status.as_u16());
            if absent {
                error = error.as_endpoint_missing();
            }
            return Err(error);
        }

        if !parsed.is_object() {
            return Err(OllamaError::new(
                format!(
                    "Ollama {kind} returned {} instead of an object from {path}",
                    value_type(Some(&parsed))
                ),
                OllamaErrorCode::BadResponse,
            )
            .with_status(status.as_u16()));
        }

        Ok(parsed)
    }

    /// Map a transport failure (connect, DNS, timeout, abort) onto a typed error.
    fn transport_error(
        &self,
        error: reqwest::Error,
        kind: &str,
        cancel: &CancellationToken,
    ) -> OllamaError {
        if cancel.is_cancelled() {
            return OllamaError::aborted(kind);
        }
        if error.is_timeout() {
            return OllamaError::new(format!("Ollama {kind} timed out"), OllamaErrorCode::Timeout);
        }
        OllamaError::new(
            format!(
                "Cannot reach Ollama at {} ({kind}): {error}. Is the daemon running? Check `ollama serve` \
                 and the OLLAMA_HOST value.",
                self.host
            ),
            OllamaErrorCode::Unreachable,
        )
    }
}

impl WebClient for OllamaClient {
    fn search<'a>(
        &'a self,
        request: SearchRequest,
        cancel: CancellationToken,
    ) -> Pin<Box<dyn Future<Output = Result<SearchResponse, OllamaError>> + Send + 'a>> {
        Box::pin(async move { OllamaClient::search(self, request, cancel).await })
    }

    fn fetch<'a>(
        &'a self,
        request: FetchRequest,
        cancel: CancellationToken,
    ) -> Pin<Box<dyn Future<Output = Result<FetchResponse, OllamaError>> + Send + 'a>> {
        Box::pin(async move { OllamaClient::fetch(self, request, cancel).await })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoint_missing_distinguishes_router_misses_from_page_failures() {
        assert!(is_endpoint_missing("404 page not found", "/api/web_search"));
        assert!(is_endpoint_missing(
            r#"{"error":"no route for /api/web_search"}"#,
            "/api/web_search"
        ));
        assert!(!is_endpoint_missing(
            r#"{"error":"not found"}"#,
            "/api/web_fetch"
        ));
        assert!(!is_endpoint_missing("<html>404</html>", "/api/web_fetch"));
        assert!(!is_endpoint_missing("", "/api/web_fetch"));
    }

    #[test]
    fn cloud_targets_try_the_stable_spelling_first() {
        let client = OllamaClient::new(Some(CLOUD_HOST), None, 0).unwrap();
        assert!(client.is_cloud());
        assert_eq!(client.search_endpoints, CLOUD_SEARCH_ENDPOINTS);
        assert_eq!(client.fetch_endpoints, CLOUD_FETCH_ENDPOINTS);
    }

    #[test]
    fn local_targets_try_the_experimental_spelling_first() {
        let client = OllamaClient::new(None, None, 0).unwrap();
        assert!(!client.is_cloud());
        assert_eq!(client.search_endpoints, SEARCH_ENDPOINTS);
        assert_eq!(client.fetch_endpoints, FETCH_ENDPOINTS);
    }

    #[test]
    fn target_description_never_contains_the_key() {
        let client = OllamaClient::new(Some(CLOUD_HOST), Some("secret".to_owned()), 0).unwrap();
        assert_eq!(client.describe_target(), "https://ollama.com (api key)");
        assert!(!client.describe_target().contains("secret"));
    }

    #[test]
    fn whitespace_collapse_handles_newlines_and_tabs() {
        assert_eq!(collapse_whitespace("  a\n\tb   c "), "a b c");
    }
}
