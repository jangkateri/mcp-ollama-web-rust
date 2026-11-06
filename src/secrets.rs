//! Read an API key from the environment or a file, so a secret can live in a
//! store the operating system or a harness owns instead of on a command line.
//!
//! Two file shapes are accepted:
//!
//! 1. A credentials document — DeepSeek Harness keeps one at
//!    `<harness home>/.credentials.yaml`, where `refs:` maps environment-variable
//!    names to values:
//!
//!    ```yaml
//!    version: 1
//!    refs:
//!      OLLAMA_API_KEY: b323...
//!    ```
//!
//! 2. A bare secret file — the file's trimmed text is the key, the shape used by
//!    Docker secrets, systemd credentials, and Kubernetes secret mounts.
//!
//! Only the requested reference is extracted; the rest of the document is never
//! read back, logged, or echoed into an error message.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::error::ApiKeyFileError;

/// The reference looked up in a credentials document by default.
pub const API_KEY_REF: &str = "OLLAMA_API_KEY";

/// Names the credentials document to read. Survives DeepSeek Harness's env
/// scrub, unlike `*_KEY` names.
pub const CREDENTIALS_FILE_ENV: &str = "OLLAMA_CREDENTIALS_FILE";

/// File name a harness credential store uses, relative to its home directory.
pub const CREDENTIALS_FILE_NAME: &str = ".credentials.yaml";

/// A credentials file found by discovery, or named explicitly.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CredentialsLocation {
    pub path: PathBuf,
    /// `true` when the user named the file; a missing or unusable file is then an error.
    pub explicit: bool,
}

impl CredentialsLocation {
    pub fn none() -> Self {
        Self {
            path: PathBuf::new(),
            explicit: false,
        }
    }

    pub fn found(&self) -> bool {
        !self.path.as_os_str().is_empty()
    }
}

/// Locate the credentials document to read a key from.
///
/// `OLLAMA_CREDENTIALS_FILE` names it explicitly and is trusted even when the
/// file is missing, so a wrong path is reported rather than ignored. Otherwise
/// the conventional harness-home locations are probed, and none of them
/// existing is normal: a local daemon needs no key at all.
pub fn find_credentials_file(env: &HashMap<String, String>) -> CredentialsLocation {
    let explicit = env
        .get(CREDENTIALS_FILE_ENV)
        .map(|value| value.trim())
        .unwrap_or_default();
    if !explicit.is_empty() {
        return CredentialsLocation {
            path: PathBuf::from(explicit),
            explicit: true,
        };
    }

    let mut candidates = Vec::new();
    // DSH_HOME survives only in a shell that exported it; DeepSeek Harness
    // strips DSH_* from a spawned child, which is why the explicit variable exists.
    let harness_home = env
        .get("DSH_HOME")
        .map(|value| value.trim())
        .unwrap_or_default();
    if !harness_home.is_empty() {
        candidates.push(Path::new(harness_home).join(CREDENTIALS_FILE_NAME));
    }

    let home = env
        .get("USERPROFILE")
        .or_else(|| env.get("HOME"))
        .map(|value| value.trim())
        .unwrap_or_default();
    if !home.is_empty() {
        candidates.push(Path::new(home).join(".dsh").join(CREDENTIALS_FILE_NAME));
    }

    for candidate in candidates {
        if candidate.is_file() {
            return CredentialsLocation {
                path: candidate,
                explicit: false,
            };
        }
    }
    CredentialsLocation::none()
}

/// Strip one layer of matching quotes from a scalar.
fn unquote(value: &str) -> String {
    let trimmed = value.trim();
    let Some(first) = trimmed.chars().next() else {
        return String::new();
    };
    if (first == '"' || first == '\'') && trimmed.chars().count() >= 2 && trimmed.ends_with(first) {
        let mut characters = trimmed.chars();
        characters.next();
        characters.next_back();
        let inner: String = characters.collect();
        if first == '"' {
            return inner.replace("\\\"", "\"").replace("\\\\", "\\");
        }
        return inner.replace("''", "'");
    }
    trimmed.to_owned()
}

/// Walk `text` and split on `separator`, ignoring separators inside quotes or
/// brackets. Used for flow-mapping entries.
fn split_respecting_quotes(text: &str, separator: char) -> Vec<String> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    let mut depth = 0i32;
    let mut previous = '\0';

    for character in text.chars() {
        if let Some(active) = quote {
            current.push(character);
            if character == active && previous != '\\' {
                quote = None;
            }
            previous = character;
            continue;
        }
        if character == '"' || character == '\'' {
            quote = Some(character);
            current.push(character);
        } else {
            if character == '{' || character == '[' {
                depth += 1;
            }
            if character == '}' || character == ']' {
                depth -= 1;
            }
            if depth == 0 && character == separator {
                parts.push(std::mem::take(&mut current));
            } else {
                current.push(character);
            }
        }
        previous = character;
    }
    parts.push(current);
    parts
}

