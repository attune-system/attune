//! Public declared-input consent contract. This never selects execution API grants.
use anyhow::{bail, Context, Result};
use attune_common::{
    rbac::{Action, Resource},
    runtime_authority::{InvocationBinding, PermissionGrantDefinition},
    schema::RefValidator,
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::client::ApiClient;

pub async fn discover(
    client: &mut ApiClient,
    pack: &str,
    policy_revision: Option<i64>,
    page: u32,
    page_size: u32,
) -> Result<
    crate::client::PaginatedResponse<
        attune_common::repositories::public_input_approval::DeclaredInputApprovalTarget,
    >,
> {
    collection_path(pack)?;
    if page == 0 || !(1..=100).contains(&page_size) || policy_revision.is_some_and(|id| id <= 0) {
        bail!("Target discovery needs positive page/policy revision and page_size 1 to 100");
    }
    let mut path =
        format!("/packs/{pack}/input-approval-targets?page={page}&page_size={page_size}");
    if let Some(policy) = policy_revision {
        path.push_str(&format!("&policy_revision={policy}"));
    }
    client.get_paginated_response(&path).await
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct InputApprovalRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub renderer_execution: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub policy_revision: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub invocation_binding: Option<InvocationBinding>,
    pub declaration_revision: String,
    pub source_path: String,
    pub expression: String,
    pub grants: Vec<PermissionGrantDefinition>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<DateTime<Utc>>,
}

impl InputApprovalRequest {
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        // Do not attach serde's untrusted input values to error output.
        let request: Self = serde_json::from_slice(bytes).map_err(|_| anyhow::anyhow!(
            "Invalid input approval JSON. Use renderer_execution OR policy_revision with invocation_binding; include declaration_revision, source_path, expression, grants and optional RFC3339 expires_at. Unknown fields are rejected."
        ))?;
        request.validate()?;
        Ok(request)
    }

    pub fn validate(&self) -> Result<()> {
        match (self.renderer_execution, self.policy_revision, &self.invocation_binding) {
            (Some(id), None, None) if id > 0 => {},
            (None, Some(id), Some(binding)) if id > 0 => {
                RefValidator::validate_component_ref(binding.action_ref())
                    .map_err(|_| anyhow::anyhow!("Invalid invocation action ref"))?;
                match binding {
                    InvocationBinding::Action { .. } => {},
                    InvocationBinding::WorkflowTask { workflow_ref, task_path, .. } => {
                        RefValidator::validate_component_ref(workflow_ref)
                            .map_err(|_| anyhow::anyhow!("Invalid workflow ref"))?;
                        if !task_path.starts_with("/tasks/") || task_path.contains("//") {
                            bail!("Workflow invocation task_path must be an exact /tasks/... JSON pointer");
                        }
                    },
                    InvocationBinding::Rule { rule_ref, .. } => {
                        RefValidator::validate_component_ref(rule_ref)
                            .map_err(|_| anyhow::anyhow!("Invalid rule ref"))?;
                    },
                    InvocationBinding::Queue { queue_ref, .. } => {
                        RefValidator::validate_work_queue_ref(queue_ref)
                            .map_err(|_| anyhow::anyhow!("Invalid queue ref"))?;
                    },
                }
            },
            _ => bail!("Choose a positive renderer_execution OR positive policy_revision with invocation_binding"),
        }
        if self.declaration_revision.is_empty()
            || self.source_path.is_empty()
            || self.expression.is_empty()
        {
            bail!("declaration_revision, source_path and expression must identify the exact immutable declaration");
        }
        if !self.source_path.starts_with('/') {
            bail!("source_path must be the declaration's exact JSON pointer");
        }
        if self.grants.is_empty() || self.grants.len() > 256 {
            bail!("Input approval requires 1 to 256 fixed grants");
        }
        let mut grants = Vec::with_capacity(self.grants.len());
        for definition in &self.grants {
            let grant = definition.clone().into_external_approval_grant()
                .context("Input grants must have fixed external scope and refs, not server-owned IDs or human-relative scope")?;
            if !matches!(grant.resource, Resource::Keys | Resource::Packs)
                || grant.actions.is_empty()
                || grant
                    .actions
                    .iter()
                    .any(|action| !matches!(action, Action::Read | Action::Decrypt))
            {
                bail!("Input-only approval supports keys/packs read/decrypt, not execution SDK/API permissions");
            }
            grants.push(grant);
        }
        attune_common::runtime_authority::validate_external_approval_grants(&grants)?;
        if self.expires_at.is_some_and(|expiry| expiry <= Utc::now()) {
            bail!("expires_at must be in the future or omitted");
        }
        Ok(())
    }
}

