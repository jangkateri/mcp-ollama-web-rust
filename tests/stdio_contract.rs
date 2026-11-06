//! Contract tests that drive the real executable over a real stdio pipe.
//!
//! These assert the two things unit tests cannot: stdout carries only
//! newline-delimited JSON-RPC, and the legacy handshake works in a child
//! process with no daemon required.

mod support;

use std::collections::HashMap;
use std::io::Write;

use serde_json::Value;
use support::server_command;

#[test]
fn handshake_tools_list_and_discover_work_over_stdio() {
    let mut child = server_command().spawn().expect("server starts");
    {
        let stdin = child.stdin.as_mut().expect("stdin is piped");
        for message in [
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","clientInfo":{"name":"contract-test"}}}"#,
            r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}"#,
            r#"{"jsonrpc":"2.0","id":3,"method":"server/discover","params":{}}"#,
            r#"{"jsonrpc":"2.0","id":4,"method":"ping","params":{}}"#,
        ] {
            writeln!(stdin, "{message}").expect("request writes");
        }
    }
    // Close stdin so the server drains in-flight calls and exits.
    drop(child.stdin.take());

    let output = child.wait_with_output().expect("server exits");
    assert!(
        output.status.success(),
        "server failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8(output.stdout).expect("stdout is UTF-8");
    let mut responses: HashMap<i64, Value> = HashMap::new();
    for line in stdout.lines() {
        let message: Value =
            serde_json::from_str(line).expect("every stdout line is JSON-RPC, and nothing else");
        assert_eq!(message["jsonrpc"], "2.0");
        if let Some(id) = message["id"].as_i64() {
            responses.insert(id, message);
        }
    }
    assert_eq!(responses.len(), 4, "one response per request");

    let initialize = &responses[&1]["result"];
    assert_eq!(initialize["protocolVersion"], "2025-11-25");
    assert_eq!(initialize["serverInfo"]["name"], "mcp-ollama-web-rust");
    assert!(initialize["instructions"].is_string());

    let tools = responses[&2]["result"]["tools"].as_array().unwrap();
    assert_eq!(tools.len(), 2);
    let names: Vec<&str> = tools
        .iter()
        .filter_map(|tool| tool["name"].as_str())
        .collect();
    assert_eq!(names, vec!["web_search", "web_fetch"]);

    // The deliberate legacy-era signal: a 2026-capable client falls back.
    assert_eq!(responses[&3]["error"]["code"], -32601);

    assert_eq!(responses[&4]["result"], serde_json::json!({}));

    let stderr = String::from_utf8(output.stderr).expect("stderr is UTF-8");
    assert!(stderr.contains("initialize from contract-test"));
}

#[test]
fn help_goes_to_stdout_and_errors_go_to_stderr() {
    let help = server_command().arg("--help").output().expect("help runs");
    assert!(help.status.success());
    let text = String::from_utf8(help.stdout).unwrap();
    assert!(text.contains("web_search"));
    assert!(text.contains("web_fetch"));
    assert!(!text.contains("--api-key"));

    let failure = server_command()
        .arg("--not-a-flag")
        .output()
        .expect("bad arg runs");
    assert_eq!(failure.status.code(), Some(2));
    assert!(
        failure.stdout.is_empty(),
        "configuration errors stay off stdout"
    );
    assert!(String::from_utf8_lossy(&failure.stderr).contains("unknown argument"));
}
