//! Command-line flags and environment configuration.
//!
//! Configuration comes from flags first, then environment variables, then
//! built-in defaults. There is deliberately no key flag: the environment and
//! the credential store are the only two places a secret comes from, so it
//! never reaches a process list, a shell history, or a client config file.

use std::collections::HashMap;
use std::path::PathBuf;

use crate::error::ConfigError;
use crate::ollama::{DEFAULT_HOST, normalize_api_key, normalize_host};
use crate::secrets::{API_KEY_REF, CREDENTIALS_FILE_ENV, find_credentials_file, read_api_key_file};
use crate::server::{SERVER_NAME, SERVER_VERSION};

/// Default per-request ceiling; comfortably under a typical 60 s client tool timeout.
pub const DEFAULT_TIMEOUT_MS: u64 = 55_000;

/// Where the effective credential came from, for a log line that never carries the value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeySource {
    None,
    Environment,
    File(PathBuf),
}

impl KeySource {
    /// A log-safe label.
    pub fn label(&self) -> String {
        match self {
            Self::None => "none".to_owned(),
            Self::Environment => "environment".to_owned(),
            Self::File(path) => format!("file {}", path.display()),
        }
    }
}

/// Parsed flags before validation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Options {
    pub help: bool,
    pub version: bool,
    pub verbose: bool,
    pub host: String,
    pub timeout_ms: u64,
    pub log_file: String,
    pub errors: Vec<String>,
}

/// Fully resolved, validated configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    /// Normalized Ollama base URL.
    pub host: String,
    /// Resolved credential. Never logged.
    pub api_key: Option<String>,
    /// Where the credential came from, for diagnostics.
    pub key_source: KeySource,
    pub timeout_ms: u64,
    /// Empty means "stderr only".
    pub log_file: String,
    pub verbose: bool,
}

impl Config {
    pub fn log_file_path(&self) -> Option<&str> {
        (!self.log_file.is_empty()).then_some(self.log_file.as_str())
    }
}

/// `--help` text, printed to stdout before any MCP client is attached.
pub fn usage() -> String {
    format!(
        "{SERVER_NAME} {SERVER_VERSION} — MCP server for Ollama web search and web fetch

Usage:
  {SERVER_NAME} [options]

  OLLAMA_HOST is the whole target selection: point it at a local daemon
  ({DEFAULT_HOST}, the default) or at https://ollama.com. The key is just a
  header and is used when one is available; a daemon ignores it.

Options:
  --host <url>         Ollama base URL (default: {DEFAULT_HOST}, env OLLAMA_HOST)
  --timeout-ms <n>     Per-request ceiling in milliseconds (default: {DEFAULT_TIMEOUT_MS},
                       env MCP_OLLAMA_TIMEOUT_MS; 0 disables the ceiling)
  --log-file <path>    Append diagnostics to this file as well as stderr
                       (env MCP_OLLAMA_LOG_FILE); useful because MCP clients
                       usually hide a stdio server's stderr
  --verbose            Log request handling and timing, not just warnings
  -h, --help           Print this help and exit
  --version            Print the version and exit

There is deliberately no key flag: environment and store are the two places a
credential comes from, and neither puts it on a command line.

Credential lookup, first hit wins:
  1. OLLAMA_API_KEY in the environment
  2. a credentials document holding {API_KEY_REF} under its refs: section,
     located by {CREDENTIALS_FILE_ENV}, or discovered at
     $DSH_HOME/.credentials.yaml or ~/.dsh/.credentials.yaml

Tools published over stdio:
  web_search(query, max_results?)    Ollama web search, 1-10 ranked results
  web_fetch(url, max_chars?)         Extracted page text for one URL"
    )
}

/// Parse command-line arguments.
pub fn parse_args(args: &[String], env: &HashMap<String, String>) -> Options {
    let mut options = Options {
        help: false,
        version: false,
        verbose: false,
        host: env
            .get("OLLAMA_HOST")
            .cloned()
            .unwrap_or_else(|| DEFAULT_HOST.to_owned()),
        timeout_ms: DEFAULT_TIMEOUT_MS,
        log_file: env
            .get("MCP_OLLAMA_LOG_FILE")
            .map(|value| value.trim().to_owned())
            .unwrap_or_default(),
        errors: Vec::new(),
    };

    if let Some(value) = env.get("MCP_OLLAMA_TIMEOUT_MS") {
        let value = value.trim();
        if !value.is_empty() {
            match value.parse::<u64>() {
                Ok(parsed) => options.timeout_ms = parsed,
                Err(_) => options.errors.push(format!(
                    "MCP_OLLAMA_TIMEOUT_MS must be a non-negative number, got {value:?}"
                )),
            }
        }
    }

    let mut index = 0;
    while index < args.len() {
        let argument = args[index].as_str();
        match argument {
            "-h" | "--help" => options.help = true,
            "--version" => options.version = true,
            "--verbose" => options.verbose = true,
            "--host" => match args.get(index + 1) {
                Some(value) => {
                    options.host = value.clone();
                    index += 1;
                }
                None => options.errors.push("--host requires a value".to_owned()),
            },
            "--timeout-ms" => match args.get(index + 1) {
                Some(value) => {
                    // Consume the value even when invalid, so the bad value is
                    // reported once rather than again as an unknown argument.
                    index += 1;
                    match value.parse::<u64>() {
                        Ok(parsed) => options.timeout_ms = parsed,
                        Err(_) => options.errors.push(format!(
                            "--timeout-ms must be a non-negative number, got {value:?}"
                        )),
                    }
                }
                None => options
                    .errors
                    .push("--timeout-ms requires a value".to_owned()),
            },
            "--log-file" => match args.get(index + 1) {
                Some(value) => {
                    options.log_file = value.clone();
                    index += 1;
                }
                None => options
                    .errors
                    .push("--log-file requires a value".to_owned()),
            },
            other => options.errors.push(format!("unknown argument {other:?}")),
        }
        index += 1;
    }

    options
}

