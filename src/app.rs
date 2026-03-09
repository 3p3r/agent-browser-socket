use crate::configuration::{load_config, AppConfig};
use crate::embedded_binary::{clean_cached_binary, resolve_binary_path};
use crate::screenshot::capture_all_screenshots;
use crate::server::{build_router, AppState};
use crossterm::cursor;
use crossterm::execute;
use crossterm::terminal::{EnterAlternateScreen, LeaveAlternateScreen};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::prelude::*;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use std::error::Error;
use std::ffi::OsString;
use std::future::Future;
use std::io::IsTerminal;
use std::process::Stdio;
use std::sync::mpsc;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};
use sysuri::UriScheme;
use tokio::net::TcpListener;
use tokio::process::Command;
use tokio::sync::mpsc as tokio_mpsc;
use url::Url;

const URI_SCHEME: &str = "abs";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CliMode {
    Serve,
    UriLaunch(String),
    RegisterUri,
    UnregisterUri,
    Version,
    Clean,
    Screenshot,
    Mcp,
    Command(Vec<OsString>),
}

pub fn parse_cli_mode(args: &[OsString]) -> CliMode {
    let command_flag = OsString::from("--command");
    if let Some(index) = args.iter().position(|arg| arg == &command_flag) {
        let forwarded = args.iter().skip(index + 1).cloned().collect();
        return CliMode::Command(forwarded);
    }

    let mcp_mode = args
        .iter()
        .any(|arg| matches!(arg.to_string_lossy().as_ref(), "--mcp"));
    if mcp_mode {
        return CliMode::Mcp;
    }

    let register_uri = args
        .iter()
        .any(|arg| matches!(arg.to_string_lossy().as_ref(), "--register-uri"));
    if register_uri {
        return CliMode::RegisterUri;
    }

    let unregister_uri = args
        .iter()
        .any(|arg| matches!(arg.to_string_lossy().as_ref(), "--unregister-uri"));
    if unregister_uri {
        return CliMode::UnregisterUri;
    }

    let show_version = args.iter().any(|arg| {
        matches!(
            arg.to_string_lossy().as_ref(),
            "version" | "--version" | "-V"
        )
    });

    let take_screenshot = args
        .iter()
        .any(|arg| matches!(arg.to_string_lossy().as_ref(), "--screenshot"));
    let clean_binary = args
        .iter()
        .any(|arg| matches!(arg.to_string_lossy().as_ref(), "--clean"));

    if clean_binary {
        return CliMode::Clean;
    }

    if take_screenshot {
        return CliMode::Screenshot;
    }

    if show_version {
        CliMode::Version
    } else if let Some(uri) = args.iter().find_map(|arg| {
        let candidate = arg.to_string_lossy();
        if candidate.contains("://") {
            Some(candidate.to_string())
        } else {
            None
        }
    }) {
        CliMode::UriLaunch(uri)
    } else {
        CliMode::Serve
    }
}

fn register_uri_scheme() -> Result<(), Box<dyn Error>> {
    let executable = std::env::current_exe()?;
    let uri_scheme = UriScheme::new(URI_SCHEME, "Agent Browser Socket", executable);
    sysuri::register(&uri_scheme)?;
    Ok(())
}

fn ensure_uri_scheme_registered() -> Result<(), Box<dyn Error>> {
    if !sysuri::is_registered(URI_SCHEME)? {
        register_uri_scheme()?;
    }

    Ok(())
}

fn apply_uri_overrides(config: &mut AppConfig, uri: &str) -> Result<(), Box<dyn Error>> {
    let parsed = Url::parse(uri)?;

    if let Some(host) = parsed
        .query_pairs()
        .find_map(|(key, value)| (key == "host").then(|| value.into_owned()))
    {
        if !host.is_empty() {
            config.host = host;
        }
    }

    if let Some(port) = parsed
        .query_pairs()
        .find_map(|(key, value)| (key == "port").then(|| value.into_owned()))
    {
        config.port = port.parse::<u16>()?;
    }

    Ok(())
}

struct IdleAnimationGuard {
    stop_tx: mpsc::Sender<()>,
    worker: Option<thread::JoinHandle<()>>,
}

impl IdleAnimationGuard {
    fn start(host: &str, port: u16) -> Option<Self> {
        let tui_disabled = std::env::var("ABS_DISABLE_TUI")
            .map(|value| matches!(value.as_str(), "1" | "true" | "TRUE" | "yes" | "YES"))
            .unwrap_or(false);
        if tui_disabled {
            return None;
        }

        if !std::io::stdout().is_terminal() {
            return None;
        }

        let host = host.to_string();
        let (stop_tx, stop_rx) = mpsc::channel::<()>();
        let worker = thread::spawn(move || {
            run_idle_animation_loop(&host, port, stop_rx);
        });

        Some(Self {
            stop_tx,
            worker: Some(worker),
        })
    }

