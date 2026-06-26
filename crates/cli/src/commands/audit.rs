use anyhow::Result;
use clap::{Args, Subcommand};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

use crate::client::ApiClient;
use crate::config::CliConfig;
use crate::output::{self, OutputFormat};

#[derive(Subcommand)]
pub enum AuditCommands {
    /// List audit events with optional filters
    List {
        #[command(flatten)]
        args: Box<AuditListArgs>,
    },

    /// Show a single audit event by id
    Show {
        /// Audit event id
        id: i64,
    },

    /// Show all audit events sharing a request id
    Chain {
        /// Request id (UUID)
        request_id: String,
    },
}

#[derive(Args)]
pub struct AuditListArgs {
    /// Category (api, auth, rbac, secret, admin, execution, pack)
    #[arg(long)]
    category: Option<String>,

    /// Exact event_type match (e.g., "auth.login.success")
    #[arg(long)]
    event_type: Option<String>,

    /// Outcome (success, failure, denied)
    #[arg(long)]
    outcome: Option<String>,

    /// Substring match against actor login
    #[arg(long)]
    actor_login: Option<String>,

    /// Actor identity numeric id
    #[arg(long)]
    actor_identity: Option<i64>,

    /// Resource type filter (e.g., "key", "execution")
    #[arg(long)]
    resource_type: Option<String>,

    /// Resource numeric id
    #[arg(long)]
    resource_id: Option<i64>,

    /// Resource ref filter (snapshot at event time)
    #[arg(long)]
    resource_ref: Option<String>,

    /// HTTP method filter (GET, POST, ...)
    #[arg(long)]
    http_method: Option<String>,

    /// HTTP status code filter (e.g., 401)
    #[arg(long)]
    http_status: Option<i32>,

    /// Substring match on http_path
    #[arg(long)]
    http_path: Option<String>,

    /// Request id (UUID) filter
    #[arg(long)]
    request_id: Option<String>,

    /// ISO-8601 lower bound on `created`
    #[arg(long)]
    after: Option<String>,

    /// ISO-8601 upper bound on `created`
    #[arg(long)]
    before: Option<String>,

    /// Page number (default 1)
    #[arg(long, default_value = "1")]
    page: u32,

