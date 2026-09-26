use std::io::{self, Read};

use anyhow::{Context, Result};
use clap::{Args, Subcommand, ValueEnum};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

use crate::client::{ApiClient, PaginatedResponse};
use crate::config::CliConfig;
use crate::output::{self, OutputFormat};

#[derive(Subcommand)]
pub enum QueueCommands {
    /// List visible work queues
    List(QueueListArgs),
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
    /// Enqueue one work item from a JSON request
    Enqueue(QueueEnqueueArgs),
    /// Update queue metadata
    Update {
        /// Queue reference
        queue_ref: String,

        /// Set execution trace tag template
        #[arg(long, conflicts_with = "clear_trace_tag_template")]
        trace_tag_template: Option<String>,

        /// Clear execution trace tag template
        #[arg(long)]
        clear_trace_tag_template: bool,
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
    /// List queue items for operational inspection
    List(QueueItemListArgs),
    /// Show one queue item
    Show {
        /// Queue item ID
        item_id: i64,
    },
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
pub struct QueueListArgs {
    /// Limit results to queues owned by this pack
    #[arg(long)]
    pack: Option<String>,
    /// Filter by enabled state
    #[arg(long)]
    enabled: Option<bool>,
    /// Filter by ad hoc or pack-managed queue type
    #[arg(long)]
    is_adhoc: Option<bool>,
    /// Search queue refs, labels, and descriptions
    #[arg(long)]
    search: Option<String>,
    /// Pack that intends to submit items, used for restricted queue discovery
    #[arg(long)]
    referencing_pack_ref: Option<String>,
    /// Result page, starting at 1
    #[arg(long, default_value_t = 1, value_parser = parse_page)]
    page: u32,
    /// Results per page
    #[arg(long, default_value_t = 50, value_parser = parse_per_page)]
    per_page: u32,
}

#[derive(Args)]
pub struct QueueEnqueueArgs {
    /// Queue reference
    queue_ref: String,
    /// Complete enqueue request as JSON
    #[arg(
        long,
        required_unless_present = "request_file",
        conflicts_with = "request_file"
    )]
    request_json: Option<String>,
    /// JSON request file, or '-' to read from stdin
    #[arg(
        long,
        required_unless_present = "request_json",
        conflicts_with = "request_json"
    )]
    request_file: Option<String>,
}

#[derive(Args)]
pub struct QueueItemListArgs {
    /// Filter by exact item key
    #[arg(long)]
    item_key: Option<String>,
    /// Filter by enqueue source
    #[arg(long)]
    enqueue_source: Option<String>,
    /// Filter by status; may be repeated
    #[arg(long, value_enum)]
    status: Vec<QueueItemStatus>,
    /// Result page, starting at 1
    #[arg(long, default_value_t = 1, value_parser = parse_page)]
    page: u32,
    /// Results per page
    #[arg(long, default_value_t = 50, value_parser = parse_per_page)]
    per_page: u32,
}

#[derive(Clone, Copy, ValueEnum)]
pub enum QueueItemStatus {
    Queued,
    Leased,
    Retry,
    Completed,
    Failed,
    Skipped,
    Cancelled,
}

impl QueueItemStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Leased => "leased",
            Self::Retry => "retry",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Skipped => "skipped",
            Self::Cancelled => "cancelled",
        }
    }
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
    #[serde(default)]
    trace_tag_template: Option<String>,
    reference_visibility: String,
    #[serde(default)]
    reference_allowed_pack_refs: Vec<String>,
    created: String,
    updated: String,
}

fn default_true() -> bool {
    true
}

fn default_json_object() -> JsonValue {
    serde_json::json!({})
}

#[derive(Debug, Serialize, Deserialize)]
struct QueueSummary {
    id: i64,
    #[serde(rename = "ref")]
    queue_ref: String,
    #[serde(default)]
    pack_ref: Option<String>,
    is_adhoc: bool,
    label: String,
    #[serde(default)]
    description: Option<String>,
    enabled: bool,
    accepting_new_items: bool,
    dispatch_action_ref: String,
    #[serde(default)]
    trace_tag_template: Option<String>,
    reference_visibility: String,
    #[serde(default)]
    reference_allowed_pack_refs: Vec<String>,
    #[serde(default)]
    retired_at: Option<String>,
    created: String,
    updated: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct EnqueueQueueItemRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    item_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    priority: Option<i32>,
    payload: JsonValue,
    #[serde(default = "default_json_object")]
    metadata: JsonValue,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    trace_tag: Option<String>,
}

#[derive(Debug, Serialize)]
struct UpdateQueueOperationalFlags {
    enabled: bool,
}

