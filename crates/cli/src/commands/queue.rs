use anyhow::{Context, Result};
use clap::{Args, Subcommand};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

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
    /// Query and maintain pending queue items
    Items {
        /// Queue reference
        queue_ref: String,
        #[command(subcommand)]
        command: QueueItemCommands,
    },
}

#[derive(Subcommand)]
pub enum QueueItemCommands {
    /// Preview pending items matched by a SQL/JSONPath selector
    Preview(QueueItemPreviewArgs),
    /// Merge-patch payloads for pending items matched by a SQL/JSONPath selector
    Update(QueueItemUpdateArgs),
    /// Set priority for pending items matched by a SQL/JSONPath selector
    Reprioritize(QueueItemReprioritizeArgs),
    /// Delete pending items matched by a SQL/JSONPath selector by marking them cancelled
    #[command(visible_alias = "cancel")]
    Delete(QueueItemDeleteArgs),
}

#[derive(Args)]
pub struct QueueItemPreviewArgs {
    /// PostgreSQL SQL/JSONPath selector evaluated against item payload, metadata, and fields
    #[arg(long)]
    selector: String,
    /// JSON object of SQL/JSONPath variables
    #[arg(long, default_value = "{}")]
    vars_json: String,
    /// Maximum number of matched items to show, capped at 100
    #[arg(long, default_value_t = 100)]
    limit: u32,
}

#[derive(Args)]
pub struct QueueItemUpdateArgs {
    /// PostgreSQL SQL/JSONPath selector evaluated against item payload, metadata, and fields
    #[arg(long)]
    selector: String,
    /// JSON object of SQL/JSONPath variables
    #[arg(long, default_value = "{}")]
    vars_json: String,
    /// Static JSON Merge Patch object to apply to each selected payload
    #[arg(long)]
    patch_json: String,
    /// Maximum number of affected items to include in the response preview, capped at 100
    #[arg(long, default_value_t = 100)]
    preview_limit: u32,
}

#[derive(Args)]
pub struct QueueItemReprioritizeArgs {
    /// PostgreSQL SQL/JSONPath selector evaluated against item payload, metadata, and fields
    #[arg(long)]
    selector: String,
    /// JSON object of SQL/JSONPath variables
    #[arg(long, default_value = "{}")]
    vars_json: String,
    /// Priority to assign to every selected pending item
    #[arg(long)]
    priority: i32,
    /// Maximum number of affected items to include in the response preview, capped at 100
    #[arg(long, default_value_t = 100)]
    preview_limit: u32,
}

#[derive(Args)]
pub struct QueueItemDeleteArgs {
    /// PostgreSQL SQL/JSONPath selector evaluated against item payload, metadata, and fields
    #[arg(long)]
    selector: String,
    /// JSON object of SQL/JSONPath variables
    #[arg(long, default_value = "{}")]
    vars_json: String,
    /// Maximum number of affected items to include in the response preview, capped at 100
    #[arg(long, default_value_t = 100)]
    preview_limit: u32,
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
    reference_visibility: String,
    #[serde(default)]
    reference_allowed_pack_refs: Vec<String>,
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

#[derive(Debug, Serialize)]
struct QueueItemJsonPathSelector {
    path: String,
    vars: JsonValue,
}

#[derive(Debug, Serialize)]
struct PreviewQueueItemsRequest {
    selector: QueueItemJsonPathSelector,
    limit: u32,
}

#[derive(Debug, Serialize)]
struct ApplyQueueItemsRequest {
    selector: QueueItemJsonPathSelector,
    operation: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    payload_patch: Option<JsonValue>,
    #[serde(skip_serializing_if = "Option::is_none")]
    priority: Option<i32>,
    preview_limit: u32,
}

#[derive(Debug, Serialize, Deserialize)]
struct QueueItemSummary {
    id: i64,
    #[serde(default)]
    item_key: Option<String>,
    status: String,
    priority: i32,
    payload: JsonValue,
    #[serde(default)]
    metadata: JsonValue,
    #[serde(default)]
    enqueue_source: Option<String>,
    attempt_count: i32,
    created: String,
    updated: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct PreviewQueueItemsResponse {
    matched_count: i64,
    preview_count: usize,
    items: Vec<QueueItemSummary>,
}

#[derive(Debug, Serialize, Deserialize)]
struct ApplyQueueItemsResponse {
    operation: String,
    matched_count: i64,
    affected_count: i64,
    skipped_count: i64,
    preview_count: usize,
    items: Vec<QueueItemSummary>,
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
        QueueCommands::Items { queue_ref, command } => {
            handle_items(queue_ref, command, profile, api_url, output_format).await
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
                ("Reference Visibility", queue.reference_visibility.clone()),
                (
                    "Allowed Pack Refs",
                    if queue.reference_allowed_pack_refs.is_empty() {
                        "None".to_string()
                    } else {
                        queue.reference_allowed_pack_refs.join(", ")
                    },
                ),
                ("Dispatch Action", queue.dispatch_action_ref.clone()),
                ("Created", output::format_timestamp(&queue.created)),
                ("Updated", output::format_timestamp(&queue.updated)),
            ]);
        }
    }

    Ok(())
}

