use ftmi::extract_paths_from_text;
use std::collections::HashSet;
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedCommand {
    pub script: String,
    pub should_inject_page_agent: bool,
    pub agentic_prompt: Option<String>,
    pub referenced_paths: HashSet<PathBuf>,
}

#[cfg(test)]
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
pub fn has_passthrough_command(args: &[String]) -> bool {
    !args.is_empty()
}

#[cfg(test)]
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

#[cfg(test)]
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

pub fn preprocess_agentic_script(script: &str) -> Result<(String, bool, Option<String>), String> {
    if script.trim().is_empty() {
        return Err("command cannot be empty".to_string());
    }

    let has_open = script.contains("agentic-open");
    let has_prompt = script.contains("agentic-prompt");
    let should_inject = has_open || has_prompt;

    let prompt = if has_prompt {
        if let Some(tokens) = shlex::split(script) {
            if let Some(index) = tokens.iter().position(|token| token == "agentic-prompt") {
                let Some(first_arg) = tokens.get(index + 1).cloned() else {
                    return Err("usage: agent-browser agentic-prompt [<url>] <prompt>".to_string());
                };

                let first_looks_like_url =
                    first_arg.contains("://") || first_arg.starts_with("about:");

                if first_looks_like_url {
                    let Some(prompt) = tokens.get(index + 2).cloned() else {
                        return Err("usage: agent-browser agentic-prompt <url> <prompt>\n  prompt is required when a URL is provided".to_string());
                    };
                    Some(prompt)
                } else {
                    Some(first_arg)
                }
            } else {
                None
            }
        } else {
            None
        }
    } else {
        None
    };

    let rewritten = script
        .replace("agentic-open", "open")
        .replace("agentic-prompt", "open");

    Ok((rewritten, should_inject, prompt))
}

pub fn prepare_command(script: &str) -> Result<PreparedCommand, String> {
    let (script, should_inject_page_agent, agentic_prompt) = preprocess_agentic_script(script)?;
    let referenced_paths = extract_paths_from_text(&script)
        .into_iter()
        .map(PathBuf::from)
        .collect();

    Ok(PreparedCommand {
        script,
        should_inject_page_agent,
        agentic_prompt,
        referenced_paths,
    })
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

    #[test]
    fn preprocess_agentic_script_allows_shell_operators_with_synthetic_commands() {
        let result =
            preprocess_agentic_script("agent-browser agentic-open https://example.com | cat")
                .unwrap();
        assert_eq!(result.0, "agent-browser open https://example.com | cat");
        assert!(result.1);
        assert!(result.2.is_none());

        let result = preprocess_agentic_script(
            "agent-browser agentic-prompt https://example.com 'hello' && echo done",
        )
        .unwrap();
        assert_eq!(
            result.0,
            "agent-browser open https://example.com 'hello' && echo done"
        );
        assert!(result.1);
        assert_eq!(result.2, Some("hello".to_string()));
    }

    #[test]
    fn preprocess_agentic_script_passes_non_agentic_compound_shell() {
        let result = preprocess_agentic_script("echo hello | cat").unwrap();
        assert_eq!(result.0, "echo hello | cat");
        assert!(!result.1);
        assert!(result.2.is_none());
    }

    #[test]
    fn preprocess_agentic_script_rewrites_without_prefix_too() {
        let result = preprocess_agentic_script("agentic-open https://example.com").unwrap();
        assert_eq!(result.0, "open https://example.com");
        assert!(result.1);
        assert!(result.2.is_none());

        let result =
            preprocess_agentic_script("agentic-prompt https://example.com 'hello'").unwrap();
        assert_eq!(result.0, "open https://example.com 'hello'");
        assert!(result.1);
        assert_eq!(result.2, Some("hello".to_string()));
    }

    #[test]
    fn prepare_command_extracts_paths_from_rewritten_script() {
        let prepared =
            prepare_command("agent-browser agentic-open https://example.com > /tmp/report.json")
                .unwrap();

        assert_eq!(
            prepared.script,
            "agent-browser open https://example.com > /tmp/report.json"
        );
        assert!(prepared.should_inject_page_agent);
        assert!(prepared.agentic_prompt.is_none());
        assert!(prepared
            .referenced_paths
            .contains(&PathBuf::from("/tmp/report.json")));
    }
}
