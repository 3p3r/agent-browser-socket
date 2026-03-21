use crate::command_args::{
    has_passthrough_command, translate_agentic_open, translate_agentic_prompt,
};
use crate::configuration::{load_config, AppConfig, PageAgentConfig};
use crate::embedded_binary::{clean_cached_binary, resolve_binary_path};
use crate::screenshot::capture_all_screenshots;
use crate::server::{unregister_uri_scheme, URI_SCHEME};
use clap::{ArgAction, CommandFactory, Parser};
use coolor::Hsl;
use crossterm::cursor;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::prelude::*;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use std::error::Error;
use std::ffi::OsString;
use std::future::Future;
use std::io::IsTerminal;
use std::path::Path;
use std::process::Stdio;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};
use sysuri::UriScheme;
use tachyonfx::{fx, Duration as FxDuration, Effect, Shader};
use tokio::process::Command;
use tokio::sync::mpsc as tokio_mpsc;
use url::Url;

#[derive(Debug, Parser)]
#[command(
    name = "agent-browser-server",
    disable_help_subcommand = true,
    disable_version_flag = true
)]
struct CliArgs {
    #[arg(long, action = ArgAction::SetTrue)]
    mcp: bool,
    #[arg(long, action = ArgAction::SetTrue)]
    register_uri: bool,
    #[arg(long, action = ArgAction::SetTrue)]
    unregister_uri: bool,
    #[arg(long, short = 'V', action = ArgAction::SetTrue)]
    version: bool,
    #[arg(long, action = ArgAction::SetTrue)]
    clean: bool,
    #[arg(long, action = ArgAction::SetTrue)]
    screenshot: bool,
    #[arg(long, action = ArgAction::SetTrue)]
    verbose: bool,
    #[arg(long)]
    page_agent_model: Option<String>,
    #[arg(long)]
    page_agent_url: Option<String>,
    #[arg(long)]
    page_agent_key: Option<String>,
    #[arg()]
    input: Option<OsString>,
}

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
    Command(Vec<String>),
}

fn parse_cli_args(args: &[OsString]) -> Result<CliArgs, String> {
    let argv = std::iter::once(OsString::from("agent-browser-server")).chain(args.iter().cloned());
    CliArgs::try_parse_from(argv).map_err(|error| {
        let mut command = CliArgs::command();
        error.format(&mut command).to_string()
    })
}

pub fn build_page_agent_config(app_config: &AppConfig, args: &[OsString]) -> PageAgentConfig {
    let mut config = app_config.page_agent.clone();
    let command_flag = OsString::from("--command");
    let parse_slice = if let Some(index) = args.iter().position(|arg| arg == &command_flag) {
        &args[..index]
    } else {
        args
    };

    if let Ok(parsed) = parse_cli_args(parse_slice) {
        if let Some(model) = parsed.page_agent_model {
            config.model = model;
        }
        if let Some(url) = parsed.page_agent_url {
            config.url = url;
        }
        if let Some(key) = parsed.page_agent_key {
            config.key = key;
        }
    }

    config
}

pub fn parse_cli_mode(args: &[OsString]) -> CliMode {
    let command_flag = OsString::from("--command");
    if let Some(index) = args.iter().position(|arg| arg == &command_flag) {
        let forwarded = args
            .iter()
            .skip(index + 1)
            .map(|a| a.to_string_lossy().to_string())
            .collect();
        return CliMode::Command(forwarded);
    }

    let parsed = match parse_cli_args(args) {
        Ok(parsed) => parsed,
        Err(_) => return CliMode::Serve,
    };

    if parsed.mcp {
        return CliMode::Mcp;
    }

    if parsed.register_uri {
        return CliMode::RegisterUri;
    }

    if parsed.unregister_uri {
        return CliMode::UnregisterUri;
    }

    if parsed.clean {
        return CliMode::Clean;
    }

    if parsed.screenshot {
        return CliMode::Screenshot;
    }

    if parsed.version
        || parsed
            .input
            .as_ref()
            .map(|value| value.to_string_lossy() == "version")
            .unwrap_or(false)
    {
        return CliMode::Version;
    }

    if let Some(input) = parsed.input {
        let candidate = input.to_string_lossy();
        if candidate.contains("://") {
            return CliMode::UriLaunch(candidate.to_string());
        }
    }

    CliMode::Serve
}