/// Normalize and validate the parsed options, resolving the credential.
pub fn resolve_config(
    options: &Options,
    env: &HashMap<String, String>,
) -> Result<Config, ConfigError> {
    let host = normalize_host(Some(&options.host)).map_err(|error| ConfigError(error.message))?;

    let mut api_key = normalize_api_key(env.get("OLLAMA_API_KEY").map(String::as_str));
    let mut key_source = if api_key.is_some() {
        KeySource::Environment
    } else {
        KeySource::None
    };

    // The environment beats the store, and having neither is normal — a local
    // daemon needs no credential. A file the user named must be usable; a
    // merely discovered one may simply not hold this key.
    if api_key.is_none() {
        let location = find_credentials_file(env);
        if location.found() {
            match read_api_key_file(&location.path, API_KEY_REF, location.explicit) {
                Ok(Some(value)) => {
                    api_key = Some(value);
                    key_source = KeySource::File(location.path);
                }
                Ok(None) => {}
                Err(error) => return Err(ConfigError(error.to_string())),
            }
        }
    }

    Ok(Config {
        host,
        api_key,
        key_source,
        timeout_ms: options.timeout_ms,
        log_file: options.log_file.clone(),
        verbose: options.verbose,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::secrets::CREDENTIALS_FILE_ENV;

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_owned()).collect()
    }

    #[test]
    fn defaults_come_from_the_environment() {
        let mut env = HashMap::new();
        env.insert("OLLAMA_HOST".to_owned(), "127.0.0.1:1234".to_owned());
        env.insert("MCP_OLLAMA_TIMEOUT_MS".to_owned(), "1234".to_owned());
        env.insert("MCP_OLLAMA_LOG_FILE".to_owned(), "server.log".to_owned());

        let options = parse_args(&[], &env);
        assert_eq!(options.host, "127.0.0.1:1234");
        assert_eq!(options.timeout_ms, 1234);
        assert_eq!(options.log_file, "server.log");
        assert!(options.errors.is_empty());
    }

    #[test]
    fn flags_override_the_environment() {
        let mut env = HashMap::new();
        env.insert(
            "OLLAMA_HOST".to_owned(),
            "http://127.0.0.1:11434".to_owned(),
        );
        let options = parse_args(
            &args(&[
                "--host",
                "https://ollama.com",
                "--timeout-ms",
                "0",
                "--verbose",
            ]),
            &env,
        );
        assert_eq!(options.host, "https://ollama.com");
        assert_eq!(options.timeout_ms, 0);
        assert!(options.verbose);
        assert!(options.errors.is_empty());
    }

    #[test]
    fn invalid_values_are_collected_not_panicked_on() {
        let options = parse_args(&args(&["--timeout-ms", "soon", "--host"]), &HashMap::new());
        assert_eq!(options.errors.len(), 2);
        assert!(options.errors[0].contains("non-negative number"));
        assert!(options.errors[1].contains("--host requires a value"));

        let options = parse_args(&args(&["--wat"]), &HashMap::new());
        assert_eq!(options.errors.len(), 1);
        assert!(options.errors[0].contains("unknown argument"));
    }

    #[test]
    fn host_normalization_happens_during_resolution() {
        let options = parse_args(&args(&["--host", "0.0.0.0:11434"]), &HashMap::new());
        let config = resolve_config(&options, &HashMap::new()).unwrap();
        assert_eq!(config.host, "http://127.0.0.1:11434");
        assert_eq!(config.key_source, KeySource::None);
    }

    #[test]
    fn environment_key_wins_without_exposing_the_value() {
        let mut env = HashMap::new();
        env.insert("OLLAMA_API_KEY".to_owned(), "  secret  ".to_owned());
        let options = parse_args(&[], &env);
        let config = resolve_config(&options, &env).unwrap();
        assert_eq!(config.api_key.as_deref(), Some("secret"));
        assert_eq!(config.key_source, KeySource::Environment);
        assert!(!config.key_source.label().contains("secret"));
    }

    #[test]
    fn an_explicit_missing_credentials_file_fails_startup() {
        let mut env = HashMap::new();
        env.insert(
            CREDENTIALS_FILE_ENV.to_owned(),
            "C:/definitely/not/here/.credentials.yaml".to_owned(),
        );
        let options = parse_args(&[], &env);
        let error = resolve_config(&options, &env).unwrap_err();
        assert!(error.to_string().contains("Cannot read the key file"));
    }

    #[test]
    fn usage_names_both_targets_and_both_tools() {
        let text = usage();
        assert!(text.contains("127.0.0.1:11434"));
        assert!(text.contains("https://ollama.com"));
        assert!(text.contains("web_search"));
        assert!(text.contains("web_fetch"));
        assert!(!text.contains("--api-key"));
    }
}
