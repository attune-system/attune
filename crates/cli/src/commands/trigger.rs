use anyhow::Result;
use clap::Subcommand;
use serde::{Deserialize, Serialize};

use crate::client::ApiClient;
use crate::config::CliConfig;
use crate::output::{self, OutputFormat};

#[derive(Subcommand)]
pub enum TriggerCommands {
    /// List all triggers
    List {
        /// Filter by pack name
        #[arg(long)]
        pack: Option<String>,
    },
    /// Show details of a specific trigger
    Show {
        /// Trigger reference (pack.trigger or ID)
        trigger_ref: String,
    },
    /// Update a trigger
    Update {
        /// Trigger reference (pack.trigger or ID)
        trigger_ref: String,

        /// Update label
        #[arg(long)]
        label: Option<String>,

        /// Update description
        #[arg(long)]
        description: Option<String>,

        /// Update enabled status
        #[arg(long)]
        enabled: Option<bool>,

        /// Update rule subscription visibility: public, private, or restricted
        #[arg(long)]
        reference_visibility: Option<String>,

        /// Replace allowed caller pack refs for restricted visibility
        #[arg(long, value_delimiter = ',')]
        reference_allowed_pack_refs: Vec<String>,
    },
    /// Enable a trigger
    Enable {
        /// Trigger reference (pack.trigger or ID)
        trigger_ref: String,
    },
    /// Disable a trigger
    Disable {
        /// Trigger reference (pack.trigger or ID)
        trigger_ref: String,
    },
    /// Delete a trigger
    Delete {
        /// Trigger reference (pack.trigger or ID)
        trigger_ref: String,

        /// Skip confirmation prompt
        #[arg(long)]
        yes: bool,
    },
}

