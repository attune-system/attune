use anyhow::{anyhow, Context, Result};
use attune_cli::{client::ApiClient, config::CliConfig};
use axum::{
    extract::State,
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use clap::Parser;
use serde::Serialize;
use serde_json::{json, Map, Value};
use std::io::{self, BufRead, Write};
use std::sync::Arc;
use tokio::sync::Mutex;

#[derive(Parser, Debug)]
#[command(
    name = "attune-mcp",
    author,
    version,
    about = "MCP server exposing curated Attune platform tools over stdio or HTTP"
)]
struct Cli {
    /// Profile to use (overrides config)
    #[arg(short = 'p', long, env = "ATTUNE_PROFILE")]
    profile: Option<String>,

    /// API endpoint URL (overrides config)
    #[arg(long, env = "ATTUNE_API_URL")]
    api_url: Option<String>,

    /// Transport mode: stdio for local MCP clients, http for service deployment
    #[arg(long, env = "ATTUNE_MCP_TRANSPORT", default_value = "stdio")]
    transport: Transport,

    /// Listen address for the HTTP transport
    #[arg(long, env = "ATTUNE_MCP_LISTEN_ADDR", default_value = "0.0.0.0:8090")]
    listen_addr: String,

    /// Explicit Attune access token override
    #[arg(long, env = "ATTUNE_AUTH_TOKEN")]
    auth_token: Option<String>,

    /// Execution-scoped Attune API token override
    #[arg(long, env = "ATTUNE_API_TOKEN")]
    execution_token: Option<String>,

    /// Explicit Attune refresh token override
    #[arg(long, env = "ATTUNE_REFRESH_TOKEN")]
    refresh_token: Option<String>,

    /// Non-interactive login username/email for startup authentication
    #[arg(long, env = "ATTUNE_LOGIN")]
    login: Option<String>,

    /// Non-interactive login password for startup authentication
    #[arg(long, env = "ATTUNE_PASSWORD")]
    password: Option<String>,

    /// Verbose logging to stderr
    #[arg(short, long)]
    verbose: bool,
}

#[derive(Clone, Debug, clap::ValueEnum)]
enum Transport {
    Stdio,
    Http,
}

#[derive(Clone)]
struct ToolDef {
    name: &'static str,
    title: &'static str,
    description: &'static str,
    input_schema: fn() -> Value,
}

#[derive(Clone)]
struct LoginCredentials {
    api_url: String,
    login: String,
    password: String,
}

struct McpServer {
    client: ApiClient,
    /// Stored credentials for automatic re-login (None for execution token mode).
    credentials: Option<LoginCredentials>,
}

impl McpServer {
    fn new(client: ApiClient, credentials: Option<LoginCredentials>) -> Self {
        Self {
            client,
            credentials,
        }
    }

    async fn handle_request(&mut self, request: &Value) -> Result<Option<Value>> {
        let method = request
            .get("method")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("Missing JSON-RPC method"))?;
        let id = request.get("id").cloned();

