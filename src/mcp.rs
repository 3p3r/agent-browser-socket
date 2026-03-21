use crate::bashkit_executor::BashkitExecutor;
use crate::command_args::prepare_command;
use crate::configuration::{AppConfig, PageAgentConfig};
use crate::embedded_binary::resolve_binary_path;
use crate::page_agent_runtime::{run_page_agent_injection, run_page_agent_prompt};
use crate::sandbox_files::prepare_sandbox_files;
use crate::screenshot::capture_all_screenshots;
use axum::extract::Path as AxumPath;
use axum::http::{header, StatusCode};
use axum::response::{Html, IntoResponse, Response};
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
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::RwLock;
use tower_http::cors::{Any, CorsLayer, ExposeHeaders};
use uuid::Uuid;

const ADMIN_DASHBOARD_HTML: &str = include_str!("admin_dashboard.html");
include!(concat!(env!("OUT_DIR"), "/skills_manifest.rs"));

type ResourceStore = Arc<RwLock<HashMap<String, ResourceEntry>>>;

#[derive(Clone)]
struct ResourceEntry {
    name: String,
    source_path: String,
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

#[derive(Debug, Serialize, Deserialize, schemars::JsonSchema)]
pub struct SystemScreenshotInput {}

#[derive(Debug, Serialize, Deserialize, schemars::JsonSchema)]
pub struct CommandInput {
    /// Bash script to execute with agent-browser available as a function
    pub command: String,
    /// Optional environment variables
    #[serde(default)]
    pub env: Option<HashMap<String, String>>,
    /// Optional gitignore-style file used when filtering detected files before creating resources
    #[serde(default)]
    pub sandbox_ignore: Option<String>,
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
    page_agent_config: PageAgentConfig,
    resource_store: ResourceStore,
    executor: BashkitExecutor,
    tool_router: ToolRouter<Self>,
}

#[tool_router]
impl SystemMcpServer {
    async fn create_resource_with_size(
        &self,
        kind: &str,
        name: String,
        source_path: String,
        mime_type: String,
        data: Vec<u8>,
    ) -> RawResource {
        let size = data.len();
        let uri = format!("resource://{kind}/{}", Uuid::new_v4());
        let entry = ResourceEntry {
            name: name.clone(),
            source_path,
            mime_type: mime_type.clone(),
            data,
            created_at_unix_ms: current_unix_ms(),
        };

        self.resource_store.write().await.insert(uri.clone(), entry);

        RawResource::new(uri, name)
            .with_mime_type(mime_type)
            .with_size(size.min(u32::MAX as usize) as u32)
    }

    fn new(
        binary_path: PathBuf,
        page_agent_config: PageAgentConfig,
        resource_store: ResourceStore,
    ) -> Self {
        let executor = BashkitExecutor::new(binary_path.clone());
        Self {
            binary_path,
            page_agent_config,
            resource_store,
            executor,
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
                .create_resource_with_size(
                    "screenshot",
                    name,
                    format!("system-monitor-{index}.png"),
                    "image/png".to_string(),
                    data,
                )
                .await;
            contents.push(Content::resource_link(resource));
        }

        Ok(CallToolResult::success(contents))
    }

