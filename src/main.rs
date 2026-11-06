//! Executable entry point.
//!
//! Kept trivial on purpose: the library owns configuration, logging, protocol
//! dispatch, and the stdio transport.

use std::process::ExitCode;

#[tokio::main]
async fn main() -> ExitCode {
    let code = mcp_ollama_web_rust::cli::run().await;
    ExitCode::from(u8::try_from(code).unwrap_or(1))
}
