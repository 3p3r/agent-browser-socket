# agent-browser-socket (abs 💪)

Swiss Army Knife tool that bridges web apps to browser automation via [agent-browser](https://github.com/vercel-labs/agent-browser) and [page-agent](https://github.com/alibaba/page-agent):

```txt
 [.____|____.]
    [__|__]
    [__|__]
    [__|__]
```

Your web app connects to this server → server controls browser on your machine.

This project adds helpful automation features and a built-in admin dashboard to the core `agent-browser` experience, all in a single self-contained binary.

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

# Open a URL with Page Agent injected
./agent-browser-socket-* --verbose --command --headed agentic-open https://google.com

# Open Google, inject Page Agent, and submit a prompt
./agent-browser-socket-* --verbose --command --headed agentic-prompt https://google.com "search for rust async patterns"

# Submit a prompt to Page Agent on the current page (no URL)
./agent-browser-socket-* --verbose --command agentic-prompt "search for rust async patterns"

# Clean cached browser binary
./agent-browser-socket-* --clean

# Capture screenshots as JSON
./agent-browser-socket-* --screenshot
```

`--verbose` shows browser stdout/stderr (suppressed by default). `--headed` opens a visible browser window.

---

## Advanced Features

### Browser Detection on Startup

On launch, `agent-browser-socket` tries to detect a local Chrome-like browser path.

- Detection order mirrors the upstream logic: default browser lookup, known install paths, then Desktop shortcuts.
- In TUI mode, the detected path is shown below the keyboard shortcuts line.
- Detection is best-effort and never blocks startup.

### URI Launch Mode

Register with `--register-uri`, then open URLs:
- `abs://open?port=9911`
- `abs://open?port=9911&host=127.0.0.1`

**Behavior:** Auto-starts server, accepts one client, exits on disconnect.

### Embedded Admin Dashboard Assets

The admin dashboard at `/` is fully self-hosted by this binary.

- Socket.IO client is served from `/assets/socket.io.min.js`.
- Page Agent demo script is served from `/assets/page-agent.demo.js`.
- Dashboard HTML loads both embedded scripts directly (no CDN dependency).

### Page Agent Runtime Flags

Use these startup flags to control the embedded Page Agent bundle values served at `/assets/page-agent.demo.js`:

- `--page-agent-model` (default: `qwen3.5-plus`)
- `--page-agent-url` (default: `http://localhost:11434/v1`)
- `--page-agent-key` (default: `NA`)

Example:

```bash
./agent-browser-socket-* \
  --page-agent-model qwen3.5-plus \
  --page-agent-url http://localhost:11434/v1 \
  --page-agent-key NA
```

Runtime constants are replaced when the asset is served.

### MCP Mode

Run as MCP stdio server:

```bash
./agent-browser-socket-* --mcp
```

**Available tools:**

`health`, `version`, `shutdown`, `screenshot_system`, `command`

### Automatic `--executable-path` Prefill

For Socket.IO `command` and MCP `command` calls:

- If `--executable-path` is missing, the server appends `--executable-path=<detected_path>` automatically.
- If the caller already passes `--executable-path` (either `--executable-path=/x` or `--executable-path /x`), the server does not override it.
- If detection fails and no automatic `--executable-path` can be injected, run `agent-browser-socket --command install` to install a browser through this binary.

### Synthetic `agentic-open` Command

Works across CLI (`--command`), Socket.IO, and MCP:

- `agentic-open <url>` translates to `open <url>` and injects Page Agent after a successful open.
- In server mode (Socket.IO/MCP), injection loads the bundle via the embedded asset route.
- In CLI mode (`--command`), injection uses chunked `eval` directly (no network requests).

### Synthetic `agentic-prompt` Command

Works across CLI (`--command`), Socket.IO, and MCP:

- `agentic-prompt <url> <prompt>` — opens the URL, injects Page Agent, then submits the prompt.
- `agentic-prompt <prompt>` — submits the prompt to Page Agent on the current page (no navigation). URLs are distinguished by containing `://` or starting with `about:`.
- The prompt is typed into the Page Agent input and Enter is dispatched.

**MCP client config:**
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

Requires the `page-agent` npm package (used at build time):

```bash
npm install
cargo run
cargo test
cargo coverage  # outputs to coverage/
```

---

## Platform Notes

- Linux: Self-extracting, works on x86_64 and aarch64, glibc 2.35+
- macOS: Universal binary (x86_64 + aarch64)
- All binaries are 64-bit