    #[tool(
        name = "command",
        description = "Execute a bash script with agent-browser available"
    )]
    async fn command(
        &self,
        Parameters(input): Parameters<CommandInput>,
    ) -> Result<CallToolResult, McpError> {
        let prepared =
            prepare_command(&input.command).map_err(|msg| McpError::invalid_params(msg, None))?;

        let execution = self.executor.execute(&prepared, input.env.as_ref()).await;
        let stdout = execution.stdout;
        let stderr = execution.stderr;
        let exit_code = execution.exit_code;

        let page_agent_injection = if prepared.should_inject_page_agent && exit_code == 0 {
            match run_page_agent_injection(
                &self.binary_path,
                &self.page_agent_config,
                input.env.as_ref(),
            )
            .await
            {
                Ok(injection_exit) => {
                    let prompt_output = if injection_exit == 0 {
                        if let Some(prompt) = prepared.agentic_prompt.as_ref() {
                            match run_page_agent_prompt(
                                &self.binary_path,
                                prompt,
                                input.env.as_ref(),
                            )
                            .await
                            {
                                Ok(prompt_result) => Some(serde_json::json!({
                                    "stdout": prompt_result.stdout,
                                    "stderr": prompt_result.stderr,
                                    "exit_code": prompt_result.exit_code
                                })),
                                Err(error) => Some(serde_json::json!({
                                    "stdout": "",
                                    "stderr": error,
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

        let filtered_files = prepare_sandbox_files(
            Path::new("."),
            input.sandbox_ignore.as_deref().map(Path::new),
            &execution.files,
        )
        .map_err(|error| {
            McpError::invalid_params(error.to_string(), Option::<serde_json::Value>::None)
        })?;

        let result = serde_json::json!({
            "stdout": stdout,
            "stderr": stderr,
            "exit_code": exit_code,
            "page_agent_injection": page_agent_injection,
            "resources_created": filtered_files.len()
        });
        let mut contents = vec![Content::text(
            serde_json::to_string_pretty(&result).unwrap_or_default(),
        )];

        for prepared_file in filtered_files {
            let resource_name = prepared_file
                .relative_path
                .to_string_lossy()
                .replace('\\', "/");
            let resource = self
                .create_resource_with_size(
                    "file",
                    resource_name,
                    prepared_file.file.path.display().to_string(),
                    prepared_file.file.mime_type.clone(),
                    prepared_file.file.data.clone(),
                )
                .await;
            contents.push(Content::resource_link(resource));
        }

        Ok(CallToolResult::success(contents))
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
                                "source-path:{} generated-at-ms:{}",
                                entry.source_path, entry.created_at_unix_ms
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

pub async fn run_mcp_streamable_http<F>(
    config: AppConfig,
    page_agent_config: PageAgentConfig,
    shutdown: F,
) -> Result<i32, Box<dyn std::error::Error>>
where
    F: Future<Output = ()> + Send + 'static,
{
    let binary_path = resolve_binary_path(config.browser_path.as_deref())?;
    let resource_store: ResourceStore = Arc::new(RwLock::new(HashMap::new()));
    let streamable_http_service: StreamableHttpService<SystemMcpServer, LocalSessionManager> =
        StreamableHttpService::new(
            move || {
                Ok(SystemMcpServer::new(
                    binary_path.clone(),
                    page_agent_config.clone(),
                    resource_store.clone(),
                ))
            },
            Default::default(),
            StreamableHttpServerConfig::default(),
        );

    let app = Router::new()
        .route("/", get(dashboard_handler))
        .route("/skills", get(skills_index_handler))
        .route("/skills/", get(skills_index_handler))
        .route("/skills/{*path}", get(skills_file_handler))
        .nest_service("/mcp", streamable_http_service)
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

async fn skills_index_handler() -> Html<String> {
    Html(render_skills_index_html())
}

fn skills_asset(path: &str) -> Option<(&'static str, &'static str)> {
    SKILLS_ASSETS
        .iter()
        .find(|(candidate, _, _)| *candidate == path)
        .map(|(_, mime, body)| (*mime, *body))
}

fn render_skills_index_html() -> String {
    let mut items = String::new();
    for (path, _, _) in SKILLS_ASSETS {
        items.push_str("<li><a href=\"/skills/");
        items.push_str(path);
        items.push_str("\">");
        items.push_str(path);
        items.push_str("</a></li>");
    }

    format!(
        "<!doctype html><html lang=\"en\"><head><meta charset=\"UTF-8\" /><meta name=\"viewport\" content=\"width=device-width, initial-scale=1.0\" /><title>agent-browser-server skills</title><style>body{{font-family:sans-serif;margin:24px;max-width:760px;}}h1{{margin-bottom:8px;}}ul{{line-height:1.6;}}</style></head><body><h1>Skills</h1><ul>{items}</ul></body></html>"
    )
}

async fn skills_file_handler(AxumPath(path): AxumPath<String>) -> Response {
    match skills_asset(path.trim_start_matches('/')) {
        Some((mime, body)) => ([(header::CONTENT_TYPE, mime)], body).into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn command_preserves_paths_and_applies_ignore_filter() {
        let binary_path = resolve_binary_path(None).expect("resolve binary path");
        let resource_store: ResourceStore = Arc::new(RwLock::new(HashMap::new()));
        let server = SystemMcpServer::new(
            binary_path,
            PageAgentConfig::default(),
            resource_store.clone(),
        );

        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time")
            .as_nanos();
        let output_dir = std::env::temp_dir().join(format!("abs-mcp-command-ignore-{unique}"));
        std::fs::create_dir_all(&output_dir).expect("create output dir");

        let keep_path = std::env::temp_dir().join(format!("abs-mcp-keep-{unique}.txt"));
        let noisy_path = std::env::temp_dir().join(format!("abs-mcp-trace-{unique}.log"));
        let ignore_path = output_dir.join("sandbox.ignore");
        std::fs::write(&ignore_path, "*.log\n").expect("write ignore file");

        let command = format!(
            "echo keep > {} && echo noisy > {}",
            keep_path.display(),
            noisy_path.display()
        );

        let result = server
            .command(Parameters(CommandInput {
                command,
                env: None,
                sandbox_ignore: Some(ignore_path.display().to_string()),
            }))
            .await
            .expect("command tool should succeed");

        assert_eq!(result.content.len(), 2);

        let serialized = serde_json::to_string(&result).expect("serialize tool result");
        assert!(
            serialized.contains("resources_created"),
            "result={serialized}"
        );
        assert!(serialized.contains("abs-mcp-keep-"), "result={serialized}");
        assert!(!serialized.contains("trace.log"), "result={serialized}");

        let store = resource_store.read().await;
        assert_eq!(store.len(), 1);

        let (uri, entry) = store.iter().next().expect("resource entry");
        assert!(uri.starts_with("resource://file/"), "uri={uri}");
        assert_eq!(entry.name, format!("tmp/abs-mcp-keep-{unique}.txt"));
        assert_eq!(entry.source_path, keep_path.display().to_string());
        assert_eq!(entry.mime_type, "text/plain");

        drop(store);
        let _ = std::fs::remove_file(&keep_path);
        let _ = std::fs::remove_file(&noisy_path);
        let _ = std::fs::remove_dir_all(output_dir);
    }

    #[tokio::test]
    async fn command_supports_basic_shell_commands() {
        let binary_path = resolve_binary_path(None).expect("resolve binary path");
        let resource_store: ResourceStore = Arc::new(RwLock::new(HashMap::new()));
        let server = SystemMcpServer::new(
            binary_path,
            PageAgentConfig::default(),
            resource_store.clone(),
        );

        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time")
            .as_nanos();
        let shell_path = std::env::temp_dir().join(format!("abs-mcp-shell-{unique}.txt"));
        let command = format!(
            "name=world && echo hello-$name | cat && echo saved-$name > {} && cat {}",
            shell_path.display(),
            shell_path.display()
        );

        let result = server
            .command(Parameters(CommandInput {
                command,
                env: None,
                sandbox_ignore: None,
            }))
            .await
            .expect("command tool should succeed");

        assert_eq!(result.content.len(), 2);
        let value = serde_json::to_value(&result).expect("serialize tool result value");
        let text = value
            .pointer("/content/0/text")
            .and_then(|value| value.as_str())
            .expect("first content text");
        assert!(
            text.contains("\"stdout\": \"hello-world\\nsaved-world\\n\""),
            "text={text}"
        );

        let store = resource_store.read().await;
        assert_eq!(store.len(), 1);

        let (_, entry) = store.iter().next().expect("resource entry");
        assert_eq!(entry.name, format!("tmp/abs-mcp-shell-{unique}.txt"));
        assert_eq!(entry.source_path, shell_path.display().to_string());

        drop(store);
        let _ = std::fs::remove_file(&shell_path);
    }
}

pub async fn run_mcp_stdio(
    config: AppConfig,
    page_agent_config: PageAgentConfig,
) -> Result<i32, Box<dyn std::error::Error>> {
    let binary_path = resolve_binary_path(config.browser_path.as_deref())?;
    let resource_store: ResourceStore = Arc::new(RwLock::new(HashMap::new()));
    let server = SystemMcpServer::new(binary_path, page_agent_config, resource_store);

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
