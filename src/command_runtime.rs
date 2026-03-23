use crate::bashkit_executor::{AgentBrowserPathPolicy, BashkitExecutor, ExecutionResult};
use crate::command_args::{prepare_command, PreparedCommand};
use crate::configuration::PageAgentConfig;
use crate::page_agent_runtime::{run_page_agent_injection, run_page_agent_prompt};
use std::collections::HashMap;
use std::path::Path;

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
    let execution = executor.execute(&prepared, command_env).await;

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
    match run_page_agent_injection(binary_path, page_agent_config, command_env).await {
        Ok(injection_exit) => {
            let prompt = if injection_exit == 0 {
                if let Some(prompt) = prepared.agentic_prompt.as_ref() {
                    match run_page_agent_prompt(binary_path, prompt, command_env).await {
                        Ok(prompt_result) => Some(PromptExecutionResult {
                            stdout: prompt_result.stdout,
                            stderr: prompt_result.stderr,
                            exit_code: prompt_result.exit_code,
                        }),
                        Err(error) => Some(PromptExecutionResult {
                            stdout: String::new(),
                            stderr: error,
                            exit_code: -1,
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
        Err(error) => InjectionExecutionResult {
            stdout: String::new(),
            stderr: error,
            exit_code: -1,
            prompt: None,
        },
    }
}
