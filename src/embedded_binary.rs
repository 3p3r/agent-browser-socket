use dirs::cache_dir;
use std::fs;
use std::path::PathBuf;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

const EMBEDDED_BINARY: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/agent-browser-bin"));

pub fn resolve_binary_path(browser_override: Option<&str>) -> Result<PathBuf, std::io::Error> {
    if let Some(path) = browser_override {
        return Ok(PathBuf::from(path));
    }

    let cache_root = cache_dir().unwrap_or_else(std::env::temp_dir);
    let app_dir = cache_root.join("agent-browser-socket");
    fs::create_dir_all(&app_dir)?;

    let file_name = if cfg!(windows) {
        "agent-browser.exe"
    } else {
        "agent-browser"
    };

    let binary_path = app_dir.join(file_name);
    let needs_write = match fs::metadata(&binary_path) {
        Ok(metadata) => metadata.len() != EMBEDDED_BINARY.len() as u64,
        Err(_) => true,
    };

    if needs_write {
        fs::write(&binary_path, EMBEDDED_BINARY)?;
    }

    #[cfg(unix)]
    {
        let mut permissions = fs::metadata(&binary_path)?.permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&binary_path, permissions)?;
    }

    Ok(binary_path)
}
