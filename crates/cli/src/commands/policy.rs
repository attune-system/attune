use anyhow::{Context, Result};
use clap::{Subcommand, ValueEnum};
use serde::{Deserialize, Serialize};

use crate::client::ApiClient;
use crate::config::CliConfig;
use crate::output::{self, OutputFormat};

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum PolicyScopeArg {
    Global,
    Pack,
    Action,
}

impl PolicyScopeArg {
    fn as_str(self) -> &'static str {
        match self {
            Self::Global => "global",
            Self::Pack => "pack",
            Self::Action => "action",
        }
    }
}

#[derive(Debug, Clone, Copy, ValueEnum, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PolicyMethodArg {
    Cancel,
    Enqueue,
}

#[derive(Subcommand)]
pub enum PolicyCommands {
    /// List execution admission policies
    List {
        /// Filter by scope
        #[arg(long)]
        scope: Option<PolicyScopeArg>,

        /// Filter by pack reference
        #[arg(long)]
        pack: Option<String>,

        /// Filter by action reference
        #[arg(long)]
        action: Option<String>,
    },
    /// Show details of a policy
    Show {
        /// Policy reference (pack.policy)
        policy_ref: String,
    },
    /// Create a policy
    Create {
        /// Policy reference (pack.policy)
        #[arg(long)]
        r#ref: String,

        /// Human-readable policy name
        #[arg(long)]
        name: String,

        /// Optional description
        #[arg(long)]
        description: Option<String>,

        /// Scope policy to a pack
        #[arg(long, conflicts_with = "action")]
        pack: Option<String>,

        /// Scope policy to an action
        #[arg(long, conflicts_with = "pack")]
        action: Option<String>,

        /// Enforcement method when the threshold is reached
        #[arg(long, value_enum, default_value = "enqueue")]
        method: PolicyMethodArg,

        /// Numeric concurrency threshold
        #[arg(long)]
        threshold: i32,

        /// Comma-separated parameter paths for grouping
        #[arg(long)]
        parameters: Option<String>,

        /// Comma-separated tags
        #[arg(long)]
        tags: Option<String>,
    },
    /// Update a policy
    Update {
        /// Policy reference (pack.policy)
        policy_ref: String,

        /// Human-readable policy name
        #[arg(long)]
        name: Option<String>,

        /// Optional description
        #[arg(long)]
        description: Option<String>,

        /// Enforcement method when the threshold is reached
        #[arg(long, value_enum)]
        method: Option<PolicyMethodArg>,

        /// Numeric concurrency threshold
        #[arg(long)]
        threshold: Option<i32>,

        /// Comma-separated parameter paths for grouping
        #[arg(long)]
        parameters: Option<String>,

        /// Comma-separated tags
        #[arg(long)]
        tags: Option<String>,
    },
    /// Delete a policy
    Delete {
        /// Policy reference (pack.policy)
        policy_ref: String,

        /// Skip confirmation prompt
        #[arg(long)]
        yes: bool,
    },
}

#[derive(Debug, Serialize, Deserialize)]
struct Policy {
    id: i64,
    #[serde(rename = "ref")]
    policy_ref: String,
    scope: String,
    pack_ref: Option<String>,
    action_ref: Option<String>,
    parameters: Vec<String>,
    method: String,
    threshold: i32,
    name: String,
    description: Option<String>,
    tags: Vec<String>,
    created: String,
    updated: String,
}

#[derive(Debug, Serialize)]
struct CreatePolicyRequest {
    #[serde(rename = "ref")]
    policy_ref: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pack_ref: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    action_ref: Option<String>,
    parameters: Vec<String>,
    method: PolicyMethodArg,
    threshold: i32,
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    tags: Vec<String>,
}

#[derive(Debug, Serialize)]
struct UpdatePolicyRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    parameters: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    method: Option<PolicyMethodArg>,
    #[serde(skip_serializing_if = "Option::is_none")]
    threshold: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tags: Option<Vec<String>>,
}

