use anyhow::{bail, Context, Result};
use clap::{Args, Subcommand, ValueEnum};
use comfy_table::Cell;
use serde::{Deserialize, Serialize};

use crate::client::ApiClient;
use crate::config::CliConfig;
use crate::output::{self, OutputFormat};

#[derive(Subcommand)]
pub enum PolicyCommands {
    /// List policies
    List(PolicyListArgs),
    /// Show a policy
    Show {
        /// Policy reference
        policy_ref: String,
    },
    /// Create a policy
    Create(PolicyWriteArgs),
    /// Update a policy
    Update {
        /// Policy reference
        policy_ref: String,
        #[command(flatten)]
        args: PolicyUpdateArgs,
    },
    /// Enable a policy
    Enable {
        /// Policy reference
        policy_ref: String,
    },
    /// Disable a policy
    Disable {
        /// Policy reference
        policy_ref: String,
    },
    /// Delete a policy
    Delete {
        /// Policy reference
        policy_ref: String,
        /// Skip confirmation prompt
        #[arg(long)]
        yes: bool,
    },
}

#[derive(Debug, Args)]
pub struct PolicyListArgs {
    /// Filter by scope
    #[arg(long, value_enum)]
    scope: Option<PolicyScopeArg>,
    /// Filter by pack ref
    #[arg(long)]
    pack: Option<String>,
    /// Filter by action ref
    #[arg(long)]
    action: Option<String>,
    /// Filter by enabled status
    #[arg(long)]
    enabled: Option<bool>,
    /// Filter by tag
    #[arg(long)]
    tag: Option<String>,
}

#[derive(Debug, Args)]
pub struct PolicyWriteArgs {
    /// Policy reference
    #[arg(long)]
    policy_ref: String,
    /// Policy name
    #[arg(long)]
    name: String,
    /// Policy description
    #[arg(long)]
    description: Option<String>,
    /// Create the policy disabled
    #[arg(long)]
    disabled: bool,
    /// Same-scope precedence; higher wins
    #[arg(long, default_value_t = 0)]
    priority: i32,
    /// Scope where the policy applies
    #[arg(long, value_enum)]
    scope: PolicyScopeArg,
    /// Pack ref for pack scope, or optional pack context for action scope
    #[arg(long)]
    pack: Option<String>,
    /// Action ref for action scope
    #[arg(long)]
    action: Option<String>,
    /// Concurrency limit
    #[arg(long)]
    concurrency_limit: Option<i32>,
    /// Behavior when concurrency limit is reached
    #[arg(long, value_enum, default_value_t = PolicyMethodArg::Enqueue)]
    on_concurrency: PolicyMethodArg,
    /// Parameter path to group concurrency by; repeatable
    #[arg(long = "group-by")]
    group_by: Vec<String>,
    /// Maximum executions in the rate-limit window
    #[arg(long)]
    rate_limit_max: Option<i32>,
    /// Rate-limit window, e.g. 60s, 10m, 1h
    #[arg(long)]
    rate_limit_window: Option<String>,
    /// Running-executions quota limit
    #[arg(long)]
    quota_running_executions: Option<u64>,
    /// Total-executions quota limit
    #[arg(long)]
    quota_executions_total: Option<u64>,
    /// Tag to attach; repeatable
    #[arg(long)]
    tag: Vec<String>,
}

