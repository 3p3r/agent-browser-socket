use crate::command_args::{
    build_args, ensure_executable_path_arg, has_passthrough_command, translate_agentic_open,
    translate_agentic_prompt, ExecutablePathPrefill,
};
use crate::configuration::{AppConfig, PageAgentConfig};
use crate::embedded_binary::resolve_binary_path;
use crate::screenshot::capture_all_screenshots;

use rmcp::{
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::*,
    schemars, tool, tool_handler, tool_router,
    transport::stdio,
    ErrorData as McpError, ServerHandler, ServiceExt,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio as ProcessStdio;
use tokio::process::Command;

async fn run_page_agent_injection(
    binary_path: &Path,
    page_agent_config: &PageAgentConfig,
    command_env: Option<&HashMap<String, String>>,
) -> Result<i32, String> {
    let bundle = crate::server::render_page_agent_bundle(page_agent_config);
    let max_chunk_bytes = 20_000;

    let run_eval = |script: String| async move {
        let mut command = Command::new(binary_path);
        command
            .arg("eval")
            .arg(script)
            .stdout(ProcessStdio::piped())
            .stderr(ProcessStdio::piped());

        if let Some(env) = command_env {
            command.envs(env);
        }

        let output = command
            .output()
            .await
            .map_err(|error| format!("failed to spawn page-agent eval: {error}"))?;

        Ok::<i32, String>(output.status.code().unwrap_or(-1))
    };

    let init_exit = run_eval("window.__absPageAgentChunks = [];".to_string()).await?;
    if init_exit != 0 {
        return Ok(init_exit);
    }

    let mut chunk_start = 0;
    while chunk_start < bundle.len() {
        let mut chunk_end = (chunk_start + max_chunk_bytes).min(bundle.len());
        while chunk_end > chunk_start && !bundle.is_char_boundary(chunk_end) {
            chunk_end -= 1;
        }

        if chunk_end == chunk_start {
            break;
        }

        let chunk = &bundle[chunk_start..chunk_end];
        let serialized_chunk = serde_json::to_string(chunk).unwrap();
        let append_script = format!("window.__absPageAgentChunks.push({serialized_chunk});");

        let append_exit = run_eval(append_script).await?;
        if append_exit != 0 {
            return Ok(append_exit);
        }

        chunk_start = chunk_end;
    }

    let finalize_script = r#"(() => {
    if (window.PageAgent) return 'already_loaded';
    const source = (window.__absPageAgentChunks || []).join('');
    delete window.__absPageAgentChunks;
    (0, eval)(source);
    if (!window.PageAgent) throw new Error('PageAgent not found on window after eval');
    return 'loaded';
})()"#;

    run_eval(finalize_script.to_string()).await
}

#[derive(Debug, Serialize, Deserialize, schemars::JsonSchema)]
pub struct SystemScreenshotInput {}

#[derive(Debug, Serialize, Deserialize, schemars::JsonSchema)]
pub struct CommandInput {
    /// Shell-quoted command string to parse
    #[serde(default)]
    pub command: Option<String>,
    /// Command arguments as array
    #[serde(default)]
    pub args: Option<Vec<String>>,
    /// Environment variables to set
    #[serde(default)]
    pub env: Option<HashMap<String, String>>,
}

#[derive(Clone)]
pub struct SystemMcpServer {
    binary_path: PathBuf,
    detected_browser_path: Option<PathBuf>,
    page_agent_config: PageAgentConfig,
    tool_router: ToolRouter<Self>,
}

#[tool_router]
impl SystemMcpServer {
    pub fn new(
        binary_path: PathBuf,
        detected_browser_path: Option<PathBuf>,
        page_agent_config: PageAgentConfig,
    ) -> Self {
        Self {
            binary_path,
            detected_browser_path,
            page_agent_config,
            tool_router: Self::tool_router(),
        }
    }

    #[tool(name = "health", description = "Check server health status")]
    async fn health(&self) -> Result<CallToolResult, McpError> {
        Ok(CallToolResult::success(vec![Content::text(
            r#"{"status": "ok"}"#,
        )]))
    }

    #[tool(name = "version", description = "Get server version information")]
    async fn version(&self) -> Result<CallToolResult, McpError> {
        let version_info = serde_json::json!({
            "version": env!("CARGO_PKG_VERSION")
        });
        Ok(CallToolResult::success(vec![Content::text(
            serde_json::to_string_pretty(&version_info).unwrap_or_default(),
        )]))
    }

