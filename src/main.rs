mod app;
mod bashkit_executor;
mod browser_detection;
mod command_args;
mod command_runtime;
mod configuration;
mod desktop;
mod embedded_binary;
mod logging;
mod mcp;
mod page_agent_runtime;
mod runtime_shared;
mod sandbox_files;
mod screenshot;
mod server;

use crate::command_runtime::{
    execute_prepared_command, prepare_script_command, CommandExecutionMode,
};
use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine as _;
use clap::{error::ErrorKind, Parser};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use url::Url;
use winit::application::ApplicationHandler;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};

#[derive(Parser)]
#[command(
    name = "oatmeal",
    version,
    about = "System tray MCP server with browser automation"
)]
struct Cli {
    /// Override the bind address (e.g. 127.0.0.1)
    #[arg(long)]
    host: Option<String>,

    /// Override the listen port (e.g. 9607)
    #[arg(long)]
    port: Option<u16>,

    /// Run a command instead of starting the MCP server
    #[arg(long, num_args = 1.., allow_hyphen_values = true)]
    command: Option<Vec<String>>,

    /// Register oatmeal:// URI scheme and exit
    #[arg(long, action = clap::ArgAction::SetTrue, conflicts_with = "unregister_uri")]
    register_uri: bool,

    /// Unregister oatmeal:// URI scheme and exit
    #[arg(long, action = clap::ArgAction::SetTrue, conflicts_with = "register_uri")]
    unregister_uri: bool,

    /// Print version as JSON and exit
    #[arg(long, action = clap::ArgAction::SetTrue)]
    version_json: bool,

    /// Print cache directory path as JSON and exit
    #[arg(long, action = clap::ArgAction::SetTrue)]
    cache_dir: bool,

    /// Capture system screenshots to cache directory and print paths as JSON, then exit
    #[arg(long, action = clap::ArgAction::SetTrue)]
    screenshot: bool,

    /// URI launch argument (oatmeal://...?host=X&port=Y)
    #[arg(hide = true, value_parser = parse_uri_argument)]
    uri: Option<String>,
}

fn parse_uri_argument(s: &str) -> Result<String, String> {
    let trimmed = s.trim_matches('"');
    if trimmed.starts_with(&format!("{}://", server::URI_SCHEME.unsecure())) {
        Ok(trimmed.to_string())
    } else {
        Err(format!("not an oatmeal URI: {s}"))
    }
}

