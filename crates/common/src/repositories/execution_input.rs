//! Selected execution input is restored only after its recorded origins have
//! been admitted. The borrowed transaction retains those source locks through
//! later operation waits; receipt deadlines must be rechecked at the effect.
use super::{
    execution::ExecutionRepository, execution_secret_value::ExecutionSecretValueRepository,
    immutable_input_origin, inherited_input::ReceiptContext, schema_type::SchemaTypeRepository,
    FindById,
};
use crate::{
    secret_values::{restore_secret_values, StoredSecretValue, ENTITY_EXECUTION_CONFIG},
    Error, Result,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::{Postgres, Transaction};

fn denied() -> Error {
    Error::PermissionDenied("Execution input unavailable".into())
}

/// Private CP evidence, deliberately not serializable as a general DTO. Exact
/// ciphertext AND recorded origins distinguish snapshots with equal plaintext.
#[derive(Clone, PartialEq)]
pub struct ExecutionInputSnapshot {
    execution: i64,
    executor: Option<i64>,
    config: Value,
    secrets: Vec<StoredSecretValue>,
}
impl ExecutionInputSnapshot {
    pub(crate) fn execution(&self) -> i64 {
        self.execution
    }
    pub fn matches_config(&self, config: &Value) -> bool {
        &self.config == config
    }
}

/// Explicitly private encrypted-context projection. The public snapshot and
/// selected-input wrappers do not implement Serialize/Deserialize.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct FrozenInputSnapshot {
    execution: i64,
    executor: Option<i64>,
    config: Value,
    secrets: Vec<FrozenSecret>,
}
impl FrozenInputSnapshot {
    /// The origin dependency frontier must be captured before remote delivery
    /// of inherited/ordinary secrets is enabled. Until then, the executable
    /// consumer admits only the source-complete public-input case.
    pub(crate) fn public_snapshot(&self) -> Result<ExecutionInputSnapshot> {
        if !self.secrets.is_empty() {
            return Err(denied());
        }
        Ok(ExecutionInputSnapshot {
            execution: self.execution,
            executor: self.executor,
            config: self.config.clone(),
            secrets: Vec::new(),
        })
    }
}
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct FrozenSecret {
    json_path: String,
    encrypted_value: Value,
    encryption_key_hash: Option<String>,
    source_kind: String,
    source_ref: Option<String>,
}
impl From<&ExecutionInputSnapshot> for FrozenInputSnapshot {
    fn from(snapshot: &ExecutionInputSnapshot) -> Self {
        Self {
            execution: snapshot.execution,
            executor: snapshot.executor,
            config: snapshot.config.clone(),
            secrets: snapshot
                .secrets
                .iter()
                .map(|value| FrozenSecret {
                    json_path: value.json_path.clone(),
                    encrypted_value: value.encrypted_value.clone(),
                    encryption_key_hash: value.encryption_key_hash.clone(),
                    source_kind: value.source_kind.clone(),
                    source_ref: value.source_ref.clone(),
                })
                .collect(),
        }
    }
}

pub struct SelectedExecutionInput {
    pub(crate) stdin_json: String,
    pub(crate) snapshot: ExecutionInputSnapshot,
}
impl SelectedExecutionInput {
    /// Trusted producer data, not a request DTO. Freeze reauthorizes the exact
    /// source snapshot and compares restored values before accepting these bytes.
    pub fn new(value: Value, snapshot: ExecutionInputSnapshot) -> Result<Self> {
        if !value.is_object() {
            return Err(denied());
        }
        let stdin_json = serde_json::to_string(&value).map_err(|_| denied())?;
        if stdin_json.len() > crate::workload_stdin::MAX_PRIVATE_STDIN_BYTES {
            return Err(denied());
        }
        Ok(Self {
            stdin_json,
            snapshot,
        })
    }
}
impl std::fmt::Debug for SelectedExecutionInput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SelectedExecutionInput")
            .field("bytes", &self.stdin_json.len())
            .field("source", &self.snapshot)
            .finish()
    }
}
impl std::fmt::Debug for ExecutionInputSnapshot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ExecutionInputSnapshot")
            .field("execution", &self.execution)
            .field("secret_count", &self.secrets.len())
            .finish_non_exhaustive()
    }
}

