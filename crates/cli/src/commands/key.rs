use anyhow::Result;
use clap::Subcommand;
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use sha2::{Digest, Sha256};

use crate::client::ApiClient;
use crate::config::CliConfig;
use crate::output::{self, OutputFormat};

#[derive(Subcommand)]
pub enum KeyCommands {
    /// List all keys (values redacted)
    List {
        /// Filter by owner type (system, identity, pack, action, sensor)
        #[arg(long)]
        owner_type: Option<String>,

        /// Filter by owner string
        #[arg(long)]
        owner: Option<String>,

        /// Page number
        #[arg(long, default_value = "1")]
        page: u32,

        /// Items per page
        #[arg(long, default_value = "50")]
        per_page: u32,
    },
    /// Show details of a specific key
    Show {
        /// Key reference identifier
        key_ref: String,

        /// Decrypt and display the actual value (otherwise a SHA-256 hash is shown)
        #[arg(short = 'd', long)]
        decrypt: bool,
    },
    /// Create a new key/secret
    Create {
        /// Identifier within the owner scope (e.g., "github_token")
        #[arg(long)]
        local_ref: String,

        /// Human-readable name for the key
        #[arg(long)]
        name: String,

        /// The secret value to store. Plain strings are stored as JSON strings.
        /// Use JSON syntax for structured values (e.g., '{"user":"admin","pass":"s3cret"}').
        #[arg(long)]
        value: String,

        /// Owner type (system, identity, pack, action, sensor)
        #[arg(long, default_value = "system")]
        owner_type: String,

        /// Owner identity login
        #[arg(long)]
        owner_identity_login: Option<String>,

        /// Owner pack reference (auto-resolves pack ID)
        #[arg(long)]
        owner_pack_ref: Option<String>,

        /// Owner action reference (auto-resolves action ID)
        #[arg(long)]
        owner_action_ref: Option<String>,

        /// Owner sensor reference (auto-resolves sensor ID)
        #[arg(long)]
        owner_sensor_ref: Option<String>,

        /// Encrypt the value before storing (default: unencrypted)
        #[arg(short = 'e', long)]
        encrypt: bool,
    },
    /// Update an existing key/secret
    Update {
        /// Key reference identifier
        key_ref: String,

        /// Update the human-readable name
        #[arg(long)]
        name: Option<String>,

        /// Update the secret value. Plain strings are stored as JSON strings.
        /// Use JSON syntax for structured values (e.g., '{"user":"admin","pass":"s3cret"}').
        #[arg(long)]
        value: Option<String>,

        /// Update encryption status
        #[arg(long)]
        encrypted: Option<bool>,
    },
    /// Delete a key/secret
    Delete {
        /// Key reference identifier
        key_ref: String,

        /// Skip confirmation prompt
        #[arg(long)]
        yes: bool,
    },
}

// ── Response / request types used for (de)serialization against the API ────

