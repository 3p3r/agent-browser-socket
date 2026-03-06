#[path = "../src/auth.rs"]
mod auth;
#[path = "../src/configuration.rs"]
mod configuration;
#[path = "../src/embedded_binary.rs"]
mod embedded_binary;

use axum::http::StatusCode;
use axum::routing::get;
use axum::Router;
use once_cell::sync::Lazy;
use std::ffi::OsString;
use std::path::PathBuf;
use std::process::Command;
use std::sync::Mutex;
use tokio::net::TcpListener;
use tokio::sync::oneshot;

static PROCESS_ENV_LOCK: Lazy<Mutex<()>> = Lazy::new(|| Mutex::new(()));

fn lock_env() -> std::sync::MutexGuard<'static, ()> {
    PROCESS_ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

struct DirGuard {
    original: PathBuf,
}

struct EnvVarGuard {
    key: String,
    original: Option<OsString>,
}

impl EnvVarGuard {
    fn set<K: Into<String>, V: AsRef<std::ffi::OsStr>>(key: K, value: V) -> Self {
        let key = key.into();
        let original = std::env::var_os(&key);
        std::env::set_var(&key, value);
        Self { key, original }
    }
}

impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        if let Some(original) = &self.original {
            std::env::set_var(&self.key, original);
        } else {
            std::env::remove_var(&self.key);
        }
    }
}

fn create_clean_home() -> PathBuf {
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("time")
        .as_nanos();
    let home = std::env::temp_dir().join(format!("abs-home-{unique}"));
    std::fs::create_dir_all(&home).expect("create clean home");
    home
}

