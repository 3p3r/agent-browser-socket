use config::{Config, Environment, File, FileFormat};
use dirs::home_dir;
use serde::Deserialize;
use secure_string::SecureString;
use std::env;

#[derive(Debug, Clone, Deserialize)]
pub struct AppConfig {
    pub auth_url: Option<String>,
    pub port: u16,
    pub host: String,
    pub browser_path: Option<String>,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            auth_url: None,
            port: 9607,
            host: "0.0.0.0".to_string(),
            browser_path: None,
        }
    }
}

pub fn load_config() -> Result<AppConfig, config::ConfigError> {
    let defaults = AppConfig::default();
    let home_abs = home_dir().map(|home| home.join(".abs"));
    let local_abs = std::path::Path::new(".abs").to_path_buf();
    let has_home_abs = home_abs.as_ref().map(|path| path.exists()).unwrap_or(false);
    let has_local_abs = local_abs.exists();
    let has_abs_env = env::vars().any(|(key, _)| key.starts_with("ABS_"));

    let mut builder = Config::builder()
        .set_default("auth_url", defaults.auth_url)?
        .set_default("port", defaults.port)?
        .set_default("host", defaults.host)?
        .set_default("browser_path", defaults.browser_path)?;

    if !has_home_abs && !has_local_abs && !has_abs_env {
        let embedded_default = embedded_secure_default_config();
        builder = builder.add_source(File::from_str(embedded_default.unsecure(), FileFormat::Toml));
    }

    if let Some(home_abs) = home_abs {
        builder =
            builder.add_source(File::new(home_abs.to_string_lossy().as_ref(), FileFormat::Toml).required(false));
    }

    builder = builder
        .add_source(File::new(".abs", FileFormat::Toml).required(false))
        .add_source(Environment::with_prefix("ABS").separator("__"));

    let settings = builder.build()?;
    settings.try_deserialize()
}

fn embedded_secure_default_config() -> SecureString {
    SecureString::from(
        r#"port = 9607
host = "0.0.0.0"
"#,
    )
}