    /// Items per page (default 50)
    #[arg(long, default_value = "50")]
    per_page: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct AuditEventSummary {
    id: i64,
    created: String,
    category: String,
    event_type: String,
    outcome: String,
    actor_login: Option<String>,
    actor_ip: Option<String>,
    resource_type: Option<String>,
    resource_ref: Option<String>,
    http_method: Option<String>,
    http_path: Option<String>,
    http_status: Option<i32>,
    request_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct AuditEventResponse {
    id: i64,
    created: String,
    category: String,
    event_type: String,
    outcome: String,
    actor_identity: Option<i64>,
    actor_login: Option<String>,
    actor_token_type: Option<String>,
    actor_ip: Option<String>,
    actor_user_agent: Option<String>,
    resource_type: Option<String>,
    resource_id: Option<i64>,
    resource_ref: Option<String>,
    pack_ref: Option<String>,
    http_method: Option<String>,
    http_path: Option<String>,
    http_status: Option<i32>,
    request_id: Option<String>,
    parent_event_id: Option<i64>,
    details: Option<JsonValue>,
    error_message: Option<String>,
}

pub async fn handle_audit_command(
    profile: &Option<String>,
    command: AuditCommands,
    api_url: &Option<String>,
    output_format: OutputFormat,
) -> Result<()> {
    match command {
        AuditCommands::List { args } => handle_list(profile, *args, api_url, output_format).await,
        AuditCommands::Show { id } => handle_show(profile, id, api_url, output_format).await,
        AuditCommands::Chain { request_id } => {
            handle_chain(profile, request_id, api_url, output_format).await
        }
    }
}

async fn handle_list(
    profile: &Option<String>,
    args: AuditListArgs,
    api_url: &Option<String>,
    output_format: OutputFormat,
) -> Result<()> {
    let AuditListArgs {
        category,
        event_type,
        outcome,
        actor_login,
        actor_identity,
        resource_type,
        resource_id,
        resource_ref,
        http_method,
        http_status,
        http_path,
        request_id,
        after,
        before,
        page,
        per_page,
    } = args;

    let config = CliConfig::load_with_profile(profile.as_deref())?;
    let mut client = ApiClient::from_config(&config, api_url);

    let mut q = vec![format!("page={}", page), format!("per_page={}", per_page)];
    let mut push = |k: &str, v: Option<String>| {
        if let Some(v) = v {
            q.push(format!("{}={}", k, urlencoding::encode(&v)));
        }
    };
    push("category", category);
    push("event_type", event_type);
    push("outcome", outcome);
    push("actor_login", actor_login);
    push("resource_type", resource_type);
    push("resource_ref", resource_ref);
    push("http_method", http_method);
    push("http_path", http_path);
    push("request_id", request_id);
    push("created_after", after);
    push("created_before", before);
    if let Some(v) = actor_identity {
        q.push(format!("actor_identity={}", v));
    }
    if let Some(v) = resource_id {
        q.push(format!("resource_id={}", v));
    }
    if let Some(v) = http_status {
        q.push(format!("http_status={}", v));
    }

    let path = format!("/audit-events?{}", q.join("&"));
    let events: Vec<AuditEventSummary> = client.get_paginated(&path).await?;

    match output_format {
        OutputFormat::Json | OutputFormat::Yaml => {
            output::print_output(&events, output_format)?;
        }
        OutputFormat::Table => {
            if events.is_empty() {
                output::print_info("No audit events found");
            } else {
                let mut table = output::create_table();
                output::add_header(
                    &mut table,
                    vec![
                        "ID",
                        "Created",
                        "Category",
                        "Event Type",
                        "Outcome",
                        "Actor",
                        "Resource",
                        "HTTP",
                    ],
                );

                for e in events {
                    let actor = e.actor_login.clone().unwrap_or_else(|| "-".into());
                    let resource = match (&e.resource_type, &e.resource_ref) {
                        (Some(t), Some(r)) => format!("{}:{}", t, r),
                        (Some(t), None) => t.clone(),
                        _ => "-".into(),
                    };
                    let http = match (&e.http_method, &e.http_status) {
                        (Some(m), Some(s)) => format!("{} {}", m, s),
                        (Some(m), None) => m.clone(),
                        _ => "-".into(),
                    };
                    table.add_row(vec![
                        e.id.to_string(),
                        output::format_timestamp(&e.created),
                        e.category,
                        e.event_type,
                        e.outcome,
                        actor,
                        resource,
                        http,
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
    id: i64,
    api_url: &Option<String>,
    output_format: OutputFormat,
) -> Result<()> {
    let config = CliConfig::load_with_profile(profile.as_deref())?;
    let mut client = ApiClient::from_config(&config, api_url);

    let path = format!("/audit-events/{}", id);
    let event: AuditEventResponse = client.get(&path).await?;

    print_event(&event, output_format)
}

async fn handle_chain(
    profile: &Option<String>,
    request_id: String,
    api_url: &Option<String>,
    output_format: OutputFormat,
) -> Result<()> {
    let config = CliConfig::load_with_profile(profile.as_deref())?;
    let mut client = ApiClient::from_config(&config, api_url);

    let path = format!(
        "/audit-events/by-request/{}",
        urlencoding::encode(&request_id)
    );
    let events: Vec<AuditEventResponse> = client.get(&path).await?;

    match output_format {
        OutputFormat::Json | OutputFormat::Yaml => {
            output::print_output(&events, output_format)?;
        }
        OutputFormat::Table => {
            if events.is_empty() {
                output::print_info("No audit events found for that request id");
            } else {
                output::print_section(&format!(
                    "Audit chain for request {} ({} events)",
                    request_id,
                    events.len()
                ));
                let mut table = output::create_table();
                output::add_header(
                    &mut table,
                    vec![
                        "ID",
                        "Created",
                        "Category",
                        "Event Type",
                        "Outcome",
                        "Actor",
                        "Path",
                    ],
                );
                for e in events {
                    table.add_row(vec![
                        e.id.to_string(),
                        output::format_timestamp(&e.created),
                        e.category,
                        e.event_type,
                        e.outcome,
                        e.actor_login.unwrap_or_else(|| "-".into()),
                        e.http_path.unwrap_or_else(|| "-".into()),
                    ]);
                }
                println!("{}", table);
            }
        }
    }

    Ok(())
}

fn print_event(event: &AuditEventResponse, output_format: OutputFormat) -> Result<()> {
    match output_format {
        OutputFormat::Json | OutputFormat::Yaml => {
            output::print_output(event, output_format)?;
        }
        OutputFormat::Table => {
            output::print_section(&format!("Audit Event #{}", event.id));
            let pairs: Vec<(&str, String)> = vec![
                ("ID", event.id.to_string()),
                ("Created", output::format_timestamp(&event.created)),
                ("Category", event.category.clone()),
                ("Event Type", event.event_type.clone()),
                ("Outcome", event.outcome.clone()),
                (
                    "Actor Identity",
                    event
                        .actor_identity
                        .map(|v| v.to_string())
                        .unwrap_or_else(|| "-".into()),
                ),
                (
                    "Actor Login",
                    event.actor_login.clone().unwrap_or_else(|| "-".into()),
                ),
                (
                    "Token Type",
                    event.actor_token_type.clone().unwrap_or_else(|| "-".into()),
                ),
                (
                    "Actor IP",
                    event.actor_ip.clone().unwrap_or_else(|| "-".into()),
                ),
                (
                    "User Agent",
                    event.actor_user_agent.clone().unwrap_or_else(|| "-".into()),
                ),
                (
                    "Resource Type",
                    event.resource_type.clone().unwrap_or_else(|| "-".into()),
                ),
                (
                    "Resource ID",
                    event
                        .resource_id
                        .map(|v| v.to_string())
                        .unwrap_or_else(|| "-".into()),
                ),
                (
                    "Resource Ref",
                    event.resource_ref.clone().unwrap_or_else(|| "-".into()),
                ),
                (
                    "Pack Ref",
                    event.pack_ref.clone().unwrap_or_else(|| "-".into()),
                ),
                (
                    "HTTP Method",
                    event.http_method.clone().unwrap_or_else(|| "-".into()),
                ),
                (
                    "HTTP Path",
                    event.http_path.clone().unwrap_or_else(|| "-".into()),
                ),
                (
                    "HTTP Status",
                    event
                        .http_status
                        .map(|v| v.to_string())
                        .unwrap_or_else(|| "-".into()),
                ),
                (
                    "Request ID",
                    event.request_id.clone().unwrap_or_else(|| "-".into()),
                ),
                (
                    "Parent Event ID",
                    event
                        .parent_event_id
                        .map(|v| v.to_string())
                        .unwrap_or_else(|| "-".into()),
                ),
                (
                    "Error",
                    event.error_message.clone().unwrap_or_else(|| "-".into()),
                ),
            ];
            output::print_key_value_table(pairs);
            if let Some(details) = &event.details {
                output::print_section("Details");
                println!("{}", serde_json::to_string_pretty(details)?);
            }
        }
    }
    Ok(())
}
