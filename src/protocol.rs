//! MCP wire helpers: JSON-RPC 2.0 message classification, response builders,
//! protocol-version negotiation, and newline-delimited framing.
//!
//! The MCP stdio transport carries one JSON-RPC 2.0 message per line, UTF-8,
//! with no embedded newlines. Nothing else may be written to stdout;
//! diagnostics belong on stderr.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// The only JSON-RPC version this server speaks.
pub const JSONRPC_VERSION: &str = "2.0";

/// Standard JSON-RPC 2.0 error codes.
pub mod error_code {
    pub const PARSE_ERROR: i64 = -32700;
    pub const INVALID_REQUEST: i64 = -32600;
    pub const METHOD_NOT_FOUND: i64 = -32601;
    pub const INVALID_PARAMS: i64 = -32602;
    pub const INTERNAL_ERROR: i64 = -32603;
}

/// Protocol revisions this server implements, newest first.
///
/// These are all legacy-era revisions (pre-2026-07-28): they handshake with
/// `initialize`. The 2026 era replaces that with `server/discover`; this server
/// deliberately answers that probe with Method Not Found so newer clients fall
/// back to the legacy handshake.
pub const SUPPORTED_PROTOCOL_VERSIONS: &[&str] =
    &["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];

/// The revision offered when a client asks for something this server does not implement.
pub const PREFERRED_PROTOCOL_VERSION: &str = SUPPORTED_PROTOCOL_VERSIONS[0];

/// A JSON-RPC request identifier: a string or a signed integer.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(untagged)]
pub enum RequestId {
    Number(i64),
    String(String),
}

impl std::fmt::Display for RequestId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Number(value) => write!(formatter, "{value}"),
            Self::String(value) => formatter.write_str(value),
        }
    }
}

/// The shape of one decoded JSON-RPC message.
#[derive(Debug, Clone, PartialEq)]
pub enum Classified {
    Request {
        id: RequestId,
        method: String,
        params: Value,
    },
    Notification {
        method: String,
        params: Value,
    },
    Response {
        id: Option<RequestId>,
    },
    Invalid {
        id: Option<RequestId>,
        reason: String,
    },
}

/// Classify a decoded JSON-RPC message without dispatching it.
pub fn classify_message(message: &Value) -> Classified {
    let Some(object) = message.as_object() else {
        return Classified::Invalid {
            id: None,
            reason: "message must be a JSON object".to_owned(),
        };
    };

    if let Some(version) = object.get("jsonrpc")
        && version != JSONRPC_VERSION
    {
        return Classified::Invalid {
            id: None,
            reason: format!("unsupported jsonrpc version {version}"),
        };
    }

    let id = match object.get("id") {
        None | Some(Value::Null) => None,
        Some(Value::String(value)) => Some(RequestId::String(value.clone())),
        Some(Value::Number(value)) => match value.as_i64() {
            Some(number) => Some(RequestId::Number(number)),
            None => {
                return Classified::Invalid {
                    id: None,
                    reason: "id must be a string or an integer".to_owned(),
                };
            }
        },
        Some(other) => {
            return Classified::Invalid {
                id: None,
                reason: format!("id must be a string or an integer, got {other}"),
            };
        }
    };

    let params = match object.get("params") {
        None | Some(Value::Null) => json!({}),
        Some(value) if value.is_object() => value.clone(),
        Some(_) => {
            return Classified::Invalid {
                id: id.clone(),
                reason: "params must be a JSON object".to_owned(),
            };
        }
    };

    let method = match object.get("method") {
        Some(Value::String(method)) if !method.is_empty() => method.clone(),
        Some(_) => {
            return Classified::Invalid {
                id: id.clone(),
                reason: "method must be a non-empty string".to_owned(),
            };
        }
        None => {
            // A message with an id but no method is a response to a
            // server-initiated request. This server never sends requests, but
            // classifying it correctly lets the caller drop it quietly.
            if id.is_some() && (object.contains_key("result") || object.contains_key("error")) {
                return Classified::Response { id };
            }
            return Classified::Invalid {
                id,
                reason: "method must be a non-empty string".to_owned(),
            };
        }
    };

    match id {
        Some(id) => Classified::Request { id, method, params },
        None => Classified::Notification { method, params },
    }
}

