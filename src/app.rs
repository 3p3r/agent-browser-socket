use crate::configuration::{load_config, AppConfig};
use crate::embedded_binary::{clean_cached_binary, resolve_binary_path};
use crate::screenshot::capture_all_screenshots;
use crate::server::{build_router, AppState};
use std::error::Error;
use std::ffi::OsString;
use std::future::Future;
use std::process::Stdio;
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::process::Command;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CliMode {
    Serve,
    Version,
    Clean,
    Screenshot,
    Command(Vec<OsString>),
}

pub fn parse_cli_mode(args: &[OsString]) -> CliMode {
    let command_flag = OsString::from("--command");
    if let Some(index) = args.iter().position(|arg| arg == &command_flag) {
        let forwarded = args.iter().skip(index + 1).cloned().collect();
        return CliMode::Command(forwarded);
    }

    let show_version = args
        .iter()
        .any(|arg| matches!(arg.to_string_lossy().as_ref(), "version" | "--version" | "-V"));

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
    } else {
        CliMode::Serve
    }
}

pub async fn run_with_args(args: Vec<OsString>) -> Result<i32, Box<dyn Error>> {
    match parse_cli_mode(&args) {
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
        CliMode::Serve => {
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

pub async fn run_server_with_shutdown<F>(config: AppConfig, shutdown: F) -> Result<(), Box<dyn Error>>
where
    F: Future<Output = ()> + Send + 'static,
{
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
        .with_graceful_shutdown(shutdown)
        .await?;

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
            .filter_map(|(key, _)| if key.starts_with("ABS_") { Some(key) } else { None })
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
        assert_eq!(parse_cli_mode(&[OsString::from("--version")]), CliMode::Version);
        assert_eq!(parse_cli_mode(&[OsString::from("-V")]), CliMode::Version);
        assert_eq!(parse_cli_mode(&[OsString::from("version")]), CliMode::Version);
        assert_eq!(parse_cli_mode(&[OsString::from("--clean")]), CliMode::Clean);
        assert_eq!(parse_cli_mode(&[OsString::from("--screenshot")]), CliMode::Screenshot);
        assert_eq!(parse_cli_mode(&[OsString::from("serve")]), CliMode::Serve);
    }

    #[tokio::test]
    async fn run_with_args_clean_returns_zero() {
        let result = run_with_args(vec![OsString::from("--clean")]).await.expect("run clean");
        assert_eq!(result, 0);
    }

    #[tokio::test]
    async fn run_with_args_returns_2_for_empty_command_passthrough() {
        let result = run_with_args(vec![OsString::from("--command")]).await.expect("run result");
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
        assert!(result.is_ok(), "expected clean startup/shutdown for serve path");
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
