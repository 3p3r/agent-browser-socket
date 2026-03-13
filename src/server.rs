use crate::auth::check_auth;
use crate::command_args::{
    build_args, ensure_executable_path_arg, is_open_command, strip_with_page_agent_flag,
    ExecutablePathPrefill,
};
use crate::screenshot::{capture_all_screenshots, ScreenshotResult};
use axum::http::header::CONTENT_TYPE;
use axum::http::StatusCode;
use axum::response::Html;
use axum::routing::{get, post};
use axum::{Json, Router};
use once_cell::sync::Lazy;
use secure_string::SecureString;
use serde::Deserialize;
use serde_json::json;
use socketioxide::extract::{Data, SocketRef};
use socketioxide::socket::DisconnectReason;
use socketioxide::SocketIo;
use std::error::Error as StdError;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use sysuri::UriScheme;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;
use tokio::sync::mpsc;
use tower_http::cors::{Any, CorsLayer};

pub static URI_SCHEME: Lazy<SecureString> = Lazy::new(|| SecureString::from("abs"));
const ADMIN_DASHBOARD_HTML: &str = include_str!("admin_dashboard.html");
const SOCKET_IO_CLIENT_JS: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/node_modules/socket.io-client/dist/socket.io.min.js"
));
const PAGE_AGENT_JS: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/node_modules/page-agent/dist/iife/page-agent.demo.js"
));

#[derive(Clone)]
pub struct AppState {
    pub binary_path: PathBuf,
    pub detected_browser_path: Option<PathBuf>,
    pub public_origin: String,
    pub auth_url: Option<String>,
    pub http_client: reqwest::Client,
    pub disconnect_tx: Option<mpsc::Sender<()>>,
}

#[derive(Debug, Deserialize)]
pub struct CommandPayload {
    pub command: Option<String>,
    pub args: Option<Vec<String>>,
    pub env: Option<std::collections::HashMap<String, String>>,
    pub authorization: Option<String>,
    pub cookie: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ScreenshotPayload {
    pub authorization: Option<String>,
    pub cookie: Option<String>,
}

fn is_missing_uri_scheme_error(error: &(dyn StdError + 'static)) -> bool {
    if !cfg!(windows) {
        return false;
    }

    let message = error.to_string().to_ascii_lowercase();
    message.contains("failed to find the scheme key")
        || (message.contains("scheme") && message.contains("not found"))
}

pub fn unregister_uri_scheme() -> Result<bool, Box<dyn StdError>> {
    if !sysuri::is_registered(URI_SCHEME.unsecure())? {
        return Ok(false);
    }

    match sysuri::unregister(URI_SCHEME.unsecure()) {
        Ok(()) => Ok(true),
        Err(error) if is_missing_uri_scheme_error(&error) => Ok(false),
        Err(error) => Err(Box::new(error)),
    }
}

fn screenshot_response(
    screenshot_result: std::thread::Result<
        Result<Vec<ScreenshotResult>, Box<dyn std::error::Error>>,
    >,
) -> (&'static str, serde_json::Value) {
    match screenshot_result {
        Ok(Ok(screenshots)) => ("screenshot", json!(screenshots)),
        Ok(Err(error)) => (
            "error",
            json!({
                "status": 500,
                "message": format!("screenshot failed: {error}")
            }),
        ),
        Err(_) => (
            "error",
            json!({
                "status": 500,
                "message": "screenshot failed: panic in capture backend"
            }),
        ),
    }
}

fn build_page_agent_injection_script(public_origin: &str) -> String {
    let script_url = format!("{public_origin}/assets/page-agent.demo.js");
    let serialized_script_url =
        serde_json::to_string(&script_url).unwrap_or_else(|_| "\"\"".to_string());
    format!(
        r#"(() => new Promise((resolve, reject) => {{
    const existing = document.querySelector('script[data-abs-page-agent="1"]');
    if (existing) {{
        resolve('already_loaded');
        return;
    }}
    const script = document.createElement('script');
    script.src = {serialized_script_url};
    script.dataset.absPageAgent = '1';
    script.onload = () => resolve('loaded');
    script.onerror = () => reject(new Error('failed to load page-agent script'));
    document.head.appendChild(script);
}}))()"#
    )
}