static LOGO_PNG: &[u8] = include_bytes!("../logo.png");

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TrayAction {
    OpenCacheDir,
    CleanCache,
    Shutdown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TrayMenuEntry {
    label: String,
    enabled: bool,
    action: Option<TrayAction>,
}

struct TrayMenuHandles {
    open_cache_dir: tray_icon::menu::MenuItem,
    clean_cache: tray_icon::menu::MenuItem,
    shutdown: tray_icon::menu::MenuItem,
}

#[derive(Debug, Clone)]
enum TrayLoopEvent {
    Menu(tray_icon::menu::MenuEvent),
    RuntimeExited,
    ShutdownRequested,
}

fn tray_menu_entries(listen_addr: &str) -> Vec<TrayMenuEntry> {
    vec![
        TrayMenuEntry {
            label: runtime_shared::oatmeal_version_text(),
            enabled: false,
            action: None,
        },
        TrayMenuEntry {
            label: format!("Listening: {listen_addr}"),
            enabled: false,
            action: None,
        },
        TrayMenuEntry {
            label: "Open Cache Dir".to_string(),
            enabled: true,
            action: Some(TrayAction::OpenCacheDir),
        },
        TrayMenuEntry {
            label: "Clean Cache".to_string(),
            enabled: true,
            action: Some(TrayAction::CleanCache),
        },
        TrayMenuEntry {
            label: "Shutdown".to_string(),
            enabled: true,
            action: Some(TrayAction::Shutdown),
        },
    ]
}

fn build_tray_menu(listen_addr: &str) -> Result<(tray_icon::menu::Menu, TrayMenuHandles), String> {
    let entries = tray_menu_entries(listen_addr);
    let version_label = entries
        .first()
        .map(|entry| entry.label.clone())
        .ok_or_else(|| "tray menu entries are empty".to_string())?;
    let listen_label = entries
        .get(1)
        .map(|entry| entry.label.clone())
        .ok_or_else(|| "tray menu listening entry is missing".to_string())?;
    let version = tray_icon::menu::MenuItem::new(&version_label, false, None);
    let listening = tray_icon::menu::MenuItem::new(&listen_label, false, None);
    let open_cache_dir = tray_icon::menu::MenuItem::new("Open Cache Dir", true, None);
    let clean_cache = tray_icon::menu::MenuItem::new("Clean Cache", true, None);
    let shutdown = tray_icon::menu::MenuItem::new("Shutdown", true, None);

    let menu = tray_icon::menu::Menu::new();
    menu.append(&version)
        .map_err(|error| format!("tray menu append failed for version label: {error}"))?;
    menu.append(&listening)
        .map_err(|error| format!("tray menu append failed for listening label: {error}"))?;
    menu.append(&open_cache_dir)
        .map_err(|error| format!("tray menu append failed for open cache dir: {error}"))?;
    menu.append(&clean_cache)
        .map_err(|error| format!("tray menu append failed for clean cache: {error}"))?;
    menu.append(&shutdown)
        .map_err(|error| format!("tray menu append failed for shutdown: {error}"))?;

    Ok((
        menu,
        TrayMenuHandles {
            open_cache_dir,
            clean_cache,
            shutdown,
        },
    ))
}

fn request_shutdown(shutdown_tx: &mut Option<tokio::sync::oneshot::Sender<()>>) -> bool {
    match shutdown_tx.take() {
        Some(tx) => tx.send(()).is_ok(),
        None => false,
    }
}

fn dispatch_tray_action(action: TrayAction) -> Result<(), String> {
    match action {
        TrayAction::OpenCacheDir => open::that(runtime_shared::oatmeal_cache_dir())
            .map_err(|error| format!("failed to open cache directory: {error}")),
        TrayAction::CleanCache => {
            let cleaned = embedded_binary::clean_cached_binary()
                .map_err(|error| format!("failed to clean cache: {error}"))?;
            let body = if cleaned {
                "Embedded browser cache cleaned."
            } else {
                "Browser cache cleaned (nothing to remove)."
            };
            desktop::show_notification("Browser cache cleaned", body)
                .map_err(|error| format!("cache clean notification failed: {error}"))
        }
        TrayAction::Shutdown => Ok(()),
    }
}

fn load_tray_icon() -> Result<tray_icon::Icon, String> {
    let img = image::load_from_memory(LOGO_PNG)
        .map_err(|error| format!("logo.png embedded bytes are invalid: {error}"))?
        .into_rgba8();
    let (width, height) = img.dimensions();
    tray_icon::Icon::from_rgba(img.into_raw(), width, height)
        .map_err(|error| format!("Icon::from_rgba failed: {error}"))
}

fn emit_stdout(message: &str) {
    use std::io::Write;
    if !message.is_empty() {
        let mut stdout = std::io::stdout().lock();
        let _ = stdout.write_all(message.as_bytes());
        let _ = stdout.flush();
    }
}

fn emit_stderr(message: &str) {
    use std::io::Write;
    if !message.is_empty() {
        let mut stderr = std::io::stderr().lock();
        let _ = stderr.write_all(message.as_bytes());
        let _ = stderr.flush();
    }
}

fn handle_version_json() -> i32 {
    let payload = runtime_shared::oatmeal_version_payload();
    emit_stdout(&format!(
        "{}\n",
        serde_json::to_string_pretty(&payload).unwrap_or_default()
    ));
    0
}

fn handle_cache_dir() -> i32 {
    let payload = runtime_shared::oatmeal_cache_dir_payload();
    emit_stdout(&format!(
        "{}\n",
        serde_json::to_string_pretty(&payload).unwrap_or_default()
    ));
    0
}

fn handle_screenshot() -> i32 {
    let screenshots = match runtime_shared::capture_system_screenshots() {
        Ok(s) => s,
        Err(error) => {
            emit_stderr(&format!("oatmeal: {error}\n"));
            return 1;
        }
    };

    let dir = runtime_shared::oatmeal_cache_dir().join("screenshots");
    if let Err(error) = std::fs::create_dir_all(&dir) {
        emit_stderr(&format!(
            "oatmeal: failed to create screenshots directory: {error}\n"
        ));
        return 1;
    }

    let mut results = Vec::new();
    for (index, s) in screenshots.iter().enumerate() {
        let path = dir.join(format!("system-monitor-{index}.png"));
        let data = match BASE64.decode(&s.png_base64) {
            Ok(d) => d,
            Err(error) => {
                emit_stderr(&format!(
                    "oatmeal: failed to decode screenshot data: {error}\n"
                ));
                return 1;
            }
        };
        if let Err(error) = std::fs::write(&path, &data) {
            emit_stderr(&format!("oatmeal: failed to write screenshot: {error}\n"));
            return 1;
        }
        results.push(serde_json::json!({
            "path": path.display().to_string(),
            "width": s.width,
            "height": s.height,
            "monitor": s.monitor,
        }));
    }

    let payload = serde_json::json!({ "screenshots": results });
    emit_stdout(&format!(
        "{}\n",
        serde_json::to_string_pretty(&payload).unwrap_or_default()
    ));
    0
}

fn maybe_handle_command_passthrough(command_args: &[String]) -> i32 {
    if command_args.is_empty() {
        emit_stderr("oatmeal: --command requires a command payload\n");
        return 2;
    }

    let script = command_args.join(" ");
    let prepared = match prepare_script_command(&script) {
        Ok(prepared) => prepared,
        Err(error) => {
            emit_stderr(&format!("oatmeal: {error}\n"));
            return 2;
        }
    };

    let page_agent_config = match configuration::load_config() {
        Ok(config) => config.page_agent,
        Err(error) => {
            emit_stderr(&format!("oatmeal: failed to load config: {error}\n"));
            return 1;
        }
    };

    let binary_path = match embedded_binary::resolve_binary_path(None) {
        Ok(path) => path,
        Err(error) => {
            emit_stderr(&format!(
                "oatmeal: failed to resolve embedded browser binary: {error}\n"
            ));
            return 1;
        }
    };

    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            emit_stderr(&format!(
                "oatmeal: failed to build command runtime: {error}\n"
            ));
            return 1;
        }
    };

    let result = runtime.block_on(execute_prepared_command(
        &binary_path,
        &page_agent_config,
        prepared,
        None,
        CommandExecutionMode::Mcp {
            cache_root: embedded_binary::cache_root_dir(),
        },
    ));

    if !result.execution.stdout.is_empty() {
        emit_stdout(&result.execution.stdout);
    }
    if !result.execution.stderr.is_empty() {
        emit_stderr(&result.execution.stderr);
    }

    result.execution.exit_code
}