/// Pick the protocol revision for an `initialize` response.
///
/// A client sends the newest revision it supports and then requires the
/// server's answer to be one of its own supported revisions, so echoing a
/// revision both sides implement is the compatible choice; anything unknown
/// falls back to this server's newest revision.
pub fn negotiate_protocol_version(requested: Option<&str>) -> &'static str {
    match requested {
        Some(version) if SUPPORTED_PROTOCOL_VERSIONS.contains(&version) => {
            SUPPORTED_PROTOCOL_VERSIONS
                .iter()
                .find(|candidate| **candidate == version)
                .copied()
                .unwrap_or(PREFERRED_PROTOCOL_VERSION)
        }
        _ => PREFERRED_PROTOCOL_VERSION,
    }
}

/// Build a JSON-RPC success response.
pub fn result_response(id: Option<RequestId>, result: Value) -> Value {
    json!({
        "jsonrpc": JSONRPC_VERSION,
        "id": id,
        "result": result,
    })
}

/// Build a JSON-RPC error response.
pub fn error_response(id: Option<RequestId>, code: i64, message: impl Into<String>) -> Value {
    json!({
        "jsonrpc": JSONRPC_VERSION,
        "id": id,
        "error": {
            "code": code,
            "message": message.into(),
        },
    })
}

/// Build a JSON-RPC error response carrying structured `data`.
pub fn error_response_with_data(
    id: Option<RequestId>,
    code: i64,
    message: impl Into<String>,
    data: Value,
) -> Value {
    json!({
        "jsonrpc": JSONRPC_VERSION,
        "id": id,
        "error": {
            "code": code,
            "message": message.into(),
            "data": data,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefers_the_requested_supported_revision() {
        assert_eq!(negotiate_protocol_version(Some("2025-06-18")), "2025-06-18");
        assert_eq!(negotiate_protocol_version(Some("1999-01-01")), "2025-11-25");
        assert_eq!(negotiate_protocol_version(None), "2025-11-25");
    }

    #[test]
    fn classifies_requests_notifications_and_responses() {
        let request = json!({"jsonrpc":"2.0","id":1,"method":"ping","params":{}});
        assert_eq!(
            classify_message(&request),
            Classified::Request {
                id: RequestId::Number(1),
                method: "ping".to_owned(),
                params: json!({}),
            }
        );

        let notification = json!({"jsonrpc":"2.0","method":"notifications/initialized"});
        assert_eq!(
            classify_message(&notification),
            Classified::Notification {
                method: "notifications/initialized".to_owned(),
                params: json!({}),
            }
        );

        let response = json!({"jsonrpc":"2.0","id":"abc","result":{}});
        assert_eq!(
            classify_message(&response),
            Classified::Response {
                id: Some(RequestId::String("abc".to_owned())),
            }
        );
    }

    #[test]
    fn rejects_malformed_messages() {
        let array = json!([]);
        assert!(matches!(
            classify_message(&array),
            Classified::Invalid { .. }
        ));

        let bad_params = json!({"jsonrpc":"2.0","id":2,"method":"ping","params":[]});
        assert_eq!(
            classify_message(&bad_params),
            Classified::Invalid {
                id: Some(RequestId::Number(2)),
                reason: "params must be a JSON object".to_owned(),
            }
        );

        let bad_version = json!({"jsonrpc":"1.0","id":3,"method":"ping"});
        assert!(matches!(
            classify_message(&bad_version),
            Classified::Invalid { .. }
        ));
    }

    #[test]
    fn builds_responses_with_null_id_when_unknown() {
        let response = error_response(None, error_code::PARSE_ERROR, "Parse error");
        assert_eq!(response["jsonrpc"], "2.0");
        assert!(response["id"].is_null());
        assert_eq!(response["error"]["code"], -32700);
    }
}
