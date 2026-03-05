mod auth;
mod configuration;
mod embedded_binary;
mod server;

use crate::configuration::load_config;
use crate::embedded_binary::resolve_binary_path;
use crate::server::{build_router, AppState};
use std::ffi::OsString;
use std::process::Stdio;
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::process::Command;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    if let Some(forwarded_args) = extract_command_args() {
        if forwarded_args.is_empty() {
            eprintln!("missing forwarded arguments after --command");
            std::process::exit(2);
        }

        let config = load_config()?;
        let binary_path = resolve_binary_path(config.browser_path.as_deref())?;

        let status = Command::new(binary_path)
            .arg("--native")
            .args(forwarded_args)
            .stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .status()
            .await?;

        std::process::exit(status.code().unwrap_or(1));
    }

    if should_print_version() {
        println!("agent-browser-socket {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }

    let config = load_config()?;
    let binary_path = resolve_binary_path(config.browser_path.as_deref())?;

    let state = Arc::new(AppState {
        binary_path,
        auth_url: config.auth_url.clone(),
        http_client: reqwest::Client::new(),
    });

    let app = build_router(state);
    let listener = TcpListener::bind(format!("{}:{}", config.host, config.port)).await?;

    println!(
        "agent-browser-socket listening on {}:{}",
        config.host, config.port
    );

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    Ok(())
}

async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
}

fn should_print_version() -> bool {
    std::env::args()
        .skip(1)
        .any(|arg| matches!(arg.as_str(), "version" | "--version" | "-V"))
}

fn extract_command_args() -> Option<Vec<OsString>> {
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();
    let command_flag = OsString::from("--command");
    let index = args.iter().position(|arg| arg == &command_flag)?;
    Some(args.into_iter().skip(index + 1).collect())
}
