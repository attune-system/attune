//! Read-only observations for an exclusively owned acceptance deployment.
//! Never returns authority/proof bodies, origins, configuration or ciphertext.
use super::{
    execution_runtime_admission::ExecutionAdmissionRepository,
    runtime_authority::{RuntimeAuthorityRepository, RuntimeOwner},
    schema_contract::{BindingOwner, SchemaContractRepository},
};
use crate::{Error, Result};
use hmac::{Hmac, KeyInit, Mac};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::Sha256;
use sqlx::PgPool;
use std::collections::BTreeSet;

fn denied() -> Error {
    Error::invalid_state("Owned interoperability observation rejected")
}
fn digest<T: Serialize>(value: &T) -> Result<String> {
    crate::value_schema::digest(&serde_json::to_value(value)?).map_err(|_| denied())
}

fn hex(value: &Value, length: usize) -> bool {
    value
        .as_str()
        .is_some_and(|s| s.len() == length && s.bytes().all(|b| b.is_ascii_hexdigit()))
}
fn validate_receipt(value: &Value) -> Result<()> {
    let object = value.as_object().ok_or_else(denied)?;
    match value.get("protocol").and_then(Value::as_str) {
        Some("interop-v1") => {
            if object.keys().any(|k| {
                ![
                    "protocol",
                    "nonce",
                    "api_token_present",
                    "runtime_read_status",
                    "input_sha256",
                    "runtime_value_sha256",
                ]
                .contains(&k.as_str())
            }) || !hex(&value["nonce"], 24)
                || !value["api_token_present"].is_boolean()
                || !(value["runtime_read_status"].is_null()
                    || matches!(value["runtime_read_status"].as_i64(), Some(200 | 201)))
                || value.get("input_sha256").is_some_and(|v| !hex(v, 64))
                || value
                    .get("runtime_value_sha256")
                    .is_some_and(|v| !hex(v, 64))
            {
                return Err(denied());
            }
        }
        Some("interop-batch-v1") => {
            if object
                .keys()
                .any(|k| !["protocol", "api_token_present", "items"].contains(&k.as_str()))
                || value["api_token_present"] != false
            {
                return Err(denied());
            }
            let items = value["items"].as_array().ok_or_else(denied)?;
            if items.is_empty() || items.len() > 16 {
                return Err(denied());
            }
            for item in items {
                if item.as_object().is_none_or(|o| {
                    o.len() != 3
                        || o.keys()
                            .any(|k| !["id", "nonce", "secret_sha256"].contains(&k.as_str()))
                }) || item["id"].as_i64().is_none_or(|id| id <= 0)
                    || !hex(&item["nonce"], 24)
                    || !hex(&item["secret_sha256"], 64)
                {
                    return Err(denied());
                }
            }
        }
        _ => return Err(denied()),
    }
    Ok(())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Access {
    pub run_id: String,
    pub project: String,
    pub pack_id: i64,
    pub pack_ref: String,
    pub requester_ids: Vec<i64>,
    pub release_digests: Vec<String>,
    pub receipt_key: String,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub run_id: String,
    pub project: String,
    pub nonce: String,
    pub operation: Operation,
}
#[derive(Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Operation {
    Execution {
        execution_id: i64,
        expected_receipt: Option<Value>,
    },
    Discover {
        binding_kind: String,
        case_nonce: String,
    },
    QueueBatch {
        queue_ref: String,
        item_ids: Vec<i64>,
    },
}
#[derive(Serialize)]
pub struct Response {
    protocol: u8,
    request: Request,
    observations: Vec<Value>,
    mac: String,
}

