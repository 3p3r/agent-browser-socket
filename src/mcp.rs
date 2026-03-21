use crate::command_args::{
    build_args, ensure_executable_path_arg, has_passthrough_command, translate_agentic_open,
    translate_agentic_prompt, ExecutablePathPrefill,
};
use crate::configuration::{AppConfig, PageAgentConfig};
use crate::embedded_binary::resolve_binary_path;
use crate::screenshot::capture_all_screenshots;
use axum::response::Html;
use axum::routing::get;
use axum::Router;

use base64::{engine::general_purpose::STANDARD, Engine as _};
use rmcp::{
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::*,
    schemars,
    service::RequestContext,
    tool, tool_handler, tool_router,
    transport::{
        stdio,
        streamable_http_server::{
            session::local::LocalSessionManager, StreamableHttpServerConfig, StreamableHttpService,
        },
    },
    ErrorData as McpError, RoleServer, ServerHandler, ServiceExt,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::process::Stdio as ProcessStdio;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::fs;
use tokio::process::Command;
use tokio::sync::RwLock;
use tower_http::cors::{Any, CorsLayer, ExposeHeaders};
use uuid::Uuid;

const ADMIN_DASHBOARD_HTML: &str = include_str!("admin_dashboard.html");

type ResourceStore = Arc<RwLock<HashMap<String, ResourceEntry>>>;

#[derive(Clone)]
struct ResourceEntry {
    name: String,
    mime_type: String,
    data: Vec<u8>,
    created_at_unix_ms: u128,
}

fn current_unix_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0)
}

fn extension_to_mime(extension: &str) -> &'static str {
    match extension.to_ascii_lowercase().as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "pdf" => "application/pdf",
        _ => "application/octet-stream",
    }
}

fn sanitize_path_token(token: &str) -> &str {
    token.trim_matches(|ch| {
        ch == '"' || ch == '\'' || ch == '(' || ch == ')' || ch == '[' || ch == ']' || ch == ','
    })
}

fn parse_option_value(args: &[String], key: &str) -> Option<String> {
    for (index, arg) in args.iter().enumerate() {
        if arg == key {
            if let Some(value) = args.get(index + 1) {
                return Some(value.clone());
            }
        }
        if let Some(value) = arg.strip_prefix(&(key.to_string() + "=")) {
            return Some(value.to_string());
        }
    }
    None
}

fn first_positional_index(args: &[String]) -> Option<usize> {
    args.iter().position(|arg| !arg.starts_with('-'))
}

fn command_path_from_args(args: &[String], command_index: usize) -> Option<PathBuf> {
    let mut index = command_index + 1;
    while index < args.len() {
        let arg = &args[index];
        if arg == "--screenshot-dir"
            || arg == "--screenshot-format"
            || arg == "--screenshot-quality"
            || arg == "--executable-path"
        {
            index += 2;
            continue;
        }

        if arg.starts_with("--screenshot-dir=")
            || arg.starts_with("--screenshot-format=")
            || arg.starts_with("--screenshot-quality=")
            || arg.starts_with("--executable-path=")
        {
            index += 1;
            continue;
        }

        if arg.starts_with('-') {
            index += 1;
            continue;
        }

        return Some(PathBuf::from(arg));
    }
    None
}

fn find_existing_path_in_stdout(stdout: &str, extensions: &[&str]) -> Option<PathBuf> {
    let allowed: Vec<String> = extensions
        .iter()
        .map(|ext| ext.to_ascii_lowercase())
        .collect();
    for raw_token in stdout.split_whitespace() {
        let token = sanitize_path_token(raw_token);
        if token.is_empty() {
            continue;
        }
        let path = PathBuf::from(token);
        if !path.is_file() {
            continue;
        }
        let Some(ext) = path.extension().and_then(|value| value.to_str()) else {
            continue;
        };
        if allowed
            .iter()
            .any(|candidate| candidate == &ext.to_ascii_lowercase())
        {
            return Some(path);
        }
    }
    None
}

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

#[derive(Debug, Serialize, Deserialize, schemars::JsonSchema)]
pub struct DeleteResourceInput {
    pub uri: String,
}

#[derive(Debug, Serialize, Deserialize, schemars::JsonSchema)]
pub struct DeleteAllResourcesInput {}

#[derive(Clone)]
pub struct SystemMcpServer {
    binary_path: PathBuf,
    detected_browser_path: Option<PathBuf>,
    page_agent_config: PageAgentConfig,
    resource_store: ResourceStore,
    tool_router: ToolRouter<Self>,
}

