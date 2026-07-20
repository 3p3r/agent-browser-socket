use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use flate2::Compression;
use image::ImageFormat;
use std::collections::BTreeMap;
use std::env;
use std::error::Error;
use std::fs;
use std::io::{Cursor, Read, Write};
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

const VERSION: &str = "v0.32.3";
const SKILLS_ARCHIVE_URL: &str =
    "https://github.com/vercel-labs/agent-browser/archive/refs/heads/main.tar.gz";
const SKILLS_ARCHIVE_PREFIX: &str = "/skills/agent-browser/";
const LOGO_PNG_BYTES: &[u8] = include_bytes!("logo.png");

fn main() {
    if let Err(error) = run() {
        panic!("failed to prepare embedded agent-browser binary: {error}");
    }
}

fn run() -> Result<(), Box<dyn Error>> {
    println!("cargo:rerun-if-changed=build.rs");

    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR")?);
    println!(
        "cargo:rerun-if-changed={}",
        manifest_dir.join("logo.png").display()
    );

    let page_agent_source =
        manifest_dir.join("node_modules/page-agent/dist/iife/page-agent.demo.js");
    println!("cargo:rerun-if-changed={}", page_agent_source.display());

    let out_dir = PathBuf::from(env::var("OUT_DIR")?);
    prepare_logo_ico(&out_dir)?;
    prepare_sanitized_page_agent_bundle(&page_agent_source, &out_dir)?;
    prepare_upstream_skills(&out_dir)?;

    let target_os = env::var("CARGO_CFG_TARGET_OS")?;
    let target_arch = env::var("CARGO_CFG_TARGET_ARCH")?;
    prepare_windows_exe_icon(&target_os, &out_dir)?;
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

fn prepare_windows_exe_icon(target_os: &str, out_dir: &Path) -> Result<(), Box<dyn Error>> {
    if target_os != "windows" {
        return Ok(());
    }

    let icon_path = out_dir.join("logo.ico");
    let mut resource = winres::WindowsResource::new();
    resource.set_icon(icon_path.to_string_lossy().as_ref());

    if !cfg!(windows) {
        println!("cargo:rerun-if-env-changed=RC");
        let rc_path = discover_rc_compiler()?;
        let shim_path = out_dir.join("rc.exe");
        write_rc_shim(&shim_path, &rc_path)?;
        resource.set_toolkit_path(out_dir.to_string_lossy().as_ref());
    }

    resource.compile()?;
    Ok(())
}

fn discover_rc_compiler() -> Result<PathBuf, Box<dyn Error>> {
    if let Ok(explicit_rc) = env::var("RC") {
        let candidate = PathBuf::from(explicit_rc);
        if candidate.exists() {
            return Ok(candidate);
        }
    }

    for command in ["llvm-rc", "llvm-windres", "x86_64-w64-mingw32-windres"] {
        if let Ok(output) = std::process::Command::new("which").arg(command).output() {
            if output.status.success() {
                let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
                if !path.is_empty() {
                    return Ok(PathBuf::from(path));
                }
            }
        }
    }

    Err("unable to locate a resource compiler; set RC to llvm-rc (or compatible) path".into())
}

fn write_rc_shim(shim_path: &Path, rc_path: &Path) -> Result<(), Box<dyn Error>> {
    let rc_escaped = rc_path.to_string_lossy().replace('"', "\\\"");
    let script = format!("#!/usr/bin/env sh\nexec \"{}\" \"$@\"\n", rc_escaped);
    fs::write(shim_path, script)?;
    #[cfg(unix)]
    {
        let mut permissions = fs::metadata(shim_path)?.permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(shim_path, permissions)?;
    }
    Ok(())
}

fn prepare_logo_ico(out_dir: &Path) -> Result<(), Box<dyn Error>> {
    let image = image::load_from_memory_with_format(LOGO_PNG_BYTES, ImageFormat::Png)?;
    let icon_image = if image.width() > 256 || image.height() > 256 {
        image.thumbnail(256, 256)
    } else {
        image
    };
    let mut encoded = Cursor::new(Vec::new());
    icon_image.write_to(&mut encoded, ImageFormat::Ico)?;
    fs::write(out_dir.join("logo.ico"), encoded.into_inner())?;
    Ok(())
}

fn prepare_upstream_skills(out_dir: &Path) -> Result<(), Box<dyn Error>> {
    let skills_root = out_dir.join("skills");
    fs::create_dir_all(&skills_root)?;

    let client = reqwest::blocking::Client::builder()
        .user_agent("oatmeal-build")
        .build()?;
    let mut upstream_files = discover_upstream_skill_files(&client)?;
    let mut discovered_files = upstream_files.keys().cloned().collect::<Vec<_>>();

    for relative_path in &discovered_files {
        let source = upstream_files
            .remove(relative_path)
            .ok_or_else(|| format!("missing discovered upstream skill file: {relative_path}"))?;
        let processed = process_upstream_skill_file(relative_path, source);
        let destination = skills_root.join(relative_path);
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(destination, processed)?;
    }

    fs::write(
        skills_root.join("references/agentic-commands.md"),
        render_agentic_commands_reference(),
    )?;
    discovered_files.push("references/agentic-commands.md".to_string());

    discovered_files.sort();
    discovered_files.dedup();
    fs::write(
        out_dir.join("skills_manifest.rs"),
        render_skills_manifest_rs(&discovered_files),
    )?;

    Ok(())
}

fn discover_upstream_skill_files(
    client: &reqwest::blocking::Client,
) -> Result<BTreeMap<String, String>, Box<dyn Error>> {
    let response = client.get(SKILLS_ARCHIVE_URL).send()?.error_for_status()?;
    let bytes = response.bytes()?;
    let decoder = GzDecoder::new(Cursor::new(bytes));
    let mut archive = tar::Archive::new(decoder);
    let mut files = BTreeMap::new();

    for entry in archive.entries()? {
        let mut entry = entry?;
        if !entry.header().entry_type().is_file() {
            continue;
        }

        let path = entry.path()?.to_string_lossy().into_owned();
        let Some(prefix_index) = path.find(SKILLS_ARCHIVE_PREFIX) else {
            continue;
        };

        let relative_path = path[prefix_index + SKILLS_ARCHIVE_PREFIX.len()..].to_string();
        if !(relative_path.ends_with(".md") || relative_path.ends_with(".sh")) {
            continue;
        }

        let mut source = String::new();
        entry.read_to_string(&mut source)?;
        files.insert(relative_path, source);
    }

    if files.is_empty() {
        return Err("agent-browser skills archive did not contain any .md or .sh files".into());
    }

    Ok(files)
}

fn process_upstream_skill_file(relative_path: &str, source: String) -> String {
    match relative_path {
        "SKILL.md" => process_skill_md(source),
        "references/commands.md" => {
            let source = format!(
                "{source}\n\n## Synthetic Agentic Commands\n\nSee [agentic-commands.md](references/agentic-commands.md) for `agentic-open` and `agentic-prompt` in this server."
            );
            rewrite_markdown_for_runtime(&source, relative_path)
        }
        path if path.ends_with(".md") => rewrite_markdown_for_runtime(&source, relative_path),
        path if path.ends_with(".sh") => source,
        _ => source,
    }
}

fn process_skill_md(source: String) -> String {
    let mut processed = source;
    processed = processed.replace(
        "allowed-tools: Bash(npx agent-browser:*), Bash(agent-browser:*)",
        "allowed-tools: mcp__oatmeal__shell_command",
    );
    processed = processed.replace(
        "The CLI uses Chrome/Chromium via CDP directly. Install via `npm i -g agent-browser`, `brew install agent-browser`, or `cargo install agent-browser`. Run `agent-browser install` to download Chrome. Run `agent-browser upgrade` to update to the latest version.",
        "Use this skill through the MCP `shell_command` tool in this server. Commands execute in a bash context where `agent-browser` is available. Execute scripts with full bash semantics (pipes, redirections, stdin): `echo \"pass\" | agent-browser auth save github --url https://github.com/login --username user --password-stdin`.",
    );

    if !processed.contains("references/agentic-commands.md") {
        let injection = "| [references/agentic-commands.md](references/agentic-commands.md)       | Synthetic `agentic-open` and `agentic-prompt` behavior in this server |";
        processed = if let Some(index) = processed.find("## Ready-to-Use Templates") {
            format!(
                "{}\n{}\n{}",
                &processed[..index],
                injection,
                &processed[index..]
            )
        } else {
            format!("{processed}\n\n## Synthetic Agentic Commands\n\nSee [references/agentic-commands.md](references/agentic-commands.md).")
        };
    }

    rewrite_markdown_for_runtime(&processed, "SKILL.md")
}

fn rewrite_markdown_for_runtime(source: &str, current_relative_path: &str) -> String {
    let mut out = source.to_string();

    let mut cursor = 0;
    while let Some(open) = out[cursor..].find("](") {
        let start = cursor + open + 2;
        let Some(close_rel) = out[start..].find(')') else {
            break;
        };
        let end = start + close_rel;
        let target = out[start..end].to_string();

        if should_rewrite_link_target(&target) {
            if let Some(rewritten) = resolve_runtime_link(current_relative_path, &target) {
                out.replace_range(start..end, &rewritten);
                cursor = start + rewritten.len() + 1;
                continue;
            }
        }

        cursor = end + 1;
    }

    out
}

fn should_rewrite_link_target(target: &str) -> bool {
    !(target.starts_with("http://")
        || target.starts_with("https://")
        || target.starts_with("mailto:")
        || target.starts_with('#'))
}

fn resolve_runtime_link(current_relative_path: &str, target: &str) -> Option<String> {
    let (path_part, suffix) = split_link_suffix(target);
    let resolved = normalize_path(current_relative_path, path_part)?;
    if !(resolved.ends_with(".md") || resolved.ends_with(".sh")) {
        return None;
    }
    Some(format!("/skills/{resolved}{suffix}"))
}

fn split_link_suffix(target: &str) -> (&str, &str) {
    let hash_index = target.find('#');
    let query_index = target.find('?');
    let split_index = match (query_index, hash_index) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (Some(a), None) => Some(a),
        (None, Some(b)) => Some(b),
        (None, None) => None,
    };

    if let Some(index) = split_index {
        (&target[..index], &target[index..])
    } else {
        (target, "")
    }
}