fn parse_cli_or_exit() -> Cli {
    match Cli::try_parse() {
        Ok(cli) => cli,
        Err(error) => {
            let mut message = error.to_string();
            if !message.ends_with('\n') {
                message.push('\n');
            }

            let code = match error.kind() {
                ErrorKind::DisplayHelp | ErrorKind::DisplayVersion => {
                    emit_stdout(&message);
                    0
                }
                _ => {
                    emit_stderr(&message);
                    2
                }
            };

            std::process::exit(code);
        }
    }
}

fn maybe_handle_non_tray_cli_commands(cli: &Cli) -> Option<i32> {
    if cli.version_json {
        return Some(handle_version_json());
    }

    if cli.cache_dir {
        return Some(handle_cache_dir());
    }

    if cli.screenshot {
        return Some(handle_screenshot());
    }

    if let Some(command_args) = cli.command.as_ref() {
        return Some(maybe_handle_command_passthrough(command_args));
    }

    if cli.register_uri {
        return Some(match app::ensure_uri_scheme_registered() {
            Ok(()) => {
                emit_stdout("oatmeal: URI scheme registered\n");
                0
            }
            Err(error) => {
                emit_stderr(&format!(
                    "oatmeal: failed to register URI scheme: {error}\n"
                ));
                1
            }
        });
    }

    if cli.unregister_uri {
        return Some(match server::unregister_uri_scheme() {
            Ok(true) => {
                emit_stdout("oatmeal: URI scheme unregistered\n");
                0
            }
            Ok(false) => {
                emit_stdout("oatmeal: URI scheme was not registered\n");
                0
            }
            Err(error) => {
                emit_stderr(&format!(
                    "oatmeal: failed to unregister URI scheme: {error}\n"
                ));
                1
            }
        });
    }

    None
}

