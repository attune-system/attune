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
use std::collections::{HashMap, VecDeque};
use std::io::{self, BufRead, Write};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
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

    /// Root allowed for HTTP packs_check access (repeatable; env is comma-separated)
    #[arg(
        long = "packs-check-root",
        env = "ATTUNE_MCP_PACKS_CHECK_ROOTS",
        value_delimiter = ','
    )]
    packs_check_roots: Vec<PathBuf>,

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

#[derive(Clone, Copy, Debug, clap::ValueEnum)]
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
    cache_refreshes: HashMap<String, CacheRefreshMetadata>,
    cache_refresh_order: VecDeque<String>,
    packs_check_access: PacksCheckAccess,
}

#[derive(Clone)]
enum PacksCheckAccess {
    Unrestricted,
    Allowlisted(Vec<PathBuf>),
    Disabled,
}

const MAX_RETAINED_CACHE_REFRESHES: usize = 128;

#[derive(Clone)]
struct CacheRefreshMetadata {
    expected_chunk_count: i64,
    expected_record_count: Option<i64>,
    expected_size_bytes: Option<i64>,
}

impl McpServer {
    fn new(
        client: ApiClient,
        credentials: Option<LoginCredentials>,
        packs_check_access: PacksCheckAccess,
    ) -> Self {
        Self {
            client,
            credentials,
            cache_refreshes: HashMap::new(),
            cache_refresh_order: VecDeque::new(),
            packs_check_access,
        }
    }

    fn packs_check_available(&self) -> bool {
        !matches!(self.packs_check_access, PacksCheckAccess::Disabled)
    }

    fn remember_cache_refresh(&mut self, key: String, metadata: CacheRefreshMetadata) {
        if let Some(retained) = self.cache_refreshes.get_mut(&key) {
            *retained = metadata;
            return;
        }
        while self.cache_refreshes.len() >= MAX_RETAINED_CACHE_REFRESHES {
            let Some(oldest) = self.cache_refresh_order.pop_front() else {
                break;
            };
            self.cache_refreshes.remove(&oldest);
        }
        self.cache_refresh_order.push_back(key.clone());
        self.cache_refreshes.insert(key, metadata);
    }

