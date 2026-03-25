use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

pub fn find_chrome_browser() -> Option<PathBuf> {
    let default_path = get_default_browser_path();
    if let Some(path) = default_path {
        if path.exists() && is_chrome_like_path(&path) {
            return Some(path);
        }
    }

    for path in known_paths() {
        if path.exists() {
            return Some(path);
        }
    }

    search_desktop_shortcuts()
        .into_iter()
        .find(|target| target.exists())
}

fn is_chrome_like(exec_path: &str) -> bool {
    let lower = exec_path.to_ascii_lowercase();
    lower.contains("chrome")
        || lower.contains("chromium")
        || lower.contains("edge")
        || lower.contains("msedge")
        || lower.contains("brave")
}

fn is_chrome_like_path(path: &Path) -> bool {
    is_chrome_like(&path.to_string_lossy())
}

fn known_paths() -> Vec<PathBuf> {
    #[cfg(target_os = "windows")]
    {
        let mut paths = vec![
            PathBuf::from(r"C:\Program Files\Google\Chrome\Application\chrome.exe"),
            PathBuf::from(r"C:\Program Files (x86)\Google\Chrome\Application\chrome.exe"),
            PathBuf::from(r"C:\Program Files\Microsoft\Edge\Application\msedge.exe"),
            PathBuf::from(r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe"),
            PathBuf::from(r"C:\Program Files\BraveSoftware\Brave-Browser\Application\brave.exe"),
            PathBuf::from(
                r"C:\Program Files (x86)\BraveSoftware\Brave-Browser\Application\brave.exe",
            ),
            PathBuf::from(r"C:\Program Files\Chromium\Application\chrome.exe"),
            PathBuf::from(r"C:\Program Files (x86)\Chromium\Application\chrome.exe"),
        ];

        if let Some(home) = dirs::home_dir() {
            paths.push(home.join(r"AppData\Local\Google\Chrome\Application\chrome.exe"));
            paths.push(home.join(r"AppData\Local\Microsoft\Edge\Application\msedge.exe"));
            paths.push(
                home.join(r"AppData\Local\BraveSoftware\Brave-Browser\Application\brave.exe"),
            );
            paths.push(home.join(r"AppData\Local\Chromium\Application\chrome.exe"));
        }

        return paths;
    }

    #[cfg(target_os = "macos")]
    {
        let mut paths = vec![
            PathBuf::from("/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"),
            PathBuf::from("/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge"),
            PathBuf::from("/Applications/Brave Browser.app/Contents/MacOS/Brave Browser"),
            PathBuf::from("/Applications/Chromium.app/Contents/MacOS/Chromium"),
        ];

        if let Some(home) = dirs::home_dir() {
            paths.push(home.join("Applications/Google Chrome.app/Contents/MacOS/Google Chrome"));
            paths.push(home.join("Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge"));
            paths.push(home.join("Applications/Brave Browser.app/Contents/MacOS/Brave Browser"));
            paths.push(home.join("Applications/Chromium.app/Contents/MacOS/Chromium"));
        }

        return paths;
    }

    #[cfg(target_os = "linux")]
    {
        return vec![
            PathBuf::from("/usr/bin/google-chrome"),
            PathBuf::from("/usr/bin/google-chrome-stable"),
            PathBuf::from("/usr/bin/chromium"),
            PathBuf::from("/usr/bin/chromium-browser"),
            PathBuf::from("/usr/bin/microsoft-edge"),
            PathBuf::from("/usr/bin/brave-browser"),
            PathBuf::from("/opt/google/chrome/chrome"),
            PathBuf::from("/opt/google/chrome/google-chrome"),
            PathBuf::from("/opt/chromium.org/chromium/chromium"),
            PathBuf::from("/opt/brave.com/brave/brave-browser"),
            PathBuf::from("/snap/bin/brave"),
            PathBuf::from("/snap/bin/chromium"),
            PathBuf::from("/snap/bin/microsoft-edge"),
        ];
    }

    #[allow(unreachable_code)]
    Vec::new()
}

fn search_desktop_shortcuts() -> Vec<PathBuf> {
    let desktop_dir = match dirs::home_dir() {
        Some(home) => home.join("Desktop"),
        None => return Vec::new(),
    };

    if !desktop_dir.exists() {
        return Vec::new();
    }

    let mut shortcuts = Vec::new();

    let entries = match fs::read_dir(desktop_dir) {
        Ok(entries) => entries,
        Err(_) => return shortcuts,
    };

    for entry in entries.flatten() {
        let path = entry.path();
        let file_name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default()
            .to_string();

        let target = shortcut_target_for_platform(&path, &file_name);
        if let Some(target) = target {
            if is_chrome_like_path(&target) {
                shortcuts.push(target);
            }
        }
    }

    shortcuts
}

fn get_default_browser_path() -> Option<PathBuf> {
    #[cfg(target_os = "windows")]
    {
        return windows_default_browser_path();
    }

    #[cfg(target_os = "macos")]
    {
        return macos_default_browser_path();
    }

    #[cfg(target_os = "linux")]
    {
        return linux_default_browser_path();
    }

    #[allow(unreachable_code)]
    None
}

#[cfg(target_os = "windows")]
fn windows_default_browser_path() -> Option<PathBuf> {
    let user_choice = run_command(
        "reg",
        &[
            "query",
            r"HKEY_CURRENT_USER\Software\Microsoft\Windows\Shell\Associations\UrlAssociations\http\UserChoice",
            "/v",
            "ProgId",
        ],
    )?;
    let prog_id = user_choice
        .lines()
        .last()
        .and_then(|line| line.split_whitespace().last())?
        .trim()
        .to_string();

    let command_key = format!(r"HKEY_CLASSES_ROOT\{}\shell\open\command", prog_id);
    let command_output = run_command("reg", &["query", &command_key, "/ve"])?;
    extract_quoted_path(&command_output).or_else(|| extract_first_token_path(&command_output))
}

#[cfg(target_os = "macos")]
fn macos_default_browser_path() -> Option<PathBuf> {
    let output = run_command(
        "defaults",
        &[
            "read",
            "com.apple.LaunchServices/com.apple.launchservices.secure",
        ],
    )?;

    let bundle_id = output.lines().find_map(|line| {
        let trimmed = line.trim();
        if !trimmed.starts_with("LSHandlerRoleAll") {
            return None;
        }

        let value = trimmed
            .split('=')
            .nth(1)?
            .trim()
            .trim_end_matches(';')
            .trim();
        let value = value.trim_matches('"');
        if value.is_empty() {
            None
        } else {
            Some(value.to_string())
        }
    })?;

    let query = format!("kMDItemCFBundleIdentifier = \"{}\"", bundle_id);
    let mdfind_output = run_command("mdfind", &[&query])?;
    let app_path = mdfind_output
        .lines()
        .find(|line| !line.trim().is_empty())?
        .trim();

    let app = PathBuf::from(app_path);
    let app_name = app.file_stem()?.to_string_lossy().to_string();
    Some(app.join("Contents").join("MacOS").join(app_name))
}

#[cfg(target_os = "linux")]
fn linux_default_browser_path() -> Option<PathBuf> {
    let desktop_file = run_command("xdg-settings", &["get", "default-web-browser"])?;
    let desktop_file = desktop_file.trim();
    if desktop_file.is_empty() {
        return None;
    }

    let mut locations = Vec::new();
    if let Some(home) = dirs::home_dir() {
        locations.push(home.join(".local/share/applications"));
    }
    locations.push(PathBuf::from("/usr/share/applications"));
    locations.push(PathBuf::from("/usr/local/share/applications"));

    let desktop_path = locations
        .into_iter()
        .map(|location| location.join(desktop_file))
        .find(|path| path.exists())?;

    let content = fs::read_to_string(desktop_path).ok()?;
    extract_exec_path_from_desktop_content(&content)
}

fn run_command(program: &str, args: &[&str]) -> Option<String> {
    let output = Command::new(program).args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }

    String::from_utf8(output.stdout).ok()
}

