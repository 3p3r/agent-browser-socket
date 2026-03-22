# Oatmeal

Oatmeal is an opinionated set of MCP tools that ship in a single binary and provide an isolated and consistent virtual Bash environment for web-agents and in-browser automation workflows.

- [Quick Start](#quick-start)
- [Configuration](#configuration)
- [CLI Commands](#cli-commands)
  - [Sandbox File Output](#sandbox-file-output)
- [Advanced Features](#advanced-features)
  - [Browser Detection on Startup](#browser-detection-on-startup)
  - [URI Launch Mode](#uri-launch-mode)
  - [Page Agent Runtime Flags](#page-agent-runtime-flags)
  - [MCP Tools](#mcp-tools)
  - [Synthetic Commands](#synthetic-commands)
- [Platform Notes](#platform-notes)


Oatmeal is an integration of the following components in one convenient MCP server:

- `bashkit`: Virtual and cross platform Bash environment with file system isolation and auto-syncing of created files back to the real filesystem.
- `agent-browser`: Exposed as a shell builtin to Bashkit, allowing you to run `agent-browser` commands with full shell features like variables, pipes, redirection, and command chaining.
- `page-agent`: Injected on-demand into any page opened through `agent-browser` with `agentic-open`, enabling powerful agentic interactions with web content.
- `oatmeal://` URI handler to auto-start Oatmeal right from links in your browser and web pages.

---

## Quick Start

**1. Download** from [releases](https://github.com/3p3r/oatmeal/releases):
- Linux: `oatmeal-linux`
- macOS: `oatmeal-mac`  
- Windows: `oatmeal-windows.exe`

**2. Run:**

```bash
# Linux/macOS
chmod +x ./oatmeal-*
./oatmeal-*

# Windows
.\oatmeal-windows.exe
```

> **Mac users**: xattr -d com.apple.quarantine ./oatmeal-mac to run the binary after downloading from the Internet.

> **Windows users**: You may need to allow the app through Windows Defender or SmartScreen, look for "Run Anyway".

**3. Connect** to the MCP server at `http://localhost:9607/mcp` via Streamable HTTP.

---

## Configuration

**Defaults** (no config needed):
- Port: `9607`
- Host: `0.0.0.0`
- Browser: auto-detected

**Custom config** (optional) via `.oatmeal` file or `OATMEAL_*` env vars:

```toml
# .oatmeal or ~/.oatmeal
port = 9607
host = "0.0.0.0"
browser_path = "/usr/local/bin/agent-browser"

[page_agent]
model = "qwen3.5-plus"
url = "http://localhost:11434/v1"
key = "NA"
```

For nested `page_agent` values via env vars, use:
- `OATMEAL_PAGE_AGENT__MODEL`
- `OATMEAL_PAGE_AGENT__URL`
- `OATMEAL_PAGE_AGENT__KEY`

**Priority:** Built-in → `~/.oatmeal` → `./.oatmeal` → `OATMEAL_*` env vars → CLI flags

CLI flags always override file and env var configuration.

---

## CLI Commands

The wrapper supports HTTP server mode by default, `--mcp stdio` for stdio transport, and `--command` for shell-style command execution.

`--verbose` shows browser stdout/stderr (suppressed by default). Use `--headed` inside the forwarded `--command` script when you want a visible browser window.

```bash
# Default MCP server over Streamable HTTP
./oatmeal-*

# Explicit MCP transport selection
./oatmeal-* --mcp
./oatmeal-* --mcp sse
./oatmeal-* --mcp stdio

# Full shell command execution with agent-browser available
./oatmeal-* --command "agent-browser open https://example.com"
./oatmeal-* --command "agent-browser --headed open https://example.com"
```

### Sandbox File Output

Files created during command execution (both in the sandboxed shell and by `oatmeal` on the real filesystem) are automatically detected.

- **MCP mode**: Detected files are exposed as `resource://file/{id}` MCP resources alongside the command response.
- **CLI mode**: Use `--sandbox-output <dir>` to sync detected files into a directory.

When a command creates a new real filesystem file such as `/tmp/report.png`, the wrapper syncs it into `--sandbox-output` and removes the original temp file when possible. In-memory sandbox files are written directly to the output directory.

Sync-back filtering uses built-in ignore rules for common junk such as VCS metadata, editor swap files, and OS noise. You can layer additional gitignore-style patterns from a real file on disk with `--sandbox-ignore <path>`.

```bash
./oatmeal-* --sandbox-output ./output --command "agent-browser snapshot -i > /tmp/screenshot.json"

# Add custom ignore patterns on top of the built-in defaults
./oatmeal-* --sandbox-output ./output --sandbox-ignore .sandbox-ignore --command "echo keep > /keep.txt && echo noise > /trace.log"
```

---

## Advanced Features

### Browser Detection on Startup

On launch, Oatmeal tries to detect your local Chrome-like browser path. This saves you a download from the original `agent-browser`.

### URI Launch Mode

Default server startup and `--mcp sse` automatically register the URI handler when needed. You can also register it explicitly with `--register-uri`, then open URLs:
- `oatmeal://open?port=9911`
- `oatmeal://open?port=9911&host=127.0.0.1`

### Page Agent Runtime Flags

Use these startup flags to configure Page Agent behavior:

- `--page-agent-model` (default: `qwen3.5-plus`)
- `--page-agent-url` (default: `http://localhost:11434/v1`)
- `--page-agent-key` (default: `NA` - sent as Bearer token)

### MCP Tools

**Available tools:**

`health`, `version`, `shutdown`, `screenshot_system`, `shell_command`, `delete_resource`, `delete_all_resources`

### Synthetic Commands

The following convenience commands are supported in command mode:

- `agent-browser agentic-open <url>`
- `agent-browser agentic-prompt <url> <prompt>`
- `agent-browser agentic-prompt <prompt>`

---

## Platform Notes

- Linux: Self-extracting, works on x86_64 and aarch64, glibc 2.35+
- macOS: Universal binary (x86_64 + aarch64)
- All binaries are 64-bit