        match method {
            "initialize" => {
                let protocol_version = request
                    .get("params")
                    .and_then(|params| params.get("protocolVersion"))
                    .and_then(Value::as_str)
                    .unwrap_or("2025-03-26");

                let result = json!({
                    "protocolVersion": protocol_version,
                    "capabilities": {
                        "tools": {
                            "listChanged": false
                        }
                    },
                    "serverInfo": {
                        "name": "attune-mcp",
                        "version": env!("CARGO_PKG_VERSION")
                    },
                    "instructions": "Use Attune tools for discovery, execution, queue interaction, artifacts, events, and inquiries. Event creation is intentionally omitted because Attune restricts direct event emission to sensor and execution token flows."
                });
                Ok(id.map(|id| success_response(id, result)))
            }
            "notifications/initialized" => Ok(None),
            "ping" => Ok(id.map(|id| success_response(id, json!({})))),
            "tools/list" => {
                let tools = tool_defs()
                    .iter()
                    .map(|tool| {
                        json!({
                            "name": tool.name,
                            "title": tool.title,
                            "description": tool.description,
                            "inputSchema": (tool.input_schema)(),
                        })
                    })
                    .collect::<Vec<_>>();
                Ok(id.map(|id| success_response(id, json!({ "tools": tools }))))
            }
            "tools/call" => {
                let params = request
                    .get("params")
                    .and_then(Value::as_object)
                    .ok_or_else(|| anyhow!("Missing tools/call params"))?;
                let tool_name = params
                    .get("name")
                    .and_then(Value::as_str)
                    .ok_or_else(|| anyhow!("Missing tool name"))?;
                let args = params
                    .get("arguments")
                    .and_then(Value::as_object)
                    .cloned()
                    .unwrap_or_default();

                let tool_result = match self.call_tool_with_reauth(tool_name, &args).await {
                    Ok(value) => tool_success(value),
                    Err(error) => tool_error(error.to_string()),
                };

                Ok(id.map(|id| success_response(id, tool_result)))
            }
            "resources/list" => Ok(id.map(|id| success_response(id, json!({ "resources": [] })))),
            "prompts/list" => Ok(id.map(|id| success_response(id, json!({ "prompts": [] })))),
            other => Ok(id.map(|id| method_not_found_response(id, other))),
        }
    }

    /// Call a tool with automatic re-authentication on auth failures.
    ///
    /// The inner `ApiClient` already attempts a token refresh via the stored
    /// refresh token.  This wrapper adds a second layer: if the call still fails
    /// with an authentication error AND we have stored login credentials (i.e.
    /// we're not running with an execution token), perform a full re-login and
    /// retry the tool call once.
    async fn call_tool_with_reauth(
        &mut self,
        tool_name: &str,
        args: &Map<String, Value>,
    ) -> Result<Value> {
        match self.call_tool(tool_name, args).await {
            Ok(value) => Ok(value),
            Err(err) if Self::is_auth_error(&err) => {
                if let Some(creds) = &self.credentials {
                    tracing::info!("Tool call failed with auth error, attempting re-login");
                    let tokens =
                        login_with_password(&creds.api_url, &creds.login, &creds.password).await?;
                    self.client
                        .set_tokens(tokens.access_token, tokens.refresh_token);
                    // Retry once with fresh tokens
                    self.call_tool(tool_name, args).await
                } else {
                    Err(err)
                }
            }
            Err(err) => Err(err),
        }
    }

    /// Check if an error looks like an authentication/authorization failure.
    fn is_auth_error(err: &anyhow::Error) -> bool {
        let msg = err.to_string();
        msg.contains("401") || msg.contains("Unauthorized") || msg.contains("token expired")
    }

    async fn call_tool(&mut self, tool_name: &str, args: &Map<String, Value>) -> Result<Value> {
        match tool_name {
            "actions_list" => self.list_path("/actions", args).await,
            "actions_search" => self.actions_search(args).await,
            "actions_get" => {
                let action_ref = required_string(args, "ref")?;
                self.client
                    .get::<Value>(&format!("/actions/{}", encode_path(action_ref)))
                    .await
            }
            "actions_execute" => {
                let action_ref = required_string(args, "action_ref")?;
                let parameters = optional_object(args, "parameters")?;
                let env_vars = optional_object(args, "env_vars")?;
                self.client
                    .post::<Value, _>(
                        "/executions/execute",
                        &json!({
                            "action_ref": action_ref,
                            "parameters": parameters,
                            "env_vars": env_vars
                        }),
                    )
                    .await
            }
            "artifacts_list" => self.list_path("/artifacts", args).await,
            "artifacts_get" => {
                let artifact_ref = required_string(args, "ref")?;
                self.client
                    .get::<Value>(&format!("/artifacts/ref/{}", encode_path(artifact_ref)))
                    .await
            }
            "events_list" => self.list_path("/events", args).await,
            "events_get" => {
                let id = required_i64(args, "id")?;
                self.client.get::<Value>(&format!("/events/{id}")).await
            }
            "executions_get" => {
                let id = required_i64(args, "id")?;
                self.client.get::<Value>(&format!("/executions/{id}")).await
            }
            "executions_list" => self.executions_list(args).await,
            "traces_get_report" => {
                let trace_tag = required_string(args, "trace_tag")?;
                self.client
                    .get::<Value>(&format!("/traces/{}", encode_path(trace_tag)))
                    .await
            }
            "executions_cancel" => {
                let id = required_i64(args, "id")?;
                self.client
                    .post::<Value, _>(&format!("/executions/{id}/cancel"), &json!({}))
                    .await
            }
            "rules_get" => {
                let rule_ref = required_string(args, "ref")?;
                self.client
                    .get::<Value>(&format!("/rules/{}", encode_path(rule_ref)))
                    .await
            }
            "rules_update_trace_tag_template" => {
                let rule_ref = required_string(args, "ref")?;
                let trace_tag_template = optional_nullable_string(args, "trace_tag_template")?;
                self.client
                    .put::<Value, _>(
                        &format!("/rules/{}", encode_path(rule_ref)),
                        &json!({
                            "trace_tag_template": trace_tag_template
                        }),
                    )
                    .await
            }
            "inquiries_list" => self.list_path("/inquiries", args).await,
            "inquiries_respond" => {
                let id = required_i64(args, "id")?;
                let response = args
                    .get("response")
                    .cloned()
                    .ok_or_else(|| anyhow!("Missing required argument 'response'"))?;
                self.client
                    .post::<Value, _>(
                        &format!("/inquiries/{id}/respond"),
                        &json!({ "response": response }),
                    )
                    .await
            }
            "queues_list" => self.list_path("/queues", args).await,
            "queues_get" => {
                let queue_ref = required_string(args, "ref")?;
                self.client
                    .get::<Value>(&format!("/queues/{}", encode_path(queue_ref)))
                    .await
            }
            "queues_update_trace_tag_template" => {
                let queue_ref = required_string(args, "ref")?;
                let trace_tag_template = optional_nullable_string(args, "trace_tag_template")?;
                self.client
                    .put::<Value, _>(
                        &format!("/queues/{}", encode_path(queue_ref)),
                        &json!({
                            "trace_tag_template": trace_tag_template
                        }),
                    )
                    .await
            }
            "queues_enqueue" => {
                let queue_ref = required_string(args, "ref")?;
                let payload = args
                    .get("payload")
                    .cloned()
                    .ok_or_else(|| anyhow!("Missing required argument 'payload'"))?;
                let item_key = optional_string(args, "item_key");
                let priority = optional_i64(args, "priority")?;
                let metadata = optional_value(args, "metadata");
                self.client
                    .post::<Value, _>(
                        &format!("/queues/{}/items", encode_path(queue_ref)),
                        &json!({
                            "item_key": item_key,
                            "priority": priority,
                            "payload": payload,
                            "metadata": metadata
                        }),
                    )
                    .await
            }
            "workflows_list" => self.list_path("/workflows", args).await,
            "workflows_get" => {
                let workflow_ref = required_string(args, "ref")?;
                self.client
                    .get::<Value>(&format!("/workflows/{}", encode_path(workflow_ref)))
                    .await
            }
            "packs_list" => self.list_path("/packs", args).await,
            "packs_get" => {
                let pack_ref = required_string(args, "ref")?;
                self.client
                    .get::<Value>(&format!("/packs/{}", encode_path(pack_ref)))
                    .await
            }
            "packs_update_config" => {
                let pack_ref = required_string(args, "ref")?;
                let config = args
                    .get("config")
                    .cloned()
                    .ok_or_else(|| anyhow!("Missing required argument 'config'"))?;
                self.client
                    .put::<Value, _>(
                        &format!("/packs/{}", encode_path(pack_ref)),
                        &json!({ "config": config }),
                    )
                    .await
            }
            "packs_get_actions" => {
                let pack_ref = required_string(args, "ref")?;
                self.client
                    .get_paginated::<Value>(&format!(
                        "/actions/search?packs={}&page=1&page_size=100",
                        urlencoding::encode(pack_ref)
                    ))
                    .await
                    .map(Value::Array)
            }
            other => Err(anyhow!("Unknown tool '{other}'")),
        }
    }

    async fn list_path(&mut self, path: &str, args: &Map<String, Value>) -> Result<Value> {
        let page = optional_i64(args, "page")?.unwrap_or(1);
        let per_page = optional_i64(args, "per_page")?.unwrap_or(100);
        self.client
            .get_paginated::<Value>(&format!("{path}?page={page}&per_page={per_page}"))
            .await
            .map(Value::Array)
    }

    async fn actions_search(&mut self, args: &Map<String, Value>) -> Result<Value> {
        let mut params: Vec<(&str, String)> = Vec::new();
        if let Some(q) = optional_string(args, "q") {
            if !q.trim().is_empty() {
                params.push(("q", q));
            }
        }
        // packs: accept a JSON array of strings or a comma-separated string.
        if let Some(value) = args.get("packs") {
            let packs_csv = match value {
                Value::Array(items) => items
                    .iter()
                    .filter_map(|v| v.as_str().map(|s| s.trim().to_string()))
                    .filter(|s| !s.is_empty())
                    .collect::<Vec<_>>()
                    .join(","),
                Value::String(s) => s.clone(),
                Value::Null => String::new(),
                _ => {
                    return Err(anyhow!(
                        "Argument 'packs' must be an array of strings or a comma-separated string"
                    ))
                }
            };
            if !packs_csv.is_empty() {
                params.push(("packs", packs_csv));
            }
        }
        let limit = optional_i64(args, "limit")?.unwrap_or(50).clamp(1, 100);
        params.push(("page", "1".to_string()));
        params.push(("page_size", limit.to_string()));

        let qs = params
            .iter()
            .map(|(k, v)| format!("{}={}", k, urlencoding::encode(v)))
            .collect::<Vec<_>>()
            .join("&");
        self.client
            .get_paginated::<Value>(&format!("/actions/search?{qs}"))
            .await
            .map(Value::Array)
    }

    async fn executions_list(&mut self, args: &Map<String, Value>) -> Result<Value> {
        let page = optional_i64(args, "page")?.unwrap_or(1);
        let per_page = optional_i64(args, "per_page")?.unwrap_or(20);
        let mut qs = format!("page={page}&per_page={per_page}");
        if let Some(status) = optional_string(args, "status") {
            qs.push_str(&format!("&status={}", urlencoding::encode(&status)));
        }
        if let Some(action_ref) = optional_string(args, "action_ref") {
            qs.push_str(&format!("&action_ref={}", urlencoding::encode(&action_ref)));
        }
        if let Some(trace_tag) = optional_string(args, "trace_tag") {
            qs.push_str(&format!("&trace_tag={}", urlencoding::encode(&trace_tag)));
        }
        if let Some(top_level) = args.get("top_level_only").and_then(|v| v.as_bool()) {
            if top_level {
                qs.push_str("&top_level_only=true");
            }
        }
        self.client
            .get_paginated::<Value>(&format!("/executions?{qs}"))
            .await
            .map(Value::Array)
    }
}

