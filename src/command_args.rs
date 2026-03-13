//! Shared command argument parsing utilities.

use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecutablePathPrefill {
    AlreadyProvided,
    Injected,
    Unavailable,
}

/// Parse command arguments from either an args array or a shell-quoted command string.
///
/// Returns the parsed arguments on success, or an error message if neither args nor command
/// contain valid non-empty arguments.
pub fn build_args(
    command: &Option<String>,
    args: &Option<Vec<String>>,
) -> Result<Vec<String>, String> {
    if let Some(args) = args {
        if !args.is_empty() {
            return Ok(args.clone());
        }
    }

    if let Some(command) = command {
        if let Some(parsed) = shlex::split(command) {
            if !parsed.is_empty() {
                return Ok(parsed);
            }
        }
    }

    Err("provide non-empty args or command".to_string())
}

pub fn ensure_executable_path_arg(
    args: &mut Vec<String>,
    detected_browser_path: Option<&Path>,
) -> ExecutablePathPrefill {
    if args
        .iter()
        .any(|arg| arg == "--executable-path" || arg.starts_with("--executable-path="))
    {
        return ExecutablePathPrefill::AlreadyProvided;
    }

    if let Some(path) = detected_browser_path {
        args.push(format!("--executable-path={}", path.to_string_lossy()));
        return ExecutablePathPrefill::Injected;
    }

    ExecutablePathPrefill::Unavailable
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn build_args_prefers_args_over_command() {
        let args = Some(vec!["arg1".to_string(), "arg2".to_string()]);
        let command = Some("other command".to_string());

        let result = build_args(&command, &args).unwrap();
        assert_eq!(result, vec!["arg1", "arg2"]);
    }

    #[test]
    fn build_args_parses_command_when_args_empty() {
        let args = Some(vec![]);
        let command = Some("echo 'hello world'".to_string());

        let result = build_args(&command, &args).unwrap();
        assert_eq!(result, vec!["echo", "hello world"]);
    }

    #[test]
    fn build_args_returns_error_when_both_empty() {
        let args = Some(vec![]);
        let command = Some("".to_string());

        let result = build_args(&command, &args);
        assert!(result.is_err());
    }

    #[test]
    fn build_args_returns_error_when_both_none() {
        let result = build_args(&None, &None);
        assert!(result.is_err());
        assert_eq!(result.unwrap_err(), "provide non-empty args or command");
    }

    #[test]
    fn ensure_executable_path_arg_appends_when_missing() {
        let mut args = vec!["open".to_string(), "https://example.com".to_string()];
        let detected = PathBuf::from("/detected/chrome");

        let result = ensure_executable_path_arg(&mut args, Some(detected.as_path()));

        assert_eq!(result, ExecutablePathPrefill::Injected);
        assert!(args
            .iter()
            .any(|arg| arg == "--executable-path=/detected/chrome"));
    }

    #[test]
    fn ensure_executable_path_arg_does_not_override_equals_form() {
        let mut args = vec![
            "open".to_string(),
            "--executable-path=/custom/browser".to_string(),
        ];

        let result = ensure_executable_path_arg(&mut args, Some(Path::new("/detected/chrome")));

        assert_eq!(result, ExecutablePathPrefill::AlreadyProvided);
        let count = args
            .iter()
            .filter(|arg| arg.starts_with("--executable-path"))
            .count();
        assert_eq!(count, 1);
        assert!(args
            .iter()
            .any(|arg| arg == "--executable-path=/custom/browser"));
    }

    #[test]
    fn ensure_executable_path_arg_does_not_override_split_form() {
        let mut args = vec![
            "open".to_string(),
            "--executable-path".to_string(),
            "/custom/browser".to_string(),
        ];

        let result = ensure_executable_path_arg(&mut args, Some(Path::new("/detected/chrome")));

        assert_eq!(result, ExecutablePathPrefill::AlreadyProvided);
        let count = args
            .iter()
            .filter(|arg| arg.starts_with("--executable-path"))
            .count();
        assert_eq!(count, 1);
    }

    #[test]
    fn ensure_executable_path_arg_noop_without_detected_path() {
        let mut args = vec!["open".to_string()];

        let result = ensure_executable_path_arg(&mut args, None);

        assert_eq!(result, ExecutablePathPrefill::Unavailable);
        assert_eq!(args, vec!["open"]);
    }
}