#[derive(Debug, Args)]
pub struct PolicyUpdateArgs {
    /// Policy name
    #[arg(long)]
    name: Option<String>,
    /// Policy description; empty string clears it
    #[arg(long)]
    description: Option<String>,
    /// Enabled state
    #[arg(long)]
    enabled: Option<bool>,
    /// Same-scope precedence; higher wins
    #[arg(long)]
    priority: Option<i32>,
    /// Replace concurrency limit; omit with --clear-concurrency to clear
    #[arg(long, conflicts_with = "clear_concurrency")]
    concurrency_limit: Option<i32>,
    /// Behavior when concurrency limit is reached
    #[arg(long, value_enum, default_value_t = PolicyMethodArg::Enqueue)]
    on_concurrency: PolicyMethodArg,
    /// Parameter path to group concurrency by; repeatable
    #[arg(long = "group-by")]
    group_by: Vec<String>,
    /// Clear concurrency settings
    #[arg(long)]
    clear_concurrency: bool,
    /// Replace rate-limit max; requires --rate-limit-window
    #[arg(
        long,
        requires = "rate_limit_window",
        conflicts_with = "clear_rate_limit"
    )]
    rate_limit_max: Option<i32>,
    /// Replace rate-limit window, e.g. 60s, 10m, 1h
    #[arg(long, requires = "rate_limit_max", conflicts_with = "clear_rate_limit")]
    rate_limit_window: Option<String>,
    /// Clear rate-limit settings
    #[arg(long)]
    clear_rate_limit: bool,
    /// Running-executions quota limit
    #[arg(long)]
    quota_running_executions: Option<u64>,
    /// Total-executions quota limit
    #[arg(long)]
    quota_executions_total: Option<u64>,
    /// Replace quotas with none
    #[arg(long, conflicts_with_all = ["quota_running_executions", "quota_executions_total"])]
    clear_quotas: bool,
    /// Replace tags; repeatable
    #[arg(long)]
    tag: Vec<String>,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum PolicyScopeArg {
    Global,
    Pack,
    Action,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum PolicyMethodArg {
    Cancel,
    Enqueue,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum PolicyScopeType {
    Global,
    Pack,
    Action,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum PolicyMethod {
    Cancel,
    Enqueue,
}

#[derive(Debug, Serialize, Deserialize)]
struct PolicyScopeRequest {
    #[serde(rename = "type")]
    scope_type: PolicyScopeType,
    #[serde(skip_serializing_if = "Option::is_none")]
    pack_ref: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    action_ref: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct PolicyScopeResponse {
    #[serde(rename = "type")]
    scope_type: PolicyScopeType,
    #[serde(default)]
    pack_ref: Option<String>,
    #[serde(default)]
    action_ref: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct ConcurrencyPolicy {
    limit: i32,
    method: PolicyMethod,
    #[serde(default)]
    parameters: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize)]
struct RateLimitPolicy {
    max_executions: i32,
    window_seconds: i32,
}

#[derive(Debug, Serialize, Deserialize)]
struct QuotaPolicy {
    quota_type: String,
    limit: u64,
}

#[derive(Debug, Serialize, Deserialize)]
struct Policy {
    id: i64,
    #[serde(rename = "ref")]
    policy_ref: String,
    name: String,
    #[serde(default)]
    description: Option<String>,
    enabled: bool,
    priority: i32,
    scope: PolicyScopeResponse,
    #[serde(default)]
    concurrency: Option<ConcurrencyPolicy>,
    #[serde(default)]
    rate_limit: Option<RateLimitPolicy>,
    #[serde(default)]
    quotas: Vec<QuotaPolicy>,
    #[serde(default)]
    tags: Vec<String>,
    created: String,
    updated: String,
}

#[derive(Debug, Serialize)]
struct CreatePolicyRequest {
    #[serde(rename = "ref")]
    policy_ref: String,
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    enabled: bool,
    priority: i32,
    scope: PolicyScopeRequest,
    #[serde(skip_serializing_if = "Option::is_none")]
    concurrency: Option<ConcurrencyPolicy>,
    #[serde(skip_serializing_if = "Option::is_none")]
    rate_limit: Option<RateLimitPolicy>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    quotas: Vec<QuotaPolicy>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    tags: Vec<String>,
}

#[derive(Debug, Default, Serialize)]
struct UpdatePolicyRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<Option<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    enabled: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    priority: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    concurrency: Option<Option<ConcurrencyPolicy>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    rate_limit: Option<Option<RateLimitPolicy>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    quotas: Option<Vec<QuotaPolicy>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tags: Option<Vec<String>>,
}

impl From<PolicyScopeArg> for PolicyScopeType {
    fn from(value: PolicyScopeArg) -> Self {
        match value {
            PolicyScopeArg::Global => Self::Global,
            PolicyScopeArg::Pack => Self::Pack,
            PolicyScopeArg::Action => Self::Action,
        }
    }
}

impl From<PolicyMethodArg> for PolicyMethod {
    fn from(value: PolicyMethodArg) -> Self {
        match value {
            PolicyMethodArg::Cancel => Self::Cancel,
            PolicyMethodArg::Enqueue => Self::Enqueue,
        }
    }
}

pub async fn handle_policy_command(
    profile: &Option<String>,
    command: PolicyCommands,
    api_url: &Option<String>,
    output_format: OutputFormat,
) -> Result<()> {
    match command {
        PolicyCommands::List(args) => handle_list(profile, args, api_url, output_format).await,
        PolicyCommands::Show { policy_ref } => {
            handle_show(profile, policy_ref, api_url, output_format).await
        }
        PolicyCommands::Create(args) => handle_create(profile, args, api_url, output_format).await,
        PolicyCommands::Update { policy_ref, args } => {
            handle_update(profile, policy_ref, args, api_url, output_format).await
        }
        PolicyCommands::Enable { policy_ref } => {
            handle_toggle(profile, policy_ref, true, api_url, output_format).await
        }
        PolicyCommands::Disable { policy_ref } => {
            handle_toggle(profile, policy_ref, false, api_url, output_format).await
        }
        PolicyCommands::Delete { policy_ref, yes } => {
            handle_delete(profile, policy_ref, yes, api_url, output_format).await
        }
    }
}

async fn handle_list(
    profile: &Option<String>,
    args: PolicyListArgs,
    api_url: &Option<String>,
    output_format: OutputFormat,
) -> Result<()> {
    let config = CliConfig::load_with_profile(profile.as_deref())?;
    let mut client = ApiClient::from_config(&config, api_url);
    let mut query = Vec::new();
    if let Some(scope) = args.scope {
        query.push(format!("scope={}", scope_query(scope)));
    }
    if let Some(pack) = args.pack {
        query.push(format!("pack_ref={}", pack));
    }
    if let Some(action) = args.action {
        query.push(format!("action_ref={}", action));
    }
    if let Some(enabled) = args.enabled {
        query.push(format!("enabled={enabled}"));
    }
    if let Some(tag) = args.tag {
        query.push(format!("tag={tag}"));
    }
    let path = if query.is_empty() {
        "/policies".to_string()
    } else {
        format!("/policies?{}", query.join("&"))
    };
    let policies: Vec<Policy> = client.get_paginated(&path).await?;
    print_policies(&policies, output_format)
}

async fn handle_show(
    profile: &Option<String>,
    policy_ref: String,
    api_url: &Option<String>,
    output_format: OutputFormat,
) -> Result<()> {
    let config = CliConfig::load_with_profile(profile.as_deref())?;
    let mut client = ApiClient::from_config(&config, api_url);
    let policy: Policy = client.get(&format!("/policies/{policy_ref}")).await?;
    print_policy(&policy, output_format, None)
}

async fn handle_create(
    profile: &Option<String>,
    args: PolicyWriteArgs,
    api_url: &Option<String>,
    output_format: OutputFormat,
) -> Result<()> {
    let config = CliConfig::load_with_profile(profile.as_deref())?;
    let mut client = ApiClient::from_config(&config, api_url);
    let request = CreatePolicyRequest {
        policy_ref: args.policy_ref,
        name: args.name,
        description: args.description,
        enabled: !args.disabled,
        priority: args.priority,
        scope: build_scope(args.scope, args.pack, args.action)?,
        concurrency: build_concurrency(args.concurrency_limit, args.on_concurrency, args.group_by)?,
        rate_limit: build_rate_limit(args.rate_limit_max, args.rate_limit_window)?,
        quotas: build_quotas(args.quota_running_executions, args.quota_executions_total),
        tags: args.tag,
    };
    if request.concurrency.is_none() && request.rate_limit.is_none() && request.quotas.is_empty() {
        bail!("At least one policy feature must be configured");
    }
    let policy: Policy = client.post("/policies", &request).await?;
    print_policy(&policy, output_format, Some("created"))
}

async fn handle_update(
    profile: &Option<String>,
    policy_ref: String,
    args: PolicyUpdateArgs,
    api_url: &Option<String>,
    output_format: OutputFormat,
) -> Result<()> {
    let config = CliConfig::load_with_profile(profile.as_deref())?;
    let mut client = ApiClient::from_config(&config, api_url);
    let mut request = UpdatePolicyRequest {
        name: args.name,
        description: args.description.map(
            |value| {
                if value.is_empty() {
                    None
                } else {
                    Some(value)
                }
            },
        ),
        enabled: args.enabled,
        priority: args.priority,
        ..Default::default()
    };

    if args.clear_concurrency {
        request.concurrency = Some(None);
    } else if args.concurrency_limit.is_some() {
        request.concurrency = Some(build_concurrency(
            args.concurrency_limit,
            args.on_concurrency,
            args.group_by,
        )?);
    }

    if args.clear_rate_limit {
        request.rate_limit = Some(None);
    } else if args.rate_limit_max.is_some() || args.rate_limit_window.is_some() {
        request.rate_limit = Some(build_rate_limit(
            args.rate_limit_max,
            args.rate_limit_window,
        )?);
    }

    if args.clear_quotas {
        request.quotas = Some(Vec::new());
    } else if args.quota_running_executions.is_some() || args.quota_executions_total.is_some() {
        request.quotas = Some(build_quotas(
            args.quota_running_executions,
            args.quota_executions_total,
        ));
    }

    if !args.tag.is_empty() {
        request.tags = Some(args.tag);
    }

    let policy: Policy = client
        .put(&format!("/policies/{policy_ref}"), &request)
        .await?;
    print_policy(&policy, output_format, Some("updated"))
}

async fn handle_toggle(
    profile: &Option<String>,
    policy_ref: String,
    enabled: bool,
    api_url: &Option<String>,
    output_format: OutputFormat,
) -> Result<()> {
    let config = CliConfig::load_with_profile(profile.as_deref())?;
    let mut client = ApiClient::from_config(&config, api_url);
    let request = UpdatePolicyRequest {
        enabled: Some(enabled),
        ..Default::default()
    };
    let policy: Policy = client
        .put(&format!("/policies/{policy_ref}"), &request)
        .await?;
    print_policy(
        &policy,
        output_format,
        Some(if enabled { "enabled" } else { "disabled" }),
    )
}

async fn handle_delete(
    profile: &Option<String>,
    policy_ref: String,
    yes: bool,
    api_url: &Option<String>,
    output_format: OutputFormat,
) -> Result<()> {
    if !yes && output_format == OutputFormat::Table {
        output::print_warning("Use --yes to confirm deletion");
        bail!("Deletion not confirmed");
    }
    let config = CliConfig::load_with_profile(profile.as_deref())?;
    let mut client = ApiClient::from_config(&config, api_url);
    let _: serde_json::Value = client.delete(&format!("/policies/{policy_ref}")).await?;
    if output_format == OutputFormat::Table {
        output::print_success(&format!("Policy {policy_ref} deleted"));
    } else {
        output::print_output(&serde_json::json!({ "deleted": policy_ref }), output_format)?;
    }
    Ok(())
}

fn build_scope(
    scope: PolicyScopeArg,
    pack: Option<String>,
    action: Option<String>,
) -> Result<PolicyScopeRequest> {
    match scope {
        PolicyScopeArg::Global => Ok(PolicyScopeRequest {
            scope_type: PolicyScopeType::Global,
            pack_ref: None,
            action_ref: None,
        }),
        PolicyScopeArg::Pack => Ok(PolicyScopeRequest {
            scope_type: PolicyScopeType::Pack,
            pack_ref: Some(pack.context("--pack is required for pack scope")?),
            action_ref: None,
        }),
        PolicyScopeArg::Action => Ok(PolicyScopeRequest {
            scope_type: PolicyScopeType::Action,
            pack_ref: pack,
            action_ref: Some(action.context("--action is required for action scope")?),
        }),
    }
}

fn build_concurrency(
    limit: Option<i32>,
    method: PolicyMethodArg,
    parameters: Vec<String>,
) -> Result<Option<ConcurrencyPolicy>> {
    match limit {
        Some(limit) if limit > 0 => Ok(Some(ConcurrencyPolicy {
            limit,
            method: method.into(),
            parameters,
        })),
        Some(_) => bail!("--concurrency-limit must be greater than zero"),
        None => Ok(None),
    }
}

fn build_rate_limit(max: Option<i32>, window: Option<String>) -> Result<Option<RateLimitPolicy>> {
    match (max, window) {
        (Some(max_executions), Some(window)) if max_executions > 0 => Ok(Some(RateLimitPolicy {
            max_executions,
            window_seconds: parse_duration_seconds(&window)?,
        })),
        (Some(_), Some(_)) => bail!("--rate-limit-max must be greater than zero"),
        (None, None) => Ok(None),
        _ => bail!("--rate-limit-max and --rate-limit-window must be provided together"),
    }
}

fn build_quotas(running: Option<u64>, total: Option<u64>) -> Vec<QuotaPolicy> {
    let mut quotas = Vec::new();
    if let Some(limit) = running {
        quotas.push(QuotaPolicy {
            quota_type: "running_executions".to_string(),
            limit,
        });
    }
    if let Some(limit) = total {
        quotas.push(QuotaPolicy {
            quota_type: "executions_total".to_string(),
            limit,
        });
    }
    quotas
}

fn parse_duration_seconds(raw: &str) -> Result<i32> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        bail!("duration cannot be empty");
    }
    let (number, multiplier) = match trimmed.chars().last().unwrap_or_default() {
        's' | 'S' => (&trimmed[..trimmed.len() - 1], 1),
        'm' | 'M' => (&trimmed[..trimmed.len() - 1], 60),
        'h' | 'H' => (&trimmed[..trimmed.len() - 1], 3600),
        _ => (trimmed, 1),
    };
    let value: i32 = number
        .parse()
        .with_context(|| format!("Invalid duration value '{raw}'"))?;
    if value <= 0 {
        bail!("duration must be greater than zero");
    }
    value
        .checked_mul(multiplier)
        .context("duration is too large to fit in seconds")
}

fn scope_query(scope: PolicyScopeArg) -> &'static str {
    match scope {
        PolicyScopeArg::Global => "global",
        PolicyScopeArg::Pack => "pack",
        PolicyScopeArg::Action => "action",
    }
}

