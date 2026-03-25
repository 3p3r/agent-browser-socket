#[path = "../src/browser_detection.rs"]
mod browser_detection;
#[path = "../src/configuration.rs"]
mod configuration;
#[path = "../src/embedded_binary.rs"]
mod embedded_binary;

use once_cell::sync::Lazy;
use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::Mutex;

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
    let home = std::env::temp_dir().join(format!("oatmeal-home-{unique}"));
    std::fs::create_dir_all(&home).expect("create clean home");
    home
}

fn create_clean_cache_root() -> PathBuf {
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("time")
        .as_nanos();
    let cache = std::env::temp_dir().join(format!("oatmeal-cache-{unique}"));
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

#[test]
fn configuration_uses_embedded_defaults_when_no_sources_exist() {
    let _guard = lock_env();
    clear_oatmeal_env();

    let clean_home = create_clean_home();
    let _home_guard = EnvVarGuard::set("HOME", clean_home.as_os_str());
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("time")
        .as_nanos();
    let working_dir = std::env::temp_dir().join(format!("oatmeal-defaults-{unique}"));
    std::fs::create_dir_all(&working_dir).expect("create working dir");
    let _cwd = DirGuard::enter(&working_dir);

    let cfg = configuration::load_config().expect("load config defaults");
    assert_eq!(cfg.port, 9607);
    assert_eq!(cfg.host, "0.0.0.0");
    assert_eq!(cfg.page_agent.model, "qwen3.5-plus");
    assert_eq!(cfg.page_agent.url, "http://localhost:11434/v1");
    assert_eq!(cfg.page_agent.key, "NA");

    clear_oatmeal_env();
}

#[test]
fn configuration_local_oatmeal_file_overrides_home_and_defaults() {
    let _guard = lock_env();
    clear_oatmeal_env();

    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("time")
        .as_nanos();
    let root = std::env::temp_dir().join(format!("oatmeal-config-{unique}"));
    let home = root.join("home");
    let local = root.join("work");
    std::fs::create_dir_all(&home).expect("create home dir");
    std::fs::create_dir_all(&local).expect("create work dir");

    std::fs::write(
        home.join(".oatmeal"),
        "port = 9101\nhost = \"127.0.0.2\"\n[page_agent]\nmodel = \"home-model\"\nurl = \"http://home.local/v1\"\nkey = \"home-key\"\n",
    )
        .expect("write home oatmeal");
    std::fs::write(
        local.join(".oatmeal"),
        "port = 9999\nhost = \"127.0.0.9\"\n[page_agent]\nmodel = \"local-model\"\nurl = \"http://local.local/v1\"\nkey = \"local-key\"\n",
    )
        .expect("write local oatmeal");

    let _home_guard = EnvVarGuard::set("HOME", home.as_os_str());
    let _cwd = DirGuard::enter(&local);

    let cfg = configuration::load_config().expect("load config from files");
    assert_eq!(cfg.port, 9999);
    assert_eq!(cfg.host, "127.0.0.9");
    assert_eq!(cfg.page_agent.model, "local-model");
    assert_eq!(cfg.page_agent.url, "http://local.local/v1");
    assert_eq!(cfg.page_agent.key, "local-key");

    clear_oatmeal_env();
}

#[test]
fn configuration_page_agent_env_overrides_file() {
    let _guard = lock_env();
    clear_oatmeal_env();

    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("time")
        .as_nanos();
    let root = std::env::temp_dir().join(format!("oatmeal-config-page-agent-env-{unique}"));
    let home = root.join("home");
    let local = root.join("work");
    std::fs::create_dir_all(&home).expect("create home dir");
    std::fs::create_dir_all(&local).expect("create work dir");

    std::fs::write(
        local.join(".oatmeal"),
        "[page_agent]\nmodel = \"file-model\"\nurl = \"http://file.local/v1\"\nkey = \"file-key\"\n",
    )
    .expect("write local oatmeal");

    let _home_guard = EnvVarGuard::set("HOME", home.as_os_str());
    let _cwd = DirGuard::enter(&local);
    let _model_guard = EnvVarGuard::set("OATMEAL_PAGE_AGENT__MODEL", "env-model");
    let _url_guard = EnvVarGuard::set("OATMEAL_PAGE_AGENT__URL", "http://env.local/v1");
    let _key_guard = EnvVarGuard::set("OATMEAL_PAGE_AGENT__KEY", "env-key");

    let cfg = configuration::load_config().expect("load config with env override");
    assert_eq!(cfg.page_agent.model, "env-model");
    assert_eq!(cfg.page_agent.url, "http://env.local/v1");
    assert_eq!(cfg.page_agent.key, "env-key");

    clear_oatmeal_env();
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
