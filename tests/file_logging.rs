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
#[path = "../src/logging.rs"]
mod logging;
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

use flate2::read::GzDecoder;
use std::fs;
use std::io::Read;
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;
use tokio::net::TcpListener;
use tokio::sync::oneshot;

fn temp_log_dir(label: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "oatmeal-file-logging-tests-{}-{}-{}",
        label,
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system clock before unix epoch")
            .as_nanos()
    ));
    fs::create_dir_all(&dir).expect("create temp log dir");
    dir
}

fn run_child_scenario(name: &str, log_dir: &std::path::Path) -> std::process::Output {
    Command::new(std::env::current_exe().expect("resolve current test binary"))
        .arg("--exact")
        .arg("child_file_logging_driver")
        .arg("--nocapture")
        .env("OATMEAL_FILE_LOGGING_CHILD", name)
        .env("OATMEAL_FILE_LOG_DIR", log_dir)
        .output()
        .expect("spawn child logging scenario")
}

fn child_file_logging_creates_oatmeal_log_and_records_tracing_events(log_dir: &std::path::Path) {
    let temp_log_dir = log_dir.to_path_buf();
    let handle = logging::init_file_logging_with_options(&temp_log_dir, 1024 * 1024, 3)
        .expect("init file logging in child");

    tracing::info!(target: "oatmeal::test", "integration info line");
    tracing::error!(target: "oatmeal::test", "integration error line");

    std::thread::sleep(Duration::from_millis(200));
    drop(handle);

    let log_path = temp_log_dir.join("oatmeal.log");
    assert!(log_path.exists());

    let contents = fs::read_to_string(&log_path).expect("read active log file");
    assert!(contents.contains("INFO"));
    assert!(contents.contains("ERROR"));
    assert!(contents.contains("oatmeal::test"));
    assert!(contents.contains("integration info line"));
    assert!(contents.contains("integration error line"));
    assert!(contents.contains(" - integration info line"));
}

fn child_file_logging_rotation_retains_at_most_three_archives(log_dir: &std::path::Path) {
    let temp_log_dir = log_dir.to_path_buf();
    let handle = logging::init_file_logging_with_options(&temp_log_dir, 1024, 3)
        .expect("init file logging with low threshold");

    for index in 0..300 {
        tracing::info!(
            target: "oatmeal::rotation",
            "rotation payload {index} {}",
            "x".repeat(120)
        );
    }

    tracing::info!(
        target: "oatmeal::rotation",
        "rotation final marker {}",
        "x".repeat(64)
    );

    log::logger().flush();
    std::thread::sleep(Duration::from_millis(300));

    let active_log = temp_log_dir.join("oatmeal.log");
    for _ in 0..20 {
        if active_log.exists() {
            break;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    drop(handle);

    assert!(active_log.exists());

    let mut archives = Vec::new();
    for entry in fs::read_dir(&temp_log_dir).expect("read temp dir") {
        let entry = entry.expect("dir entry");
        let name = entry.file_name().to_string_lossy().to_string();
        if name.ends_with(".log.gz") {
            archives.push(entry.path());
        }
    }

    assert!(
        archives.len() <= 3,
        "expected <=3 archives, found {}",
        archives.len()
    );

    let has_expected_archive = ["oatmeal.1.log.gz", "oatmeal.2.log.gz", "oatmeal.3.log.gz"]
        .iter()
        .any(|expected| temp_log_dir.join(expected).exists());
    assert!(
        has_expected_archive,
        "expected at least one known archive name"
    );

    let first_archive = archives
        .first()
        .cloned()
        .expect("expected at least one gzip archive");
    let file = fs::File::open(first_archive).expect("open gzip archive");
    let mut decoder = GzDecoder::new(file);
    let mut decoded = String::new();
    decoder
        .read_to_string(&mut decoded)
        .expect("decode gzip archive text");
    assert!(decoded.contains("oatmeal::rotation"));
}

async fn reserve_local_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind temp listener")
        .local_addr()
        .expect("local addr")
        .port()
}

async fn child_file_logging_captures_existing_mcp_listen_log_call_site_async(
    log_dir: &std::path::Path,
) {
    let temp_log_dir = log_dir.to_path_buf();
    let _handle = logging::init_file_logging_with_options(&temp_log_dir, 1024 * 1024, 3)
        .expect("init file logging for mcp listen test");

    let mut config = configuration::AppConfig::default();
    config.host = "127.0.0.1".to_string();
    config.port = reserve_local_port().await;
    let base_url = format!("http://127.0.0.1:{}", config.port);

    let page_agent_config = configuration::PageAgentConfig::default();
    let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
    let expected_fragment = format!("Listening on 127.0.0.1:{}", config.port);

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

    tokio::time::sleep(Duration::from_millis(400)).await;
    let _ = shutdown_tx.send(());

    let log_path = temp_log_dir.join("oatmeal.log");
    let contents = fs::read_to_string(&log_path).expect("read mcp log output");
    assert!(contents.contains("Listening on"));
    assert!(contents.contains(&expected_fragment));
}

fn child_file_logging_captures_existing_mcp_listen_log_call_site(log_dir: &std::path::Path) {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("build single-thread runtime");
    runtime.block_on(child_file_logging_captures_existing_mcp_listen_log_call_site_async(log_dir));
}

#[test]
fn child_file_logging_driver() {
    let scenario = match std::env::var("OATMEAL_FILE_LOGGING_CHILD") {
        Ok(value) => value,
        Err(_) => return,
    };

    let log_dir = PathBuf::from(
        std::env::var("OATMEAL_FILE_LOG_DIR").expect("OATMEAL_FILE_LOG_DIR must be set"),
    );

    match scenario.as_str() {
        "basic" => child_file_logging_creates_oatmeal_log_and_records_tracing_events(&log_dir),
        "rotation" => child_file_logging_rotation_retains_at_most_three_archives(&log_dir),
        "mcp-listen" => child_file_logging_captures_existing_mcp_listen_log_call_site(&log_dir),
        other => panic!("unknown child scenario: {other}"),
    }
}

#[test]
fn file_logging_creates_oatmeal_log_and_records_tracing_events() {
    let temp_log_dir = temp_log_dir("basic");
    let output = run_child_scenario("basic", &temp_log_dir);
    assert!(
        output.status.success(),
        "child scenario failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn file_logging_rotation_retains_at_most_three_archives() {
    let temp_log_dir = temp_log_dir("rotation");
    let output = run_child_scenario("rotation", &temp_log_dir);
    assert!(
        output.status.success(),
        "child scenario failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn file_logging_captures_existing_mcp_listen_log_call_site() {
    let temp_log_dir = temp_log_dir("mcp-listen");
    let output = run_child_scenario("mcp-listen", &temp_log_dir);
    assert!(
        output.status.success(),
        "child scenario failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn file_logging_init_failure_uses_stderr_and_exits_nonzero() {
    let binary_path = std::env::var("CARGO_BIN_EXE_oatmeal")
        .expect("oatmeal binary path is set in integration tests");
    let output = Command::new(binary_path)
        .env("XDG_CACHE_HOME", "/proc/1")
        .env_remove("DISPLAY")
        .output()
        .expect("spawn oatmeal binary for init failure path");

    assert!(!output.status.success());

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("Failed to initialize file logging:"));
    assert!(!stderr.contains("http streaming:"));
}
