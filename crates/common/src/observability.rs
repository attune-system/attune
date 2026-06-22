use tracing::subscriber::set_global_default;
use tracing_subscriber::EnvFilter;

use crate::{
    config::{Config, LogConfig},
    Error, Result,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogLevelSource {
    Cli,
    RustLogEnv,
    Config,
    Default,
}

impl LogLevelSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Cli => "cli",
            Self::RustLogEnv => "RUST_LOG",
            Self::Config => "config.log.level",
            Self::Default => "default",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogFormat {
    Json,
    Pretty,
}

impl LogFormat {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Json => "json",
            Self::Pretty => "pretty",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedTracingConfig {
    pub level_directive: String,
    pub level_source: LogLevelSource,
    pub format: LogFormat,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TracingInitResult {
    pub resolved: ResolvedTracingConfig,
    pub initialized: bool,
}

pub fn init_tracing_from_config(
    config: &Config,
    cli_log_level: Option<&str>,
) -> Result<TracingInitResult> {
    init_tracing(cli_log_level, Some(&config.log))
}

pub fn init_tracing(
    cli_log_level: Option<&str>,
    config_log: Option<&LogConfig>,
) -> Result<TracingInitResult> {
    let rust_log = std::env::var("RUST_LOG").ok();
    let resolved = resolve_tracing_config(cli_log_level, rust_log.as_deref(), config_log)?;

    let env_filter = EnvFilter::try_new(resolved.level_directive.clone()).map_err(|error| {
        Error::configuration(format!(
            "invalid log directive from {}: {}",
            resolved.level_source.as_str(),
            error
        ))
    })?;

    let initialized = match resolved.format {
        LogFormat::Json => {
            let subscriber = tracing_subscriber::fmt()
                .with_env_filter(env_filter)
                .with_target(false)
                .with_thread_ids(true)
                .with_level(true)
                .json()
                .finish();
            set_global_default(subscriber).is_ok()
        }
        LogFormat::Pretty => {
            let subscriber = tracing_subscriber::fmt()
                .with_env_filter(env_filter)
                .with_target(false)
                .with_thread_ids(true)
                .with_level(true)
                .pretty()
                .finish();
            set_global_default(subscriber).is_ok()
        }
    };

    Ok(TracingInitResult {
        resolved,
        initialized,
    })
}

pub fn resolve_tracing_config(
    cli_log_level: Option<&str>,
    rust_log: Option<&str>,
    config_log: Option<&LogConfig>,
) -> Result<ResolvedTracingConfig> {
    let (level_directive, level_source) = if let Some(level) = normalize_setting(cli_log_level) {
        (level.to_string(), LogLevelSource::Cli)
    } else if let Some(level) = normalize_setting(rust_log) {
        (level.to_string(), LogLevelSource::RustLogEnv)
    } else if let Some(level) =
        config_log.and_then(|log| normalize_setting(Some(log.level.as_str())))
    {
        (level.to_string(), LogLevelSource::Config)
    } else {
        ("info".to_string(), LogLevelSource::Default)
    };

    let format = parse_log_format(config_log.map(|log| log.format.as_str()))?;

    Ok(ResolvedTracingConfig {
        level_directive,
        level_source,
        format,
    })
}

pub fn parse_log_format(value: Option<&str>) -> Result<LogFormat> {
    match normalize_setting(value).map(|value| value.to_ascii_lowercase()) {
        None => Ok(LogFormat::Pretty),
        Some(value) if value == "json" => Ok(LogFormat::Json),
        Some(value) if value == "pretty" => Ok(LogFormat::Pretty),
        Some(value) => Err(Error::configuration(format!(
            "unsupported log format `{}`; expected `json` or `pretty`",
            value
        ))),
    }
}

fn normalize_setting(value: Option<&str>) -> Option<&str> {
    value.and_then(|value| {
        let trimmed = value.trim();
        (!trimmed.is_empty()).then_some(trimmed)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn log_config(level: &str, format: &str) -> LogConfig {
        LogConfig {
            level: level.to_string(),
            format: format.to_string(),
            console: true,
            file: None,
        }
    }

    #[test]
    fn cli_log_level_overrides_env_and_config() {
        let config = log_config("warn", "pretty");
        let resolved = resolve_tracing_config(Some("debug"), Some("error"), Some(&config))
            .expect("resolution should succeed");

        assert_eq!(resolved.level_directive, "debug");
        assert_eq!(resolved.level_source, LogLevelSource::Cli);
    }

    #[test]
    fn rust_log_overrides_config_when_cli_is_absent() {
        let config = log_config("warn", "pretty");
        let resolved = resolve_tracing_config(None, Some("trace,sqlx=warn"), Some(&config))
            .expect("resolution should succeed");

        assert_eq!(resolved.level_directive, "trace,sqlx=warn");
        assert_eq!(resolved.level_source, LogLevelSource::RustLogEnv);
    }

    #[test]
    fn config_level_is_used_when_higher_precedence_sources_are_absent() {
        let config = log_config("warn", "json");
        let resolved =
            resolve_tracing_config(None, None, Some(&config)).expect("resolution should succeed");

        assert_eq!(resolved.level_directive, "warn");
        assert_eq!(resolved.level_source, LogLevelSource::Config);
        assert_eq!(resolved.format, LogFormat::Json);
    }

    #[test]
    fn info_is_used_as_default_level() {
        let resolved =
            resolve_tracing_config(None, Some("   "), None).expect("resolution should succeed");

        assert_eq!(resolved.level_directive, "info");
        assert_eq!(resolved.level_source, LogLevelSource::Default);
        assert_eq!(resolved.format, LogFormat::Pretty);
    }

    #[test]
    fn parse_log_format_is_case_insensitive() {
        assert_eq!(parse_log_format(Some("JSON")).unwrap(), LogFormat::Json);
        assert_eq!(parse_log_format(Some("pretty")).unwrap(), LogFormat::Pretty);
    }

    #[test]
    fn parse_log_format_rejects_invalid_values() {
        let error = parse_log_format(Some("compact")).expect_err("format should fail");
        assert!(error.to_string().contains("unsupported log format"));
    }
}