fn tool_defs() -> &'static [ToolDef] {
    &[
        ToolDef {
            name: "actions_list",
            title: "List actions",
            description: "List Attune actions visible to the authenticated user.",
            input_schema: pagination_schema,
        },
        ToolDef {
            name: "actions_search",
            title: "Search actions",
            description:
                "Search Attune actions by keyword (whitespace-separated tokens AND-matched against \
                 ref, label, description, and pack ref). Optionally restrict to one or more pack \
                 refs. Returns lean hits to keep agent context light.",
            input_schema: actions_search_schema,
        },
        ToolDef {
            name: "actions_get",
            title: "Get action",
            description: "Fetch detailed metadata for a single action by ref.",
            input_schema: ref_schema,
        },
        ToolDef {
            name: "actions_execute",
            title: "Execute action",
            description:
                "Create and queue an execution for an Attune action with structured parameters.",
            input_schema: action_execute_schema,
        },
        ToolDef {
            name: "artifacts_list",
            title: "List artifacts",
            description: "List artifacts visible to the authenticated user.",
            input_schema: pagination_schema,
        },
        ToolDef {
            name: "artifacts_get",
            title: "Get artifact",
            description: "Fetch a single artifact by ref.",
            input_schema: ref_schema,
        },
        ToolDef {
            name: "events_list",
            title: "List events",
            description: "List recorded Attune events for observability and correlation.",
            input_schema: pagination_schema,
        },
        ToolDef {
            name: "events_get",
            title: "Get event",
            description: "Fetch a single recorded event by numeric ID.",
            input_schema: id_schema,
        },
        ToolDef {
            name: "executions_get",
            title: "Get execution",
            description: "Fetch a single execution by numeric ID.",
            input_schema: id_schema,
        },
        ToolDef {
            name: "executions_list",
            title: "List executions",
            description: "List recent executions with optional filtering by status, action_ref, trace_tag, or top-level only. Useful for monitoring action runs and debugging.",
            input_schema: executions_list_schema,
        },
        ToolDef {
            name: "traces_get_report",
            title: "Get trace report",
            description: "Fetch a full cross-system activity report for an exact trace tag.",
            input_schema: trace_report_schema,
        },
        ToolDef {
            name: "executions_cancel",
            title: "Cancel execution",
            description: "Request cancellation for a queued or running execution.",
            input_schema: id_schema,
        },
        ToolDef {
            name: "rules_get",
            title: "Get rule",
            description: "Fetch a single rule definition by ref.",
            input_schema: ref_schema,
        },
        ToolDef {
            name: "rules_update_trace_tag_template",
            title: "Update rule trace tag template",
            description:
                "Set or clear a rule trace_tag_template used for executions created from that rule.",
            input_schema: ref_with_trace_tag_schema,
        },
        ToolDef {
            name: "inquiries_list",
            title: "List inquiries",
            description: "List inquiries that require or record human responses.",
            input_schema: pagination_schema,
        },
        ToolDef {
            name: "inquiries_respond",
            title: "Respond to inquiry",
            description: "Submit a structured response to a pending inquiry.",
            input_schema: inquiry_respond_schema,
        },
        ToolDef {
            name: "queues_list",
            title: "List queues",
            description: "List work queue definitions visible to the authenticated user.",
            input_schema: pagination_schema,
        },
        ToolDef {
            name: "queues_get",
            title: "Get queue",
            description: "Fetch a single work queue definition by ref.",
            input_schema: ref_schema,
        },
        ToolDef {
            name: "queues_update_trace_tag_template",
            title: "Update queue trace tag template",
            description:
                "Set or clear a queue trace_tag_template used for dispatch executions.",
            input_schema: ref_with_trace_tag_schema,
        },
        ToolDef {
            name: "queues_enqueue",
            title: "Enqueue queue item",
            description:
                "Submit a new work item into a queue-backed Attune workflow or session inbox.",
            input_schema: queue_enqueue_schema,
        },
        ToolDef {
            name: "workflows_list",
            title: "List workflows",
            description: "List workflow definitions visible to the authenticated user.",
            input_schema: pagination_schema,
        },
        ToolDef {
            name: "workflows_get",
            title: "Get workflow",
            description: "Fetch a single workflow definition by ref.",
            input_schema: ref_schema,
        },
        ToolDef {
            name: "packs_list",
            title: "List packs",
            description: "List installed Attune packs visible to the authenticated user.",
            input_schema: pagination_schema,
        },
        ToolDef {
            name: "packs_get",
            title: "Get pack",
            description: "Fetch detailed metadata for a single pack by ref, including its configuration schema and current configuration values.",
            input_schema: ref_schema,
        },
        ToolDef {
            name: "packs_update_config",
            title: "Update pack configuration",
            description: "Update the configuration values for a pack. The config object is merged with the pack's conf_schema. Requires packs:configure permission.",
            input_schema: packs_update_config_schema,
        },
        ToolDef {
            name: "packs_get_actions",
            title: "List pack actions",
            description: "List all actions belonging to a specific pack by ref.",
            input_schema: ref_schema,
        },
    ]
}

