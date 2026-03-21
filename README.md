# agent-browser-server (abs 💪)

Swiss Army Knife tool that bridges web apps to browser automation via [agent-browser](https://github.com/vercel-labs/agent-browser) and [page-agent](https://github.com/alibaba/page-agent):

```txt
 [.____|____.]
    [__|__]
    [__|__]
    [__|__]
```

Your web app connects to `abs://` → `abs` controls browser on your machine.

This project adds helpful automation features and MCP server modes (streamable HTTP + stdio) to the core `agent-browser` experience, all in a single self-contained binary.

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

**3. Connect** to MCP Streamable HTTP at `http://localhost:9607/mcp`

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

## MCP Command Tool

The `command` tool accepts any bash script with `agent-browser` available as a function. Basic shell simulation works too, including variable assignment and expansion, pipes, redirections, stdin, and command chaining.

```json
{
  "tool": "command",
  "input": {
    "command": "agent-browser open https://github.com",
    "env": {}
  }
}
```

Use this mode when you want full shell behavior (`|`, `&&`, `>`, command substitution, etc.).
Synthetic commands `agentic-open` and `agentic-prompt` are handled by this server (they are translated before calling `agent-browser`).

### Examples

**Screenshot with redirection:**
```json
{
  "command": "agent-browser snapshot -i > /tmp/screenshot.json && cat /tmp/screenshot.json"
}
```

**Pipes and stdin:**
```json
{
  "command": "echo 'mypassword' | agent-browser auth save github --url https://github.com/login --username myuser --password-stdin"
}
```

**Basic shell flow:**
```json
{
  "command": "name=world && echo hello-$name | cat && echo saved-$name > /tmp/report.txt"
}
```

**Complex bash pipelines:**
```json
{
  "command": "agent-browser open https://example.com && agent-browser click 'button' | jq '.selector'"
}
```

**Synthetic command translation:**
```json
{
  "command": "agent-browser agentic-open https://google.com"
}
```

---

## CLI Commands

`--command` supports full shell command strings, like MCP `command`, including basic shell simulation such as variables, pipelines, redirection, and command chaining.

```bash
# Show version
./agent-browser-server-* --verbose --version

# Register abs:// URL handler
./agent-browser-server-* --register-uri

# Run a full shell command (same behavior as MCP command tool)
./agent-browser-server-* --verbose --command "agent-browser open https://google.com"

# Use pipes/stdin in CLI mode
./agent-browser-server-* --verbose --command "echo 'mypassword' | agent-browser auth save github --url https://github.com/login --username myuser --password-stdin"

# Use basic shell flow in CLI mode
./agent-browser-server-* --verbose --command "name=world && echo hello-$name | cat > /report.txt"

# Open a URL with Page Agent injected (synthetic command translation)
./agent-browser-server-* --verbose --command "agent-browser agentic-open https://google.com"

# Open Google, inject Page Agent, and submit a prompt (synthetic command translation)
./agent-browser-server-* --verbose --command "agent-browser agentic-prompt https://google.com 'search for rust async patterns'"

# Submit a prompt to Page Agent on the current page (no URL, synthetic command translation)
./agent-browser-server-* --verbose --command "agent-browser agentic-prompt 'search for rust async patterns'"

# Clean cached browser binary
./agent-browser-server-* --clean

# Capture screenshots from all monitors
./agent-browser-server-* --screenshot
```

`--verbose` shows browser stdout/stderr (suppressed by default). `--headed` opens a visible browser window.

### Sandbox File Output

Files created during command execution (both in the sandboxed shell and by `agent-browser` on the real filesystem) are automatically detected.

- **MCP mode**: Detected files are exposed as `resource://file/{id}` MCP resources alongside the command response.
- **CLI mode**: Use `--sandbox-output <dir>` to sync detected files into a directory.

When a command creates a new real filesystem file such as `/tmp/report.png`, the wrapper syncs it into `--sandbox-output` and removes the original temp file when possible. In-memory sandbox files are written directly to the output directory.

Sync-back filtering uses built-in ignore rules for common junk such as VCS metadata, editor swap files, and OS noise. You can layer additional gitignore-style patterns from a real file on disk with `--sandbox-ignore <path>`.

```bash
./agent-browser-server-* --sandbox-output ./output --command "agent-browser snapshot -i > /tmp/screenshot.json"

# Add custom ignore patterns on top of the built-in defaults
./agent-browser-server-* --sandbox-output ./output --sandbox-ignore .sandbox-ignore --command "echo keep > /keep.txt && echo noise > /trace.log"
```

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

**Behavior:** Auto-starts MCP Streamable HTTP server on configured host/port and keeps running until shutdown.

### Page Agent Runtime Flags

Use these startup flags to configure Page Agent behavior:

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

Default server mode (`./agent-browser-server-*`) runs MCP over streamable HTTP at `/mcp`.

### Synthetic Commands

The following convenience commands are supported in command mode:

- `agent-browser agentic-open <url>`
- `agent-browser agentic-prompt <url> <prompt>`
- `agent-browser agentic-prompt <prompt>`

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