#[cfg(target_os = "windows")]
fn extract_quoted_path(text: &str) -> Option<PathBuf> {
    let first_quote = text.find('"')?;
    let remaining = &text[first_quote + 1..];
    let second_quote = remaining.find('"')?;
    let value = &remaining[..second_quote];
    if value.is_empty() {
        None
    } else {
        Some(PathBuf::from(value))
    }
}

#[cfg(target_os = "windows")]
fn extract_first_token_path(text: &str) -> Option<PathBuf> {
    text.split_whitespace()
        .find(|token| token.contains('/') || token.contains('\\'))
        .map(|token| {
            token
                .trim_matches('"')
                .trim_end_matches("%1")
                .trim()
                .to_string()
        })
        .filter(|token| !token.is_empty())
        .map(PathBuf::from)
}

fn extract_exec_path_from_desktop_content(content: &str) -> Option<PathBuf> {
    let exec_line = content
        .lines()
        .find(|line| line.trim_start().starts_with("Exec="))?;
    let value = exec_line.trim_start().strip_prefix("Exec=")?.trim();
    parse_desktop_exec_value(value)
}

fn parse_desktop_exec_value(value: &str) -> Option<PathBuf> {
    let chars = value.chars().peekable();
    let mut token = String::new();
    let mut in_quotes = false;

    for ch in chars {
        if ch == '"' {
            in_quotes = !in_quotes;
            continue;
        }

        if !in_quotes && ch.is_whitespace() {
            break;
        }

        token.push(ch);
    }

    let cleaned = token.trim().to_string();
    if cleaned.is_empty() {
        None
    } else {
        Some(PathBuf::from(clean_desktop_exec_token(&cleaned)))
    }
}

