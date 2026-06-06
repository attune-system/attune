use anyhow::{Context, Result};
use clap::Subcommand;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::client::ApiClient;
use crate::config::CliConfig;
use crate::output::{self, OutputFormat};
use crate::wait::{extract_stdout, spawn_execution_output_watch, wait_for_execution, WaitOptions};

#[derive(Subcommand)]
pub enum ActionCommands {
    /// List all actions
    List {
        /// Filter by pack name
        #[arg(long)]
        pack: Option<String>,

        /// Filter by action name
        #[arg(short, long)]
        name: Option<String>,
    },
    /// Search for actions by keyword across ref, label, description, and pack ref.
    /// Whitespace-separated tokens are AND-matched.
    Search {
        /// Keyword query (e.g., "slack post message")
        query: Option<String>,

        /// Restrict to one or more pack refs (repeat flag for multiple packs).
        #[arg(long = "pack", short = 'p')]
        packs: Vec<String>,

        /// Maximum number of results to return
        #[arg(long, default_value = "50")]
        limit: u32,
    },
    /// Show details of a specific action
    Show {
        /// Action reference (pack.action or ID)
        action_ref: String,
    },
    /// Update an action
    Update {
        /// Action reference (pack.action or ID)
        action_ref: String,

        /// Update label
        #[arg(long)]
        label: Option<String>,

        /// Update description
        #[arg(long)]
        description: Option<String>,

        /// Update entrypoint
        #[arg(long)]
        entrypoint: Option<String>,

        /// Update runtime ID
        #[arg(long)]
        runtime: Option<i64>,

        /// Update enabled status
        #[arg(long)]
        enabled: Option<bool>,

        /// Set the default execution timeout (seconds) for this action.
        #[arg(long)]
        timeout_seconds: Option<i32>,

        /// Clear the action's default execution timeout (revert to app default).
        #[arg(long, conflicts_with = "timeout_seconds")]
        clear_timeout: bool,
    },
    /// Enable an action
    Enable {
        /// Action reference (pack.action or ID)
        action_ref: String,
    },
    /// Disable an action
    Disable {
        /// Action reference (pack.action or ID)
        action_ref: String,
    },
    /// Delete an action
    Delete {
        /// Action reference (pack.action or ID)
        action_ref: String,

        /// Skip confirmation prompt
        #[arg(long)]
        yes: bool,
    },
    /// Execute an action
    Execute {
        /// Action reference (pack.action or ID)
        action_ref: String,

        /// Action parameters in key=value format
        #[arg(long)]
        param: Vec<String>,

        /// Parameters as JSON string
        #[arg(long, conflicts_with = "param")]
        params_json: Option<String>,

        /// Worker label selector as JSON (e.g. '{"pool":"gpu"}')
        #[arg(long)]
        worker_selector: Option<String>,

        /// Worker tolerations as JSON array
        #[arg(long)]
        worker_tolerations: Option<String>,

        /// Worker affinity as JSON object
        #[arg(long)]
        worker_affinity: Option<String>,

        /// Execution timeout override in seconds (snapshotted onto the execution).
        #[arg(long)]
        execution_timeout: Option<i32>,

        /// Watch execution until it completes
        #[arg(short, long)]
        watch: bool,

        /// Timeout in seconds when watching (default: 300)
        #[arg(long, default_value = "300", requires = "watch")]
        timeout: u64,

        /// Notifier WebSocket base URL (e.g. ws://localhost:8081).
        /// Derived from --api-url automatically when not set.
        #[arg(long, requires = "watch")]
        notifier_url: Option<String>,
    },
}

