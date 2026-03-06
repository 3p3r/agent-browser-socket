//! MCP stdio server implementation.
//!
//! Launched via `agent-browser-socket --mcp`.  Exposes all browser and API tools
//! from the mcp-browser-agent spec over the Model Context Protocol using stdio
//! transport.

use crate::configuration::load_config;
use crate::embedded_binary::resolve_binary_path;

use rmcp::{
    ErrorData as McpError, ServerHandler, ServiceExt,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::*,
    schemars, tool, tool_handler, tool_router,
    transport::stdio,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio as ProcessStdio;
use tokio::process::Command;

// ---------------------------------------------------------------------------
// Input schemas (mirroring tools.ts)
// ---------------------------------------------------------------------------

#[derive(Debug, Serialize, Deserialize, schemars::JsonSchema)]
pub struct BrowserSetViewportInput {
    /// Viewport width in pixels
    #[serde(default)]
    pub width: Option<f64>,
    /// Viewport height in pixels
    #[serde(default)]
    pub height: Option<f64>,
    /// Device scale factor (affects how content is scaled)
    #[serde(default, rename = "deviceScaleFactor")]
    pub device_scale_factor: Option<f64>,
}

#[derive(Debug, Serialize, Deserialize, schemars::JsonSchema)]
pub struct BrowserNavigateInput {
    /// URL to navigate to
    pub url: String,
    /// Navigation timeout in milliseconds
    #[serde(default)]
    pub timeout: Option<f64>,
    /// Navigation wait criteria
    #[serde(default, rename = "waitUntil")]
    pub wait_until: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, schemars::JsonSchema)]
pub struct BrowserScreenshotInput {
    /// Identifier for the screenshot
    pub name: String,
    /// CSS selector for element to capture
    #[serde(default)]
    pub selector: Option<String>,
    /// Capture full page height
    #[serde(default, rename = "fullPage")]
    pub full_page: Option<bool>,
    /// Selectors for elements to mask
    #[serde(default)]
    pub mask: Option<Vec<String>>,
    /// Path to save screenshot (default: user's Downloads folder)
    #[serde(default, rename = "savePath")]
    pub save_path: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, schemars::JsonSchema)]
pub struct BrowserClickInput {
    /// CSS selector for element to click
    pub selector: String,
}

#[derive(Debug, Serialize, Deserialize, schemars::JsonSchema)]
pub struct BrowserFillInput {
    /// CSS selector for input field
    pub selector: String,
    /// Text to enter in the field
    pub value: String,
}

#[derive(Debug, Serialize, Deserialize, schemars::JsonSchema)]
pub struct BrowserSelectInput {
    /// CSS selector for select element
    pub selector: String,
    /// Value or label to select
    pub value: String,
}

#[derive(Debug, Serialize, Deserialize, schemars::JsonSchema)]
pub struct BrowserHoverInput {
    /// CSS selector for element to hover over
    pub selector: String,
}

#[derive(Debug, Serialize, Deserialize, schemars::JsonSchema)]
pub struct BrowserEvaluateInput {
    /// JavaScript code to execute
    pub script: String,
}

#[derive(Debug, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ApiGetInput {
    /// API endpoint URL
    pub url: String,
    /// Request headers
    #[serde(default)]
    pub headers: Option<HashMap<String, String>>,
}

#[derive(Debug, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ApiPostInput {
    /// API endpoint URL
    pub url: String,
    /// Request body data (JSON string)
    pub data: String,
    /// Request headers
    #[serde(default)]
    pub headers: Option<HashMap<String, String>>,
}

#[derive(Debug, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ApiPutInput {
    /// API endpoint URL
    pub url: String,
    /// Request body data (JSON string)
    pub data: String,
    /// Request headers
    #[serde(default)]
    pub headers: Option<HashMap<String, String>>,
}

#[derive(Debug, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ApiPatchInput {
    /// API endpoint URL
    pub url: String,
    /// Request body data (JSON string)
    pub data: String,
    /// Request headers
    #[serde(default)]
    pub headers: Option<HashMap<String, String>>,
}

#[derive(Debug, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ApiDeleteInput {
    /// API endpoint URL
    pub url: String,
    /// Request headers
    #[serde(default)]
    pub headers: Option<HashMap<String, String>>,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Serialize the tool name + params into a JSON command, invoke the embedded
