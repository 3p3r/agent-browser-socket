#[path = "../src/app.rs"]
mod app;
#[path = "../src/bashkit_executor.rs"]
mod bashkit_executor;
#[path = "../src/browser_detection.rs"]
mod browser_detection;
#[path = "../src/command_args.rs"]
mod command_args;
#[path = "../src/command_runtime.rs"]
mod command_runtime;
#[path = "../src/configuration.rs"]
mod configuration;
#[path = "../src/embedded_binary.rs"]
mod embedded_binary;
#[path = "../src/mcp.rs"]
mod mcp;
#[path = "../src/page_agent_runtime.rs"]
mod page_agent_runtime;
#[path = "../src/runtime_shared.rs"]
mod runtime_shared;
#[path = "../src/sandbox_files.rs"]
mod sandbox_files;
#[path = "../src/screenshot.rs"]
mod screenshot;
#[path = "../src/server.rs"]
mod server;

use configuration::AppConfig;
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::sync::oneshot;

fn touch_imported_symbols() {
    let config = configuration::AppConfig::default();
    let _ = &config.page_agent;
    let _ = configuration::load_config;
    let _ = embedded_binary::clean_cached_binary;
    let _ = runtime_shared::oatmeal_version;
    let _ = runtime_shared::oatmeal_version_text;
    let _ = runtime_shared::oatmeal_version_payload;
    let _ = runtime_shared::capture_system_screenshots;
    let _ = command_runtime::CommandExecutionMode::Mcp {
        cache_root: std::path::PathBuf::new(),
    };
    let _ = &server::URI_SCHEME;
    let _ = server::unregister_uri_scheme;
}

async fn reserve_local_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind temp listener")
        .local_addr()
        .expect("local addr")
        .port()
}

async fn start_mcp_streamable_http_server() -> (String, oneshot::Sender<()>) {
    touch_imported_symbols();

    let mut config = AppConfig::default();
    config.host = "127.0.0.1".to_string();
    config.port = reserve_local_port().await;

    let page_agent_config = configuration::PageAgentConfig::default();
    let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
    let base_url = format!("http://{}:{}", config.host, config.port);

    tokio::spawn(async move {
        let _ = mcp::run_mcp_streamable_http(
            config,
            page_agent_config,
            async move {
                let _ = shutdown_rx.await;
            },
            None,
        )
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
async fn mcp_streamable_http_initialize_returns_session_header() {
    let (base_url, shutdown) = start_mcp_streamable_http_server().await;

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
async fn mcp_streamable_http_rejects_get_without_session_id() {
    let (base_url, shutdown) = start_mcp_streamable_http_server().await;

    let response = reqwest::Client::new()
        .get(format!("{base_url}/mcp"))
        .header("Accept", "text/event-stream")
        .send()
        .await
        .expect("get request should return response");

    assert_eq!(response.status(), reqwest::StatusCode::BAD_REQUEST);

    let _ = shutdown.send(());
}
