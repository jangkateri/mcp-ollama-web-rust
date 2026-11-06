//! Host and credential normalization for Ollama targets.
//!
//! Ollama serves web grounding in two places with the same request shapes:
//!
//! - a local daemon at `http://127.0.0.1:11434`, no credential; or
//! - the hosted API at `https://ollama.com`, sent as `Authorization: Bearer`.

use url::Url;

use crate::error::{OllamaError, OllamaErrorCode};

/// Default daemon address, matching `OLLAMA_HOST`'s own default.
pub const DEFAULT_HOST: &str = "http://127.0.0.1:11434";

/// Ollama's hosted web search and fetch API.
pub const CLOUD_HOST: &str = "https://ollama.com";

/// Where an API key is created, quoted in errors so a 401 is actionable.
pub const CLOUD_KEY_URL: &str = "https://ollama.com/settings/keys";

/// Whether a string begins with an `http(s)`-style scheme.
fn has_scheme(value: &str) -> bool {
    let Some(index) = value.find("://") else {
        return false;
    };
    let scheme = &value[..index];
    let mut characters = scheme.chars();
    matches!(characters.next(), Some(first) if first.is_ascii_alphabetic())
        && characters.all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '+' | '-' | '.')
        })
}

/// Validate and normalize an Ollama host.
///
/// Forgiving on purpose, because `OLLAMA_HOST` is widely set to a bare bind
/// address by the Ollama installer:
///
/// - `11434`, `host:11434`, and `:11434` gain an `http://` scheme (`:port`
///   maps to loopback);
/// - a wildcard bind address (`0.0.0.0`, `::`) maps to loopback, since a client
///   can never dial a wildcard;
/// - trailing slashes are dropped.
pub fn normalize_host(host: Option<&str>) -> Result<String, OllamaError> {
    let raw = host.map(str::trim).unwrap_or_default();
    let candidate = if raw.is_empty() { DEFAULT_HOST } else { raw };

    let candidate = if has_scheme(candidate) {
        candidate.to_owned()
    } else if let Some(port) = candidate.strip_prefix(':') {
        format!("http://127.0.0.1:{port}")
    } else {
        format!("http://{candidate}")
    };

    // Wildcard bind addresses cannot be dialled. Map them before parsing so
    // IPv6 keeps the brackets a URL requires.
    let candidate = candidate
        .replace("://0.0.0.0", "://127.0.0.1")
        .replace("://[::]", "://[::1]")
        .replace("://::", "://[::1]");

    let url = Url::parse(&candidate).map_err(|_| {
        OllamaError::new(
            format!("Ollama host is not a valid URL: {candidate:?}"),
            OllamaErrorCode::InvalidHost,
        )
    })?;

    if url.scheme() != "http" && url.scheme() != "https" {
        return Err(OllamaError::new(
            format!("Ollama host must use http or https, got {:?}", url.scheme()),
            OllamaErrorCode::InvalidHost,
        ));
    }

    let mut normalized = url.to_string();
    while normalized.ends_with('/') {
        normalized.pop();
    }
    Ok(normalized)
}

/// Whether a normalized host is Ollama's hosted API rather than a local daemon.
pub fn is_ollama_cloud(host: &str) -> bool {
    Url::parse(host)
        .ok()
        .and_then(|url| url.host_str().map(str::to_owned))
        .is_some_and(|hostname| hostname == "ollama.com" || hostname.ends_with(".ollama.com"))
}

/// Normalize an API key: blank and placeholder spellings mean "no credential".
///
/// A harness that resolves `OLLAMA_API_KEY: !!js process.env.OLLAMA_API_KEY`
/// with the variable unset may stringify the result, which would otherwise be
/// sent as `Bearer undefined`. No real Ollama key is spelled that way.
pub fn normalize_api_key(api_key: Option<&str>) -> Option<String> {
    let trimmed = api_key?.trim();
    if trimmed.is_empty()
        || trimmed.eq_ignore_ascii_case("undefined")
        || trimmed.eq_ignore_ascii_case("null")
    {
        None
    } else {
        Some(trimmed.to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_and_bare_hosts_are_accepted() {
        assert_eq!(normalize_host(None).unwrap(), DEFAULT_HOST);
        assert_eq!(
            normalize_host(Some("127.0.0.1:11434")).unwrap(),
            "http://127.0.0.1:11434"
        );
        assert_eq!(
            normalize_host(Some(":11434")).unwrap(),
            "http://127.0.0.1:11434"
        );
        assert_eq!(
            normalize_host(Some("localhost:11434")).unwrap(),
            "http://localhost:11434"
        );
        assert_eq!(
            normalize_host(Some("http://127.0.0.1:11434///")).unwrap(),
            "http://127.0.0.1:11434"
        );
    }

    #[test]
    fn wildcard_bind_addresses_map_to_loopback() {
        assert_eq!(
            normalize_host(Some("0.0.0.0:11434")).unwrap(),
            "http://127.0.0.1:11434"
        );
        assert_eq!(
            normalize_host(Some("http://[::]:11434")).unwrap(),
            "http://[::1]:11434"
        );
    }

    #[test]
    fn unsupported_schemes_are_rejected() {
        let error = normalize_host(Some("ftp://example.com")).unwrap_err();
        assert_eq!(error.code, OllamaErrorCode::InvalidHost);
    }

    #[test]
    fn cloud_hosts_are_detected_by_hostname() {
        assert!(is_ollama_cloud("https://ollama.com"));
        assert!(is_ollama_cloud("https://api.ollama.com"));
        assert!(!is_ollama_cloud("http://127.0.0.1:11434"));
        assert!(!is_ollama_cloud("https://notollama.com"));
    }

    #[test]
    fn placeholder_keys_mean_no_credential() {
        assert_eq!(normalize_api_key(None), None);
        assert_eq!(normalize_api_key(Some("  ")), None);
        assert_eq!(normalize_api_key(Some("undefined")), None);
        assert_eq!(normalize_api_key(Some("NULL")), None);
        assert_eq!(
            normalize_api_key(Some("  abc123  ")).as_deref(),
            Some("abc123")
        );
    }
}