/// agent-browser binary with `--native --mcp-tool`, read its JSON stdout, and
/// return the text result.
async fn invoke_tool(
    binary: &PathBuf,
    tool_name: &str,
    params: &impl Serialize,
) -> Result<CallToolResult, McpError> {
    let _payload = serde_json::json!({
        "tool": tool_name,
        "input": params,
    });

    let child = Command::new(binary)
        .arg("--native")
        .arg("--mcp-tool")
        .stdin(ProcessStdio::piped())
        .stdout(ProcessStdio::piped())
        .stderr(ProcessStdio::piped())
        .spawn()
        .map_err(|e| McpError::internal_error(format!("failed to spawn agent-browser: {e}"), None))?;

    let output = child
        .wait_with_output()
        .await
        .map_err(|e| McpError::internal_error(format!("agent-browser io error: {e}"), None))?;

    // If the binary doesn't support --mcp-tool yet, fall back to a
    // simple exec-with-args approach and return raw stdout.
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();

    if !output.status.success() {
        // Still return the output as tool content so the LLM can see the error.
        let msg = if stderr.is_empty() { &stdout } else { &stderr };
        return Ok(CallToolResult::error(vec![Content::text(format!(
            "agent-browser exited {}: {}",
            output.status,
            msg.trim()
        ))]));
    }

    // Try to unwrap a JSON envelope; if not JSON, return raw text.
    if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&stdout) {
        if let Some(text) = parsed.get("result").and_then(|v| v.as_str()) {
            return Ok(CallToolResult::success(vec![Content::text(text.to_string())]));
        }
        // Return pretty-printed JSON
        Ok(CallToolResult::success(vec![Content::text(
            serde_json::to_string_pretty(&parsed).unwrap_or(stdout),
        )]))
    } else {
        Ok(CallToolResult::success(vec![Content::text(stdout)]))
    }
}

// ---------------------------------------------------------------------------
// MCP Server
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct BrowserMcpServer {
    binary_path: PathBuf,
    tool_router: ToolRouter<Self>,
}

#[tool_router]
impl BrowserMcpServer {
    pub fn new(binary_path: PathBuf) -> Self {
        Self {
            binary_path,
            tool_router: Self::tool_router(),
        }
    }

    // --- Browser tools -----------------------------------------------------

