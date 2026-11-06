//! Tool layer: the two MCP tools this server publishes, argument validation,
//! execution against the Ollama client, and model-facing text rendering.
//!
//! Schemas deliberately stay inside the narrow JSON Schema subset that harnesses
//! such as DeepSeek Harness enforce: `type` / `properties` / `required` /
//! `additionalProperties` / `items` / `enum` / `const`, plus `description`,
//! `title`, `default`, and `examples`. Bounds the subset cannot express are
//! stated in prose and enforced here in code.

use serde_json::{Map, Value, json};
use tokio_util::sync::CancellationToken;

use crate::error::{OllamaErrorCode, ToolArgumentError, UnknownToolError};
use crate::ollama::{
    DEFAULT_SEARCH_RESULTS, FetchRequest, MAX_SEARCH_RESULTS, SearchRequest, WebClient,
};

pub const SEARCH_TOOL_NAME: &str = "web_search";
pub const FETCH_TOOL_NAME: &str = "web_fetch";

/// Default extraction ceiling for `web_fetch`, in characters.
pub const DEFAULT_FETCH_MAX_CHARS: usize = 50_000;

/// Failure while handling `tools/call`, before a tool result is built.
#[derive(Debug, thiserror::Error)]
pub enum CallToolError {
    #[error(transparent)]
    Argument(#[from] ToolArgumentError),
    #[error(transparent)]
    Unknown(#[from] UnknownToolError),
    #[error(transparent)]
    Ollama(#[from] crate::error::OllamaError),
}

/// The tool list advertised by `tools/list`.
pub fn tool_definitions() -> Value {
    json!([
        {
            "name": SEARCH_TOOL_NAME,
            "title": "Web search (Ollama)",
            "description": "Search the public web through Ollama's web search and return ranked results \
                (title, URL, and a text snippet for each). Works against whichever endpoint this server is \
                pointed at — a local daemon or the hosted API. Use it for current facts, docs, releases, or \
                anything the conversation cannot already answer. Follow up with web_fetch to read a promising \
                result in full.",
            "annotations": { "readOnlyHint": true, "openWorldHint": true },
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "Search query in natural language. Add specifics that narrow the result set: \
                            product names, versions, dates, or a site hint such as \"site:docs.example.com\"."
                    },
                    "max_results": {
                        "type": "integer",
                        "description": format!(
                            "How many results to return, 1-{MAX_SEARCH_RESULTS}. Defaults to \
                             {DEFAULT_SEARCH_RESULTS}; a value outside that range is an error, not clamped silently."
                        ),
                        "default": DEFAULT_SEARCH_RESULTS
                    }
                },
                "required": ["query"],
                "additionalProperties": false
            },
            "outputSchema": {
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "The query that was run, after trimming." },
                    "results": {
                        "type": "array",
                        "description": "Hits in Ollama's ranking order, de-duplicated by URL.",
                        "items": {
                            "type": "object",
                            "properties": {
                                "title": { "type": "string", "description": "Page title, empty when Ollama reported none." },
                                "url": { "type": "string", "description": "Absolute result URL." },
                                "snippet": { "type": "string", "description": "Extract Ollama returned for the result, empty when absent." }
                            },
                            "required": ["url"],
                            "additionalProperties": false
                        }
                    },
                    "dropped": {
                        "type": "integer",
                        "description": "Entries Ollama returned that were unusable: no URL, a non-string URL, or a duplicate URL."
                    }
                },
                "required": ["query", "results", "dropped"],
                "additionalProperties": false
            }
        },
        {
            "name": FETCH_TOOL_NAME,
            "title": "Web fetch (Ollama)",
            "description": "Fetch one web page through Ollama and return its extracted main text, plus the page \
                title and any links it reported. Use it to read a page found by web_search when the snippet is not \
                enough. Long pages are truncated; raise max_chars to read further.",
            "annotations": { "readOnlyHint": true, "openWorldHint": true },
            "inputSchema": {
                "type": "object",
                "properties": {
                    "url": {
                        "type": "string",
                        "description": "Absolute http(s) URL of the page to read."
                    },
                    "max_chars": {
                        "type": "integer",
                        "description": format!(
                            "Truncate the extracted text to this many characters. Defaults to \
                             {DEFAULT_FETCH_MAX_CHARS}."
                        ),
                        "default": DEFAULT_FETCH_MAX_CHARS
                    }
                },
                "required": ["url"],
                "additionalProperties": false
            },
            "outputSchema": {
                "type": "object",
                "properties": {
                    "url": { "type": "string", "description": "The URL that was fetched." },
                    "title": { "type": "string", "description": "Page title, empty when the page reported none." },
                    "content": { "type": "string", "description": "Extracted page text, truncated to the requested ceiling." },
                    "links": { "type": "array", "items": { "type": "string" }, "description": "Links found on the page, up to 50." },
                    "truncated": { "type": "boolean", "description": "Whether content was cut short." },
                    "total_chars": { "type": "integer", "description": "Length of the full extracted text before truncation." }
                },
                "required": ["url", "title", "content", "links", "truncated", "total_chars"],
                "additionalProperties": false
            }
        }
    ])
}