fn parse_cli_verbose(args: &[OsString]) -> bool {
    let command_flag = OsString::from("--command");
    let parse_slice = if let Some(index) = args.iter().position(|arg| arg == &command_flag) {
        &args[..index]
    } else {
        args
    };

    match parse_cli_args(parse_slice) {
        Ok(parsed) => parsed.verbose,
        Err(_) => false,
    }
}

async fn run_page_agent_injection(
    binary_path: &Path,
    page_agent_config: &PageAgentConfig,
) -> Result<i32, Box<dyn Error>> {
    use crate::server::render_page_agent_bundle;

    let bundle = render_page_agent_bundle(page_agent_config);
    let max_chunk_bytes = 20_000;

    let run_eval = |script: String| async move {
        let status = Command::new(binary_path)
            .arg("eval")
            .arg(script)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await?;
        Ok::<i32, Box<dyn Error>>(status.code().unwrap_or(1))
    };

    let init_exit = run_eval("window.__absPageAgentChunks = [];".to_string()).await?;
    if init_exit != 0 {
        return Ok(init_exit);
    }

    let mut chunk_start = 0;
    while chunk_start < bundle.len() {
        let mut chunk_end = (chunk_start + max_chunk_bytes).min(bundle.len());
        while chunk_end > chunk_start && !bundle.is_char_boundary(chunk_end) {
            chunk_end -= 1;
        }

        if chunk_end == chunk_start {
            break;
        }

        let chunk = &bundle[chunk_start..chunk_end];
        let serialized_chunk = serde_json::to_string(chunk).unwrap_or_else(|_| "\"\"".to_string());
        let append_script = format!("window.__absPageAgentChunks.push({serialized_chunk});");

        let append_exit = run_eval(append_script).await?;
        if append_exit != 0 {
            return Ok(append_exit);
        }

        chunk_start = chunk_end;
    }

    let finalize_script = r#"(() => {
    if (window.PageAgent) return 'already_loaded';
    const source = (window.__absPageAgentChunks || []).join('');
    delete window.__absPageAgentChunks;
    (0, eval)(source);
    if (!window.PageAgent) throw new Error('PageAgent not found on window after eval');
    return 'loaded';
})()"#;

    run_eval(finalize_script.to_string()).await
}

async fn run_page_agent_prompt(binary_path: &Path, prompt: &str) -> Result<i32, Box<dyn Error>> {
    let script = crate::server::build_page_agent_prompt_script(prompt);
    let status = Command::new(binary_path)
        .arg("eval")
        .arg(script)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await?;

    Ok(status.code().unwrap_or(1))
}

fn register_uri_scheme() -> Result<(), Box<dyn Error>> {
    let executable = std::env::current_exe()?;
    let uri_scheme = UriScheme::new(URI_SCHEME.unsecure(), "Agent Browser Server", executable);
    sysuri::register(&uri_scheme)?;
    Ok(())
}

