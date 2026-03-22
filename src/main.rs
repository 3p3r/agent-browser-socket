mod app;
mod bashkit_executor;
mod browser_detection;
mod command_args;
mod command_runtime;
mod configuration;
mod embedded_binary;
mod mcp;
mod page_agent_runtime;
mod runtime_shared;
mod sandbox_files;
mod screenshot;
mod server;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = std::env::args_os().skip(1).collect();
    let code = app::run_with_args(args).await?;
    std::process::exit(code)
}