pub fn build_router(state: Arc<AppState>) -> (Router, SocketIo) {
    let (layer, io) = SocketIo::new_layer();
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);
    let client_connected = Arc::new(AtomicBool::new(false));
    let shutdown_io = io.clone();

    io.ns("/", move |socket: SocketRef| {
        let state = state.clone();
        let client_connected = client_connected.clone();
        let shutdown_io = shutdown_io.clone();
        async move {
            if state.disconnect_tx.is_some()
                && client_connected
                    .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
                    .is_err()
            {
                let _ = socket.disconnect();
                return;
            }

            let screenshot_state = state.clone();
            let command_state = state.clone();
            let shutdown_state = state.clone();

            if let Some(disconnect_tx) = state.disconnect_tx.clone() {
                let disconnect_state = client_connected.clone();
                socket.on_disconnect(move |_: DisconnectReason| {
                    let disconnect_tx = disconnect_tx.clone();
                    let disconnect_state = disconnect_state.clone();
                    async move {
                        disconnect_state.store(false, Ordering::SeqCst);
                        let _ = disconnect_tx.send(()).await;
                    }
                });
            }

            socket.on("health", |socket: SocketRef| async move {
                let _ = socket.emit(
                    "health",
                    &json!({
                        "status": "ok"
                    }),
                );
            });

            socket.on("version", |socket: SocketRef| async move {
                let _ = socket.emit(
                    "version",
                    &json!({
                        "version": env!("CARGO_PKG_VERSION")
                    }),
                );
            });

            socket.on("shutdown", move |socket: SocketRef| {
                let state = shutdown_state.clone();
                let shutdown_io = shutdown_io.clone();
                async move {
                    if let Some(disconnect_tx) = state.disconnect_tx.clone() {
                        let _ = socket.emit(
                            "shutdown",
                            &json!({
                                "status": "closing"
                            }),
                        );
                        let _ = shutdown_io.disconnect().await;
                        let _ = disconnect_tx.send(()).await;
                    } else {
                        let _ = socket.emit(
                            "error",
                            &json!({
                                "status": 403,
                                "message": "shutdown is only available in URI launch mode"
                            }),
                        );
                    }
                }
            });

            socket.on(
                "screenshot",
                move |socket: SocketRef, Data(payload): Data<ScreenshotPayload>| {
                    let state = screenshot_state.clone();
                    async move {
                        if let Err(code) = check_auth(
                            &state.http_client,
                            state.auth_url.as_deref(),
                            payload.authorization.as_deref(),
                            payload.cookie.as_deref(),
                        )
                        .await
                        {
                            let _ = socket.emit(
                                "error",
                                &json!({
                                    "status": code.as_u16(),
                                    "message": "authorization denied"
                                }),
                            );
                            return;
                        }

                        let screenshot_result = std::panic::catch_unwind(capture_all_screenshots);

                        let (event, payload) = screenshot_response(screenshot_result);
                        let _ = socket.emit(event, &payload);
                    }
                },
            );

            socket.on("command", move |socket: SocketRef, Data(payload): Data<CommandPayload>| {
                let state = command_state.clone();
                async move {
                    if let Err(code) = check_auth(
                        &state.http_client,
                        state.auth_url.as_deref(),
                        payload.authorization.as_deref(),
                        payload.cookie.as_deref(),
                    )
                    .await
                    {
                        let _ = socket.emit(
                            "error",
                            &json!({
                                "status": code.as_u16(),
                                "message": "authorization denied"
                            }),
                        );
                        return;
                    }

                    let mut arguments = match build_args(&payload.command, &payload.args) {
                        Ok(arguments) => arguments,
                        Err(message) => {
                            let _ = socket.emit(
                                "error",
                                &json!({
                                    "status": 400,
                                    "message": message
                                }),
                            );
                            return;
                        }
                    };

                    let with_page_agent = strip_with_page_agent_flag(&mut arguments);
                    let should_inject_page_agent = with_page_agent && is_open_command(&arguments);
                    let command_env = payload.env.clone();

                    let prefill = ensure_executable_path_arg(
                        &mut arguments,
                        state.detected_browser_path.as_deref(),
                    );

                    if prefill == ExecutablePathPrefill::Unavailable {
                        let _ = socket.emit(
                            "stderr",
                            &json!({
                                "line": "executable path auto-detection unavailable; run `agent-browser-socket --command install` to install a browser through this binary"
                            }),
                        );
                    }

                    let mut command = Command::new(&state.binary_path);
                    command
                        .arg("--native")
                        .args(&arguments)
                        .stdout(Stdio::piped())
                        .stderr(Stdio::piped());

                    if let Some(env) = command_env.as_ref() {
                        command.envs(env);
                    }

                    let spawned = command.spawn();
                    let mut child = match spawned {
                        Ok(child) => child,
                        Err(error) => {
                            let _ = socket.emit(
                                "error",
                                &json!({
                                    "status": 500,
                                    "message": format!("failed to spawn process: {error}")
                                }),
                            );
                            return;
                        }
                    };

                    let mut stdout_lines = child.stdout.take().map(|stdout| BufReader::new(stdout).lines());
                    let mut stderr_lines = child.stderr.take().map(|stderr| BufReader::new(stderr).lines());
                    let mut wait_fut = Box::pin(child.wait());
                    let mut exit_code: Option<i32> = None;

                    loop {
                        let stdout_done = stdout_lines.is_none();
                        let stderr_done = stderr_lines.is_none();
                        let process_done = exit_code.is_some();

                        if stdout_done && stderr_done && process_done {
                            break;
                        }

                        tokio::select! {
                            status = &mut wait_fut, if exit_code.is_none() => {
                                match status {
                                    Ok(status) => {
                                        exit_code = Some(status.code().unwrap_or(-1));
                                    }
                                    Err(error) => {
                                        let _ = socket.emit(
                                            "error",
                                            &json!({
                                                "status": 500,
                                                "message": format!("process wait failed: {error}")
                                            }),
                                        );
                                        return;
                                    }
                                }
                            }
                            line = async { stdout_lines.as_mut().unwrap().next_line().await }, if stdout_lines.is_some() => {
                                match line {
                                    Ok(Some(line)) => {
                                        let _ = socket.emit("stdout", &json!({ "line": line }));
                                    }
                                    Ok(None) => {
                                        stdout_lines = None;
                                    }
                                    Err(error) => {
                                        let _ = socket.emit(
                                            "error",
                                            &json!({
                                                "status": 500,
                                                "message": format!("stdout read failed: {error}")
                                            }),
                                        );
                                        stdout_lines = None;
                                    }
                                }
                            }
                            line = async { stderr_lines.as_mut().unwrap().next_line().await }, if stderr_lines.is_some() => {
                                match line {
                                    Ok(Some(line)) => {
                                        let _ = socket.emit("stderr", &json!({ "line": line }));
                                    }
                                    Ok(None) => {
                                        stderr_lines = None;
                                    }
                                    Err(error) => {
                                        let _ = socket.emit(
                                            "error",
                                            &json!({
                                                "status": 500,
                                                "message": format!("stderr read failed: {error}")
                                            }),
                                        );
                                        stderr_lines = None;
                                    }
                                }
                            }
                        }
                    }

                    if should_inject_page_agent && exit_code == Some(0) {
                        let _ = socket.emit(
                            "stderr",
                            &json!({ "line": "injecting page-agent after successful open" }),
                        );

                        let injection_script =
                            build_page_agent_injection_script(&state.public_origin);
                        let mut injection_command = Command::new(&state.binary_path);
                        injection_command
                            .arg("--native")
                            .arg("eval")
                            .arg(&injection_script)
                            .stdout(Stdio::piped())
                            .stderr(Stdio::piped());

                        if let Some(env) = command_env.as_ref() {
                            injection_command.envs(env);
                        }

                        match injection_command.output().await {
                            Ok(output) => {
                                let stdout = String::from_utf8_lossy(&output.stdout);
                                for line in stdout.lines() {
                                    if !line.trim().is_empty() {
                                        let _ = socket.emit(
                                            "stdout",
                                            &json!({ "line": format!("[page-agent] {line}") }),
                                        );
                                    }
                                }

                                let stderr = String::from_utf8_lossy(&output.stderr);
                                for line in stderr.lines() {
                                    if !line.trim().is_empty() {
                                        let _ = socket.emit(
                                            "stderr",
                                            &json!({ "line": format!("[page-agent] {line}") }),
                                        );
                                    }
                                }

                                if !output.status.success() {
                                    let _ = socket.emit(
                                        "error",
                                        &json!({
                                            "status": 500,
                                            "message": format!(
                                                "page-agent eval injection failed with exit code {}",
                                                output.status.code().unwrap_or(-1)
                                            )
                                        }),
                                    );
                                }
                            }
                            Err(error) => {
                                let _ = socket.emit(
                                    "error",
                                    &json!({
                                        "status": 500,
                                        "message": format!(
                                            "failed to spawn page-agent eval injection: {error}"
                                        )
                                    }),
                                );
                            }
                        }
                    }

                    let _ = socket.emit("exit", &json!({ "code": exit_code.unwrap_or(-1) }));
                }
            });
        }
    });

    (
        Router::new()
            .route("/", get(dashboard_handler))
            .route("/assets/socket.io.min.js", get(socket_io_client_handler))
            .route("/assets/page-agent.demo.js", get(page_agent_handler))
            .route("/health", get(health_handler))
            .route("/version", get(version_handler))
            .route("/register-uri", post(register_uri_handler))
            .route("/unregister-uri", post(unregister_uri_handler))
            .layer(layer)
            .layer(cors),
        io,
    )
}