    fn stop(mut self) {
        let _ = self.stop_tx.send(());
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn run_idle_animation_loop(host: &str, port: u16, stop_rx: mpsc::Receiver<()>) {
    let mut stdout = std::io::stdout();
    if execute!(stdout, EnterAlternateScreen, cursor::Hide).is_err() {
        return;
    }

    let backend = CrosstermBackend::new(stdout);
    let mut terminal = match Terminal::new(backend) {
        Ok(terminal) => terminal,
        Err(_) => {
            return;
        }
    };

    let start = Instant::now();
    let spinner = ["◜", "◠", "◝", "◞", "◡", "◟"];
    let pulse = [
        "▁", "▂", "▃", "▄", "▅", "▆", "▇", "█", "▇", "▆", "▅", "▄", "▃", "▂",
    ];

    loop {
        if stop_rx.try_recv().is_ok() {
            break;
        }

        let elapsed = start.elapsed();
        let spinner_index = ((elapsed.as_millis() / 140) as usize) % spinner.len();
        let pulse_index = ((elapsed.as_millis() / 90) as usize) % pulse.len();
        let wave_offset = ((elapsed.as_millis() / 100) as usize) % pulse.len();
        let wave = (0..24)
            .map(|index| pulse[(index + wave_offset) % pulse.len()])
            .collect::<String>();

        let title = format!(" agent-browser-socket {} ", spinner[spinner_index]);
        let subtitle = format!("Listening on {host}:{port}");
        let uptime = format!("uptime {}s", elapsed.as_secs());
        let energy = format!("{}{}{}", pulse[pulse_index], wave, pulse[pulse_index]);

        if terminal
            .draw(|frame| {
                let area = frame.area();
                let card = centered_rect(area, 78, 9);

                let status_lines = vec![
                    Line::from(Span::styled(
                        subtitle,
                        Style::default()
                            .fg(Color::Cyan)
                            .add_modifier(Modifier::BOLD),
                    )),
                    Line::from(Span::styled(
                        uptime,
                        Style::default().fg(Color::Gray).add_modifier(Modifier::DIM),
                    )),
                    Line::from(Span::styled(
                        energy,
                        Style::default().fg(Color::Blue).add_modifier(Modifier::DIM),
                    )),
                ];

                let paragraph = Paragraph::new(status_lines)
                    .block(Block::default().borders(Borders::ALL).title(title))
                    .alignment(Alignment::Center);

                frame.render_widget(paragraph, card);
            })
            .is_err()
        {
            break;
        }

        thread::sleep(Duration::from_millis(120));
    }

    let _ = terminal.show_cursor();
    let _ = execute!(std::io::stdout(), LeaveAlternateScreen, cursor::Show);
}

fn centered_rect(area: Rect, width_percent: u16, height: u16) -> Rect {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(0),
            Constraint::Length(height.min(area.height)),
            Constraint::Min(0),
        ])
        .split(area);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(((100 - width_percent) / 2).min(50)),
            Constraint::Percentage(width_percent.min(100)),
            Constraint::Percentage(((100 - width_percent) / 2).min(50)),
        ])
        .split(vertical[1])[1]
}

pub async fn run_with_args(args: Vec<OsString>) -> Result<i32, Box<dyn Error>> {
    match parse_cli_mode(&args) {
        CliMode::RegisterUri => {
            register_uri_scheme()?;
            println!("registered {}:// URI scheme", URI_SCHEME);
            Ok(0)
        }
        CliMode::UnregisterUri => {
            sysuri::unregister(URI_SCHEME)?;
            println!("unregistered {}:// URI scheme", URI_SCHEME);
            Ok(0)
        }
        CliMode::Command(forwarded_args) => {
            if forwarded_args.is_empty() {
                eprintln!("missing forwarded arguments after --command");
                return Ok(2);
            }

            let config = load_config()?;
            let binary_path = resolve_binary_path(config.browser_path.as_deref())?;
            let exit_code = run_command_passthrough(binary_path, forwarded_args).await?;
            Ok(exit_code)
        }
        CliMode::Version => {
            println!("agent-browser-socket {}", env!("CARGO_PKG_VERSION"));
            Ok(0)
        }
        CliMode::Clean => {
            if clean_cached_binary()? {
                println!("cleaned cached embedded browser binary");
            } else {
                println!("no cached embedded browser binary found");
            }

            Ok(0)
        }
        CliMode::Screenshot => {
            let screenshots = capture_all_screenshots()?;
            println!("{}", serde_json::to_string(&screenshots)?);
            Ok(0)
        }
        CliMode::Mcp => crate::mcp::run_mcp_stdio().await,
        CliMode::UriLaunch(uri) => {
            ensure_uri_scheme_registered()?;

            let mut config = load_config()?;
            apply_uri_overrides(&mut config, &uri)?;

            let (disconnect_tx, mut disconnect_rx) = tokio_mpsc::channel::<()>(1);
            let shutdown = async move {
                tokio::select! {
                    _ = shutdown_signal() => {}
                    _ = disconnect_rx.recv() => {}
                }
            };

            run_server_with_shutdown_internal(config, shutdown, Some(disconnect_tx)).await?;
            Ok(0)
        }
        CliMode::Serve => {
            ensure_uri_scheme_registered()?;
            let config = load_config()?;
            run_server_with_shutdown(config, shutdown_signal()).await?;
            Ok(0)
        }
    }
}