fn pagination_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "page": { "type": "integer", "minimum": 1, "description": "1-based page number" },
            "per_page": { "type": "integer", "minimum": 1, "maximum": 100, "description": "Page size" }
        },
        "additionalProperties": false
    })
}

fn ref_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "ref": { "type": "string", "description": "Attune reference identifier" }
        },
        "required": ["ref"],
        "additionalProperties": false
    })
}

fn id_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "id": { "type": "integer", "description": "Numeric database ID" }
        },
        "required": ["id"],
        "additionalProperties": false
    })
}

fn action_execute_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "action_ref": { "type": "string", "description": "Action ref, for example core.echo" },
            "parameters": { "type": "object", "description": "Structured action parameters", "additionalProperties": true },
            "env_vars": { "type": "object", "description": "Optional execution environment variables", "additionalProperties": { "type": "string" } }
        },
        "required": ["action_ref"],
        "additionalProperties": false
    })
}

fn actions_search_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "q": { "type": "string", "description": "Keyword query. Whitespace-separated tokens are AND-matched against ref, label, description, and pack ref (case-insensitive substring)." },
            "packs": {
                "description": "Optional pack ref filter. Either an array of pack refs (e.g. [\"core\", \"slack\"]) or a comma-separated string (\"core,slack\").",
                "oneOf": [
                    { "type": "array", "items": { "type": "string" } },
                    { "type": "string" }
                ]
            },
            "limit": { "type": "integer", "minimum": 1, "maximum": 100, "description": "Max number of hits to return (default 50)." }
        },
        "additionalProperties": false
    })
}