    #[tool(
        name = "screenshot_system",
        description = "Capture screenshots of all system monitors"
    )]
    async fn screenshot_system(
        &self,
        Parameters(_input): Parameters<SystemScreenshotInput>,
    ) -> Result<CallToolResult, McpError> {
        let screenshot_result = std::panic::catch_unwind(capture_all_screenshots);

        match screenshot_result {
            Ok(Ok(screenshots)) => {
                let json =
                    serde_json::to_string_pretty(&screenshots).unwrap_or_else(|_| "[]".to_string());
                Ok(CallToolResult::success(vec![Content::text(json)]))
            }
            Ok(Err(error)) => Ok(CallToolResult::error(vec![Content::text(format!(
                "screenshot failed: {}",
                error
            ))])),
            Err(_) => Ok(CallToolResult::error(vec![Content::text(
                "screenshot failed: panic in capture backend".to_string(),
            )])),
        }
    }

    #[tool(
        name = "command",
        description = "Execute agent-browser with custom arguments"
    )]
    async fn command(
        &self,
        Parameters(input): Parameters<CommandInput>,
    ) -> Result<CallToolResult, McpError> {
        let mut arguments = build_args(&input.command, &input.args)
            .map_err(|msg| McpError::invalid_params(msg, None))?;

        let agentic_prompt = translate_agentic_prompt(&mut arguments)
            .map_err(|msg| McpError::invalid_params(msg, None))?;
        let should_inject_page_agent = translate_agentic_open(&mut arguments)
            .map_err(|msg| McpError::invalid_params(msg, None))?
            || agentic_prompt.is_some();

        let prefill =
            ensure_executable_path_arg(&mut arguments, self.detected_browser_path.as_deref());

        let (stdout, stderr, exit_code) = if has_passthrough_command(&arguments) {
            let mut command = Command::new(&self.binary_path);
            command
                .args(&arguments)
                .stdout(ProcessStdio::piped())
                .stderr(ProcessStdio::piped());

            if let Some(env) = &input.env {
                command.envs(env);
            }

            let output = command.output().await.map_err(|e| {
                McpError::internal_error(format!("failed to spawn process: {}", e), None)
            })?;

            (
                String::from_utf8_lossy(&output.stdout).to_string(),
                String::from_utf8_lossy(&output.stderr).to_string(),
                output.status.code().unwrap_or(-1),
            )
        } else {
            (String::new(), String::new(), 0)
        };

        let page_agent_injection = if should_inject_page_agent && exit_code == 0 {
            match run_page_agent_injection(
                &self.binary_path,
                &self.page_agent_config,
                input.env.as_ref(),
            )
            .await
            {
                Ok(injection_exit) => {
                    let prompt_output = if injection_exit == 0 {
                        if let Some(prompt) = agentic_prompt.as_ref() {
                            let mut prompt_command = Command::new(&self.binary_path);
                            prompt_command
                                .arg("eval")
                                .arg(crate::server::build_page_agent_prompt_script(prompt))
                                .stdout(ProcessStdio::piped())
                                .stderr(ProcessStdio::piped());

                            if let Some(env) = &input.env {
                                prompt_command.envs(env);
                            }

                            match prompt_command.output().await {
                                Ok(prompt_result) => Some(serde_json::json!({
                                    "stdout": String::from_utf8_lossy(&prompt_result.stdout),
                                    "stderr": String::from_utf8_lossy(&prompt_result.stderr),
                                    "exit_code": prompt_result.status.code().unwrap_or(-1)
                                })),
                                Err(error) => Some(serde_json::json!({
                                    "stdout": "",
                                    "stderr": format!("failed to spawn page-agent prompt eval: {error}"),
                                    "exit_code": -1
                                })),
                            }
                        } else {
                            None
                        }
                    } else {
                        None
                    };

                    Some(serde_json::json!({
                        "stdout": "",
                        "stderr": "",
                        "exit_code": injection_exit,
                        "prompt": prompt_output
                    }))
                }
                Err(error) => Some(serde_json::json!({
                    "stdout": "",
                    "stderr": error,
                    "exit_code": -1
                })),
            }
        } else {
            None
        };

        let install_hint = if prefill == ExecutablePathPrefill::Unavailable {
            Some("executable path auto-detection unavailable; run `agent-browser-socket --command install` to install a browser through this binary")
        } else {
            None
        };

        let result = serde_json::json!({
            "stdout": stdout,
            "stderr": stderr,
            "exit_code": exit_code,
            "hint": install_hint,
            "page_agent_injection": page_agent_injection
        });

        Ok(CallToolResult::success(vec![Content::text(
            serde_json::to_string_pretty(&result).unwrap_or_default(),
        )]))
    }

    #[tool(name = "shutdown", description = "Shutdown the MCP server")]
    async fn shutdown(&self) -> Result<CallToolResult, McpError> {
        let response = serde_json::json!({"status": "closing"});

        // Spawn a task to exit after a brief delay to allow the response to be sent
        tokio::spawn(async {
            tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
            std::process::exit(0);
        });

        Ok(CallToolResult::success(vec![Content::text(
            serde_json::to_string(&response).unwrap_or_default(),
        )]))
    }
}

#[tool_handler]
impl ServerHandler for SystemMcpServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new(
                "agent-browser-socket",
                env!("CARGO_PKG_VERSION"),
            ))
            .with_instructions(
                "MCP server that mirrors Socket.IO server handlers. \
                 Tools: health, version, shutdown, screenshot_system, command."
                    .to_string(),
            )
    }
}

pub async fn run_mcp_stdio(
    config: AppConfig,
    page_agent_config: PageAgentConfig,
) -> Result<i32, Box<dyn std::error::Error>> {
    let binary_path = resolve_binary_path(config.browser_path.as_deref())?;
    let detected_browser_path = crate::browser_detection::find_chrome_browser();
    let server = SystemMcpServer::new(binary_path, detected_browser_path, page_agent_config);

    let transport = stdio();
    let service = server
        .serve(transport)
        .await
        .map_err(|e| -> Box<dyn std::error::Error> { Box::new(e) })?;
    service
        .waiting()
        .await
        .map_err(|e| -> Box<dyn std::error::Error> { Box::new(e) })?;

    Ok(0)
}
