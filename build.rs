use flate2::write::GzEncoder;
use flate2::Compression;
use std::env;
use std::error::Error;
use std::fs;
use std::io::Write;
use std::path::PathBuf;

const VERSION: &str = "v0.16.3";

fn main() {
    if let Err(error) = run() {
        panic!("failed to prepare embedded agent-browser binary: {error}");
    }
}

fn run() -> Result<(), Box<dyn Error>> {
    println!("cargo:rerun-if-changed=build.rs");

    let target_os = env::var("CARGO_CFG_TARGET_OS")?;
    let target_arch = env::var("CARGO_CFG_TARGET_ARCH")?;
    let asset_name = asset_name_for_target(&target_os, &target_arch)?;
    let download_url = format!(
        "https://github.com/vercel-labs/agent-browser/releases/download/{VERSION}/{asset_name}"
    );

    let out_dir = PathBuf::from(env::var("OUT_DIR")?);
    let out_file = out_dir.join("agent-browser-bin.gz");

    if !out_file.exists() {
        let response = reqwest::blocking::get(download_url)?.error_for_status()?;
        let bytes = response.bytes()?;
        let file = fs::File::create(&out_file)?;
        let mut encoder = GzEncoder::new(file, Compression::best());
        encoder.write_all(&bytes)?;
        encoder.finish()?;
    }

    Ok(())
}

fn asset_name_for_target(target_os: &str, target_arch: &str) -> Result<&'static str, String> {
    match (target_os, target_arch) {
        ("linux", "x86_64") => Ok("agent-browser-linux-x64"),
        ("linux", "aarch64") => Ok("agent-browser-linux-arm64"),
        ("macos", "x86_64") => Ok("agent-browser-darwin-x64"),
        ("macos", "aarch64") => Ok("agent-browser-darwin-arm64"),
        ("windows", "x86_64") => Ok("agent-browser-win32-x64.exe"),
        _ => Err(format!(
            "unsupported target: os={target_os}, arch={target_arch}. Supported: linux x86_64/aarch64, macos x86_64/aarch64, windows x86_64"
        )),
    }
}
