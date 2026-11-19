//! Selected execution input is restored only after its recorded origins have
//! been admitted. The borrowed transaction retains those source locks through
//! later operation waits; receipt deadlines must be rechecked at the effect.
mod frontier;
pub(crate) use frontier::{origin_epoch, retain_dependency, FrozenInputFrontier};

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
    pub(crate) fn secret_paths(&self) -> Vec<String> {
        self.secrets
            .iter()
            .map(|secret| secret.json_path.clone())
            .collect()
    }

    /// Exact private evidence only, never disclosure or launch authority. The
    /// native consumer must restore this snapshot through the canonical issuer
    /// and compare its retained frontier before admitting an effect.
    pub(crate) fn retained_snapshot(&self) -> ExecutionInputSnapshot {
        ExecutionInputSnapshot {
            execution: self.execution,
            executor: self.executor,
            config: self.config.clone(),
            secrets: self
                .secrets
                .iter()
                .map(|secret| StoredSecretValue {
                    json_path: secret.json_path.clone(),
                    encrypted_value: secret.encrypted_value.clone(),
                    encryption_key_hash: secret.encryption_key_hash.clone(),
                    source_kind: secret.source_kind.clone(),
                    source_ref: secret.source_ref.clone(),
                })
                .collect(),
        }
    }

    /// The separate workload-launch consumer remains public-only until its own
    /// complete origin-frontier migration is implemented and validated.
    pub(crate) fn public_snapshot(&self) -> Result<ExecutionInputSnapshot> {
        if !self.secrets.is_empty() {
            return Err(denied());
        }
        Ok(self.retained_snapshot())
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
    pub(crate) fn frozen_frontier(&self) -> Result<FrozenInputFrontier> {
        self.ensure_fresh()?;
        let mut fence = self.origin_fence.clone();
        for receipt in &self.receipts {
            fence = fence.intersect(receipt.input_effect_fence());
        }
        let frontier = FrozenInputFrontier::from_held(&fence);
        frontier.ensure_complete()?;
        Ok(frontier)
    }
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
    pub(crate) async fn retain_provenance_buffer(
        connection: &mut sqlx::PgConnection,
        expected: &super::execution_secret_value::SecretProvenanceRow,
    ) -> Result<super::runtime_authority::InputAuthorityDependency> {
        let id:Option<i64> = sqlx::query_scalar(
            "SELECT id FROM execution_secret_value WHERE entity_type=$1 AND entity_id=$2 AND json_path=$3 \
             AND source_kind=$4 AND source_ref IS NOT DISTINCT FROM $5 FOR SHARE"
        ).bind(&expected.entity_type).bind(expected.entity_id).bind(&expected.json_path)
            .bind(&expected.source_kind).bind(&expected.source_ref).fetch_optional(connection).await?;
        Ok(
            super::runtime_authority::InputAuthorityDependency::ProvenanceBuffer(
                id.ok_or_else(denied)?,
            ),
        )
    }

    pub(crate) async fn retain_origin_buffer(
        connection: &mut sqlx::PgConnection,
        kind: &str,
        entity: i64,
        expected: &StoredSecretValue,
    ) -> Result<super::runtime_authority::InputAuthorityDependency> {
        let id: Option<i64> = sqlx::query_scalar(
            "SELECT id FROM execution_secret_value WHERE entity_type=$1 AND entity_id=$2 AND json_path=$3 \
             AND source_kind=$4 AND source_ref IS NOT DISTINCT FROM $5 AND encrypted_value=$6 \
             AND encryption_key_hash IS NOT DISTINCT FROM $7 FOR SHARE"
        ).bind(kind).bind(entity).bind(&expected.json_path).bind(&expected.source_kind)
            .bind(&expected.source_ref).bind(&expected.encrypted_value).bind(&expected.encryption_key_hash)
            .fetch_optional(connection).await?;
        Ok(
            super::runtime_authority::InputAuthorityDependency::SecretBuffer(
                id.ok_or_else(denied)?,
            ),
        )
    }

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
    fn input_evidence_frozen_secret_paths_deny_cli_binding() {
        use crate::{
            action_cli::{ArgumentLimits, ArgumentPlan, InputDelivery},
            value_schema::{Limits, RegistryView, SourceIdentity},
        };
        let input = FrozenInputSnapshot::from(&ExecutionInputSnapshot {
            execution: 1,
            executor: None,
            config: serde_json::json!({"token":"redacted"}),
            secrets: vec![StoredSecretValue {
                json_path: "/token".into(),
                encrypted_value: Value::String("ciphertext".into()),
                encryption_key_hash: Some("key-hash".into()),
                source_kind: "provenance".into(),
                source_ref: Some("retained-origin".into()),
            }],
        });
        let schema = serde_json::json!({"token":{"type":"string","cli":{"kind":"option","name":"--token","order":1}}});
        let plan = ArgumentPlan::compile(
            SourceIdentity::Released {
                component_ref: "owned.cli".into(),
                release_id: 1,
                release_digest: "a".repeat(64),
            },
            &schema,
            &[],
            &RegistryView::new(),
            Limits::default(),
        )
        .unwrap();
        let parameters = serde_json::json!({"token":"private-test-value"});
        // An undeclared binding remains protected by the actual frozen origin
        // paths, even when authoring metadata classifies the field as public.
        assert!(plan
            .materialize(
                "/bin/tool",
                &parameters,
                &[],
                InputDelivery::Stdin,
                ArgumentLimits::default()
            )
            .is_ok());
        assert!(plan
            .materialize(
                "/bin/tool",
                &parameters,
                &input.secret_paths(),
                InputDelivery::Stdin,
                ArgumentLimits::default()
            )
            .is_err());
        assert_eq!(input.secret_paths(), vec!["/token"]);
        assert!(input.public_snapshot().is_err());
    }

    #[test]
    fn input_evidence_frozen_projection_preserves_exact_private_snapshot() {
        let original = ExecutionInputSnapshot {
            execution: 17,
            executor: Some(29),
            config: serde_json::json!({"token": "redacted", "public": [true, 3]}),
            secrets: vec![
                StoredSecretValue {
                    json_path: "/token".into(),
                    encrypted_value: serde_json::json!({"ciphertext": "opaque-private-envelope"}),
                    encryption_key_hash: Some("private-key-hash".into()),
                    source_kind: "provenance".into(),
                    source_ref: Some("exact-retained-origin".into()),
                },
                StoredSecretValue {
                    json_path: "/nested/0".into(),
                    encrypted_value: Value::String("second-envelope".into()),
                    encryption_key_hash: None,
                    source_kind: "unattributed".into(),
                    source_ref: None,
                },
            ],
        };
        let stored = serde_json::to_vec(&FrozenInputSnapshot::from(&original)).unwrap();
        let frozen: FrozenInputSnapshot = serde_json::from_slice(&stored).unwrap();
        assert_eq!(frozen.retained_snapshot(), original);
        assert!(frozen.public_snapshot().is_err());
        let mut changed = frozen.clone();
        changed.secrets[0].encryption_key_hash = None;
        assert_ne!(changed.retained_snapshot(), original);
        changed = frozen.clone();
        changed.secrets[0].source_kind = "unattributed".into();
        assert_ne!(changed.retained_snapshot(), original);
        changed = frozen;
        changed.secrets[0].json_path = "/other".into();
        assert_ne!(changed.retained_snapshot(), original);
    }

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