    fn forget_cache_refresh(&mut self, key: &str) {
        self.cache_refreshes.remove(key);
        self.cache_refresh_order.retain(|retained| retained != key);
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
                    .filter(|tool| tool.name != "packs_check" || self.packs_check_available())
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
            "packs_check" => {
                if !self.packs_check_available() {
                    anyhow::bail!(
                        "Tool 'packs_check' is unavailable: HTTP filesystem access requires at least one --packs-check-root"
                    );
                }
                let path = required_string(args, "path")?;
                let path = PathBuf::from(path);
                let access = self.packs_check_access.clone();
                tokio::task::spawn_blocking(move || {
                    let canonical_path = path.canonicalize().with_context(|| {
                        format!(
                            "Pack path '{}' does not exist or is invalid",
                            path.display()
                        )
                    })?;
                    if !canonical_path.is_dir() {
                        anyhow::bail!(
                            "Pack path '{}' is not a directory",
                            canonical_path.display()
                        );
                    }
                    if let PacksCheckAccess::Allowlisted(roots) = &access {
                        if !roots.iter().any(|root| canonical_path.starts_with(root)) {
                            anyhow::bail!(
                                "Pack path '{}' is outside the configured packs_check roots",
                                canonical_path.display()
                            );
                        }
                    }

                    serde_json::to_value(attune_common::pack_check::check_pack(&canonical_path))
                        .context("Failed to serialize pack check report")
                })
                .await
                .context("packs_check filesystem task failed")?
            }
            "cache_namespaces_list" => self.cache_namespaces_list(args).await,
            "cache_namespace_get" => self.cache_namespace_get(args).await,
            "cache_namespace_create" => self.cache_namespace_create(args).await,
            "cache_namespace_update" => self.cache_namespace_update(args).await,
            "cache_namespace_delete" => self.cache_namespace_delete(args).await,
            "cache_entry_get" => self.cache_entry_get(args).await,
            "cache_entries_get_many" => self.cache_entries_get_many(args).await,
            "cache_entries_scan" => self.cache_entries_scan(args).await,
            "cache_generations_list" => self.cache_generations_list(args).await,
            "cache_generation_get" => self.cache_generation_get(args).await,
            "cache_refresh_begin" => self.cache_refresh_begin(args).await,
            "cache_refresh_upload_chunk" => self.cache_refresh_upload_chunk(args).await,
            "cache_refresh_seal" => self.cache_refresh_seal(args).await,
            "cache_refresh_promote" => self.cache_refresh_promote(args).await,
            "cache_refresh_abort" => self.cache_refresh_abort(args).await,
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

    async fn cache_namespaces_list(&mut self, args: &Map<String, Value>) -> Result<Value> {
        let owner = cache_owner(args)?;
        let limit = cache_metadata_limit(args)?;
        let mut query = owner.query();
        if let Some(namespace) = optional_string(args, "namespace") {
            query.push_str("&namespace=");
            query.push_str(&urlencoding::encode(&namespace));
        }
        if let Some(freshness) = optional_string(args, "freshness") {
            if !matches!(freshness.as_str(), "fresh" | "stale" | "unpopulated") {
                anyhow::bail!("Argument 'freshness' must be fresh, stale, or unpopulated");
            }
            query.push_str("&freshness=");
            query.push_str(&freshness);
        }
        if let Some(limit) = limit {
            query.push_str(&format!("&limit={limit}"));
        }
        if let Some(cursor) = optional_string(args, "cursor") {
            query.push_str("&cursor=");
            query.push_str(&urlencoding::encode(&cursor));
        }
        self.client
            .cache_get(&format!("/cache/namespaces?{query}"))
            .await
    }

    async fn cache_namespace_get(&mut self, args: &Map<String, Value>) -> Result<Value> {
        let namespace = required_string(args, "namespace")?;
        let owner = cache_owner(args)?;
        self.client
            .cache_get(&format!(
                "{}?{}",
                cache_namespace_path(namespace),
                owner.query()
            ))
            .await
    }

    async fn cache_namespace_create(&mut self, args: &Map<String, Value>) -> Result<Value> {
        let namespace = required_string(args, "namespace")?;
        let owner = cache_owner(args)?;
        let mut body = cache_policy(args)?;
        body.insert(
            "namespace".to_string(),
            Value::String(namespace.to_string()),
        );
        self.client
            .cache_post("/cache/namespaces", &owner.scoped_payload(body))
            .await
    }

    async fn cache_namespace_update(&mut self, args: &Map<String, Value>) -> Result<Value> {
        let namespace = required_string(args, "namespace")?;
        let owner = cache_owner(args)?;
        let body = cache_policy(args)?;
        if body.is_empty() {
            anyhow::bail!("Provide at least one mutable cache namespace policy field");
        }
        self.client
            .cache_put(
                &cache_namespace_path(namespace),
                &owner.scoped_payload(body),
            )
            .await
    }

    async fn cache_namespace_delete(&mut self, args: &Map<String, Value>) -> Result<Value> {
        let namespace = required_string(args, "namespace")?;
        let owner = cache_owner(args)?;
        self.client
            .cache_delete(&format!(
                "{}?{}",
                cache_namespace_path(namespace),
                owner.query()
            ))
            .await?;
        Ok(json!({ "namespace": namespace, "deleted": true }))
    }

    async fn cache_entry_get(&mut self, args: &Map<String, Value>) -> Result<Value> {
        let namespace = required_string(args, "namespace")?;
        let external_id = required_string(args, "external_id")?;
        let owner = cache_owner(args)?;
        self.client
            .cache_post(
                &format!("{}/entries/lookup", cache_namespace_path(namespace)),
                &owner.scoped_payload(
                    json!({
                        "external_id": external_id,
                        "generation_id": optional_i64(args, "generation_id")?,
                        "require_fresh": false,
                    })
                    .as_object()
                    .expect("object")
                    .clone(),
                ),
            )
            .await
    }

    async fn cache_entries_get_many(&mut self, args: &Map<String, Value>) -> Result<Value> {
        let namespace = required_string(args, "namespace")?;
        let external_ids = required_string_array(args, "external_ids", 1_000)?;
        let owner = cache_owner(args)?;
        self.client
            .cache_post(
                &format!("{}/entries/lookup-many", cache_namespace_path(namespace)),
                &owner.scoped_payload(
                    json!({
                        "external_ids": external_ids,
                        "generation_id": optional_i64(args, "generation_id")?,
                        "require_fresh": false,
                    })
                    .as_object()
                    .expect("object")
                    .clone(),
                ),
            )
            .await
    }

    async fn cache_entries_scan(&mut self, args: &Map<String, Value>) -> Result<Value> {
        let namespace = required_string(args, "namespace")?;
        let owner = cache_owner(args)?;
        let limit = optional_i64(args, "limit")?.unwrap_or(100);
        if !(1..=1_000).contains(&limit) {
            anyhow::bail!("Argument 'limit' must be between 1 and 1000");
        }
        let mut query = format!("{}&limit={limit}", owner.query());
        if let Some(generation_id) = optional_i64(args, "generation_id")? {
            query.push_str(&format!("&generation={generation_id}"));
        }
        if let Some(cursor) = optional_string(args, "cursor") {
            query.push_str(&format!("&cursor={}", urlencoding::encode(&cursor)));
        }
        // The API always returns values for authorized scans. The input flag is
        // retained as an explicit acknowledgement of that disclosure.
        let include_values = optional_bool(args, "include_values")?.unwrap_or(false);
        if include_values {
            query.push_str("&include_values=true");
        }
        let mut page: Value = self
            .client
            .cache_get(&format!(
                "{}/entries?{query}",
                cache_namespace_path(namespace)
            ))
            .await?;
        if !include_values {
            if let Some(items) = page.get_mut("items").and_then(Value::as_array_mut) {
                for item in items {
                    if let Some(item) = item.as_object_mut() {
                        item.remove("value");
                    }
                }
            }
        }
        Ok(page)
    }

    async fn cache_generations_list(&mut self, args: &Map<String, Value>) -> Result<Value> {
        let namespace = required_string(args, "namespace")?;
        let owner = cache_owner(args)?;
        let limit = cache_metadata_limit(args)?;
        let mut query = owner.query();
        if let Some(limit) = limit {
            query.push_str(&format!("&limit={limit}"));
        }
        if let Some(cursor) = optional_string(args, "cursor") {
            query.push_str("&cursor=");
            query.push_str(&urlencoding::encode(&cursor));
        }
        self.client
            .cache_get(&format!(
                "{}/generations?{query}",
                cache_namespace_path(namespace),
            ))
            .await
    }

    async fn cache_generation_get(&mut self, args: &Map<String, Value>) -> Result<Value> {
        let namespace = required_string(args, "namespace")?;
        let generation_id = required_i64(args, "generation_id")?;
        let owner = cache_owner(args)?;
        self.client
            .cache_get(&format!(
                "{}/generations/{generation_id}?{}",
                cache_namespace_path(namespace),
                owner.query()
            ))
            .await
    }

    async fn cache_refresh_begin(&mut self, args: &Map<String, Value>) -> Result<Value> {
        let namespace = required_string(args, "namespace")?;
        let owner = cache_owner(args)?;
        let expected_chunk_count = required_i64(args, "expected_chunk_count")?;
        if !(0..=i32::MAX as i64).contains(&expected_chunk_count) {
            anyhow::bail!("Argument 'expected_chunk_count' must be a nonnegative 32-bit integer");
        }
        let expected_active_generation_id = expected_active_generation(args)?;
        let expected_record_count = optional_i64(args, "expected_record_count")?;
        let expected_size_bytes = optional_i64(args, "expected_size_bytes")?;
        let response: Value = self
            .client
            .cache_post(
                &format!("{}/generations", cache_namespace_path(namespace)),
                &owner.scoped_payload(
                    json!({
                        "client_refresh_id": optional_string(args, "client_refresh_id")
                            .unwrap_or_else(|| format!(
                                "mcp-{}",
                                SystemTime::now()
                                    .duration_since(UNIX_EPOCH)
                                    .unwrap_or_default()
                                    .as_nanos()
                            )),
                        "expected_active_generation_id": expected_active_generation_id,
                        "expected_chunk_count": expected_chunk_count,
                        "expected_record_count": expected_record_count,
                        "expected_size_bytes": expected_size_bytes,
                        "source_revision": optional_string(args, "source_revision"),
                    })
                    .as_object()
                    .expect("object")
                    .clone(),
                ),
            )
            .await?;
        if let Some(generation_id) = response.get("generation_id").and_then(Value::as_i64) {
            self.remember_cache_refresh(
                cache_refresh_key(&owner, namespace, generation_id),
                CacheRefreshMetadata {
                    expected_chunk_count,
                    expected_record_count,
                    expected_size_bytes,
                },
            );
        }
        Ok(response)
    }

    async fn cache_refresh_upload_chunk(&mut self, args: &Map<String, Value>) -> Result<Value> {
        let namespace = required_string(args, "namespace")?;
        let generation_id = required_i64(args, "generation_id")?;
        let chunk_index = required_i64(args, "chunk_index")?;
        if chunk_index < 0 || chunk_index > i32::MAX as i64 {
            anyhow::bail!("Argument 'chunk_index' must be a nonnegative 32-bit integer");
        }
        let entries = required_array(args, "entries", 10_000)?;
        if entries.is_empty() {
            anyhow::bail!("Argument 'entries' must not be empty");
        }
        let owner = cache_owner(args)?;
        self.client
            .cache_put(
                &format!(
                    "{}/generations/{generation_id}/chunks/{chunk_index}",
                    cache_namespace_path(namespace)
                ),
                &owner.scoped_payload(
                    json!({ "entries": entries })
                        .as_object()
                        .expect("object")
                        .clone(),
                ),
            )
            .await
    }

    async fn cache_refresh_seal(&mut self, args: &Map<String, Value>) -> Result<Value> {
        let namespace = required_string(args, "namespace")?;
        let generation_id = required_i64(args, "generation_id")?;
        let owner = cache_owner(args)?;
        let refresh_key = cache_refresh_key(&owner, namespace, generation_id);
        let retained = self.cache_refreshes.get(&refresh_key);
        let expected_chunk_count = optional_i64(args, "expected_chunk_count")?
            .or_else(|| retained.map(|metadata| metadata.expected_chunk_count))
            .context(
                "Provide 'expected_chunk_count' when sealing a generation not begun by this MCP process",
            )?;
        if !(0..=i32::MAX as i64).contains(&expected_chunk_count) {
            anyhow::bail!("Argument 'expected_chunk_count' must be a nonnegative 32-bit integer");
        }
        let expected_record_count = optional_i64(args, "expected_record_count")?
            .or_else(|| retained.and_then(|metadata| metadata.expected_record_count));
        let expected_size_bytes = optional_i64(args, "expected_size_bytes")?
            .or_else(|| retained.and_then(|metadata| metadata.expected_size_bytes));
        let response = self
            .client
            .cache_post(
                &format!(
                    "{}/generations/{generation_id}/seal",
                    cache_namespace_path(namespace)
                ),
                &owner.scoped_payload(
                    json!({
                        "expected_chunk_count": expected_chunk_count,
                        "expected_record_count": expected_record_count,
                        "expected_size_bytes": expected_size_bytes,
                    })
                    .as_object()
                    .expect("object")
                    .clone(),
                ),
            )
            .await?;
        self.forget_cache_refresh(&refresh_key);
        Ok(response)
    }

    async fn cache_refresh_promote(&mut self, args: &Map<String, Value>) -> Result<Value> {
        let namespace = required_string(args, "namespace")?;
        let generation_id = required_i64(args, "generation_id")?;
        let owner = cache_owner(args)?;
        let refresh_key = cache_refresh_key(&owner, namespace, generation_id);
        let response = self
            .client
            .cache_post(
                &format!(
                    "{}/generations/{generation_id}/promote",
                    cache_namespace_path(namespace)
                ),
                &owner.scoped_payload(
                    json!({ "expected_active_generation_id": expected_active_generation(args)? })
                        .as_object()
                        .expect("object")
                        .clone(),
                ),
            )
            .await?;
        self.forget_cache_refresh(&refresh_key);
        Ok(response)
    }

    async fn cache_refresh_abort(&mut self, args: &Map<String, Value>) -> Result<Value> {
        let namespace = required_string(args, "namespace")?;
        let generation_id = required_i64(args, "generation_id")?;
        let owner = cache_owner(args)?;
        let refresh_key = cache_refresh_key(&owner, namespace, generation_id);
        let response = self
            .client
            .cache_post(
                &format!(
                    "{}/generations/{generation_id}/abandon",
                    cache_namespace_path(namespace)
                ),
                &owner.scoped_payload(Map::new()),
            )
            .await?;
        self.forget_cache_refresh(&refresh_key);
        Ok(response)
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
        ToolDef {
            name: "packs_check",
            title: "Check local pack",
            description: "Validate metadata in a local pack directory without contacting Attune. The path is resolved on the attune-mcp server host, not the MCP client host.",
            input_schema: pack_check_schema,
        },
        ToolDef {
            name: "cache_namespaces_list",
            title: "List cache namespaces",
            description: "List cache namespaces for one explicit owner scope.",
            input_schema: cache_namespace_list_schema,
        },
        ToolDef {
            name: "cache_namespace_get",
            title: "Get cache namespace",
            description: "Fetch metadata and policy for an owner-scoped cache namespace.",
            input_schema: cache_namespace_schema,
        },
        ToolDef {
            name: "cache_namespace_create",
            title: "Create cache namespace",
            description: "Create an owner-scoped cache namespace. Ownership cannot later be changed.",
            input_schema: cache_namespace_policy_schema,
        },
        ToolDef {
            name: "cache_namespace_update",
            title: "Update cache namespace policy",
            description: "Update one or more mutable policy fields for a cache namespace.",
            input_schema: cache_namespace_policy_schema,
        },
        ToolDef {
            name: "cache_namespace_delete",
            title: "Delete cache namespace",
            description: "Tombstone a cache namespace and make its generations unavailable.",
            input_schema: cache_namespace_schema,
        },
        ToolDef {
            name: "cache_entry_get",
            title: "Get cache entry",
            description: "Read one cache value from the active or a retained generation. Values may contain sensitive business data.",
            input_schema: cache_entry_get_schema,
        },
        ToolDef {
            name: "cache_entries_get_many",
            title: "Get multiple cache entries",
            description: "Read up to 1000 cache values from the active or a retained generation.",
            input_schema: cache_entries_get_many_schema,
        },
        ToolDef {
            name: "cache_entries_scan",
            title: "Scan cache entries",
            description: "Read one bounded cache page. Use the returned cursor for the next page; no unbounded scan is exposed.",
            input_schema: cache_entries_scan_schema,
        },
        ToolDef {
            name: "cache_generations_list",
            title: "List cache generations",
            description: "List immutable generations for an owner-scoped cache namespace.",
            input_schema: cache_generation_list_schema,
        },
        ToolDef {
            name: "cache_generation_get",
            title: "Get cache generation",
            description: "Fetch validation and lifecycle details for one cache generation.",
            input_schema: cache_generation_schema,
        },
        ToolDef {
            name: "cache_refresh_begin",
            title: "Begin cache refresh",
            description: "Create an idempotent staging generation. Requires explicit cache-write permission.",
            input_schema: cache_refresh_begin_schema,
        },
        ToolDef {
            name: "cache_refresh_upload_chunk",
            title: "Upload cache refresh chunk",
            description: "Upload one bounded structured chunk to a staging generation. Requires explicit cache-write permission.",
            input_schema: cache_refresh_upload_schema,
        },
        ToolDef {
            name: "cache_refresh_seal",
            title: "Seal cache refresh",
            description: "Validate a fully uploaded staging generation using begin-time expected metadata. Supply expected_chunk_count when begin was performed by another MCP process.",
            input_schema: cache_refresh_seal_schema,
        },
        ToolDef {
            name: "cache_refresh_promote",
            title: "Promote cache refresh",
            description: "Atomically publish a ready generation with an optimistic active-generation precondition.",
            input_schema: cache_refresh_promote_schema,
        },
        ToolDef {
            name: "cache_refresh_abort",
            title: "Abort cache refresh",
            description: "Abandon a staging generation. This destructive operation requires explicit cache-write permission.",
            input_schema: cache_generation_schema,
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

fn pack_check_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "path": {
                "type": "string",
                "minLength": 1,
                "description": "Absolute or relative pack directory path on the attune-mcp host"
            }
        },
        "required": ["path"],
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

fn cache_owner_properties() -> Map<String, Value> {
    let mut properties = Map::new();
    properties.insert("owner_type".to_string(), json!({
        "type": "string",
        "enum": ["system", "identity", "pack", "action", "sensor"],
        "description": "Cache owner type. Pack, action, and sensor owners require their matching owner reference."
    }));
    properties.insert("owner_pack_ref".to_string(), json!({ "type": "string" }));
    properties.insert("owner_action_ref".to_string(), json!({ "type": "string" }));
    properties.insert("owner_sensor_ref".to_string(), json!({ "type": "string" }));
    properties
}

fn cache_schema(mut properties: Map<String, Value>, required: &[&str]) -> Value {
    for (key, value) in cache_owner_properties() {
        properties.insert(key, value);
    }
    let mut required = required
        .iter()
        .map(|value| (*value).to_string())
        .collect::<Vec<_>>();
    required.push("owner_type".to_string());
    json!({
        "type": "object",
        "properties": properties,
        "required": required,
        "additionalProperties": false
    })
}

fn cache_namespace_list_schema() -> Value {
    let mut properties = Map::new();
    properties.insert(
        "namespace".to_string(),
        json!({ "type": "string", "description": "Case-insensitive namespace substring filter" }),
    );
    properties.insert(
        "freshness".to_string(),
        json!({ "type": "string", "enum": ["fresh", "stale", "unpopulated"] }),
    );
    properties.insert(
        "limit".to_string(),
        json!({ "type": "integer", "minimum": 1, "maximum": 500 }),
    );
    properties.insert("cursor".to_string(), json!({ "type": "string" }));
    cache_schema(properties, &[])
}

fn cache_namespace_schema() -> Value {
    let mut properties = Map::new();
    properties.insert("namespace".to_string(), json!({ "type": "string" }));
    cache_schema(properties, &["namespace"])
}

fn cache_namespace_policy_schema() -> Value {
    let mut properties = Map::new();
    properties.insert("namespace".to_string(), json!({ "type": "string" }));
    for field in [
        "freshness_target_seconds",
        "max_records_per_generation",
        "max_generation_bytes",
        "max_retained_bytes",
        "max_retained_generations",
        "max_staging_generations",
    ] {
        properties.insert(
            field.to_string(),
            json!({ "type": "integer", "minimum": 0 }),
        );
    }
    cache_schema(properties, &["namespace"])
}

fn cache_entry_get_schema() -> Value {
    let mut properties = Map::new();
    properties.insert("namespace".to_string(), json!({ "type": "string" }));
    properties.insert("external_id".to_string(), json!({ "type": "string" }));
    properties.insert("generation_id".to_string(), json!({ "type": "integer" }));
    cache_schema(properties, &["namespace", "external_id"])
}

fn cache_entries_get_many_schema() -> Value {
    let mut properties = Map::new();
    properties.insert("namespace".to_string(), json!({ "type": "string" }));
    properties.insert(
        "external_ids".to_string(),
        json!({
            "type": "array", "minItems": 1, "maxItems": 1000, "items": { "type": "string" }
        }),
    );
    properties.insert("generation_id".to_string(), json!({ "type": "integer" }));
    cache_schema(properties, &["namespace", "external_ids"])
}

fn cache_entries_scan_schema() -> Value {
    let mut properties = Map::new();
    properties.insert("namespace".to_string(), json!({ "type": "string" }));
    properties.insert("generation_id".to_string(), json!({ "type": "integer" }));
    properties.insert("cursor".to_string(), json!({ "type": "string" }));
    properties.insert(
        "limit".to_string(),
        json!({ "type": "integer", "minimum": 1, "maximum": 1000 }),
    );
    properties.insert(
        "include_values".to_string(),
        json!({ "type": "boolean", "default": false }),
    );
    cache_schema(properties, &["namespace"])
}

fn cache_generation_schema() -> Value {
    let mut properties = Map::new();
    properties.insert("namespace".to_string(), json!({ "type": "string" }));
    properties.insert("generation_id".to_string(), json!({ "type": "integer" }));
    cache_schema(properties, &["namespace", "generation_id"])
}

fn cache_generation_list_schema() -> Value {
    let mut properties = Map::new();
    properties.insert("namespace".to_string(), json!({ "type": "string" }));
    properties.insert(
        "limit".to_string(),
        json!({ "type": "integer", "minimum": 1, "maximum": 500 }),
    );
    properties.insert("cursor".to_string(), json!({ "type": "string" }));
    cache_schema(properties, &["namespace"])
}

fn cache_refresh_begin_schema() -> Value {
    let mut properties = Map::new();
    properties.insert("namespace".to_string(), json!({ "type": "string" }));
    properties.insert("client_refresh_id".to_string(), json!({ "type": "string" }));
    properties.insert(
        "expected_chunk_count".to_string(),
        json!({ "type": "integer", "minimum": 0 }),
    );
    properties.insert(
        "expected_record_count".to_string(),
        json!({ "type": "integer", "minimum": 0 }),
    );
    properties.insert(
        "expected_size_bytes".to_string(),
        json!({ "type": "integer", "minimum": 0 }),
    );
    properties.insert("source_revision".to_string(), json!({ "type": "string" }));
    properties.insert(
        "expected_active_generation_id".to_string(),
        json!({ "type": "integer" }),
    );
    properties.insert("expect_empty".to_string(), json!({ "type": "boolean" }));
    cache_schema(properties, &["namespace", "expected_chunk_count"])
}

fn cache_refresh_seal_schema() -> Value {
    let mut properties = Map::new();
    properties.insert("namespace".to_string(), json!({ "type": "string" }));
    properties.insert("generation_id".to_string(), json!({ "type": "integer" }));
    properties.insert(
        "expected_chunk_count".to_string(),
        json!({
            "type": "integer",
            "minimum": 0,
            "description": "Required unless this MCP process handled the matching begin request"
        }),
    );
    properties.insert(
        "expected_record_count".to_string(),
        json!({ "type": "integer", "minimum": 0 }),
    );
    properties.insert(
        "expected_size_bytes".to_string(),
        json!({ "type": "integer", "minimum": 0 }),
    );
    cache_schema(properties, &["namespace", "generation_id"])
}

fn cache_refresh_upload_schema() -> Value {
    let mut properties = Map::new();
    properties.insert("namespace".to_string(), json!({ "type": "string" }));
    properties.insert("generation_id".to_string(), json!({ "type": "integer" }));
    properties.insert(
        "chunk_index".to_string(),
        json!({ "type": "integer", "minimum": 0 }),
    );
    properties.insert(
        "entries".to_string(),
        json!({
            "type": "array", "minItems": 1, "maxItems": 10000,
            "items": {
                "type": "object",
                "properties": {
                    "external_id": { "type": "string" },
                    "value": {},
                    "source_updated_at": { "type": "string" },
                    "source_checksum": { "type": "string" }
                },
                "required": ["external_id", "value"],
                "additionalProperties": false
            }
        }),
    );
    cache_schema(
        properties,
        &["namespace", "generation_id", "chunk_index", "entries"],
    )
}

fn cache_refresh_promote_schema() -> Value {
    let mut properties = Map::new();
    properties.insert("namespace".to_string(), json!({ "type": "string" }));
    properties.insert("generation_id".to_string(), json!({ "type": "integer" }));
    properties.insert(
        "expected_active_generation_id".to_string(),
        json!({ "type": "integer" }),
    );
    properties.insert("expect_empty".to_string(), json!({ "type": "boolean" }));
    cache_schema(properties, &["namespace", "generation_id"])
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

fn optional_bool(args: &Map<String, Value>, key: &str) -> Result<Option<bool>> {
    match args.get(key) {
        Some(value) => value
            .as_bool()
            .map(Some)
            .ok_or_else(|| anyhow!("Argument '{key}' must be a boolean")),
        None => Ok(None),
    }
}

fn required_array(args: &Map<String, Value>, key: &str, max_len: usize) -> Result<Vec<Value>> {
    let values = args
        .get(key)
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("Missing required array argument '{key}'"))?;
    if values.len() > max_len {
        anyhow::bail!("Argument '{key}' may contain at most {max_len} items");
    }
    Ok(values.clone())
}

fn required_string_array(
    args: &Map<String, Value>,
    key: &str,
    max_len: usize,
) -> Result<Vec<String>> {
    required_array(args, key, max_len)?
        .into_iter()
        .map(|value| {
            value
                .as_str()
                .map(ToOwned::to_owned)
                .ok_or_else(|| anyhow!("Argument '{key}' must contain only strings"))
        })
        .collect()
}

struct CacheOwner {
    owner_type: String,
    owner_ref: Option<String>,
}

impl CacheOwner {
    fn query(&self) -> String {
        let mut query = format!("owner_type={}", self.owner_type);
        if let Some(owner_ref) = &self.owner_ref {
            query.push_str("&owner_ref=");
            query.push_str(&urlencoding::encode(owner_ref));
        }
        query
    }

    fn scoped_payload(&self, mut body: Map<String, Value>) -> Value {
        body.insert(
            "owner_type".to_string(),
            Value::String(self.owner_type.clone()),
        );
        if let Some(owner_ref) = &self.owner_ref {
            body.insert("owner_ref".to_string(), Value::String(owner_ref.clone()));
        }
        Value::Object(body)
    }
}

fn cache_owner(args: &Map<String, Value>) -> Result<CacheOwner> {
    let owner_type = required_string(args, "owner_type")?;
    let (owner_type, owner_ref_key) = match owner_type {
        "system" | "identity" => (owner_type.to_string(), None),
        "pack" => (owner_type.to_string(), Some("owner_pack_ref")),
        "action" => (owner_type.to_string(), Some("owner_action_ref")),
        "sensor" => (owner_type.to_string(), Some("owner_sensor_ref")),
        _ => anyhow::bail!(
            "Argument 'owner_type' must be one of system, identity, pack, action, or sensor"
        ),
    };
    let owner_ref = match owner_ref_key {
        Some(key) => Some(required_string(args, key)?.to_string()),
        None => None,
    };
    Ok(CacheOwner {
        owner_type,
        owner_ref,
    })
}

fn cache_metadata_limit(args: &Map<String, Value>) -> Result<Option<i64>> {
    let limit = optional_i64(args, "limit")?;
    if limit.is_some_and(|limit| !(1..=500).contains(&limit)) {
        anyhow::bail!("Argument 'limit' must be between 1 and 500");
    }
    Ok(limit)
}

fn cache_refresh_key(owner: &CacheOwner, namespace: &str, generation_id: i64) -> String {
    format!(
        "{}\u{0}{}\u{0}{}\u{0}{generation_id}",
        owner.owner_type,
        owner.owner_ref.as_deref().unwrap_or_default(),
        namespace
    )
}

fn cache_namespace_path(namespace: &str) -> String {
    format!("/cache/namespaces/{}", encode_path(namespace))
}

fn cache_policy(args: &Map<String, Value>) -> Result<Map<String, Value>> {
    const FIELDS: &[&str] = &[
        "freshness_target_seconds",
        "max_records_per_generation",
        "max_generation_bytes",
        "max_retained_bytes",
        "max_retained_generations",
        "max_staging_generations",
    ];
    let mut policy = Map::new();
    for field in FIELDS {
        if let Some(value) = optional_i64(args, field)? {
            policy.insert((*field).to_string(), Value::from(value));
        }
    }
    Ok(policy)
}

fn expected_active_generation(args: &Map<String, Value>) -> Result<Option<i64>> {
    let expected_active = optional_i64(args, "expected_active_generation_id")?;
    let expect_empty = optional_bool(args, "expect_empty")?.unwrap_or(false);
    match (expected_active, expect_empty) {
        (Some(_), true) => anyhow::bail!(
            "Arguments 'expected_active_generation_id' and 'expect_empty' are mutually exclusive"
        ),
        (Some(id), false) => Ok(Some(id)),
        (None, true) => Ok(None),
        (None, false) => {
            anyhow::bail!("Provide 'expected_active_generation_id' or set 'expect_empty' to true")
        }
    }
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

    let packs_check_access = match cli.transport {
        Transport::Stdio => PacksCheckAccess::Unrestricted,
        Transport::Http if cli.packs_check_roots.is_empty() => PacksCheckAccess::Disabled,
        Transport::Http => {
            let roots = cli.packs_check_roots.clone();
            let roots = tokio::task::spawn_blocking(move || {
                roots
                    .into_iter()
                    .map(|root| {
                        let canonical = root.canonicalize().with_context(|| {
                            format!(
                                "Configured packs_check root '{}' does not exist or is invalid",
                                root.display()
                            )
                        })?;
                        if !canonical.is_dir() {
                            anyhow::bail!(
                                "Configured packs_check root '{}' is not a directory",
                                canonical.display()
                            );
                        }
                        Ok(canonical)
                    })
                    .collect::<Result<Vec<_>>>()
            })
            .await
            .context("Failed to canonicalize packs_check roots")??;
            PacksCheckAccess::Allowlisted(roots)
        }
    };

    Ok(McpServer::new(
        ApiClient::from_config(&config, &cli.api_url),
        credentials,
        packs_check_access,
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
    use wiremock::{
        matchers::{body_json, method, path, query_param},
        Mock, MockServer, ResponseTemplate,
    };

    fn test_server(api_url: String) -> McpServer {
        test_server_with_access(api_url, PacksCheckAccess::Unrestricted)
    }

    fn test_server_with_access(api_url: String, packs_check_access: PacksCheckAccess) -> McpServer {
        let mut config = CliConfig::default();
        config
            .current_profile_mut()
            .expect("default profile")
            .api_url = api_url;
        McpServer::new(
            ApiClient::from_config(&config, &None),
            None,
            packs_check_access,
        )
    }

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
        let mut server = McpServer::new(
            ApiClient::from_config(&config, &None),
            None,
            PacksCheckAccess::Unrestricted,
        );
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
        assert!(names.contains(&"packs_check"));
        assert!(names.contains(&"rules_update_trace_tag_template"));
        assert!(names.contains(&"queues_update_trace_tag_template"));
        for cache_tool in [
            "cache_namespaces_list",
            "cache_namespace_get",
            "cache_namespace_create",
            "cache_namespace_update",
            "cache_namespace_delete",
            "cache_entry_get",
            "cache_entries_get_many",
            "cache_entries_scan",
            "cache_generations_list",
            "cache_generation_get",
            "cache_refresh_begin",
            "cache_refresh_upload_chunk",
            "cache_refresh_seal",
            "cache_refresh_promote",
            "cache_refresh_abort",
        ] {
            assert!(names.contains(&cache_tool), "missing {cache_tool}");
        }
    }

    #[tokio::test]
    async fn packs_check_returns_structured_local_report() {
        let directory = tempfile::TempDir::new().expect("temp directory");
        std::fs::write(
            directory.path().join("pack.yaml"),
            "ref: mcp_test\nversion: 1.0.0\n",
        )
        .expect("manifest");
        let mut server = test_server("http://127.0.0.1:1".to_string());
        let args = serde_json::from_value(json!({
            "path": directory.path().to_string_lossy()
        }))
        .expect("arguments");

        let report = server
            .call_tool("packs_check", &args)
            .await
            .expect("local check");

        assert_eq!(report["valid"], true);
        assert_eq!(report["pack_ref"], "mcp_test");
        assert_eq!(report["files_checked"], 1);
    }

    #[tokio::test]
    async fn packs_check_preserves_invalid_report_as_tool_success() {
        let directory = tempfile::TempDir::new().expect("temp directory");
        let mut server = test_server("http://127.0.0.1:1".to_string());
        let args = serde_json::from_value(json!({
            "path": directory.path().to_string_lossy()
        }))
        .expect("arguments");

        let report = server
            .call_tool("packs_check", &args)
            .await
            .expect("validation report");

        assert_eq!(report["valid"], false);
        assert_eq!(report["diagnostics"][0]["code"], "manifest.missing");
    }

    #[tokio::test]
    async fn stdio_server_advertises_packs_check() {
        let mut server = test_server("http://127.0.0.1:1".to_string());
        let response = server
            .handle_request(&json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "tools/list"
            }))
            .await
            .expect("tools/list")
            .expect("response");
        let names = response["result"]["tools"]
            .as_array()
            .expect("tools")
            .iter()
            .filter_map(|tool| tool["name"].as_str())
            .collect::<Vec<_>>();

        assert!(names.contains(&"packs_check"));
    }

    #[tokio::test]
    async fn default_http_server_omits_and_rejects_packs_check() {
        let mut server =
            test_server_with_access("http://127.0.0.1:1".to_string(), PacksCheckAccess::Disabled);
        let response = server
            .handle_request(&json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "tools/list"
            }))
            .await
            .expect("tools/list")
            .expect("response");
        let tools = response["result"]["tools"].as_array().expect("tools");
        assert!(!tools.iter().any(|tool| tool["name"] == "packs_check"));

        let args = serde_json::from_value(json!({ "path": "." })).expect("arguments");
        let error = server
            .call_tool("packs_check", &args)
            .await
            .expect_err("packs_check must be disabled");
        assert!(error.to_string().contains("unavailable"));
    }

