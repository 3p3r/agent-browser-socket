#[path = "../src/auth.rs"]
mod auth;
#[path = "../src/screenshot.rs"]
mod screenshot;
#[path = "../src/server.rs"]
mod server;

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::routing::any;
use axum::{Json, Router};
use serde_json::json;
use serde_json::Value;
use server::{build_router, AppState};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use tokio::process::Command;

#[derive(Clone)]
struct AuthState {
    status: StatusCode,
    seen: Arc<Mutex<Vec<HeaderMap>>>,
}

struct RunningServer {
    base_url: String,
    shutdown: Option<oneshot::Sender<()>>,
    handle: tokio::task::JoinHandle<()>,
}

impl RunningServer {
    async fn stop(mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        let _ = self.handle.await;
    }
}

async fn ensure_node_client_installed() {
    if std::path::Path::new("node_modules/socket.io-client").exists() {
        return;
    }

    let status = Command::new("npm")
        .args(["install", "--silent"])
        .status()
        .await
        .expect("failed to run npm install for test client");

    assert!(status.success(), "npm install failed with status {status}");
}

async fn run_socket_client(base_url: &str, event_name: &str, payload: Value) -> Vec<Value> {
    ensure_node_client_installed().await;

    let output = Command::new("node")
        .args([
            "tests/socket_client.mjs",
            base_url,
            event_name,
            &payload.to_string(),
            "2200",
        ])
        .output()
        .await
        .expect("failed to run node socket client");

    assert!(
        output.status.success(),
        "socket client failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    serde_json::from_slice(&output.stdout).expect("invalid socket client output json")
}

async fn start_auth_server(status: StatusCode) -> (RunningServer, Arc<Mutex<Vec<HeaderMap>>>) {
    let seen_headers = Arc::new(Mutex::new(Vec::new()));
    let auth_state = AuthState {
        status,
        seen: seen_headers.clone(),
    };

    async fn auth_handler(State(state): State<AuthState>, headers: HeaderMap) -> (StatusCode, Json<serde_json::Value>) {
        state.seen.lock().expect("lock poisoned").push(headers);
        (state.status, Json(json!({ "ok": true })))
    }

    let app = Router::new().route("/auth", any(auth_handler)).with_state(auth_state);
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind auth server");
    let port = listener.local_addr().expect("auth local addr").port();
    let base_url = format!("http://127.0.0.1:{port}");
    let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();

    let handle = tokio::spawn(async move {
        let _ = axum::serve(listener, app)
            .with_graceful_shutdown(async {
                let _ = shutdown_rx.await;
            })
            .await;
    });

    (
        RunningServer {
            base_url,
            shutdown: Some(shutdown_tx),
            handle,
        },
        seen_headers,
    )
}

fn create_mock_binary() -> PathBuf {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("time")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("abs-e2e-{unique}"));
    std::fs::create_dir_all(&dir).expect("create test dir");

    #[cfg(windows)]
    {
        let path = dir.join("mock-agent-browser.cmd");
        std::fs::write(
            &path,
            "@echo off\r\nif \"%1\"==\"--native\" shift\r\nif \"%1\"==\"fail\" (echo boom 1>&2 & exit /b 5)\r\n:loop\r\nif \"%1\"==\"\" goto done\r\necho %1\r\nshift\r\ngoto loop\r\n:done\r\nexit /b 0\r\n",
        )
        .expect("write mock cmd");
        return path;
    }

    #[cfg(not(windows))]
    {
        use std::os::unix::fs::PermissionsExt;

        let path = dir.join("mock-agent-browser.sh");
        std::fs::write(
            &path,
            "#!/bin/sh\nif [ \"$1\" = \"--native\" ]; then shift; fi\nif [ \"$1\" = \"fail\" ]; then echo boom 1>&2; exit 5; fi\nfor arg in \"$@\"; do\n  echo \"$arg\"\ndone\nexit 0\n",
        )
        .expect("write mock shell");
        let mut permissions = std::fs::metadata(&path).expect("metadata").permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&path, permissions).expect("chmod");
        path
    }
}

