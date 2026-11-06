# mcp-ollama-web-rust — deployment guide

This bundle installs the `mcp-ollama-web-rust` MCP stdio server on a Windows x64 machine. The
server exposes two MCP tools — `web_search` and `web_fetch` — backed by Ollama's web search and web
fetch API, either through a local Ollama daemon or through `https://ollama.com`.

**Version:** 0.5.0  
**Binary:** Windows x64, built with Rust 1.99.0  
**Bundle contents:** see [Package contents](#package-contents).

## Requirements

- Windows 10 or Windows 11, x64.
- One of:
  - an **Ollama API key** for the hosted API — create one at <https://ollama.com/settings/keys>; or
  - a **local Ollama daemon** with web search available (`ollama serve`, signed in to an Ollama
    account). A daemon needs no key.
- A corporate proxy only if the machine cannot reach `ollama.com` directly. Proxy settings are
  **optional** and are supplied through the MCP client's `env` block.

## Package contents

| File | Purpose |
|---|---|
| `mcp-ollama-web-rust.exe` | The MCP server. |
| `install.ps1` | Copies the server into an install directory, creates `.credentials` if absent, and generates `mcp-config.json`. |
| `credentials.example` | The committed definition of the credentials-document format; contains **no key**. |
| `mcp-config.example.json` | Example MCP client entry with absolute-path placeholders. |
| `INSTALL.md` | This guide. |
| `LICENSE` | MIT license. |
| `SHA256SUMS.txt` | SHA-256 hashes for the files in this bundle. |

No real credential is included. Never send a `.credentials` file that contains a key.

## Install

### Option A — run the installer (recommended)

From the extracted bundle directory:

```powershell
powershell -ExecutionPolicy Bypass -File .\install.ps1
```

The default install directory is `%LOCALAPPDATA%\Programs\mcp-ollama-web-rust`. To choose another:

```powershell
powershell -ExecutionPolicy Bypass -File .\install.ps1 -InstallDir "C:\Tools\mcp-ollama-web-rust"
```

If the machine is behind the corporate proxy, pass the proxy values (all three are optional):

```powershell
powershell -ExecutionPolicy Bypass -File .\install.ps1 `
  -HttpProxy  "http://proxy_ip:proxy_port" `
  -HttpsProxy "http://proxy_ip:proxy_port" `
  -NoProxy    "localhost,127.0.0.1,*.company.com,.company.com"
```

The installer:

1. Copies the executable and license into the install directory.
2. Creates `.credentials` from `credentials.example` **only if it does not already exist**, so an
   existing key is never overwritten.
3. Generates `mcp-config.json` with the absolute executable and credentials paths, plus any proxy
   values you passed.
4. Prints the installed version.

### Option B — manual install

```powershell
$dir = "$env:LOCALAPPDATA\Programs\mcp-ollama-web-rust"
New-Item -ItemType Directory -Force -Path $dir | Out-Null
Copy-Item .\mcp-ollama-web-rust.exe $dir
Copy-Item .\LICENSE $dir
Copy-Item .\credentials.example "$dir\.credentials"
notepad "$dir\.credentials"
```

Replace `replace-with-your-key` in `$dir\.credentials` with your Ollama API key, keeping the file on
one line for the value. The file is a credentials document; the server reads only its
`OLLAMA_API_KEY` entry.

## Configure the MCP client

The installer writes `mcp-config.json`, a generic stdio entry. Use it as-is for clients that read
the `mcpServers` shape, or copy the snippet for your assistant below. All examples use the default
install directory:

```text
C:\Users\<you>\AppData\Local\Programs\mcp-ollama-web-rust
```

Both the executable and `.credentials` paths **must be absolute**; a client may launch the server
from a different working directory. JSON examples use forward slashes; YAML can use Windows
backslashes.

### Claude Code

Add the server with the CLI (project scope shown; use `--scope user` for all projects):

```powershell
claude mcp add --scope project `
  --env "OLLAMA_HOST=https://ollama.com" `
  --env "OLLAMA_CREDENTIALS_FILE=C:/Users/<you>/AppData/Local/Programs/mcp-ollama-web-rust/.credentials" `
  --transport stdio `
  ollama-web -- "C:/Users/<you>/AppData/Local/Programs/mcp-ollama-web-rust/mcp-ollama-web-rust.exe"
```

Or put this in `.mcp.json` at the project root. Claude Code reads an entry without `type` as a
stdio server:

```json
{
  "mcpServers": {
    "ollama-web": {
      "command": "C:/Users/<you>/AppData/Local/Programs/mcp-ollama-web-rust/mcp-ollama-web-rust.exe",
      "args": [],
      "env": {
        "OLLAMA_HOST": "https://ollama.com",
        "OLLAMA_CREDENTIALS_FILE": "C:/Users/<you>/AppData/Local/Programs/mcp-ollama-web-rust/.credentials"
      }
    }
  }
}
```

Check with `claude mcp list` or `/mcp` inside Claude Code, then restart the session.

### OpenCode

Add this to the `opencode.jsonc` your OpenCode installation reads, adjusting the absolute paths:

```jsonc
{
  "$schema": "https://opencode.ai/config.json",
  "mcp": {
    "ollama-web": {
      "type": "local",
      "command": [
        "C:/Users/<you>/AppData/Local/Programs/mcp-ollama-web-rust/mcp-ollama-web-rust.exe"
      ],
      "enabled": true,
      "environment": {
        "OLLAMA_HOST": "https://ollama.com",
        "OLLAMA_CREDENTIALS_FILE": "C:/Users/<you>/AppData/Local/Programs/mcp-ollama-web-rust/.credentials"
      },
      "timeout": 120000
    }
  }
}
```

`timeout` is in milliseconds. Current OpenCode defaults it to `30000`, while this server's own
request ceiling defaults to `55000`; `120000` lets the server report a timeout before the client
does. OpenCode also supports `opencode mcp add` in newer builds — the config file avoids depending
on that version.

The OpenCode v2 configuration nests servers under `mcp.servers` and uses `disabled` instead of
`enabled`:

```jsonc
{
  "$schema": "https://opencode.ai/config.json",
  "mcp": {
    "servers": {
      "ollama-web": {
        "type": "local",
        "command": [
          "C:/Users/<you>/AppData/Local/Programs/mcp-ollama-web-rust/mcp-ollama-web-rust.exe"
        ],
        "environment": {
          "OLLAMA_HOST": "https://ollama.com",
          "OLLAMA_CREDENTIALS_FILE": "C:/Users/<you>/AppData/Local/Programs/mcp-ollama-web-rust/.credentials"
        }
      }
    }
  }
}
```

Use the shape your installed OpenCode version documents. Check with `opencode mcp list` or `/mcps`
in the app.

### DeepSeek Harness

DSH strips credential-shaped variables (`*_KEY`, `*_PASSWORD`, `*_SECRET`, `*_TOKEN`) and `DSH_*`
from a spawned stdio child. `OLLAMA_CREDENTIALS_FILE` is a path, not a secret, and this file lives
outside DSH's credential store, so the entry supplies the target and the file path directly:

```yaml
- insert:
    - id: mcp-ollama-web-rust
      name: '@deepseek-ai/dsh-mcp-client'
      config:
        serverName: ollama
        transport: stdio
        command: C:\Users\<you>\AppData\Local\Programs\mcp-ollama-web-rust\mcp-ollama-web-rust.exe
        cwd: C:\Users\<you>\AppData\Local\Programs\mcp-ollama-web-rust
        env:
          OLLAMA_HOST: https://ollama.com
          OLLAMA_CREDENTIALS_FILE: C:\Users\<you>\AppData\Local\Programs\mcp-ollama-web-rust\.credentials
        toolCallTimeoutMs: 120000
```

Add it to the DSH profile patch your profile loads, reload or restart DSH, and start a **new
session**. The tools appear as `mcp__ollama__web_search` and `mcp__ollama__web_fetch`. Setting
`failOnStartupError: true` in the entry turns a broken server into a loud startup failure instead of
a harness that quietly has no web tools.

If the key lives in DSH's credential store instead, point at it and put `OLLAMA_API_KEY` under its
`refs:` section:

```yaml
          OLLAMA_CREDENTIALS_FILE: !!js process.env.DSH_HOME + '/.credentials.yaml'
```

DSH evaluates `!!js` inside the harness, where `DSH_HOME` still exists; the spawned child never
sees it.

### Local daemon instead of the hosted API

Set `OLLAMA_HOST` to the daemon and omit the credential:

```json
"env": {
  "OLLAMA_HOST": "http://127.0.0.1:11434"
}
```

For OpenCode this is the `environment` object; for DSH it is `env:`. A local daemon needs no key.

### Behind a corporate proxy (optional)

Only when the machine cannot reach `ollama.com` directly, add these keys to the same environment
block. Omit all three when no proxy is required.

```json
"HTTP_PROXY": "http://proxy_ip:proxy_port",
"HTTPS_PROXY": "http://proxy_ip:proxy_port",
"NO_PROXY": "localhost,127.0.0.1,*.company.com,.company.com"
```

- **Claude Code:** add them to `env` in `.mcp.json`, or add three `--env` flags to `claude mcp add`.
- **OpenCode:** add them to `environment`.
- **DeepSeek Harness:** add them to `env`. Quote `NO_PROXY` in YAML, because an unquoted value
  beginning with `*` is parsed as a YAML alias:

```yaml
        env:
          OLLAMA_HOST: https://ollama.com
          OLLAMA_CREDENTIALS_FILE: C:\Users\<you>\AppData\Local\Programs\mcp-ollama-web-rust\.credentials
          HTTP_PROXY: http://proxy_ip:proxy_port
          HTTPS_PROXY: http://proxy_ip:proxy_port
          NO_PROXY: 'localhost,127.0.0.1,*.company.com,.company.com'
```

`NO_PROXY` keeps the local daemon (`localhost`, `127.0.0.1`) off the proxy, while hosted calls to
`ollama.com` go through it. `.company.com` matches the domain and its subdomains, so `*.company.com`
is redundant but harmless. Replace `proxy_ip`, `proxy_port`, and `company.com` with your network's
values, and omit all three variables when the server is not behind a proxy.

## Verify

Check the executable:

```powershell
$dir = "$env:LOCALAPPDATA\Programs\mcp-ollama-web-rust"
& "$dir\mcp-ollama-web-rust.exe" --version
```

Run a handshake without any MCP client:

```powershell
'{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25"}}' |
  & "$dir\mcp-ollama-web-rust.exe"
```

The response must contain `"serverInfo":{"name":"mcp-ollama-web-rust"...}` and
`"protocolVersion":"2025-11-25"`.

Run a live search against the configured target:

```powershell
@'
{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25"}}
{"jsonrpc":"2.0","method":"notifications/initialized"}
{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"web_search","arguments":{"query":"Ollama official website","max_results":3}}}
'@ | & "$dir\mcp-ollama-web-rust.exe"
```

The second response should contain ranked results. If it returns an `isError` result, see
[Troubleshooting](#troubleshooting).

### Verify in your client

- **Claude Code:** `claude mcp list` should show `ollama-web` connected, and `/mcp` inside a session
  lists its tools. Project-scoped `.mcp.json` entries are approved on first use.
- **OpenCode:** `opencode mcp list` (or `/mcps` in the app) should show the server connected.
- **DeepSeek Harness:** reload or restart DSH and start a new session; `mcp__ollama__web_search`
  and `mcp__ollama__web_fetch` should be available.

## Troubleshooting

| Symptom | Cause and fix |
|---|---|
| `requires an API key` or `rejected the API key` (HTTP 401) | The hosted API did not receive a usable key. Edit `<INSTALL_DIR>\.credentials` and replace the placeholder with a real key, or set `OLLAMA_API_KEY` in the client's `env`. |
| `Cannot read the key file ...` | `OLLAMA_CREDENTIALS_FILE` names a missing or unreadable file. Check the absolute path and run the installer again if needed. |
| `no "OLLAMA_API_KEY" entry` | The named file is a credentials document without that entry. Add `OLLAMA_API_KEY: <key>` under `refs:`, or point at a bare secret file. |
| `Cannot reach Ollama at ...` | The daemon is not running or `OLLAMA_HOST` is wrong. For a local daemon, `curl http://127.0.0.1:11434/api/version` should answer. |
| `Ollama has no search endpoint` | The daemon is old or web search is unavailable. Update Ollama and sign in, or point `OLLAMA_HOST` at `https://ollama.com`. |
| Proxy errors, timeouts, or DNS failures | Confirm `HTTP_PROXY`, `HTTPS_PROXY`, and `NO_PROXY` in the client's environment block (`env` in Claude Code/DSH, `environment` in OpenCode). Remove them when the machine is not behind a proxy. |
| Tools do not appear in the client | Validate the client's JSON, confirm both paths are absolute, and restart the client. Run the server manually with `--log-file <path> --verbose`; MCP clients usually discard the child's stderr. |
| Call times out | Raise `MCP_OLLAMA_TIMEOUT_MS` (milliseconds, default 55000) in the same environment block, and raise the client's own tool timeout (OpenCode `timeout`, DSH `toolCallTimeoutMs`). |

## Upgrade

Replace `mcp-ollama-web-rust.exe` in the install directory with the new build. Keep the existing
`.credentials`; the installer never overwrites it. Restart the MCP client.

## Uninstall

Remove the install directory and delete the `ollama-web` entry from the MCP client's configuration.
No system-wide environment variables are set by the installer.

## Verify the bundle

From the bundle directory, in PowerShell:

```powershell
Get-FileHash .\mcp-ollama-web-rust.exe -Algorithm SHA256
Get-Content .\SHA256SUMS.txt
```

The first hash must match the line for `mcp-ollama-web-rust.exe`. In Git Bash, use:

```sh
sha256sum -c SHA256SUMS.txt
```
