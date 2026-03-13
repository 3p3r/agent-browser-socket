//! Shared command argument parsing utilities.

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

#[cfg(test)]
mod tests {
    use super::*;

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
}