    #[tokio::test]
    async fn http_packs_check_accepts_path_within_allowlisted_root() {
        let root = tempfile::TempDir::new().expect("root");
        let pack = root.path().join("pack");
        std::fs::create_dir(&pack).expect("pack directory");
        std::fs::write(
            pack.join("pack.yaml"),
            "ref: allowlisted_test\nversion: 1.0.0\n",
        )
        .expect("manifest");
        let canonical_root = root.path().canonicalize().expect("canonical root");
        let mut server = test_server_with_access(
            "http://127.0.0.1:1".to_string(),
            PacksCheckAccess::Allowlisted(vec![canonical_root]),
        );
        let args = serde_json::from_value(json!({ "path": pack })).expect("arguments");

        let report = server
            .call_tool("packs_check", &args)
            .await
            .expect("allowlisted check");

        assert_eq!(report["valid"], true);
        assert_eq!(report["pack_ref"], "allowlisted_test");
    }

    #[tokio::test]
    async fn http_packs_check_rejects_path_outside_allowlisted_root() {
        let root = tempfile::TempDir::new().expect("root");
        let outside = tempfile::TempDir::new().expect("outside");
        let canonical_root = root.path().canonicalize().expect("canonical root");
        let mut server = test_server_with_access(
            "http://127.0.0.1:1".to_string(),
            PacksCheckAccess::Allowlisted(vec![canonical_root]),
        );
        let args = serde_json::from_value(json!({ "path": outside.path() })).expect("arguments");

        let error = server
            .call_tool("packs_check", &args)
            .await
            .expect_err("outside path must be rejected");

        assert!(error.to_string().contains("outside"));
    }