fn normalize_path(current_relative_path: &str, target: &str) -> Option<String> {
    let mut segments: Vec<&str> = Vec::new();

    if !target.starts_with('/') {
        let parent = Path::new(current_relative_path)
            .parent()
            .unwrap_or_else(|| Path::new(""));
        for component in parent.components() {
            if let std::path::Component::Normal(part) = component {
                segments.push(part.to_str()?);
            }
        }
    }

    let normalized_target = target.trim_start_matches('/');
    for part in normalized_target.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                segments.pop()?;
            }
            value => segments.push(value),
        }
    }

    if segments.is_empty() {
        return None;
    }

    Some(segments.join("/"))
}

fn render_skills_manifest_rs(files: &[String]) -> String {
    let mut out = String::new();
    out.push_str("pub(crate) const SKILLS_ASSETS: &[(&str, &str, &str)] = &[\n");

    for relative_path in files {
        let mime = mime_for_path(relative_path);
        let path_literal = serde_json::to_string(relative_path).unwrap_or_else(|_| "\"\"".into());
        let mime_literal = serde_json::to_string(mime).unwrap_or_else(|_| "\"\"".into());
        out.push_str("    (");
        out.push_str(&path_literal);
        out.push_str(", ");
        out.push_str(&mime_literal);
        out.push_str(", include_str!(concat!(env!(\"OUT_DIR\"), \"/skills/");
        out.push_str(relative_path);
        out.push_str("\"))),\n");
    }

    out.push_str("];\n");
    out
}

fn mime_for_path(path: &str) -> &'static str {
    if path.ends_with(".md") {
        "text/markdown; charset=utf-8"
    } else if path.ends_with(".sh") {
        "text/x-shellscript; charset=utf-8"
    } else {
        "application/octet-stream"
    }
}

fn render_agentic_commands_reference() -> &'static str {
    r#"# Synthetic Agentic Commands

This server supports two synthetic commands in the MCP `shell_command` tool.

## `agentic-open`

- Usage: `agentic-open <url>`
- Translation: rewritten to `open <url>`
- Behavior: after open succeeds, page-agent is injected so follow-up prompts can run in-page.

## `agentic-prompt`

- Usage: `agentic-prompt [<url>] <prompt>`
- Form A: `agentic-prompt <url> <prompt>` rewrites to `open <url>` and runs the prompt after injection.
- Form B: `agentic-prompt <prompt>` keeps the current page and runs the prompt after injection.

## Notes

- URL detection follows command parsing rules used by this server (`://` or `about:` prefix).
- If URL form is used without a prompt, command validation fails.
- These behaviors are implemented by command translation before browser execution.
"#
}

fn prepare_sanitized_page_agent_bundle(
    source_file: &Path,
    out_dir: &Path,
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