fn queue_enqueue_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "ref": { "type": "string", "description": "Queue ref" },
            "item_key": { "type": "string", "description": "Optional idempotency or coalescing key" },
            "priority": { "type": "integer", "description": "Optional explicit item priority" },
            "payload": { "description": "Queue item payload" },
            "metadata": { "type": "object", "description": "Optional queue item metadata", "additionalProperties": true }
        },
        "required": ["ref", "payload"],
        "additionalProperties": false
    })
}

fn inquiry_respond_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "id": { "type": "integer", "description": "Inquiry ID" },
            "response": { "description": "Structured inquiry response payload" }
        },
        "required": ["id", "response"],
        "additionalProperties": false
    })
}

fn packs_update_config_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "ref": { "type": "string", "description": "Pack reference identifier (e.g. \"slack\", \"core\")" },
            "config": {
                "type": "object",
                "description": "Configuration values to set on the pack. Keys must match the pack's conf_schema. Pass an empty object {} to clear all config values.",
                "additionalProperties": true
            }
        },
        "required": ["ref", "config"],
        "additionalProperties": false
    })
}

fn executions_list_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "page": { "type": "integer", "minimum": 1, "description": "1-based page number" },
            "per_page": { "type": "integer", "minimum": 1, "maximum": 100, "description": "Page size (default 20)" },
            "status": { "type": "string", "description": "Filter by execution status (e.g. running, completed, failed, timeout, cancelled)" },
            "action_ref": { "type": "string", "description": "Filter by action ref (exact match)" },
            "trace_tag": { "type": "string", "description": "Filter by exact trace tag" },
            "top_level_only": { "type": "boolean", "description": "If true, exclude workflow child executions" }
        },
        "additionalProperties": false
    })
}

fn ref_with_trace_tag_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "ref": { "type": "string", "description": "Attune reference identifier" },
            "trace_tag_template": {
                "type": ["string", "null"],
                "description": "Template string to set, or null to clear"
            }
        },
        "required": ["ref", "trace_tag_template"],
        "additionalProperties": false
    })
}

fn trace_report_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "trace_tag": { "type": "string", "description": "Exact trace tag" }
        },
        "required": ["trace_tag"],
        "additionalProperties": false
    })
}

fn success_response(id: Value, result: Value) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": result
    })
}

fn error_response(id: Option<Value>, code: i64, message: impl Into<String>) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id.unwrap_or(Value::Null),
        "error": {
            "code": code,
            "message": message.into()
        }
    })
}

fn method_not_found_response(id: Value, method: &str) -> Value {
    error_response(Some(id), -32601, format!("Method not found: {method}"))
}

fn tool_success(value: Value) -> Value {
    let text = serde_json::to_string_pretty(&value).unwrap_or_else(|_| value.to_string());
    // structuredContent must be a JSON object per MCP spec.
    // Wrap arrays in {"items": [...]} so the schema is always satisfied.
    let structured = match &value {
        Value::Object(_) => value,
        Value::Array(_) => json!({ "items": value }),
        _ => json!({ "value": value }),
    };
    json!({
        "content": [
            {
                "type": "text",
                "text": text
            }
        ],
        "structuredContent": structured,
        "isError": false
    })
}