fn scope_display(scope: &PolicyScopeResponse) -> String {
    match scope.scope_type {
        PolicyScopeType::Global => "global".to_string(),
        PolicyScopeType::Pack => format!("pack:{}", scope.pack_ref.as_deref().unwrap_or("-")),
        PolicyScopeType::Action => {
            format!("action:{}", scope.action_ref.as_deref().unwrap_or("-"))
        }
    }
}

fn feature_display(policy: &Policy) -> String {
    let mut features = Vec::new();
    if let Some(concurrency) = &policy.concurrency {
        features.push(format!(
            "concurrency {}/{}",
            concurrency.limit,
            method_display(&concurrency.method)
        ));
    }
    if let Some(rate_limit) = &policy.rate_limit {
        features.push(format!(
            "rate {}/{}s",
            rate_limit.max_executions, rate_limit.window_seconds
        ));
    }
    if !policy.quotas.is_empty() {
        features.push(format!("{} quotas", policy.quotas.len()));
    }
    if features.is_empty() {
        "none".to_string()
    } else {
        features.join(", ")
    }
}

fn method_display(method: &PolicyMethod) -> &'static str {
    match method {
        PolicyMethod::Cancel => "cancel",
        PolicyMethod::Enqueue => "enqueue",
    }
}

fn print_policies(policies: &[Policy], output_format: OutputFormat) -> Result<()> {
    match output_format {
        OutputFormat::Json | OutputFormat::Yaml => output::print_output(&policies, output_format),
        OutputFormat::Table => {
            let mut table = output::create_table();
            output::add_header(
                &mut table,
                vec!["Ref", "Name", "Scope", "Features", "Priority", "Enabled"],
            );
            for policy in policies {
                table.add_row(vec![
                    Cell::new(&policy.policy_ref),
                    Cell::new(&policy.name),
                    Cell::new(scope_display(&policy.scope)),
                    Cell::new(feature_display(policy)),
                    Cell::new(policy.priority),
                    Cell::new(output::format_bool(policy.enabled)),
                ]);
            }
            println!("{table}");
            Ok(())
        }
    }
}