/// True when the collected section text starts a flow mapping that has not closed.
fn flow_is_open(text: &str) -> bool {
    let mut quote: Option<char> = None;
    let mut depth = 0i32;
    let mut previous = '\0';
    for character in text.chars() {
        if let Some(active) = quote {
            if character == active && previous != '\\' {
                quote = None;
            }
        } else if character == '"' || character == '\'' {
            quote = Some(character);
        } else if character == '{' {
            depth += 1;
        } else if character == '}' {
            depth -= 1;
        }
        previous = character;
    }
    depth > 0
}

/// Collect the raw text of the `refs:` section, in block or flow style.
fn refs_section_text(text: &str) -> Option<String> {
    let lines: Vec<&str> = text.lines().collect();
    let header_index = lines.iter().position(|line| {
        let trimmed = line.trim_start_matches([' ', '\t']);
        trimmed.starts_with("refs") && trimmed[4..].trim_start().starts_with(':')
    })?;

    let header = lines[header_index];
    let header_indent = header.len() - header.trim_start_matches([' ', '\t']).len();
    let inline = header
        .split_once(':')
        .map(|(_, value)| value.trim())
        .unwrap_or_default();

    let mut collected: Vec<String> = Vec::new();
    if !inline.is_empty() {
        collected.push(inline.to_owned());
    }

    for line in &lines[header_index + 1..] {
        if line.trim().is_empty() {
            collected.push((*line).to_owned());
            continue;
        }
        let indent = line.len() - line.trim_start_matches([' ', '\t']).len();
        if indent <= header_indent && !flow_is_open(&collected.join("\n")) {
            break;
        }
        collected.push((*line).to_owned());
    }

    Some(
        collected
            .join("\n")
            .trim_end_matches([' ', '\t', '\r', '\n'])
            .to_owned(),
    )
}

/// A `|` or `>` block scalar, optionally with a chomping indicator: `|`, `>-`,
/// `|+2`, and so on.
fn is_block_scalar(raw: &str) -> bool {
    let mut characters = raw.chars();
    match characters.next() {
        Some('|') | Some('>') => {}
        _ => return false,
    }
    let rest: String = characters.collect();
    let rest = rest
        .strip_prefix(|c: char| c == '+' || c == '-')
        .unwrap_or(&rest);
    rest.chars().all(|character| character.is_ascii_digit())
}

/// Extract one reference from the `refs:` section of a credentials document,
/// in block or flow style.
fn extract_ref(text: &str, reference: &str) -> Result<Option<String>, ApiKeyFileError> {
    let Some(section) = refs_section_text(text) else {
        return Ok(None);
    };
    if section.is_empty() {
        return Ok(None);
    }

    let mut raw = None;
    if let Some(flow) = section.strip_prefix('{') {
        let inner = flow.strip_suffix('}').unwrap_or(flow);
        for entry in split_respecting_quotes(inner, ',') {
            let Some((name, value)) = entry.split_once(':') else {
                continue;
            };
            if unquote(name) == reference {
                raw = Some(value.trim().to_owned());
                break;
            }
        }
    } else {
        for line in section.lines() {
            let Some((name, value)) = block_entry(line) else {
                continue;
            };
            if name == reference {
                raw = Some(value);
                break;
            }
        }
    }

    let Some(raw) = raw else {
        return Ok(None);
    };

    // `|`, `>`, `|-`, `>+`, `|2` ... continue on following lines, which this
    // reader does not follow; say so rather than sending "|" as a key.
    let block_scalar = is_block_scalar(&raw);
    let value = if block_scalar {
        String::new()
    } else {
        unquote(&raw)
    };
    if value.is_empty() {
        return Err(ApiKeyFileError(format!(
            "{reference} in the refs section has no single-line value{}. Store the key on one line, \
             or remove the entry.",
            if block_scalar {
                " (it uses a block scalar)"
            } else {
                ""
            }
        )));
    }
    Ok(Some(value))
}

/// Parse one indented `NAME: value` line from a block-style `refs:` section.
fn block_entry(line: &str) -> Option<(String, String)> {
    let trimmed = line.trim_start_matches([' ', '\t']);
    if trimmed.len() == line.len() {
        // Entries must be indented under `refs:`.
        return None;
    }
    let (name, value) = trimmed.split_once(':')?;
    let name = name.trim();
    if name.is_empty() || name.contains('#') {
        return None;
    }
    Some((unquote(name), value.trim().to_owned()))
}

/// Whether text carries a top-level `version:`, `refs:`, or `records:` key.
fn is_credentials_document(text: &str) -> bool {
    text.lines().any(|line| {
        ["version", "refs", "records"]
            .iter()
            .any(|key| line.starts_with(key) && line[key.len()..].trim_start().starts_with(':'))
    })
}