#[derive(Debug, Serialize)]
struct UpdateQueueRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    trace_tag_template: Option<Option<String>>,
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
    queue: i64,
    queue_ref: String,
    #[serde(default)]
    item_key: Option<String>,
    status: String,
    priority: i32,
    payload: JsonValue,
    #[serde(default)]
    metadata: JsonValue,
    enqueue_source: String,
    #[serde(default)]
    trace_tag: Option<String>,
    #[serde(default)]
    requested_by_identity: Option<i64>,
    #[serde(default)]
    requested_by_execution: Option<i64>,
    #[serde(default)]
    requested_by_enforcement: Option<i64>,
    #[serde(default)]
    leased_execution: Option<i64>,
    #[serde(default)]
    lease_token: Option<String>,
    #[serde(default)]
    lease_expires_at: Option<String>,
    attempt_count: i32,
    #[serde(default)]
    last_error: Option<JsonValue>,
    #[serde(default)]
    ack_summary: Option<JsonValue>,
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
        QueueCommands::List(args) => handle_list(args, profile, api_url, output_format).await,
        QueueCommands::Show { queue_ref } => {
            handle_show(queue_ref, profile, api_url, output_format).await
        }
        QueueCommands::Enable { queue_ref } => {
            handle_toggle(queue_ref, true, profile, api_url, output_format).await
        }
        QueueCommands::Disable { queue_ref } => {
            handle_toggle(queue_ref, false, profile, api_url, output_format).await
        }
        QueueCommands::Enqueue(args) => handle_enqueue(args, profile, api_url, output_format).await,
        QueueCommands::Update {
            queue_ref,
            trace_tag_template,
            clear_trace_tag_template,
        } => {
            handle_update(
                queue_ref,
                trace_tag_template,
                clear_trace_tag_template,
                profile,
                api_url,
                output_format,
            )
            .await
        }
        QueueCommands::Items { queue_ref, command } => {
            handle_items(queue_ref, command, profile, api_url, output_format).await
        }
    }
}

async fn handle_list(
    args: QueueListArgs,
    profile: &Option<String>,
    api_url: &Option<String>,
    output_format: OutputFormat,
) -> Result<()> {
    let config = CliConfig::load_with_profile(profile.as_deref())?;
    let mut client = ApiClient::from_config(&config, api_url);
    let base_path = match args.pack.as_deref() {
        Some(pack_ref) => format!("/packs/{}/queues", encode_path_segment(pack_ref)),
        None => "/queues".to_string(),
    };
    let mut query = url::form_urlencoded::Serializer::new(String::new());
    if let Some(enabled) = args.enabled {
        query.append_pair("enabled", &enabled.to_string());
    }
    if let Some(is_adhoc) = args.is_adhoc {
        query.append_pair("is_adhoc", &is_adhoc.to_string());
    }
    if let Some(search) = args.search.as_deref() {
        query.append_pair("search", search);
    }
    if let Some(pack_ref) = args.referencing_pack_ref.as_deref() {
        query.append_pair("referencing_pack_ref", pack_ref);
    }
    query.append_pair("page", &args.page.to_string());
    query.append_pair("per_page", &args.per_page.to_string());
    let path = format!("{base_path}?{}", query.finish());
    let response: PaginatedResponse<QueueSummary> = client.get_paginated_response(&path).await?;
    print_queue_list(&response, output_format)
}

async fn handle_enqueue(
    args: QueueEnqueueArgs,
    profile: &Option<String>,
    api_url: &Option<String>,
    output_format: OutputFormat,
) -> Result<()> {
    let request = read_enqueue_request(args.request_json, args.request_file)?;
    let config = CliConfig::load_with_profile(profile.as_deref())?;
    let mut client = ApiClient::from_config(&config, api_url);
    let path = format!("/queues/{}/items", encode_path_segment(&args.queue_ref));
    let item: QueueItemSummary = client.post(&path, &request).await?;
    print_queue_item(&item, output_format)
}

async fn handle_show(
    queue_ref: String,
    profile: &Option<String>,
    api_url: &Option<String>,
    output_format: OutputFormat,
) -> Result<()> {
    let config = CliConfig::load_with_profile(profile.as_deref())?;
    let mut client = ApiClient::from_config(&config, api_url);

    let path = format!("/queues/{}", encode_path_segment(&queue_ref));
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

    let path = format!("/queues/{}", encode_path_segment(&queue_ref));
    let queue: QueueDetail = client
        .put(&path, &UpdateQueueOperationalFlags { enabled })
        .await?;
    print_queue(
        queue,
        output_format,
        Some(if enabled { "enabled" } else { "disabled" }),
    )
}

