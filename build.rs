use flate2::write::GzEncoder;
use flate2::Compression;
use std::env;
use std::error::Error;
use std::fs;
use std::io::Write;
use std::path::PathBuf;

const VERSION: &str = "v0.21.4";

fn main() {
    if let Err(error) = run() {
        panic!("failed to prepare embedded agent-browser binary: {error}");
    }
}

fn run() -> Result<(), Box<dyn Error>> {
    println!("cargo:rerun-if-changed=build.rs");

    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR")?);
    let page_agent_source =
        manifest_dir.join("node_modules/page-agent/dist/iife/page-agent.demo.js");
    println!("cargo:rerun-if-changed={}", page_agent_source.display());

    let out_dir = PathBuf::from(env::var("OUT_DIR")?);
    prepare_sanitized_page_agent_bundle(&page_agent_source, &out_dir)?;

    let target_os = env::var("CARGO_CFG_TARGET_OS")?;
    let target_arch = env::var("CARGO_CFG_TARGET_ARCH")?;
    let asset_name = asset_name_for_target(&target_os, &target_arch)?;
    let download_url = format!(
        "https://github.com/vercel-labs/agent-browser/releases/download/{VERSION}/{asset_name}"
    );

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

fn prepare_sanitized_page_agent_bundle(
    source_file: &PathBuf,
    out_dir: &PathBuf,
) -> Result<(), Box<dyn Error>> {
    let source = fs::read_to_string(source_file)?;
    let sanitized = replace_js_string_constant(&source, "DEMO_BASE_URL", "about:blank");
    fs::write(out_dir.join("page-agent.demo.sanitized.js"), sanitized)?;
    Ok(())
}

fn replace_js_string_constant(source: &str, constant_name: &str, replacement: &str) -> String {
    let replacement_value =
        serde_json::to_string(replacement).unwrap_or_else(|_| "\"\"".to_string());
    let needle = format!("{constant_name}=");

    let mut output = source.to_string();
    let mut search_start = 0;

    while let Some(relative_index) = output[search_start..].find(&needle) {
        let name_index = search_start + relative_index;
        let mut value_start = name_index + needle.len();

        while value_start < output.len()
            && output[value_start..]
                .chars()
                .next()
                .map(|ch| ch.is_whitespace())
                .unwrap_or(false)
        {
            value_start += output[value_start..]
                .chars()
                .next()
                .map(|ch| ch.len_utf8())
                .unwrap_or(1);
        }

        let quote_char = match output[value_start..].chars().next() {
            Some('"') => '"',
            Some('\'') => '\'',
            _ => {
                search_start = value_start;
                continue;
            }
        };

        let content_start = value_start + quote_char.len_utf8();
        let mut escaped = false;
        let mut closing_quote = None;

        for (offset, ch) in output[content_start..].char_indices() {
            if escaped {
                escaped = false;
                continue;
            }
            if ch == '\\' {
                escaped = true;
                continue;
            }
            if ch == quote_char {
                closing_quote = Some(content_start + offset);
                break;
            }
        }

        let Some(closing_quote) = closing_quote else {
            break;
        };

        output.replace_range(value_start..=closing_quote, &replacement_value);
        search_start = value_start + replacement_value.len();
    }

    output
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
