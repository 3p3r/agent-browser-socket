use crate::bashkit_executor::{SandboxFile, SandboxFileOrigin};
use crate::command_runtime::{
    execute_prepared_command, prepare_script_command, CommandExecutionMode,
};
use crate::configuration::{load_config, AppConfig, PageAgentConfig};
use crate::embedded_binary::{clean_cached_binary, resolve_binary_path};
use crate::runtime_shared::{
    capture_system_screenshots, oatmeal_cache_dir_text, oatmeal_version_text,
};
use crate::sandbox_files::prepare_sandbox_files;
use crate::server::{unregister_uri_scheme, URI_SCHEME};
use clap::error::ErrorKind;
use clap::{ArgAction, CommandFactory, Parser, ValueEnum};
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
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};
use std::error::Error;
use std::ffi::OsString;
use std::future::Future;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};
use sysuri::UriScheme;
use tachyonfx::{fx, Duration as FxDuration, Effect, Shader};
use tokio::sync::mpsc as tokio_mpsc;
use url::Url;

#[derive(Debug, Parser)]
#[command(
    name = "oatmeal",
    about = "Single-binary MCP server with HTTP and stdio transports plus sandboxed browser shell execution",
    long_about = "Oatmeal bundles MCP server transports, a sandboxed Bash execution environment, and an embedded agent-browser runtime into one binary.",
    after_help = "Examples:\n  oatmeal --command \"agent-browser open https://example.com\"\n  oatmeal --verbose --sandbox-output ./out --command \"name=world && echo hello-\\$name > /report.txt\"",
    disable_help_subcommand = true,
    disable_version_flag = true
)]
struct CliArgs {
    #[arg(
        long,
        value_enum,
        num_args = 0..=1,
        default_missing_value = "sse",
        help = "Run MCP mode with the selected transport; bare --mcp defaults to sse"
    )]
    mcp: Option<McpProtocol>,
    #[arg(long, action = ArgAction::SetTrue, help = "Register the oatmeal:// URI scheme with the OS")]
    register_uri: bool,
    #[arg(long, action = ArgAction::SetTrue, help = "Remove the oatmeal:// URI scheme registration")]
    unregister_uri: bool,
    #[arg(long, short = 'V', action = ArgAction::SetTrue, help = "Print the oatmeal version")]
    version: bool,
    #[arg(long, action = ArgAction::SetTrue, help = "Delete the cached embedded browser binary")]
    clean: bool,
    #[arg(long, action = ArgAction::SetTrue, help = "Print the cache directory used by oatmeal")]
    cache_dir: bool,
    #[arg(long, action = ArgAction::SetTrue, help = "Capture screenshots from all system monitors")]
    screenshot: bool,
    #[arg(long, action = ArgAction::SetTrue, help = "Print forwarded browser stdout and stderr")]
    verbose: bool,
    #[arg(
        long,
        num_args = 0..,
        allow_hyphen_values = true,
        help = "Execute the CLI equivalent of the MCP shell_command tool"
    )]
    command: Option<Vec<OsString>>,
    #[arg(
        long,
        help = "Export detected sandbox files into this directory after --command completes"
    )]
    sandbox_output: Option<PathBuf>,
    #[arg(
        long,
        help = "Apply additional gitignore-style filters when exporting sandbox files"
    )]
    sandbox_ignore: Option<PathBuf>,
    #[arg(long, help = "Override the page-agent model name")]
    page_agent_model: Option<String>,
    #[arg(long, help = "Override the page-agent API base URL")]
    page_agent_url: Option<String>,
    #[arg(long, help = "Override the page-agent API key or bearer token")]
    page_agent_key: Option<String>,
    #[arg(help = "Optional oatmeal:// URI to launch, or a positional alias such as version")]
    input: Option<OsString>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum McpProtocol {
    #[value(help = "Run MCP over stdin/stdout using JSON-RPC")]
    Stdio,
    #[value(help = "Run MCP over the HTTP server endpoint at /mcp")]
    Sse,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CliMode {
    Serve,
    UriLaunch(String),
    Help(String),
    InvalidArgs(String),
    RegisterUri,
    UnregisterUri,
    Version,
    Clean,
    CacheDir,
    Screenshot,
    Mcp(McpProtocol),
    Command(Vec<String>),
}

struct CliParseError {
    kind: ErrorKind,
    message: String,
}

fn parse_cli_args(args: &[OsString]) -> Result<CliArgs, CliParseError> {
    let argv = std::iter::once(OsString::from("oatmeal")).chain(args.iter().cloned());
    CliArgs::try_parse_from(argv).map_err(|error| {
        let kind = error.kind().clone();
        let mut command = CliArgs::command();
        let message = error.format(&mut command).to_string();
        CliParseError { kind, message }
    })
}

pub struct ParsedCli {
    pub mode: CliMode,
    pub flags: CliFlags,
}

pub struct CliFlags {
    pub verbose: bool,
    pub sandbox_output: Option<PathBuf>,
    pub sandbox_ignore: Option<PathBuf>,
    pub page_agent_model: Option<String>,
    pub page_agent_url: Option<String>,
    pub page_agent_key: Option<String>,
}

pub fn parse_cli(args: &[OsString]) -> ParsedCli {
    let parsed = parse_cli_args(args);

    let mode = match &parsed {
        Ok(p) => {
            if let Some(command) = p.command.as_ref() {
                CliMode::Command(
                    command
                        .iter()
                        .map(|value| value.to_string_lossy().to_string())
                        .collect(),
                )
            } else if let Some(protocol) = p.mcp {
                CliMode::Mcp(protocol)
            } else if p.register_uri {
                CliMode::RegisterUri
            } else if p.unregister_uri {
                CliMode::UnregisterUri
            } else if p.clean {
                CliMode::Clean
            } else if p.cache_dir {
                CliMode::CacheDir
            } else if p.screenshot {
                CliMode::Screenshot
            } else if p.version
                || p.input
                    .as_ref()
                    .map(|value| value.to_string_lossy() == "version")
                    .unwrap_or(false)
            {
                CliMode::Version
            } else if p
                .input
                .as_ref()
                .map(|value| {
                    let value = value.to_string_lossy();
                    value == "cache-dir" || value == "cache_dir"
                })
                .unwrap_or(false)
            {
                CliMode::CacheDir
            } else if let Some(ref input) = p.input {
                let candidate = input.to_string_lossy();
                if candidate.contains("://") {
                    CliMode::UriLaunch(candidate.to_string())
                } else {
                    CliMode::Serve
                }
            } else {
                CliMode::Serve
            }
        }
        Err(error) if error.kind == ErrorKind::DisplayHelp => CliMode::Help(error.message.clone()),
        Err(error) => CliMode::InvalidArgs(error.message.clone()),
    };

    let parsed_ref = parsed.as_ref().ok();

    ParsedCli {
        mode,
        flags: CliFlags {
            verbose: parsed_ref
                .map(|p| p.verbose || p.command.is_some())
                .unwrap_or(false),
            sandbox_output: parsed_ref.and_then(|p| p.sandbox_output.clone()),
            sandbox_ignore: parsed_ref.and_then(|p| p.sandbox_ignore.clone()),
            page_agent_model: parsed_ref.and_then(|p| p.page_agent_model.clone()),
            page_agent_url: parsed_ref.and_then(|p| p.page_agent_url.clone()),
            page_agent_key: parsed_ref.and_then(|p| p.page_agent_key.clone()),
        },
    }
}

pub fn build_page_agent_config(app_config: &AppConfig, flags: &CliFlags) -> PageAgentConfig {
    let mut config = app_config.page_agent.clone();
    if let Some(ref model) = flags.page_agent_model {
        config.model = model.clone();
    }
    if let Some(ref url) = flags.page_agent_url {
        config.url = url.clone();
    }
    if let Some(ref key) = flags.page_agent_key {
        config.key = key.clone();
    }
    config
}

fn register_uri_scheme() -> Result<(), Box<dyn Error>> {
    let executable = std::env::current_exe()?;
    let uri_scheme = UriScheme::new(URI_SCHEME.unsecure(), "Oatmeal", executable);
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
        let tui_disabled = std::env::var("OATMEAL_DISABLE_TUI")
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

        let title = format!(" oatmeal {} ", spinner[spinner_index]);
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
                    None => "browser: not found (run `oatmeal --command install`)".to_string(),
                };
                status_lines.push(Line::from(Span::styled(
                    detected_line,
                    Style::default().fg(Color::Gray).add_modifier(Modifier::DIM),
                )));

                let dashboard_host = if host == "0.0.0.0" { "localhost" } else { host };
                let dashboard_line = format!("http streaming: http://{dashboard_host}:{port}/mcp");
                status_lines.push(Line::from(Span::styled(
                    dashboard_line,
                    Style::default().fg(Color::Gray).add_modifier(Modifier::DIM),
                )));
                status_lines.push(Line::from(Span::styled(
                    format!("dashboard: http://{dashboard_host}:{port}/"),
                    Style::default().fg(Color::Gray).add_modifier(Modifier::DIM),
                )));
                status_lines.push(Line::from(Span::styled(
                    format!("cache folder: {}", oatmeal_cache_dir_text()),
                    Style::default().fg(Color::Gray).add_modifier(Modifier::DIM),
                )));

                let paragraph = Paragraph::new(status_lines)
                    .block(Block::default().borders(Borders::ALL).title(title.clone()))
                    .alignment(Alignment::Center)
                    .wrap(Wrap { trim: true });

                frame.render_widget(Clear, card);
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
    let cli = parse_cli(&args);

    match cli.mode {
        CliMode::Help(output) => {
            print!("{output}");
            Ok(0)
        }
        CliMode::InvalidArgs(output) => {
            eprint!("{output}");
            Ok(2)
        }
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

            let config = load_config()?;
            let page_agent_config = build_page_agent_config(&config, &cli.flags);
            let binary_path = resolve_binary_path(config.browser_path.as_deref())?;
            let exit_code = run_command_passthrough(
                &binary_path,
                std::mem::take(&mut forwarded_args),
                cli.flags.verbose,
                &page_agent_config,
                cli.flags.sandbox_output.as_deref(),
                cli.flags.sandbox_ignore.as_deref(),
            )
            .await?;

            Ok(exit_code)
        }
        CliMode::Version => {
            println!("{}", oatmeal_version_text());
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
        CliMode::CacheDir => {
            println!("{}", oatmeal_cache_dir_text());
            Ok(0)
        }
        CliMode::Screenshot => {
            let screenshots = capture_system_screenshots().map_err(std::io::Error::other)?;
            println!("{}", serde_json::to_string(&screenshots)?);
            Ok(0)
        }
        CliMode::Mcp(McpProtocol::Stdio) => {
            let config = load_config()?;
            let page_agent_config = build_page_agent_config(&config, &cli.flags);
            crate::mcp::run_mcp_stdio(config, page_agent_config).await
        }
        CliMode::Mcp(McpProtocol::Sse) => {
            let config = load_config()?;
            let page_agent_config = build_page_agent_config(&config, &cli.flags);
            run_http_server_mode(config, page_agent_config).await?;
            Ok(0)
        }
        CliMode::UriLaunch(uri) => {
            ensure_uri_scheme_registered()?;

            let mut config = load_config()?;
            let page_agent_config = build_page_agent_config(&config, &cli.flags);
            apply_uri_overrides(&mut config, &uri)?;

            let (quit_tx, mut quit_rx) = tokio_mpsc::channel::<()>(1);
            let shutdown = async move {
                tokio::select! {
                    _ = shutdown_signal() => {}
                    _ = quit_rx.recv() => {}
                }
            };

            run_mcp_streamable_http_with_shutdown_internal(
                config,
                page_agent_config,
                shutdown,
                Some(quit_tx),
            )
            .await?;
            Ok(0)
        }
        CliMode::Serve => {
            let config = load_config()?;
            let page_agent_config = build_page_agent_config(&config, &cli.flags);
            run_http_server_mode(config, page_agent_config).await?;
            Ok(0)
        }
    }
}

