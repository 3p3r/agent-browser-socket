use crate::auth::check_auth;
use crate::screenshot::{capture_screenshot, ScreenshotResult};
use axum::routing::get;
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::json;
use socketioxide::extract::{Data, SocketRef};
use socketioxide::SocketIo;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;

#[derive(Clone)]
pub struct AppState {
    pub binary_path: PathBuf,
    pub auth_url: Option<String>,
    pub http_client: reqwest::Client,
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

fn screenshot_response(
    screenshot_result: std::thread::Result<Result<ScreenshotResult, Box<dyn std::error::Error>>>,
) -> (&'static str, serde_json::Value) {
    match screenshot_result {
        Ok(Ok(screenshot)) => (
            "screenshot",
            json!({
                "width": screenshot.width,
                "height": screenshot.height,
                "monitor": screenshot.monitor,
                "png_base64": screenshot.png_base64
            }),
        ),
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

pub fn build_router(state: Arc<AppState>) -> Router {
    let (layer, io) = SocketIo::new_layer();

    io.ns("/", move |socket: SocketRef| {
        let state = state.clone();
        async move {
            let screenshot_state = state.clone();
            let command_state = state.clone();

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

                        let screenshot_result = std::panic::catch_unwind(capture_screenshot);

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

                    let arguments = match build_args(&payload) {
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

                    let mut command = Command::new(&state.binary_path);
                    command
                        .arg("--native")
                        .args(arguments)
                        .stdout(Stdio::piped())
                        .stderr(Stdio::piped());

                    if let Some(env) = payload.env {
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

                    let _ = socket.emit("exit", &json!({ "code": exit_code.unwrap_or(-1) }));
                }
            });
        }
    });

    Router::new()
        .route("/health", get(health_handler))
        .route("/version", get(version_handler))
        .layer(layer)
}

async fn health_handler() -> Json<serde_json::Value> {
    Json(json!({ "status": "ok" }))
}

async fn version_handler() -> Json<serde_json::Value> {
    Json(json!({ "version": env!("CARGO_PKG_VERSION") }))
}

fn build_args(payload: &CommandPayload) -> Result<Vec<String>, String> {
    if let Some(args) = &payload.args {
        if !args.is_empty() {
            return Ok(args.clone());
        }
    }

    if let Some(command) = &payload.command {
        if let Some(parsed) = shlex::split(command) {
            if !parsed.is_empty() {
                return Ok(parsed);
            }
        }
    }

    Err("provide non-empty args or command".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn screenshot_response_emits_screenshot_payload_on_success() {
        let result = ScreenshotResult {
            width: 1280,
            height: 720,
            monitor: Some("main".to_string()),
            png_base64: "abc123".to_string(),
        };

        let (event, payload) = screenshot_response(Ok(Ok(result)));

        assert_eq!(event, "screenshot");
        assert_eq!(payload["width"], 1280);
        assert_eq!(payload["height"], 720);
        assert_eq!(payload["monitor"], "main");
        assert_eq!(payload["png_base64"], "abc123");
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
        assert_eq!(payload["message"], "screenshot failed: panic in capture backend");
    }
}