fn create_clean_cache_root() -> PathBuf {
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("time")
        .as_nanos();
    let cache = std::env::temp_dir().join(format!("abs-cache-{unique}"));
    std::fs::create_dir_all(&cache).expect("create clean cache root");
    cache
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

fn create_mock_browser_binary() -> PathBuf {
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("time")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("abs-cli-{unique}"));
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

fn resolve_wrapper_executable() -> PathBuf {
    let mut path = std::env::current_exe().expect("current exe path");
    path.pop();
    if path.ends_with("deps") {
        path.pop();
    }

    let exe_name = if cfg!(windows) {
        "agent-browser-socket.exe"
    } else {
        "agent-browser-socket"
    };
    let candidate = path.join(exe_name);

    if candidate.exists() {
        return candidate;
    }

    let status = Command::new("cargo")
        .args(["build"])
        .status()
        .expect("run cargo build for wrapper binary");
    assert!(status.success(), "cargo build failed while preparing wrapper executable");

    assert!(candidate.exists(), "wrapper executable not found at {}", candidate.display());
    candidate
}

#[tokio::test]
async fn auth_check_skips_when_none_or_whitespace_url() {
    let client = reqwest::Client::new();

    let result_none = auth::check_auth(&client, None, None, None).await;
    assert!(result_none.is_ok());

    let result_blank = auth::check_auth(&client, Some("   "), None, None).await;
    assert!(result_blank.is_ok());
}

#[tokio::test]
async fn auth_check_handles_unreachable_endpoint_as_500() {
    let client = reqwest::Client::new();
    let result = auth::check_auth(&client, Some("http://127.0.0.1:1/auth"), None, None).await;
    assert_eq!(result, Err(StatusCode::INTERNAL_SERVER_ERROR));
}

#[tokio::test]
async fn auth_check_maps_status_codes() {
    async fn status_200() -> StatusCode {
        StatusCode::OK
    }
    async fn status_401() -> StatusCode {
        StatusCode::UNAUTHORIZED
    }
    async fn status_403() -> StatusCode {
        StatusCode::FORBIDDEN
    }
    async fn status_500() -> StatusCode {
        StatusCode::INTERNAL_SERVER_ERROR
    }

    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind auth test server");
    let port = listener.local_addr().expect("local addr").port();

    let app = Router::new()
        .route("/ok", get(status_200))
        .route("/unauth", get(status_401))
        .route("/forbidden", get(status_403))
        .route("/err", get(status_500));

    let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
    let handle = tokio::spawn(async move {
        let _ = axum::serve(listener, app)
            .with_graceful_shutdown(async {
                let _ = shutdown_rx.await;
            })
            .await;
    });

    let client = reqwest::Client::new();

    let ok = auth::check_auth(&client, Some(&format!("http://127.0.0.1:{port}/ok")), None, None).await;
    assert!(ok.is_ok());

    let unauth = auth::check_auth(
        &client,
        Some(&format!("http://127.0.0.1:{port}/unauth")),
        Some("Bearer abc"),
        Some("sid=1"),
    )
    .await;
    assert_eq!(unauth, Err(StatusCode::UNAUTHORIZED));

    let forbidden = auth::check_auth(&client, Some(&format!("http://127.0.0.1:{port}/forbidden")), None, None).await;
    assert_eq!(forbidden, Err(StatusCode::FORBIDDEN));

    let err = auth::check_auth(&client, Some(&format!("http://127.0.0.1:{port}/err")), None, None).await;
    assert_eq!(err, Err(StatusCode::INTERNAL_SERVER_ERROR));

    let _ = shutdown_tx.send(());
    let _ = handle.await;
}

#[test]
fn configuration_uses_embedded_defaults_when_no_sources_exist() {
    let _guard = lock_env();
    clear_abs_env();

    let clean_home = create_clean_home();
    let _home_guard = EnvVarGuard::set("HOME", clean_home.as_os_str());
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("time")
        .as_nanos();
    let working_dir = std::env::temp_dir().join(format!("abs-defaults-{unique}"));
    std::fs::create_dir_all(&working_dir).expect("create working dir");
    let _cwd = DirGuard::enter(&working_dir);

    let cfg = configuration::load_config().expect("load config defaults");
    assert_eq!(cfg.port, 9607);
    assert_eq!(cfg.host, "0.0.0.0");
    assert!(cfg.auth_url.is_none());
    assert!(cfg.browser_path.is_none());

    clear_abs_env();
}

#[test]
fn configuration_local_abs_file_overrides_home_and_defaults() {
    let _guard = lock_env();
    clear_abs_env();

    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("time")
        .as_nanos();
    let root = std::env::temp_dir().join(format!("abs-config-{unique}"));
    let home = root.join("home");
    let local = root.join("work");
    std::fs::create_dir_all(&home).expect("create home dir");
    std::fs::create_dir_all(&local).expect("create work dir");

    std::fs::write(
        home.join(".abs"),
        "port = 9101\nhost = \"127.0.0.2\"\n",
    )
    .expect("write home abs");
    std::fs::write(
        local.join(".abs"),
        "port = 9999\nhost = \"127.0.0.9\"\n",
    )
    .expect("write local abs");

    let _home_guard = EnvVarGuard::set("HOME", home.as_os_str());
    let _cwd = DirGuard::enter(&local);

    let cfg = configuration::load_config().expect("load config from files");
    assert_eq!(cfg.port, 9999);
    assert_eq!(cfg.host, "127.0.0.9");

    clear_abs_env();
}

#[test]
fn cli_version_and_command_paths_work() {
    let _guard = lock_env();
    clear_abs_env();

    let clean_home = create_clean_home();
    let _home_guard = EnvVarGuard::set("HOME", clean_home.as_os_str());
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("time")
        .as_nanos();
    let working_dir = std::env::temp_dir().join(format!("abs-cli-cwd-{unique}"));
    std::fs::create_dir_all(&working_dir).expect("create cli cwd");
    let _cwd = DirGuard::enter(&working_dir);

    let exe = resolve_wrapper_executable();

    let version_output = Command::new(&exe)
        .arg("--version")
        .output()
        .expect("run --version");
    assert!(version_output.status.success());
    let version_stdout = String::from_utf8_lossy(&version_output.stdout);
    assert!(version_stdout.contains("agent-browser-socket"));

    let missing_command_output = Command::new(&exe)
        .arg("--command")
        .output()
        .expect("run --command without args");
    assert_eq!(missing_command_output.status.code(), Some(2));
    let missing_stderr = String::from_utf8_lossy(&missing_command_output.stderr);
    assert!(missing_stderr.contains("missing forwarded arguments"));

    let mock_browser = create_mock_browser_binary();
    std::fs::write(
        working_dir.join(".abs"),
        format!("browser_path = \"{}\"\n", mock_browser.display()),
    )
    .expect("write local .abs for cli test");

    let passthrough_output = Command::new(&exe)
        .args(["--command", "one", "two"])
        .output()
        .expect("run passthrough command");
    assert!(
        passthrough_output.status.success(),
        "passthrough failed: stdout={} stderr={}",
        String::from_utf8_lossy(&passthrough_output.stdout),
        String::from_utf8_lossy(&passthrough_output.stderr)
    );
    let passthrough_stdout = String::from_utf8_lossy(&passthrough_output.stdout);
    assert!(passthrough_stdout.contains("one"));
    assert!(passthrough_stdout.contains("two"));

    clear_abs_env();
}

#[test]
fn embedded_binary_override_path_is_returned_unchanged() {
    let _guard = lock_env();
    let override_path = if cfg!(windows) {
        "C:\\temp\\custom-agent-browser.exe"
    } else {
        "/tmp/custom-agent-browser"
    };

    let path = embedded_binary::resolve_binary_path(Some(override_path)).expect("resolve override path");
    assert_eq!(path, PathBuf::from(override_path));
}

#[test]
fn embedded_binary_extracts_and_reuses_cache_file() {
    let _guard = lock_env();

    let cache_root = create_clean_cache_root();
    let _xdg_guard = EnvVarGuard::set("XDG_CACHE_HOME", cache_root.as_os_str());
    #[cfg(windows)]
    let _localapp_guard = EnvVarGuard::set("LOCALAPPDATA", cache_root.as_os_str());

    let first = embedded_binary::resolve_binary_path(None).expect("first extraction");
    assert!(first.exists(), "extracted binary should exist");

    let first_meta = std::fs::metadata(&first).expect("metadata first");
    assert!(first_meta.len() > 0, "extracted binary should be non-empty");

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = first_meta.permissions().mode();
        assert!(mode & 0o111 != 0, "binary should be executable on unix");
    }

    let second = embedded_binary::resolve_binary_path(None).expect("second extraction call");
    assert_eq!(first, second, "extraction path should be stable and reused");

    let second_meta = std::fs::metadata(&second).expect("metadata second");
    assert_eq!(first_meta.len(), second_meta.len(), "binary length should remain stable");
}

#[test]
fn cli_clean_removes_cached_embedded_binary() {
    let _guard = lock_env();

    let cache_root = create_clean_cache_root();
    let _xdg_guard = EnvVarGuard::set("XDG_CACHE_HOME", cache_root.as_os_str());
    #[cfg(windows)]
    let _localapp_guard = EnvVarGuard::set("LOCALAPPDATA", cache_root.as_os_str());

    let extracted = embedded_binary::resolve_binary_path(None).expect("extract binary before clean");
    assert!(extracted.exists(), "binary should exist before clean");

    let clean_result = embedded_binary::clean_cached_binary().expect("clean cached binary");
    assert!(clean_result, "clean should report removed binary");
    assert!(!extracted.exists(), "binary should be removed after clean");

    let second_clean = embedded_binary::clean_cached_binary().expect("clean cached binary second time");
    assert!(!second_clean, "second clean should report nothing to remove");
}