impl Access {
    pub fn validate(&self, request: &Request) -> Result<()> {
        if request.run_id != self.run_id
            || request.project != self.project
            || self.run_id.is_empty()
            || self.run_id.len() > 20
            || !self.run_id.as_bytes()[0].is_ascii_lowercase()
            || !self
                .run_id
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
            || !self
                .project
                .starts_with(&format!("eaccept-{}-", self.run_id))
            || self.project.len() > 128
            || !self
                .project
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'-' | b'_'))
            || self.pack_ref != format!("ginterop_{}", self.run_id.replace('-', "_"))
            || self.pack_id <= 0
            || self.requester_ids.is_empty()
            || self.requester_ids.len() > 5
            || self.requester_ids.iter().any(|id| *id <= 0)
            || self.requester_ids.iter().collect::<BTreeSet<_>>().len() != self.requester_ids.len()
            || self.release_digests.is_empty()
            || self.release_digests.len() > 8
            || self.release_digests.iter().any(|s| {
                s.len() != 64
                    || !s
                        .bytes()
                        .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
            })
            || self.receipt_key.len() < 32
            || self.receipt_key.len() > 256
            || request.nonce.len() != 24
            || !request.nonce.bytes().all(|b| b.is_ascii_hexdigit())
        {
            return Err(denied());
        }
        match &request.operation {
            Operation::Execution {
                execution_id,
                expected_receipt,
            } if *execution_id > 0 => {
                if let Some(receipt) = expected_receipt {
                    validate_receipt(receipt)?;
                }
            }
            Operation::Discover {
                binding_kind,
                case_nonce,
            } if ["action", "workflow_task", "rule", "queue"].contains(&binding_kind.as_str())
                && case_nonce.len() == 24
                && case_nonce.bytes().all(|b| b.is_ascii_hexdigit()) =>
            {
                ()
            }
            Operation::QueueBatch {
                queue_ref,
                item_ids,
            } if queue_ref.starts_with(&(self.pack_ref.clone() + "."))
                && queue_ref.len() <= 255
                && queue_ref.bytes().all(|b| {
                    b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'_' | b'-' | b'.')
                })
                && !item_ids.is_empty()
                && item_ids.len() <= 16
                && item_ids.iter().all(|id| *id > 0)
                && item_ids.iter().collect::<BTreeSet<_>>().len() == item_ids.len() =>
            {
                ()
            }
            _ => return Err(denied()),
        }
        Ok(())
    }
}