pub async fn handle_policy_command(
    profile: &Option<String>,
    command: PolicyCommands,
    api_url: &Option<String>,
    output_format: OutputFormat,
) -> Result<()> {
    match command {
        PolicyCommands::List {
            scope,
            pack,
            action,
        } => handle_list(profile, scope, pack, action, api_url, output_format).await,
        PolicyCommands::Show { policy_ref } => {
            handle_show(profile, policy_ref, api_url, output_format).await
        }
        PolicyCommands::Create {
            r#ref,
            name,
            description,
            pack,
            action,
            method,
            threshold,
            parameters,
            tags,
        } => {
            handle_create(
                profile,
                r#ref,
                name,
                description,
                pack,
                action,
                method,
                threshold,
                parameters,
                tags,
                api_url,
                output_format,
            )
            .await
        }
        PolicyCommands::Update {
            policy_ref,
            name,
            description,
            method,
            threshold,
            parameters,
            tags,
        } => {
            handle_update(
                profile,
                policy_ref,
                name,
                description,
                method,
                threshold,
                parameters,
                tags,
                api_url,
                output_format,
            )
            .await
        }
        PolicyCommands::Delete { policy_ref, yes } => {
            handle_delete(profile, policy_ref, yes, api_url, output_format).await
        }
    }
}

async fn handle_list(
    profile: &Option<String>,
    scope: Option<PolicyScopeArg>,
    pack: Option<String>,
    action: Option<String>,
    api_url: &Option<String>,
    output_format: OutputFormat,
) -> Result<()> {
    let config = CliConfig::load_with_profile(profile.as_deref())?;
    let mut client = ApiClient::from_config(&config, api_url);
    let mut params = Vec::new();
    if let Some(scope) = scope {
        params.push(("scope", scope.as_str().to_string()));
    }
    if let Some(pack) = pack {
        params.push(("pack_ref", pack));
    }
    if let Some(action) = action {
        params.push(("action_ref", action));
    }

    let path = if params.is_empty() {
        "/policies".to_string()
    } else {
        let qs = params
            .iter()
            .map(|(key, value)| format!("{}={}", key, urlencoding::encode(value)))
            .collect::<Vec<_>>()
            .join("&");
        format!("/policies?{}", qs)
    };
    let policies: Vec<Policy> = client.get_paginated(&path).await?;
    print_policy_list(&policies, output_format)
}

async fn handle_show(
    profile: &Option<String>,
    policy_ref: String,
    api_url: &Option<String>,
    output_format: OutputFormat,
) -> Result<()> {
    let config = CliConfig::load_with_profile(profile.as_deref())?;
    let mut client = ApiClient::from_config(&config, api_url);
    let policy: Policy = client.get(&format!("/policies/{}", policy_ref)).await?;
    print_policy_detail(&policy, output_format)
}

#[allow(clippy::too_many_arguments)]
async fn handle_create(
    profile: &Option<String>,
    policy_ref: String,
    name: String,
    description: Option<String>,
    pack: Option<String>,
    action: Option<String>,
    method: PolicyMethodArg,
    threshold: i32,
    parameters: Option<String>,
    tags: Option<String>,
    api_url: &Option<String>,
    output_format: OutputFormat,
) -> Result<()> {
    let config = CliConfig::load_with_profile(profile.as_deref())?;
    let mut client = ApiClient::from_config(&config, api_url);
    let request = CreatePolicyRequest {
        policy_ref,
        pack_ref: pack,
        action_ref: action,
        parameters: parse_csv(parameters),
        method,
        threshold,
        name,
        description,
        tags: parse_csv(tags),
    };
    let policy: Policy = client.post("/policies", &request).await?;

    if matches!(output_format, OutputFormat::Table) {
        output::print_success(&format!(
            "Policy '{}' created successfully",
            policy.policy_ref
        ));
    }
    print_policy_detail(&policy, output_format)
}