fn tool_error(message: String) -> Value {
    json!({
        "content": [
            {
                "type": "text",
                "text": message
            }
        ],
        "isError": true
    })
}

fn required_string<'a>(args: &'a Map<String, Value>, key: &str) -> Result<&'a str> {
    args.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("Missing required string argument '{key}'"))
}

fn optional_string(args: &Map<String, Value>, key: &str) -> Option<String> {
    args.get(key).and_then(Value::as_str).map(ToOwned::to_owned)
}

fn optional_nullable_string(args: &Map<String, Value>, key: &str) -> Result<Option<String>> {
    match args.get(key) {
        Some(Value::String(value)) => Ok(Some(value.clone())),
        Some(Value::Null) => Ok(None),
        Some(_) => Err(anyhow!("Argument '{key}' must be a string or null")),
        None => Err(anyhow!("Missing required argument '{key}'")),
    }
}

fn required_i64(args: &Map<String, Value>, key: &str) -> Result<i64> {
    args.get(key)
        .and_then(Value::as_i64)
        .ok_or_else(|| anyhow!("Missing required integer argument '{key}'"))
}

fn optional_i64(args: &Map<String, Value>, key: &str) -> Result<Option<i64>> {
    match args.get(key) {
        Some(value) => value
            .as_i64()
            .map(Some)
            .ok_or_else(|| anyhow!("Argument '{key}' must be an integer")),
        None => Ok(None),
    }
}

fn optional_object(args: &Map<String, Value>, key: &str) -> Result<Option<Value>> {
    match args.get(key) {
        Some(Value::Object(map)) => Ok(Some(Value::Object(map.clone()))),
        Some(Value::Null) => Ok(None),
        Some(_) => Err(anyhow!("Argument '{key}' must be an object")),
        None => Ok(None),
    }
}

fn optional_value(args: &Map<String, Value>, key: &str) -> Option<Value> {
    args.get(key).cloned()
}

fn encode_path(value: &str) -> String {
    urlencoding::encode(value).into_owned()
}

fn read_message(reader: &mut impl BufRead) -> Result<Option<Vec<u8>>> {
    loop {
        let mut line = String::new();
        let bytes_read = reader.read_line(&mut line)?;
        if bytes_read == 0 {
            return Ok(None);
        }

        let trimmed = line.trim_matches(|c: char| c == '\r' || c == '\n');
        if trimmed.is_empty() {
            continue;
        }

        return Ok(Some(trimmed.as_bytes().to_vec()));
    }
}

fn write_message(writer: &mut impl Write, value: &impl Serialize) -> Result<()> {
    let body = serde_json::to_vec(value).context("Failed to encode JSON-RPC response")?;
    writer.write_all(&body)?;
    writer.write_all(b"\n")?;
    writer.flush()?;
    Ok(())
}

#[derive(serde::Serialize)]
struct LoginRequest {
    login: String,
    password: String,
}

#[derive(serde::Deserialize)]
struct TokenResponse {
    access_token: String,
    refresh_token: String,
}

#[derive(serde::Deserialize)]
struct WrappedResponse<T> {
    data: T,
}

enum AuthMode {
    ExecutionToken,
    ExplicitToken,
    StartupLogin,
    ProfileToken,
    Anonymous,
}

async fn login_with_password(api_url: &str, login: &str, password: &str) -> Result<TokenResponse> {
    let response = reqwest::Client::new()
        .post(format!("{api_url}/auth/login"))
        .json(&LoginRequest {
            login: login.to_string(),
            password: password.to_string(),
        })
        .send()
        .await
        .context("Failed to send Attune login request")?;

    let status = response.status();
    if !status.is_success() {
        let body = response
            .text()
            .await
            .unwrap_or_else(|_| "Unknown error".to_string());
        anyhow::bail!("Attune login failed ({status}): {body}");
    }

    response
        .json::<WrappedResponse<TokenResponse>>()
        .await
        .map(|wrapped| wrapped.data)
        .context("Failed to parse Attune login response")
}

fn build_config(cli: &Cli) -> Result<CliConfig> {
    let mut config = if cli.profile.is_some() {
        // A profile was explicitly requested (via --profile or ATTUNE_PROFILE).
        // Propagate any error so that a missing profile is reported rather than
        // silently falling back to defaults.
        CliConfig::load_with_profile(cli.profile.as_deref())?
    } else {
        // No profile override — fall back to defaults on a fresh install.
        CliConfig::load_with_profile(None).unwrap_or_default()
    };
    ensure_current_profile_exists(&mut config);

    if let Some(auth_token) = &cli.auth_token {
        config.current_profile_mut()?.auth_token = Some(auth_token.clone());
    }
    if let Some(refresh_token) = &cli.refresh_token {
        config.current_profile_mut()?.refresh_token = Some(refresh_token.clone());
    }
    if let Some(execution_token) = &cli.execution_token {
        let profile = config.current_profile_mut()?;
        profile.auth_token = Some(execution_token.clone());
        profile.refresh_token = None;
    }

    Ok(config)
}