#[derive(Debug, Serialize, Deserialize)]
struct Trigger {
    id: i64,
    #[serde(rename = "ref")]
    trigger_ref: String,
    #[serde(default)]
    pack: Option<i64>,
    #[serde(default)]
    pack_ref: Option<String>,
    label: String,
    description: Option<String>,
    enabled: bool,
    #[serde(default)]
    param_schema: Option<serde_json::Value>,
    #[serde(default)]
    out_schema: Option<serde_json::Value>,
    #[serde(default)]
    webhook_enabled: Option<bool>,
    #[serde(default)]
    webhook_key: Option<String>,
    #[serde(default)]
    reference_visibility: Option<String>,
    #[serde(default)]
    reference_allowed_pack_refs: Vec<String>,
    created: String,
    updated: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct TriggerDetail {
    id: i64,
    #[serde(rename = "ref")]
    trigger_ref: String,
    #[serde(default)]
    pack: Option<i64>,
    #[serde(default)]
    pack_ref: Option<String>,
    label: String,
    description: Option<String>,
    enabled: bool,
    #[serde(default)]
    param_schema: Option<serde_json::Value>,
    #[serde(default)]
    out_schema: Option<serde_json::Value>,
    #[serde(default)]
    webhook_enabled: Option<bool>,
    #[serde(default)]
    webhook_key: Option<String>,
    #[serde(default)]
    reference_visibility: Option<String>,
    #[serde(default)]
    reference_allowed_pack_refs: Vec<String>,
    created: String,
    updated: String,
}

pub async fn handle_trigger_command(
    profile: &Option<String>,
    command: TriggerCommands,
    api_url: &Option<String>,
    output_format: OutputFormat,
) -> Result<()> {
    match command {
        TriggerCommands::List { pack } => handle_list(pack, profile, api_url, output_format).await,
        TriggerCommands::Show { trigger_ref } => {
            handle_show(trigger_ref, profile, api_url, output_format).await
        }
        TriggerCommands::Update {
            trigger_ref,
            label,
            description,
            enabled,
            reference_visibility,
            reference_allowed_pack_refs,
        } => {
            handle_update(
                trigger_ref,
                label,
                description,
                enabled,
                reference_visibility,
                reference_allowed_pack_refs,
                profile,
                api_url,
                output_format,
            )
            .await
        }
        TriggerCommands::Enable { trigger_ref } => {
            handle_toggle(trigger_ref, true, profile, api_url, output_format).await
        }
        TriggerCommands::Disable { trigger_ref } => {
            handle_toggle(trigger_ref, false, profile, api_url, output_format).await
        }
        TriggerCommands::Delete { trigger_ref, yes } => {
            handle_delete(trigger_ref, yes, profile, api_url, output_format).await
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
        format!("/triggers?pack={}", pack_name)
    } else {
        "/triggers".to_string()
    };

    let triggers: Vec<Trigger> = client.get(&path).await?;

    match output_format {
        OutputFormat::Json | OutputFormat::Yaml => {
            output::print_output(&triggers, output_format)?;
        }
        OutputFormat::Table => {
            if triggers.is_empty() {
                output::print_info("No triggers found");
            } else {
                let mut table = output::create_table();
                output::add_header(
                    &mut table,
                    vec![
                        "Ref",
                        "Pack",
                        "Label",
                        "Enabled",
                        "Visibility",
                        "Description",
                    ],
                );

                for trigger in triggers {
                    table.add_row(vec![
                        trigger.trigger_ref.clone(),
                        trigger.pack_ref.as_deref().unwrap_or("").to_string(),
                        trigger.label.clone(),
                        output::format_bool(trigger.enabled),
                        trigger
                            .reference_visibility
                            .unwrap_or_else(|| "public".to_string()),
                        output::truncate(&trigger.description.unwrap_or_default(), 50),
                    ]);
                }

                println!("{}", table);
            }
        }
    }

    Ok(())
}

async fn handle_show(
    trigger_ref: String,
    profile: &Option<String>,
    api_url: &Option<String>,
    output_format: OutputFormat,
) -> Result<()> {
    let config = CliConfig::load_with_profile(profile.as_deref())?;
    let mut client = ApiClient::from_config(&config, api_url);

    let path = format!("/triggers/{}", trigger_ref);
    let trigger: TriggerDetail = client.get(&path).await?;

    match output_format {
        OutputFormat::Json | OutputFormat::Yaml => {
            output::print_output(&trigger, output_format)?;
        }
        OutputFormat::Table => {
            output::print_section(&format!("Trigger: {}", trigger.trigger_ref));
            output::print_key_value_table(vec![
                ("Ref", trigger.trigger_ref.clone()),
                (
                    "Pack",
                    trigger.pack_ref.as_deref().unwrap_or("None").to_string(),
                ),
                ("Label", trigger.label.clone()),
                (
                    "Description",
                    trigger.description.unwrap_or_else(|| "None".to_string()),
                ),
                ("Enabled", output::format_bool(trigger.enabled)),
                (
                    "Webhook Enabled",
                    output::format_bool(trigger.webhook_enabled.unwrap_or(false)),
                ),
                (
                    "Subscription Visibility",
                    trigger
                        .reference_visibility
                        .clone()
                        .unwrap_or_else(|| "public".to_string()),
                ),
                (
                    "Allowed Caller Packs",
                    if trigger.reference_allowed_pack_refs.is_empty() {
                        "None".to_string()
                    } else {
                        trigger.reference_allowed_pack_refs.join(", ")
                    },
                ),
                ("Created", output::format_timestamp(&trigger.created)),
                ("Updated", output::format_timestamp(&trigger.updated)),
            ]);

            if let Some(webhook_key) = &trigger.webhook_key {
                output::print_section("Webhook");
                output::print_info(&format!("Key: {}", webhook_key));
            }

            if let Some(param_schema) = &trigger.param_schema {
                if !param_schema.is_null() {
                    output::print_section("Parameter Schema");
                    println!("{}", serde_json::to_string_pretty(param_schema)?);
                }
            }

            if let Some(out_schema) = &trigger.out_schema {
                if !out_schema.is_null() {
                    output::print_section("Output Schema");
                    println!("{}", serde_json::to_string_pretty(out_schema)?);
                }
            }
        }
    }

    Ok(())
}

async fn handle_update(
    trigger_ref: String,
    label: Option<String>,
    description: Option<String>,
    enabled: Option<bool>,
    reference_visibility: Option<String>,
    reference_allowed_pack_refs: Vec<String>,
    profile: &Option<String>,
    api_url: &Option<String>,
    output_format: OutputFormat,
) -> Result<()> {
    let config = CliConfig::load_with_profile(profile.as_deref())?;
    let mut client = ApiClient::from_config(&config, api_url);

    // Check that at least one field is provided
    if label.is_none()
        && description.is_none()
        && enabled.is_none()
        && reference_visibility.is_none()
        && reference_allowed_pack_refs.is_empty()
    {
        anyhow::bail!("At least one field must be provided to update");
    }

    #[derive(Serialize)]
    #[serde(tag = "op", content = "value", rename_all = "snake_case")]
    enum TriggerDescriptionPatch {
        Set(String),
    }

    #[derive(Serialize)]
    struct UpdateTriggerRequest {
        #[serde(skip_serializing_if = "Option::is_none")]
        label: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        description: Option<TriggerDescriptionPatch>,
        #[serde(skip_serializing_if = "Option::is_none")]
        enabled: Option<bool>,
        #[serde(skip_serializing_if = "Option::is_none")]
        reference_visibility: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        reference_allowed_pack_refs: Option<Vec<String>>,
    }

    let request = UpdateTriggerRequest {
        label,
        description: description.map(TriggerDescriptionPatch::Set),
        enabled,
        reference_visibility,
        reference_allowed_pack_refs: if reference_allowed_pack_refs.is_empty() {
            None
        } else {
            Some(reference_allowed_pack_refs)
        },
    };

    let path = format!("/triggers/{}", trigger_ref);
    let trigger: TriggerDetail = client.put(&path, &request).await?;

    match output_format {
        OutputFormat::Json | OutputFormat::Yaml => {
            output::print_output(&trigger, output_format)?;
        }
        OutputFormat::Table => {
            output::print_success(&format!(
                "Trigger '{}' updated successfully",
                trigger.trigger_ref
            ));
            output::print_key_value_table(vec![
                ("Ref", trigger.trigger_ref.clone()),
                (
                    "Pack",
                    trigger.pack_ref.as_deref().unwrap_or("None").to_string(),
                ),
                ("Label", trigger.label.clone()),
                (
                    "Description",
                    trigger.description.unwrap_or_else(|| "None".to_string()),
                ),
                ("Enabled", output::format_bool(trigger.enabled)),
                (
                    "Subscription Visibility",
                    trigger
                        .reference_visibility
                        .clone()
                        .unwrap_or_else(|| "public".to_string()),
                ),
                ("Updated", output::format_timestamp(&trigger.updated)),
            ]);
        }
    }

    Ok(())
}

async fn handle_toggle(
    trigger_ref: String,
    enabled: bool,
    profile: &Option<String>,
    api_url: &Option<String>,
    output_format: OutputFormat,
) -> Result<()> {
    let config = CliConfig::load_with_profile(profile.as_deref())?;
    let mut client = ApiClient::from_config(&config, api_url);

    let path = format!(
        "/triggers/{}/{}",
        trigger_ref,
        if enabled { "enable" } else { "disable" }
    );
    let trigger: TriggerDetail = client.post(&path, &serde_json::json!({})).await?;

    match output_format {
        OutputFormat::Json | OutputFormat::Yaml => {
            output::print_output(&trigger, output_format)?;
        }
        OutputFormat::Table => {
            output::print_success(&format!(
                "Trigger '{}' {} successfully",
                trigger.trigger_ref,
                if enabled { "enabled" } else { "disabled" }
            ));
            output::print_key_value_table(vec![
                ("Ref", trigger.trigger_ref.clone()),
                ("Enabled", output::format_bool(trigger.enabled)),
                ("Updated", output::format_timestamp(&trigger.updated)),
            ]);
        }
    }

    Ok(())
}

async fn handle_delete(
    trigger_ref: String,
    yes: bool,
    profile: &Option<String>,
    api_url: &Option<String>,
    output_format: OutputFormat,
) -> Result<()> {
    let config = CliConfig::load_with_profile(profile.as_deref())?;
    let mut client = ApiClient::from_config(&config, api_url);

    // Confirm deletion unless --yes is provided
    if !yes && matches!(output_format, OutputFormat::Table) {
        let confirm = dialoguer::Confirm::new()
            .with_prompt(format!(
                "Are you sure you want to delete trigger '{}'?",
                trigger_ref
            ))
            .default(false)
            .interact()?;

        if !confirm {
            output::print_info("Delete cancelled");
            return Ok(());
        }
    }

    let path = format!("/triggers/{}", trigger_ref);
    client.delete_no_response(&path).await?;

    match output_format {
        OutputFormat::Json | OutputFormat::Yaml => {
            let msg = serde_json::json!({"message": "Trigger deleted successfully"});
            output::print_output(&msg, output_format)?;
        }
        OutputFormat::Table => {
            output::print_success(&format!("Trigger '{}' deleted successfully", trigger_ref));
        }
    }

    Ok(())
}