#[derive(Debug, Serialize, Deserialize)]
struct KeyResponse {
    id: i64,
    #[serde(rename = "ref")]
    key_ref: String,
    local_ref: String,
    owner_type: String,
    #[serde(default)]
    owner: Option<String>,
    #[serde(default)]
    owner_identity: Option<i64>,
    #[serde(default)]
    owner_pack: Option<i64>,
    #[serde(default)]
    owner_pack_ref: Option<String>,
    #[serde(default)]
    owner_action: Option<i64>,
    #[serde(default)]
    owner_action_ref: Option<String>,
    #[serde(default)]
    owner_sensor: Option<i64>,
    #[serde(default)]
    owner_sensor_ref: Option<String>,
    name: String,
    encrypted: bool,
    #[serde(default)]
    value: JsonValue,
    created: String,
    updated: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct KeySummary {
    id: i64,
    #[serde(rename = "ref")]
    key_ref: String,
    local_ref: String,
    owner_type: String,
    #[serde(default)]
    owner: Option<String>,
    name: String,
    encrypted: bool,
    created: String,
}

#[derive(Debug, Serialize)]
struct CreateKeyRequestBody {
    local_ref: String,
    owner_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    owner_identity_login: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    owner_pack_ref: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    owner_action_ref: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    owner_sensor_ref: Option<String>,
    name: String,
    value: JsonValue,
    encrypted: bool,
}

#[derive(Debug, Serialize)]
struct UpdateKeyRequestBody {
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    value: Option<JsonValue>,
    #[serde(skip_serializing_if = "Option::is_none")]
    encrypted: Option<bool>,
}

// ── Command dispatch ───────────────────────────────────────────────────────

pub async fn handle_key_command(
    profile: &Option<String>,
    command: KeyCommands,
    api_url: &Option<String>,
    output_format: OutputFormat,
) -> Result<()> {
    match command {
        KeyCommands::List {
            owner_type,
            owner,
            page,
            per_page,
        } => {
            handle_list(
                profile,
                owner_type,
                owner,
                page,
                per_page,
                api_url,
                output_format,
            )
            .await
        }
        KeyCommands::Show { key_ref, decrypt } => {
            handle_show(profile, key_ref, decrypt, api_url, output_format).await
        }
        KeyCommands::Create {
            local_ref,
            name,
            value,
            owner_type,
            owner_identity_login,
            owner_pack_ref,
            owner_action_ref,
            owner_sensor_ref,
            encrypt,
        } => {
            handle_create(
                profile,
                local_ref,
                name,
                value,
                owner_type,
                owner_identity_login,
                owner_pack_ref,
                owner_action_ref,
                owner_sensor_ref,
                encrypt,
                api_url,
                output_format,
            )
            .await
        }
        KeyCommands::Update {
            key_ref,
            name,
            value,
            encrypted,
        } => {
            handle_update(
                profile,
                key_ref,
                name,
                value,
                encrypted,
                api_url,
                output_format,
            )
            .await
        }
        KeyCommands::Delete { key_ref, yes } => {
            handle_delete(profile, key_ref, yes, api_url, output_format).await
        }
    }
}

// ── Handlers ───────────────────────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
async fn handle_list(
    profile: &Option<String>,
    owner_type: Option<String>,
    owner: Option<String>,
    page: u32,
    per_page: u32,
    api_url: &Option<String>,
    output_format: OutputFormat,
) -> Result<()> {
    let config = CliConfig::load_with_profile(profile.as_deref())?;
    let mut client = ApiClient::from_config(&config, api_url);

    let mut query = url::form_urlencoded::Serializer::new(String::new());
    query.append_pair("page", &page.to_string());
    query.append_pair("per_page", &per_page.to_string());
    if let Some(owner_type) = owner_type {
        query.append_pair("owner_type", &owner_type);
    }
    if let Some(owner) = owner {
        query.append_pair("owner", &owner);
    }

    let path = format!("/keys?{}", query.finish());
    let keys: Vec<KeySummary> = client.get_paginated(&path).await?;

    match output_format {
        OutputFormat::Json | OutputFormat::Yaml => {
            output::print_output(&keys, output_format)?;
        }
        OutputFormat::Table => {
            if keys.is_empty() {
                output::print_info("No keys found");
            } else {
                let mut table = output::create_table();
                output::add_header(
                    &mut table,
                    vec!["Ref", "Name", "Owner", "Encrypted", "Created"],
                );

                for key in keys {
                    let owner_display = key.owner.as_deref().unwrap_or("-");
                    table.add_row(vec![
                        key.key_ref.clone(),
                        key.name.clone(),
                        format_typed_owner(&key.owner_type, owner_display),
                        output::format_bool(key.encrypted),
                        output::format_timestamp(&key.created),
                    ]);
                }

                println!("{}", table);
            }
        }
    }

    Ok(())
}

async fn handle_show(
    profile: &Option<String>,
    key_ref: String,
    decrypt: bool,
    api_url: &Option<String>,
    output_format: OutputFormat,
) -> Result<()> {
    let config = CliConfig::load_with_profile(profile.as_deref())?;
    let mut client = ApiClient::from_config(&config, api_url);

    let path = format!("/keys/{}", urlencoding::encode(&key_ref));
    let key: KeyResponse = client.get(&path).await?;

    match output_format {
        OutputFormat::Json | OutputFormat::Yaml => {
            if decrypt {
                output::print_output(&key, output_format)?;
            } else {
                // Redact value — replace with hash
                let mut redacted = serde_json::to_value(&key)?;
                if let Some(obj) = redacted.as_object_mut() {
                    obj.insert(
                        "value".to_string(),
                        JsonValue::String(hash_value_for_display(&key.value)),
                    );
                }
                output::print_output(&redacted, output_format)?;
            }
        }
        OutputFormat::Table => {
            output::print_section(&format!("Key: {}", key.key_ref));

            let mut pairs = vec![
                ("Reference", key.key_ref.clone()),
                ("Local Reference", key.local_ref.clone()),
                ("Name", key.name.clone()),
                (
                    "Owner",
                    format_typed_owner(
                        &key.owner_type,
                        &format_owner_display(
                            &key.owner_type,
                            key.owner.as_deref(),
                            key.owner_pack_ref.as_deref(),
                            key.owner_action_ref.as_deref(),
                            key.owner_sensor_ref.as_deref(),
                        ),
                    ),
                ),
            ];

            if let Some(ref pack_ref) = key.owner_pack_ref {
                pairs.push(("Owner Pack", pack_ref.clone()));
            }
            if let Some(ref action_ref) = key.owner_action_ref {
                pairs.push(("Owner Action", action_ref.clone()));
            }
            if let Some(ref sensor_ref) = key.owner_sensor_ref {
                pairs.push(("Owner Sensor", sensor_ref.clone()));
            }

            pairs.push(("Encrypted", output::format_bool(key.encrypted)));

            if decrypt {
                pairs.push(("Value", format_value_for_display(&key.value)));
            } else {
                pairs.push(("Value (SHA-256)", hash_value_for_display(&key.value)));
                pairs.push((
                    "",
                    "(use --decrypt / -d to reveal the actual value)".to_string(),
                ));
            }

            pairs.push(("Created", output::format_timestamp(&key.created)));
            pairs.push(("Updated", output::format_timestamp(&key.updated)));

            output::print_key_value_table(pairs);
        }
    }

    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn handle_create(
    profile: &Option<String>,
    local_ref: String,
    name: String,
    value: String,
    owner_type: String,
    owner_identity_login: Option<String>,
    owner_pack_ref: Option<String>,
    owner_action_ref: Option<String>,
    owner_sensor_ref: Option<String>,
    encrypted: bool,
    api_url: &Option<String>,
    output_format: OutputFormat,
) -> Result<()> {
    // Validate owner_type before sending
    validate_owner_type(&owner_type)?;

    let config = CliConfig::load_with_profile(profile.as_deref())?;
    let mut client = ApiClient::from_config(&config, api_url);

    let json_value = parse_value_as_json(&value);

    let request = CreateKeyRequestBody {
        local_ref,
        owner_type,
        owner_identity_login,
        owner_pack_ref,
        owner_action_ref,
        owner_sensor_ref,
        name,
        value: json_value,
        encrypted,
    };

    let key: KeyResponse = client.post("/keys", &request).await?;

    match output_format {
        OutputFormat::Json | OutputFormat::Yaml => {
            output::print_output(&key, output_format)?;
        }
        OutputFormat::Table => {
            output::print_success(&format!("Key '{}' created successfully", key.key_ref));
            output::print_key_value_table(vec![
                ("Reference", key.key_ref.clone()),
                ("Local Reference", key.local_ref.clone()),
                ("Name", key.name.clone()),
                (
                    "Owner",
                    format_typed_owner(
                        &key.owner_type,
                        &format_owner_display(
                            &key.owner_type,
                            key.owner.as_deref(),
                            key.owner_pack_ref.as_deref(),
                            key.owner_action_ref.as_deref(),
                            key.owner_sensor_ref.as_deref(),
                        ),
                    ),
                ),
                ("Encrypted", output::format_bool(key.encrypted)),
                ("Created", output::format_timestamp(&key.created)),
            ]);
        }
    }

    Ok(())
}

async fn handle_update(
    profile: &Option<String>,
    key_ref: String,
    name: Option<String>,
    value: Option<String>,
    encrypted: Option<bool>,
    api_url: &Option<String>,
    output_format: OutputFormat,
) -> Result<()> {
    if name.is_none() && value.is_none() && encrypted.is_none() {
        anyhow::bail!(
            "At least one field must be provided to update (--name, --value, or --encrypted)"
        );
    }

    let config = CliConfig::load_with_profile(profile.as_deref())?;
    let mut client = ApiClient::from_config(&config, api_url);

    let json_value = value.map(|v| parse_value_as_json(&v));

    let request = UpdateKeyRequestBody {
        name,
        value: json_value,
        encrypted,
    };

    let path = format!("/keys/{}", urlencoding::encode(&key_ref));
    let key: KeyResponse = client.put(&path, &request).await?;

    match output_format {
        OutputFormat::Json | OutputFormat::Yaml => {
            output::print_output(&key, output_format)?;
        }
        OutputFormat::Table => {
            output::print_success(&format!("Key '{}' updated successfully", key.key_ref));
            output::print_key_value_table(vec![
                ("Reference", key.key_ref.clone()),
                ("Name", key.name.clone()),
                (
                    "Owner",
                    format_typed_owner(
                        &key.owner_type,
                        &format_owner_display(
                            &key.owner_type,
                            key.owner.as_deref(),
                            key.owner_pack_ref.as_deref(),
                            key.owner_action_ref.as_deref(),
                            key.owner_sensor_ref.as_deref(),
                        ),
                    ),
                ),
                ("Encrypted", output::format_bool(key.encrypted)),
                ("Updated", output::format_timestamp(&key.updated)),
            ]);
        }
    }

    Ok(())
}

async fn handle_delete(
    profile: &Option<String>,
    key_ref: String,
    yes: bool,
    api_url: &Option<String>,
    output_format: OutputFormat,
) -> Result<()> {
    let config = CliConfig::load_with_profile(profile.as_deref())?;
    let mut client = ApiClient::from_config(&config, api_url);

    // Confirm deletion unless --yes is provided
    if !yes && matches!(output_format, OutputFormat::Table) {
        let confirm = dialoguer::Confirm::new()
            .with_prompt(format!(
                "Are you sure you want to delete key '{}'?",
                key_ref
            ))
            .default(false)
            .interact()?;

        if !confirm {
            output::print_info("Deletion cancelled");
            return Ok(());
        }
    }

    let path = format!("/keys/{}", urlencoding::encode(&key_ref));
    client.delete_no_response(&path).await?;

    match output_format {
        OutputFormat::Json | OutputFormat::Yaml => {
            let msg =
                serde_json::json!({"message": format!("Key '{}' deleted successfully", key_ref)});
            output::print_output(&msg, output_format)?;
        }
        OutputFormat::Table => {
            output::print_success(&format!("Key '{}' deleted successfully", key_ref));
        }
    }

    Ok(())
}

// ── Helpers ────────────────────────────────────────────────────────────────

/// Validate that the owner_type string is one of the accepted values.
fn validate_owner_type(owner_type: &str) -> Result<()> {
    const VALID: &[&str] = &["system", "identity", "pack", "action", "sensor"];
    if !VALID.contains(&owner_type) {
        anyhow::bail!(
            "Invalid owner type '{}'. Must be one of: {}",
            owner_type,
            VALID.join(", ")
        );
    }
    Ok(())
}

/// Parse a CLI string value into a [`JsonValue`].
///
/// If the input is valid JSON (object, array, number, boolean, null, or
/// quoted string), it is used as-is. Otherwise, it is treated as a plain
/// string and wrapped in a JSON string value.
fn parse_value_as_json(input: &str) -> JsonValue {
    match serde_json::from_str::<JsonValue>(input) {
        Ok(v) => v,
        Err(_) => JsonValue::String(input.to_string()),
    }
}

/// Format a [`JsonValue`] for table display.
fn format_value_for_display(value: &JsonValue) -> String {
    match value {
        JsonValue::String(s) => s.clone(),
        other => serde_json::to_string_pretty(other).unwrap_or_else(|_| other.to_string()),
    }
}

/// Compute a SHA-256 hash of the JSON value for display purposes.
///
/// This lets users verify a value matches expectations without revealing
/// the actual content (e.g., to confirm it hasn't changed).
fn hash_value_for_display(value: &JsonValue) -> String {
    let serialized = serde_json::to_string(value).unwrap_or_default();
    let mut hasher = Sha256::new();
    hasher.update(serialized.as_bytes());
    let result = hasher.finalize();
    format!(
        "sha256:{}",
        result
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    )
}

fn format_owner_display(
    owner_type: &str,
    owner: Option<&str>,
    owner_pack_ref: Option<&str>,
    owner_action_ref: Option<&str>,
    owner_sensor_ref: Option<&str>,
) -> String {
    let fallback = owner.unwrap_or("-").to_string();
    match owner_type.to_ascii_lowercase().as_str() {
        "pack" => owner_pack_ref.unwrap_or(&fallback).to_string(),
        "action" => owner_action_ref.unwrap_or(&fallback).to_string(),
        "sensor" => owner_sensor_ref.unwrap_or(&fallback).to_string(),
        _ => fallback,
    }
}

fn format_typed_owner(owner_type: &str, owner_display: &str) -> String {
    if owner_display == "-" {
        owner_type.to_string()
    } else {
        format!("{owner_type}: {owner_display}")
    }
}
