use anyhow::{Context, Result};
use clap::Subcommand;
use eventsource_stream::Eventsource;
use futures::StreamExt;
use reqwest::header::{self, ACCEPT};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::{self, Write};
use std::time::Duration;

use crate::client::ApiClient;
use crate::config::CliConfig;
use crate::output::{self, OutputFormat};
use crate::wait::{
    extract_stdout, spawn_execution_output_watch, wait_for_execution, WaitOptions,
    OUTPUT_WATCH_DRAIN_GRACE,
};

#[derive(Subcommand)]
pub enum ExecutionCommands {
    /// List executions
    List {
        /// Filter by pack name
        #[arg(long)]
        pack: Option<String>,

        /// Filter by action reference
        #[arg(short, long)]
        action: Option<String>,

        /// Filter by rule reference
        #[arg(long)]
        rule: Option<String>,

        /// Filter by trigger reference
        #[arg(long)]
        trigger: Option<String>,

        /// Filter by status (repeat for multiple values)
        #[arg(short, long)]
        status: Vec<String>,

        /// Search in execution result (case-insensitive)
        #[arg(short, long)]
        result: Option<String>,

        /// Filter by exact trace tag
        #[arg(long)]
        trace_tag: Option<String>,

        /// Show only top-level executions
        #[arg(long)]
        top_level_only: bool,

        /// Limit number of results
        #[arg(short, long, default_value = "50")]
        limit: i32,
    },
    /// Watch executions live
    Watch {
        /// Execution ID to observe instead of watching the list
        execution_id: Option<i64>,

        /// Filter by pack name
        #[arg(long)]
        pack: Option<String>,

        /// Filter by action reference
        #[arg(short, long)]
        action: Option<String>,

        /// Filter by rule reference
        #[arg(long)]
        rule: Option<String>,

        /// Filter by trigger reference
        #[arg(long)]
        trigger: Option<String>,

        /// Filter by status (repeat for multiple values)
        #[arg(short, long)]
        status: Vec<String>,

        /// Search in execution result (case-insensitive)
        #[arg(short, long)]
        result: Option<String>,

        /// Filter by exact trace tag
        #[arg(long)]
        trace_tag: Option<String>,

        /// Show only top-level executions
        #[arg(long)]
        top_level_only: bool,

        /// Limit number of results shown
        #[arg(short, long, default_value = "50")]
        limit: i32,

        /// Timeout in seconds when observing (default: 300)
        #[arg(long, default_value = "300")]
        timeout: u64,

        /// Notifier WebSocket base URL (e.g. ws://localhost:8081).
        /// Derived from --api-url automatically when not set.
        #[arg(long)]
        notifier_url: Option<String>,
    },
    /// Show details of a specific execution
    Show {
        /// Execution ID
        execution_id: i64,
    },
    /// Show a cross-system activity report for an exact trace tag
    TraceReport {
        /// Exact trace tag
        trace_tag: String,
    },
    /// Show execution logs
    Logs {
        /// Execution ID
        execution_id: i64,

        /// Follow log output
        #[arg(short, long)]
        follow: bool,
    },
    /// Cancel a running execution
    Cancel {
        /// Execution ID
        execution_id: i64,

        /// Skip confirmation prompt
        #[arg(long)]
        yes: bool,
    },
    /// Get raw execution result
    Result {
        /// Execution ID
        execution_id: i64,

        /// Output format (json or yaml, default: json)
        #[arg(short = 'f', long, value_enum, default_value = "json")]
        format: ResultFormat,
    },
    /// Re-run an existing execution with the same (or edited) parameters
    Rerun {
        /// Execution ID to clone
        execution_id: i64,

        /// Interactively edit parameters before submitting
        #[arg(short, long)]
        interactive: bool,

        /// Override a single parameter (key=value, JSON or string).
        /// Repeat for multiple values. Applied on top of the original config.
        #[arg(long)]
        param: Vec<String>,

        /// Replace parameters entirely with this JSON object
        #[arg(long, conflicts_with_all = ["param", "interactive"])]
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

        /// Watch the new execution until it completes
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

#[derive(Debug, Clone, Copy, clap::ValueEnum)]
pub enum ResultFormat {
    Json,
    Yaml,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ExecutionSummaryRow {
    id: i64,
    action_ref: String,
    status: String,
    #[serde(default)]
    parent: Option<i64>,
    #[serde(default)]
    enforcement: Option<i64>,
    #[serde(default)]
    rule_ref: Option<String>,
    #[serde(default)]
    trigger_ref: Option<String>,
    #[serde(default)]
    trace_tag: Option<String>,
    #[serde(default)]
    result: Option<serde_json::Value>,
    created: String,
    #[serde(default)]
    updated: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ExecutionDetail {
    id: i64,
    #[serde(default)]
    action: Option<i64>,
    action_ref: String,
    #[serde(default)]
    config: Option<serde_json::Value>,
    status: String,
    #[serde(default)]
    result: Option<serde_json::Value>,
    #[serde(default)]
    parent: Option<i64>,
    #[serde(default)]
    enforcement: Option<i64>,
    #[serde(default)]
    executor: Option<i64>,
    #[serde(default)]
    timeout_seconds: Option<i32>,
    #[serde(default)]
    trace_tag: Option<String>,
    created: String,
    updated: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct ExecutionLogs {
    execution_id: i64,
    logs: Vec<LogEntry>,
}

#[derive(Debug, Serialize, Deserialize)]
struct LogEntry {
    timestamp: String,
    level: String,
    message: String,
}

#[derive(Debug, Deserialize)]
struct ExecutionStreamNotification {
    entity_id: i64,
    payload: ExecutionStreamPayload,
}

#[derive(Debug, Deserialize)]
struct ExecutionStreamPayload {
    #[serde(default)]
    id: Option<i64>,
    #[serde(default)]
    action_ref: Option<String>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    parent: Option<i64>,
    #[serde(default)]
    enforcement: Option<i64>,
    #[serde(default)]
    rule_ref: Option<String>,
    #[serde(default)]
    trigger_ref: Option<String>,
    #[serde(default)]
    trace_tag: Option<String>,
    #[serde(default)]
    result: Option<serde_json::Value>,
    #[serde(default)]
    created: Option<String>,
    #[serde(default)]
    updated: Option<String>,
}

#[derive(Debug, Clone)]
struct ExecutionListFilters {
    pack: Option<String>,
    action: Option<String>,
    rule: Option<String>,
    trigger: Option<String>,
    trace_tag: Option<String>,
    statuses: Vec<String>,
    result: Option<String>,
    top_level_only: bool,
    limit: usize,
}

#[derive(Debug)]
struct ExecutionListArgs {
    pack: Option<String>,
    action: Option<String>,
    rule: Option<String>,
    trigger: Option<String>,
    trace_tag: Option<String>,
    status: Vec<String>,
    result: Option<String>,
    top_level_only: bool,
    limit: i32,
}

impl ExecutionListFilters {
    fn from_args(args: ExecutionListArgs) -> Self {
        Self {
            pack: args.pack,
            action: args.action,
            rule: args.rule,
            trigger: args.trigger,
            trace_tag: args.trace_tag,
            statuses: normalize_statuses(args.status),
            result: args.result.map(|value| value.to_lowercase()),
            top_level_only: args.top_level_only,
            limit: args.limit.max(1) as usize,
        }
    }
}

pub async fn handle_execution_command(
    profile: &Option<String>,
    command: ExecutionCommands,
    api_url: &Option<String>,
    output_format: OutputFormat,
) -> Result<()> {
    match command {
        ExecutionCommands::List {
            pack,
            action,
            rule,
            trigger,
            trace_tag,
            status,
            result,
            top_level_only,
            limit,
        } => {
            let filters = ExecutionListFilters::from_args(ExecutionListArgs {
                pack,
                action,
                rule,
                trigger,
                trace_tag,
                status,
                result,
                top_level_only,
                limit,
            });
            handle_list(profile, filters, api_url, output_format).await
        }
        ExecutionCommands::Watch {
            execution_id,
            pack,
            action,
            rule,
            trigger,
            trace_tag,
            status,
            result,
            top_level_only,
            limit,
            timeout,
            notifier_url,
        } => {
            let args = ExecutionListArgs {
                pack,
                action,
                rule,
                trigger,
                trace_tag,
                status,
                result,
                top_level_only,
                limit,
            };
            if let Some(execution_id) = execution_id {
                ensure_watch_execution_mode_has_no_list_filters(&args)?;
                handle_watch_execution(
                    profile,
                    execution_id,
                    timeout,
                    notifier_url,
                    api_url,
                    output_format,
                )
                .await
            } else {
                let filters = ExecutionListFilters::from_args(args);
                handle_watch_list(profile, filters, api_url, output_format).await
            }
        }
        ExecutionCommands::Show { execution_id } => {
            handle_show(profile, execution_id, api_url, output_format).await
        }
        ExecutionCommands::TraceReport { trace_tag } => {
            handle_trace_report(profile, trace_tag, api_url, output_format).await
        }
        ExecutionCommands::Logs {
            execution_id,
            follow,
        } => handle_logs(profile, execution_id, follow, api_url, output_format).await,
        ExecutionCommands::Cancel { execution_id, yes } => {
            handle_cancel(profile, execution_id, yes, api_url, output_format).await
        }
        ExecutionCommands::Result {
            execution_id,
            format,
        } => handle_result(profile, execution_id, format, api_url).await,
        ExecutionCommands::Rerun {
            execution_id,
            interactive,
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
            handle_rerun(
                profile,
                execution_id,
                interactive,
                param,
                params_json,
                worker_selector,
                worker_tolerations,
                worker_affinity,
                execution_timeout,
                watch,
                timeout,
                notifier_url,
                api_url,
                output_format,
            )
            .await
        }
    }
}

async fn handle_list(
    profile: &Option<String>,
    filters: ExecutionListFilters,
    api_url: &Option<String>,
    output_format: OutputFormat,
) -> Result<()> {
    let config = CliConfig::load_with_profile(profile.as_deref())?;
    let mut client = ApiClient::from_config(&config, api_url);
    let executions = fetch_matching_executions(&mut client, &filters).await?;

    match output_format {
        OutputFormat::Json | OutputFormat::Yaml => {
            output::print_output(&executions, output_format)?;
        }
        OutputFormat::Table => {
            print_execution_rows(&executions);
        }
    }

    Ok(())
}

async fn handle_watch_list(
    profile: &Option<String>,
    filters: ExecutionListFilters,
    api_url: &Option<String>,
    output_format: OutputFormat,
) -> Result<()> {
    if output_format != OutputFormat::Table {
        anyhow::bail!("execution watch currently supports table output only");
    }

    let config = CliConfig::load_with_profile(profile.as_deref())?;
    let base_url = config.effective_api_url(api_url);
    let auth_token = config.auth_token()?;
    let mut client = ApiClient::from_config(&config, api_url);

    let initial = fetch_matching_executions(&mut client, &filters).await?;
    render_watch_table(&initial, &filters)?;

    match open_execution_stream(&base_url, auth_token.as_deref()).await {
        Ok(mut event_source) => {
            let mut executions: HashMap<i64, ExecutionSummaryRow> = initial
                .into_iter()
                .map(|execution| (execution.id, execution))
                .collect();

            loop {
                tokio::select! {
                    _ = tokio::signal::ctrl_c() => {
                        println!();
                        break;
                    }
                    message = event_source.next() => {
                        match message {
                            Some(Ok(event)) => {
                                if event.data.is_empty() {
                                    continue;
                                }

                                let notification: ExecutionStreamNotification = serde_json::from_str(&event.data)
                                    .context("Failed to parse execution stream notification")?;
                                if let Some(updated) = merge_execution_stream_payload(
                                    executions.get(&notification.entity_id),
                                    notification,
                                ) {
                                    if matches_execution_filters(&updated, &filters) {
                                        executions.insert(updated.id, updated);
                                    } else {
                                        executions.remove(&updated.id);
                                    }
                                    let rows = limit_execution_rows(executions.values().cloned().collect(), filters.limit);
                                    render_watch_table(&rows, &filters)?;
                                }
                            }
                            Some(Err(err)) => {
                                output::print_warning(&format!("Execution stream disconnected: {}", err));
                                break;
                            }
                            None => break,
                        }
                    }
                }
            }
        }
        Err(err) => {
            output::print_warning(&format!(
                "Live stream unavailable ({}); falling back to polling every 2s",
                err
            ));
            loop {
                tokio::select! {
                    _ = tokio::signal::ctrl_c() => {
                        println!();
                        break;
                    }
                    _ = tokio::time::sleep(Duration::from_secs(2)) => {
                        match fetch_matching_executions(&mut client, &filters).await {
                            Ok(rows) => {
                                render_watch_table(&rows, &filters)?;
                            }
                            Err(err) => {
                                output::print_warning(&format!("Execution watch refresh failed: {}", err));
                            }
                        }
                    }
                }
            }
        }
    }

    Ok(())
}

async fn handle_watch_execution(
    profile: &Option<String>,
    execution_id: i64,
    timeout: u64,
    notifier_url: Option<String>,
    api_url: &Option<String>,
    output_format: OutputFormat,
) -> Result<()> {
    let config = CliConfig::load_with_profile(profile.as_deref())?;
    let mut client = ApiClient::from_config(&config, api_url);

    if output_format == OutputFormat::Table {
        output::print_info(&format!("Observing execution {}...", execution_id));
    }

    let interactive_wait = true;
    let stream_live_logs = true;
    let debug_wait = false;
    let watch_task = Some(spawn_execution_output_watch(
        ApiClient::from_config(&config, api_url),
        execution_id,
        notifier_url.clone(),
        interactive_wait,
        stream_live_logs,
        debug_wait,
    ));
    let wait_result = wait_for_execution(WaitOptions {
        execution_id,
        timeout_secs: timeout,
        api_client: &mut client,
        notifier_ws_url: notifier_url,
        verbose: debug_wait,
    })
    .await;
    let watch_drain_grace = if wait_result.is_ok() {
        OUTPUT_WATCH_DRAIN_GRACE
    } else {
        Duration::ZERO
    };
    let (delivered_output, root_stdout_completed) = match watch_task {
        Some(task) => task.finish_or_stop(watch_drain_grace).await,
        None => (false, false),
    };
    let summary = wait_result?;
    let suppress_final_stdout = delivered_output && root_stdout_completed;

    render_watched_execution_summary(summary, output_format, suppress_final_stdout)
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

fn parse_param_overrides(params: &[String]) -> Result<Vec<(String, serde_json::Value)>> {
    let mut overrides = Vec::with_capacity(params.len());
    for p in params {
        let parts: Vec<&str> = p.splitn(2, '=').collect();
        if parts.len() != 2 {
            anyhow::bail!("Invalid parameter format: '{}'. Expected key=value", p);
        }
        let value: serde_json::Value = serde_json::from_str(parts[1])
            .unwrap_or_else(|_| serde_json::Value::String(parts[1].to_string()));
        overrides.push((parts[0].to_string(), value));
    }
    Ok(overrides)
}

/// Prompt the user to edit each existing parameter; allow skipping (keep
/// original) or removing (empty + confirm). Also offers to add new keys.
fn interactively_edit_parameters(
    initial: &serde_json::Map<String, serde_json::Value>,
) -> Result<serde_json::Map<String, serde_json::Value>> {
    use std::io::IsTerminal;

    if !std::io::stdin().is_terminal() {
        anyhow::bail!("--interactive requires a TTY");
    }

    output::print_info(
        "Edit parameters (press Enter to keep current value; values are parsed as JSON when possible):",
    );
    let mut result = serde_json::Map::new();
    for (key, value) in initial {
        let current = serde_json::to_string(value).unwrap_or_else(|_| value.to_string());
        let input: String = dialoguer::Input::new()
            .with_prompt(format!("  {}", key))
            .default(current.clone())
            .allow_empty(true)
            .interact_text()?;

        let new_value = if input.is_empty() {
            value.clone()
        } else {
            serde_json::from_str::<serde_json::Value>(&input)
                .unwrap_or(serde_json::Value::String(input))
        };
        result.insert(key.clone(), new_value);
    }

    loop {
        let add_more = dialoguer::Confirm::new()
            .with_prompt("Add another parameter?")
            .default(false)
            .interact()?;
        if !add_more {
            break;
        }
        let key: String = dialoguer::Input::new()
            .with_prompt("  key")
            .interact_text()?;
        if key.trim().is_empty() {
            continue;
        }
        let value_input: String = dialoguer::Input::new()
            .with_prompt("  value (JSON or string)")
            .allow_empty(true)
            .interact_text()?;
        let value = serde_json::from_str::<serde_json::Value>(&value_input)
            .unwrap_or(serde_json::Value::String(value_input));
        result.insert(key, value);
    }

    Ok(result)
}

#[allow(clippy::too_many_arguments)]
async fn handle_rerun(
    profile: &Option<String>,
    execution_id: i64,
    interactive: bool,
    params: Vec<String>,
    params_json: Option<String>,
    worker_selector: Option<String>,
    worker_tolerations: Option<String>,
    worker_affinity: Option<String>,
    execution_timeout: Option<i32>,
    watch: bool,
    timeout: u64,
    notifier_url: Option<String>,
    api_url: &Option<String>,
    output_format: OutputFormat,
) -> Result<()> {
    let config = CliConfig::load_with_profile(profile.as_deref())?;
    let mut client = ApiClient::from_config(&config, api_url);

    let original: ExecutionDetail = client
        .get(&format!("/executions/{}", execution_id))
        .await
        .with_context(|| format!("Failed to fetch execution {}", execution_id))?;

    if original.action_ref.is_empty() {
        anyhow::bail!(
            "Execution {} has no action_ref (may have been triggered by a deleted action)",
            execution_id
        );
    }

    let base_map = match original.config.clone() {
        Some(serde_json::Value::Object(map)) => map,
        Some(serde_json::Value::Null) | None => serde_json::Map::new(),
        Some(other) => {
            anyhow::bail!(
                "Original execution config is not a JSON object (got {}); cannot rerun",
                match other {
                    serde_json::Value::Array(_) => "array",
                    serde_json::Value::String(_) => "string",
                    serde_json::Value::Number(_) => "number",
                    serde_json::Value::Bool(_) => "bool",
                    _ => "unknown",
                }
            );
        }
    };

    let parameters: serde_json::Value = if let Some(json_str) = params_json {
        serde_json::from_str(&json_str).context("Invalid --params-json")?
    } else if interactive {
        serde_json::Value::Object(interactively_edit_parameters(&base_map)?)
    } else {
        let mut map = base_map;
        for (k, v) in parse_param_overrides(&params)? {
            map.insert(k, v);
        }
        serde_json::Value::Object(map)
    };

    if output_format == OutputFormat::Table {
        output::print_info(&format!(
            "Rerunning execution {} (action: {})",
            execution_id, original.action_ref
        ));
    }

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
        action_ref: original.action_ref.clone(),
        parameters,
        worker_selector: selector,
        worker_tolerations: tolerations,
        worker_affinity: affinity,
        timeout_seconds: execution_timeout,
    };

    let new_execution: ExecutionDetail = client.post("/executions/execute", &request).await?;

    if !watch {
        match output_format {
            OutputFormat::Json | OutputFormat::Yaml => {
                output::print_output(&new_execution, output_format)?;
            }
            OutputFormat::Table => {
                output::print_success(&format!(
                    "Execution {} started (rerun of {})",
                    new_execution.id, execution_id
                ));
                output::print_key_value_table(vec![
                    ("Execution ID", new_execution.id.to_string()),
                    ("Action", new_execution.action_ref.clone()),
                    ("Status", output::format_status(&new_execution.status)),
                    ("Rerun of", execution_id.to_string()),
                ]);
            }
        }
        return Ok(());
    }

    if output_format == OutputFormat::Table {
        output::print_info(&format!(
            "Waiting for execution {} to complete...",
            new_execution.id
        ));
    }

    let interactive_wait = true;
    let stream_live_logs = true;
    let debug_wait = false;
    let watch_task = Some(spawn_execution_output_watch(
        ApiClient::from_config(&config, api_url),
        new_execution.id,
        notifier_url.clone(),
        interactive_wait,
        stream_live_logs,
        debug_wait,
    ));
    let wait_result = wait_for_execution(WaitOptions {
        execution_id: new_execution.id,
        timeout_secs: timeout,
        api_client: &mut client,
        notifier_ws_url: notifier_url,
        verbose: debug_wait,
    })
    .await;
    let watch_drain_grace = if wait_result.is_ok() {
        OUTPUT_WATCH_DRAIN_GRACE
    } else {
        Duration::ZERO
    };
    let (delivered_output, root_stdout_completed) = match watch_task {
        Some(task) => task.finish_or_stop(watch_drain_grace).await,
        None => (false, false),
    };
    let summary = wait_result?;
    let suppress_final_stdout = delivered_output && root_stdout_completed;

    render_watched_execution_summary(summary, output_format, suppress_final_stdout)
}

fn ensure_watch_execution_mode_has_no_list_filters(args: &ExecutionListArgs) -> Result<()> {
    if args.pack.is_some()
        || args.action.is_some()
        || args.rule.is_some()
        || args.trigger.is_some()
        || args.trace_tag.is_some()
        || !args.status.is_empty()
        || args.result.is_some()
        || args.top_level_only
        || args.limit != 50
    {
        anyhow::bail!(
            "execution watch <id> observes a single execution and does not accept list-watch filters"
        );
    }

    Ok(())
}

async fn handle_show(
    profile: &Option<String>,
    execution_id: i64,
    api_url: &Option<String>,
    output_format: OutputFormat,
) -> Result<()> {
    let config = CliConfig::load_with_profile(profile.as_deref())?;
    let mut client = ApiClient::from_config(&config, api_url);

    let path = format!("/executions/{}", execution_id);
    let execution: ExecutionDetail = client.get(&path).await?;

    match output_format {
        OutputFormat::Json | OutputFormat::Yaml => {
            output::print_output(&execution, output_format)?;
        }
        OutputFormat::Table => {
            output::print_section(&format!("Execution: {}", execution.id));

            output::print_key_value_table(vec![
                ("ID", execution.id.to_string()),
                ("Action", execution.action_ref.clone()),
                ("Status", output::format_status(&execution.status)),
                (
                    "Trace Tag",
                    execution
                        .trace_tag
                        .clone()
                        .unwrap_or_else(|| "None".to_string()),
                ),
                ("Timeout", format_timeout_seconds(execution.timeout_seconds)),
                (
                    "Parent ID",
                    execution
                        .parent
                        .map(|id| id.to_string())
                        .unwrap_or_else(|| "None".to_string()),
                ),
                (
                    "Enforcement ID",
                    execution
                        .enforcement
                        .map(|id| id.to_string())
                        .unwrap_or_else(|| "None".to_string()),
                ),
                (
                    "Executor ID",
                    execution
                        .executor
                        .map(|id| id.to_string())
                        .unwrap_or_else(|| "None".to_string()),
                ),
                ("Created", output::format_timestamp(&execution.created)),
                ("Updated", output::format_timestamp(&execution.updated)),
            ]);

            if let Some(config) = execution.config {
                if !config.is_null() {
                    output::print_section("Configuration");
                    println!("{}", serde_json::to_string_pretty(&config)?);
                }
            }

            if let Some(result) = execution.result {
                if !result.is_null() {
                    output::print_section("Result");
                    println!("{}", serde_json::to_string_pretty(&result)?);
                }
            }
        }
    }

    Ok(())
}

async fn handle_trace_report(
    profile: &Option<String>,
    trace_tag: String,
    api_url: &Option<String>,
    output_format: OutputFormat,
) -> Result<()> {
    let config = CliConfig::load_with_profile(profile.as_deref())?;
    let mut client = ApiClient::from_config(&config, api_url);
    let path = format!("/traces/{}", urlencoding::encode(&trace_tag));
    let response: serde_json::Value = client.get(&path).await?;
    let report = response.get("data").cloned().unwrap_or(response);

    match output_format {
        OutputFormat::Json | OutputFormat::Yaml => output::print_output(&report, output_format)?,
        OutputFormat::Table => {
            let origins = report
                .get("origins")
                .and_then(|value| value.as_array())
                .map(|items| {
                    items
                        .iter()
                        .filter_map(|item| item.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                })
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "-".to_string());
            output::print_section(&format!("Trace Report: {}", trace_tag));
            output::print_key_value_table(vec![
                (
                    "Executions",
                    report
                        .get("executions")
                        .and_then(|v| v.as_array())
                        .map(|v| v.len().to_string())
                        .unwrap_or_else(|| "0".to_string()),
                ),
                (
                    "Enforcements",
                    report
                        .get("enforcements")
                        .and_then(|v| v.as_array())
                        .map(|v| v.len().to_string())
                        .unwrap_or_else(|| "0".to_string()),
                ),
                (
                    "Events",
                    report
                        .get("events")
                        .and_then(|v| v.as_array())
                        .map(|v| v.len().to_string())
                        .unwrap_or_else(|| "0".to_string()),
                ),
                (
                    "Queue Dispatches",
                    report
                        .get("queue_dispatches")
                        .and_then(|v| v.as_array())
                        .map(|v| v.len().to_string())
                        .unwrap_or_else(|| "0".to_string()),
                ),
                (
                    "Queue Items",
                    report
                        .get("queue_items")
                        .and_then(|v| v.as_array())
                        .map(|v| v.len().to_string())
                        .unwrap_or_else(|| "0".to_string()),
                ),
                ("Origins", origins),
            ]);
            output::print_output(&report, OutputFormat::Json)?;
        }
    }

    Ok(())
}

fn format_timeout_seconds(timeout_seconds: Option<i32>) -> String {
    match timeout_seconds {
        Some(seconds) if seconds > 0 => {
            let hours = seconds / 3600;
            let minutes = (seconds % 3600) / 60;
            let remaining_seconds = seconds % 60;

            let human = if hours > 0 {
                format!("{hours}h {minutes}m")
            } else if minutes > 0 {
                format!("{minutes}m {remaining_seconds}s")
            } else {
                format!("{remaining_seconds}s")
            };

            format!("{human} ({seconds}s)")
        }
        _ => "Not configured".to_string(),
    }
}

async fn handle_logs(
    profile: &Option<String>,
    execution_id: i64,
    follow: bool,
    api_url: &Option<String>,
    output_format: OutputFormat,
) -> Result<()> {
    let config = CliConfig::load_with_profile(profile.as_deref())?;
    let mut client = ApiClient::from_config(&config, api_url);

    let path = format!("/executions/{}/logs", execution_id);

    if follow {
        let mut last_count = 0;
        loop {
            let logs: ExecutionLogs = client.get(&path).await?;

            for log in logs.logs.iter().skip(last_count) {
                match output_format {
                    OutputFormat::Json => {
                        println!("{}", serde_json::to_string(log)?);
                    }
                    OutputFormat::Yaml => {
                        println!("{}", serde_yaml_ng::to_string(log)?);
                    }
                    OutputFormat::Table => {
                        println!(
                            "[{}] [{}] {}",
                            output::format_timestamp(&log.timestamp),
                            log.level.to_uppercase(),
                            log.message
                        );
                    }
                }
            }

            last_count = logs.logs.len();

            let exec_path = format!("/executions/{}", execution_id);
            let execution: ExecutionDetail = client.get(&exec_path).await?;
            let status_lower = execution.status.to_lowercase();
            if matches!(
                status_lower.as_str(),
                "succeeded" | "failed" | "canceled" | "cancelled"
            ) {
                break;
            }

            tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
        }
    } else {
        let logs: ExecutionLogs = client.get(&path).await?;

        match output_format {
            OutputFormat::Json | OutputFormat::Yaml => {
                output::print_output(&logs, output_format)?;
            }
            OutputFormat::Table => {
                if logs.logs.is_empty() {
                    output::print_info("No logs available");
                } else {
                    for log in logs.logs {
                        println!(
                            "[{}] [{}] {}",
                            output::format_timestamp(&log.timestamp),
                            log.level.to_uppercase(),
                            log.message
                        );
                    }
                }
            }
        }
    }

    Ok(())
}

async fn handle_result(
    profile: &Option<String>,
    execution_id: i64,
    format: ResultFormat,
    api_url: &Option<String>,
) -> Result<()> {
    let config = CliConfig::load_with_profile(profile.as_deref())?;
    let mut client = ApiClient::from_config(&config, api_url);

    let path = format!("/executions/{}", execution_id);
    let execution: ExecutionDetail = client.get(&path).await?;

    if let Some(result) = execution.result {
        match format {
            ResultFormat::Json => {
                println!("{}", serde_json::to_string_pretty(&result)?);
            }
            ResultFormat::Yaml => {
                println!("{}", serde_yaml_ng::to_string(&result)?);
            }
        }
    } else {
        anyhow::bail!("Execution {} has no result yet", execution_id);
    }

    Ok(())
}

async fn handle_cancel(
    profile: &Option<String>,
    execution_id: i64,
    yes: bool,
    api_url: &Option<String>,
    output_format: OutputFormat,
) -> Result<()> {
    let config = CliConfig::load_with_profile(profile.as_deref())?;
    let mut client = ApiClient::from_config(&config, api_url);

    if !yes && matches!(output_format, OutputFormat::Table) {
        let confirm = dialoguer::Confirm::new()
            .with_prompt(format!(
                "Are you sure you want to cancel execution {}?",
                execution_id
            ))
            .default(false)
            .interact()?;

        if !confirm {
            output::print_info("Cancellation aborted");
            return Ok(());
        }
    }

    let path = format!("/executions/{}/cancel", execution_id);
    let execution: ExecutionDetail = client.post(&path, &serde_json::json!({})).await?;

    match output_format {
        OutputFormat::Json | OutputFormat::Yaml => {
            output::print_output(&execution, output_format)?;
        }
        OutputFormat::Table => {
            output::print_success(&format!("Execution {} cancelled", execution_id));
        }
    }

    Ok(())
}

async fn fetch_matching_executions(
    client: &mut ApiClient,
    filters: &ExecutionListFilters,
) -> Result<Vec<ExecutionSummaryRow>> {
    let query = build_execution_query(filters);
    let path = if query.is_empty() {
        "/executions".to_string()
    } else {
        format!("/executions?{}", query)
    };
    let response: Vec<ExecutionSummaryRow> = client.get_paginated(&path).await?;
    Ok(limit_execution_rows(
        response
            .into_iter()
            .filter(|execution| matches_execution_filters(execution, filters))
            .collect(),
        filters.limit,
    ))
}

fn build_execution_query(filters: &ExecutionListFilters) -> String {
    let mut query_params = vec![format!("per_page={}", filters.limit)];

    if let Some(pack_name) = &filters.pack {
        query_params.push(format!("pack_name={}", urlencoding::encode(pack_name)));
    }
    if let Some(action_ref) = &filters.action {
        query_params.push(format!("action_ref={}", urlencoding::encode(action_ref)));
    }
    if let Some(rule_ref) = &filters.rule {
        query_params.push(format!("rule_ref={}", urlencoding::encode(rule_ref)));
    }
    if let Some(trigger_ref) = &filters.trigger {
        query_params.push(format!("trigger_ref={}", urlencoding::encode(trigger_ref)));
    }
    if let Some(trace_tag) = &filters.trace_tag {
        query_params.push(format!("trace_tag={}", urlencoding::encode(trace_tag)));
    }
    if filters.statuses.len() == 1 {
        query_params.push(format!(
            "status={}",
            urlencoding::encode(&filters.statuses[0])
        ));
    }
    if let Some(result_search) = &filters.result {
        query_params.push(format!(
            "result_contains={}",
            urlencoding::encode(result_search)
        ));
    }
    if filters.top_level_only {
        query_params.push("top_level_only=true".to_string());
    }

    query_params.join("&")
}

fn matches_execution_filters(
    execution: &ExecutionSummaryRow,
    filters: &ExecutionListFilters,
) -> bool {
    if filters.top_level_only && execution.parent.is_some() {
        return false;
    }

    if let Some(pack) = &filters.pack {
        let expected_prefix = format!("{pack}.");
        if !execution.action_ref.starts_with(&expected_prefix) {
            return false;
        }
    }

    if !matches_ref_filter(Some(&execution.action_ref), filters.action.as_deref()) {
        return false;
    }
    if !matches_ref_filter(execution.rule_ref.as_deref(), filters.rule.as_deref()) {
        return false;
    }
    if !matches_ref_filter(execution.trigger_ref.as_deref(), filters.trigger.as_deref()) {
        return false;
    }
    if filters.trace_tag.is_some() && execution.trace_tag != filters.trace_tag {
        return false;
    }

    if !filters.statuses.is_empty()
        && !filters
            .statuses
            .iter()
            .any(|status| status.eq_ignore_ascii_case(&execution.status))
    {
        return false;
    }

    if let Some(result_search) = &filters.result {
        let result_text = execution
            .result
            .as_ref()
            .map(|value| value.to_string().to_lowercase())
            .unwrap_or_default();
        if !result_text.contains(result_search) {
            return false;
        }
    }

    true
}

fn matches_ref_filter(actual: Option<&str>, filter: Option<&str>) -> bool {
    let Some(filter) = filter else {
        return true;
    };
    let Some(actual) = actual else {
        return false;
    };

    if let Some(prefix) = filter.strip_suffix(".*") {
        return !prefix.is_empty() && actual.starts_with(&format!("{prefix}."));
    }

    actual == filter
}

fn limit_execution_rows(
    mut rows: Vec<ExecutionSummaryRow>,
    limit: usize,
) -> Vec<ExecutionSummaryRow> {
    rows.sort_by(|left, right| {
        right
            .created
            .cmp(&left.created)
            .then_with(|| right.id.cmp(&left.id))
    });
    rows.truncate(limit);
    rows
}

fn merge_execution_stream_payload(
    existing: Option<&ExecutionSummaryRow>,
    notification: ExecutionStreamNotification,
) -> Option<ExecutionSummaryRow> {
    let mut merged = existing.cloned().unwrap_or(ExecutionSummaryRow {
        id: notification.entity_id,
        action_ref: String::new(),
        status: String::new(),
        parent: None,
        enforcement: None,
        rule_ref: None,
        trigger_ref: None,
        trace_tag: None,
        result: None,
        created: String::new(),
        updated: None,
    });

    let payload = notification.payload;
    merged.id = payload.id.unwrap_or(notification.entity_id);
    if let Some(action_ref) = payload.action_ref {
        merged.action_ref = action_ref;
    }
    if let Some(status) = payload.status {
        merged.status = status;
    }
    if let Some(parent) = payload.parent {
        merged.parent = Some(parent);
    }
    if let Some(enforcement) = payload.enforcement {
        merged.enforcement = Some(enforcement);
    }
    if let Some(rule_ref) = payload.rule_ref {
        merged.rule_ref = Some(rule_ref);
    }
    if let Some(trigger_ref) = payload.trigger_ref {
        merged.trigger_ref = Some(trigger_ref);
    }
    if let Some(trace_tag) = payload.trace_tag {
        merged.trace_tag = Some(trace_tag);
    }
    if let Some(result) = payload.result {
        merged.result = Some(result);
    }
    if let Some(created) = payload.created {
        merged.created = created;
    }
    if let Some(updated) = payload.updated {
        merged.updated = Some(updated);
    }

    if merged.action_ref.is_empty() || merged.status.is_empty() || merged.created.is_empty() {
        return None;
    }

    Some(merged)
}

async fn open_execution_stream(
    base_url: &str,
    auth_token: Option<&str>,
) -> Result<
    impl futures::Stream<
        Item = Result<
            eventsource_stream::Event,
            eventsource_stream::EventStreamError<reqwest::Error>,
        >,
    >,
> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(300))
        .build()
        .context("Failed to build execution stream HTTP client")?;

    let mut request = client
        .get(format!(
            "{}/api/v1/executions/stream",
            base_url.trim_end_matches('/')
        ))
        .header(ACCEPT, "text/event-stream");
    if let Some(token) = auth_token {
        request = request.header(header::AUTHORIZATION, format!("Bearer {}", token));
    }

    let response = request
        .send()
        .await
        .context("Failed to open execution stream")?;
    if !response.status().is_success() {
        anyhow::bail!("HTTP {}", response.status());
    }

    Ok(response.bytes_stream().eventsource())
}

fn render_watch_table(rows: &[ExecutionSummaryRow], filters: &ExecutionListFilters) -> Result<()> {
    print!("\x1B[2J\x1B[H");
    io::stdout().flush()?;

    println!("Watching executions");
    println!("Press Ctrl+C to stop");
    println!("Filters: {}", format_filter_summary(filters));
    println!();

    print_execution_rows(rows);
    io::stdout().flush()?;
    Ok(())
}

fn format_filter_summary(filters: &ExecutionListFilters) -> String {
    let mut parts = Vec::new();
    if let Some(pack) = &filters.pack {
        parts.push(format!("pack={pack}"));
    }
    if let Some(action) = &filters.action {
        parts.push(format!("action={action}"));
    }
    if let Some(rule) = &filters.rule {
        parts.push(format!("rule={rule}"));
    }
    if let Some(trigger) = &filters.trigger {
        parts.push(format!("trigger={trigger}"));
    }
    if let Some(trace_tag) = &filters.trace_tag {
        parts.push(format!("trace_tag={trace_tag}"));
    }
    if !filters.statuses.is_empty() {
        parts.push(format!("status={}", filters.statuses.join(",")));
    }
    if let Some(result) = &filters.result {
        parts.push(format!("result~={result}"));
    }
    parts.push(format!(
        "scope={}",
        if filters.top_level_only {
            "top-level"
        } else {
            "all"
        }
    ));
    parts.push(format!("limit={}", filters.limit));

    if parts.is_empty() {
        "none".to_string()
    } else {
        parts.join(" ")
    }
}

fn print_execution_rows(rows: &[ExecutionSummaryRow]) {
    if rows.is_empty() {
        output::print_info("No executions found");
        return;
    }

    let mut table = output::create_table();
    output::add_header(
        &mut table,
        vec![
            "ID",
            "Action",
            "Trace Tag",
            "Rule",
            "Trigger",
            "Status",
            "Created",
        ],
    );

    for execution in rows {
        table.add_row(vec![
            execution.id.to_string(),
            execution.action_ref.clone(),
            execution
                .trace_tag
                .clone()
                .unwrap_or_else(|| "-".to_string()),
            execution
                .rule_ref
                .clone()
                .unwrap_or_else(|| "-".to_string()),
            execution
                .trigger_ref
                .clone()
                .unwrap_or_else(|| "-".to_string()),
            output::format_status(&execution.status),
            output::format_timestamp(&execution.created),
        ]);
    }

    println!("{}", table);
}

fn render_watched_execution_summary(
    summary: crate::wait::ExecutionSummary,
    output_format: OutputFormat,
    suppress_final_stdout: bool,
) -> Result<()> {
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

fn normalize_statuses(statuses: Vec<String>) -> Vec<String> {
    statuses
        .into_iter()
        .map(|status| status.trim().to_lowercase())
        .filter(|status| !status.is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_execution_query_includes_new_filters() {
        let filters = ExecutionListFilters::from_args(ExecutionListArgs {
            pack: Some("core".to_string()),
            action: Some("core.echo".to_string()),
            rule: Some("core.rule".to_string()),
            trigger: Some("core.trigger".to_string()),
            trace_tag: Some("core.trigger.7".to_string()),
            status: vec!["running".to_string()],
            result: Some("boom".to_string()),
            top_level_only: true,
            limit: 25,
        });

        let query = build_execution_query(&filters);

        assert!(query.contains("per_page=25"));
        assert!(query.contains("pack_name=core"));
        assert!(query.contains("action_ref=core.echo"));
        assert!(query.contains("rule_ref=core.rule"));
        assert!(query.contains("trigger_ref=core.trigger"));
        assert!(query.contains("trace_tag=core.trigger.7"));
        assert!(query.contains("status=running"));
        assert!(query.contains("result_contains=boom"));
        assert!(query.contains("top_level_only=true"));
    }

    #[test]
    fn build_execution_query_omits_status_when_multiple_selected() {
        let filters = ExecutionListFilters::from_args(ExecutionListArgs {
            pack: None,
            action: None,
            rule: None,
            trigger: None,
            trace_tag: None,
            status: vec!["running".to_string(), "failed".to_string()],
            result: None,
            top_level_only: false,
            limit: 50,
        });

        let query = build_execution_query(&filters);

        assert!(!query.contains("status="));
    }

    #[test]
    fn matches_execution_filters_supports_wildcards_and_top_level() {
        let execution = ExecutionSummaryRow {
            id: 42,
            action_ref: "core.echo".to_string(),
            status: "running".to_string(),
            parent: None,
            enforcement: None,
            rule_ref: Some("core.on_timer".to_string()),
            trigger_ref: Some("core.timer".to_string()),
            trace_tag: None,
            result: Some(serde_json::json!({"message": "hello"})),
            created: "2024-01-01T00:00:00Z".to_string(),
            updated: Some("2024-01-01T00:00:00Z".to_string()),
        };

        let filters = ExecutionListFilters::from_args(ExecutionListArgs {
            pack: None,
            action: Some("core.*".to_string()),
            rule: Some("core.*".to_string()),
            trigger: Some("core.timer".to_string()),
            trace_tag: None,
            status: vec!["running".to_string(), "scheduled".to_string()],
            result: Some("hello".to_string()),
            top_level_only: true,
            limit: 50,
        });

        assert!(matches_execution_filters(&execution, &filters));
    }

    #[test]
    fn merge_execution_stream_payload_updates_existing_row() {
        let existing = ExecutionSummaryRow {
            id: 7,
            action_ref: "core.echo".to_string(),
            status: "running".to_string(),
            parent: None,
            enforcement: None,
            rule_ref: Some("core.rule".to_string()),
            trigger_ref: Some("core.trigger".to_string()),
            trace_tag: Some("core.trigger.7".to_string()),
            result: None,
            created: "2024-01-01T00:00:00Z".to_string(),
            updated: Some("2024-01-01T00:00:00Z".to_string()),
        };
        let notification = ExecutionStreamNotification {
            entity_id: 7,
            payload: ExecutionStreamPayload {
                id: None,
                action_ref: None,
                status: Some("completed".to_string()),
                parent: None,
                enforcement: None,
                rule_ref: None,
                trigger_ref: None,
                trace_tag: Some("core.trigger.7".to_string()),
                result: Some(serde_json::json!({"ok": true})),
                created: None,
                updated: Some("2024-01-01T00:01:00Z".to_string()),
            },
        };

        let merged = merge_execution_stream_payload(Some(&existing), notification).unwrap();
        assert_eq!(merged.status, "completed");
        assert_eq!(merged.result, Some(serde_json::json!({"ok": true})));
        assert_eq!(merged.action_ref, "core.echo");
        assert_eq!(merged.trace_tag.as_deref(), Some("core.trigger.7"));
    }
}
