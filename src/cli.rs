//! Command-line entry point for the Ollama web search MCP server.
//!
//! Runs nothing but the stdio transport: stdout carries JSON-RPC messages, and
//! every diagnostic goes to stderr.

use std::collections::HashMap;
use std::sync::Arc;

use crate::config::{parse_args, resolve_config, usage};
use crate::logging::Logger;
use crate::ollama::{CLOUD_HOST, CLOUD_KEY_URL, OllamaClient};
use crate::secrets::{API_KEY_REF, CREDENTIALS_FILE_ENV};
use crate::server::{McpServer, SERVER_NAME, SERVER_VERSION};
use crate::transport;

const EXIT_OK: i32 = 0;
const EXIT_FAILURE: i32 = 1;
const EXIT_CONFIG: i32 = 2;

/// Install the signal handler that aborts in-flight calls when the client or a
/// terminal interrupts us.
fn spawn_signal_handler(server: Arc<McpServer<OllamaClient>>, logger: Logger) {
    tokio::spawn(async move {
        #[cfg(unix)]
        {
            let mut terminate =
                tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                    .expect("cannot install a SIGTERM handler");
            tokio::select! {
                _ = tokio::signal::ctrl_c() => {}
                _ = terminate.recv() => {}
            }
        }
        #[cfg(not(unix))]
        {
            let _ = tokio::signal::ctrl_c().await;
        }
        logger.debug("signal received; aborting in-flight calls");
        server.abort_all();
    });
}

/// Run the server until stdin closes.
pub async fn run() -> i32 {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let env: HashMap<String, String> = std::env::vars().collect();
    run_with(args, env).await
}

/// Testable form of [`run`]: arguments and environment are injected.
pub async fn run_with(args: Vec<String>, env: HashMap<String, String>) -> i32 {
    let options = parse_args(&args, &env);

    if options.help {
        println!("{}", usage());
        return EXIT_OK;
    }
    if options.version {
        println!("{SERVER_VERSION}");
        return EXIT_OK;
    }
    if !options.errors.is_empty() {
        for message in &options.errors {
            eprintln!("{SERVER_NAME}: {message}");
        }
        eprintln!("{SERVER_NAME}: run with --help for usage");
        return EXIT_CONFIG;
    }

    let config = match resolve_config(&options, &env) {
        Ok(config) => config,
        Err(error) => {
            eprintln!("{SERVER_NAME}: {error}");
            return EXIT_CONFIG;
        }
    };

    // An MCP client owns this process's stderr and usually discards it, so an
    // optional log file is often the only place to see what happened.
    let logger = match config.log_file_path() {
        Some(path) => match Logger::with_log_file(config.verbose, path) {
            Ok(logger) => logger,
            Err(error) => {
                eprintln!("{SERVER_NAME}: cannot open the log file {path}: {error}");
                Logger::new(config.verbose)
            }
        },
        None => Logger::new(config.verbose),
    };

    let client = match OllamaClient::new(
        Some(&config.host),
        config.api_key.clone(),
        config.timeout_ms,
    ) {
        Ok(client) => client,
        Err(error) => {
            eprintln!("{SERVER_NAME}: {}", error.message);
            return EXIT_CONFIG;
        }
    };

    // describe_target never includes the key, and neither does the source
    // label, so this line is safe to log anywhere.
    logger.debug(format!(
        "serving stdio; target={} keySource={} timeoutMs={} pid={}{}",
        client.describe_target(),
        config.key_source.label(),
        config.timeout_ms,
        std::process::id(),
        config
            .log_file_path()
            .map(|path| format!(" log={path}"))
            .unwrap_or_default()
    ));

    // The host decides the target; the key is only a header. A daemon ignores
    // it, so say nothing unless asked. The hosted API does care, and a 401 from
    // it is clearer than a refusal to start: warn, then let the first call
    // report what the API actually said.
    if !client.authenticated() && client.is_cloud() {
        logger.warn(format!(
            "{} is Ollama's hosted API and normally requires an API key, but none was found, so calls \
             will come back 401. Set OLLAMA_API_KEY, or add {API_KEY_REF} under refs: in a credentials \
             document and name it with {CREDENTIALS_FILE_ENV} (create a key at {CLOUD_KEY_URL}).",
            config.host
        ));
    }
    if client.authenticated() && !client.is_cloud() {
        logger.debug(format!(
            "a key is configured, but {} is not {CLOUD_HOST}; a local daemon ignores the header. Point \
             OLLAMA_HOST at {CLOUD_HOST} to call the hosted API.",
            config.host
        ));
    }

    let server = Arc::new(McpServer::new(Arc::new(client), logger.clone()));
    spawn_signal_handler(Arc::clone(&server), logger.clone());

    let result = transport::serve(
        server,
        tokio::io::stdin(),
        tokio::io::stdout(),
        logger.clone(),
    )
    .await;

    match result {
        Ok(()) => {
            logger.debug("stdio closed; exiting");
            EXIT_OK
        }
        Err(error) => {
            logger.error(format!("stdio transport failed: {error}"));
            EXIT_FAILURE
        }
    }
}