async fn dashboard_handler() -> Html<&'static str> {
    Html(ADMIN_DASHBOARD_HTML)
}

async fn socket_io_client_handler() -> (
    [(axum::http::header::HeaderName, &'static str); 1],
    &'static str,
) {
    (
        [(CONTENT_TYPE, "application/javascript")],
        SOCKET_IO_CLIENT_JS,
    )
}

async fn page_agent_handler() -> (
    [(axum::http::header::HeaderName, &'static str); 1],
    &'static str,
) {
    ([(CONTENT_TYPE, "application/javascript")], PAGE_AGENT_JS)
}

async fn health_handler() -> Json<serde_json::Value> {
    Json(json!({ "status": "ok" }))
}

async fn version_handler() -> Json<serde_json::Value> {
    Json(json!({ "version": env!("CARGO_PKG_VERSION") }))
}

async fn register_uri_handler() -> (StatusCode, Json<serde_json::Value>) {
    let executable = match std::env::current_exe() {
        Ok(executable) => executable,
        Err(error) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "status": "error", "message": error.to_string() })),
            );
        }
    };

    let uri_scheme = UriScheme::new(URI_SCHEME.unsecure(), "Agent Browser Socket", executable);
    match sysuri::register(&uri_scheme) {
        Ok(()) => (
            StatusCode::OK,
            Json(json!({ "status": "registered", "scheme": URI_SCHEME.unsecure() })),
        ),
        Err(error) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "status": "error", "message": error.to_string() })),
        ),
    }
}