fn print_policy(
    policy: &Policy,
    output_format: OutputFormat,
    status_message: Option<&str>,
) -> Result<()> {
    match output_format {
        OutputFormat::Json | OutputFormat::Yaml => output::print_output(policy, output_format)?,
        OutputFormat::Table => {
            if let Some(status) = status_message {
                output::print_success(&format!("Policy {} {}", policy.policy_ref, status));
            }
            output::print_key_value_table(vec![
                ("Ref", policy.policy_ref.clone()),
                ("Name", policy.name.clone()),
                (
                    "Description",
                    policy
                        .description
                        .clone()
                        .unwrap_or_else(|| "-".to_string()),
                ),
                ("Enabled", output::format_bool(policy.enabled)),
                ("Scope", scope_display(&policy.scope)),
                ("Priority", policy.priority.to_string()),
                ("Features", feature_display(policy)),
                (
                    "Tags",
                    if policy.tags.is_empty() {
                        "-".to_string()
                    } else {
                        policy.tags.join(", ")
                    },
                ),
            ]);

            if let Some(concurrency) = &policy.concurrency {
                output::print_section("Concurrency");
                output::print_key_value_table(vec![
                    ("Limit", concurrency.limit.to_string()),
                    ("Behavior", method_display(&concurrency.method).to_string()),
                    (
                        "Group By",
                        if concurrency.parameters.is_empty() {
                            "-".to_string()
                        } else {
                            concurrency.parameters.join(", ")
                        },
                    ),
                ]);
            }

            if let Some(rate_limit) = &policy.rate_limit {
                output::print_section("Rate Limit");
                output::print_key_value_table(vec![
                    ("Max Executions", rate_limit.max_executions.to_string()),
                    ("Window Seconds", rate_limit.window_seconds.to_string()),
                ]);
            }

            if !policy.quotas.is_empty() {
                output::print_section("Quotas");
                let mut table = output::create_table();
                output::add_header(&mut table, vec!["Type", "Limit"]);
                for quota in &policy.quotas {
                    table.add_row(vec![Cell::new(&quota.quota_type), Cell::new(quota.limit)]);
                }
                println!("{table}");
            }
        }
    }
    Ok(())
}