/// Cannot be constructed or deserialized by callers. No transaction commit or
/// savepoint rollback can occur through this object while its admission lives.
pub struct AuthorizedExecutionInput<'tx, 'connection> {
    transaction: &'tx mut Transaction<'connection, Postgres>,
    value: Value,
    snapshot: ExecutionInputSnapshot,
    receipts: Vec<ReceiptContext>,
    origin_fence: super::runtime_authority::ApprovalEffectFence,
}
impl AuthorizedExecutionInput<'_, '_> {
    pub fn value(&self) -> &Value {
        &self.value
    }
    pub fn snapshot(&self) -> &ExecutionInputSnapshot {
        &self.snapshot
    }
    pub fn secret_paths(&self) -> Vec<String> {
        self.snapshot
            .secrets
            .iter()
            .map(|secret| secret.json_path.clone())
            .collect()
    }
    pub fn connection(&mut self) -> &mut sqlx::PgConnection {
        self.transaction
    }
    /// Recheck after every later lock wait and immediately before disclosure or
    /// an acknowledged effect. Each receipt is bound to this same transaction.
    pub async fn ensure_fresh(&mut self) -> Result<()> {
        self.origin_fence.ensure_fresh_on(self.transaction).await?;
        for receipt in &self.receipts {
            receipt.ensure_fresh_on(self.transaction).await?;
        }
        Ok(())
    }
    /// Release only the Rust borrow. The caller still owns the transaction and
    /// must commit it before returning data, checking this clock fence again.
    pub fn into_parts(self) -> (Value, Vec<String>, ExecutionInputSnapshot, InputExpiryFence) {
        let paths = self.secret_paths();
        (
            self.value,
            paths,
            self.snapshot,
            InputExpiryFence {
                receipts: self.receipts,
                origin_fence: self.origin_fence,
            },
        )
    }
}
/// Post-COMMIT expiry check only; this is never a new authorization grant.
pub struct InputExpiryFence {
    receipts: Vec<ReceiptContext>,
    origin_fence: super::runtime_authority::ApprovalEffectFence,
}
impl InputExpiryFence {
    pub async fn ensure_fresh_on(&self, connection: &mut sqlx::PgConnection) -> Result<()> {
        self.origin_fence.ensure_fresh_on(connection).await?;
        for receipt in &self.receipts {
            receipt.ensure_fresh_on(connection).await?;
        }
        Ok(())
    }
    pub fn ensure_fresh(&self) -> Result<()> {
        self.origin_fence.ensure_fresh()?;
        for receipt in &self.receipts {
            receipt.ensure_fresh()?;
        }
        Ok(())
    }
}

