use crate::embedded_binary::cache_root_dir;
use crate::screenshot::{capture_all_screenshots, ScreenshotResult};
use serde_json::Value;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartupReady {
    pub listen_addr: String,
    pub display_url: String,
}

pub fn oatmeal_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

pub fn oatmeal_version_text() -> String {
    format!("Oatmeal v{}", oatmeal_version())
}

pub fn mcp_display_url(host: &str, port: u16) -> String {
    let display_host = if host == "0.0.0.0" { "localhost" } else { host };
    format!("http://{display_host}:{port}/mcp")
}

pub fn oatmeal_version_payload() -> Value {
    serde_json::json!({
        "version": oatmeal_version()
    })
}

pub fn oatmeal_cache_dir() -> std::path::PathBuf {
    cache_root_dir().join("oatmeal")
}

pub fn oatmeal_cache_dir_text() -> String {
    oatmeal_cache_dir().display().to_string()
}

pub fn oatmeal_cache_dir_payload() -> Value {
    serde_json::json!({
        "cache_dir": oatmeal_cache_dir_text()
    })
}

fn binary_probe(path: Option<&str>) -> Value {
    let Some(path) = path else {
        return serde_json::json!({
            "path": null,
            "exists": false,
        });
    };

    let path_ref = Path::new(path);
    let exists = path_ref.exists();

    serde_json::json!({
        "path": path,
        "exists": exists,
    })
}

pub fn diagnostics_payload() -> Value {
    let oatmeal_detected_path =
        crate::browser_detection::find_chrome_browser().map(|p| p.display().to_string());
    let cli_binary_path = crate::embedded_binary::resolve_binary_path(None)
        .ok()
        .map(|p| p.display().to_string());

    let config_result = crate::configuration::load_config();
    let (config_browser_path, config_host, config_port, config_error) = match config_result {
        Ok(config) => (
            config.browser_path,
            Some(config.host),
            Some(config.port),
            None,
        ),
        Err(error) => (None, None, None, Some(error.to_string())),
    };

    let mcp_resolved_path = crate::embedded_binary::resolve_binary_path(None)
        .ok()
        .map(|p| p.display().to_string());
    let legacy_mcp_override_path =
        crate::embedded_binary::resolve_binary_path(config_browser_path.as_deref())
            .ok()
            .map(|p| p.display().to_string());

    let cache_root = cache_root_dir();
    let oatmeal_cache = oatmeal_cache_dir();

    serde_json::json!({
        "timestamp_unix_ms": SystemTime::now().duration_since(UNIX_EPOCH).ok().map(|d| d.as_millis()),
        "runtime": {
            "platform": std::env::consts::OS,
            "arch": std::env::consts::ARCH,
            "exe": std::env::current_exe().ok().map(|p| p.display().to_string()),
            "cwd": std::env::current_dir().ok().map(|p| p.display().to_string()),
        },
        "paths": {
            "cache_root": cache_root.display().to_string(),
            "oatmeal_cache": oatmeal_cache.display().to_string(),
        },
        "config": {
            "loaded": config_error.is_none(),
            "error": config_error,
            "host": config_host,
            "port": config_port,
            "browser_path": config_browser_path,
        },
        "detection": {
            "oatmeal_detected_chrome": oatmeal_detected_path,
            "cli_binary_path_resolved": cli_binary_path,
            "mcp_binary_path_resolved": mcp_resolved_path,
            "legacy_mcp_override_resolution": legacy_mcp_override_path,
        },
        "probes": {
            "oatmeal_detected_chrome": binary_probe(oatmeal_detected_path.as_deref()),
            "cli_binary": binary_probe(cli_binary_path.as_deref()),
            "mcp_binary": binary_probe(mcp_resolved_path.as_deref()),
        },
        "env": {
            "AGENT_BROWSER_HEADED": std::env::var("AGENT_BROWSER_HEADED").ok(),
            "AGENT_BROWSER_ENGINE": std::env::var("AGENT_BROWSER_ENGINE").ok(),
            "AGENT_BROWSER_HOME": std::env::var("AGENT_BROWSER_HOME").ok(),
            "AGENT_BROWSER_SESSION": std::env::var("AGENT_BROWSER_SESSION").ok(),
            "OATMEAL_HOST": std::env::var("OATMEAL_HOST").ok(),
            "OATMEAL_PORT": std::env::var("OATMEAL_PORT").ok(),
            "OATMEAL_BROWSER_PATH": std::env::var("OATMEAL_BROWSER_PATH").ok(),
        },
    })
}

pub fn capture_system_screenshots() -> Result<Vec<ScreenshotResult>, String> {
    match std::panic::catch_unwind(capture_all_screenshots) {
        Ok(Ok(screenshots)) => Ok(screenshots),
        Ok(Err(error)) => Err(format!("screenshot failed: {error}")),
        Err(_) => Err("screenshot failed: panic in capture backend".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mcp_display_url_normalizes_wildcard_host() {
        assert_eq!(
            mcp_display_url("0.0.0.0", 9607),
            "http://localhost:9607/mcp"
        );
    }

    #[test]
    fn mcp_display_url_preserves_explicit_host() {
        assert_eq!(
            mcp_display_url("127.0.0.1", 9607),
            "http://127.0.0.1:9607/mcp"
        );
    }
}
