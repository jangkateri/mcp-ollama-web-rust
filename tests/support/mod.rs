//! Shared integration-test support: a hermetic server command and a tiny
//! scripted HTTP server that stands in for Ollama.
//!
//! The mock speaks plain HTTP/1.1 only (no TLS), which is enough because every
//! test points `OLLAMA_HOST` at a loopback address.

// Each integration-test binary includes this module but uses only a subset of
// the helpers; unused items in one binary are used by another.
#![allow(dead_code)]

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use serde_json::Value;

/// A command for the real server binary with ambient configuration removed.
///
/// Tests must be hermetic: no real key, credentials document, proxy, or daemon
/// may influence the result.
pub fn server_command() -> Command {
    let isolated_home = std::env::temp_dir().join(format!(
        "mcp-ollama-web-rust-test-home-{}",
        std::process::id()
    ));

    let mut command = Command::new(env!("CARGO_BIN_EXE_mcp-ollama-web-rust"));
    command
        .env("OLLAMA_HOST", "http://127.0.0.1:9")
        .env("USERPROFILE", &isolated_home)
        .env("HOME", &isolated_home)
        .env("NO_PROXY", "localhost,127.0.0.1")
        .env_remove("OLLAMA_API_KEY")
        .env_remove("OLLAMA_CREDENTIALS_FILE")
        .env_remove("DSH_HOME")
        .env_remove("MCP_OLLAMA_LOG_FILE")
        .env_remove("MCP_OLLAMA_TIMEOUT_MS")
        .env_remove("HTTP_PROXY")
        .env_remove("HTTPS_PROXY")
        .env_remove("ALL_PROXY")
        .env_remove("http_proxy")
        .env_remove("https_proxy")
        .env_remove("all_proxy")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    command
}

/// Run one complete stdio session against `host`.
///
/// Writes every message, closes stdin, waits for the child to exit, and returns
/// the parsed JSON-RPC responses plus stderr.
pub fn run_session(host: &str, messages: &[Value]) -> (Vec<Value>, String) {
    let mut command = server_command();
    command.env("OLLAMA_HOST", host);
    let mut child = command.spawn().expect("server binary starts");

    {
        let stdin = child.stdin.as_mut().expect("stdin is piped");
        for message in messages {
            let line = serde_json::to_string(message).expect("message serializes");
            writeln!(stdin, "{line}").expect("request writes");
        }
    }
    drop(child.stdin.take());

    let output = child.wait_with_output().expect("server exits");
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    assert!(output.status.success(), "server failed: {stderr}");

    let stdout = String::from_utf8(output.stdout).expect("stdout is UTF-8");
    let responses = stdout
        .lines()
        .map(|line| serde_json::from_str(line).expect("stdout carries only JSON-RPC"))
        .collect();
    (responses, stderr)
}

/// Find the response for a request id.
pub fn response_for(responses: &[Value], id: i64) -> &Value {
    responses
        .iter()
        .find(|response| response["id"].as_i64() == Some(id))
        .unwrap_or_else(|| panic!("no response for id {id}: {responses:?}"))
}

/// A scripted HTTP response.
#[derive(Clone, Debug)]
pub enum MockReply {
    Json(u16, Value),
    Text(u16, &'static str),
}

/// A request the server sent to the mock.
#[derive(Clone, Debug, PartialEq)]
pub struct RecordedRequest {
    pub method: String,
    pub path: String,
    pub body: Value,
}

/// A loopback HTTP server keyed by exact path.
pub struct MockOllama {
    base_url: String,
    requests: Arc<Mutex<Vec<RecordedRequest>>>,
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl MockOllama {
    pub fn start(routes: Vec<(&str, MockReply)>) -> Self {
        let listener = TcpListener::bind(("127.0.0.1", 0)).expect("mock server binds");
        listener
            .set_nonblocking(true)
            .expect("listener is nonblocking");
        let base_url = format!("http://{}", listener.local_addr().unwrap());
        let routes: HashMap<String, MockReply> = routes
            .into_iter()
            .map(|(path, reply)| (path.to_owned(), reply))
            .collect();

        let requests = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let thread_requests = Arc::clone(&requests);
        let thread_stop = Arc::clone(&stop);

        let handle = thread::spawn(move || {
            while !thread_stop.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        // Accepted sockets inherit nonblocking mode on some
                        // platforms; make the handler deterministic.
                        let _ = stream.set_nonblocking(false);
                        let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
                        handle_connection(stream, &routes, &thread_requests);
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(2));
                    }
                    Err(_) => break,
                }
            }
        });

        Self {
            base_url,
            requests,
            stop,
            handle: Some(handle),
        }
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    pub fn requests(&self) -> Vec<RecordedRequest> {
        self.requests.lock().unwrap().clone()
    }
}

impl Drop for MockOllama {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

fn handle_connection(
    mut stream: TcpStream,
    routes: &HashMap<String, MockReply>,
    requests: &Mutex<Vec<RecordedRequest>>,
) {
    let Some(request) = read_request(&mut stream) else {
        return;
    };
    let reply = routes
        .get(&request.path)
        .cloned()
        .unwrap_or(MockReply::Text(404, "404 page not found"));
    requests.lock().unwrap().push(request);

    let (status, content_type, body) = match reply {
        MockReply::Json(status, value) => (
            status,
            "application/json",
            serde_json::to_vec(&value).expect("mock JSON serializes"),
        ),
        MockReply::Text(status, text) => (status, "text/plain", text.as_bytes().to_vec()),
    };
    let reason = match status {
        200 => "OK",
        401 => "Unauthorized",
        404 => "Not Found",
        _ => "Error",
    };

    let header = format!(
        "HTTP/1.1 {status} {reason}\r\n\
         Content-Type: {content_type}\r\n\
         Content-Length: {}\r\n\
         Connection: close\r\n\
         \r\n",
        body.len()
    );
    let _ = stream.write_all(header.as_bytes());
    let _ = stream.write_all(&body);
    let _ = stream.flush();
    let _ = stream.shutdown(Shutdown::Both);
}

fn read_request(stream: &mut TcpStream) -> Option<RecordedRequest> {
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 8192];

    let header_end = loop {
        if let Some(index) = find_header_end(&buffer) {
            break index;
        }
        let read = stream.read(&mut chunk).ok()?;
        if read == 0 {
            return None;
        }
        buffer.extend_from_slice(&chunk[..read]);
        if buffer.len() > 1024 * 1024 {
            return None;
        }
    };

    let header = String::from_utf8_lossy(&buffer[..header_end]);
    let mut lines = header.lines();
    let request_line = lines.next()?;
    let mut parts = request_line.split_whitespace();
    let method = parts.next()?.to_owned();
    let path = parts.next()?.to_owned();

    let mut content_length = 0usize;
    for line in lines {
        if let Some((name, value)) = line.split_once(':')
            && name.eq_ignore_ascii_case("content-length")
        {
            content_length = value.trim().parse().unwrap_or(0);
        }
    }

    let body_start = header_end + 4;
    while buffer.len() < body_start + content_length {
        let read = stream.read(&mut chunk).ok()?;
        if read == 0 {
            break;
        }
        buffer.extend_from_slice(&chunk[..read]);
    }
    let body_end = (body_start + content_length).min(buffer.len());
    let body = serde_json::from_slice(&buffer[body_start..body_end]).unwrap_or(Value::Null);

    Some(RecordedRequest { method, path, body })
}

fn find_header_end(buffer: &[u8]) -> Option<usize> {
    buffer.windows(4).position(|window| window == b"\r\n\r\n")
}