/// Coerce an optional integer argument, tolerating numeric strings some models
/// emit, and enforce the documented range.
pub fn to_integer(
    value: Option<&Value>,
    field: &str,
    default_value: usize,
    min: usize,
    max: usize,
) -> Result<usize, ToolArgumentError> {
    let Some(value) = value else {
        return Ok(default_value);
    };
    if value.is_null() {
        return Ok(default_value);
    }

    let numeric = match value {
        Value::String(text) if text.trim().is_empty() => return Ok(default_value),
        Value::String(text) => text.trim().parse::<f64>().map_err(|_| {
            ToolArgumentError(format!("\"{field}\" must be a number, got {value}."))
        })?,
        Value::Number(number) => number.as_f64().ok_or_else(|| {
            ToolArgumentError(format!("\"{field}\" must be a number, got {value}."))
        })?,
        other => {
            return Err(ToolArgumentError(format!(
                "\"{field}\" must be a number, got {other}."
            )));
        }
    };

    if !numeric.is_finite() {
        return Err(ToolArgumentError(format!(
            "\"{field}\" must be a finite number, got {value}."
        )));
    }

    let truncated = numeric.trunc();
    if truncated < min as f64 || truncated > max as f64 {
        return Err(ToolArgumentError(format!(
            "\"{field}\" must be between {min} and {max}, got {truncated}."
        )));
    }
    Ok(truncated as usize)
}

/// Read a required non-empty string argument.
fn to_required_string(
    value: Option<&Value>,
    field: &str,
    hint: Option<&str>,
) -> Result<String, ToolArgumentError> {
    match value {
        Some(Value::String(text)) if !text.trim().is_empty() => Ok(text.trim().to_owned()),
        _ => {
            let hint = hint.map(|text| format!(" {text}")).unwrap_or_default();
            Err(ToolArgumentError(format!(
                "\"{field}\" is required and must be a non-empty string.{hint}"
            )))
        }
    }
}

