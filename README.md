# agent-browser-socket

Swiss Army Knife tool that bridges web apps to browser automation via `agent-browser`.

Your web app connects to this server → server controls browser on your machine.

---

## Quick Start

**1. Download** from [releases](https://github.com/3p3r/agent-browser-socket/releases):
- Linux: `agent-browser-socket-linux`
- macOS: `agent-browser-socket-mac`  
- Windows: `agent-browser-socket-windows.exe`

**2. Run:**

```bash
# Linux/macOS
chmod +x ./agent-browser-socket-*
./agent-browser-socket-*

# Windows
.\agent-browser-socket-windows.exe
```

> **Mac users**: xattr -d com.apple.quarantine ./agent-browser-socket-mac to run the binary after downloading from the Internet.

**3. Connect** to `http://localhost:9607`

Check health: `GET /health` → `{"status": "ok"}`

---

## Configuration

**Defaults** (no config needed):
- Port: `9607`
- Host: `0.0.0.0`
- Auth: disabled
- Browser: auto-downloaded

**Custom config** (optional) via `.abs` file or `ABS_*` env vars:

```toml
# .abs or ~/.abs
port = 9607
host = "0.0.0.0"
auth_url = "http://localhost:8080/auth"
browser_path = "/usr/local/bin/agent-browser"
```

**Priority:** Built-in → `~/.abs` → `./.abs` → `ABS_*` env vars

---

## Common Commands

```bash
# Show version
./agent-browser-socket-* --version

# Register abs:// URL handler
./agent-browser-socket-* --register-uri

# Pass commands to agent-browser
./agent-browser-socket-* --command --version

# Clean cached browser binary
./agent-browser-socket-* --clean

# Capture screenshots as JSON
./agent-browser-socket-* --screenshot
```

---

## Advanced Features

### URI Launch Mode

Register with `--register-uri`, then open URLs:
- `abs://open?port=9911`
- `abs://open?port=9911&host=127.0.0.1`

**Behavior:** Auto-starts server, accepts one client, exits on disconnect.

### MCP Mode

Run as MCP stdio server:

```bash
./agent-browser-socket-* --mcp
```

**Available tools:**
- Browser: `navigate`, `screenshot`, `click`, `fill`, `select`, `hover`, `evaluate`, `set_viewport`
- API: `get`, `post`, `put`, `patch`, `delete`

**Client config:**
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

### Auth Protection

Set `auth_url` to protect Socket.IO commands via auth subrequest:

**Responses:**
- `2xx` → allowed
- `401`/`403` → denied
- Other → error

**Forwarded headers:** `Authorization`, `Cookie`, `X-Original-URI: /socket.io`

---

## Development

Build from source:

```bash
cargo run
cargo test
cargo coverage  # outputs to coverage/
```

---

## Platform Notes

- Linux: Self-extracting, works on x86_64 and aarch64, glibc 2.35+
- macOS: Universal binary (x86_64 + aarch64)
- All binaries are 64-bit
