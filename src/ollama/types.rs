//! Domain types exchanged between the tool layer and the Ollama client.

use serde::Serialize;

/// One ranked web-search hit, already normalized for display.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SearchHit {
    /// Page title, empty when Ollama reported none.
    pub title: String,
    /// Absolute result URL; always non-empty after normalization.
    pub url: String,
    /// Extract Ollama returned, empty when absent.
    pub snippet: String,
}

/// Normalized answer to one web search.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SearchResponse {
    /// The query that was run, after trimming.
    pub query: String,
    /// Hits in Ollama's ranking order, de-duplicated by URL.
    pub results: Vec<SearchHit>,
    /// Entries Ollama returned that had no URL, a non-string URL, or a duplicate URL.
    pub dropped: usize,
}

/// Normalized answer to one page fetch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct FetchResponse {
    /// The URL that was fetched.
    pub url: String,
    /// Page title, empty when the page reported none.
    pub title: String,
    /// Extracted page text, truncated to the requested ceiling.
    pub content: String,
    /// Links found on the page, up to 50.
    pub links: Vec<String>,
    /// Whether `content` was cut short.
    pub truncated: bool,
    /// Length of the full extracted text before truncation, in characters.
    pub total_chars: usize,
}

/// Request passed to [`WebClient::search`](super::WebClient::search).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchRequest {
    pub query: String,
    pub max_results: usize,
}

/// Request passed to [`WebClient::fetch`](super::WebClient::fetch).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FetchRequest {
    pub url: String,
    pub max_chars: Option<usize>,
}