pub struct ExecutionInputRepository;
impl ExecutionInputRepository {
    /// Call before target/operation locks. Existing canonical receipt and
    /// ordinary-origin consumers preserve captured input grants, contributor
    /// separation and external approval revocation; no current-IAM substitute.
    pub async fn restore<'tx, 'connection>(
        transaction: &'tx mut Transaction<'connection, Postgres>,
        execution: i64,
        encryption_key: Option<&str>,
        expected: Option<&ExecutionInputSnapshot>,
    ) -> Result<AuthorizedExecutionInput<'tx, 'connection>> {
        if execution <= 0 {
            return Err(denied());
        }
        SchemaTypeRepository::lock_selection(transaction).await?;
        let row = ExecutionRepository::find_by_id(&mut **transaction, execution)
            .await?
            .ok_or_else(denied)?;
        let secrets = ExecutionSecretValueRepository::find_bounded_stored_by_entity(
            transaction,
            ENTITY_EXECUTION_CONFIG,
            execution,
        )
        .await?;
        let snapshot = ExecutionInputSnapshot {
            execution,
            executor: row.executor,
            config: row.config.unwrap_or_else(|| serde_json::json!({})),
            secrets,
        };
        if !snapshot.config.is_object() || expected.is_some_and(|value| value != &snapshot) {
            return Err(denied());
        }
        let (ordinary, receipts) = if snapshot.secrets.is_empty() {
            (Vec::new(), Vec::new())
        } else {
            immutable_input_origin::prepare_input_receipts(
                transaction,
                ENTITY_EXECUTION_CONFIG,
                execution,
                &snapshot.secrets,
                encryption_key.ok_or_else(denied)?,
            )
            .await?
        };
        // This also validates the recorded input contributor/liveness contract
        // when the value happens to have no protected fields.
        let origin_fence = crate::key_access::authorize_execution_parameter_origins_with_evidence(
            transaction,
            execution,
            row.executor,
            &ordinary,
        )
        .await?;
        // Contributor sources precede the destination lock. Recheck the flat
        // config selected before those waits, then retain every destination
        // ciphertext/origin row through the later operation effect.
        let selected: Option<(Option<Value>, Option<i64>)> =
            sqlx::query_as("SELECT config,executor FROM execution WHERE id=$1 FOR SHARE")
                .bind(execution)
                .fetch_optional(&mut **transaction)
                .await?;
        let (config, executor) = selected.ok_or_else(denied)?;
        if config.unwrap_or_else(|| serde_json::json!({})) != snapshot.config
            || executor != snapshot.executor
        {
            return Err(denied());
        }
        ExecutionSecretValueRepository::bind_bounded_snapshot(
            transaction,
            ENTITY_EXECUTION_CONFIG,
            execution,
            &snapshot.secrets,
        )
        .await?;
        immutable_input_origin::bind_input_receipt_buffers(
            transaction,
            ENTITY_EXECUTION_CONFIG,
            execution,
            &snapshot.secrets,
        )
        .await?;
        let value = if snapshot.secrets.is_empty() {
            snapshot.config.clone()
        } else {
            restore_secret_values(
                snapshot.config.clone(),
                &snapshot.secrets,
                encryption_key.ok_or_else(denied)?,
            )
            .map_err(|_| denied())?
        };
        if serde_json::to_vec(&value).map_err(|_| denied())?.len()
            > crate::workload_stdin::MAX_PRIVATE_STDIN_BYTES
        {
            return Err(denied());
        }
        let mut input = AuthorizedExecutionInput {
            transaction,
            value,
            snapshot,
            receipts,
            origin_fence,
        };
        input.ensure_fresh().await?;
        Ok(input)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn snapshots_bind_origins_and_ciphertext_without_exposing_them() {
        let original = ExecutionInputSnapshot {
            execution: 1,
            executor: Some(2),
            config: serde_json::json!({"private": "unserialized-sensitive-config"}),
            secrets: vec![StoredSecretValue {
                json_path: "/private".into(),
                encrypted_value: Value::String("private-ciphertext".into()),
                encryption_key_hash: Some("private-key-hash".into()),
                source_kind: "provenance".into(),
                source_ref: Some("private-origin".into()),
            }],
        };
        let mut changed = original.clone();
        changed.secrets[0].source_ref = Some("replacement-origin".into());
        assert_ne!(original, changed);
        changed = original.clone();
        changed.secrets[0].encrypted_value = Value::String("replacement-ciphertext".into());
        assert_ne!(original, changed);
        changed = original.clone();
        changed.executor = Some(3);
        assert_ne!(original, changed);
        let debug = format!("{original:?}");
        for private in [
            "unserialized-sensitive-config",
            "private-ciphertext",
            "private-key-hash",
            "private-origin",
            "/private",
        ] {
            assert!(!debug.contains(private));
        }
        let selected = SelectedExecutionInput::new(
            serde_json::json!({"value":"selected-private-input"}),
            original.clone(),
        )
        .unwrap();
        assert!(!format!("{selected:?}").contains("selected-private-input"));
        assert!(SelectedExecutionInput::new(serde_json::json!([]), original).is_err());
    }
}