fn clean_desktop_exec_token(token: &str) -> String {
    let placeholders = ["%u", "%U", "%f", "%F", "%i", "%c", "%k"];
    let mut value = token.to_string();
    for placeholder in placeholders {
        value = value.replace(placeholder, "");
    }
    value.trim().to_string()
}

fn shortcut_target_for_platform(path: &Path, file_name: &str) -> Option<PathBuf> {
    #[cfg(target_os = "windows")]
    {
        if !file_name.to_ascii_lowercase().ends_with(".lnk") {
            return None;
        }

        let script = format!(
            "$ws = New-Object -ComObject WScript.Shell; $sc = $ws.CreateShortcut('{}'); $sc.TargetPath",
            path.to_string_lossy().replace('\'', "''")
        );
        let out = run_command("powershell.exe", &["-Command", &script])?;
        let target = out.trim();
        return if target.is_empty() {
            None
        } else {
            Some(PathBuf::from(target))
        };
    }

    #[cfg(target_os = "macos")]
    {
        let full_path = path.to_string_lossy().replace('"', "\\\"");
        let kind_out = run_command("mdls", &["-name", "kMDItemKind", &full_path])?;
        if !kind_out.contains("Alias") {
            return None;
        }

        let script = format!(
            "tell application \"Finder\" to get POSIX path of (original item of item (POSIX file \"{}\") as alias)",
            full_path
        );
        let out = run_command("osascript", &["-e", &script])?;
        let target = out.trim();
        return if target.is_empty() {
            None
        } else {
            Some(PathBuf::from(target))
        };
    }

    #[cfg(target_os = "linux")]
    {
        if !file_name.ends_with(".desktop") {
            return None;
        }

        let content = fs::read_to_string(path).ok()?;
        return extract_exec_path_from_desktop_content(&content);
    }

    #[allow(unreachable_code)]
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_desktop_exec_value_handles_quotes() {
        let parsed = parse_desktop_exec_value("\"/opt/google/chrome/chrome\" --new-window %U")
            .expect("parsed path");
        assert_eq!(parsed, PathBuf::from("/opt/google/chrome/chrome"));
    }

    #[test]
    fn parse_desktop_exec_value_handles_plain_exec() {
        let parsed = parse_desktop_exec_value("/usr/bin/google-chrome-stable %U").expect("path");
        assert_eq!(parsed, PathBuf::from("/usr/bin/google-chrome-stable"));
    }

    #[test]
    fn extract_exec_path_from_desktop_content_reads_exec_line() {
        let content =
            "[Desktop Entry]\nName=Chrome\nExec=/usr/bin/chromium --flag %U\nType=Application\n";
        let parsed = extract_exec_path_from_desktop_content(content).expect("exec path");
        assert_eq!(parsed, PathBuf::from("/usr/bin/chromium"));
    }
}
