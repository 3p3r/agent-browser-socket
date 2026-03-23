use log::LevelFilter;
use log4rs::append::rolling_file::policy::compound::{
    roll::fixed_window::FixedWindowRoller,
    trigger::size::SizeTrigger,
    CompoundPolicy,
};
use log4rs::append::rolling_file::RollingFileAppender;
use log4rs::config::{Appender, Config, Root};
use log4rs::encode::pattern::PatternEncoder;

pub fn oatmeal_logs_dir() -> std::path::PathBuf {
    crate::runtime_shared::oatmeal_cache_dir().join("logs")
}

pub fn build_file_logging_config(
    log_path: &std::path::Path,
    archive_pattern: &str,
    max_size_bytes: u64,
    archive_count: u32,
) -> Result<Config, Box<dyn std::error::Error>> {
    let trigger = SizeTrigger::new(max_size_bytes);
    let roller = FixedWindowRoller::builder()
        .base(1)
        .build(&archive_pattern, archive_count)?;
    let policy = CompoundPolicy::new(Box::new(trigger), Box::new(roller));

    let logfile = RollingFileAppender::builder()
        .append(true)
        .encoder(Box::new(PatternEncoder::new(
            "{d(%Y-%m-%dT%H:%M:%S%.3f%:z)} {l:<5} {t} - {m}{n}",
        )))
        .build(log_path, Box::new(policy))?;

    let config = Config::builder()
        .appender(Appender::builder().build("logfile", Box::new(logfile)))
        .build(Root::builder().appender("logfile").build(LevelFilter::Info))?;

    Ok(config)
}

pub fn init_file_logging() -> Result<log4rs::Handle, Box<dyn std::error::Error>> {
    let log_dir = oatmeal_logs_dir();
    std::fs::create_dir_all(&log_dir)?;

    let log_path = log_dir.join("oatmeal.log");
    let archive_pattern = log_dir.join("oatmeal.{}.log.gz").to_string_lossy().into_owned();

    let config = build_file_logging_config(&log_path, &archive_pattern, 5 * 1024 * 1024, 3)?;
    let handle = log4rs::init_config(config)?;
    Ok(handle)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oatmeal_logs_dir_ends_with_expected_suffix() {
        assert!(oatmeal_logs_dir().to_string_lossy().ends_with("oatmeal/logs"));
    }

    #[test]
    fn build_file_logging_config_succeeds_for_temp_paths() {
        let temp_root = std::env::temp_dir().join(format!(
            "oatmeal-log-config-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system clock before unix epoch")
                .as_nanos()
        ));
        std::fs::create_dir_all(&temp_root).expect("failed to create temp root");

        let log_path = temp_root.join("oatmeal.log");
        let archive_pattern = temp_root.join("oatmeal.{}.log.gz").to_string_lossy().into_owned();

        let config = build_file_logging_config(&log_path, &archive_pattern, 5 * 1024 * 1024, 3);
        assert!(config.is_ok());
        assert!(archive_pattern.ends_with("oatmeal.{}.log.gz"));

        let _ = std::fs::remove_dir_all(&temp_root);
    }
}