async fn start_main_server(auth_url: Option<String>) -> RunningServer {
    let state = Arc::new(AppState {
        binary_path: create_mock_binary(),
        auth_url,
        http_client: reqwest::Client::new(),
    });

    let app = build_router(state);
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind main server");
    let port = listener.local_addr().expect("main local addr").port();
    let base_url = format!("http://127.0.0.1:{port}");
    let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();

    let handle = tokio::spawn(async move {
        let _ = axum::serve(listener, app)
            .with_graceful_shutdown(async {
                let _ = shutdown_rx.await;
            })
            .await;
    });

    RunningServer {
        base_url,
        shutdown: Some(shutdown_tx),
        handle,
    }
}

#[tokio::test]
async fn socket_health_event_returns_ok() {
    let server = start_main_server(None).await;
    let events = run_socket_client(&server.base_url, "health", json!({})).await;
    server.stop().await;

    let health = events
        .iter()
        .find(|entry| entry.get("event") == Some(&json!("health")))
        .expect("health event missing");
    assert_eq!(health["data"]["status"], "ok");
}

#[tokio::test]
async fn socket_version_event_returns_version() {
    let server = start_main_server(None).await;
    let events = run_socket_client(&server.base_url, "version", json!({})).await;
    server.stop().await;

    let version = events
        .iter()
        .find(|entry| entry.get("event") == Some(&json!("version")))
        .expect("version event missing");
    assert!(version["data"]["version"].as_str().is_some());
}

#[tokio::test]
async fn socket_command_emits_stdout_and_exit() {
    let server = start_main_server(None).await;
    let events = run_socket_client(&server.base_url, "command", json!({ "args": ["hello", "world"] })).await;
    server.stop().await;

    let stdout_lines: Vec<_> = events
        .iter()
        .filter(|entry| entry.get("event") == Some(&json!("stdout")))
        .filter_map(|entry| entry["data"]["line"].as_str())
        .collect();
    assert!(stdout_lines.iter().any(|line| line.contains("hello")));
    assert!(stdout_lines.iter().any(|line| line.contains("world")));

    let exit = events
        .iter()
        .find(|entry| entry.get("event") == Some(&json!("exit")))
        .expect("exit event missing");
    assert_eq!(exit["data"]["code"], 0);
}

#[tokio::test]
async fn socket_command_empty_input_emits_error() {
    let server = start_main_server(None).await;
    let events = run_socket_client(&server.base_url, "command", json!({ "args": [] })).await;
    server.stop().await;

    let error = events
        .iter()
        .find(|entry| entry.get("event") == Some(&json!("error")))
        .expect("error event missing");
    assert_eq!(error["data"]["status"], 400);
}

#[tokio::test]
async fn socket_command_auth_skipped_without_auth_url() {
    let server = start_main_server(None).await;
    let events = run_socket_client(&server.base_url, "command", json!({ "args": ["no-auth-needed"] })).await;
    server.stop().await;

    assert!(events.iter().any(|entry| entry.get("event") == Some(&json!("exit"))));
    assert!(
        !events.iter().any(|entry| {
            entry.get("event") == Some(&json!("error"))
                && entry["data"]["message"] == "authorization denied"
        })
    );
}

#[tokio::test]
async fn socket_command_auth_401_emits_error() {
    let (auth_server, _) = start_auth_server(StatusCode::UNAUTHORIZED).await;
    let server = start_main_server(Some(format!("{}/auth", auth_server.base_url))).await;
    let events = run_socket_client(&server.base_url, "command", json!({ "args": ["blocked"] })).await;
    server.stop().await;
    auth_server.stop().await;

    let error = events
        .iter()
        .find(|entry| entry.get("event") == Some(&json!("error")))
        .expect("error event missing");
    assert_eq!(error["data"]["status"], 401);
}

#[tokio::test]
async fn socket_command_auth_403_emits_error() {
    let (auth_server, _) = start_auth_server(StatusCode::FORBIDDEN).await;
    let server = start_main_server(Some(format!("{}/auth", auth_server.base_url))).await;
    let events = run_socket_client(&server.base_url, "command", json!({ "args": ["blocked"] })).await;
    server.stop().await;
    auth_server.stop().await;

    let error = events
        .iter()
        .find(|entry| entry.get("event") == Some(&json!("error")))
        .expect("error event missing");
    assert_eq!(error["data"]["status"], 403);
}