pub fn collection_path(pack: &str) -> Result<String> {
    RefValidator::validate_pack_ref(pack).map_err(|_| anyhow::anyhow!("Invalid pack ref"))?;
    Ok(format!("/packs/{pack}/external-approvals"))
}

pub fn record_path(pack: &str, id: i64) -> Result<String> {
    if id <= 0 {
        bail!("Approval ID must be positive");
    }
    Ok(format!("{}/{id}", collection_path(pack)?))
}

pub fn issue_path(pack: &str) -> Result<String> {
    collection_path(pack)?;
    Ok(format!("/packs/{pack}/external-input-approvals"))
}

pub async fn issue(
    client: &mut ApiClient,
    pack: &str,
    request: &InputApprovalRequest,
) -> Result<Value> {
    request.validate()?;
    client.post(&issue_path(pack)?, request).await
}

pub async fn get(client: &mut ApiClient, pack: &str, id: i64) -> Result<Value> {
    client.get(&record_path(pack, id)?).await
}

pub async fn list(client: &mut ApiClient, pack: &str) -> Result<Vec<Value>> {
    client.get_paginated(&collection_path(pack)?).await
}

pub async fn revoke(client: &mut ApiClient, pack: &str, id: i64, confirmed: bool) -> Result<Value> {
    if !confirmed {
        bail!("Revoking an input approval requires explicit confirmation");
    }
    client
        .post(&format!("{}/revoke", record_path(pack, id)?), &json!({}))
        .await
}

pub fn issue_schema() -> Value {
    let binding = |kind: &str, fields: &[&str]| {
        let mut properties = serde_json::Map::new();
        properties.insert("kind".into(), json!({"const":kind}));
        for field in fields {
            properties.insert((*field).into(), json!({"type":"string","minLength":1}));
        }
        let mut required = vec!["kind"];
        required.extend_from_slice(fields);
        json!({"type":"object","properties":properties,"required":required,"additionalProperties":false})
    };
    json!({"type":"object","properties":{
        "ref":{"type":"string","description":"Subject pack ref"},
        "renderer_execution":{"type":"integer","minimum":1},
        "policy_revision":{"type":"integer","minimum":1,"description":"Admitted policy revision, usable before execution"},
        "invocation_binding":{"oneOf":[binding("action", &["action_ref"]),binding("workflow_task", &["workflow_ref","task_path","action_ref"]),binding("rule", &["rule_ref","action_ref"]),binding("queue", &["queue_ref","action_ref"])]},
        "declaration_revision":{"type":"string","minLength":1,"description":"Exact retained declaration revision; its input purpose is server-bound"},
        "source_path":{"type":"string","pattern":"^/","description":"Exact immutable declaration JSON pointer"},
        "expression":{"type":"string","minLength":1,"description":"Exact authored expression, not evaluated plaintext"},
        "grants":{"type":"array","minItems":1,"maxItems":256,"items":{"type":"object","properties":{
            "resource":{"enum":["keys","packs"]},"actions":{"type":"array","minItems":1,"items":{"enum":["read","decrypt"]}},
            "scope":{"const":"external"},"constraints":{"type":"object","description":"Fixed canonical grant constraints using refs, never ids/managing_pack_id", "properties":{
                "refs":{"type":"array","minItems":1,"items":{"type":"string","minLength":1}},
                "pack_refs":{"type":"array","minItems":1,"items":{"type":"string","minLength":1}},
                "owner_refs":{"type":"array","minItems":1,"items":{"type":"string","minLength":1}},
                "owner":{"enum":["any","none"]},"execution_scope":{"const":"any"},
                "owner_types":{"type":"array","minItems":1,"items":{"enum":["system","identity","pack","action","sensor"]}},
                "visibility":{"type":"array","minItems":1,"items":{"enum":["public","private"]}},
                "encrypted":{"type":"boolean"},"attributes":{"type":"object","maxProperties":0}
            },"additionalProperties":false}
        },"required":["resource","actions"],"additionalProperties":false}},
        "expires_at":{"type":["string","null"],"format":"date-time"}
    },"required":["ref","declaration_revision","source_path","expression","grants"],
    "oneOf":[{"required":["renderer_execution"],"not":{"anyOf":[{"required":["policy_revision"]},{"required":["invocation_binding"]}]}},{"required":["policy_revision","invocation_binding"],"not":{"required":["renderer_execution"]}}],"additionalProperties":false})
}
