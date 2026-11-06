# CLAUDE.md — working rules for this repository

Loaded into every session, so it stays short.

## The failure this file exists to prevent

A code change lands and the text around it does not. In this project the messages **are** the
product: a model reads the tool descriptions to pick a tool, and a user reads the error to fix a
config. Stale text is a functional bug.

The surfaces that must move together when behaviour changes:

- `src/tools.rs` tool descriptions and schemas
- `src/server.rs` `SERVER_INSTRUCTIONS`, error messages, version
- `src/config.rs` `usage()` and the flag/env behaviour it advertises
- `README.md` configuration table, tool tables, endpoint/credential claims
- `BUILDING.md` target commands, artifact names, and platform notes
- `src/error.rs` stable codes and their messages
- `tests/` — the assertion that keeps the two in step

## The rule

A change is not done when the code works. It is done when every surface that describes the code
agrees with it and these pass:

```sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test --all-targets
```

## Architecture, in one glance

`main.rs` starts `cli.rs`; `transport.rs` reads newline-delimited JSON-RPC from stdin and spawns one
task per message; `protocol.rs` classifies it; `server.rs` dispatches `initialize`, `ping`,
`tools/list`, `tools/call`, and `logging/setLevel`; `tools.rs` validates arguments and calls
`ollama::WebClient`; `ollama/client.rs` talks HTTP. `WebClient` is the seam for tests.

## Always true

- **stdout carries JSON-RPC only.** Every diagnostic goes to stderr. No `println!` outside
  `--help` / `--version`.
- **No key in any tracked file and no key flag.** The committed `credentials.example` is the format
  definition and must stay secret-free; the only key-bearing file is the git-ignored `.credentials`.
  Credentials come from `OLLAMA_API_KEY` or a file named by `OLLAMA_CREDENTIALS_FILE`. Adding an
  `--api-key` flag is a regression.
- `OLLAMA_HOST` is the whole target selection. There is no `--cloud`.
- Proxy settings are optional: behind a corporate proxy, `HTTP_PROXY`, `HTTPS_PROXY`, and `NO_PROXY`
  are read natively from the server's environment. Do not add proxy flags.
- Model-facing text names both backends (local daemon and hosted API) or neither; do not present
  one target as the only supported path.
- Tool schemas stay inside the narrow subset in `tools.rs` (`type`, `properties`, `required`,
  `additionalProperties`, `items`, `enum`, `const`, `description`, `title`, `default`, `examples`).
  A wider schema can drop the server's entire tool set in DSH.
- The legacy protocol era is deliberate: `initialize` answers `2025-11-25`, and `server/discover`
  answers `-32601` so 2026-era clients fall back.
- Never state DSH behaviour from memory. Check its source or say "unverified".
- `dist/` is a deployment bundle. Refresh the executable and `SHA256SUMS.txt` when the binary
  changes, keep `dist/INSTALL.md`'s version in step, and never copy `.credentials` into it.
- Writes outside this workspace (DSH profile, credential store) need approval — hand the user the
  snippet instead of editing their config.

## Diagnostics

When a live `web_search` or `web_fetch` fails and the cause is not obvious, first ask whether the
network or Ollama decided. `curl -s http://127.0.0.1:11434/api/version` isolates the daemon; a
targeted `web_fetch` through the server isolates the API. Run the server with `--verbose
--log-file <path>` because an MCP client usually discards the child's stderr.
