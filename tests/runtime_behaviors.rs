#[path = "../src/auth.rs"]
mod auth;
#[path = "../src/browser_detection.rs"]
mod browser_detection;
#[path = "../src/configuration.rs"]
mod configuration;
#[path = "../src/embedded_binary.rs"]
mod embedded_binary;

use axum::http::StatusCode;
use axum::routing::get;
use axum::Router;
use once_cell::sync::Lazy;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
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

fn reset_test_artifact_dir(test_name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("artifacts")
        .join("runtime-behaviors")
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
    let dir = std::env::temp_dir().join(format!("abs-{name}-{unique}"));
    std::fs::create_dir_all(&dir).expect("create temp test dir");
    dir
}

fn mirrored_export_path(base: &Path, source: &Path) -> PathBuf {
    let mut relative = PathBuf::new();
    for component in source.components() {
        if let std::path::Component::Normal(part) = component {
            relative.push(part);
        }
    }
    base.join(relative)
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

fn require_detected_browser() -> PathBuf {
    browser_detection::find_chrome_browser().expect("expected a detected Chrome-like browser")
}

fn resolve_wrapper_executable() -> PathBuf {
    let mut path = std::env::current_exe().expect("current exe path");
    path.pop();
    if path.ends_with("deps") {
        path.pop();
    }

    let exe_name = if cfg!(windows) {
        "agent-browser-server.exe"
    } else {
        "agent-browser-server"
    };
    let candidate = path.join(exe_name);

    if candidate.exists() {
        return candidate;
    }

    let status = Command::new("cargo")
        .args(["build"])
        .status()
        .expect("run cargo build for wrapper binary");
    assert!(
        status.success(),
        "cargo build failed while preparing wrapper executable"
    );

    assert!(
        candidate.exists(),
        "wrapper executable not found at {}",
        candidate.display()
    );
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

    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind auth test server");
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

    let ok = auth::check_auth(
        &client,
        Some(&format!("http://127.0.0.1:{port}/ok")),
        None,
        None,
    )
    .await;
    assert!(ok.is_ok());

    let unauth = auth::check_auth(
        &client,
        Some(&format!("http://127.0.0.1:{port}/unauth")),
        Some("Bearer abc"),
        Some("sid=1"),
    )
    .await;
    assert_eq!(unauth, Err(StatusCode::UNAUTHORIZED));

    let forbidden = auth::check_auth(
        &client,
        Some(&format!("http://127.0.0.1:{port}/forbidden")),
        None,
        None,
    )
    .await;
    assert_eq!(forbidden, Err(StatusCode::FORBIDDEN));

    let err = auth::check_auth(
        &client,
        Some(&format!("http://127.0.0.1:{port}/err")),
        None,
        None,
    )
    .await;
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
    assert_eq!(cfg.page_agent.model, "qwen3.5-plus");
    assert_eq!(cfg.page_agent.url, "http://localhost:11434/v1");
    assert_eq!(cfg.page_agent.key, "NA");

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
        "port = 9101\nhost = \"127.0.0.2\"\n[page_agent]\nmodel = \"home-model\"\nurl = \"http://home.local/v1\"\nkey = \"home-key\"\n",
    )
        .expect("write home abs");
    std::fs::write(
        local.join(".abs"),
        "port = 9999\nhost = \"127.0.0.9\"\n[page_agent]\nmodel = \"local-model\"\nurl = \"http://local.local/v1\"\nkey = \"local-key\"\n",
    )
        .expect("write local abs");

    let _home_guard = EnvVarGuard::set("HOME", home.as_os_str());
    let _cwd = DirGuard::enter(&local);

    let cfg = configuration::load_config().expect("load config from files");
    assert_eq!(cfg.port, 9999);
    assert_eq!(cfg.host, "127.0.0.9");
    assert_eq!(cfg.page_agent.model, "local-model");
    assert_eq!(cfg.page_agent.url, "http://local.local/v1");
    assert_eq!(cfg.page_agent.key, "local-key");

    clear_abs_env();
}