async fn handle_update(
    queue_ref: String,
    trace_tag_template: Option<String>,
    clear_trace_tag_template: bool,
    profile: &Option<String>,
    api_url: &Option<String>,
    output_format: OutputFormat,
) -> Result<()> {
    if trace_tag_template.is_none() && !clear_trace_tag_template {
        anyhow::bail!("At least one field must be provided to update");
    }

    let config = CliConfig::load_with_profile(profile.as_deref())?;
    let mut client = ApiClient::from_config(&config, api_url);
    let path = format!("/queues/{}", encode_path_segment(&queue_ref));
    let request = UpdateQueueRequest {
        trace_tag_template: if clear_trace_tag_template {
            Some(None)
        } else {
            trace_tag_template.map(Some)
        },
    };
    let queue: QueueDetail = client.put(&path, &request).await?;
    print_queue(queue, output_format, Some("updated"))
}

fn print_queue_list(
    response: &PaginatedResponse<QueueSummary>,
    output_format: OutputFormat,
) -> Result<()> {
    if output_format != OutputFormat::Table {
        return output::print_output(response, output_format);
    }
    if response.items.is_empty() {
        output::print_info("No work queues found.");
        return Ok(());
    }

    let mut table = output::create_table();
    output::add_header(
        &mut table,
        vec![
            "ID",
            "Ref",
            "Pack",
            "Label",
            "Enabled",
            "Accepting",
            "Dispatch action",
            "Created",
        ],
    );
    for queue in &response.items {
        table.add_row(vec![
            queue.id.to_string(),
            queue.queue_ref.clone(),
            queue.pack_ref.as_deref().unwrap_or("-").to_string(),
            queue.label.clone(),
            output::format_bool(queue.enabled),
            output::format_bool(queue.accepting_new_items),
            queue.dispatch_action_ref.clone(),
            output::format_timestamp(&queue.created),
        ]);
    }
    println!("{table}");
    Ok(())
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
                (
                    "Trace Tag Template",
                    queue
                        .trace_tag_template
                        .clone()
                        .unwrap_or_else(|| "None".to_string()),
                ),
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
        QueueItemCommands::List(args) => {
            let mut query = url::form_urlencoded::Serializer::new(String::new());
            if let Some(item_key) = args.item_key.as_deref() {
                query.append_pair("item_key", item_key);
            }
            if let Some(enqueue_source) = args.enqueue_source.as_deref() {
                query.append_pair("enqueue_source", enqueue_source);
            }
            for status in args.status {
                query.append_pair("statuses", status.as_str());
            }
            query.append_pair("page", &args.page.to_string());
            query.append_pair("per_page", &args.per_page.to_string());
            let path = format!(
                "/queues/{}/items?{}",
                encode_path_segment(&queue_ref),
                query.finish()
            );
            let response: PaginatedResponse<QueueItemSummary> =
                client.get_paginated_response(&path).await?;
            print_queue_item_list(&response, output_format)
        }
        QueueItemCommands::Show { item_id } => {
            let path = format!(
                "/queues/{}/items/{item_id}",
                encode_path_segment(&queue_ref)
            );
            let item: QueueItemSummary = client.get(&path).await?;
            print_queue_item(&item, output_format)
        }
        QueueItemCommands::Preview(args) => {
            let request = PreviewQueueItemsRequest {
                selector: parse_selector(args.selector, args.vars_json)?,
                limit: validate_preview_limit(args.limit)?,
            };
            let path = format!(
                "/queues/{}/items/query/preview",
                encode_path_segment(&queue_ref)
            );
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
    let path = format!(
        "/queues/{}/items/query/apply",
        encode_path_segment(queue_ref)
    );
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

fn read_enqueue_request(
    request_json: Option<String>,
    request_file: Option<String>,
) -> Result<EnqueueQueueItemRequest> {
    let content = match (request_json, request_file) {
        (Some(content), None) => content,
        (None, Some(path)) if path == "-" => {
            let mut content = String::new();
            io::stdin()
                .read_to_string(&mut content)
                .context("Failed to read enqueue request from stdin")?;
            content
        }
        (None, Some(path)) => std::fs::read_to_string(&path)
            .with_context(|| format!("Failed to read enqueue request file '{path}'"))?,
        _ => unreachable!("clap requires exactly one enqueue request source"),
    };
    serde_json::from_str(&content).map_err(|error| {
        anyhow::anyhow!(
            "Enqueue request must be valid JSON with payload and optional item_key, priority, metadata, and trace_tag fields: {error}"
        )
    })
}

fn encode_path_segment(value: &str) -> String {
    urlencoding::encode(value).into_owned()
}

fn parse_page(value: &str) -> std::result::Result<u32, String> {
    let page = value
        .parse::<u32>()
        .map_err(|_| "page must be an integer".to_string())?;
    if page == 0 {
        Err("page must be at least 1".to_string())
    } else {
        Ok(page)
    }
}

fn parse_per_page(value: &str) -> std::result::Result<u32, String> {
    let per_page = value
        .parse::<u32>()
        .map_err(|_| "per-page must be an integer".to_string())?;
    if (1..=100).contains(&per_page) {
        Ok(per_page)
    } else {
        Err("per-page must be between 1 and 100".to_string())
    }
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
            print_items_table(&response.items, "No matching pending queue items.")
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
            print_items_table(&response.items, "No matching pending queue items.")
        }
    }
}

