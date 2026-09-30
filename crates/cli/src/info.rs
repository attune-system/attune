use crate::{
    client::{sanitize_url_for_display, ApiClient},
    config::CliConfig,
    output::{self, OutputFormat},
};
use anyhow::{bail, Result};
use attune_common::build_info::BuildInfo;
use serde::Serialize;
use std::time::Duration;

#[derive(Debug, Serialize)]
pub struct LocalInfo {
    pub binary: String,
    #[serde(flatten)]
    pub build: BuildInfo,
}

impl LocalInfo {
    pub fn current(binary: &str) -> Self {
        Self {
            binary: binary.to_string(),
            build: BuildInfo::current(),
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ServerInfo {
    Connected {
        profile: String,
        api_url: String,
        #[serde(flatten)]
        build: BuildInfo,
    },
    Unavailable {
        profile: String,
        api_url: String,
        error: String,
    },
}

#[derive(Debug, Serialize)]
pub struct ClientInfo {
    pub local: LocalInfo,
    pub server: ServerInfo,
}

impl ClientInfo {
    pub fn server_available(&self) -> bool {
        matches!(self.server, ServerInfo::Connected { .. })
    }
}

pub async fn get_info(binary: &str, profile: &str, client: &mut ApiClient) -> ClientInfo {
    let profile = profile.to_string();
    let api_url = sanitize_url_for_display(client.base_url());
    let response = tokio::time::timeout(Duration::from_secs(10), client.get::<BuildInfo>("/info"))
        .await
        .map_err(|_| "Server info request timed out after 10 seconds".to_string())
        .and_then(|response| response.map_err(|error| format!("{error:#}")));
    let server = match response {
        Ok(build) => ServerInfo::Connected {
            profile,
            api_url,
            build,
        },
        Err(error) => ServerInfo::Unavailable {
            profile,
            api_url,
            error,
        },
    };
    ClientInfo {
        local: LocalInfo::current(binary),
        server,
    }
}

fn print_local(local: &LocalInfo) {
    output::print_key_value_table(vec![
        ("Local binary", local.binary.clone()),
        ("Local version", local.build.version.clone()),
        ("Local Git SHA", local.build.git_sha.clone()),
    ]);
}

pub async fn handle(
    profile: &Option<String>,
    api_url: &Option<String>,
    local_only: bool,
    format: OutputFormat,
) -> Result<()> {
    if local_only {
        let local = LocalInfo::current("attune");
        if format == OutputFormat::Table {
            print_local(&local);
        } else {
            output::print_output(&local, format)?;
        }
        return Ok(());
    }
    let config = CliConfig::load_with_profile(profile.as_deref())?;
    let mut client = ApiClient::from_config_with_timeout(&config, api_url, Duration::from_secs(10));
    let report = get_info("attune", &config.current_profile, &mut client).await;
    match format {
        OutputFormat::Table => {
            print_local(&report.local);
            match &report.server {
                ServerInfo::Connected {
                    profile,
                    api_url,
                    build,
                } => output::print_key_value_table(vec![
                    ("Server profile", profile.clone()),
                    ("Server API", api_url.clone()),
                    ("Server version", build.version.clone()),
                    ("Server Git SHA", build.git_sha.clone()),
                ]),
                ServerInfo::Unavailable {
                    profile,
                    api_url,
                    error,
                } => output::print_key_value_table(vec![
                    ("Server profile", profile.clone()),
                    ("Server API", api_url.clone()),
                    ("Server error", error.clone()),
                ]),
            }
        }
        _ => output::print_output(&report, format)?,
    }
    if !report.server_available() {
        bail!("Could not fetch server build information");
    }
    Ok(())
}