pub async fn run_command_passthrough(
    binary_path: std::path::PathBuf,
    forwarded_args: Vec<OsString>,
) -> Result<i32, Box<dyn Error>> {
    let status = Command::new(binary_path)
        .arg("--native")
        .args(forwarded_args)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .await?;

    Ok(status.code().unwrap_or(1))
}

pub async fn run_server_with_shutdown<F>(
    config: AppConfig,
    shutdown: F,
) -> Result<(), Box<dyn Error>>
where
    F: Future<Output = ()> + Send + 'static,
{
    run_server_with_shutdown_internal(config, shutdown, None).await
}

async fn run_server_with_shutdown_internal<F>(
    config: AppConfig,
    shutdown: F,
    disconnect_tx: Option<tokio_mpsc::Sender<()>>,
) -> Result<(), Box<dyn Error>>
where
    F: Future<Output = ()> + Send + 'static,
{
    let binary_path = resolve_binary_path(config.browser_path.as_deref())?;

    let state = Arc::new(AppState {
        binary_path,
        auth_url: config.auth_url.clone(),
        http_client: reqwest::Client::new(),
        disconnect_tx,
    });

    let app = build_router(state);
    let listener = TcpListener::bind(format!("{}:{}", config.host, config.port)).await?;

    let animation = IdleAnimationGuard::start(&config.host, config.port);
    if animation.is_none() {
        println!(
            "agent-browser-socket listening on {}:{}",
            config.host, config.port
        );
    }

    let serve_result = axum::serve(listener, app)
        .with_graceful_shutdown(shutdown)
        .await;

    if let Some(animation) = animation {
        animation.stop();
    }

    serve_result?;
    Ok(())
}