/// Read an API key from `path`.
///
/// `required` is `true` for a file the user named: a missing entry, an unusable
/// entry, or an unreadable file is an error. For a merely discovered file,
/// only a credentials document holding the reference yields a key; anything
/// else quietly yields none, so probing a location can never fail startup.
pub fn read_api_key_file(
    path: &Path,
    reference: &str,
    required: bool,
) -> Result<Option<String>, ApiKeyFileError> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) => {
            if !required {
                return Ok(None);
            }
            return Err(ApiKeyFileError(format!(
                "Cannot read the key file {}: {error}",
                path.display()
            )));
        }
    };

    if text.trim().is_empty() {
        if !required {
            return Ok(None);
        }
        return Err(ApiKeyFileError(format!(
            "The key file {} is empty.",
            path.display()
        )));
    }

    if is_credentials_document(&text) {
        return match extract_ref(&text, reference)? {
            Some(value) => Ok(Some(value)),
            None if !required => Ok(None),
            None => Err(ApiKeyFileError(format!(
                "The key file {} is a credentials document with no \"{reference}\" entry in its refs \
                 section. Add \"{reference}: <value>\" under refs:, or point {CREDENTIALS_FILE_ENV} at a \
                 file that contains only the key.",
                path.display()
            ))),
        };
    }

    // A bare secret file: its whole trimmed text is the key. Only a file the
    // user named is read this way; a discovered path must look like a
    // credentials document, so an unrelated file is never sent as a token.
    Ok(required.then(|| unquote(&text)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn temp_file(name: &str, contents: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "mcp-ollama-web-rust-{name}-{}.tmp",
            std::process::id()
        ));
        let mut file = std::fs::File::create(&path).unwrap();
        file.write_all(contents.as_bytes()).unwrap();
        path
    }

    #[test]
    fn reads_block_style_refs() {
        let path = temp_file(
            "block",
            "version: 1\nrefs:\n  OLLAMA_API_KEY: abc123\n  OTHER: nope\n",
        );
        let key = read_api_key_file(&path, API_KEY_REF, true).unwrap();
        assert_eq!(key.as_deref(), Some("abc123"));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn reads_flow_style_refs() {
        let path = temp_file("flow", "refs: {OLLAMA_API_KEY: 'quoted-key', OTHER: x}\n");
        let key = read_api_key_file(&path, API_KEY_REF, true).unwrap();
        assert_eq!(key.as_deref(), Some("quoted-key"));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn reads_flow_style_spread_over_lines() {
        let path = temp_file(
            "flow-multiline",
            "version: 1\nrefs: {\n  OTHER: x,\n  OLLAMA_API_KEY: abc\n}\nnext: value\n",
        );
        let key = read_api_key_file(&path, API_KEY_REF, true).unwrap();
        assert_eq!(key.as_deref(), Some("abc"));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn reads_a_bare_secret_file_when_named_explicitly() {
        let path = temp_file("bare", "  \"secret-value\"  \n");
        let key = read_api_key_file(&path, API_KEY_REF, true).unwrap();
        assert_eq!(key.as_deref(), Some("secret-value"));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn a_discovered_non_document_yields_nothing() {
        let path = temp_file("discovered", "just an unrelated file\n");
        let key = read_api_key_file(&path, API_KEY_REF, false).unwrap();
        assert_eq!(key, None);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn missing_reference_error_never_mentions_another_value() {
        let path = temp_file("missing", "refs:\n  OTHER_KEY: super-secret\n");
        let error = read_api_key_file(&path, API_KEY_REF, true).unwrap_err();
        assert!(error.to_string().contains("no \"OLLAMA_API_KEY\" entry"));
        assert!(!error.to_string().contains("super-secret"));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn block_scalars_are_reported_instead_of_sent_verbatim() {
        let path = temp_file("scalar", "refs:\n  OLLAMA_API_KEY: |\n    abc\n");
        let error = read_api_key_file(&path, API_KEY_REF, true).unwrap_err();
        assert!(error.to_string().contains("block scalar"));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn explicit_paths_are_trusted_even_when_absent() {
        let mut env = HashMap::new();
        env.insert(
            CREDENTIALS_FILE_ENV.to_owned(),
            "C:/definitely/not/here/.credentials.yaml".to_owned(),
        );
        let location = find_credentials_file(&env);
        assert!(location.explicit);
        assert!(location.found());
    }

    #[test]
    fn committed_credentials_example_defines_the_document_shape() {
        let text = include_str!("../credentials.example");
        assert!(is_credentials_document(text));
        assert_eq!(
            extract_ref(text, API_KEY_REF).unwrap().as_deref(),
            Some("replace-with-your-key"),
            "the committed template must stay a placeholder, never a key"
        );
        assert_eq!(extract_ref(text, "SOME_OTHER_KEY").unwrap(), None);
    }
}
