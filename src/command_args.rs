use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecutablePathPrefill {
    AlreadyProvided,
    Injected,
    Unavailable,
}

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
        let executable_path_arg = format!("--executable-path={}", path.to_string_lossy());
        let insert_index = args
            .iter()
            .position(|arg| !arg.starts_with('-'))
            .map(|index| index + 1)
            .unwrap_or(0);
        args.insert(insert_index, executable_path_arg);
        return ExecutablePathPrefill::Injected;
    }

    ExecutablePathPrefill::Unavailable
}

pub fn has_passthrough_command(args: &[String]) -> bool {
    !args.is_empty()
}

pub fn translate_agentic_open(args: &mut [String]) -> Result<bool, String> {
    if let Some(index) = args.iter().position(|arg| arg == "agentic-open") {
        if args.get(index + 1).is_some() {
            args[index] = "open".to_string();
            return Ok(true);
        }
        return Err("usage: agentic-open <url>".to_string());
    }

    Ok(false)
}

pub fn translate_agentic_prompt(args: &mut Vec<String>) -> Result<Option<String>, String> {
    if let Some(index) = args.iter().position(|arg| arg == "agentic-prompt") {
        let Some(first_arg) = args.get(index + 1).cloned() else {
            return Err("usage: agentic-prompt [<url>] <prompt>".to_string());
        };

        let second_arg = args.get(index + 2).cloned();
        let first_looks_like_url = first_arg.contains("://") || first_arg.starts_with("about:");

        if first_looks_like_url {
            let Some(prompt) = second_arg else {
                return Err("usage: agentic-prompt <url> <prompt>\n  prompt is required when a URL is provided".to_string());
            };
            args[index] = "open".to_string();
            args[index + 1] = first_arg;
            args.remove(index + 2);
            return Ok(Some(prompt));
        }

        args.remove(index + 1);
        args.remove(index);
        return Ok(Some(first_arg));
    }

    Ok(None)
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
    fn ensure_executable_path_arg_inserts_after_subcommand() {
        let mut args = vec!["screenshot".to_string(), "--full".to_string()];
        let detected = PathBuf::from("/detected/chrome");

        let result = ensure_executable_path_arg(&mut args, Some(detected.as_path()));

        assert_eq!(result, ExecutablePathPrefill::Injected);
        assert_eq!(args[0], "screenshot");
        assert_eq!(args[1], "--executable-path=/detected/chrome");
        assert_eq!(args[2], "--full");
    }

    #[test]
    fn has_passthrough_command_true_for_flag_only_args() {
        let args = vec!["--version".to_string()];
        assert!(has_passthrough_command(&args));
    }

    #[test]
    fn has_passthrough_command_false_for_empty_args() {
        let args: Vec<String> = vec![];
        assert!(!has_passthrough_command(&args));
    }

    #[test]
    fn has_passthrough_command_true_for_positional_args() {
        let args = vec!["open".to_string(), "https://example.com".to_string()];
        assert!(has_passthrough_command(&args));
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

    #[test]
    fn translate_agentic_open_rewrites_command_and_returns_true() {
        let mut args = vec![
            "agentic-open".to_string(),
            "https://example.com".to_string(),
        ];

        let should_inject = translate_agentic_open(&mut args).unwrap();

        assert!(should_inject);
        assert_eq!(args, vec!["open", "https://example.com"]);
    }

    #[test]
    fn translate_agentic_open_returns_false_for_plain_open() {
        let mut args = vec!["open".to_string(), "https://example.com".to_string()];

        let should_inject = translate_agentic_open(&mut args).unwrap();

        assert!(!should_inject);
        assert_eq!(args, vec!["open", "https://example.com"]);
    }

    #[test]
    fn translate_agentic_open_requires_target_url() {
        let mut missing_url = vec!["agentic-open".to_string()];
        assert!(translate_agentic_open(&mut missing_url).is_err());

        let mut unrelated = vec!["goto".to_string(), "https://example.com".to_string()];
        assert!(!translate_agentic_open(&mut unrelated).unwrap());
        assert_eq!(unrelated, vec!["goto", "https://example.com"]);
    }

    #[test]
    fn translate_agentic_open_handles_leading_flags() {
        let mut args = vec![
            "--headed".to_string(),
            "agentic-open".to_string(),
            "https://example.com".to_string(),
        ];

        let should_inject = translate_agentic_open(&mut args).unwrap();

        assert!(should_inject);
        assert_eq!(args, vec!["--headed", "open", "https://example.com"]);
    }

    #[test]
    fn translate_agentic_prompt_rewrites_command_and_extracts_prompt() {
        let mut args = vec![
            "agentic-prompt".to_string(),
            "https://example.com".to_string(),
            "write tests".to_string(),
        ];

        let prompt = translate_agentic_prompt(&mut args).expect("translate");

        assert_eq!(prompt, Some("write tests".to_string()));
        assert_eq!(args, vec!["open", "https://example.com"]);
    }

    #[test]
    fn translate_agentic_prompt_supports_prompt_only_current_page() {
        let mut args = vec!["agentic-prompt".to_string(), "write tests".to_string()];

        let prompt = translate_agentic_prompt(&mut args).expect("translate");

        assert_eq!(prompt, Some("write tests".to_string()));
        assert!(args.is_empty());
    }

    #[test]
    fn translate_agentic_prompt_requires_url_and_prompt() {
        let mut missing_prompt = vec!["agentic-prompt".to_string()];
        assert!(translate_agentic_prompt(&mut missing_prompt).is_err());

        let mut unrelated = vec!["open".to_string(), "https://example.com".to_string()];
        assert_eq!(translate_agentic_prompt(&mut unrelated).unwrap(), None);
        assert_eq!(unrelated, vec!["open", "https://example.com"]);
    }
}