fn ensure_current_profile_exists(config: &mut CliConfig) {
    if config.profiles.contains_key(&config.current_profile) {
        return;
    }

    let default_config = CliConfig::default();
    let fallback_profile = default_config
        .profiles
        .get("default")
        .expect("default CLI config must include a default profile")
        .clone();
    config
        .profiles
        .insert(config.current_profile.clone(), fallback_profile);
}

fn selected_auth_mode(cli: &Cli, config: &CliConfig) -> Result<AuthMode> {
    if cli.execution_token.is_some() {
        return Ok(AuthMode::ExecutionToken);
    }
    if cli.auth_token.is_some() {
        return Ok(AuthMode::ExplicitToken);
    }
    if cli.login.is_some() || cli.password.is_some() {
        return Ok(AuthMode::StartupLogin);
    }
    if config.auth_token()?.is_some() {
        return Ok(AuthMode::ProfileToken);
    }
    Ok(AuthMode::Anonymous)
}

async fn build_server(cli: &Cli) -> Result<McpServer> {
    let mut config = build_config(cli)?;
    let effective_api_url = config.effective_api_url(&cli.api_url);
    let auth_mode = selected_auth_mode(cli, &config)?;

    // Store credentials for automatic re-login (only for login/password mode, not execution tokens)
    let credentials = match &auth_mode {
        AuthMode::ExecutionToken => None,
        _ => match (cli.login.as_deref(), cli.password.as_deref()) {
            (Some(login), Some(password)) => Some(LoginCredentials {
                api_url: effective_api_url.clone(),
                login: login.to_string(),
                password: password.to_string(),
            }),
            _ => None,
        },
    };

    if config.auth_token()?.is_none() {
        match (cli.login.as_deref(), cli.password.as_deref()) {
            (Some(login), Some(password)) => {
                let tokens = login_with_password(&effective_api_url, login, password).await?;
                let profile = config.current_profile_mut()?;
                profile.auth_token = Some(tokens.access_token);
                profile.refresh_token = Some(tokens.refresh_token);
            }
            (Some(_), None) | (None, Some(_)) => {
                anyhow::bail!(
                    "ATTUNE_LOGIN and ATTUNE_PASSWORD must both be set when using startup login"
                );
            }
            (None, None) => {}
        }
    }

    tracing::info!(
        api_url = %effective_api_url,
        transport = ?cli.transport,
        auth_mode = %match auth_mode {
            AuthMode::ExecutionToken => "execution_token",
            AuthMode::ExplicitToken => "explicit_token",
            AuthMode::StartupLogin => "startup_login",
            AuthMode::ProfileToken => "profile_token",
            AuthMode::Anonymous => "anonymous",
        },
        "Starting Attune MCP server"
    );

    Ok(McpServer::new(
        ApiClient::from_config(&config, &cli.api_url),
        credentials,
    ))
}

async fn run_stdio(server: &mut McpServer) -> Result<()> {
    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut reader = io::BufReader::new(stdin.lock());
    let mut writer = stdout.lock();

    while let Some(body) = read_message(&mut reader)? {
        let request: Value =
            serde_json::from_slice(&body).context("Failed to parse JSON-RPC request body")?;

        let response = match server.handle_request(&request).await {
            Ok(Some(response)) => Some(response),
            Ok(None) => None,
            Err(error) => Some(error_response(
                request.get("id").cloned(),
                -32603,
                error.to_string(),
            )),
        };

        if let Some(response) = response {
            write_message(&mut writer, &response)?;
        }
    }

    Ok(())
}

async fn http_health() -> StatusCode {
    StatusCode::OK
}

async fn http_mcp(
    State(server): State<Arc<Mutex<McpServer>>>,
    Json(request): Json<Value>,
) -> (StatusCode, Json<Value>) {
    let mut server = server.lock().await;
    let response = match server.handle_request(&request).await {
        Ok(Some(response)) => response,
        Ok(None) => return (StatusCode::NO_CONTENT, Json(Value::Null)),
        Err(error) => error_response(request.get("id").cloned(), -32603, error.to_string()),
    };

    (StatusCode::OK, Json(response))
}

