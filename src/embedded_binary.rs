use dirs::cache_dir;
use flate2::read::GzDecoder;
use std::fs;
use std::io::Read;
use std::path::PathBuf;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

const EMBEDDED_BINARY_GZ: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/agent-browser-bin.gz"));

fn cached_binary_path() -> PathBuf {
    let cache_root = cache_dir().unwrap_or_else(std::env::temp_dir);
    let app_dir = cache_root.join("agent-browser-socket");
    let file_name = if cfg!(windows) {
        "agent-browser.exe"
    } else {
        "agent-browser"
    };

    app_dir.join(file_name)
}

pub fn resolve_binary_path(browser_override: Option<&str>) -> Result<PathBuf, std::io::Error> {
    if let Some(path) = browser_override {
        return Ok(PathBuf::from(path));
    }

    let embedded_binary = decompress_embedded_binary()?;

    let binary_path = cached_binary_path();
    let app_dir = binary_path
        .parent()
        .map(std::path::Path::to_path_buf)
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::Other, "invalid cache binary path"))?;
    fs::create_dir_all(&app_dir)?;
    let needs_write = match fs::metadata(&binary_path) {
        Ok(metadata) => metadata.len() != embedded_binary.len() as u64,
        Err(_) => true,
    };

    if needs_write {
        fs::write(&binary_path, embedded_binary)?;
    }

    #[cfg(unix)]
    {
        let mut permissions = fs::metadata(&binary_path)?.permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&binary_path, permissions)?;
    }

    Ok(binary_path)
}

pub fn clean_cached_binary() -> Result<bool, std::io::Error> {
    let binary_path = cached_binary_path();
    if !binary_path.exists() {
        return Ok(false);
    }

    fs::remove_file(&binary_path)?;

    if let Some(parent) = binary_path.parent() {
        let _ = fs::remove_dir(parent);
    }

    Ok(true)
}

fn decompress_embedded_binary() -> Result<Vec<u8>, std::io::Error> {
    let mut decoder = GzDecoder::new(EMBEDDED_BINARY_GZ);
    let mut bytes = Vec::new();
    decoder.read_to_end(&mut bytes)?;
    Ok(bytes)
}