#[cfg(target_os = "windows")]
fn detach_console_for_tray_mode() {
    use windows_sys::Win32::System::Console::FreeConsole;

    unsafe {
        let _ = FreeConsole();
    }
}

fn apply_uri_overrides(config: &mut configuration::AppConfig, uri: &str) -> Result<(), String> {
    let parsed = Url::parse(uri).map_err(|error| format!("invalid URI launch payload: {error}"))?;

    if let Some(host) = parsed
        .query_pairs()
        .find_map(|(key, value)| (key == "host").then(|| value.into_owned()))
        .filter(|host| !host.is_empty())
    {
        config.host = host;
    }

    if let Some(port) = parsed
        .query_pairs()
        .find_map(|(key, value)| (key == "port").then(|| value.into_owned()))
    {
        config.port = port
            .parse::<u16>()
            .map_err(|error| format!("invalid URI launch port '{port}': {error}"))?;
    }

    Ok(())
}

struct WinitTrayApp {
    proxy: EventLoopProxy<TrayLoopEvent>,
    shutdown_tx: Option<tokio::sync::oneshot::Sender<()>>,
    tray_items: TrayMenuHandles,
    runtime_finished: Arc<AtomicBool>,
    runtime_exit_sent: bool,
    _tray: tray_icon::TrayIcon,
}

impl WinitTrayApp {
    fn handle_menu_event(
        &mut self,
        event: tray_icon::menu::MenuEvent,
        event_loop: &ActiveEventLoop,
    ) {
        let action = if event.id == self.tray_items.open_cache_dir.id() {
            Some(TrayAction::OpenCacheDir)
        } else if event.id == self.tray_items.clean_cache.id() {
            Some(TrayAction::CleanCache)
        } else {
            None
        };

        if let Some(action) = action {
            if let Err(error) = dispatch_tray_action(action) {
                tracing::error!(target: "oatmeal::tray", "tray action failed: {error}");
                let action_name = match action {
                    TrayAction::OpenCacheDir => "Open Cache Dir",
                    TrayAction::CleanCache => "Clean Cache",
                    TrayAction::Shutdown => "Shutdown",
                };
                if let Err(notify_error) = desktop::action_failure_notification(action_name, &error)
                {
                    tracing::error!(
                        target: "oatmeal::tray",
                        "tray failure notification failed: {notify_error}"
                    );
                }
            }
        }

        if self.runtime_finished.load(Ordering::SeqCst) {
            event_loop.exit();
        }
    }

    fn handle_shutdown(&mut self, event_loop: &ActiveEventLoop) {
        if !request_shutdown(&mut self.shutdown_tx) {
            tracing::warn!(target: "oatmeal::tray", "shutdown already requested");
        }
        event_loop.exit();
    }
}

