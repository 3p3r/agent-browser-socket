use crate::embedded_binary::cache_root_dir;
use crate::screenshot::{capture_all_screenshots, ScreenshotResult};
use serde_json::Value;

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