fn print_queue_item_list(
    response: &PaginatedResponse<QueueItemSummary>,
    output_format: OutputFormat,
) -> Result<()> {
    if output_format != OutputFormat::Table {
        return output::print_output(response, output_format);
    }
    print_items_table(&response.items, "No queue items found.")
}

fn print_queue_item(item: &QueueItemSummary, output_format: OutputFormat) -> Result<()> {
    if output_format != OutputFormat::Table {
        return output::print_output(item, output_format);
    }

    output::print_key_value_table(vec![
        ("ID", item.id.to_string()),
        ("Queue", item.queue_ref.clone()),
        (
            "Item key",
            item.item_key.clone().unwrap_or_else(|| "-".to_string()),
        ),
        ("Status", item.status.clone()),
        ("Priority", item.priority.to_string()),
        ("Attempts", item.attempt_count.to_string()),
        ("Enqueue source", item.enqueue_source.clone()),
        (
            "Trace tag",
            item.trace_tag.clone().unwrap_or_else(|| "-".to_string()),
        ),
        (
            "Requested by identity",
            item.requested_by_identity
                .map(|id| id.to_string())
                .unwrap_or_else(|| "-".to_string()),
        ),
        (
            "Requested by execution",
            item.requested_by_execution
                .map(|id| id.to_string())
                .unwrap_or_else(|| "-".to_string()),
        ),
        (
            "Requested by enforcement",
            item.requested_by_enforcement
                .map(|id| id.to_string())
                .unwrap_or_else(|| "-".to_string()),
        ),
        (
            "Leased execution",
            item.leased_execution
                .map(|id| id.to_string())
                .unwrap_or_else(|| "-".to_string()),
        ),
        (
            "Lease token",
            item.lease_token.clone().unwrap_or_else(|| "-".to_string()),
        ),
        (
            "Lease expires",
            item.lease_expires_at
                .as_deref()
                .map(output::format_timestamp)
                .unwrap_or_else(|| "-".to_string()),
        ),
        ("Payload", compact_json(&item.payload)),
        ("Metadata", compact_json(&item.metadata)),
        (
            "Last error",
            item.last_error
                .as_ref()
                .map(compact_json)
                .unwrap_or_else(|| "-".to_string()),
        ),
        (
            "Acknowledgement",
            item.ack_summary
                .as_ref()
                .map(compact_json)
                .unwrap_or_else(|| "-".to_string()),
        ),
        ("Created", output::format_timestamp(&item.created)),
        ("Updated", output::format_timestamp(&item.updated)),
    ]);
    Ok(())
}

fn print_items_table(items: &[QueueItemSummary], empty_message: &str) -> Result<()> {
    if items.is_empty() {
        output::print_info(empty_message);
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

#[cfg(test)]
mod tests {
    use clap::CommandFactory;

    use crate::cli::Cli;

    #[test]
    fn queue_command_tree_contains_expected_verbs() {
        let command = Cli::command();
        let queue = command
            .get_subcommands()
            .find(|command| command.get_name() == "queue")
            .expect("queue command");
        let mut queue_verbs = queue
            .get_subcommands()
            .map(|command| command.get_name())
            .collect::<Vec<_>>();
        queue_verbs.sort_unstable();
        assert_eq!(
            queue_verbs,
            ["disable", "enable", "enqueue", "items", "list", "show", "update"]
        );

        let items = queue
            .get_subcommands()
            .find(|command| command.get_name() == "items")
            .expect("queue items command");
        let mut item_verbs = items
            .get_subcommands()
            .map(|command| command.get_name())
            .collect::<Vec<_>>();
        item_verbs.sort_unstable();
        assert_eq!(
            item_verbs,
            [
                "delete",
                "list",
                "preview",
                "reprioritize",
                "show",
                "update"
            ]
        );
    }
}