async fn handle_items(
    queue_ref: String,
    command: QueueItemCommands,
    profile: &Option<String>,
    api_url: &Option<String>,
    output_format: OutputFormat,
) -> Result<()> {
    let config = CliConfig::load_with_profile(profile.as_deref())?;
    let mut client = ApiClient::from_config(&config, api_url);

    match command {
        QueueItemCommands::Preview(args) => {
            let request = PreviewQueueItemsRequest {
                selector: parse_selector(args.selector, args.vars_json)?,
                limit: validate_preview_limit(args.limit)?,
            };
            let path = format!("/queues/{}/items/query/preview", queue_ref);
            let response: PreviewQueueItemsResponse = client.post(&path, &request).await?;
            print_preview_response(response, output_format)
        }
        QueueItemCommands::Update(args) => {
            let request = ApplyQueueItemsRequest {
                selector: parse_selector(args.selector, args.vars_json)?,
                operation: "patch_payload".to_string(),
                payload_patch: Some(parse_json_object(&args.patch_json, "--patch-json")?),
                priority: None,
                preview_limit: validate_preview_limit(args.preview_limit)?,
            };
            apply_items(&mut client, &queue_ref, request, output_format).await
        }
        QueueItemCommands::Reprioritize(args) => {
            let request = ApplyQueueItemsRequest {
                selector: parse_selector(args.selector, args.vars_json)?,
                operation: "reprioritize".to_string(),
                payload_patch: None,
                priority: Some(args.priority),
                preview_limit: validate_preview_limit(args.preview_limit)?,
            };
            apply_items(&mut client, &queue_ref, request, output_format).await
        }
        QueueItemCommands::Delete(args) => {
            let request = ApplyQueueItemsRequest {
                selector: parse_selector(args.selector, args.vars_json)?,
                operation: "cancel".to_string(),
                payload_patch: None,
                priority: None,
                preview_limit: validate_preview_limit(args.preview_limit)?,
            };
            apply_items(&mut client, &queue_ref, request, output_format).await
        }
    }
}

async fn apply_items(
    client: &mut ApiClient,
    queue_ref: &str,
    request: ApplyQueueItemsRequest,
    output_format: OutputFormat,
) -> Result<()> {
    let path = format!("/queues/{}/items/query/apply", queue_ref);
    let response: ApplyQueueItemsResponse = client.post(&path, &request).await?;
    print_apply_response(response, output_format)
}

fn parse_selector(path: String, vars_json: String) -> Result<QueueItemJsonPathSelector> {
    Ok(QueueItemJsonPathSelector {
        path,
        vars: parse_json_object(&vars_json, "--vars-json")?,
    })
}

fn parse_json_object(input: &str, flag_name: &str) -> Result<JsonValue> {
    let value: JsonValue =
        serde_json::from_str(input).with_context(|| format!("Invalid JSON for {flag_name}"))?;
    if !value.is_object() {
        anyhow::bail!("{flag_name} must be a JSON object");
    }
    Ok(value)
}

fn validate_preview_limit(limit: u32) -> Result<u32> {
    if !(1..=100).contains(&limit) {
        anyhow::bail!("preview limit must be between 1 and 100");
    }
    Ok(limit)
}

fn print_preview_response(
    response: PreviewQueueItemsResponse,
    output_format: OutputFormat,
) -> Result<()> {
    match output_format {
        OutputFormat::Json | OutputFormat::Yaml => output::print_output(&response, output_format),
        OutputFormat::Table => {
            output::print_section("Queue Item Selector Preview");
            output::print_key_value_table(vec![
                ("Matched", response.matched_count.to_string()),
                ("Previewed", response.preview_count.to_string()),
            ]);
            print_items_table(&response.items)
        }
    }
}

fn print_apply_response(
    response: ApplyQueueItemsResponse,
    output_format: OutputFormat,
) -> Result<()> {
    match output_format {
        OutputFormat::Json | OutputFormat::Yaml => output::print_output(&response, output_format),
        OutputFormat::Table => {
            output::print_success(&format!(
                "Applied {} to {} pending queue item(s)",
                operation_label(&response.operation),
                response.affected_count
            ));
            output::print_key_value_table(vec![
                (
                    "Operation",
                    operation_label(&response.operation).to_string(),
                ),
                ("Matched", response.matched_count.to_string()),
                ("Affected", response.affected_count.to_string()),
                ("Skipped", response.skipped_count.to_string()),
                ("Previewed", response.preview_count.to_string()),
            ]);
            print_items_table(&response.items)
        }
    }
}

fn print_items_table(items: &[QueueItemSummary]) -> Result<()> {
    if items.is_empty() {
        output::print_info("No matching pending queue items.");
        return Ok(());
    }

    let mut table = output::create_table();
    output::add_header(
        &mut table,
        vec![
            "ID", "Key", "Status", "Priority", "Attempts", "Payload", "Created",
        ],
    );

    for item in items {
        table.add_row(vec![
            item.id.to_string(),
            item.item_key.as_deref().unwrap_or("").to_string(),
            output::format_status(&item.status),
            item.priority.to_string(),
            item.attempt_count.to_string(),
            output::truncate(&compact_json(&item.payload), 96),
            output::format_timestamp(&item.created),
        ]);
    }

    println!("{}", table);
    Ok(())
}

fn operation_label(operation: &str) -> &str {
    match operation {
        "patch_payload" => "update",
        "reprioritize" => "reprioritize",
        "cancel" => "delete/cancel",
        other => other,
    }
}

fn compact_json(value: &JsonValue) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "null".to_string())
}