    #[test]
    fn cache_scan_schema_is_bounded_and_redacts_values_by_default() {
        let schema = cache_entries_scan_schema();
        assert_eq!(schema["properties"]["limit"]["maximum"], 1_000);
        assert_eq!(schema["properties"]["include_values"]["default"], false);
        assert_eq!(schema["additionalProperties"], false);
    }

    #[test]
    fn cache_owner_requires_matching_reference() {
        let pack_owner = cache_owner(
            &serde_json::from_value(json!({
                "owner_type": "pack", "owner_pack_ref": "core"
            }))
            .expect("object"),
        )
        .expect("pack owner should parse");
        assert_eq!(pack_owner.query(), "owner_type=pack&owner_ref=core");

        let missing_pack_ref =
            serde_json::from_value(json!({ "owner_type": "pack" })).expect("object");
        assert!(cache_owner(&missing_pack_ref).is_err());
    }

    #[tokio::test]
    async fn cache_namespace_list_dispatches_encoded_filters_and_cursor() {
        let api = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/cache/namespaces"))
            .and(query_param("owner_type", "pack"))
            .and(query_param("owner_ref", "sales/force"))
            .and(query_param("namespace", "alpha users"))
            .and(query_param("freshness", "unpopulated"))
            .and(query_param("limit", "17"))
            .and(query_param("cursor", "next/page + one"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "data": {"namespaces": [], "next_cursor": "another/cursor"}
            })))
            .expect(1)
            .mount(&api)
            .await;

        let mut server = test_server(api.uri());
        let args = serde_json::from_value(json!({
            "owner_type": "pack",
            "owner_pack_ref": "sales/force",
            "namespace": "alpha users",
            "freshness": "unpopulated",
            "limit": 17,
            "cursor": "next/page + one"
        }))
        .expect("arguments");
        let response = server
            .call_tool("cache_namespaces_list", &args)
            .await
            .expect("list should succeed");

        assert_eq!(response["next_cursor"], "another/cursor");
    }

    #[tokio::test]
    async fn cache_generation_list_dispatches_pagination_and_surfaces_api_errors() {
        let api = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/cache/namespaces/users%2Factive/generations"))
            .and(query_param("owner_type", "system"))
            .and(query_param("limit", "2"))
            .and(query_param("cursor", "bad cursor"))
            .respond_with(ResponseTemplate::new(400).set_body_json(json!({
                "error": "cursor page shape mismatch",
                "code": "cache_cursor_invalid"
            })))
            .expect(1)
            .mount(&api)
            .await;

        let mut server = test_server(api.uri());
        let args = serde_json::from_value(json!({
            "owner_type": "system",
            "namespace": "users/active",
            "limit": 2,
            "cursor": "bad cursor"
        }))
        .expect("arguments");
        let error = server
            .call_tool("cache_generations_list", &args)
            .await
            .expect_err("API error should be returned");

        assert!(error.to_string().contains("cache_cursor_invalid"));
        assert!(error.to_string().contains("cursor page shape mismatch"));
    }

    #[tokio::test]
    async fn cache_metadata_continuations_do_not_inject_a_default_limit() {
        let api = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/cache/namespaces"))
            .and(query_param("owner_type", "system"))
            .and(query_param("cursor", "namespace cursor"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "data": {"namespaces": [], "next_cursor": null}
            })))
            .expect(1)
            .mount(&api)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/v1/cache/namespaces/users/generations"))
            .and(query_param("owner_type", "system"))
            .and(query_param("cursor", "generation cursor"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "data": {"generations": [], "next_cursor": null}
            })))
            .expect(1)
            .mount(&api)
            .await;

        let mut server = test_server(api.uri());
        let namespace_args = serde_json::from_value(json!({
            "owner_type": "system",
            "cursor": "namespace cursor"
        }))
        .expect("namespace arguments");
        server
            .call_tool("cache_namespaces_list", &namespace_args)
            .await
            .expect("namespace continuation should succeed");
        let generation_args = serde_json::from_value(json!({
            "owner_type": "system",
            "namespace": "users",
            "cursor": "generation cursor"
        }))
        .expect("generation arguments");
        server
            .call_tool("cache_generations_list", &generation_args)
            .await
            .expect("generation continuation should succeed");

        let requests = api
            .received_requests()
            .await
            .expect("request recording should be enabled");
        assert_eq!(requests.len(), 2);
        for request in requests {
            assert!(
                request.url.query_pairs().all(|(key, _)| key != "limit"),
                "continuation request unexpectedly included a limit: {}",
                request.url
            );
        }
    }

    #[tokio::test]
    async fn empty_refresh_begin_metadata_is_reused_by_write_only_seal() {
        let api = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/v1/cache/namespaces/users/generations"))
            .and(body_json(json!({
                "owner_type": "pack",
                "owner_ref": "salesforce",
                "client_refresh_id": "empty-refresh",
                "expected_active_generation_id": null,
                "expected_chunk_count": 0,
                "expected_record_count": 0,
                "expected_size_bytes": 0,
                "source_revision": "revision/empty"
            })))
            .respond_with(ResponseTemplate::new(201).set_body_json(json!({
                "data": {"generation_id": 42, "status": "staging"}
            })))
            .expect(1)
            .mount(&api)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/v1/cache/namespaces/users/generations/42/seal"))
            .and(body_json(json!({
                "owner_type": "pack",
                "owner_ref": "salesforce",
                "expected_chunk_count": 0,
                "expected_record_count": 0,
                "expected_size_bytes": 0
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "data": {"generation_id": 42, "status": "ready"}
            })))
            .expect(1)
            .mount(&api)
            .await;

        let mut server = test_server(api.uri());
        let begin = serde_json::from_value(json!({
            "owner_type": "pack",
            "owner_pack_ref": "salesforce",
            "namespace": "users",
            "client_refresh_id": "empty-refresh",
            "expected_chunk_count": 0,
            "expected_record_count": 0,
            "expected_size_bytes": 0,
            "source_revision": "revision/empty",
            "expect_empty": true
        }))
        .expect("begin arguments");
        server
            .call_tool("cache_refresh_begin", &begin)
            .await
            .expect("empty begin should succeed");

        let seal = serde_json::from_value(json!({
            "owner_type": "pack",
            "owner_pack_ref": "salesforce",
            "namespace": "users",
            "generation_id": 42
        }))
        .expect("seal arguments");
        let sealed = server
            .call_tool("cache_refresh_seal", &seal)
            .await
            .expect("seal should use retained begin metadata");
        assert_eq!(sealed["status"], "ready");
        assert!(server.cache_refreshes.is_empty());
        assert!(server.cache_refresh_order.is_empty());
    }

    #[tokio::test]
    async fn seal_accepts_explicit_metadata_without_a_generation_read() {
        let api = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/v1/cache/namespaces/users/generations/77/seal"))
            .and(body_json(json!({
                "owner_type": "system",
                "expected_chunk_count": 3,
                "expected_record_count": 21,
                "expected_size_bytes": 900
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "data": {"generation_id": 77, "status": "ready"}
            })))
            .expect(1)
            .mount(&api)
            .await;

        let mut server = test_server(api.uri());
        let args = serde_json::from_value(json!({
            "owner_type": "system",
            "namespace": "users",
            "generation_id": 77,
            "expected_chunk_count": 3,
            "expected_record_count": 21,
            "expected_size_bytes": 900
        }))
        .expect("arguments");
        server
            .call_tool("cache_refresh_seal", &args)
            .await
            .expect("explicit seal should succeed");
    }

    #[test]
    fn retained_cache_refresh_metadata_is_insertion_order_bounded() {
        let mut server = test_server("http://127.0.0.1:1".to_string());
        for generation_id in 0..=MAX_RETAINED_CACHE_REFRESHES as i64 {
            server.remember_cache_refresh(
                format!("refresh-{generation_id}"),
                CacheRefreshMetadata {
                    expected_chunk_count: generation_id,
                    expected_record_count: None,
                    expected_size_bytes: None,
                },
            );
        }

        assert_eq!(server.cache_refreshes.len(), MAX_RETAINED_CACHE_REFRESHES);
        assert_eq!(
            server.cache_refresh_order.len(),
            MAX_RETAINED_CACHE_REFRESHES
        );
        assert!(!server.cache_refreshes.contains_key("refresh-0"));
        assert!(server.cache_refreshes.contains_key("refresh-1"));
        assert!(server
            .cache_refreshes
            .contains_key(&format!("refresh-{MAX_RETAINED_CACHE_REFRESHES}")));
    }

    #[test]
    fn updating_retained_cache_refresh_preserves_order_and_size() {
        let mut server = test_server("http://127.0.0.1:1".to_string());
        for (key, expected_chunk_count) in [("first", 1), ("second", 2)] {
            server.remember_cache_refresh(
                key.to_string(),
                CacheRefreshMetadata {
                    expected_chunk_count,
                    expected_record_count: None,
                    expected_size_bytes: None,
                },
            );
        }

        server.remember_cache_refresh(
            "first".to_string(),
            CacheRefreshMetadata {
                expected_chunk_count: 3,
                expected_record_count: None,
                expected_size_bytes: None,
            },
        );

        assert_eq!(server.cache_refreshes.len(), 2);
        assert_eq!(
            server.cache_refresh_order,
            VecDeque::from(["first".to_string(), "second".to_string()])
        );
        assert_eq!(server.cache_refreshes["first"].expected_chunk_count, 3);
    }

    #[tokio::test]
    async fn promote_and_abandon_remove_retained_refresh_metadata() {
        let api = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path(
                "/api/v1/cache/namespaces/users/generations/41/promote",
            ))
            .and(body_json(json!({
                "owner_type": "system",
                "expected_active_generation_id": null
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "data": {"generation_id": 41, "status": "active"}
            })))
            .expect(1)
            .mount(&api)
            .await;
        Mock::given(method("POST"))
            .and(path(
                "/api/v1/cache/namespaces/users/generations/42/abandon",
            ))
            .and(body_json(json!({"owner_type": "system"})))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "data": {"generation_id": 42, "status": "abandoned"}
            })))
            .expect(1)
            .mount(&api)
            .await;

        let mut server = test_server(api.uri());
        for generation_id in [41, 42] {
            server.remember_cache_refresh(
                cache_refresh_key(
                    &CacheOwner {
                        owner_type: "system".to_string(),
                        owner_ref: None,
                    },
                    "users",
                    generation_id,
                ),
                CacheRefreshMetadata {
                    expected_chunk_count: 1,
                    expected_record_count: None,
                    expected_size_bytes: None,
                },
            );
        }

        let promote_args = serde_json::from_value(json!({
            "owner_type": "system",
            "namespace": "users",
            "generation_id": 41,
            "expect_empty": true
        }))
        .expect("promote arguments");
        server
            .call_tool("cache_refresh_promote", &promote_args)
            .await
            .expect("promotion should succeed");
        let abandon_args = serde_json::from_value(json!({
            "owner_type": "system",
            "namespace": "users",
            "generation_id": 42
        }))
        .expect("abandon arguments");
        server
            .call_tool("cache_refresh_abort", &abandon_args)
            .await
            .expect("abandonment should succeed");

        assert!(server.cache_refreshes.is_empty());
        assert!(server.cache_refresh_order.is_empty());
    }

    #[test]
    fn build_config_applies_token_overrides() {
        let cli = Cli {
            profile: None,
            api_url: None,
            transport: Transport::Stdio,
            listen_addr: "127.0.0.1:8090".to_string(),
            packs_check_roots: Vec::new(),
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
            packs_check_roots: Vec::new(),
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
            packs_check_roots: Vec::new(),
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