async fn run_http(server: McpServer, listen_addr: &str) -> Result<()> {
    let app = Router::new()
        .route("/health", get(http_health))
        .route("/mcp", post(http_mcp))
        .with_state(Arc::new(Mutex::new(server)));

    let listener = tokio::net::TcpListener::bind(listen_addr)
        .await
        .with_context(|| format!("Failed to bind MCP HTTP listener at {listen_addr}"))?;

    axum::serve(listener, app)
        .await
        .context("MCP HTTP server exited unexpectedly")
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    attune_common::auth::install_crypto_provider();

    let cli = Cli::parse();
    if cli.verbose {
        tracing_subscriber::fmt()
            .with_writer(io::stderr)
            .with_max_level(tracing::Level::DEBUG)
            .init();
    }

    let mut server = build_server(&cli).await?;

    match cli.transport {
        Transport::Stdio => run_stdio(&mut server).await,
        Transport::Http => run_http(server, &cli.listen_addr).await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_message_parses_ndjson_frames() {
        let payload = br#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#;
        let mut input = io::Cursor::new([payload.as_slice(), b"\n"].concat());
        let body = read_message(&mut input)
            .expect("frame should parse")
            .expect("frame should exist");
        assert_eq!(body, payload);
    }

    #[test]
    fn read_message_skips_blank_lines() {
        let payload = br#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#;
        let mut input = io::Cursor::new([b"\n\r\n".as_slice(), payload, b"\n"].concat());
        let body = read_message(&mut input)
            .expect("frame should parse")
            .expect("frame should exist");
        assert_eq!(body, payload);
    }

    #[test]
    fn write_message_emits_ndjson_frame() {
        let mut output = Vec::new();
        write_message(&mut output, &json!({"jsonrpc":"2.0","id":1,"result":{}}))
            .expect("frame should write");
        let rendered = String::from_utf8(output).expect("utf8 output");
        assert!(rendered.ends_with('\n'));
        assert!(!rendered.contains("Content-Length"));
        assert_eq!(rendered.matches('\n').count(), 1);
    }

    #[test]
    fn initialize_uses_requested_protocol_version() {
        let config = CliConfig::default();
        let mut server = McpServer::new(ApiClient::from_config(&config, &None), None);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime");

        let response = runtime
            .block_on(server.handle_request(&json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": {
                    "protocolVersion": "2025-03-26"
                }
            })))
            .expect("initialize should succeed")
            .expect("initialize should respond");

        assert_eq!(
            response["result"]["protocolVersion"],
            Value::String("2025-03-26".to_string())
        );
        assert_eq!(response["result"]["serverInfo"]["name"], "attune-mcp");
    }

    #[test]
    fn tool_catalog_includes_queue_enqueue_and_execute() {
        let names = tool_defs().iter().map(|tool| tool.name).collect::<Vec<_>>();
        assert!(names.contains(&"actions_execute"));
        assert!(names.contains(&"queues_enqueue"));
        assert!(names.contains(&"events_list"));
        assert!(names.contains(&"rules_update_trace_tag_template"));
        assert!(names.contains(&"queues_update_trace_tag_template"));
    }

    #[test]
    fn build_config_applies_token_overrides() {
        let cli = Cli {
            profile: None,
            api_url: None,
            transport: Transport::Stdio,
            listen_addr: "127.0.0.1:8090".to_string(),
            auth_token: Some("access".to_string()),
            execution_token: None,
            refresh_token: Some("refresh".to_string()),
            login: None,
            password: None,
            verbose: false,
        };

        let config = build_config(&cli).expect("config should build");
        let profile = config.current_profile().expect("default profile");
        assert_eq!(profile.auth_token.as_deref(), Some("access"));
        assert_eq!(profile.refresh_token.as_deref(), Some("refresh"));
    }

    #[test]
    fn build_config_prefers_execution_token_and_clears_refresh_token() {
        let cli = Cli {
            profile: None,
            api_url: None,
            transport: Transport::Stdio,
            listen_addr: "127.0.0.1:8090".to_string(),
            auth_token: None,
            execution_token: Some("execution-token".to_string()),
            refresh_token: Some("refresh".to_string()),
            login: None,
            password: None,
            verbose: false,
        };

        let config = build_config(&cli).expect("config should build");
        let profile = config.current_profile().expect("default profile");
        assert_eq!(profile.auth_token.as_deref(), Some("execution-token"));
        assert_eq!(profile.refresh_token.as_deref(), None);
    }

    #[test]
    fn selected_auth_mode_prefers_execution_token() {
        let cli = Cli {
            profile: None,
            api_url: None,
            transport: Transport::Stdio,
            listen_addr: "127.0.0.1:8090".to_string(),
            auth_token: Some("explicit".to_string()),
            execution_token: Some("execution".to_string()),
            refresh_token: None,
            login: None,
            password: None,
            verbose: false,
        };

        let config = build_config(&cli).expect("config should build");
        let mode = selected_auth_mode(&cli, &config).expect("auth mode");
        assert!(matches!(mode, AuthMode::ExecutionToken));
    }

    #[test]
    fn ensure_current_profile_exists_inserts_missing_profile() {
        let mut config = CliConfig {
            current_profile: "missing".to_string(),
            ..CliConfig::default()
        };
        config.profiles.clear();

        ensure_current_profile_exists(&mut config);

        let profile = config.current_profile().expect("profile should exist");
        assert_eq!(profile.api_url, "http://localhost:8080");
        assert_eq!(profile.auth_token, None);
        assert_eq!(profile.refresh_token, None);
    }
}
