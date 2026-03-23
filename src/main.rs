mod app;
mod bashkit_executor;
mod browser_detection;
mod command_args;
mod command_runtime;
mod configuration;
mod embedded_binary;
mod logging;
mod mcp;
mod page_agent_runtime;
mod runtime_shared;
mod sandbox_files;
mod screenshot;
mod server;

static LOGO_PNG: &[u8] = include_bytes!("../logo.png");

fn load_tray_icon() -> Result<tray_icon::Icon, String> {
    let img = image::load_from_memory(LOGO_PNG)
        .map_err(|error| format!("logo.png embedded bytes are invalid: {error}"))?
        .into_rgba8();
    let (width, height) = img.dimensions();
    tray_icon::Icon::from_rgba(img.into_raw(), width, height)
        .map_err(|error| format!("Icon::from_rgba failed: {error}"))
}

fn main() {
    let _log_handle = match logging::init_file_logging() {
        Ok(handle) => handle,
        Err(error) => {
            eprintln!("Failed to initialize file logging: {error}");
            std::process::exit(1);
        }
    };

    let config = match configuration::load_config() {
        Ok(config) => config,
        Err(error) => {
            tracing::error!(target: "oatmeal::startup", "config load failed: {error}");
            std::process::exit(1);
        }
    };
    let page_agent_config = config.page_agent.clone();

    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
    let (ready_tx, ready_rx) = tokio::sync::oneshot::channel::<Result<(), String>>();

    let bg_thread = match std::thread::Builder::new()
        .name("oatmeal-rt".into())
        .spawn(move || {
            let rt = match tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
            {
                Ok(runtime) => runtime,
                Err(error) => {
                    tracing::error!(
                        target: "oatmeal::startup",
                        "tokio runtime build failed: {error}"
                    );
                    std::process::exit(1);
                }
            };
            let _ = rt.block_on(app::run_with_readiness(
                config,
                page_agent_config,
                ready_tx,
                shutdown_rx,
            ));
            rt.shutdown_timeout(std::time::Duration::from_secs(5));
        })
    {
        Ok(thread) => thread,
        Err(error) => {
            tracing::error!(
                target: "oatmeal::startup",
                "failed to spawn runtime thread: {error}"
            );
            std::process::exit(1);
        }
    };

    match ready_rx.blocking_recv() {
        Ok(Ok(())) => {}
        Ok(Err(error)) => {
            tracing::error!(target: "oatmeal::startup", "Runtime startup failed: {error}");
            std::process::exit(1);
        }
        Err(_) => {
            tracing::error!(
                target: "oatmeal::startup",
                "Runtime thread exited before signaling readiness"
            );
            std::process::exit(1);
        }
    }

    #[cfg(target_os = "linux")]
    {
        if std::env::var("DISPLAY").is_err() {
            tracing::error!(
                target: "oatmeal::tray",
                "DISPLAY not set - tray init skipped (headless mode)"
            );
            shutdown_tx.send(()).ok();
            std::process::exit(1);
        }

        if let Err(error) = gtk::init() {
            tracing::error!(target: "oatmeal::tray", "GTK init failed: {error}");
            shutdown_tx.send(()).ok();
            std::process::exit(1);
        }

        let quit_item = tray_icon::menu::MenuItem::new("Quit", true, None);
        let menu = tray_icon::menu::Menu::new();
        if let Err(error) = menu.append(&quit_item) {
            tracing::error!(target: "oatmeal::tray", "tray menu append failed: {error}");
            shutdown_tx.send(()).ok();
            std::process::exit(1);
        }

        let tray_icon = match load_tray_icon() {
            Ok(icon) => icon,
            Err(error) => {
                tracing::error!(target: "oatmeal::tray", "tray icon load failed: {error}");
                shutdown_tx.send(()).ok();
                std::process::exit(1);
            }
        };

        let _tray = match tray_icon::TrayIconBuilder::new()
            .with_icon(tray_icon)
            .with_tooltip("Oatmeal")
            .with_menu(Box::new(menu))
            .build()
        {
            Ok(tray) => tray,
            Err(error) => {
                tracing::error!(target: "oatmeal::tray", "tray icon build failed: {error}");
                shutdown_tx.send(()).ok();
                std::process::exit(1);
            }
        };

        loop {
            while gtk::events_pending() {
                gtk::main_iteration_do(true);
            }
            if bg_thread.is_finished() {
                break;
            }
            if let Ok(event) = tray_icon::menu::MenuEvent::receiver().try_recv() {
                if event.id == quit_item.id() {
                    shutdown_tx.send(()).ok();
                    break;
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }

    #[cfg(not(target_os = "linux"))]
    {
        tracing::warn!(
            target: "oatmeal::tray",
            "Tray event loop not implemented for this platform (deferred to Phase 5)"
        );
        shutdown_tx.send(()).ok();
    }

    let timeout = std::time::Duration::from_secs(10);
    let start = std::time::Instant::now();
    loop {
        if bg_thread.is_finished() {
            break;
        }
        if start.elapsed() > timeout {
            tracing::warn!(
                target: "oatmeal::shutdown",
                "runtime thread did not exit within 10s"
            );
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    let _ = bg_thread.join();
}

#[cfg(test)]
mod tray_icon_tests {
    use super::*;

    #[test]
    fn load_tray_icon_returns_valid_icon() {
        let _ = load_tray_icon().expect("tray icon should load from embedded asset");
    }
}
