# agent-browser-socket

`agent-browser-socket` is a local bridge for web apps with browser-based agents.

Primary use: your web app talks to this local Socket.IO server, and the server runs `agent-browser` on the same machine.

This README is for users downloading the release binaries.

## Download

Download from:

[https://github.com/3p3r/agent-browser-socket/releases](https://github.com/3p3r/agent-browser-socket/releases)

Then pick the file for your OS:

- Linux: `agent-browser-socket-linux`
- macOS: `agent-browser-socket-mac`
- Windows: `agent-browser-socket-windows.exe`

Notes:

- Linux binary is a self-extracting launcher that works on both `x86_64` and `aarch64`.
- macOS binary is universal (`x86_64` + `aarch64`).
- Linux builds target broad glibc compatibility (roughly `2.35+`).
- Binaries are all 64 bit

## Run (Socket.IO server)

This is the main mode used by web apps.

### Linux

```bash
chmod +x ./agent-browser-socket-linux
./agent-browser-socket-linux
```

### macOS

```bash
chmod +x ./agent-browser-socket-mac
./agent-browser-socket-mac
```

### Windows (PowerShell)

```powershell
.\agent-browser-socket-windows.exe
```

Default server address: `http://0.0.0.0:9607`

Health check:

- `GET /health` → `{ "status": "ok" }`
- `GET /version` → `{ "version": "<wrapper-version>" }`

## Optional configuration

If you do nothing, defaults are used:

- `port = 9607`
- `host = "0.0.0.0"`
- `auth_url = null`
- `browser_path = null`

Load order (last one wins):

1. built-in defaults
2. `~/.abs`
3. `./.abs`
4. `ABS_` environment variables

Example `.abs` file:

```toml
auth_url = "http://127.0.0.1:8080/auth"
port = 9607
host = "0.0.0.0"
browser_path = "/usr/local/bin/agent-browser"
```

## Useful binary flags

```bash
# show wrapper version
./agent-browser-socket-linux --version

# pass args directly to inner agent-browser
./agent-browser-socket-linux --command --version

# delete cached embedded browser binary
./agent-browser-socket-linux --clean

# capture desktop screenshots as JSON
./agent-browser-socket-linux --screenshot
```

## MCP mode (secondary)

Start as an MCP stdio server:

```bash
./agent-browser-socket-linux --mcp
```

Exposed MCP tools:

- Browser: `browser_navigate`, `browser_screenshot`, `browser_click`, `browser_fill`, `browser_select`, `browser_hover`, `browser_evaluate`, `browser_set_viewport`
- API: `api_get`, `api_post`, `api_put`, `api_patch`, `api_delete`

Example MCP client config:

```json
{
  "mcpServers": {
    "agent-browser-socket": {
      "command": "agent-browser-socket",
      "args": ["--mcp"]
    }
  }
}
```

## Socket.IO auth behavior (optional)

If `auth_url` is set, every `command` event runs an auth subrequest:

- `2xx`: command allowed
- `401` / `403`: denied
- any other status or network failure: error

Forwarded headers:

- `Authorization`
- `Cookie`
- `X-Original-URI: /socket.io`

## For developers (optional)

If you are building from source:

```bash
cargo run
cargo test
cargo coverage
```

Coverage outputs:

- `coverage/tarpaulin-report.html`
- `coverage/lcov.info`