#[tokio::test]
async fn socket_command_auth_200_allows_and_forwards_headers() {
    let (auth_server, seen_headers) = start_auth_server(StatusCode::OK).await;
    let server = start_main_server(Some(format!("{}/auth", auth_server.base_url))).await;
    let events = run_socket_client(
        &server.base_url,
        "command",
        json!({
            "args": ["allowed"],
            "authorization": "Bearer token-123",
            "cookie": "sid=abc"
        }),
    )
    .await;
    server.stop().await;
    auth_server.stop().await;

    assert!(events.iter().any(|entry| entry.get("event") == Some(&json!("exit"))));
    assert!(!events.iter().any(|entry| entry.get("event") == Some(&json!("error"))));

    let captured = seen_headers.lock().expect("lock headers");
    assert!(!captured.is_empty(), "auth endpoint should be called");

    let headers = &captured[0];
    assert_eq!(
        headers.get("authorization").and_then(|v| v.to_str().ok()),
        Some("Bearer token-123")
    );
    assert_eq!(headers.get("cookie").and_then(|v| v.to_str().ok()), Some("sid=abc"));
    assert_eq!(headers.get("x-original-uri").and_then(|v| v.to_str().ok()), Some("/socket.io"));
}

#[tokio::test]
async fn socket_command_fail_emits_stderr_and_nonzero_exit() {
    let server = start_main_server(None).await;
    let events = run_socket_client(&server.base_url, "command", json!({ "args": ["fail"] })).await;
    server.stop().await;

    let stderr = events
        .iter()
        .find(|entry| entry.get("event") == Some(&json!("stderr")))
        .expect("stderr event missing");
    assert_eq!(stderr["data"]["line"], "boom");

    let exit = events
        .iter()
        .find(|entry| entry.get("event") == Some(&json!("exit")))
        .expect("exit event missing");
    assert_eq!(exit["data"]["code"], 5);
}

#[tokio::test]
async fn socket_command_auth_500_maps_to_error_500() {
    let (auth_server, _) = start_auth_server(StatusCode::INTERNAL_SERVER_ERROR).await;
    let server = start_main_server(Some(format!("{}/auth", auth_server.base_url))).await;
    let events = run_socket_client(&server.base_url, "command", json!({ "args": ["blocked"] })).await;
    server.stop().await;
    auth_server.stop().await;

    let error = events
        .iter()
        .find(|entry| entry.get("event") == Some(&json!("error")))
        .expect("error event missing");
    assert_eq!(error["data"]["status"], 500);
}

#[tokio::test]
async fn socket_screenshot_auth_401_emits_error() {
    let (auth_server, _) = start_auth_server(StatusCode::UNAUTHORIZED).await;
    let server = start_main_server(Some(format!("{}/auth", auth_server.base_url))).await;
    let events = run_socket_client(&server.base_url, "screenshot", json!({})).await;
    server.stop().await;
    auth_server.stop().await;

    let error = events
        .iter()
        .find(|entry| entry.get("event") == Some(&json!("error")))
        .expect("error event missing");
    assert_eq!(error["data"]["status"], 401);
}

#[tokio::test]
async fn socket_screenshot_auth_200_forwards_headers_and_responds() {
    let (auth_server, seen_headers) = start_auth_server(StatusCode::OK).await;
    let server = start_main_server(Some(format!("{}/auth", auth_server.base_url))).await;
    let events = run_socket_client(
        &server.base_url,
        "screenshot",
        json!({
            "authorization": "Bearer screenshot-token",
            "cookie": "sid=screenshot"
        }),
    )
    .await;
    server.stop().await;
    auth_server.stop().await;

    let captured = seen_headers.lock().expect("lock headers");
    assert!(!captured.is_empty(), "auth endpoint should be called");

    let headers = &captured[0];
    assert_eq!(
        headers.get("authorization").and_then(|v| v.to_str().ok()),
        Some("Bearer screenshot-token")
    );
    assert_eq!(
        headers.get("cookie").and_then(|v| v.to_str().ok()),
        Some("sid=screenshot")
    );

    let screenshot_event = events
        .iter()
        .find(|entry| entry.get("event") == Some(&json!("screenshot")));
    let error_event = events
        .iter()
        .find(|entry| entry.get("event") == Some(&json!("error")));

    assert!(
        screenshot_event.is_some() || error_event.is_some(),
        "expected screenshot or error event"
    );

    if let Some(error) = error_event {
        assert_eq!(error["data"]["status"], 500);
    } else if let Some(screenshot) = screenshot_event {
        assert!(
            screenshot["data"].is_array(),
            "screenshot event should return an array of monitor screenshots"
        );
    }
}
