//! Diagnostic logging.
//!
//! The MCP stdio transport owns stdout: it carries newline-delimited JSON-RPC
//! and nothing else. Every log line therefore goes to stderr, and optionally to
//! an append-only file for clients that hide their child's stderr.

use std::fs::OpenOptions;
use std::io::Write;
use std::sync::{Arc, Mutex};

use crate::server::SERVER_NAME;

#[derive(Debug)]
struct Sink {
    verbose: bool,
    file: Mutex<Option<std::fs::File>>,
}

/// A cheap, cloneable logger shared by the CLI, the server, and transport tasks.
#[derive(Debug, Clone)]
pub struct Logger {
    inner: Arc<Sink>,
}

impl Logger {
    /// A logger that writes warnings and errors to stderr.
    pub fn new(verbose: bool) -> Self {
        Self {
            inner: Arc::new(Sink {
                verbose,
                file: Mutex::new(None),
            }),
        }
    }

    /// A logger that additionally appends every line to `path`.
    pub fn with_log_file(verbose: bool, path: &str) -> Result<Self, std::io::Error> {
        let file = OpenOptions::new().create(true).append(true).open(path)?;
        Ok(Self {
            inner: Arc::new(Sink {
                verbose,
                file: Mutex::new(Some(file)),
            }),
        })
    }

    pub fn debug(&self, message: impl AsRef<str>) {
        if self.inner.verbose {
            self.write("debug", message.as_ref());
        }
    }

    pub fn info(&self, message: impl AsRef<str>) {
        self.write("info", message.as_ref());
    }

    pub fn warn(&self, message: impl AsRef<str>) {
        self.write("warn", message.as_ref());
    }

    pub fn error(&self, message: impl AsRef<str>) {
        self.write("error", message.as_ref());
    }

    fn write(&self, level: &str, message: &str) {
        let line = format!("[{SERVER_NAME}] {level} {message}\n");
        // stderr is allowed to fail (for example when the client closed the
        // pipe); logging must never take the server down.
        let _ = std::io::stderr().write_all(line.as_bytes());
        if let Ok(mut guard) = self.inner.file.lock()
            && let Some(file) = guard.as_mut()
        {
            let _ = file.write_all(line.as_bytes());
        }
    }
}