    #[tool(
        name = "browser_set_viewport",
        description = "Change the browser's viewport size and scale factor"
    )]
    async fn browser_set_viewport(
        &self,
        Parameters(input): Parameters<BrowserSetViewportInput>,
    ) -> Result<CallToolResult, McpError> {
        invoke_tool(&self.binary_path, "browser_set_viewport", &input).await
    }

    #[tool(
        name = "browser_navigate",
        description = "Navigate to a specific URL"
    )]
    async fn browser_navigate(
        &self,
        Parameters(input): Parameters<BrowserNavigateInput>,
    ) -> Result<CallToolResult, McpError> {
        invoke_tool(&self.binary_path, "browser_navigate", &input).await
    }

    #[tool(
        name = "browser_screenshot",
        description = "Capture a screenshot of the current page or a specific element"
    )]
    async fn browser_screenshot(
        &self,
        Parameters(input): Parameters<BrowserScreenshotInput>,
    ) -> Result<CallToolResult, McpError> {
        invoke_tool(&self.binary_path, "browser_screenshot", &input).await
    }

    #[tool(
        name = "browser_click",
        description = "Click an element on the page"
    )]
    async fn browser_click(
        &self,
        Parameters(input): Parameters<BrowserClickInput>,
    ) -> Result<CallToolResult, McpError> {
        invoke_tool(&self.binary_path, "browser_click", &input).await
    }

    #[tool(
        name = "browser_fill",
        description = "Fill a form input with text"
    )]
    async fn browser_fill(
        &self,
        Parameters(input): Parameters<BrowserFillInput>,
    ) -> Result<CallToolResult, McpError> {
        invoke_tool(&self.binary_path, "browser_fill", &input).await
    }

    #[tool(
        name = "browser_select",
        description = "Select an option from a dropdown menu"
    )]
    async fn browser_select(
        &self,
        Parameters(input): Parameters<BrowserSelectInput>,
    ) -> Result<CallToolResult, McpError> {
        invoke_tool(&self.binary_path, "browser_select", &input).await
    }

    #[tool(
        name = "browser_hover",
        description = "Hover over an element on the page"
    )]
    async fn browser_hover(
        &self,
        Parameters(input): Parameters<BrowserHoverInput>,
    ) -> Result<CallToolResult, McpError> {
        invoke_tool(&self.binary_path, "browser_hover", &input).await
    }

    #[tool(
        name = "browser_evaluate",
        description = "Execute JavaScript in the browser context"
    )]
    async fn browser_evaluate(
        &self,
        Parameters(input): Parameters<BrowserEvaluateInput>,
    ) -> Result<CallToolResult, McpError> {
        invoke_tool(&self.binary_path, "browser_evaluate", &input).await
    }

    // --- API tools ---------------------------------------------------------

    #[tool(
        name = "api_get",
        description = "Perform a GET request to an API endpoint"
    )]
    async fn api_get(
        &self,
        Parameters(input): Parameters<ApiGetInput>,
    ) -> Result<CallToolResult, McpError> {
        invoke_tool(&self.binary_path, "api_get", &input).await
    }

    #[tool(
        name = "api_post",
        description = "Perform a POST request to an API endpoint"
    )]
    async fn api_post(
        &self,
        Parameters(input): Parameters<ApiPostInput>,
    ) -> Result<CallToolResult, McpError> {
        invoke_tool(&self.binary_path, "api_post", &input).await
    }

    #[tool(
        name = "api_put",
        description = "Perform a PUT request to an API endpoint"
    )]
    async fn api_put(
        &self,
        Parameters(input): Parameters<ApiPutInput>,
    ) -> Result<CallToolResult, McpError> {
        invoke_tool(&self.binary_path, "api_put", &input).await
    }

    #[tool(
        name = "api_patch",
        description = "Perform a PATCH request to an API endpoint"
    )]
    async fn api_patch(
        &self,
        Parameters(input): Parameters<ApiPatchInput>,
    ) -> Result<CallToolResult, McpError> {
        invoke_tool(&self.binary_path, "api_patch", &input).await
    }

    #[tool(
        name = "api_delete",
        description = "Perform a DELETE request to an API endpoint"
    )]
    async fn api_delete(
        &self,
        Parameters(input): Parameters<ApiDeleteInput>,
    ) -> Result<CallToolResult, McpError> {
        invoke_tool(&self.binary_path, "api_delete", &input).await
    }
}

#[tool_handler]
impl ServerHandler for BrowserMcpServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(
            ServerCapabilities::builder()
                .enable_tools()
                .build(),
        )
        .with_server_info(Implementation::new("agent-browser-socket", env!("CARGO_PKG_VERSION")))
        .with_instructions(
            "MCP server for browser automation and API requests. \
             Browser tools: browser_navigate, browser_screenshot, browser_click, \
             browser_fill, browser_select, browser_hover, browser_evaluate, \
             browser_set_viewport. \
             API tools: api_get, api_post, api_put, api_patch, api_delete."
                .to_string(),
        )
    }
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