pub struct InteroperabilityInspectionRepository;
impl InteroperabilityInspectionRepository {
    pub async fn inspect(
        pool: &PgPool,
        schema: &str,
        access: &Access,
        request: Request,
    ) -> Result<Response> {
        access.validate(&request)?;
        if schema.is_empty()
            || schema.len() > 63
            || !schema
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_')
        {
            return Err(denied());
        }
        let mut tx = pool.begin().await?;
        sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY")
            .execute(&mut *tx)
            .await?;
        sqlx::query("SELECT set_config('search_path',quote_ident($1),true),set_config('statement_timeout','5s',true)").bind(schema).execute(&mut *tx).await?;
        let observer: bool = sqlx::query_scalar("SELECT NOT r.rolsuper AND NOT r.rolcreaterole AND NOT r.rolcreatedb AND NOT r.rolbypassrls AND NOT has_table_privilege(current_user,'execution','INSERT,UPDATE,DELETE') FROM pg_roles r WHERE r.rolname=current_user").fetch_one(&mut *tx).await?;
        if !observer {
            return Err(denied());
        }
        let ids: Vec<i64> = match &request.operation {
            Operation::Execution { execution_id, .. } => vec![*execution_id],
            Operation::Discover { binding_kind, case_nonce } => {
                sqlx::query_scalar("SELECT e.id FROM execution e JOIN runtime_authority_admission a ON a.owner_kind='execution' AND a.owner_id=e.id WHERE a.authority->'owner'->>'pack_id'=$1 AND a.authority->'binding'->>'kind'=$2 AND e.config->>'nonce'=$3 ORDER BY e.id LIMIT 17")
                    .bind(access.pack_id.to_string()).bind(binding_kind).bind(case_nonce).fetch_all(&mut *tx).await?
            }
            Operation::QueueBatch { queue_ref, item_ids } => {
                let owned: i64 = sqlx::query_scalar("SELECT count(*) FROM work_queue_item i JOIN work_queue q ON q.id=i.queue WHERE i.id=ANY($1) AND q.ref=$2 AND q.pack=$3 AND i.requested_by_identity=ANY($4)")
                    .bind(item_ids).bind(queue_ref).bind(access.pack_id).bind(&access.requester_ids).fetch_one(&mut *tx).await?;
                if owned as usize != item_ids.len() { return Err(denied()); }
                sqlx::query_scalar("SELECT a.owner_id FROM runtime_authority_admission a WHERE a.owner_kind='execution' AND a.authority->'owner'->>'pack_id'=$1 AND a.authority->'binding'->>'queue_ref'=$2 AND a.authority->'attribution'->>'kind'='queue_batch' AND EXISTS(SELECT 1 FROM jsonb_object_keys(a.authority->'attribution'->'items') k WHERE k::BIGINT=ANY($3)) ORDER BY a.owner_id LIMIT 17")
                    .bind(access.pack_id.to_string()).bind(queue_ref).bind(item_ids).fetch_all(&mut *tx).await?
            }
        };
        if ids.len() > 16 {
            return Err(denied());
        }
        let mut observations = Vec::new();
        for id in ids {
            let admission =
                ExecutionAdmissionRepository::find(&mut tx, RuntimeOwner::Execution(id))
                    .await?
                    .ok_or_else(denied)?;
            let policy =
                RuntimeAuthorityRepository::find_policy(&mut tx, admission.policy_revision)
                    .await?
                    .ok_or_else(denied)?;
            admission.authority.validate_against(&policy.policy)?;
            if digest(&admission.authority.0)? != admission.authority_digest
                || admission.authority.policy_id != admission.policy_revision
                || admission.authority.owner.pack_id != access.pack_id
                || admission.authority.owner.pack_ref != access.pack_ref
                || !access
                    .release_digests
                    .contains(&admission.authority.target_release_digest)
            {
                return Err(denied());
            }
            let row: (String, Option<i64>, i64, String, Option<i64>) = sqlx::query_as("SELECT status::TEXT,executor,pack_release,pack_release_digest,worker FROM execution WHERE id=$1")
                .bind(id).fetch_one(&mut *tx).await?;
            if row.2 != admission.authority.target_release_id
                || row.3 != admission.authority.target_release_digest
            {
                return Err(denied());
            }
            let executable_matches:bool=sqlx::query_scalar("SELECT executable_snapshot #>> '{release,id}'=$2 AND executable_snapshot #>> '{release,digest}'=$3 AND executable_snapshot #>> '{executable,action,ref}'=$4 FROM execution WHERE id=$1")
                .bind(id).bind(row.2.to_string()).bind(&row.3).bind(admission.authority.binding.action_ref()).fetch_one(&mut *tx).await?;
            if !executable_matches {
                return Err(denied());
            }
            let attribution = match &admission.authority.attribution {
                crate::runtime_authority::RequestAttribution::Human { actor } => {
                    if !access.requester_ids.contains(&actor.identity_id) {
                        return Err(denied());
                    }
                    json!({"kind":"human","identity_id":actor.identity_id})
                }
                crate::runtime_authority::RequestAttribution::QueueBatch { queue_ref, items } => {
                    if items.values().any(|actor| {
                        actor
                            .as_ref()
                            .is_none_or(|actor| !access.requester_ids.contains(&actor.identity_id))
                    }) {
                        return Err(denied());
                    }
                    json!({"kind":"queue_batch","queue_ref":queue_ref,"items":items.iter().map(|(id,actor)| (id.to_string(),actor.as_ref().map(|a|a.identity_id))).collect::<std::collections::BTreeMap<_,_>>()})
                }
                crate::runtime_authority::RequestAttribution::Rule { rule_ref } => {
                    json!({"kind":"rule","rule_ref":rule_ref})
                }
                crate::runtime_authority::RequestAttribution::ParentExecution { execution_id } => {
                    json!({"kind":"parent_execution","execution_id":execution_id})
                }
            };
            let mut approvals = Vec::new();
            let approval_ids: BTreeSet<_> = admission
                .authority
                .grants
                .iter()
                .filter_map(|g| g.approval_id())
                .collect();
            for aid in approval_ids {
                let approval = RuntimeAuthorityRepository::find_approval(&mut tx, aid)
                    .await?
                    .ok_or_else(denied)?;
                if approval.subject_pack != access.pack_id {
                    return Err(denied());
                }
                approval.approval()?;
                let expired:bool=sqlx::query_scalar("SELECT COALESCE(expires_at<=clock_timestamp(),false) FROM runtime_external_approval WHERE id=$1").bind(aid).fetch_one(&mut *tx).await?;
                approvals.push(json!({"id":aid,"digest":approval.approval_digest,"expires_at":approval.expires_at,"expired":expired,"revoked_at":approval.revoked_at}));
            }
            // Exact immutable input selections, including each consumed fixed
            // input approval. Do NOT infer it from all approvals for a declaration.
            let receipts: Vec<(i64, i64, i64, Option<i64>, Value, String)> = sqlx::query_as("WITH RECURSIVE closure(id) AS (SELECT id FROM runtime_input_receipt WHERE execution=$1 UNION SELECT edge.source_receipt FROM runtime_input_receipt_source edge JOIN closure c ON c.id=edge.receipt) SELECT r.id,r.execution,r.renderer_admission,r.declaration,CASE WHEN octet_length(r.proof::TEXT)<=1048576 THEN r.proof END,r.proof_digest FROM runtime_input_receipt r JOIN closure c ON c.id=r.id ORDER BY r.id LIMIT 257")
                .bind(id).fetch_all(&mut *tx).await?;
            if receipts.len() > 256 {
                return Err(denied());
            }
            let mut input_receipts = Vec::new();
            for (receipt, execution, renderer, declaration_id, proof, expected) in receipts {
                if digest(&proof)? != expected
                    || proof.pointer("/site/execution").and_then(Value::as_i64) != Some(execution)
                    || proof.get("renderer").and_then(Value::as_i64) != Some(renderer)
                    || proof.get("declaration").and_then(Value::as_i64) != declaration_id
                {
                    return Err(denied());
                }
                let mut selected = BTreeSet::new();
                for selection in proof
                    .get("selections")
                    .and_then(Value::as_array)
                    .ok_or_else(denied)?
                {
                    if selection.pointer("/source/purpose").and_then(Value::as_str)
                        == Some("authored")
                    {
                        if let Some(approval) = selection
                            .pointer("/source/input_authority")
                            .and_then(Value::as_i64)
                        {
                            selected.insert(approval);
                        }
                    }
                }
                let mut fixed = Vec::new();
                for aid in selected {
                    let approval = RuntimeAuthorityRepository::find_approval(&mut tx, aid)
                        .await?
                        .ok_or_else(denied)?;
                    if approval.subject_pack != access.pack_id {
                        return Err(denied());
                    }
                    approval.approval()?;
                    let declaration = proof
                        .get("declaration")
                        .and_then(Value::as_i64)
                        .ok_or_else(denied)?;
                    let retained: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM runtime_declared_input_approval a JOIN runtime_input_only_approval p ON p.approval=a.approval WHERE a.declaration=$1 AND a.approval=$2)").bind(declaration).bind(aid).fetch_one(&mut *tx).await?;
                    if !retained {
                        return Err(denied());
                    }
                    fixed.push(json!({"id":aid,"digest":approval.approval_digest,"declaration_id":declaration}));
                }
                input_receipts
                    .push(json!({"id":receipt,"proof_digest":expected,"input_approvals":fixed}));
            }
            let contributors: Vec<(i64,i32,Value,String,bool)> = sqlx::query_as("SELECT c.item,c.expected_items,CASE WHEN octet_length(c.authority::TEXT)<=1048576 THEN c.authority END,c.authority_digest,EXISTS(SELECT 1 FROM recorded_execution_input_authority original WHERE original.owner_kind='queue_item' AND original.owner_id=c.item AND original.authority_digest=c.authority_digest AND original.authority=c.authority) FROM recorded_queue_execution_input c WHERE c.execution=$1 ORDER BY c.item,c.contributor LIMIT 257").bind(id).fetch_all(&mut *tx).await?;
            if contributors.len() > 256 {
                return Err(denied());
            }
            let mut contributor_metadata = Vec::new();
            for (item, expected_items, authority, expected, retained_match) in contributors {
                let actor = authority
                    .get("identity_id")
                    .and_then(Value::as_i64)
                    .ok_or_else(denied)?;
                if digest(&authority)? != expected
                    || !access.requester_ids.contains(&actor)
                    || !retained_match
                {
                    return Err(denied());
                }
                contributor_metadata.push(json!({"item_id":item,"identity_id":actor,"expected_items":expected_items,"authority_digest":expected,"retained_item_authority_match":retained_match}));
            }
            let receipt_matches = if let Operation::Execution {
                expected_receipt: Some(receipt),
                ..
            } = &request.operation
            {
                Some(
                    sqlx::query_scalar::<_, bool>(
                        "SELECT COALESCE(result->'data'=$2,false) FROM execution WHERE id=$1",
                    )
                    .bind(id)
                    .bind(receipt)
                    .fetch_one(&mut *tx)
                    .await?,
                )
            } else {
                None
            };
            let schema_binding =
                SchemaContractRepository::find(&mut tx, BindingOwner::Execution, id).await?;
            let schema_binding_digest = schema_binding.as_ref().map(digest).transpose()?;
            let entrypoint:Option<String> = sqlx::query_scalar("SELECT CASE WHEN octet_length(executable_snapshot #>> '{executable,action,entrypoint}')<=256 THEN executable_snapshot #>> '{executable,action,entrypoint}' END FROM execution WHERE id=$1").bind(id).fetch_one(&mut *tx).await?;
            observations.push(json!({"execution_id":id,"status":row.0,"worker_id":row.4,"executor_identity":row.1,"admission_id":admission.id,"policy_revision":admission.policy_revision,"policy_digest":policy.policy_digest,"authority_digest":admission.authority_digest,"binding":admission.authority.binding,"selected_refs":admission.authority.selected_refs,"release_id":row.2,"release_digest":row.3,"schema_binding_digest":schema_binding_digest,"entrypoint":entrypoint,"attribution":attribution,"runtime_approvals":approvals,"input_receipts":input_receipts,"queue_contributors":contributor_metadata,"receipt_matches":receipt_matches}));
        }
        tx.commit().await?;
        let mut signer =
            Hmac::<Sha256>::new_from_slice(access.receipt_key.as_bytes()).map_err(|_| denied())?;
        signer.update(&serde_jcs::to_vec(&(1u8, &request, &observations)).map_err(|_| denied())?);
        let mac = signer
            .finalize()
            .into_bytes()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        Ok(Response {
            protocol: 1,
            request,
            observations,
            mac,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn scope_rejects_foreign_run_duplicate_actors_and_unbounded_queries() {
        let mut access = Access {
            run_id: "g1".into(),
            project: "eaccept-g1-sharedfile".into(),
            pack_id: 1,
            pack_ref: "ginterop_g1".into(),
            requester_ids: vec![2, 3],
            release_digests: vec!["a".repeat(64)],
            receipt_key: "x".repeat(32),
        };
        let mut request = Request {
            run_id: access.run_id.clone(),
            project: access.project.clone(),
            nonce: "a".repeat(24),
            operation: Operation::Execution {
                execution_id: 4,
                expected_receipt: None,
            },
        };
        assert!(access.validate(&request).is_ok());
        request.project = "foreign".into();
        assert!(access.validate(&request).is_err());
        request.project = access.project.clone();
        access.requester_ids.push(2);
        assert!(access.validate(&request).is_err());
        access.requester_ids.pop();
        request.operation = Operation::QueueBatch {
            queue_ref: "other.queue".into(),
            item_ids: vec![1],
        };
        assert!(access.validate(&request).is_err());
    }

    #[test]
    fn receipt_comparison_accepts_only_hashes_booleans_and_known_identifiers() {
        let valid = json!({"protocol":"interop-v1","nonce":"a".repeat(24),"api_token_present":false,"runtime_read_status":null,"input_sha256":"b".repeat(64)});
        assert!(validate_receipt(&valid).is_ok());
        let mut unsafe_value = valid.clone();
        unsafe_value["plaintext"] = json!("not-permitted");
        assert!(validate_receipt(&unsafe_value).is_err());
        unsafe_value = valid;
        unsafe_value["input_sha256"] = json!("not-a-digest");
        assert!(validate_receipt(&unsafe_value).is_err());
        assert!(validate_receipt(&json!({"protocol":"interop-batch-v1","api_token_present":false,"items":[{"id":1,"nonce":"a".repeat(24),"secret_sha256":"b".repeat(64)}]})).is_ok());
    }
}
