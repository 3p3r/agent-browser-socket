# agent-browser-socket

Socket.IO wrapper for `agent-browser` that embeds the correct release binary for the target platform at build time.

## What it does

- Downloads and embeds `agent-browser` from GitHub Releases during build.
- Runs a local Socket.IO server.
- Accepts CLI commands over Socket.IO and executes the embedded `agent-browser` binary.
- Streams `stdout` and `stderr` back to the client in real time.
- Performs nginx-style auth subrequest checks per command event when configured.

## Supported platforms

- Linux: `x86_64`, `aarch64`
- macOS: `x86_64`, `aarch64`
- Windows: `x86_64`

## Config

Config is optional and loaded in this order:

1. defaults
2. `~/.abs` (fallback)
3. `./.abs` (local override)
4. `ABS_` environment variables (highest priority)

Defaults:

- `port = 9607`
- `host = "0.0.0.0"`
- `auth_url = null`
- `browser_path = null`

Example `.abs` (TOML):

```toml
auth_url = "http://127.0.0.1:8080/auth"
port = 9607
host = "0.0.0.0"
browser_path = "/usr/local/bin/agent-browser"
```

Env examples:

```bash
ABS_PORT=9607
ABS_HOST=0.0.0.0
ABS_AUTH_URL=http://127.0.0.1:8080/auth
ABS_BROWSER_PATH=/custom/path/agent-browser
```

## Run

```bash
cargo run
```

The server listens on `host:port` from config.

## Test

```bash
cargo test --test e2e -- --nocapture
```

The E2E suite uses the official Node `socket.io-client` and will auto-run `npm install --silent` on first run.

## Coverage

Install once:

```bash
cargo install cargo-tarpaulin
```

Run coverage:

```bash
cargo coverage
```

CLI version:

```bash
cargo run -- --version
# or
cargo run -- version
```

CLI passthrough to embedded/override agent-browser:

```bash
cargo run -- --command --version
cargo run -- --command open https://example.com
```

`--command` forwards every following argument to the inner `agent-browser` process and exits with the same exit code.

## HTTP endpoints

- `GET /health` -> `{ "status": "ok" }`
- `GET /version` -> `{ "version": "<wrapper-version>" }`

## Socket.IO protocol

Client emits `command`:

```json
{
	"args": ["--version"],
	"env": {
		"AGENT_BROWSER_SESSION": "my-session"
	},
	"authorization": "Bearer ...",
	"cookie": "session=..."
}
```

Or:

```json
{
	"command": "open https://example.com"
}
```

Server emits:

- `stdout` → `{ "line": "..." }`
- `stderr` → `{ "line": "..." }`
- `exit` → `{ "code": 0 }`
- `error` → `{ "status": 401, "message": "authorization denied" }`

Additional Socket.IO events:

- client emits `health` -> server emits `health` with `{ "status": "ok" }`
- client emits `version` -> server emits `version` with `{ "version": "<wrapper-version>" }`

## Auth subrequest behavior

When `auth_url` is set, every `command` event triggers a subrequest:

- 2xx: command allowed
- 401: denied (unauthorized)
- 403: denied (forbidden)
- other: treated as error

The subrequest forwards:

- `Authorization` (from event payload)
- `Cookie` (from event payload)
- `X-Original-URI: /socket.io`