fn ensure_uri_scheme_registered() -> Result<(), Box<dyn Error>> {
    if !sysuri::is_registered(URI_SCHEME.unsecure())? {
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
    fn start(
        host: &str,
        port: u16,
        detected_browser_path: Option<String>,
        quit_tx: Option<tokio_mpsc::Sender<()>>,
    ) -> Option<Self> {
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
            run_idle_animation_loop(&host, port, detected_browser_path, stop_rx, quit_tx);
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

fn run_idle_animation_loop(
    host: &str,
    port: u16,
    detected_browser_path: Option<String>,
    stop_rx: mpsc::Receiver<()>,
    quit_tx: Option<tokio_mpsc::Sender<()>>,
) {
    let mut stdout = std::io::stdout();
    if enable_raw_mode().is_err() {
        return;
    }
    if execute!(stdout, EnterAlternateScreen, cursor::Hide).is_err() {
        let _ = disable_raw_mode();
        return;
    }

    let backend = CrosstermBackend::new(stdout);
    let mut terminal = match Terminal::new(backend) {
        Ok(terminal) => terminal,
        Err(_) => {
            let _ = disable_raw_mode();
            return;
        }
    };

    let start = Instant::now();
    let spinner = ["◜", "◠", "◝", "◞", "◡", "◟"];
    let mut status_message: Option<(String, Instant)> = None;
    let mut wave_effect = create_wave_effect();

    loop {
        if stop_rx.try_recv().is_ok() {
            break;
        }

        // Poll for keyboard events
        if event::poll(Duration::from_millis(120)).unwrap_or(false) {
            if let Ok(Event::Key(KeyEvent {
                code, modifiers, ..
            })) = event::read()
            {
                match code {
                    KeyCode::Char('c') | KeyCode::Char('C')
                        if modifiers.contains(KeyModifiers::CONTROL) =>
                    {
                        if let Some(tx) = &quit_tx {
                            let _ = tx.blocking_send(());
                        }
                        break;
                    }
                    KeyCode::Char('r') | KeyCode::Char('R') => match register_uri_scheme() {
                        Ok(()) => {
                            status_message =
                                Some(("✓ URI scheme registered".to_string(), Instant::now()));
                        }
                        Err(e) => {
                            status_message =
                                Some((format!("✗ Register failed: {}", e), Instant::now()));
                        }
                    },
                    KeyCode::Char('u') | KeyCode::Char('U') => match unregister_uri_scheme() {
                        Ok(true) => {
                            status_message =
                                Some(("✓ URI scheme unregistered".to_string(), Instant::now()));
                        }
                        Ok(false) => {
                            status_message = Some((
                                "✓ URI scheme already unregistered".to_string(),
                                Instant::now(),
                            ));
                        }
                        Err(e) => {
                            status_message =
                                Some((format!("✗ Unregister failed: {}", e), Instant::now()));
                        }
                    },
                    KeyCode::Char('q') | KeyCode::Char('Q') | KeyCode::Esc => {
                        if let Some(tx) = &quit_tx {
                            let _ = tx.blocking_send(());
                        }
                        break;
                    }
                    _ => {}
                }
            }
        }

        // Clear status message after 3 seconds
        if let Some((_, timestamp)) = &status_message {
            if timestamp.elapsed() > Duration::from_secs(3) {
                status_message = None;
            }
        }

        let elapsed = start.elapsed();
        let spinner_index = ((elapsed.as_millis() / 140) as usize) % spinner.len();

        let title = format!(" agent-browser-server {} ", spinner[spinner_index]);
        let subtitle = format!("Listening on {host}:{port}");
        let uptime = format!("uptime {}s", elapsed.as_secs());

        if terminal
            .draw(|frame| {
                let area = frame.area();
                let card = centered_rect(area, 78, 16);

                let mut status_lines = vec![
                    Line::from(Span::styled(
                        subtitle.clone(),
                        Style::default()
                            .fg(Color::Cyan)
                            .add_modifier(Modifier::BOLD),
                    )),
                    Line::from(Span::styled(
                        uptime.clone(),
                        Style::default().fg(Color::Gray).add_modifier(Modifier::DIM),
                    )),
                    Line::from(""),
                ];

                if let Some((msg, _)) = &status_message {
                    let style = if msg.starts_with('✓') {
                        Style::default().fg(Color::Green)
                    } else {
                        Style::default().fg(Color::Red)
                    };
                    status_lines.push(Line::from(Span::styled(msg.clone(), style)));
                } else {
                    status_lines.push(Line::from(""));
                }

                status_lines.push(Line::from(""));
                status_lines.push(Line::from(Span::styled(
                    "r=register  u=unregister  q/esc=quit",
                    Style::default().fg(Color::Gray).add_modifier(Modifier::DIM),
                )));

                status_lines.push(Line::from(""));
                let detected_line = match &detected_browser_path {
                    Some(path) => format!("browser: {path}"),
                    None => "browser: not found (run `agent-browser-server --command install`)"
                        .to_string(),
                };
                status_lines.push(Line::from(Span::styled(
                    detected_line,
                    Style::default().fg(Color::Gray).add_modifier(Modifier::DIM),
                )));

                let dashboard_host = if host == "0.0.0.0" { "localhost" } else { host };
                let dashboard_line = format!("mcp sse: http://{dashboard_host}:{port}/mcp");
                status_lines.push(Line::from(Span::styled(
                    dashboard_line,
                    Style::default().fg(Color::Gray).add_modifier(Modifier::DIM),
                )));

                let paragraph = Paragraph::new(status_lines)
                    .block(Block::default().borders(Borders::ALL).title(title.clone()))
                    .alignment(Alignment::Center);

                frame.render_widget(paragraph, card);

                let wave_area = Rect {
                    x: card.x + 2,
                    y: card.y + 5,
                    width: card.width.saturating_sub(4),
                    height: 1,
                };

                render_wave_line(frame.buffer_mut(), wave_area, elapsed.as_secs_f32());

                let fx_duration = FxDuration::from_millis(elapsed.as_millis() as u32);
                wave_effect.process(fx_duration, frame.buffer_mut(), wave_area);
            })
            .is_err()
        {
            break;
        }
    }

    let _ = terminal.show_cursor();
    let _ = disable_raw_mode();
    let _ = execute!(std::io::stdout(), LeaveAlternateScreen, cursor::Show);
}

fn create_wave_effect() -> Effect {
    fx::repeating(fx::hsl_shift_fg([60.0, 0.0, 0.0], 1000))
}

fn render_wave_line(buf: &mut ratatui::buffer::Buffer, area: Rect, time: f32) {
    let wave_chars = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];

    for x in 0..area.width {
        let cell_x = area.x + x;
        let cell_y = area.y;

        if cell_x < buf.area.right() && cell_y < buf.area.bottom() {
            let phase = (x as f32 * 0.5) + (time * 3.0);
            let wave_value = (phase.sin() + 1.0) / 2.0;
            let char_index = (wave_value * (wave_chars.len() - 1) as f32) as usize;
            let char_index = char_index.min(wave_chars.len() - 1);

            let hue = ((x as f32 / area.width as f32) * 360.0 + time * 60.0) % 360.0;
            let hsl = Hsl::new(hue, 0.7, 0.6);
            let rgb = hsl.to_rgb();
            let color = Color::Rgb(rgb.r, rgb.g, rgb.b);

            let cell = &mut buf[(cell_x, cell_y)];
            cell.set_char(wave_chars[char_index]);
            cell.set_fg(color);
        }
    }
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
    let verbose = parse_cli_verbose(&args);

    match parse_cli_mode(&args) {
        CliMode::RegisterUri => {
            register_uri_scheme()?;
            println!("registered {}:// URI scheme", URI_SCHEME.unsecure());
            Ok(0)
        }
        CliMode::UnregisterUri => {
            match unregister_uri_scheme()? {
                true => println!("unregistered {}:// URI scheme", URI_SCHEME.unsecure()),
                false => println!(
                    "{}:// URI scheme already unregistered",
                    URI_SCHEME.unsecure()
                ),
            }
            Ok(0)
        }
        CliMode::Command(mut forwarded_args) => {
            if forwarded_args.is_empty() {
                eprintln!("missing forwarded arguments after --command");
                return Ok(2);
            }

            let prompt = match translate_agentic_prompt(&mut forwarded_args) {
                Ok(prompt) => prompt,
                Err(message) => {
                    eprintln!("{message}");
                    return Ok(2);
                }
            };
            let should_inject_page_agent = match translate_agentic_open(&mut forwarded_args) {
                Ok(opened) => opened || prompt.is_some(),
                Err(message) => {
                    eprintln!("{message}");
                    return Ok(2);
                }
            };

            let config = load_config()?;
            let page_agent_config = build_page_agent_config(&config, &args);
            let binary_path = resolve_binary_path(config.browser_path.as_deref())?;
            let exit_code = if has_passthrough_command(&forwarded_args) {
                run_command_passthrough(&binary_path, forwarded_args, verbose).await?
            } else {
                0
            };

            if should_inject_page_agent && exit_code == 0 {
                let injection_exit_code =
                    run_page_agent_injection(&binary_path, &page_agent_config).await?;
                if injection_exit_code != 0 {
                    eprintln!(
                        "page-agent injection failed with exit code {}",
                        injection_exit_code
                    );
                    return Ok(injection_exit_code);
                }

                if let Some(prompt) = prompt {
                    let prompt_exit_code = run_page_agent_prompt(&binary_path, &prompt).await?;
                    if prompt_exit_code != 0 {
                        eprintln!(
                            "page-agent prompt execution failed with exit code {}",
                            prompt_exit_code
                        );
                        return Ok(prompt_exit_code);
                    }
                }
            }

            Ok(exit_code)
        }
        CliMode::Version => {
            println!("agent-browser-server {}", env!("CARGO_PKG_VERSION"));
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
        CliMode::Mcp => {
            let config = load_config()?;
            let page_agent_config = build_page_agent_config(&config, &args);
            crate::mcp::run_mcp_stdio(config, page_agent_config).await
        }
        CliMode::UriLaunch(uri) => {
            ensure_uri_scheme_registered()?;

            let mut config = load_config()?;
            let page_agent_config = build_page_agent_config(&config, &args);
            apply_uri_overrides(&mut config, &uri)?;

            let (quit_tx, mut quit_rx) = tokio_mpsc::channel::<()>(1);
            let shutdown = async move {
                tokio::select! {
                    _ = shutdown_signal() => {}
                    _ = quit_rx.recv() => {}
                }
            };

            run_mcp_sse_with_shutdown_internal(config, page_agent_config, shutdown, Some(quit_tx))
                .await?;
            Ok(0)
        }
        CliMode::Serve => {
            ensure_uri_scheme_registered()?;
            let config = load_config()?;
            let page_agent_config = build_page_agent_config(&config, &args);

            let (quit_tx, mut quit_rx) = tokio_mpsc::channel::<()>(1);
            let shutdown = async move {
                tokio::select! {
                    _ = shutdown_signal() => {}
                    _ = quit_rx.recv() => {}
                }
            };

            run_mcp_sse_with_shutdown_internal(config, page_agent_config, shutdown, Some(quit_tx))
                .await?;
            Ok(0)
        }
    }
}

pub async fn run_command_passthrough(
    binary_path: &Path,
    forwarded_args: Vec<String>,
    verbose: bool,
) -> Result<i32, Box<dyn Error>> {
    let mut command = Command::new(binary_path);
    command.args(forwarded_args).stdin(Stdio::inherit());

    if verbose {
        command.stdout(Stdio::inherit()).stderr(Stdio::inherit());
    } else {
        command.stdout(Stdio::null()).stderr(Stdio::null());
    }

    let status = command.status().await?;

    Ok(status.code().unwrap_or(1))
}

async fn run_mcp_sse_with_shutdown_internal<F>(
    config: AppConfig,
    page_agent_config: PageAgentConfig,
    shutdown: F,
    quit_tx: Option<tokio_mpsc::Sender<()>>,
) -> Result<(), Box<dyn Error>>
where
    F: Future<Output = ()> + Send + 'static,
{
    let detected_browser_path = crate::browser_detection::find_chrome_browser();

    let animation = IdleAnimationGuard::start(
        &config.host,
        config.port,
        detected_browser_path
            .as_ref()
            .map(|path| path.to_string_lossy().to_string()),
        quit_tx,
    );
    if animation.is_none() {
        println!(
            "agent-browser-server listening on {}:{}",
            config.host, config.port
        );
    }

    let serve_result = crate::mcp::run_mcp_sse(config, page_agent_config, shutdown).await;

    if let Some(animation) = animation {
        animation.stop();
    }

    serve_result?;
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
    run_mcp_sse_with_shutdown_internal(config, PageAgentConfig::default(), shutdown, None).await
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
                "@echo off\r\n:loop\r\nif \"%1\"==\"\" goto done\r\necho %1\r\nshift\r\ngoto loop\r\n:done\r\nexit /b 0\r\n",
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
                "#!/bin/sh\nfor arg in \"$@\"; do\n  echo \"$arg\"\ndone\n",
            )
            .expect("write shell script");
            let mut permissions = std::fs::metadata(&path).expect("metadata").permissions();
            permissions.set_mode(0o755);
            std::fs::set_permissions(&path, permissions).expect("chmod");
            path
        }
    }

    fn create_mock_browser_binary_with_eval_failure() -> PathBuf {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("time")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("abs-app-eval-fail-{unique}"));
        std::fs::create_dir_all(&dir).expect("create temp dir");

        #[cfg(windows)]
        {
            let path = dir.join("mock-browser-eval-fail.cmd");
            std::fs::write(
                &path,
                "@echo off\r\nif \"%1\"==\"eval\" exit /b 7\r\n:loop\r\nif \"%1\"==\"\" goto done\r\necho %1\r\nshift\r\ngoto loop\r\n:done\r\nexit /b 0\r\n",
            )
            .expect("write cmd");
            path
        }

        #[cfg(not(windows))]
        {
            use std::os::unix::fs::PermissionsExt;

            let path = dir.join("mock-browser-eval-fail.sh");
            std::fs::write(
                &path,
                "#!/bin/sh\nif [ \"$1\" = \"eval\" ]; then\n  exit 7\nfi\nfor arg in \"$@\"; do\n  echo \"$arg\"\ndone\n",
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
            CliMode::Command(vec!["open".to_string()])
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
    fn parse_cli_mode_ignores_page_agent_url_values_for_uri_detection() {
        assert_eq!(
            parse_cli_mode(&[
                OsString::from("--page-agent-url"),
                OsString::from("http://localhost:11434/v1")
            ]),
            CliMode::Serve
        );
        assert_eq!(
            parse_cli_mode(&[OsString::from("--page-agent-url=http://localhost:11434/v1")]),
            CliMode::Serve
        );
    }

    #[test]
    fn build_page_agent_config_supports_split_and_equals_forms() {
        let app_config = AppConfig::default();
        let parsed = build_page_agent_config(
            &app_config,
            &[
                OsString::from("--page-agent-model"),
                OsString::from("my-model"),
                OsString::from("--page-agent-url=http://127.0.0.1:5000/v1"),
                OsString::from("--page-agent-key"),
                OsString::from("secret-key"),
            ],
        );

        assert_eq!(parsed.model, "my-model");
        assert_eq!(parsed.url, "http://127.0.0.1:5000/v1");
        assert_eq!(parsed.key, "secret-key");
    }

    #[test]
    fn build_page_agent_config_cli_overrides_loaded_config() {
        let mut app_config = AppConfig::default();
        app_config.page_agent.model = "env-model".to_string();
        app_config.page_agent.url = "http://env.local/v1".to_string();
        app_config.page_agent.key = "env-key".to_string();

        let parsed = build_page_agent_config(
            &app_config,
            &[
                OsString::from("--page-agent-model"),
                OsString::from("cli-model"),
                OsString::from("--page-agent-url=http://cli.local/v1"),
            ],
        );

        assert_eq!(parsed.model, "cli-model");
        assert_eq!(parsed.url, "http://cli.local/v1");
        assert_eq!(parsed.key, "env-key");
    }

    #[test]
    fn parse_cli_verbose_defaults_off_and_enables_with_flag() {
        assert!(!parse_cli_verbose(&[]));
        assert!(parse_cli_verbose(&[OsString::from("--verbose")]));
        assert!(parse_cli_verbose(&[
            OsString::from("--verbose"),
            OsString::from("--command"),
            OsString::from("open"),
            OsString::from("https://example.com")
        ]));
    }

    #[test]
    fn apply_uri_overrides_supports_partial_or_full_config() {
        let mut config = AppConfig {
            auth_url: None,
            port: 9607,
            host: "0.0.0.0".to_string(),
            browser_path: None,
            page_agent: PageAgentConfig::default(),
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
    async fn run_with_args_agentic_open_fails_when_injection_fails() {
        let _guard = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        clear_abs_env();
        let mock_browser = create_mock_browser_binary_with_eval_failure();

        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("time")
            .as_nanos();
        let home = std::env::temp_dir().join(format!("abs-app-home-strict-{unique}"));
        let cwd = std::env::temp_dir().join(format!("abs-app-cwd-strict-{unique}"));
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
            OsString::from("agentic-open"),
            OsString::from("https://example.com"),
        ])
        .await
        .expect("run passthrough");

        assert_eq!(result, 7);

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
            page_agent: PageAgentConfig::default(),
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
            page_agent: PageAgentConfig::default(),
        };

        let result = run_server_with_shutdown(config, async {}).await;
        assert!(result.is_err(), "invalid host should produce bind error");
    }
}