fn format_runtime(
    runtime_ref: Option<&str>,
    version_constraint: Option<&str>,
    is_workflow: bool,
) -> String {
    if is_workflow {
        return "Workflow".to_string();
    }
    match (runtime_ref, version_constraint) {
        (Some(r), Some(v)) => format!("{} ({})", r, v),
        (Some(r), None) => r.to_string(),
        (None, Some(v)) => format!("(unknown) ({})", v),
        (None, None) => "none".to_string(),
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct Action {
    id: i64,
    #[serde(rename = "ref")]
    action_ref: String,
    pack_ref: String,
    label: String,
    description: Option<String>,
    entrypoint: String,
    runtime: Option<i64>,
    #[serde(default)]
    runtime_ref: Option<String>,
    #[serde(default)]
    runtime_version_constraint: Option<String>,
    #[serde(default)]
    workflow_def: Option<i64>,
    #[serde(default = "default_true")]
    enabled: bool,
    created: String,
    updated: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct ActionDetail {
    id: i64,
    #[serde(rename = "ref")]
    action_ref: String,
    pack: i64,
    pack_ref: String,
    label: String,
    description: Option<String>,
    entrypoint: String,
    runtime: Option<i64>,
    #[serde(default)]
    runtime_ref: Option<String>,
    #[serde(default)]
    runtime_version_constraint: Option<String>,
    #[serde(default)]
    workflow_def: Option<i64>,
    #[serde(default = "default_true")]
    enabled: bool,
    param_schema: Option<serde_json::Value>,
    out_schema: Option<serde_json::Value>,
    created: String,
    updated: String,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Serialize)]
struct UpdateActionRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    entrypoint: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    runtime: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    enabled: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    timeout_seconds: Option<serde_json::Value>,
}

#[derive(Debug, Serialize)]
struct ExecuteActionRequest {
    action_ref: String,
    parameters: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    worker_selector: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    worker_tolerations: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    worker_affinity: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    timeout_seconds: Option<i32>,
}

#[derive(Debug, Serialize, Deserialize)]
struct Execution {
    id: i64,
    action: Option<i64>,
    action_ref: String,
    config: Option<serde_json::Value>,
    parent: Option<i64>,
    enforcement: Option<i64>,
    executor: Option<i64>,
    status: String,
    result: Option<serde_json::Value>,
    created: String,
    updated: String,
}

pub async fn handle_action_command(
    profile: &Option<String>,
    command: ActionCommands,
    api_url: &Option<String>,
    output_format: OutputFormat,
) -> Result<()> {
    match command {
        ActionCommands::List { pack, name } => {
            handle_list(pack, name, profile, api_url, output_format).await
        }
        ActionCommands::Search {
            query,
            packs,
            limit,
        } => handle_search(query, packs, limit, profile, api_url, output_format).await,
        ActionCommands::Show { action_ref } => {
            handle_show(action_ref, profile, api_url, output_format).await
        }
        ActionCommands::Update {
            action_ref,
            label,
            description,
            entrypoint,
            runtime,
            enabled,
            timeout_seconds,
            clear_timeout,
        } => {
            handle_update(
                action_ref,
                label,
                description,
                entrypoint,
                runtime,
                enabled,
                timeout_seconds,
                clear_timeout,
                profile,
                api_url,
                output_format,
            )
            .await
        }
        ActionCommands::Enable { action_ref } => {
            handle_toggle(action_ref, true, profile, api_url, output_format).await
        }
        ActionCommands::Disable { action_ref } => {
            handle_toggle(action_ref, false, profile, api_url, output_format).await
        }
        ActionCommands::Delete { action_ref, yes } => {
            handle_delete(action_ref, yes, profile, api_url, output_format).await
        }
        ActionCommands::Execute {
            action_ref,
            param,
            params_json,
            worker_selector,
            worker_tolerations,
            worker_affinity,
            execution_timeout,
            watch,
            timeout,
            notifier_url,
        } => {
            handle_execute(
                action_ref,
                param,
                params_json,
                worker_selector,
                worker_tolerations,
                worker_affinity,
                execution_timeout,
                profile,
                api_url,
                watch,
                timeout,
                notifier_url,
                output_format,
            )
            .await
        }
    }
}

async fn handle_list(
    pack: Option<String>,
    name: Option<String>,
    profile: &Option<String>,
    api_url: &Option<String>,
    output_format: OutputFormat,
) -> Result<()> {
    let config = CliConfig::load_with_profile(profile.as_deref())?;
    let mut client = ApiClient::from_config(&config, api_url);

    // Use pack-specific endpoint if pack filter is specified
    let path = if let Some(pack_ref) = pack {
        format!("/packs/{}/actions", pack_ref)
    } else {
        "/actions".to_string()
    };

    let mut actions: Vec<Action> = client.get_paginated(&path).await?;

    // Filter by name if specified (client-side filtering)
    if let Some(action_name) = name {
        actions.retain(|a| a.action_ref.contains(&action_name));
    }

    match output_format {
        OutputFormat::Json | OutputFormat::Yaml => {
            output::print_output(&actions, output_format)?;
        }
        OutputFormat::Table => {
            if actions.is_empty() {
                output::print_info("No actions found");
            } else {
                let mut table = output::create_table();
                output::add_header(
                    &mut table,
                    vec!["Ref", "Label", "Runtime", "Enabled", "Description"],
                );

                for action in actions {
                    let is_workflow = action.workflow_def.is_some();
                    table.add_row(vec![
                        action.action_ref.clone(),
                        action.label.clone(),
                        format_runtime(
                            action.runtime_ref.as_deref(),
                            action.runtime_version_constraint.as_deref(),
                            is_workflow,
                        ),
                        output::format_bool(action.enabled),
                        output::truncate(&action.description.unwrap_or_default(), 40),
                    ]);
                }

                println!("{}", table);
            }
        }
    }

    Ok(())
}

#[derive(Debug, Serialize, Deserialize)]
struct ActionSearchHit {
    #[serde(rename = "ref")]
    action_ref: String,
    pack_ref: String,
    label: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    runtime_ref: Option<String>,
    #[serde(default)]
    is_workflow: bool,
    #[serde(default)]
    accesses_mcp: bool,
}

async fn handle_search(
    query: Option<String>,
    packs: Vec<String>,
    limit: u32,
    profile: &Option<String>,
    api_url: &Option<String>,
    output_format: OutputFormat,
) -> Result<()> {
    let config = CliConfig::load_with_profile(profile.as_deref())?;
    let mut client = ApiClient::from_config(&config, api_url);

    let mut params: Vec<(&str, String)> = Vec::new();
    if let Some(q) = query.as_ref().map(|q| q.trim()).filter(|q| !q.is_empty()) {
        params.push(("q", q.to_string()));
    }
    if !packs.is_empty() {
        params.push(("packs", packs.join(",")));
    }
    let limit = limit.clamp(1, 100);
    params.push(("page_size", limit.to_string()));
    params.push(("page", "1".to_string()));

    let qs = params
        .iter()
        .map(|(k, v)| format!("{}={}", k, urlencoding::encode(v)))
        .collect::<Vec<_>>()
        .join("&");
    let path = format!("/actions/search?{}", qs);

    let hits: Vec<ActionSearchHit> = client.get_paginated(&path).await?;

    match output_format {
        OutputFormat::Json | OutputFormat::Yaml => {
            output::print_output(&hits, output_format)?;
        }
        OutputFormat::Table => {
            if hits.is_empty() {
                output::print_info("No actions matched the search query");
            } else {
                let mut table = output::create_table();
                output::add_header(
                    &mut table,
                    vec!["Ref", "Pack", "Label", "Runtime", "MCP", "Description"],
                );
                for hit in &hits {
                    table.add_row(vec![
                        hit.action_ref.clone(),
                        hit.pack_ref.clone(),
                        hit.label.clone(),
                        format_runtime(hit.runtime_ref.as_deref(), None, hit.is_workflow),
                        if hit.accesses_mcp { "yes" } else { "" }.to_string(),
                        output::truncate(hit.description.as_deref().unwrap_or(""), 50),
                    ]);
                }
                println!("{}", table);
            }
        }
    }

    Ok(())
}

async fn handle_show(
    action_ref: String,
    profile: &Option<String>,
    api_url: &Option<String>,
    output_format: OutputFormat,
) -> Result<()> {
    let config = CliConfig::load_with_profile(profile.as_deref())?;
    let mut client = ApiClient::from_config(&config, api_url);

    let path = format!("/actions/{}", action_ref);
    let action: ActionDetail = client.get(&path).await?;

    match output_format {
        OutputFormat::Json | OutputFormat::Yaml => {
            output::print_output(&action, output_format)?;
        }
        OutputFormat::Table => {
            output::print_section(&format!("Action: {}", action.action_ref));
            let is_workflow = action.workflow_def.is_some();
            output::print_key_value_table(vec![
                ("Reference", action.action_ref.clone()),
                ("Pack", action.pack_ref.clone()),
                ("Label", action.label.clone()),
                (
                    "Description",
                    action.description.unwrap_or_else(|| "None".to_string()),
                ),
                ("Entry Point", action.entrypoint.clone()),
                (
                    "Runtime",
                    format_runtime(
                        action.runtime_ref.as_deref(),
                        action.runtime_version_constraint.as_deref(),
                        is_workflow,
                    ),
                ),
                ("Enabled", output::format_bool(action.enabled)),
                ("Created", output::format_timestamp(&action.created)),
                ("Updated", output::format_timestamp(&action.updated)),
            ]);

            if let Some(params) = action.param_schema {
                if !params.is_null() {
                    output::print_section("Parameters Schema");
                    output::print_schema(&params)?;
                }
            }
        }
    }

    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn handle_update(
    action_ref: String,
    label: Option<String>,
    description: Option<String>,
    entrypoint: Option<String>,
    runtime: Option<i64>,
    enabled: Option<bool>,
    timeout_seconds: Option<i32>,
    clear_timeout: bool,
    profile: &Option<String>,
    api_url: &Option<String>,
    output_format: OutputFormat,
) -> Result<()> {
    let config = CliConfig::load_with_profile(profile.as_deref())?;
    let mut client = ApiClient::from_config(&config, api_url);

    // Build the optional timeout patch ({"op":"set","value":N} or {"op":"clear"}).
    let timeout_patch = if clear_timeout {
        Some(serde_json::json!({ "op": "clear" }))
    } else {
        timeout_seconds.map(|value| serde_json::json!({ "op": "set", "value": value }))
    };

    // Check that at least one field is provided
    if label.is_none()
        && description.is_none()
        && entrypoint.is_none()
        && runtime.is_none()
        && enabled.is_none()
        && timeout_patch.is_none()
    {
        anyhow::bail!("At least one field must be provided to update");
    }

    let request = UpdateActionRequest {
        label,
        description,
        entrypoint,
        runtime,
        enabled,
        timeout_seconds: timeout_patch,
    };

    let path = format!("/actions/{}", action_ref);
    let action: ActionDetail = client.put(&path, &request).await?;

    match output_format {
        OutputFormat::Json | OutputFormat::Yaml => {
            output::print_output(&action, output_format)?;
        }
        OutputFormat::Table => {
            output::print_success(&format!(
                "Action '{}' updated successfully",
                action.action_ref
            ));
            let is_workflow = action.workflow_def.is_some();
            output::print_key_value_table(vec![
                ("Ref", action.action_ref.clone()),
                ("Pack", action.pack_ref.clone()),
                ("Label", action.label.clone()),
                (
                    "Description",
                    action.description.unwrap_or_else(|| "None".to_string()),
                ),
                ("Entrypoint", action.entrypoint.clone()),
                (
                    "Runtime",
                    format_runtime(
                        action.runtime_ref.as_deref(),
                        action.runtime_version_constraint.as_deref(),
                        is_workflow,
                    ),
                ),
                ("Enabled", output::format_bool(action.enabled)),
                ("Updated", output::format_timestamp(&action.updated)),
            ]);
        }
    }

    Ok(())
}

async fn handle_toggle(
    action_ref: String,
    enabled: bool,
    profile: &Option<String>,
    api_url: &Option<String>,
    output_format: OutputFormat,
) -> Result<()> {
    handle_update(
        action_ref,
        None,
        None,
        None,
        None,
        Some(enabled),
        None,
        false,
        profile,
        api_url,
        output_format,
    )
    .await
}

async fn handle_delete(
    action_ref: String,
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
                "Are you sure you want to delete action '{}'?",
                action_ref
            ))
            .default(false)
            .interact()?;

        if !confirm {
            output::print_info("Delete cancelled");
            return Ok(());
        }
    }

    let path = format!("/actions/{}", action_ref);
    client.delete_no_response(&path).await?;

    match output_format {
        OutputFormat::Json | OutputFormat::Yaml => {
            let msg = serde_json::json!({"message": "Action deleted successfully"});
            output::print_output(&msg, output_format)?;
        }
        OutputFormat::Table => {
            output::print_success(&format!("Action '{}' deleted successfully", action_ref));
        }
    }

    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn handle_execute(
    action_ref: String,
    params: Vec<String>,
    params_json: Option<String>,
    worker_selector: Option<String>,
    worker_tolerations: Option<String>,
    worker_affinity: Option<String>,
    execution_timeout: Option<i32>,
    profile: &Option<String>,
    api_url: &Option<String>,
    watch: bool,
    timeout: u64,
    notifier_url: Option<String>,
    output_format: OutputFormat,
) -> Result<()> {
    let config = CliConfig::load_with_profile(profile.as_deref())?;
    let mut client = ApiClient::from_config(&config, api_url);

    // Parse parameters
    let parameters = if let Some(json_str) = params_json {
        serde_json::from_str(&json_str)?
    } else if !params.is_empty() {
        let mut map = HashMap::new();
        for p in params {
            let parts: Vec<&str> = p.splitn(2, '=').collect();
            if parts.len() != 2 {
                anyhow::bail!("Invalid parameter format: '{}'. Expected key=value", p);
            }
            // Try to parse as JSON value, fall back to string
            let value: serde_json::Value = serde_json::from_str(parts[1])
                .unwrap_or_else(|_| serde_json::Value::String(parts[1].to_string()));
            map.insert(parts[0].to_string(), value);
        }
        serde_json::to_value(map)?
    } else {
        serde_json::json!({})
    };

    let selector = worker_selector
        .map(|s| serde_json::from_str(&s))
        .transpose()
        .context("Invalid --worker-selector JSON")?;
    let tolerations = worker_tolerations
        .map(|s| serde_json::from_str(&s))
        .transpose()
        .context("Invalid --worker-tolerations JSON")?;
    let affinity = worker_affinity
        .map(|s| serde_json::from_str(&s))
        .transpose()
        .context("Invalid --worker-affinity JSON")?;

    let request = ExecuteActionRequest {
        action_ref: action_ref.clone(),
        parameters,
        worker_selector: selector,
        worker_tolerations: tolerations,
        worker_affinity: affinity,
        timeout_seconds: execution_timeout,
    };

    if output_format == OutputFormat::Table {
        output::print_info(&format!("Executing action: {}", action_ref));
    }

    let path = "/executions/execute".to_string();
    let execution: Execution = client.post(&path, &request).await?;

    if !watch {
        match output_format {
            OutputFormat::Json | OutputFormat::Yaml => {
                output::print_output(&execution, output_format)?;
            }
            OutputFormat::Table => {
                output::print_success(&format!("Execution {} started", execution.id));
                output::print_key_value_table(vec![
                    ("Execution ID", execution.id.to_string()),
                    ("Action", execution.action_ref.clone()),
                    ("Status", output::format_status(&execution.status)),
                ]);
            }
        }
        return Ok(());
    }

    if output_format == OutputFormat::Table {
        output::print_info(&format!(
            "Waiting for execution {} to complete...",
            execution.id
        ));
    }

    let interactive_wait = true;
    let stream_live_logs = true;
    let debug_wait = false;
    let watch_task = Some(spawn_execution_output_watch(
        ApiClient::from_config(&config, api_url),
        execution.id,
        notifier_url.clone(),
        interactive_wait,
        stream_live_logs,
        debug_wait,
    ));
    let summary = wait_for_execution(WaitOptions {
        execution_id: execution.id,
        timeout_secs: timeout,
        api_client: &mut client,
        notifier_ws_url: notifier_url,
        verbose: debug_wait,
    })
    .await?;
    let (delivered_output, root_stdout_completed) = match watch_task {
        Some(task) => task.join().await,
        None => (false, false),
    };
    let suppress_final_stdout = delivered_output && root_stdout_completed;

    match output_format {
        OutputFormat::Json | OutputFormat::Yaml => {
            output::print_output(&summary, output_format)?;
        }
        OutputFormat::Table => {
            output::print_success(&format!("Execution {} completed", summary.id));
            output::print_section("Execution Details");
            output::print_key_value_table(vec![
                ("Execution ID", summary.id.to_string()),
                ("Action", summary.action_ref.clone()),
                ("Status", output::format_status(&summary.status)),
                ("Created", output::format_timestamp(&summary.created)),
                ("Updated", output::format_timestamp(&summary.updated)),
            ]);

            let stdout = extract_stdout(&summary.result);
            if !suppress_final_stdout {
                if let Some(stdout) = &stdout {
                    output::print_section("Stdout");
                    println!("{}", stdout);
                }
            }

            if let Some(mut result) = summary.result {
                if stdout.is_some() {
                    if let Some(obj) = result.as_object_mut() {
                        obj.remove("stdout");
                    }
                }
                if !result.is_null() {
                    output::print_section("Result");
                    println!("{}", serde_json::to_string_pretty(&result)?);
                }
            }
        }
    }

    Ok(())
}
