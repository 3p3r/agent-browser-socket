use crate::embedded_binary::cache_root_dir;
use crate::screenshot::{capture_all_screenshots, ScreenshotResult};
use serde_json::Value;

pub fn oatmeal_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

pub fn oatmeal_version_text() -> String {
    format!("oatmeal {}", oatmeal_version())
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
