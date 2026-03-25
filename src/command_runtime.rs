use crate::bashkit_executor::{AgentBrowserPathPolicy, BashkitExecutor, ExecutionResult};
use crate::command_args::{prepare_command, PreparedCommand};
use crate::configuration::PageAgentConfig;
use crate::page_agent_runtime::{run_page_agent_injection, run_page_agent_prompt};
use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;
use tokio::time::timeout;

pub struct PromptExecutionResult {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
}

pub struct InjectionExecutionResult {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
    pub prompt: Option<PromptExecutionResult>,
}

pub struct CommandExecutionResult {
    pub execution: ExecutionResult,
    pub page_agent_injection: Option<InjectionExecutionResult>,
}

#[derive(Clone)]
pub enum CommandExecutionMode {
    Mcp { cache_root: std::path::PathBuf },
}

pub fn prepare_script_command(script: &str) -> Result<PreparedCommand, String> {
    prepare_command(script)
}

pub async fn execute_prepared_command(
    binary_path: &Path,
    page_agent_config: &PageAgentConfig,
    prepared: PreparedCommand,
    command_env: Option<&HashMap<String, String>>,
    execution_mode: CommandExecutionMode,
) -> CommandExecutionResult {
    let CommandExecutionMode::Mcp { cache_root } = execution_mode;
    let executor = BashkitExecutor::new_with_path_policy(
        binary_path.to_path_buf(),
        AgentBrowserPathPolicy::McpCacheRooted(cache_root),
    );
    const SHELL_TIMEOUT: Duration = Duration::from_secs(300);

    let execution = match timeout(SHELL_TIMEOUT, executor.execute(&prepared, command_env)).await {
        Ok(result) => result,
        Err(_elapsed) => ExecutionResult {
            stdout: String::new(),
            stderr: format!(
                "shell execution timed out after {} seconds",
                SHELL_TIMEOUT.as_secs()
            ),
            exit_code: 124,
            files: Vec::new(),
        },
    };

    let page_agent_injection = if prepared.should_inject_page_agent && execution.exit_code == 0 {
        Some(run_page_agent_followups(binary_path, page_agent_config, &prepared, command_env).await)
    } else {
        None
    };

    CommandExecutionResult {
        execution,
        page_agent_injection,
    }
}

async fn run_page_agent_followups(
    binary_path: &Path,
    page_agent_config: &PageAgentConfig,
    prepared: &PreparedCommand,
    command_env: Option<&HashMap<String, String>>,
) -> InjectionExecutionResult {
    const INJECTION_TIMEOUT: Duration = Duration::from_secs(120);

    match timeout(
        INJECTION_TIMEOUT,
        run_page_agent_injection(binary_path, page_agent_config, command_env),
    )
    .await
    {
        Ok(Ok(injection_exit)) => {
            let prompt = if injection_exit == 0 {
                if let Some(prompt) = prepared.agentic_prompt.as_ref() {
                    const PROMPT_TIMEOUT: Duration = Duration::from_secs(60);

                    match timeout(
                        PROMPT_TIMEOUT,
                        run_page_agent_prompt(binary_path, prompt, command_env),
                    )
                    .await
                    {
                        Ok(Ok(prompt_result)) => Some(PromptExecutionResult {
                            stdout: prompt_result.stdout,
                            stderr: prompt_result.stderr,
                            exit_code: prompt_result.exit_code,
                        }),
                        Ok(Err(error)) => Some(PromptExecutionResult {
                            stdout: String::new(),
                            stderr: error,
                            exit_code: -1,
                        }),
                        Err(_elapsed) => Some(PromptExecutionResult {
                            stdout: String::new(),
                            stderr: format!(
                                "page-agent prompt timed out after {} seconds",
                                PROMPT_TIMEOUT.as_secs()
                            ),
                            exit_code: 124,
                        }),
                    }
                } else {
                    None
                }
            } else {
                None
            };

            InjectionExecutionResult {
                stdout: String::new(),
                stderr: String::new(),
                exit_code: injection_exit,
                prompt,
            }
        }
        Ok(Err(error)) => InjectionExecutionResult {
            stdout: String::new(),
            stderr: error,
            exit_code: -1,
            prompt: None,
        },
        Err(_elapsed) => InjectionExecutionResult {
            stdout: String::new(),
            stderr: format!(
                "page-agent injection timed out after {} seconds",
                INJECTION_TIMEOUT.as_secs()
            ),
            exit_code: 124,
            prompt: None,
        },
    }
}