async fn run_http_server_mode(
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

    run_mcp_streamable_http_with_shutdown_internal(
        config,
        page_agent_config,
        shutdown,
        Some(quit_tx),
    )
    .await
}

pub async fn run_command_passthrough(
    binary_path: &Path,
    forwarded_args: Vec<String>,
    verbose: bool,
    page_agent_config: &PageAgentConfig,
    sandbox_output: Option<&Path>,
    sandbox_ignore: Option<&Path>,
) -> Result<i32, Box<dyn Error>> {
    let original_script = if forwarded_args.len() == 1 {
        forwarded_args[0].clone()
    } else {
        shlex::try_join(forwarded_args.iter().map(|part| part.as_str()))
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e.to_string()))?
    };
    let prepared = prepare_script_command(&original_script)
        .map_err(|message| std::io::Error::new(std::io::ErrorKind::InvalidInput, message))?;

    let result = execute_prepared_command(
        binary_path,
        page_agent_config,
        prepared,
        None,
        CommandExecutionMode::Cli,
    )
    .await;
    let stdout = result.execution.stdout;
    let stderr = result.execution.stderr;
    let exit_code = result.execution.exit_code;

    if let Some(output_dir) = sandbox_output {
        export_sandbox_files(output_dir, sandbox_ignore, &result.execution.files, verbose)?;
    }

    if verbose {
        if !stdout.is_empty() {
            print!("{stdout}");
        }
        if !stderr.is_empty() {
            eprint!("{stderr}");
        }
    }

    if let Some(injection) = result.page_agent_injection {
        if injection.exit_code != 0 {
            if verbose {
                if !injection.stderr.is_empty() {
                    eprintln!("{}", injection.stderr);
                } else {
                    eprintln!(
                        "page-agent injection failed with exit code {}",
                        injection.exit_code
                    );
                }
            }
            return Ok(injection.exit_code);
        }

        if let Some(prompt) = injection.prompt {
            if prompt.exit_code != 0 {
                if verbose {
                    if !prompt.stderr.is_empty() {
                        eprintln!("{}", prompt.stderr);
                    } else {
                        eprintln!(
                            "page-agent prompt execution failed with exit code {}",
                            prompt.exit_code
                        );
                    }
                }
                return Ok(prompt.exit_code);
            }
        }
    }

    Ok(exit_code)
}