#[allow(clippy::too_many_arguments)]
async fn handle_update(
    profile: &Option<String>,
    policy_ref: String,
    name: Option<String>,
    description: Option<String>,
    method: Option<PolicyMethodArg>,
    threshold: Option<i32>,
    parameters: Option<String>,
    tags: Option<String>,
    api_url: &Option<String>,
    output_format: OutputFormat,
) -> Result<()> {
    if name.is_none()
        && description.is_none()
        && method.is_none()
        && threshold.is_none()
        && parameters.is_none()
        && tags.is_none()
    {
        anyhow::bail!("At least one field must be provided to update");
    }

    let config = CliConfig::load_with_profile(profile.as_deref())?;
    let mut client = ApiClient::from_config(&config, api_url);
    let request = UpdatePolicyRequest {
        parameters: parameters.map(|value| parse_csv(Some(value))),
        method,
        threshold,
        name,
        description,
        tags: tags.map(|value| parse_csv(Some(value))),
    };
    let policy: Policy = client
        .put(&format!("/policies/{}", policy_ref), &request)
        .await?;

    if matches!(output_format, OutputFormat::Table) {
        output::print_success(&format!(
            "Policy '{}' updated successfully",
            policy.policy_ref
        ));
    }
    print_policy_detail(&policy, output_format)
}

async fn handle_delete(
    profile: &Option<String>,
    policy_ref: String,
    yes: bool,
    api_url: &Option<String>,
    output_format: OutputFormat,
) -> Result<()> {
    let config = CliConfig::load_with_profile(profile.as_deref())?;
    let mut client = ApiClient::from_config(&config, api_url);

    if !yes && matches!(output_format, OutputFormat::Table) {
        let confirm = dialoguer::Confirm::new()
            .with_prompt(format!(
                "Are you sure you want to delete policy '{}'?",
                policy_ref
            ))
            .default(false)
            .interact()
            .context("Failed to read confirmation")?;
        if !confirm {
            output::print_info("Delete cancelled");
            return Ok(());
        }
    }

    client
        .delete_no_response(&format!("/policies/{}", policy_ref))
        .await?;
    match output_format {
        OutputFormat::Json | OutputFormat::Yaml => {
            output::print_output(
                &serde_json::json!({ "message": "Policy deleted successfully" }),
                output_format,
            )?;
        }
        OutputFormat::Table => {
            output::print_success(&format!("Policy '{}' deleted successfully", policy_ref));
        }
    }
    Ok(())
}

fn parse_csv(value: Option<String>) -> Vec<String> {
    value
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(ToString::to_string)
        .collect()
}

fn print_policy_list(policies: &Vec<Policy>, output_format: OutputFormat) -> Result<()> {
    match output_format {
        OutputFormat::Json | OutputFormat::Yaml => output::print_output(policies, output_format),
        OutputFormat::Table => {
            if policies.is_empty() {
                output::print_info("No policies found");
                return Ok(());
            }
            let mut table = output::create_table();
            output::add_header(
                &mut table,
                vec![
                    "Ref",
                    "Scope",
                    "Target",
                    "Method",
                    "Threshold",
                    "Groups",
                    "Name",
                ],
            );
            for policy in policies {
                table.add_row(vec![
                    policy.policy_ref.clone(),
                    policy.scope.clone(),
                    policy_target(policy),
                    policy.method.clone(),
                    policy.threshold.to_string(),
                    policy.parameters.join(","),
                    policy.name.clone(),
                ]);
            }
            println!("{}", table);
            Ok(())
        }
    }
}

fn print_policy_detail(policy: &Policy, output_format: OutputFormat) -> Result<()> {
    match output_format {
        OutputFormat::Json | OutputFormat::Yaml => output::print_output(policy, output_format),
        OutputFormat::Table => {
            output::print_section(&format!("Policy: {}", policy.policy_ref));
            output::print_key_value_table(vec![
                ("Reference", policy.policy_ref.clone()),
                ("Scope", policy.scope.clone()),
                ("Target", policy_target(policy)),
                ("Method", policy.method.clone()),
                ("Threshold", policy.threshold.to_string()),
                ("Parameters", policy.parameters.join(",")),
                ("Name", policy.name.clone()),
                (
                    "Description",
                    policy
                        .description
                        .clone()
                        .unwrap_or_else(|| "None".to_string()),
                ),
                ("Tags", policy.tags.join(",")),
                ("Created", output::format_timestamp(&policy.created)),
                ("Updated", output::format_timestamp(&policy.updated)),
            ]);
            Ok(())
        }
    }
}

fn policy_target(policy: &Policy) -> String {
    policy
        .action_ref
        .clone()
        .or_else(|| policy.pack_ref.clone())
        .unwrap_or_else(|| "global".to_string())
}