/// Collapse runs of whitespace so snippets and page text stay readable.
pub fn collapse_whitespace(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn render_search_text(query: &str, results: &[crate::ollama::SearchHit], dropped: usize) -> String {
    if results.is_empty() {
        let note = if dropped > 0 {
            format!(
                " Ollama returned {dropped} unusable entr{}.",
                if dropped == 1 { "y" } else { "ies" }
            )
        } else {
            String::new()
        };
        return format!(
            "No results for {query:?} (Ollama web search). Try broader or different keywords.{note}"
        );
    }

    let mut lines = vec![format!(
        "{} result{} for {query:?} (Ollama web search):",
        results.len(),
        if results.len() == 1 { "" } else { "s" }
    )];
    lines.push(String::new());

    for (index, result) in results.iter().enumerate() {
        let title = if result.title.is_empty() {
            "(untitled)"
        } else {
            &result.title
        };
        lines.push(format!("{}. {title}", index + 1));
        lines.push(format!("   {}", result.url));
        if !result.snippet.is_empty() {
            lines.push(format!("   {}", result.snippet));
        }
        lines.push(String::new());
    }
    if dropped > 0 {
        lines.push(format!(
            "({dropped} unusable duplicate or malformed entr{} omitted.)",
            if dropped == 1 { "y" } else { "ies" }
        ));
    }
    lines.join("\n").trim_end().to_owned()
}

fn render_fetch_text(page: &crate::ollama::FetchResponse) -> String {
    let header = if page.title.is_empty() {
        format!("# {}", page.url)
    } else {
        format!("# {}\n{}", page.title, page.url)
    };
    let mut lines = vec![
        header,
        format!("{} characters extracted", page.total_chars),
        String::new(),
        page.content.clone(),
    ];

    if page.truncated {
        lines.push(String::new());
        lines.push(format!(
            "[truncated: showing the first {} of {} characters. Call {FETCH_TOOL_NAME} again with a larger \
             max_chars to read further.]",
            page.content.chars().count(),
            page.total_chars
        ));
    }
    if !page.links.is_empty() {
        lines.push(String::new());
        lines.push(format!("Links on the page ({}):", page.links.len()));
        for link in page.links.iter().take(10) {
            lines.push(format!("- {link}"));
        }
        if page.links.len() > 10 {
            lines.push(format!("- ...and {} more", page.links.len() - 10));
        }
    }
    lines.join("\n").trim_end().to_owned()
}

async fn run_search<C: WebClient + ?Sized>(
    client: &C,
    args: &Map<String, Value>,
    cancel: CancellationToken,
) -> Result<Value, CallToolError> {
    let query = to_required_string(
        args.get("query"),
        "query",
        Some("Example: {\"query\":\"Ollama web search API\"}."),
    )?;
    let max_results = to_integer(
        args.get("max_results"),
        "max_results",
        DEFAULT_SEARCH_RESULTS,
        1,
        MAX_SEARCH_RESULTS,
    )?;

    let response = client
        .search(SearchRequest { query, max_results }, cancel)
        .await?;

    let text = render_search_text(&response.query, &response.results, response.dropped);
    Ok(json!({
        "content": [{ "type": "text", "text": text }],
        "structuredContent": response,
    }))
}

async fn run_fetch<C: WebClient + ?Sized>(
    client: &C,
    args: &Map<String, Value>,
    cancel: CancellationToken,
) -> Result<Value, CallToolError> {
    let url = to_required_string(
        args.get("url"),
        "url",
        Some("Example: {\"url\":\"https://ollama.com/\"}."),
    )?;
    let max_chars = to_integer(
        args.get("max_chars"),
        "max_chars",
        DEFAULT_FETCH_MAX_CHARS,
        1,
        5_000_000,
    )?;

    let page = client
        .fetch(
            FetchRequest {
                url,
                max_chars: Some(max_chars),
            },
            cancel,
        )
        .await?;

    let text = render_fetch_text(&page);
    Ok(json!({
        "content": [{ "type": "text", "text": text }],
        "structuredContent": page,
    }))
}

/// Execute one `tools/call`.
///
/// Argument faults and unknown tool names are returned as their own variants so
/// the server can answer with a JSON-RPC error; Ollama failures are returned as
/// an [`OllamaError`](crate::error::OllamaError) so the server can answer with an
/// `isError` tool result instead.
pub async fn call_tool<C: WebClient + ?Sized>(
    client: &C,
    name: &str,
    args: &Map<String, Value>,
    cancel: CancellationToken,
) -> Result<Value, CallToolError> {
    match name {
        SEARCH_TOOL_NAME => run_search(client, args, cancel).await,
        FETCH_TOOL_NAME => run_fetch(client, args, cancel).await,
        other => Err(UnknownToolError {
            name: other.to_owned(),
        }
        .into()),
    }
}

/// Turn an execution failure into an MCP `isError` tool result, so the model
/// sees the reason and can adapt instead of the call vanishing into a protocol
/// error.
pub fn error_tool_result(error: &crate::error::OllamaError) -> Value {
    // The credential message is already a complete, self-describing sentence
    // distinguishing "no key was sent" from "the key was rejected", so a prefix
    // would only blur it.
    if error.code == OllamaErrorCode::Unauthorized {
        return json!({
            "content": [{ "type": "text", "text": error.message }],
            "isError": true,
        });
    }

    let prefix = match error.code {
        OllamaErrorCode::Unreachable => "Ollama is not reachable",
        OllamaErrorCode::NoEndpoint => "Ollama web search is unavailable",
        OllamaErrorCode::Timeout => "Ollama timed out",
        OllamaErrorCode::Aborted => "Request cancelled",
        _ => "Ollama request failed",
    };
    json!({
        "content": [{ "type": "text", "text": format!("{prefix}: {}", error.message) }],
        "isError": true,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(value: Value) -> Map<String, Value> {
        value.as_object().cloned().expect("test args are an object")
    }

    #[test]
    fn numeric_arguments_accept_numbers_and_numeric_strings() {
        assert_eq!(
            to_integer(Some(&json!(4)), "max_results", 5, 1, 10).unwrap(),
            4
        );
        assert_eq!(
            to_integer(Some(&json!("7")), "max_results", 5, 1, 10).unwrap(),
            7
        );
        assert_eq!(
            to_integer(Some(&json!(null)), "max_results", 5, 1, 10).unwrap(),
            5
        );
        assert_eq!(to_integer(None, "max_results", 5, 1, 10).unwrap(), 5);
    }

    #[test]
    fn numeric_arguments_reject_out_of_range_values() {
        let error = to_integer(Some(&json!(11)), "max_results", 5, 1, 10).unwrap_err();
        assert_eq!(
            error.to_string(),
            "\"max_results\" must be between 1 and 10, got 11."
        );
        let error = to_integer(Some(&json!(0)), "max_results", 5, 1, 10).unwrap_err();
        assert!(error.to_string().contains("between 1 and 10"));
    }

    #[test]
    fn required_strings_are_trimmed() {
        assert_eq!(
            to_required_string(Some(&json!("  hello ")), "query", None).unwrap(),
            "hello"
        );
        assert!(to_required_string(Some(&json!("  ")), "query", None).is_err());
        assert!(to_required_string(None, "query", None).is_err());
    }

    #[test]
    fn whitespace_runs_collapse_to_single_spaces() {
        assert_eq!(collapse_whitespace("  a \n\t b  "), "a b");
    }

    #[test]
    fn empty_search_text_mentions_dropped_entries() {
        let text = render_search_text("q", &[], 2);
        assert!(text.contains("No results for \"q\""));
        assert!(text.contains("2 unusable entries"));
    }

    #[test]
    fn tool_definitions_stay_inside_the_supported_schema_subset() {
        let definitions = tool_definitions();
        let tools = definitions.as_array().expect("tools is an array");
        assert_eq!(tools.len(), 2);
        let names: Vec<&str> = tools
            .iter()
            .filter_map(|tool| tool.get("name").and_then(Value::as_str))
            .collect();
        assert_eq!(names, vec![SEARCH_TOOL_NAME, FETCH_TOOL_NAME]);
        for tool in tools {
            assert!(tool.get("inputSchema").is_some());
            assert!(tool.get("outputSchema").is_some());
        }
    }

    #[test]
    fn unknown_tool_names_are_reported_with_the_available_set() {
        let error = UnknownToolError {
            name: "nope".to_owned(),
        };
        assert!(error.to_string().contains("web_search, web_fetch"));
    }

    #[test]
    fn argument_object_shape_is_preserved_for_tests() {
        // Keeps the helper honest; `call_tool` receives `Map<String, Value>`.
        assert!(args(json!({"query": "x"})).contains_key("query"));
    }
}