#[test]
fn configuration_page_agent_env_overrides_file() {
    let _guard = lock_env();
    clear_abs_env();

    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("time")
        .as_nanos();
    let root = std::env::temp_dir().join(format!("abs-config-page-agent-env-{unique}"));
    let home = root.join("home");
    let local = root.join("work");
    std::fs::create_dir_all(&home).expect("create home dir");
    std::fs::create_dir_all(&local).expect("create work dir");

    std::fs::write(
        local.join(".abs"),
        "[page_agent]\nmodel = \"file-model\"\nurl = \"http://file.local/v1\"\nkey = \"file-key\"\n",
    )
    .expect("write local abs");

    let _home_guard = EnvVarGuard::set("HOME", home.as_os_str());
    let _cwd = DirGuard::enter(&local);
    let _model_guard = EnvVarGuard::set("ABS_PAGE_AGENT__MODEL", "env-model");
    let _url_guard = EnvVarGuard::set("ABS_PAGE_AGENT__URL", "http://env.local/v1");
    let _key_guard = EnvVarGuard::set("ABS_PAGE_AGENT__KEY", "env-key");

    let cfg = configuration::load_config().expect("load config with env override");
    assert_eq!(cfg.page_agent.model, "env-model");
    assert_eq!(cfg.page_agent.url, "http://env.local/v1");
    assert_eq!(cfg.page_agent.key, "env-key");

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
    assert!(version_stdout.contains("agent-browser-server"));

    let missing_command_output = Command::new(&exe)
        .arg("--command")
        .output()
        .expect("run --command without args");
    assert_eq!(missing_command_output.status.code(), Some(2));
    let missing_stderr = String::from_utf8_lossy(&missing_command_output.stderr);
    assert!(missing_stderr.contains("missing forwarded arguments"));

    let browser_path = require_detected_browser();

    let passthrough_output = Command::new(&exe)
        .env("AGENT_BROWSER_EXECUTABLE_PATH", browser_path.as_os_str())
        .args(["--verbose", "--command", "agent-browser", "--version"])
        .output()
        .expect("run passthrough command");
    assert!(
        passthrough_output.status.success(),
        "passthrough failed: stdout={} stderr={}",
        String::from_utf8_lossy(&passthrough_output.stdout),
        String::from_utf8_lossy(&passthrough_output.stderr)
    );
    let passthrough_stdout = String::from_utf8_lossy(&passthrough_output.stdout);
    assert!(passthrough_stdout.contains("agent-browser"));

    clear_abs_env();
}

#[test]
fn cli_browser_screenshot_exports_real_png() {
    let _guard = lock_env();
    clear_abs_env();

    let clean_home = create_clean_home();
    let _home_guard = EnvVarGuard::set("HOME", clean_home.as_os_str());
    let artifact_dir = reset_test_artifact_dir("cli_browser_screenshot_exports_real_png");
    let working_dir = create_temp_test_dir("runtime-cwd-screenshot");
    let source_dir = create_temp_test_dir("runtime-shot-source");
    let sandbox_output = artifact_dir.join("sandbox-output");
    let screenshot_path = source_dir.join("browser-shot.png");
    let _cwd = DirGuard::enter(&working_dir);

    let exe = resolve_wrapper_executable();
    let browser_path = require_detected_browser();
    let screenshot_command = format!(
        "agent-browser open about:blank && agent-browser screenshot {}",
        screenshot_path.display()
    );

    let output = Command::new(&exe)
        .env("AGENT_BROWSER_EXECUTABLE_PATH", browser_path.as_os_str())
        .args([
            "--verbose",
            "--sandbox-output",
            sandbox_output.to_str().expect("sandbox output utf8"),
            "--command",
            &screenshot_command,
        ])
        .output()
        .expect("run real screenshot command");

    assert!(
        output.status.success(),
        "real screenshot command failed: stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let exported = mirrored_export_path(&sandbox_output, &screenshot_path);
    assert!(
        exported.exists(),
        "expected exported PNG at {}",
        exported.display()
    );
    let bytes = std::fs::read(&exported).expect("read exported screenshot");
    assert!(bytes.starts_with(b"\x89PNG\r\n\x1a\n"));
    assert!(
        !screenshot_path.exists(),
        "real fs screenshot should be moved into sandbox output"
    );

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

    let path =
        embedded_binary::resolve_binary_path(Some(override_path)).expect("resolve override path");
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
    assert_eq!(
        first_meta.len(),
        second_meta.len(),
        "binary length should remain stable"
    );
}

#[test]
fn cli_clean_removes_cached_embedded_binary() {
    let _guard = lock_env();

    let cache_root = create_clean_cache_root();
    let _xdg_guard = EnvVarGuard::set("XDG_CACHE_HOME", cache_root.as_os_str());
    #[cfg(windows)]
    let _localapp_guard = EnvVarGuard::set("LOCALAPPDATA", cache_root.as_os_str());

    let extracted =
        embedded_binary::resolve_binary_path(None).expect("extract binary before clean");
    assert!(extracted.exists(), "binary should exist before clean");

    let clean_result = embedded_binary::clean_cached_binary().expect("clean cached binary");
    assert!(clean_result, "clean should report removed binary");
    assert!(!extracted.exists(), "binary should be removed after clean");

    let second_clean =
        embedded_binary::clean_cached_binary().expect("clean cached binary second time");
    assert!(
        !second_clean,
        "second clean should report nothing to remove"
    );
}
