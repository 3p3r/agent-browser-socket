mod app;
mod auth;
mod configuration;
mod embedded_binary;
mod server;
mod screenshot;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = std::env::args_os().skip(1).collect();
    let code = app::run_with_args(args).await?;
    std::process::exit(code)
}