#[tool_router]
impl SystemMcpServer {
    async fn create_resource_with_size(
        &self,
        kind: &str,
        name: String,
        mime_type: String,
        data: Vec<u8>,
    ) -> RawResource {
        let size = data.len();
        let uri = format!("resource://{kind}/{}", Uuid::new_v4());
        let entry = ResourceEntry {
            name: name.clone(),
            mime_type: mime_type.clone(),
            data,
            created_at_unix_ms: current_unix_ms(),
        };

        self.resource_store.write().await.insert(uri.clone(), entry);

        RawResource::new(uri, name)
            .with_mime_type(mime_type)
            .with_size(size.min(u32::MAX as usize) as u32)
    }

    async fn maybe_capture_command_output_resource(
        &self,
        arguments: &[String],
        stdout: &str,
    ) -> Option<RawResource> {
        let command_index = first_positional_index(arguments)?;
        let command_name = arguments.get(command_index)?.as_str();

        if command_name == "screenshot" {
            let requested_format = parse_option_value(arguments, "--screenshot-format")
                .unwrap_or_else(|| "png".to_string());
            let explicit_path = command_path_from_args(arguments, command_index);
            let mut candidate_path = explicit_path.filter(|path| path.is_file());

            if candidate_path.is_none() {
                candidate_path =
                    find_existing_path_in_stdout(stdout, &["png", "jpg", "jpeg", "webp"]);
            }

            let path = candidate_path?;
            let data = fs::read(&path).await.ok()?;
            let extension = path
                .extension()
                .and_then(|value| value.to_str())
                .unwrap_or(&requested_format);
            let mime = extension_to_mime(extension).to_string();
            let name = path
                .file_name()
                .and_then(|value| value.to_str())
                .unwrap_or("screenshot")
                .to_string();

            return Some(
                self.create_resource_with_size("screenshot", name, mime, data)
                    .await,
            );
        }

        if command_name == "pdf" {
            let explicit_path = command_path_from_args(arguments, command_index);
            let mut candidate_path = explicit_path.filter(|path| path.is_file());

            if candidate_path.is_none() {
                candidate_path = find_existing_path_in_stdout(stdout, &["pdf"]);
            }

            let path = candidate_path?;
            let data = fs::read(&path).await.ok()?;
            let name = path
                .file_name()
                .and_then(|value| value.to_str())
                .unwrap_or("document.pdf")
                .to_string();

            return Some(
                self.create_resource_with_size("pdf", name, "application/pdf".to_string(), data)
                    .await,
            );
        }

        None
    }