fn export_sandbox_files(
    output_dir: &Path,
    sandbox_ignore: Option<&Path>,
    files: &[SandboxFile],
    verbose: bool,
) -> Result<(), Box<dyn Error>> {
    std::fs::create_dir_all(output_dir)?;
    let prepared_files = prepare_sandbox_files(output_dir, sandbox_ignore, files)?;

    for prepared_file in prepared_files {
        let destination_path = output_dir.join(&prepared_file.relative_path);
        if let Some(parent) = destination_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        export_sandbox_file(prepared_file.file, destination_path.as_path())?;
        if verbose {
            println!("sandbox file exported: {}", destination_path.display());
        }
    }

    Ok(())
}

fn export_sandbox_file(file: &SandboxFile, destination_path: &Path) -> Result<(), Box<dyn Error>> {
    match file.origin {
        SandboxFileOrigin::Sandbox => {
            std::fs::write(destination_path, &file.data)?;
        }
        SandboxFileOrigin::RealFs => {
            if destination_path.exists() {
                std::fs::remove_file(destination_path)?;
            }

            match std::fs::rename(&file.path, destination_path) {
                Ok(()) => {}
                Err(_) => {
                    std::fs::write(destination_path, &file.data)?;
                    if file.path.is_file() {
                        let _ = std::fs::remove_file(&file.path);
                    }
                }
            }
        }
    }

    Ok(())
}