async fn unregister_uri_handler() -> (StatusCode, Json<serde_json::Value>) {
    match unregister_uri_scheme() {
        Ok(true) => (
            StatusCode::OK,
            Json(json!({ "status": "unregistered", "scheme": URI_SCHEME.unsecure() })),
        ),
        Ok(false) => (
            StatusCode::OK,
            Json(json!({ "status": "already_unregistered", "scheme": URI_SCHEME.unsecure() })),
        ),
        Err(error) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "status": "error", "message": error.to_string() })),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn screenshot_response_emits_screenshot_payload_on_success() {
        let result = vec![
            ScreenshotResult {
                width: 1280,
                height: 720,
                monitor: Some("main".to_string()),
                png_base64: "abc123".to_string(),
            },
            ScreenshotResult {
                width: 1920,
                height: 1080,
                monitor: Some("second".to_string()),
                png_base64: "def456".to_string(),
            },
        ];

        let (event, payload) = screenshot_response(Ok(Ok(result)));

        assert_eq!(event, "screenshot");
        assert!(payload.is_array());
        assert_eq!(payload[0]["width"], 1280);
        assert_eq!(payload[0]["height"], 720);
        assert_eq!(payload[0]["monitor"], "main");
        assert_eq!(payload[0]["png_base64"], "abc123");
        assert_eq!(payload[1]["width"], 1920);
        assert_eq!(payload[1]["height"], 1080);
        assert_eq!(payload[1]["monitor"], "second");
        assert_eq!(payload[1]["png_base64"], "def456");
    }

    #[test]
    fn screenshot_response_emits_error_payload_on_capture_error() {
        let error = std::io::Error::other("capture backend unavailable");
        let (event, payload) = screenshot_response(Ok(Err(Box::new(error))));

        assert_eq!(event, "error");
        assert_eq!(payload["status"], 500);
        assert!(payload["message"]
            .as_str()
            .expect("error message")
            .contains("capture backend unavailable"));
    }

    #[test]
    fn screenshot_response_emits_error_payload_on_panic() {
        let (event, payload) = screenshot_response(Err(Box::new("panic")));

        assert_eq!(event, "error");
        assert_eq!(payload["status"], 500);
        assert_eq!(
            payload["message"],
            "screenshot failed: panic in capture backend"
        );
    }
}
