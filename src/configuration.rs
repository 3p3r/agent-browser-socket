use config::{Config, Environment, File, FileFormat};
use dirs::home_dir;
use secure_string::SecureString;
use serde::Deserialize;
use std::env;

#[derive(Debug, Clone, Deserialize)]
pub struct PageAgentConfig {
    pub model: String,
    pub url: String,
    pub key: String,
}

impl Default for PageAgentConfig {
    fn default() -> Self {
        Self {
            model: "qwen3.5-plus".to_string(),
            url: "http://localhost:11434/v1".to_string(),
            key: "NA".to_string(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct AppConfig {
    pub port: u16,
    pub host: String,
    pub browser_path: Option<String>,
    pub page_agent: PageAgentConfig,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            port: 9607,
            host: "0.0.0.0".to_string(),
            browser_path: None,
            page_agent: PageAgentConfig::default(),
        }
    }
}

pub fn load_config() -> Result<AppConfig, config::ConfigError> {
    let defaults = AppConfig::default();
    let home_oatmeal = home_dir().map(|home| home.join(".oatmeal"));
    let local_oatmeal = std::path::Path::new(".oatmeal").to_path_buf();
    let has_home_oatmeal = home_oatmeal
        .as_ref()
        .map(|path| path.exists())
        .unwrap_or(false);
    let has_local_oatmeal = local_oatmeal.exists();
    let has_oatmeal_env = env::vars().any(|(key, _)| key.starts_with("OATMEAL_"));

    let mut builder = Config::builder()
        .set_default("port", defaults.port)?
        .set_default("host", defaults.host)?
        .set_default("browser_path", defaults.browser_path)?
        .set_default("page_agent.model", defaults.page_agent.model)?
        .set_default("page_agent.url", defaults.page_agent.url)?
        .set_default("page_agent.key", defaults.page_agent.key)?;

    if !has_home_oatmeal && !has_local_oatmeal && !has_oatmeal_env {
        let embedded_default = embedded_secure_default_config();
        builder = builder.add_source(File::from_str(
            embedded_default.unsecure(),
            FileFormat::Toml,
        ));
    }

    if let Some(home_oatmeal) = home_oatmeal {
        builder = builder.add_source(
            File::new(home_oatmeal.to_string_lossy().as_ref(), FileFormat::Toml).required(false),
        );
    }

    builder = builder
        .add_source(File::new(".oatmeal", FileFormat::Toml).required(false))
        .add_source(Environment::with_prefix("OATMEAL").separator("__"));

    let mut config: AppConfig = builder.build()?.try_deserialize()?;
    if let Ok(v) = env::var("OATMEAL_PAGE_AGENT__MODEL") {
        if !v.is_empty() {
            config.page_agent.model = v;
        }
    }
    if let Ok(v) = env::var("OATMEAL_PAGE_AGENT__URL") {
        if !v.is_empty() {
            config.page_agent.url = v;
        }
    }
    if let Ok(v) = env::var("OATMEAL_PAGE_AGENT__KEY") {
        if !v.is_empty() {
            config.page_agent.key = v;
        }
    }
    if config.browser_path.is_none() {
        config.browser_path = crate::browser_detection::find_chrome_browser()
            .map(|path| path.to_string_lossy().to_string());
    }
    Ok(config)
}

fn embedded_secure_default_config() -> SecureString {
    SecureString::from(
        r#"port = 9607
host = "0.0.0.0"

[page_agent]
model = "qwen3.5-plus"
url = "http://localhost:11434/v1"
key = "NA"
"#,
    )
}