async fn run_mcp_streamable_http_with_shutdown_internal<F>(
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
        let dashboard_host = if config.host == "0.0.0.0" {
            "localhost".to_string()
        } else {
            config.host.clone()
        };
        println!(
            "http streaming: http://{}:{}/mcp",
            dashboard_host, config.port
        );
        println!("cache folder: {}", oatmeal_cache_dir_text());
    }

    let serve_result =
        crate::mcp::run_mcp_streamable_http(config, page_agent_config, shutdown).await;

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

#[cfg(test)]
mod tests {
    use super::*;
    use once_cell::sync::Lazy;
    use std::ffi::OsString;
    use std::path::PathBuf;
    use std::sync::Mutex;

    static ENV_LOCK: Lazy<Mutex<()>> = Lazy::new(|| Mutex::new(()));

    fn clear_oatmeal_env() {
        let keys: Vec<String> = std::env::vars()
            .filter_map(|(key, _)| {
                if key.starts_with("OATMEAL_") {
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

    fn reset_test_artifact_dir(test_name: &str) -> PathBuf {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("artifacts")
            .join("app-tests")
            .join(test_name);
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create test artifact dir");
        dir
    }

    fn create_temp_test_dir(name: &str) -> PathBuf {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("time")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("oatmeal-{name}-{unique}"));
        std::fs::create_dir_all(&dir).expect("create temp test dir");
        dir
    }

    #[test]
    fn parse_cli_prefers_command_over_version() {
        let args = vec![
            OsString::from("--version"),
            OsString::from("--command"),
            OsString::from("open"),
        ];

        assert_eq!(
            parse_cli(&args).mode,
            CliMode::Command(vec!["open".to_string()])
        );
    }

    #[test]
    fn parse_cli_recognizes_help_and_does_not_fall_through_to_serve() {
        match parse_cli(&[OsString::from("--help")]).mode {
            CliMode::Help(output) => {
                assert!(output.contains("Usage:"), "output={output}");
                assert!(output.contains("--mcp"), "output={output}");
                assert!(
                    output.contains("--command [<COMMAND>...]"),
                    "output={output}"
                );
            }
            other => panic!("expected help mode, got {other:?}"),
        }
    }

    #[test]
    fn parse_cli_recognizes_version_aliases() {
        assert_eq!(
            parse_cli(&[OsString::from("--version")]).mode,
            CliMode::Version
        );
        assert_eq!(parse_cli(&[OsString::from("-V")]).mode, CliMode::Version);
        assert_eq!(
            parse_cli(&[OsString::from("version")]).mode,
            CliMode::Version
        );
        assert_eq!(parse_cli(&[OsString::from("--clean")]).mode, CliMode::Clean);
        assert_eq!(
            parse_cli(&[OsString::from("--cache-dir")]).mode,
            CliMode::CacheDir
        );
        assert_eq!(
            parse_cli(&[OsString::from("cache-dir")]).mode,
            CliMode::CacheDir
        );
        assert_eq!(
            parse_cli(&[OsString::from("--screenshot")]).mode,
            CliMode::Screenshot
        );
        assert_eq!(
            parse_cli(&[OsString::from("--mcp")]).mode,
            CliMode::Mcp(McpProtocol::Sse)
        );
        assert_eq!(
            parse_cli(&[OsString::from("--mcp"), OsString::from("stdio")]).mode,
            CliMode::Mcp(McpProtocol::Stdio)
        );
        assert_eq!(
            parse_cli(&[OsString::from("--mcp=sse")]).mode,
            CliMode::Mcp(McpProtocol::Sse)
        );
        assert_eq!(
            parse_cli(&[OsString::from("--register-uri")]).mode,
            CliMode::RegisterUri
        );
        assert_eq!(
            parse_cli(&[OsString::from("--unregister-uri")]).mode,
            CliMode::UnregisterUri
        );
        assert_eq!(
            parse_cli(&[OsString::from("oatmeal://open?port=9876")]).mode,
            CliMode::UriLaunch("oatmeal://open?port=9876".to_string())
        );
        assert_eq!(parse_cli(&[OsString::from("serve")]).mode, CliMode::Serve);
    }

    #[test]
    fn parse_cli_ignores_page_agent_url_values_for_uri_detection() {
        assert_eq!(
            parse_cli(&[
                OsString::from("--page-agent-url"),
                OsString::from("http://localhost:11434/v1")
            ])
            .mode,
            CliMode::Serve
        );
        assert_eq!(
            parse_cli(&[OsString::from("--page-agent-url=http://localhost:11434/v1")]).mode,
            CliMode::Serve
        );
    }

    #[test]
    fn build_page_agent_config_supports_split_and_equals_forms() {
        let app_config = AppConfig::default();
        let cli = parse_cli(&[
            OsString::from("--page-agent-model"),
            OsString::from("my-model"),
            OsString::from("--page-agent-url=http://127.0.0.1:5000/v1"),
            OsString::from("--page-agent-key"),
            OsString::from("secret-key"),
        ]);
        let parsed = build_page_agent_config(&app_config, &cli.flags);

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

        let cli = parse_cli(&[
            OsString::from("--page-agent-model"),
            OsString::from("cli-model"),
            OsString::from("--page-agent-url=http://cli.local/v1"),
        ]);
        let parsed = build_page_agent_config(&app_config, &cli.flags);

        assert_eq!(parsed.model, "cli-model");
        assert_eq!(parsed.url, "http://cli.local/v1");
        assert_eq!(parsed.key, "env-key");
    }

    #[test]
    fn parse_cli_verbose_defaults_off_and_enables_with_flag() {
        assert!(!parse_cli(&[]).flags.verbose);
        assert!(parse_cli(&[OsString::from("--verbose")]).flags.verbose);
        assert!(
            parse_cli(&[OsString::from("--command"), OsString::from("open")])
                .flags
                .verbose
        );
        assert!(
            parse_cli(&[
                OsString::from("--verbose"),
                OsString::from("--command"),
                OsString::from("open"),
                OsString::from("https://example.com")
            ])
            .flags
            .verbose
        );
    }

    #[test]
    fn parse_cli_sandbox_output_reads_flag_before_command() {
        let cli = parse_cli(&[
            OsString::from("--sandbox-output"),
            OsString::from("/tmp/oatmeal-sandbox"),
            OsString::from("--command"),
            OsString::from("echo"),
            OsString::from("hello"),
        ]);

        assert_eq!(
            cli.flags.sandbox_output,
            Some(PathBuf::from("/tmp/oatmeal-sandbox"))
        );
    }

    #[test]
    fn parse_cli_sandbox_ignore_reads_flag_before_command() {
        let cli = parse_cli(&[
            OsString::from("--sandbox-ignore"),
            OsString::from("/tmp/oatmeal-ignore"),
            OsString::from("--command"),
            OsString::from("echo"),
            OsString::from("hello"),
        ]);

        assert_eq!(
            cli.flags.sandbox_ignore,
            Some(PathBuf::from("/tmp/oatmeal-ignore"))
        );
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

        apply_uri_overrides(&mut config, "oatmeal://open?port=7777").expect("port override");
        assert_eq!(config.port, 7777);
        assert_eq!(config.host, "0.0.0.0");

        apply_uri_overrides(&mut config, "oatmeal://open?port=8888&host=127.0.0.1")
            .expect("full override");
        assert_eq!(config.port, 8888);
        assert_eq!(config.host, "127.0.0.1");

        apply_uri_overrides(&mut config, "oatmeal://localhost").expect("no host query override");
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

        clear_oatmeal_env();

        let home = create_temp_test_dir("app-home-passthrough");
        let cwd = create_temp_test_dir("app-cwd-passthrough");

        let original_home: Option<OsString> = std::env::var_os("HOME");
        std::env::set_var("HOME", &home);
        let _cwd_guard = DirGuard::enter(&cwd);

        let result = run_with_args(vec![
            OsString::from("--command"),
            OsString::from("agent-browser"),
            OsString::from("--version"),
        ])
        .await
        .expect("run passthrough");

        assert_eq!(result, 0);

        if let Some(home) = original_home {
            std::env::set_var("HOME", home);
        } else {
            std::env::remove_var("HOME");
        }

        clear_oatmeal_env();
    }

    #[tokio::test]
    async fn run_with_args_command_exports_sandbox_files() {
        let _guard = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        clear_oatmeal_env();

        let artifact_dir = reset_test_artifact_dir("run_with_args_command_exports_sandbox_files");
        let home = create_temp_test_dir("app-home-export");
        let cwd = create_temp_test_dir("app-cwd-export");
        let sandbox_output = artifact_dir.join("sandbox-output");

        let original_home: Option<OsString> = std::env::var_os("HOME");
        std::env::set_var("HOME", &home);
        let _cwd_guard = DirGuard::enter(&cwd);

        let result = run_with_args(vec![
            OsString::from("--sandbox-output"),
            sandbox_output.as_os_str().to_os_string(),
            OsString::from("--command"),
            OsString::from("echo hello > /report.txt"),
        ])
        .await
        .expect("run passthrough");

        assert_eq!(result, 0);
        let exported_file = sandbox_output.join("report.txt");
        let exported_data = std::fs::read_to_string(exported_file).expect("read exported file");
        assert_eq!(exported_data, "hello\n");

        if let Some(home) = original_home {
            std::env::set_var("HOME", home);
        } else {
            std::env::remove_var("HOME");
        }

        clear_oatmeal_env();
    }

    #[tokio::test]
    async fn run_with_args_supports_basic_shell_commands() {
        let _guard = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        clear_oatmeal_env();

        let artifact_dir = reset_test_artifact_dir("run_with_args_supports_basic_shell_commands");
        let home = create_temp_test_dir("app-home-shell-basic");
        let cwd = create_temp_test_dir("app-cwd-shell-basic");
        let sandbox_output = artifact_dir.join("sandbox-output");

        let original_home: Option<OsString> = std::env::var_os("HOME");
        std::env::set_var("HOME", &home);
        let _cwd_guard = DirGuard::enter(&cwd);

        let result = run_with_args(vec![
            OsString::from("--sandbox-output"),
            sandbox_output.as_os_str().to_os_string(),
            OsString::from("--command"),
            OsString::from("name=world && echo hello-$name | cat > /report.txt"),
        ])
        .await
        .expect("run basic shell command");

        assert_eq!(result, 0);
        let exported_file = sandbox_output.join("report.txt");
        let exported_data = std::fs::read_to_string(exported_file).expect("read exported file");
        assert_eq!(exported_data, "hello-world\n");

        if let Some(home) = original_home {
            std::env::set_var("HOME", home);
        } else {
            std::env::remove_var("HOME");
        }

        clear_oatmeal_env();
    }

    #[tokio::test]
    async fn run_with_args_supports_python_command_mode() {
        let _guard = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        clear_oatmeal_env();

        let artifact_dir = reset_test_artifact_dir("run_with_args_supports_python_command_mode");
        let home = create_temp_test_dir("app-home-python");
        let cwd = create_temp_test_dir("app-cwd-python");
        let sandbox_output = artifact_dir.join("sandbox-output");

        let original_home: Option<OsString> = std::env::var_os("HOME");
        std::env::set_var("HOME", &home);
        let _cwd_guard = DirGuard::enter(&cwd);

        let result = run_with_args(vec![
            OsString::from("--sandbox-output"),
            sandbox_output.as_os_str().to_os_string(),
            OsString::from("--command"),
            OsString::from(
                "python3 -c \"from pathlib import Path; _ = Path('/report.txt').write_text('hello from python\\n')\"",
            ),
        ])
        .await
        .expect("run python command");

        assert_eq!(result, 0);
        let exported_file = sandbox_output.join("report.txt");
        let exported_data = std::fs::read_to_string(exported_file).expect("read exported file");
        assert_eq!(exported_data, "hello from python\n");

        if let Some(home) = original_home {
            std::env::set_var("HOME", home);
        } else {
            std::env::remove_var("HOME");
        }

        clear_oatmeal_env();
    }

    #[tokio::test]
    async fn run_with_args_applies_sandbox_ignore_filters() {
        let _guard = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        clear_oatmeal_env();

        let artifact_dir = reset_test_artifact_dir("run_with_args_applies_sandbox_ignore_filters");
        let home = create_temp_test_dir("app-home-ignore");
        let cwd = create_temp_test_dir("app-cwd-ignore");
        let sandbox_output = artifact_dir.join("sandbox-output");
        let sandbox_ignore = create_temp_test_dir("app-ignore-file").join("sandbox.ignore");

        let original_home: Option<OsString> = std::env::var_os("HOME");
        std::env::set_var("HOME", &home);
        let _cwd_guard = DirGuard::enter(&cwd);

        std::fs::write(&sandbox_ignore, "*.log\n").expect("write sandbox ignore");

        let result = run_with_args(vec![
            OsString::from("--sandbox-output"),
            sandbox_output.as_os_str().to_os_string(),
            OsString::from("--sandbox-ignore"),
            sandbox_ignore.as_os_str().to_os_string(),
            OsString::from("--command"),
            OsString::from(
                "echo keep > /keep.txt && echo hidden > /.DS_Store && echo noisy > /trace.log",
            ),
        ])
        .await
        .expect("run passthrough");

        assert_eq!(result, 0);
        assert!(sandbox_output.join("keep.txt").exists());
        assert!(!sandbox_output.join(".DS_Store").exists());
        assert!(!sandbox_output.join("trace.log").exists());

        if let Some(home) = original_home {
            std::env::set_var("HOME", home);
        } else {
            std::env::remove_var("HOME");
        }

        clear_oatmeal_env();
    }

    #[tokio::test]
    async fn run_server_with_shutdown_starts_and_exits_cleanly() {
        let config = AppConfig {
            auth_url: None,
            port: 0,
            host: "127.0.0.1".to_string(),
            browser_path: None,
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
        let config = AppConfig {
            auth_url: None,
            port: 9607,
            host: "256.256.256.256".to_string(),
            browser_path: None,
            page_agent: PageAgentConfig::default(),
        };

        let result = run_server_with_shutdown(config, async {}).await;
        assert!(result.is_err(), "invalid host should produce bind error");
    }
}
