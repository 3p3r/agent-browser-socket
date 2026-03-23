mod app;
mod bashkit_executor;
mod browser_detection;
mod command_args;
mod command_runtime;
mod configuration;
mod embedded_binary;
mod mcp;
mod page_agent_runtime;
mod runtime_shared;
mod sandbox_files;
mod screenshot;
mod server;

static LOGO_PNG: &[u8] = include_bytes!("../logo.png");

fn load_tray_icon() -> tray_icon::Icon {
    let img = image::load_from_memory(LOGO_PNG)
        .expect("logo.png embedded bytes are invalid")
        .into_rgba8();
    let (width, height) = img.dimensions();
    tray_icon::Icon::from_rgba(img.into_raw(), width, height).expect("Icon::from_rgba failed")
}

fn main() {
    let config = configuration::load_config().expect("config load failed");
    let page_agent_config = config.page_agent.clone();

    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
    let (ready_tx, ready_rx) = tokio::sync::oneshot::channel::<Result<(), String>>();

    let bg_thread = std::thread::Builder::new()
        .name("oatmeal-rt".into())
        .spawn(move || {
            let rt = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .expect("tokio runtime build failed");
            let _ = rt.block_on(app::run_with_readiness(
                config,
                page_agent_config,
                ready_tx,
                shutdown_rx,
            ));
            rt.shutdown_timeout(std::time::Duration::from_secs(5));
        })
        .expect("failed to spawn runtime thread");

    match ready_rx.blocking_recv() {
        Ok(Ok(())) => {}
        Ok(Err(error)) => {
            eprintln!("Runtime startup failed: {error}");
            std::process::exit(1);
        }
        Err(_) => {
            eprintln!("Runtime thread exited before signaling readiness");
            std::process::exit(1);
        }
    }

    #[cfg(target_os = "linux")]
    {
        if std::env::var("DISPLAY").is_err() {
            eprintln!("DISPLAY not set — tray init skipped (headless mode)");
            shutdown_tx.send(()).ok();
            std::process::exit(1);
        }

        gtk::init().expect("GTK init failed");

        let quit_item = tray_icon::menu::MenuItem::new("Quit", true, None);
        let menu = tray_icon::menu::Menu::new();
        menu.append(&quit_item).unwrap();

        let _tray = tray_icon::TrayIconBuilder::new()
            .with_icon(load_tray_icon())
            .with_tooltip("Oatmeal")
            .with_menu(Box::new(menu))
            .build()
            .expect("tray icon build failed");

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
        eprintln!("Tray event loop not implemented for this platform (deferred to Phase 5)");
        shutdown_tx.send(()).ok();
    }

    let timeout = std::time::Duration::from_secs(10);
    let start = std::time::Instant::now();
    loop {
        if bg_thread.is_finished() {
            break;
        }
        if start.elapsed() > timeout {
            eprintln!("warning: runtime thread did not exit within 10s");
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
        let _ = load_tray_icon();
    }
}
