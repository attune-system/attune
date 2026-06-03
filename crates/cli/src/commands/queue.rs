use anyhow::Result;
use clap::Subcommand;
use serde::{Deserialize, Serialize};

use crate::client::ApiClient;
use crate::config::CliConfig;
use crate::output::{self, OutputFormat};

#[derive(Subcommand)]
pub enum QueueCommands {
    /// Show details of a work queue
    Show {
        /// Queue reference
        queue_ref: String,
    },
    /// Enable queue processing
    Enable {
        /// Queue reference
        queue_ref: String,
    },
    /// Disable queue processing
    Disable {
        /// Queue reference
        queue_ref: String,
    },
}

#[derive(Debug, Serialize, Deserialize)]
struct QueueDetail {
    id: i64,
    #[serde(rename = "ref")]
    queue_ref: String,
    #[serde(default)]
    pack_ref: Option<String>,
    label: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default = "default_true")]
    enabled: bool,
    #[serde(default = "default_true")]
    accepting_new_items: bool,
    dispatch_action_ref: String,
    created: String,
    updated: String,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Serialize)]
struct UpdateQueueOperationalFlags {
    enabled: bool,
}

pub async fn handle_queue_command(
    profile: &Option<String>,
    command: QueueCommands,
    api_url: &Option<String>,
    output_format: OutputFormat,
) -> Result<()> {
    match command {
        QueueCommands::Show { queue_ref } => {
            handle_show(queue_ref, profile, api_url, output_format).await
        }
        QueueCommands::Enable { queue_ref } => {
            handle_toggle(queue_ref, true, profile, api_url, output_format).await
        }
        QueueCommands::Disable { queue_ref } => {
            handle_toggle(queue_ref, false, profile, api_url, output_format).await
        }
    }
}

async fn handle_show(
    queue_ref: String,
    profile: &Option<String>,
    api_url: &Option<String>,
    output_format: OutputFormat,
) -> Result<()> {
    let config = CliConfig::load_with_profile(profile.as_deref())?;
    let mut client = ApiClient::from_config(&config, api_url);

    let path = format!("/queues/{}", queue_ref);
    let queue: QueueDetail = client.get(&path).await?;
    print_queue(queue, output_format, None)
}

async fn handle_toggle(
    queue_ref: String,
    enabled: bool,
    profile: &Option<String>,
    api_url: &Option<String>,
    output_format: OutputFormat,
) -> Result<()> {
    let config = CliConfig::load_with_profile(profile.as_deref())?;
    let mut client = ApiClient::from_config(&config, api_url);

    let path = format!("/queues/{}", queue_ref);
    let queue: QueueDetail = client
        .put(&path, &UpdateQueueOperationalFlags { enabled })
        .await?;
    print_queue(
        queue,
        output_format,
        Some(if enabled { "enabled" } else { "disabled" }),
    )
}

fn print_queue(
    queue: QueueDetail,
    output_format: OutputFormat,
    status_message: Option<&str>,
) -> Result<()> {
    match output_format {
        OutputFormat::Json | OutputFormat::Yaml => {
            output::print_output(&queue, output_format)?;
        }
        OutputFormat::Table => {
            if let Some(status_message) = status_message {
                output::print_success(&format!(
                    "Queue '{}' {} successfully",
                    queue.queue_ref, status_message
                ));
            } else {
                output::print_section(&format!("Queue: {}", queue.queue_ref));
            }
            output::print_key_value_table(vec![
                ("Ref", queue.queue_ref.clone()),
                (
                    "Pack",
                    queue.pack_ref.as_deref().unwrap_or("None").to_string(),
                ),
                ("Label", queue.label.clone()),
                (
                    "Description",
                    queue.description.unwrap_or_else(|| "None".to_string()),
                ),
                ("Enabled", output::format_bool(queue.enabled)),
                (
                    "Accepting Items",
                    output::format_bool(queue.accepting_new_items),
                ),
                ("Dispatch Action", queue.dispatch_action_ref.clone()),
                ("Created", output::format_timestamp(&queue.created)),
                ("Updated", output::format_timestamp(&queue.updated)),
            ]);
        }
    }

    Ok(())
}