pub async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use once_cell::sync::Lazy;
    use std::ffi::OsString;
    use std::path::PathBuf;
    use std::sync::Mutex;

    static ENV_LOCK: Lazy<Mutex<()>> = Lazy::new(|| Mutex::new(()));

    fn clear_abs_env() {
        let keys: Vec<String> = std::env::vars()
            .filter_map(|(key, _)| {
                if key.starts_with("ABS_") {
                    Some(key)
                } else {
                    None
                }
            })
            .collect();

        for key in keys {
            std::env::remove_var(key);
        }
    }

    struct DirGuard {
        original: PathBuf,
    }

    impl DirGuard {
        fn enter(path: &std::path::Path) -> Self {
            let original = std::env::current_dir().expect("current dir");
            std::env::set_current_dir(path).expect("set current dir");
            Self { original }
        }
    }

    impl Drop for DirGuard {
        fn drop(&mut self) {
            let _ = std::env::set_current_dir(&self.original);
        }
    }

    fn create_mock_browser_binary() -> PathBuf {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("time")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("abs-app-{unique}"));
        std::fs::create_dir_all(&dir).expect("create temp dir");

        #[cfg(windows)]
        {
            let path = dir.join("mock-browser.cmd");
            std::fs::write(
                &path,
                "@echo off\r\nif \"%1\"==\"--native\" shift\r\n:loop\r\nif \"%1\"==\"\" goto done\r\necho %1\r\nshift\r\ngoto loop\r\n:done\r\nexit /b 0\r\n",
            )
            .expect("write cmd");
            path
        }

        #[cfg(not(windows))]
        {
            use std::os::unix::fs::PermissionsExt;

            let path = dir.join("mock-browser.sh");
            std::fs::write(
                &path,
                "#!/bin/sh\nif [ \"$1\" = \"--native\" ]; then shift; fi\nfor arg in \"$@\"; do\n  echo \"$arg\"\ndone\n",
            )
            .expect("write shell script");
            let mut permissions = std::fs::metadata(&path).expect("metadata").permissions();
            permissions.set_mode(0o755);
            std::fs::set_permissions(&path, permissions).expect("chmod");
            path
        }
    }

    #[test]
    fn parse_cli_mode_prefers_command_over_version() {
        let args = vec![
            OsString::from("--version"),
            OsString::from("--command"),
            OsString::from("open"),
        ];

        assert_eq!(
            parse_cli_mode(&args),
            CliMode::Command(vec![OsString::from("open")])
        );
    }

    #[test]
    fn parse_cli_mode_recognizes_version_aliases() {
        assert_eq!(
            parse_cli_mode(&[OsString::from("--version")]),
            CliMode::Version
        );
        assert_eq!(parse_cli_mode(&[OsString::from("-V")]), CliMode::Version);
        assert_eq!(
            parse_cli_mode(&[OsString::from("version")]),
            CliMode::Version
        );
        assert_eq!(parse_cli_mode(&[OsString::from("--clean")]), CliMode::Clean);
        assert_eq!(
            parse_cli_mode(&[OsString::from("--screenshot")]),
            CliMode::Screenshot
        );
        assert_eq!(parse_cli_mode(&[OsString::from("--mcp")]), CliMode::Mcp);
        assert_eq!(
            parse_cli_mode(&[OsString::from("--register-uri")]),
            CliMode::RegisterUri
        );
        assert_eq!(
            parse_cli_mode(&[OsString::from("--unregister-uri")]),
            CliMode::UnregisterUri
        );
        assert_eq!(
            parse_cli_mode(&[OsString::from("abs://open?port=9876")]),
            CliMode::UriLaunch("abs://open?port=9876".to_string())
        );
        assert_eq!(parse_cli_mode(&[OsString::from("serve")]), CliMode::Serve);
    }

    #[test]
    fn apply_uri_overrides_supports_partial_or_full_config() {
        let mut config = AppConfig {
            auth_url: None,
            port: 9607,
            host: "0.0.0.0".to_string(),
            browser_path: None,
        };

        apply_uri_overrides(&mut config, "abs://open?port=7777").expect("port override");
        assert_eq!(config.port, 7777);
        assert_eq!(config.host, "0.0.0.0");

        apply_uri_overrides(&mut config, "abs://open?port=8888&host=127.0.0.1")
            .expect("full override");
        assert_eq!(config.port, 8888);
        assert_eq!(config.host, "127.0.0.1");

        apply_uri_overrides(&mut config, "abs://localhost").expect("no host query override");
        assert_eq!(config.port, 8888);
        assert_eq!(config.host, "127.0.0.1");
    }

    #[tokio::test]
    async fn run_with_args_clean_returns_zero() {
        let result = run_with_args(vec![OsString::from("--clean")])
            .await
            .expect("run clean");
        assert_eq!(result, 0);
    }

    #[tokio::test]
    async fn run_with_args_returns_2_for_empty_command_passthrough() {
        let result = run_with_args(vec![OsString::from("--command")])
            .await
            .expect("run result");
        assert_eq!(result, 2);
    }

    #[tokio::test]
    async fn run_with_args_executes_command_passthrough() {
        let _guard = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        clear_abs_env();
        let mock_browser = create_mock_browser_binary();

        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("time")
            .as_nanos();
        let home = std::env::temp_dir().join(format!("abs-app-home-{unique}"));
        let cwd = std::env::temp_dir().join(format!("abs-app-cwd-{unique}"));
        std::fs::create_dir_all(&home).expect("create home");
        std::fs::create_dir_all(&cwd).expect("create cwd");

        let original_home: Option<OsString> = std::env::var_os("HOME");
        std::env::set_var("HOME", &home);
        let _cwd_guard = DirGuard::enter(&cwd);

        std::fs::write(
            cwd.join(".abs"),
            format!("browser_path = \"{}\"\n", mock_browser.display()),
        )
        .expect("write .abs");

        let result = run_with_args(vec![
            OsString::from("--command"),
            OsString::from("one"),
            OsString::from("two"),
        ])
        .await
        .expect("run passthrough");

        assert_eq!(result, 0);

        if let Some(home) = original_home {
            std::env::set_var("HOME", home);
        } else {
            std::env::remove_var("HOME");
        }

        clear_abs_env();
    }

    #[tokio::test]
    async fn run_server_with_shutdown_starts_and_exits_cleanly() {
        let mock_browser = create_mock_browser_binary();
        let config = AppConfig {
            auth_url: None,
            port: 0,
            host: "127.0.0.1".to_string(),
            browser_path: Some(mock_browser.to_string_lossy().to_string()),
        };

        let result = run_server_with_shutdown(config, async {}).await;
        assert!(
            result.is_ok(),
            "expected clean startup/shutdown for serve path"
        );
    }

    #[tokio::test]
    async fn run_server_with_shutdown_returns_error_for_invalid_bind_host() {
        let mock_browser = create_mock_browser_binary();
        let config = AppConfig {
            auth_url: None,
            port: 9607,
            host: "256.256.256.256".to_string(),
            browser_path: Some(mock_browser.to_string_lossy().to_string()),
        };

        let result = run_server_with_shutdown(config, async {}).await;
        assert!(result.is_err(), "invalid host should produce bind error");
    }
}
