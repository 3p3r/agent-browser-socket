use std::path::Path;
#[cfg(any(target_os = "windows", test))]
use std::path::PathBuf;
use std::process::Command;

#[cfg(any(target_os = "windows", test))]
const WINDOWS_APP_USER_MODEL_ID: &str = "com.everruns.oatmeal";
#[cfg(any(target_os = "windows", test))]
const WINDOWS_START_MENU_SHORTCUT: &str = "Oatmeal.lnk";

#[allow(dead_code)]
pub fn open_in_file_manager(path: &Path) -> Result<(), String> {
    let (command, args) = open_in_file_manager_command(path);
    let status = Command::new(&command)
        .args(&args)
        .status()
        .map_err(|error| format!("failed to launch file manager with {command}: {error}"))?;

    if status.success() {
        Ok(())
    } else {
        Err(format!(
            "file manager command failed: {command} {}",
            args.join(" ")
        ))
    }
}

pub fn show_notification(title: &str, body: &str) -> Result<(), String> {
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    {
        notify_rust::Notification::new()
            .summary(title)
            .body(body)
            .show()
            .map(|_| ())
            .map_err(|error| format!("notification failed: {error}"))
    }

    #[cfg(target_os = "windows")]
    {
        ensure_windows_notification_prerequisite()?;
        notify_rust::Notification::new()
            .summary(title)
            .body(body)
            .appname(WINDOWS_APP_USER_MODEL_ID)
            .show()
            .map(|_| ())
            .map_err(|error| format!("notification failed: {error}"))
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
    {
        let _ = title;
        let _ = body;
        Err("notification failed: unsupported platform".to_string())
    }
}

pub fn action_failure_notification(action_name: &str, error: &str) -> Result<(), String> {
    show_notification(
        "Oatmeal action failed",
        &format!("{action_name} failed: {error}"),
    )
}

#[allow(dead_code)]
pub fn open_in_file_manager_command(path: &Path) -> (String, Vec<String>) {
    let target = path.display().to_string();

    #[cfg(target_os = "windows")]
    {
        ("explorer".to_string(), vec![target])
    }

    #[cfg(target_os = "macos")]
    {
        ("open".to_string(), vec![target])
    }

    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        ("xdg-open".to_string(), vec![target])
    }
}

#[cfg(any(target_os = "windows", test))]
fn windows_start_menu_shortcut_path_from(app_data_dir: &Path) -> PathBuf {
    app_data_dir
        .join("Microsoft")
        .join("Windows")
        .join("Start Menu")
        .join("Programs")
        .join(WINDOWS_START_MENU_SHORTCUT)
}

#[cfg(any(target_os = "windows", test))]
fn prepare_windows_notification_prerequisite<FExists, FCreateDir, FCreateShortcut>(
    app_data_dir: Option<PathBuf>,
    current_exe: Option<PathBuf>,
    path_exists: FExists,
    create_dir_all: FCreateDir,
    create_shortcut: FCreateShortcut,
) -> Result<(), String>
where
    FExists: Fn(&Path) -> bool,
    FCreateDir: Fn(&Path) -> Result<(), String>,
    FCreateShortcut: Fn(&Path, &Path) -> Result<(), String>,
{
    let Some(app_data_dir) = app_data_dir else {
        return Err("Windows notification prerequisite missing: APPDATA is not set".to_string());
    };

    let Some(current_exe) = current_exe else {
        return Err(
            "Windows notification prerequisite missing: current executable path is unavailable"
                .to_string(),
        );
    };

    let programs_dir = app_data_dir
        .join("Microsoft")
        .join("Windows")
        .join("Start Menu")
        .join("Programs");

    create_dir_all(&programs_dir)?;

    let shortcut = windows_start_menu_shortcut_path_from(&app_data_dir);
    if path_exists(&shortcut) {
        return Ok(());
    }

    create_shortcut(&current_exe, &shortcut)
}

#[cfg(target_os = "windows")]
fn ensure_windows_notification_prerequisite() -> Result<(), String> {
    prepare_windows_notification_prerequisite(
        std::env::var_os("APPDATA").map(PathBuf::from),
        std::env::current_exe().ok(),
        |path| path.exists(),
        |path| {
            std::fs::create_dir_all(path).map_err(|error| {
                format!(
                    "failed to create Start Menu directory {}: {error}",
                    path.display()
                )
            })
        },
        |target, shortcut| {
            let mut shell_link = mslnk::ShellLink::new(target).map_err(|error| {
                format!(
                    "failed to initialize Windows shortcut target {}: {error}",
                    target.display()
                )
            })?;
            shell_link.set_name(Some("Oatmeal".to_string()));
            shell_link.create_lnk(shortcut).map_err(|error| {
                format!(
                    "failed to create shortcut {} for AppUserModelID {} via mslnk; if toast binding fails, use IShellLinkW + IPropertyStore fallback: {error}",
                    shortcut.display(),
                    WINDOWS_APP_USER_MODEL_ID
                )
            })
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_notification_prerequisite_is_prepared() {
        let app_data = PathBuf::from("C:/Users/example/AppData/Roaming");
        let expected = windows_start_menu_shortcut_path_from(&app_data);

        let result = prepare_windows_notification_prerequisite(
            Some(app_data),
            Some(PathBuf::from("C:/Program Files/Oatmeal/oatmeal.exe")),
            |path| path == expected,
            |_| Ok(()),
            |_, _| Ok(()),
        );
        assert!(result.is_ok());
    }

    #[test]
    fn windows_notification_prerequisite_failure_propagates() {
        let app_data = PathBuf::from("C:/Users/example/AppData/Roaming");
        let result = prepare_windows_notification_prerequisite(
            Some(app_data),
            Some(PathBuf::from("C:/Program Files/Oatmeal/oatmeal.exe")),
            |_| false,
            |_| Ok(()),
            |_, shortcut| {
                Err(format!(
                    "failed to create shortcut {} with AppUserModelID {}",
                    shortcut.display(),
                    WINDOWS_APP_USER_MODEL_ID
                ))
            },
        );
        let error = result.expect_err("expected missing shortcut prerequisite to fail");

        assert!(error.contains("Oatmeal.lnk"));
        assert!(error.contains("com.everruns.oatmeal"));
    }
}
