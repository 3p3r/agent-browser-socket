use crate::configuration::{AppConfig, PageAgentConfig};
use crate::runtime_shared::oatmeal_cache_dir_text;
use crate::server::URI_SCHEME;
use std::error::Error;
use std::future::Future;
use sysuri::UriScheme;
use tokio::sync::mpsc as tokio_mpsc;
use tokio::sync::oneshot;

fn register_uri_scheme() -> Result<(), Box<dyn Error>> {
    let executable = std::env::current_exe()?;
    let uri_scheme = UriScheme::new(URI_SCHEME.unsecure(), "Oatmeal", executable);
    sysuri::register(&uri_scheme)?;
    Ok(())
}

pub fn ensure_uri_scheme_registered() -> Result<(), Box<dyn Error>> {
    if !sysuri::is_registered(URI_SCHEME.unsecure())? {
        register_uri_scheme()?;
    }

    Ok(())
}

pub async fn run_with_readiness(
    config: AppConfig,
    page_agent_config: PageAgentConfig,
    ready_tx: oneshot::Sender<Result<(), String>>,
    shutdown_rx: oneshot::Receiver<()>,
) -> Result<(), Box<dyn Error>> {
    if let Err(error) = ensure_uri_scheme_registered() {
        let _ = ready_tx.send(Err(error.to_string()));
        return Err(error);
    }

    #[cfg(unix)]
    let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .expect("SIGTERM handler registration failed");

    #[cfg(unix)]
    let shutdown = async move {
        tokio::select! {
            _ = shutdown_rx => {}
            _ = sigterm.recv() => {}
        }
    };

    #[cfg(not(unix))]
    let shutdown = async move {
        let _ = shutdown_rx.await;
    };

    ready_tx.send(Ok(())).ok();
    crate::mcp::run_mcp_streamable_http(config, page_agent_config, shutdown).await?;
    Ok(())
}

pub async fn run_http_server_mode(
    config: AppConfig,
    page_agent_config: PageAgentConfig,
) -> Result<(), Box<dyn Error>> {
    ensure_uri_scheme_registered()?;

    let (quit_tx, mut quit_rx) = tokio_mpsc::channel::<()>(1);
    let shutdown = async move {
        tokio::select! {
            _ = shutdown_signal() => {}
            _ = quit_rx.recv() => {}
        }
    };

    run_mcp_streamable_http_with_shutdown_internal(config, page_agent_config, shutdown, Some(quit_tx)).await
}

pub async fn run_mcp_streamable_http_with_shutdown_internal<F>(
    config: AppConfig,
    page_agent_config: PageAgentConfig,
    shutdown: F,
    quit_tx: Option<tokio_mpsc::Sender<()>>,
) -> Result<(), Box<dyn Error>>
where
    F: Future<Output = ()> + Send + 'static,
{
    let _ = quit_tx;
    let dashboard_host = if config.host == "0.0.0.0" {
        "localhost".to_string()
    } else {
        config.host.clone()
    };
    tracing::info!(
        target: "oatmeal::startup",
        "http streaming: http://{}:{}/mcp",
        dashboard_host,
        config.port
    );
    tracing::info!(
        target: "oatmeal::startup",
        "cache folder: {}",
        oatmeal_cache_dir_text()
    );

    crate::mcp::run_mcp_streamable_http(config, page_agent_config, shutdown).await?;
    Ok(())
}

#[cfg(test)]
pub async fn run_server_with_shutdown<F>(
    config: AppConfig,
    shutdown: F,
) -> Result<(), Box<dyn Error>>
where
    F: Future<Output = ()> + Send + 'static,
{
    run_mcp_streamable_http_with_shutdown_internal(
        config,
        PageAgentConfig::default(),
        shutdown,
        None,
    )
    .await
}

pub async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
}
