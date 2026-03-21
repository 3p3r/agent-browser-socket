#[path = "../src/browser_detection.rs"]
mod browser_detection;
#[path = "../src/command_args.rs"]
mod command_args;
#[path = "../src/configuration.rs"]
mod configuration;
#[path = "../src/embedded_binary.rs"]
mod embedded_binary;
#[path = "../src/mcp.rs"]
mod mcp;
#[path = "../src/screenshot.rs"]
mod screenshot;
#[path = "../src/server.rs"]
mod server;

use configuration::AppConfig;
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::net::TcpListener;
use tokio::sync::oneshot;

fn touch_imported_symbols() {
    let config = configuration::AppConfig::default();
    let _ = (&config.auth_url, &config.page_agent);
    let _ = configuration::load_config;
    let _ = embedded_binary::clean_cached_binary;
    let _ = mcp::run_mcp_stdio;
    let _ = &server::URI_SCHEME;
    let _ = server::unregister_uri_scheme;
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
            "@echo off\r\n:loop\r\nif \"%1\"==\"\" goto done\r\necho %1\r\nshift\r\ngoto loop\r\n:done\r\nexit /b 0\r\n",
        )
        .expect("write mock cmd");
        path
    }

    #[cfg(not(windows))]
    {
        use std::os::unix::fs::PermissionsExt;

        let path = dir.join("mock-agent-browser.sh");
        std::fs::write(
            &path,
            "#!/bin/sh\nfor arg in \"$@\"; do\n  echo \"$arg\"\ndone\nexit 0\n",
        )
        .expect("write mock shell");
        let mut permissions = std::fs::metadata(&path).expect("metadata").permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&path, permissions).expect("chmod");
        path
    }
}

async fn reserve_local_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind temp listener")
        .local_addr()
        .expect("local addr")
        .port()
}

async fn start_mcp_sse_server() -> (String, oneshot::Sender<()>) {
    touch_imported_symbols();

    let mut config = AppConfig::default();
    config.host = "127.0.0.1".to_string();
    config.port = reserve_local_port().await;
    config.browser_path = Some(create_mock_binary().to_string_lossy().to_string());

    let page_agent_config = configuration::PageAgentConfig::default();
    let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
    let base_url = format!("http://{}:{}", config.host, config.port);

    tokio::spawn(async move {
        let _ = mcp::run_mcp_sse(config, page_agent_config, async move {
            let _ = shutdown_rx.await;
        })
        .await;
    });

    let client = reqwest::Client::new();
    for _ in 0..40 {
        let response = client
            .post(format!("{base_url}/mcp"))
            .header("Content-Type", "application/json")
            .header("Accept", "application/json, text/event-stream")
            .body(r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}"#)
            .send()
            .await;

        if response.is_ok() {
            break;
        }

        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    (base_url, shutdown_tx)
}

#[tokio::test]
async fn mcp_sse_initialize_returns_session_header() {
    let (base_url, shutdown) = start_mcp_sse_server().await;

    let response = reqwest::Client::new()
        .post(format!("{base_url}/mcp"))
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .body(r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-11-25","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}"#)
        .send()
        .await
        .expect("initialize request should succeed");

    assert_eq!(response.status(), reqwest::StatusCode::OK);
    assert!(response.headers().get("mcp-session-id").is_some());

    let body = response.text().await.expect("initialize response body");
    assert!(body.contains("\"result\""));

    let _ = shutdown.send(());
}

#[tokio::test]
async fn mcp_sse_rejects_get_without_session_id() {
    let (base_url, shutdown) = start_mcp_sse_server().await;

    let response = reqwest::Client::new()
        .get(format!("{base_url}/mcp"))
        .header("Accept", "text/event-stream")
        .send()
        .await
        .expect("get request should return response");

    assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);

    let _ = shutdown.send(());
}
