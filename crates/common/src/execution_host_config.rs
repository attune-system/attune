//! Execution-only host configuration. Workload hosts never load the general
//! service Config, database/broker settings, or JWT/master encryption secrets.
use crate::{workload_launch::LaunchToken, workload_launch_io::LaunchHttpPolicy, Error, Result};
use std::{collections::HashMap, fmt, path::PathBuf};

pub struct ExecutionHostConfig {
    pub api_origin: reqwest::Url,
    pub enrollment: LaunchToken,
    pub custody_root: PathBuf,
    pub http_policy: LaunchHttpPolicy,
}
impl fmt::Debug for ExecutionHostConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ExecutionHostConfig")
            .field("api_origin", &self.api_origin)
            .field("enrollment", &self.enrollment)
            .field("custody_root", &self.custody_root)
            .finish_non_exhaustive()
    }
}
fn invalid(message: &'static str) -> Error {
    Error::configuration(message)
}
impl ExecutionHostConfig {
    pub fn from_environment() -> Result<Self> {
        let values = std::env::vars_os()
            .filter_map(|(key, value)| key.into_string().ok().map(|key| (key, value)))
            .map(|(key, value)| value.into_string().map(|value| (key, value)))
            .collect::<std::result::Result<HashMap<_, _>, _>>()
            .map_err(|_| invalid("Execution host environment must contain valid UTF-8 values"))?;
        Self::from_values(&values)
    }
    /// Instance-local input permits deterministic tests without changing the
    /// process environment shared with concurrent clients and child launchers.
    pub fn from_values(values: &HashMap<String, String>) -> Result<Self> {
        if values.keys().any(|key| {
            matches!(
                key.as_str(),
                "ATTUNE_CONFIG"
                    | "DATABASE_URL"
                    | "ATTUNE_DATABASE_URL"
                    | "MESSAGE_QUEUE_URL"
                    | "RABBITMQ_URL"
                    | "JWT_SECRET"
                    | "ENCRYPTION_KEY"
            ) || key.starts_with("ATTUNE__DATABASE__")
                || key.starts_with("ATTUNE__MESSAGE_QUEUE__")
                || key.starts_with("ATTUNE__SECURITY__")
        }) {
            return Err(invalid("Execution hosts reject general service configuration and database, broker, JWT or master encryption credentials"));
        }
        let http_policy = match values
            .get("ATTUNE_WORKLOAD_ALLOW_DEVELOPMENT_HTTP")
            .map(String::as_str)
        {
            None | Some("false" | "0") => LaunchHttpPolicy::TlsOrLoopback,
            Some("true" | "1") => LaunchHttpPolicy::DevelopmentPlainHttp,
            _ => return Err(invalid("Invalid execution host development HTTP policy")),
        };
        let api_origin = values.get("ATTUNE_API_URL")
            .and_then(|value| reqwest::Url::parse(value).ok())
            .filter(|url| http_policy.allows_origin(url)
                && url.host_str().is_some() && url.path()=="/"
                && url.username().is_empty() && url.password().is_none()
                && url.query().is_none() && url.fragment().is_none())
            .ok_or_else(||invalid("ATTUNE_API_URL must be an admitted HTTPS API origin (literal loopback HTTP or explicit trusted development HTTP policy only)"))?;
        let enrollment = LaunchToken::new(
            values
                .get("ATTUNE_WORKLOAD_ENROLLMENT_TOKEN")
                .cloned()
                .ok_or_else(|| invalid("Execution host enrollment credential is required"))?,
        )
        .map_err(|_| invalid("Invalid execution host enrollment credential"))?;
        let custody_root = PathBuf::from(
            values
                .get("ATTUNE_WORKLOAD_CUSTODY_DIR")
                .map(String::as_str)
                .unwrap_or("/var/lib/attune/custody"),
        );
        if !custody_root.is_absolute()
            || custody_root.parent().is_none()
            || custody_root
                .components()
                .any(|part| matches!(part, std::path::Component::ParentDir))
        {
            return Err(invalid(
                "Execution host custody must use a dedicated absolute persistent directory",
            ));
        }
        Ok(Self {
            api_origin,
            enrollment,
            custody_root,
            http_policy,
        })
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn values() -> HashMap<String, String> {
        HashMap::from([
            ("ATTUNE_API_URL".into(), "https://owned-api.invalid/".into()),
            (
                "ATTUNE_WORKLOAD_ENROLLMENT_TOKEN".into(),
                "owned-narrow-enrollment-fixture".into(),
            ),
        ])
    }
    #[test]
    fn rejects_host_held_control_plane_authority_without_printing_values() {
        for name in [
            "ATTUNE_CONFIG",
            "DATABASE_URL",
            "JWT_SECRET",
            "ENCRYPTION_KEY",
            "ATTUNE__DATABASE__URL",
            "ATTUNE__MESSAGE_QUEUE__URL",
            "ATTUNE__SECURITY__JWT_SECRET",
        ] {
            let mut input = values();
            input.insert(name.into(), "owned-master-secret-value".into());
            let error = ExecutionHostConfig::from_values(&input).unwrap_err();
            assert!(!error.to_string().contains("owned-master-secret-value"));
        }
        let config = ExecutionHostConfig::from_values(&values()).unwrap();
        assert!(!format!("{config:?}").contains("owned-narrow-enrollment-fixture"));
    }
    #[test]
    fn rejects_ambiguous_urls_and_requires_explicit_remote_development_http() {
        for url in [
            "http://owned-api.invalid/",
            "https://user:password@owned-api.invalid/",
            "https://owned-api.invalid/api/v1",
            "https://owned-api.invalid/?token=secret",
        ] {
            let mut input = values();
            input.insert("ATTUNE_API_URL".into(), url.into());
            assert!(ExecutionHostConfig::from_values(&input).is_err());
        }
        let mut input = values();
        input.insert("ATTUNE_API_URL".into(), "http://owned-api.invalid/".into());
        input.insert(
            "ATTUNE_WORKLOAD_ALLOW_DEVELOPMENT_HTTP".into(),
            "true".into(),
        );
        assert!(ExecutionHostConfig::from_values(&input).is_ok());
        input.insert(
            "ATTUNE_WORKLOAD_ALLOW_DEVELOPMENT_HTTP".into(),
            "maybe".into(),
        );
        assert!(ExecutionHostConfig::from_values(&input).is_err());
    }
}
