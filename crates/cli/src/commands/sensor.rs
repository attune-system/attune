use anyhow::Result;
use clap::Subcommand;
use serde::{Deserialize, Serialize};

use crate::client::ApiClient;
use crate::config::CliConfig;
use crate::output::{self, OutputFormat};

#[derive(Subcommand)]
pub enum SensorCommands {
    /// List all sensors
    List {
        /// Filter by pack name
        #[arg(long)]
        pack: Option<String>,
    },
    /// Show details of a specific sensor
    Show {
        /// Sensor reference (pack.sensor or ID)
        sensor_ref: String,
    },
    /// Enable a sensor
    Enable {
        /// Sensor reference (pack.sensor or ID)
        sensor_ref: String,
    },
    /// Disable a sensor
    Disable {
        /// Sensor reference (pack.sensor or ID)
        sensor_ref: String,
    },
}

#[derive(Debug, Serialize, Deserialize)]
struct Sensor {
    id: i64,
    #[serde(rename = "ref")]
    sensor_ref: String,
    #[serde(default)]
    pack: Option<i64>,
    #[serde(default)]
    pack_ref: Option<String>,
    label: String,
    description: Option<String>,
    #[serde(default)]
    trigger_types: Vec<String>,
    enabled: bool,
    created: String,
    updated: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct SensorDetail {
    id: i64,
    #[serde(rename = "ref")]
    sensor_ref: String,
    #[serde(default)]
    pack: Option<i64>,
    #[serde(default)]
    pack_ref: Option<String>,
    label: String,
    description: Option<String>,
    #[serde(default)]
    trigger_types: Vec<String>,
    #[serde(default)]
    entry_point: Option<String>,
    enabled: bool,
    #[serde(default)]
    poll_interval: Option<i32>,
    #[serde(default)]
    metadata: Option<serde_json::Value>,
    created: String,
    updated: String,
}

pub async fn handle_sensor_command(
    profile: &Option<String>,
    command: SensorCommands,
    api_url: &Option<String>,
    output_format: OutputFormat,
) -> Result<()> {
    match command {
        SensorCommands::List { pack } => handle_list(pack, profile, api_url, output_format).await,
        SensorCommands::Show { sensor_ref } => {
            handle_show(sensor_ref, profile, api_url, output_format).await
        }
        SensorCommands::Enable { sensor_ref } => {
            handle_toggle(sensor_ref, true, profile, api_url, output_format).await
        }
        SensorCommands::Disable { sensor_ref } => {
            handle_toggle(sensor_ref, false, profile, api_url, output_format).await
        }
    }
}

async fn handle_list(
    pack: Option<String>,
    profile: &Option<String>,
    api_url: &Option<String>,
    output_format: OutputFormat,
) -> Result<()> {
    let config = CliConfig::load_with_profile(profile.as_deref())?;
    let mut client = ApiClient::from_config(&config, api_url);

    let path = if let Some(pack_name) = pack {
        format!("/sensors?pack={}", pack_name)
    } else {
        "/sensors".to_string()
    };

    let sensors: Vec<Sensor> = client.get_paginated(&path).await?;

    match output_format {
        OutputFormat::Json | OutputFormat::Yaml => {
            output::print_output(&sensors, output_format)?;
        }
        OutputFormat::Table => {
            if sensors.is_empty() {
                output::print_info("No sensors found");
            } else {
                let mut table = output::create_table();
                output::add_header(
                    &mut table,
                    vec!["Ref", "Pack", "Label", "Trigger", "Enabled", "Description"],
                );

                for sensor in sensors {
                    table.add_row(vec![
                        sensor.sensor_ref.clone(),
                        sensor.pack_ref.as_deref().unwrap_or("").to_string(),
                        sensor.label.clone(),
                        sensor.trigger_types.join(", "),
                        output::format_bool(sensor.enabled),
                        output::truncate(&sensor.description.unwrap_or_default(), 50),
                    ]);
                }

                println!("{}", table);
            }
        }
    }

    Ok(())
}

async fn handle_toggle(
    sensor_ref: String,
    enabled: bool,
    profile: &Option<String>,
    api_url: &Option<String>,
    output_format: OutputFormat,
) -> Result<()> {
    let config = CliConfig::load_with_profile(profile.as_deref())?;
    let mut client = ApiClient::from_config(&config, api_url);

    let path = format!(
        "/sensors/{}/{}",
        sensor_ref,
        if enabled { "enable" } else { "disable" }
    );
    let sensor: SensorDetail = client.post(&path, &serde_json::json!({})).await?;

    match output_format {
        OutputFormat::Json | OutputFormat::Yaml => {
            output::print_output(&sensor, output_format)?;
        }
        OutputFormat::Table => {
            output::print_success(&format!(
                "Sensor '{}' {} successfully",
                sensor.sensor_ref,
                if enabled { "enabled" } else { "disabled" }
            ));
            output::print_key_value_table(vec![
                ("Ref", sensor.sensor_ref.clone()),
                ("Enabled", output::format_bool(sensor.enabled)),
                ("Updated", output::format_timestamp(&sensor.updated)),
            ]);
        }
    }

    Ok(())
}

async fn handle_show(
    sensor_ref: String,
    profile: &Option<String>,
    api_url: &Option<String>,
    output_format: OutputFormat,
) -> Result<()> {
    let config = CliConfig::load_with_profile(profile.as_deref())?;
    let mut client = ApiClient::from_config(&config, api_url);

    let path = format!("/sensors/{}", sensor_ref);
    let sensor: SensorDetail = client.get(&path).await?;

    match output_format {
        OutputFormat::Json | OutputFormat::Yaml => {
            output::print_output(&sensor, output_format)?;
        }
        OutputFormat::Table => {
            output::print_section(&format!("Sensor: {}", sensor.sensor_ref));
            output::print_key_value_table(vec![
                ("Ref", sensor.sensor_ref.clone()),
                (
                    "Pack",
                    sensor.pack_ref.as_deref().unwrap_or("None").to_string(),
                ),
                ("Label", sensor.label.clone()),
                (
                    "Description",
                    sensor.description.unwrap_or_else(|| "None".to_string()),
                ),
                ("Trigger Types", sensor.trigger_types.join(", ")),
                (
                    "Entry Point",
                    sensor.entry_point.as_deref().unwrap_or("N/A").to_string(),
                ),
                ("Enabled", output::format_bool(sensor.enabled)),
                (
                    "Poll Interval",
                    sensor
                        .poll_interval
                        .map(|i| format!("{}s", i))
                        .unwrap_or_else(|| "N/A".to_string()),
                ),
                ("Created", output::format_timestamp(&sensor.created)),
                ("Updated", output::format_timestamp(&sensor.updated)),
            ]);

            if let Some(metadata) = sensor.metadata {
                if !metadata.is_null() {
                    output::print_section("Metadata");
                    println!("{}", serde_json::to_string_pretty(&metadata)?);
                }
            }
        }
    }

    Ok(())
}
