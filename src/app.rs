use crate::configuration::{AppConfig, PageAgentConfig};
use crate::runtime_shared::StartupReady;
use crate::server::URI_SCHEME;
use std::error::Error;
use sysuri::UriScheme;
use tokio::sync::oneshot;

fn register_uri_scheme() -> Result<(), Box<dyn Error>> {
    let executable = std::env::current_exe()?;
    let uri_scheme = UriScheme::new(URI_SCHEME.unsecure(), "Oatmeal", executable);
    sysuri::register(&uri_scheme)?;
    Ok(())
}

pub fn ensure_uri_scheme_registered() -> Result<(), Box<dyn Error>> {
    let was_registered = sysuri::is_registered(URI_SCHEME.unsecure())?;
    if !was_registered {
        register_uri_scheme()?;
        return Ok(());
    }

    if let Err(error) = register_uri_scheme() {
        tracing::warn!(
            target: "oatmeal::startup",
            "URI scheme refresh skipped: {error}"
        );
    }

    Ok(())
}

pub async fn run_with_readiness(
    config: AppConfig,
    page_agent_config: PageAgentConfig,
    ready_tx: oneshot::Sender<Result<StartupReady, String>>,
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

    crate::mcp::run_mcp_streamable_http(config, page_agent_config, shutdown, Some(ready_tx))
        .await?;
    Ok(())
}
