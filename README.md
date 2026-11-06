# mcp-ollama-web-rust

An [MCP](https://modelcontextprotocol.io) server, written in Rust, that publishes two tools —
`web_search` and `web_fetch` — backed by **Ollama's web grounding**. It speaks the MCP **stdio**
transport: newline-delimited JSON-RPC 2.0 over stdin/stdout, with every diagnostic on stderr.

## Project goal

Provide a standalone, reusable MCP stdio server that turns Ollama's web search and web fetch APIs
into the two tools every MCP client can call, so an assistant can ground its answers in current
public-web content without a browser or a vendor-specific plugin. The server must work unchanged
against a **local Ollama daemon** (no credential) or **Ollama's hosted API** (`OLLAMA_API_KEY`),
keep stdout reserved for protocol traffic, and expose stable, actionable errors when the daemon,
the network, or a credential is the problem.

The server runs against either Ollama target with the same requests and the same tool surface:

| Target | Host | Credential | Endpoints tried, in order |
|---|---|---|---|
| Local daemon | `http://127.0.0.1:11434` (default) | none needed — the daemon uses the account it is signed in to | `/api/experimental/web_search`, then `/api/web_search` |
| Ollama hosted API | `https://ollama.com` | `Authorization: Bearer <key>` from `OLLAMA_API_KEY` or a credentials document | `/api/web_search`, then `/api/experimental/web_search` |

`OLLAMA_HOST` is the whole target selection. The API key is only a header: a local daemon ignores
it, and the hosted API answers `401` when it is missing or wrong.

The behaviour and module boundaries are a deliberate port of the Node.js reference implementation
`mcp-ollama-web-search`: tool names and descriptions, endpoint fallback, 404 classification, error
vocabulary, protocol era, and credential lookup are kept in parity so the two servers behave the
same from a client's point of view.

## Status

| Area | State |
|---|---|
| MCP stdio transport, `initialize` / `ping` / `tools/list` / `tools/call` | Implemented |
| `web_search` and `web_fetch` against a local daemon | Implemented and live-verified against Ollama `0.40.13` on 2026-11-06 |
| Hosted API (`https://ollama.com`) | Implemented and covered by the mock-backed tests; not exercised against the live hosted API by the automated suite |
| `OLLAMA_API_KEY`, bare secret file, and credentials-document lookup | Implemented |
| Timeouts, cancellation, concurrent request handling | Implemented |
| Unit, stdio contract, and tool end-to-end tests | 51 unit + 12 integration tests pass; `clippy -D warnings` clean |

## Requirements

- **Rust and Cargo 1.99.0** (the project uses `edition = "2024"`).
- **One of:**
  - a running **Ollama daemon** with web search available — `ollama serve`, signed in to an Ollama
    account; or
  - an **Ollama API key** for the hosted API — create one at <https://ollama.com/settings/keys>.
- TLS is provided by `rustls`, so no OpenSSL installation is required.

## Quick start

```sh
# Build and show the options
cargo build --release
./target/release/mcp-ollama-web-rust --help

# Local daemon (the default) — nothing else to configure
echo '{"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}}' \
  | ./target/release/mcp-ollama-web-rust

# First time: create the local, git-ignored key file from the committed template
cp credentials.example .credentials
# then edit .credentials and replace the placeholder with your key

# Hosted API: the host selects it, the key comes from the environment
OLLAMA_API_KEY=... OLLAMA_HOST=https://ollama.com ./target/release/mcp-ollama-web-rust

# Hosted API with the key in the git-ignored .credentials file
OLLAMA_HOST=https://ollama.com \
OLLAMA_CREDENTIALS_FILE="C:/Users/<you>/rust-proj/mcp-ollama-web-rust/.credentials" \
  ./target/release/mcp-ollama-web-rust

# A complete handshake and a live search
printf '%s\n' \
  '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","clientInfo":{"name":"demo"}}}' \
  '{"jsonrpc":"2.0","method":"notifications/initialized"}' \
  '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"web_search","arguments":{"query":"Ollama official website","max_results":3}}}' \
  | ./target/release/mcp-ollama-web-rust
```

On Windows the executable is `target\release\mcp-ollama-web-rust.exe`.

## Client configuration

Any MCP client that supports stdio servers can launch the binary. For example:

```json
{
  "mcpServers": {
    "ollama-web": {
      "command": "C:/Users/you/rust-proj/mcp-ollama-web-rust/target/release/mcp-ollama-web-rust.exe",
      "env": {
        "OLLAMA_HOST": "http://127.0.0.1:11434"
      }
    }
  }
}
```

Per-assistant instructions for **Claude Code**, **OpenCode**, and **DeepSeek Harness** are in
[`dist/INSTALL.md`](dist/INSTALL.md#configure-the-mcp-client).

For the hosted API, set `OLLAMA_HOST` to `https://ollama.com` and point
`OLLAMA_CREDENTIALS_FILE` at the **absolute path** of the git-ignored `.credentials` file:

```json
{
  "mcpServers": {
    "ollama-web": {
      "command": "C:/Users/<you>/rust-proj/mcp-ollama-web-rust/target/release/mcp-ollama-web-rust.exe",
      "env": {
        "OLLAMA_HOST": "https://ollama.com",
        "OLLAMA_CREDENTIALS_FILE": "C:/Users/<you>/rust-proj/mcp-ollama-web-rust/.credentials"
      }
    }
  }
}
```

The credentials path must be absolute because an MCP client may launch the server from a different
working directory. Use forward slashes in JSON, or escape backslashes (`C:\\Users\\...`).

The key itself can also come from `OLLAMA_API_KEY` in the client's `env`. There is deliberately
**no `--api-key` flag**: a secret on a command line ends up in a process list and a shell history.

### Behind a corporate proxy (optional)

The proxy variables are **not required**. Add them to the `env` block only when the server runs
inside a network that requires a proxy:

```json
{
  "mcpServers": {
    "ollama-web": {
      "command": "C:/Users/<you>/rust-proj/mcp-ollama-web-rust/target/release/mcp-ollama-web-rust.exe",
      "env": {
        "OLLAMA_HOST": "https://ollama.com",
        "OLLAMA_CREDENTIALS_FILE": "C:/Users/<you>/rust-proj/mcp-ollama-web-rust/.credentials",
        "HTTP_PROXY": "http://proxy_ip:proxy_port",
        "HTTPS_PROXY": "http://proxy_ip:proxy_port",
        "NO_PROXY": "localhost,127.0.0.1,*.company.com,.company.com"
      }
    }
  }
}
```

The Rust HTTP client reads these variables natively; no server flag is needed. `NO_PROXY` keeps the
local daemon (`localhost`, `127.0.0.1`) off the proxy, while hosted calls to `ollama.com` go through
it. `.company.com` matches the domain and its subdomains, so `*.company.com` is redundant but
harmless. Replace `proxy_ip`, `proxy_port`, and `company.com` with your network's values, and omit
all three variables when the server is not behind a proxy.

## Tools

### `web_search`

| Argument | Type | Notes |
|---|---|---|
| `query` | string, required | Natural language. Add names, versions, dates, or a `site:` hint. |
| `max_results` | integer, optional | 1–10, default 5. Values outside the range are rejected, not silently clamped. |

Returns ranked, URL-de-duplicated hits as a text block plus `structuredContent`:

```json
{
  "query": "Ollama official website",
  "results": [{ "title": "Ollama", "url": "https://ollama.com/", "snippet": "..." }],
  "dropped": 0
}
```

### `web_fetch`

| Argument | Type | Notes |
|---|---|---|
| `url` | string, required | Absolute `http`/`https` URL. |
| `max_chars` | integer, optional | Truncate the extracted text; default 50000. |

Returns the page's extracted text with its title, the links it reported (up to 50), a `truncated`
flag, and `total_chars` describing the **untruncated** text.

## Configuration

| Flag | Environment | Default | Meaning |
|---|---|---|---|
| `--host <url>` | `OLLAMA_HOST` | `http://127.0.0.1:11434` | Ollama base URL. This is the target selection. |
| `--timeout-ms <n>` | `MCP_OLLAMA_TIMEOUT_MS` | `55000` | Ceiling per HTTP request; `0` disables it. Sits just under the usual 60 s client tool timeout, so this server reports the timeout first. |
| `--log-file <path>` | `MCP_OLLAMA_LOG_FILE` | off | Append diagnostics to a file as well as stderr. MCP clients usually hide a child's stderr, so this is often the only place to see what happened. |
| `--verbose` | — | off | Log request handling and timing, not just warnings. |
| `-h`, `--help`, `--version` | — | — | Usage and version, printed to stdout before an MCP client is attached. |

`OLLAMA_HOST` accepts the forms the Ollama installer writes: `127.0.0.1:11434`, `:11434`, or a
wildcard bind address. A wildcard (`0.0.0.0`, `::`) is mapped to loopback, because a client cannot
dial a wildcard. This matters on Windows, where the installer commonly leaves
`OLLAMA_HOST=0.0.0.0:11434` in the environment.

### Credential lookup

First hit wins, and having none is normal — a local daemon needs no credential:

1. `OLLAMA_API_KEY` in the environment.
2. A file named by `OLLAMA_CREDENTIALS_FILE`, in either of two shapes:
   - a **credentials document** holding `OLLAMA_API_KEY` under its `refs:` section — the shape of
     this checkout's `.credentials`. Block style and hand-edited flow style
     (`refs: {OLLAMA_API_KEY: …}`) are both read, including a flow mapping spread over several
     lines; or
   - a **bare secret file** whose trimmed contents are the key on one line.
3. Otherwise, a credentials document discovered at `$DSH_HOME/.credentials.yaml` and then
   `~/.dsh/.credentials.yaml`. A discovered file is only read as a credentials document, never as a
   bare secret.

`credentials.example` is the committed definition of the document format: `version: 1`, then
`refs:` with `OLLAMA_API_KEY: replace-with-your-key`. The live file is `.credentials`, which
`.gitignore` excludes. Create it from the template, replace the placeholder value with your key,
and point `OLLAMA_CREDENTIALS_FILE` at its absolute path:

```sh
cp credentials.example .credentials
```

Because `.credentials` is ignored, a key placed there is never committed; `credentials.example`
itself is safe to commit because it holds no secret.

Only the requested entry is extracted; the rest of a credentials document is never read back,
logged, or quoted in an error. A file you name must be usable — a wrong path, an empty file, or a
missing entry fails startup with a clear message — while a merely discovered file may simply not
hold the key, which is not an error. The startup log line names the effective target and the
credential *source*, never the value.

## How it talks to Ollama

Per call, in order:

1. `POST {host}/api/experimental/web_search` with `{ query, max_results }` →
   `{ results: [{ title, url, content }] }`
2. `POST {host}/api/web_search` — the stable spelling, tried only when the first has no such route.

Fetch behaves the same with `/api/experimental/web_fetch` and `/api/web_fetch`, body `{ url }` →
`{ title, content, links }`. The hosted API tries the stable spelling first; that order is an
optimization, not a correctness requirement, because each spelling falls through to the other.

**A `404` is not automatically a missing endpoint.** The response body says which case it is:

- plain text (`404 page not found`), or JSON naming the requested path → the *route* is absent, so
  the next spelling is tried;
- JSON that says only `not found` → the route exists and the page itself could not be retrieved, so
  the request is reported against the URL with the likely causes, not retried;
- HTML → neither, and it is reported as a bad response.

Results are normalized on the way through: whitespace runs are collapsed, entries without a usable
URL and duplicate URLs are dropped and counted in `dropped`, links are filtered to non-empty strings
and capped at 50, and `total_chars` always describes the full text.

## Architecture

The crate is split into a **library** (`src/lib.rs`) and a thin **binary** (`src/main.rs`). The
library owns every behaviour; the binary only starts the CLI. That split is what lets the protocol
surface be tested in-process without spawning a child process.

```text
stdin
  │  newline-delimited JSON-RPC
  ▼
transport::serve ──► protocol::classify_message ──► server::handle_request
                                                         │
                                           tools::call_tool
                                                         │
                                          ollama::OllamaClient ──► HTTP (daemon / cloud)
                                                         │
stdout ◄── transport::write_message ◄── result_response / error_response
```

Design decisions worth knowing before editing:

- **stdout is protocol-only.** Every log line goes to stderr; a stray `println!` corrupts the
  JSON-RPC stream. The contract test parses every stdout line as JSON to catch this.
- **Concurrency and cancellation.** Each request runs in its own task, so
  `notifications/cancelled` can abort a slow call instead of queueing behind it. Every request is
  also bounded by `--timeout-ms`. The transport drains all in-flight responses before exiting, which
  makes piped tests deterministic.
- **A narrow schema subset.** Tool schemas stay inside `type` / `properties` / `required` /
  `additionalProperties` / `items` / `enum` / `const` plus `description`, `title`, `default`, and
  `examples`. Harnesses such as DeepSeek Harness reject anything wider — and a rejected schema drops
  the server's entire tool set. Bounds the subset cannot express are stated in prose and enforced
  in code.
- **Legacy protocol era on purpose.** `initialize` is answered at `2025-11-25`, and
  `server/discover` is answered `-32601 Method Not Found`, which is the signal a 2026-era client
  uses to fall back to the legacy handshake. Claiming the modern era would promise a per-request
  `_meta` envelope this server does not implement.
- **`WebClient` is a trait.** `ollama::WebClient` isolates the server and tool layer from HTTP, so
  tests inject a stub instead of running a daemon. `OllamaClient` is the production implementation.
- **Secrets never reach a log or a tool result.** `OllamaClient::describe_target` reports
  `(api key)` or `(no api key)`, and credential-file errors name the missing entry, not the file's
  other values.

## Project layout

```text
mcp-ollama-web-rust/
├── Cargo.toml                 # package, binary, dependency and lint profile
├── Cargo.lock                 # reproducible dependency resolution
├── rustfmt.toml               # formatting rules used by `cargo fmt`
├── README.md                  # this document
├── BUILDING.md                # native, cross, CI, and per-OS packaging instructions
├── LICENSE                    # MIT
├── .gitignore                 # target/, IDE files, .credentials, logs
├── credentials.example        # committed definition/template; no secret
├── .credentials               # local key file (git-ignored; placeholder until replaced)
├── dist/                      # self-contained Windows x64 deployment bundle
│   ├── INSTALL.md             # deployment guide for the target machine
│   ├── install.ps1            # installer and MCP config generator
│   ├── mcp-ollama-web-rust.exe # release binary
│   ├── credentials.example     # template only; never a key
│   ├── mcp-config.example.json # MCP client entry with placeholders
│   ├── LICENSE
│   └── SHA256SUMS.txt
├── src/
│   ├── main.rs                # executable entry point; starts the CLI only
│   ├── lib.rs                 # library root and public module map
│   ├── cli.rs                 # run loop, logging setup, signal handling, stdio wiring
│   ├── config.rs              # flags, environment, credential resolution, --help text
│   ├── error.rs               # stable error codes and typed errors
│   ├── logging.rs             # stderr logger with an optional append-only file
│   ├── protocol.rs            # JSON-RPC classification, negotiation, response builders
│   ├── server.rs              # McpServer: dispatch, initialize, tools/call lifecycle
│   ├── transport.rs           # newline framing, concurrent dispatch, stdout writes
│   ├── secrets.rs             # env, bare-secret-file, and credentials-document readers
│   ├── tools.rs               # tool schemas, argument validation, text rendering
│   └── ollama/
│       ├── mod.rs             # endpoint constants, WebClient trait, re-exports
│       ├── client.rs          # HTTP client, endpoint fallback, 404 classification
│       ├── host.rs            # OLLAMA_HOST normalization, cloud detection, key normalization
│       └── types.rs           # Search/Fetch request and response types
└── tests/
    ├── support/mod.rs         # hermetic child env + scripted mock Ollama HTTP server
    ├── stdio_contract.rs      # handshake, tools/list, discover, stdout purity
    ├── mcp_tools_e2e.rs       # web_search and web_fetch over stdio against mock Ollama
    └── dist_bundle.rs         # version and no-secret guards for dist/
```

Where the Node.js reference maps onto this layout:

| Node.js | Rust |
|---|---|
| `bin/mcp-ollama-web-search.mjs` | `src/main.rs` |
| `src/cli.js` | `src/cli.rs` + `src/config.rs` |
| `src/server.js` | `src/server.rs` |
| `src/protocol.js` | `src/protocol.rs` + `src/transport.rs` |
| `src/tools.js` | `src/tools.rs` |
| `src/ollama.js` | `src/ollama/` |
| `src/errors.js` | `src/error.rs` |
| `src/secrets.js` | `src/secrets.rs` |
| `test/` | inline unit tests + `tests/` integration suite |

## Development

```sh
cargo fmt --check                                # formatting
cargo clippy --all-targets -- -D warnings        # lints, warnings denied
cargo test --all-targets                         # unit + stdio contract tests
cargo build --release                            # optimized binary
```

For native builds on Linux and macOS, cross-compilation, a CI matrix, and per-OS bundle layout, see
[BUILDING.md](BUILDING.md).

The tests need no daemon, no network, and no key. `tests/stdio_contract.rs` drives the real MCP
handshake, while `tests/mcp_tools_e2e.rs` launches the real binary against the loopback mock Ollama
server in `tests/support/mod.rs` and exercises `web_search` and `web_fetch`, endpoint fallback, the
two kinds of 404, authentication errors, and argument validation. Cancellation is covered by a unit
test in `src/server.rs`.

To verify a live daemon by hand:

```sh
curl -s http://127.0.0.1:11434/api/version      # should answer with a version

printf '%s\n' \
  '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25"}}' \
  '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"web_fetch","arguments":{"url":"https://ollama.com/","max_chars":400}}}' \
  | ./target/release/mcp-ollama-web-rust
```

When changing model-facing text (tool descriptions, `SERVER_INSTRUCTIONS`, error messages), change
the code and the text together: a model picks a tool from its description, and a user fixes a
config from the error. Stale wording is a functional bug, not a cosmetic one.

## Distribution

`dist/` is a self-contained Windows x64 deployment bundle. Copy or zip it to the target machine and
follow [`dist/INSTALL.md`](dist/INSTALL.md). It contains no credential: only
`credentials.example`, which the installer copies to a local `.credentials` on first install. For
Linux and macOS bundles, see [BUILDING.md](BUILDING.md#package-a-platform-bundle).

When the binary or version changes, refresh the bundle and its checksums:

```sh
cargo build --release
cp target/release/mcp-ollama-web-rust.exe dist/
cp credentials.example LICENSE dist/
cd dist
sha256sum mcp-ollama-web-rust.exe install.ps1 credentials.example \
  mcp-config.example.json INSTALL.md LICENSE > SHA256SUMS.txt
```

`install.ps1` never overwrites an existing `.credentials`. Proxy values in the generated
`mcp-config.json` are optional and only needed behind a corporate proxy.

## Not yet covered

Deliberately left for later:

- An end-to-end timeout test. The per-request timeout is implemented, but the mock suite does not
  yet exercise it.
- A live hosted-API smoke test. The cloud path is implemented but is not part of the automated
  suite, since that would spend quota.
- Consistency guards that fail when `--help`, the README, and the tool descriptions drift apart.
- A changelog and release automation.

## License

MIT — see [LICENSE](LICENSE).
