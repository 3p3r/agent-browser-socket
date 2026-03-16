use crate::command_args::{
    build_args, ensure_executable_path_arg, has_passthrough_command, translate_agentic_open,
    translate_agentic_prompt, ExecutablePathPrefill,
};
use crate::configuration::load_config;
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
use std::path::PathBuf;
use std::process::Stdio as ProcessStdio;
use tokio::process::Command;

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
    public_origin: String,
    tool_router: ToolRouter<Self>,
}

#[tool_router]
impl SystemMcpServer {
    pub fn new(
        binary_path: PathBuf,
        detected_browser_path: Option<PathBuf>,
        public_origin: String,
    ) -> Self {
        Self {
            binary_path,
            detected_browser_path,
            public_origin,
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
        let should_inject_page_agent =
            translate_agentic_open(&mut arguments) || agentic_prompt.is_some();

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
            let mut injection = Command::new(&self.binary_path);
            injection
                .arg("eval")
                .arg(crate::server::build_page_agent_injection_script(
                    &self.public_origin,
                ))
                .stdout(ProcessStdio::piped())
                .stderr(ProcessStdio::piped());

            if let Some(env) = &input.env {
                injection.envs(env);
            }

            match injection.output().await {
                Ok(injection_output) => {
                    let injection_exit = injection_output.status.code().unwrap_or(-1);
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
                        "stdout": String::from_utf8_lossy(&injection_output.stdout),
                        "stderr": String::from_utf8_lossy(&injection_output.stderr),
                        "exit_code": injection_exit,
                        "prompt": prompt_output
                    }))
                }
                Err(error) => Some(serde_json::json!({
                    "stdout": "",
                    "stderr": format!("failed to spawn page-agent eval injection: {error}"),
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

pub async fn run_mcp_stdio() -> Result<i32, Box<dyn std::error::Error>> {
    let config = load_config()?;
    let binary_path = resolve_binary_path(config.browser_path.as_deref())?;
    let detected_browser_path = crate::browser_detection::find_chrome_browser();
    let public_host = if config.host == "0.0.0.0" {
        "localhost".to_string()
    } else {
        config.host.clone()
    };
    let public_origin = format!("http://{public_host}:{}", config.port);
    let server = SystemMcpServer::new(binary_path, detected_browser_path, public_origin);

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
