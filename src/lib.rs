//! An MCP (Model Context Protocol) stdio server exposing two tools backed by
//! Ollama's web grounding:
//!
//! - [`tools::SEARCH_TOOL_NAME`] — ranked public-web search results;
//! - [`tools::FETCH_TOOL_NAME`] — extracted text for one URL.
//!
//! The server talks to a local Ollama daemon with no credential, or to
//! `https://ollama.com` with `OLLAMA_API_KEY`. `OLLAMA_HOST` is the whole target
//! selection; the key is only a header.
//!
//! The crate is split into a library and a thin binary so the protocol surface
//! can be tested in-process without spawning a child process:
//!
//! - [`config`] — flags, environment, credential resolution;
//! - [`protocol`] — JSON-RPC classification, negotiation, response builders;
//! - [`server`] — message dispatch and tool-call lifecycle;
//! - [`transport`] — newline-delimited JSON-RPC over stdio;
//! - [`ollama`] — target normalization, HTTP client, endpoint fallback;
//! - [`tools`] — tool schemas, argument validation, text rendering;
//! - [`secrets`] — credential-store readers;
//! - [`error`] — stable error vocabulary.

pub mod cli;
pub mod config;
pub mod error;
pub mod logging;
pub mod ollama;
pub mod protocol;
pub mod secrets;
pub mod server;
pub mod tools;
pub mod transport;