    fn new(
        binary_path: PathBuf,
        detected_browser_path: Option<PathBuf>,
        page_agent_config: PageAgentConfig,
        resource_store: ResourceStore,
    ) -> Self {
        Self {
            binary_path,
            detected_browser_path,
            page_agent_config,
            resource_store,
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
        let screenshots = match std::panic::catch_unwind(capture_all_screenshots) {
            Ok(Ok(screenshots)) => screenshots,
            Ok(Err(error)) => {
                return Ok(CallToolResult::error(vec![Content::text(format!(
                    "screenshot failed: {}",
                    error
                ))]))
            }
            Err(_) => {
                return Ok(CallToolResult::error(vec![Content::text(
                    "screenshot failed: panic in capture backend".to_string(),
                )]))
            }
        };

        let mut contents = Vec::new();
        for (index, screenshot) in screenshots.iter().enumerate() {
            let data = STANDARD.decode(&screenshot.png_base64).map_err(|error| {
                McpError::internal_error(
                    format!("failed to decode screenshot payload: {error}"),
                    None,
                )
            })?;
            let name = format!("system-monitor-{index}.png");
            let resource = self
                .create_resource_with_size("screenshot", name, "image/png".to_string(), data)
                .await;
            contents.push(Content::resource_link(resource));
        }

        Ok(CallToolResult::success(contents))
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
            Some("executable path auto-detection unavailable; run `agent-browser-server --command install` to install a browser through this binary")
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

        let mut content = vec![Content::text(
            serde_json::to_string_pretty(&result).unwrap_or_default(),
        )];

        if exit_code == 0 {
            if let Some(resource) = self
                .maybe_capture_command_output_resource(&arguments, &stdout)
                .await
            {
                content.push(Content::resource_link(resource));
            }
        }

        Ok(CallToolResult::success(content))
    }

    #[tool(
        name = "delete_resource",
        description = "Delete a generated MCP resource by URI"
    )]
    async fn delete_resource(
        &self,
        Parameters(input): Parameters<DeleteResourceInput>,
    ) -> Result<CallToolResult, McpError> {
        let removed = self
            .resource_store
            .write()
            .await
            .remove(&input.uri)
            .is_some();
        let payload = serde_json::json!({
            "uri": input.uri,
            "deleted": removed
        });

        Ok(CallToolResult::success(vec![Content::text(
            serde_json::to_string_pretty(&payload).unwrap_or_default(),
        )]))
    }

    #[tool(
        name = "delete_all_resources",
        description = "Delete all generated MCP resources"
    )]
    async fn delete_all_resources(
        &self,
        Parameters(_input): Parameters<DeleteAllResourcesInput>,
    ) -> Result<CallToolResult, McpError> {
        let mut store = self.resource_store.write().await;
        let deleted = store.len();
        store.clear();

        let payload = serde_json::json!({ "deleted": deleted });
        Ok(CallToolResult::success(vec![Content::text(
            serde_json::to_string_pretty(&payload).unwrap_or_default(),
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
        ServerInfo::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_resources()
                .enable_resources_list_changed()
                .build(),
        )
            .with_server_info(Implementation::new(
                "agent-browser-server",
                env!("CARGO_PKG_VERSION"),
            ))
            .with_instructions(
                "MCP server for browser automation and system tooling. \
                 Tools: health, version, shutdown, screenshot_system, command, delete_resource, delete_all_resources."
                    .to_string(),
            )
    }

    fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<ListResourcesResult, McpError>> + Send + '_ {
        async move {
            let store = self.resource_store.read().await;
            let resources = store
                .iter()
                .map(|(uri, entry)| {
                    Resource::new(
                        RawResource::new(uri.clone(), entry.name.clone())
                            .with_mime_type(entry.mime_type.clone())
                            .with_size(entry.data.len().min(u32::MAX as usize) as u32)
                            .with_description(format!(
                                "generated-at-ms:{}",
                                entry.created_at_unix_ms
                            )),
                        None,
                    )
                })
                .collect();

            Ok(ListResourcesResult::with_all_items(resources))
        }
    }

    fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<ReadResourceResult, McpError>> + Send + '_ {
        async move {
            let store = self.resource_store.read().await;
            let Some(entry) = store.get(&request.uri) else {
                return Err(McpError::resource_not_found(
                    "Resource not found",
                    Some(serde_json::json!({ "uri": request.uri })),
                ));
            };

            let blob = STANDARD.encode(&entry.data);
            let content =
                ResourceContents::blob(blob, request.uri).with_mime_type(entry.mime_type.clone());
            Ok(ReadResourceResult::new(vec![content]))
        }
    }
}

pub async fn run_mcp_sse<F>(
    config: AppConfig,
    page_agent_config: PageAgentConfig,
    shutdown: F,
) -> Result<i32, Box<dyn std::error::Error>>
where
    F: Future<Output = ()> + Send + 'static,
{
    let binary_path = resolve_binary_path(config.browser_path.as_deref())?;
    let detected_browser_path = crate::browser_detection::find_chrome_browser();
    let resource_store: ResourceStore = Arc::new(RwLock::new(HashMap::new()));
    let sse_service: StreamableHttpService<SystemMcpServer, LocalSessionManager> =
        StreamableHttpService::new(
            move || {
                Ok(SystemMcpServer::new(
                    binary_path.clone(),
                    detected_browser_path.clone(),
                    page_agent_config.clone(),
                    resource_store.clone(),
                ))
            },
            Default::default(),
            StreamableHttpServerConfig::default(),
        );

    let app = Router::new()
        .route("/", get(dashboard_handler))
        .nest_service("/mcp", sse_service)
        .layer(
            CorsLayer::new()
                .allow_origin(Any)
                .allow_methods(Any)
                .allow_headers(Any)
                .expose_headers(ExposeHeaders::list([axum::http::HeaderName::from_static(
                    "mcp-session-id",
                )])),
        );

    let listener =
        tokio::net::TcpListener::bind(format!("{}:{}", config.host, config.port)).await?;
    axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            shutdown.await;
            tokio::spawn(async {
                tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;
                std::process::exit(0);
            });
        })
        .await?;

    Ok(0)
}

async fn dashboard_handler() -> Html<&'static str> {
    Html(ADMIN_DASHBOARD_HTML)
}

pub async fn run_mcp_stdio(
    config: AppConfig,
    page_agent_config: PageAgentConfig,
) -> Result<i32, Box<dyn std::error::Error>> {
    let binary_path = resolve_binary_path(config.browser_path.as_deref())?;
    let detected_browser_path = crate::browser_detection::find_chrome_browser();
    let resource_store: ResourceStore = Arc::new(RwLock::new(HashMap::new()));
    let server = SystemMcpServer::new(
        binary_path,
        detected_browser_path,
        page_agent_config,
        resource_store,
    );

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
