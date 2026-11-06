//! Ollama web search and fetch: target normalization, HTTP transport, and the
//! domain types consumed by the tool layer.

pub mod client;
pub mod host;
pub mod types;

pub use client::{OllamaClient, collapse_whitespace};
pub use host::{
    CLOUD_HOST, CLOUD_KEY_URL, DEFAULT_HOST, is_ollama_cloud, normalize_api_key, normalize_host,
};
pub use types::{FetchRequest, FetchResponse, SearchHit, SearchRequest, SearchResponse};

use std::future::Future;
use std::pin::Pin;

use tokio_util::sync::CancellationToken;

use crate::error::OllamaError;

/// Ollama's documented ceiling for `max_results`.
pub const MAX_SEARCH_RESULTS: usize = 10;

/// Default `max_results` when a caller does not choose one.
pub const DEFAULT_SEARCH_RESULTS: usize = 5;

/// Endpoint spellings tried, most specific first, for a local daemon.
pub const SEARCH_ENDPOINTS: &[&str] = &["/api/experimental/web_search", "/api/web_search"];
pub const FETCH_ENDPOINTS: &[&str] = &["/api/experimental/web_fetch", "/api/web_fetch"];

/// Endpoint spellings tried for the hosted API, where the stable paths are documented.
pub const CLOUD_SEARCH_ENDPOINTS: &[&str] = &["/api/web_search", "/api/experimental/web_search"];
pub const CLOUD_FETCH_ENDPOINTS: &[&str] = &["/api/web_fetch", "/api/experimental/web_fetch"];

/// The capability the MCP server needs from an Ollama backend.
///
/// A trait rather than a concrete type so the server and tool layer can be
/// driven by a stub in tests without a daemon or an API key. Boxed futures keep
/// it dyn-compatible while allowing `async` implementations.
pub trait WebClient: Send + Sync {
    fn search<'a>(
        &'a self,
        request: SearchRequest,
        cancel: CancellationToken,
    ) -> Pin<Box<dyn Future<Output = Result<SearchResponse, OllamaError>> + Send + 'a>>;

    fn fetch<'a>(
        &'a self,
        request: FetchRequest,
        cancel: CancellationToken,
    ) -> Pin<Box<dyn Future<Output = Result<FetchResponse, OllamaError>> + Send + 'a>>;
}