impl ApplicationHandler<TrayLoopEvent> for WinitTrayApp {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        event_loop.set_control_flow(ControlFlow::Wait);
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: TrayLoopEvent) {
        match event {
            TrayLoopEvent::Menu(menu_event) => self.handle_menu_event(menu_event, event_loop),
            TrayLoopEvent::RuntimeExited => event_loop.exit(),
            TrayLoopEvent::ShutdownRequested => self.handle_shutdown(event_loop),
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if self.runtime_finished.load(Ordering::SeqCst) && !self.runtime_exit_sent {
            self.runtime_exit_sent = true;
            if self.proxy.send_event(TrayLoopEvent::RuntimeExited).is_err() {
                event_loop.exit();
            }
        }
    }

    fn window_event(
        &mut self,
        _event_loop: &ActiveEventLoop,
        _window_id: winit::window::WindowId,
        _event: winit::event::WindowEvent,
    ) {
    }
}

fn main() {
    let cli = parse_cli_or_exit();

    if let Some(code) = maybe_handle_non_tray_cli_commands(&cli) {
        std::process::exit(code);
    }

    #[cfg(target_os = "windows")]
    detach_console_for_tray_mode();

    let _log_handle = match logging::init_file_logging() {
        Ok(handle) => handle,
        Err(error) => {
            eprintln!("Failed to initialize file logging: {error}");
            std::process::exit(1);
        }
    };

    let mut config = match configuration::load_config() {
        Ok(config) => config,
        Err(error) => {
            tracing::error!(target: "oatmeal::startup", "config load failed: {error}");
            std::process::exit(1);
        }
    };

    if let Some(ref uri) = cli.uri {
        if let Err(error) = apply_uri_overrides(&mut config, uri) {
            tracing::error!(target: "oatmeal::startup", "URI launch parse failed: {error}");
            std::process::exit(1);
        }
    }

    if let Some(host) = cli.host {
        config.host = host;
    }
    if let Some(port) = cli.port {
        config.port = port;
    }

    let page_agent_config = config.page_agent.clone();

    let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel::<()>();
    let (ready_tx, ready_rx) =
        tokio::sync::oneshot::channel::<Result<runtime_shared::StartupReady, String>>();

    let runtime_finished = Arc::new(AtomicBool::new(false));
    let runtime_finished_worker = Arc::clone(&runtime_finished);

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
            runtime_finished_worker.store(true, Ordering::SeqCst);
        }) {
        Ok(thread) => thread,
        Err(error) => {
            tracing::error!(
                target: "oatmeal::startup",
                "failed to spawn runtime thread: {error}"
            );
            std::process::exit(1);
        }
    };

    let startup_ready = match ready_rx.blocking_recv() {
        Ok(Ok(startup_ready)) => {
            let startup_message = format!(
                "MCP available at {} ({})",
                startup_ready.display_url, startup_ready.listen_addr
            );
            if let Err(error) = desktop::show_notification("Oatmeal is running", &startup_message) {
                tracing::error!(
                    target: "oatmeal::startup",
                    "startup notification failed: {error}"
                );
            }
            startup_ready
        }
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
    };

    let mut shutdown_tx = Some(shutdown_tx);

    #[cfg(target_os = "linux")]
    if is_wsl::is_wsl()
        || (std::env::var_os("DISPLAY").is_none() && std::env::var_os("WAYLAND_DISPLAY").is_none())
    {
        eprintln!(
            "Oatmeal requires a native Linux graphical session (not WSL, and DISPLAY or WAYLAND_DISPLAY must be set)"
        );
        request_shutdown(&mut shutdown_tx);
        std::process::exit(1);
    }

    let (menu, tray_items) = match build_tray_menu(&startup_ready.listen_addr) {
        Ok(value) => value,
        Err(error) => {
            tracing::error!(target: "oatmeal::tray", "{error}");
            request_shutdown(&mut shutdown_tx);
            std::process::exit(1);
        }
    };

    let tray_icon = match load_tray_icon() {
        Ok(icon) => icon,
        Err(error) => {
            tracing::error!(target: "oatmeal::tray", "tray icon load failed: {error}");
            request_shutdown(&mut shutdown_tx);
            std::process::exit(1);
        }
    };

    let tray = match tray_icon::TrayIconBuilder::new()
        .with_icon(tray_icon)
        .with_tooltip("Oatmeal")
        .with_menu(Box::new(menu))
        .build()
    {
        Ok(tray) => tray,
        Err(error) => {
            tracing::error!(target: "oatmeal::tray", "tray icon build failed: {error}");
            request_shutdown(&mut shutdown_tx);
            std::process::exit(1);
        }
    };

    let event_loop = EventLoop::<TrayLoopEvent>::with_user_event()
        .build()
        .expect("winit event loop should build");
    let proxy = event_loop.create_proxy();
    let handler_proxy = proxy.clone();
    let shutdown_id = tray_items.shutdown.id().clone();
    tray_icon::menu::MenuEvent::set_event_handler(Some(
        move |event: tray_icon::menu::MenuEvent| {
            let forwarded = if event.id == shutdown_id {
                TrayLoopEvent::ShutdownRequested
            } else {
                TrayLoopEvent::Menu(event)
            };
            let _ = handler_proxy.send_event(forwarded);
        },
    ));

    let mut app = WinitTrayApp {
        proxy,
        shutdown_tx,
        tray_items,
        runtime_finished,
        runtime_exit_sent: false,
        _tray: tray,
    };

    if let Err(error) = event_loop.run_app(&mut app) {
        tracing::error!(target: "oatmeal::tray", "winit run_app failed: {error}");
        request_shutdown(&mut app.shutdown_tx);
    }
    tray_icon::menu::MenuEvent::set_event_handler::<fn(tray_icon::menu::MenuEvent)>(None);

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

    #[test]
    fn tray_menu_contract_matches_phase4_requirements() {
        let entries = tray_menu_entries("127.0.0.1:9607");
        let labels: Vec<&str> = entries.iter().map(|entry| entry.label.as_str()).collect();
        let version_label = runtime_shared::oatmeal_version_text();
        assert_eq!(
            labels,
            vec![
                version_label.as_str(),
                "Listening: 127.0.0.1:9607",
                "Open Cache Dir",
                "Clean Cache",
                "Shutdown"
            ]
        );
    }

    #[test]
    fn tray_version_entry_is_disabled() {
        let entries = tray_menu_entries("127.0.0.1:9607");
        assert!(!entries[0].enabled);
        assert!(entries[0].action.is_none());
        assert!(!entries[1].enabled);
        assert!(entries[1].action.is_none());
    }

    #[test]
    fn tray_shutdown_action_uses_oneshot_path() {
        let (tx, _rx) = tokio::sync::oneshot::channel::<()>();
        let mut shutdown = Some(tx);
        assert!(request_shutdown(&mut shutdown));
    }

    #[test]
    fn repeated_shutdown_action_is_benign() {
        let (tx, _rx) = tokio::sync::oneshot::channel::<()>();
        let mut shutdown = Some(tx);
        assert!(request_shutdown(&mut shutdown));
        assert!(!request_shutdown(&mut shutdown));
    }

    #[test]
    fn uri_overrides_apply_host_and_port() {
        let mut config = configuration::AppConfig::default();
        apply_uri_overrides(&mut config, "oatmeal://open?host=127.0.0.1&port=9911")
            .expect("valid URI overrides");
        assert_eq!(config.host, "127.0.0.1");
        assert_eq!(config.port, 9911);
    }

    #[test]
    fn uri_overrides_reject_invalid_port() {
        let mut config = configuration::AppConfig::default();
        let error = apply_uri_overrides(&mut config, "oatmeal://open?port=not-a-port")
            .expect_err("invalid port should fail");
        assert!(error.contains("invalid URI launch port"));
    }
}