/// Run the MCP stdio server.  Blocks until the client closes stdin.
pub async fn run_mcp_stdio() -> Result<i32, Box<dyn std::error::Error>> {
    let config = load_config()?;
    let binary_path = resolve_binary_path(config.browser_path.as_deref())?;
    let server = BrowserMcpServer::new(binary_path);

    let transport = stdio();
    let service = server.serve(transport).await
        .map_err(|e| -> Box<dyn std::error::Error> { Box::new(e) })?;
    service.waiting().await
        .map_err(|e| -> Box<dyn std::error::Error> { Box::new(e) })?;

    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::ffi::OsStr;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

    fn unique_temp_dir(prefix: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time")
            .as_nanos();
        let seq = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let pid = std::process::id();
        let dir = std::env::temp_dir().join(format!("{prefix}-{pid}-{nanos}-{seq}"));
        fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    fn create_mock_binary(stdout: &str, stderr: &str, exit_code: i32) -> PathBuf {
        let dir = unique_temp_dir("abs-mcp");

        #[cfg(windows)]
        {
            let path = dir.join("mock-browser.cmd");
            let script = format!(
                "@echo off\r\nif not \"{}\"==\"\" echo {}\r\nif not \"{}\"==\"\" 1>&2 echo {}\r\nexit /b {}\r\n",
                stdout,
                stdout,
                stderr,
                stderr,
                exit_code
            );
            fs::write(&path, script).expect("write cmd");
            path
        }

        #[cfg(not(windows))]
        {
            use std::os::unix::fs::PermissionsExt;

            let path = dir.join("mock-browser.sh");
            let escaped_stdout = stdout.replace('"', "\\\"");
            let escaped_stderr = stderr.replace('"', "\\\"");
            let script = format!(
                "#!/bin/sh\nif [ -n \"{escaped_stdout}\" ]; then\n  printf '%s\\n' \"{escaped_stdout}\"\nfi\nif [ -n \"{escaped_stderr}\" ]; then\n  printf '%s\\n' \"{escaped_stderr}\" 1>&2\nfi\nexit {exit_code}\n"
            );
            fs::write(&path, script).expect("write shell script");
            let mut perms = fs::metadata(&path).expect("metadata").permissions();
            perms.set_mode(0o755);
            fs::set_permissions(&path, perms).expect("chmod");
            path
        }
    }

    fn result_json(result: &CallToolResult) -> serde_json::Value {
        serde_json::to_value(result).expect("serialize call tool result")
    }

    fn first_text(result: &CallToolResult) -> String {
        let value = result_json(result);
        value
            .get("content")
            .and_then(|v| v.as_array())
            .and_then(|arr| arr.first())
            .and_then(|item| item.get("text"))
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string()
    }

    #[tokio::test]
    async fn invoke_tool_success_with_result_field() {
        let binary = create_mock_binary(r#"{"result":"ok"}"#, "", 0);
        let result = invoke_tool(&binary, "browser_navigate", &serde_json::json!({"url":"https://example.com"}))
            .await
            .expect("invoke tool");

        let value = result_json(&result);
        assert_eq!(value.get("isError").and_then(|v| v.as_bool()), Some(false));
        assert_eq!(first_text(&result), "ok");
    }

    #[tokio::test]
    async fn invoke_tool_success_with_structured_json_stdout() {
        let binary = create_mock_binary(r#"{"answer":42}"#, "", 0);
        let result = invoke_tool(&binary, "browser_evaluate", &serde_json::json!({"script":"1+1"}))
            .await
            .expect("invoke tool");

        let text = first_text(&result);
        assert!(text.contains("\"answer\": 42"));
    }

    #[tokio::test]
    async fn invoke_tool_success_with_plain_text_stdout() {
        let binary = create_mock_binary("plain-output", "", 0);
        let result = invoke_tool(&binary, "api_get", &serde_json::json!({"url":"https://example.com"}))
            .await
            .expect("invoke tool");

        assert_eq!(first_text(&result), "plain-output\n");
    }

    #[tokio::test]
    async fn invoke_tool_nonzero_exit_returns_error_result() {
        let binary = create_mock_binary("", "boom", 7);
        let result = invoke_tool(&binary, "api_post", &serde_json::json!({"url":"https://example.com","data":"{}"}))
            .await
            .expect("invoke tool");

        let value = result_json(&result);
        assert_eq!(value.get("isError").and_then(|v| v.as_bool()), Some(true));
        let text = first_text(&result);
        assert!(text.contains("agent-browser exited"));
        assert!(text.contains("boom"));
    }

    #[tokio::test]
    async fn invoke_tool_spawn_failure_maps_to_mcp_error() {
        let bad_path = PathBuf::from(OsStr::new("/definitely/not/a/binary"));
        let error = invoke_tool(&bad_path, "api_delete", &serde_json::json!({"url":"https://example.com"}))
            .await
            .expect_err("expected spawn failure");

        let text = format!("{error}");
        assert!(text.contains("failed to spawn agent-browser"));
    }

    #[test]
    fn server_info_advertises_tools_and_instructions() {
        let server = BrowserMcpServer::new(PathBuf::from("mock-binary"));
        let info = server.get_info();
        let rendered = serde_json::to_string(&info).expect("serialize server info");

        assert!(rendered.contains("tools"));
        assert!(rendered.contains("browser_navigate"));
        assert!(rendered.contains("api_delete"));
    }
}
