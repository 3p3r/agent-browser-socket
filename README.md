# agent-browser-server (abs 💪)

Swiss Army Knife tool that bridges web apps to browser automation via [agent-browser](https://github.com/vercel-labs/agent-browser) and [page-agent](https://github.com/alibaba/page-agent):

```txt
 [.____|____.]
    [__|__]
    [__|__]
    [__|__]
```

Your web app connects to `abs://` → `abs` controls browser on your machine.

This project adds helpful automation features and MCP server modes (SSE + stdio) to the core `agent-browser` experience, all in a single self-contained binary.

---

## Quick Start

**1. Download** from [releases](https://github.com/3p3r/agent-browser-server/releases):
- Linux: `agent-browser-server-linux`
- macOS: `agent-browser-server-mac`  
- Windows: `agent-browser-server-windows.exe`

**2. Run:**

```bash
# Linux/macOS
chmod +x ./agent-browser-server-*
./agent-browser-server-*

# Windows
.\agent-browser-server-windows.exe
```

> **Mac users**: xattr -d com.apple.quarantine ./agent-browser-server-mac to run the binary after downloading from the Internet.

**3. Connect** to MCP SSE at `http://localhost:9607/mcp`

---

## Configuration

**Defaults** (no config needed):
- Port: `9607`
- Host: `0.0.0.0`
- Browser: auto-detected

**Custom config** (optional) via `.abs` file or `ABS_*` env vars:

```toml
# .abs or ~/.abs
port = 9607
host = "0.0.0.0"
browser_path = "/usr/local/bin/agent-browser"

[page_agent]
model = "qwen3.5-plus"
url = "http://localhost:11434/v1"
key = "NA"
```

For nested `page_agent` values via env vars, use:
- `ABS_PAGE_AGENT__MODEL`
- `ABS_PAGE_AGENT__URL`
- `ABS_PAGE_AGENT__KEY`

**Priority:** Built-in → `~/.abs` → `./.abs` → `ABS_*` env vars → CLI flags

CLI flags always override file and env var configuration.

---

## Common Commands

```bash
# Show version
./agent-browser-server-* --verbose --version

# Register abs:// URL handler
./agent-browser-server-* --register-uri

# Pass commands to agent-browser
./agent-browser-server-* --verbose --command --version

# Open a URL with Page Agent injected
./agent-browser-server-* --verbose --command --headed agentic-open https://google.com

# Open Google, inject Page Agent, and submit a prompt
./agent-browser-server-* --verbose --command --headed agentic-prompt https://google.com "search for rust async patterns"

# Submit a prompt to Page Agent on the current page (no URL)
./agent-browser-server-* --verbose --command agentic-prompt "search for rust async patterns"

# Clean cached browser binary
./agent-browser-server-* --clean

# Capture screenshots from all monitors
./agent-browser-server-* --screenshot
```

`--verbose` shows browser stdout/stderr (suppressed by default). `--headed` opens a visible browser window.

---

## Advanced Features

### Browser Detection on Startup

On launch, `agent-browser-server` tries to detect a local Chrome-like browser path.

- Detection order mirrors the upstream logic: default browser lookup, known install paths, then Desktop shortcuts.
- In TUI mode, the detected path is shown below the keyboard shortcuts line.
- Detection is best-effort and never blocks startup.

### URI Launch Mode

Register with `--register-uri`, then open URLs:
- `abs://open?port=9911`
- `abs://open?port=9911&host=127.0.0.1`

**Behavior:** Auto-starts MCP SSE server on configured host/port and keeps running until shutdown.

### Page Agent Runtime Flags

Use these startup flags to control the embedded Page Agent bundle values served at `/assets/page-agent.demo.js`:

- `--page-agent-model` (default: `qwen3.5-plus`)
- `--page-agent-url` (default: `http://localhost:11434/v1`)
- `--page-agent-key` (default: `NA` - sent as Bearer token)

Example:

```bash
./agent-browser-server-* \
  --page-agent-model qwen3.5-plus \
  --page-agent-url http://localhost:11434/v1 \
  --page-agent-key NA
```

Runtime constants are replaced when the asset is served.

These values can also be configured through `.abs` / `~/.abs` (`[page_agent]` table) or `ABS_PAGE_AGENT__*` env vars. CLI flags remain the highest priority.

### MCP Mode

Run as MCP stdio server:

```bash
./agent-browser-server-* --mcp
```

**Available tools:**

`health`, `version`, `shutdown`, `screenshot_system`, `command`, `delete_resource`, `delete_all_resources`

Default server mode (`./agent-browser-server-*`) runs MCP over SSE at `/mcp`.

### MCP Resources for Screenshots and PDFs

The MCP server now exposes generated image/PDF outputs as MCP Resources with `resource://` URIs.

- `screenshot_system` creates one resource per monitor screenshot and returns `resource_link` content with each resource URI.
- `command` intercepts successful `agent-browser screenshot ...` and `agent-browser pdf ...` calls; when output files are produced, their bytes are stored as MCP Resources and returned as `resource_link` content.
- Resources are available through `resources/list` and `resources/read`.
- Generated resources are in-memory (session/server lifetime) and are cleared on process restart.

Cleanup tools:

- `delete_resource` removes a single generated resource by URI.
- `delete_all_resources` removes all generated resources currently in memory.

### Automatic `--executable-path` Prefill

For MCP `command` calls (stdio and SSE):

- If `--executable-path` is missing, the server appends `--executable-path=<detected_path>` automatically.
- If the caller already passes `--executable-path` (either `--executable-path=/x` or `--executable-path /x`), the server does not override it.
- If detection fails and no automatic `--executable-path` can be injected, run `agent-browser-server --command install` to install a browser through this binary.

### Synthetic `agentic-open` Command

`agentic-open <url>` translates to `open <url>` and injects Page Agent after a successful open.

### Synthetic `agentic-prompt` Command

`agentic-prompt <url> <prompt>` — opens the URL, injects Page Agent, then submits the prompt.

**MCP client config:**
```json
{
  "mcpServers": {
    "agent-browser-server": {
      "command": "agent-browser-server",
      "args": ["--mcp"]
    }
  }
}
```

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
