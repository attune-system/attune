//! Protected copies of action acknowledgement details into queue item state.
//! Controls are consumed internally; public metadata retains selective markers.

use serde_json::Value;
use sqlx::PgConnection;

use super::{
    delegated_execution::ProtectedQueueAck,
    execution_secret_value::{ExecutionSecretValueRepository, SecretProvenanceRow},
    immutable_input_origin::InputReceiptFrontier,
};
use crate::secret_provenance::{SecretOrigin, SecretProvenance};
use crate::secret_values::{
    prepare_secret_values, redact_secret_path_sources, PreparedSecretValue, SecretPathSource,
    SecretSource, ENTITY_EXECUTION_RESULT,
};
use crate::{
    models::{Id, WorkQueueItemStatus},
    Error, Result,
};

pub const ENTITY_QUEUE_ITEM_ACK_SUMMARY: &str = "queue_item_ack_summary";
pub const ENTITY_QUEUE_ITEM_LAST_ERROR: &str = "queue_item_last_error";
/// The exact terminal metadata value consumed by native queue waits.
pub const ENTITY_QUEUE_ITEM_RESULT: &str = "queue_item_result";

pub struct WorkQueueOutputRepository;

pub struct PreparedQueueItemOutput {
    pub summary: Value,
    pub error: Option<Value>,
    summary_fields: Vec<PreparedSecretValue>,
    error_fields: Vec<PreparedSecretValue>,
    status: WorkQueueItemStatus,
}

pub struct AcceptedQueueCompletion {
    ack_version: i32,
    retry_limit: u32,
    execution: Id,
    items: Vec<Id>,
    transaction: String,
    fence: super::runtime_authority::ApprovalEffectFence,
}

impl AcceptedQueueCompletion {
    pub fn ack_version(&self) -> i32 {
        self.ack_version
    }

    pub fn retry_limit(&self) -> u32 {
        self.retry_limit
    }

    pub async fn ensure_fresh_on(&self, connection: &mut PgConnection) -> Result<()> {
        let transaction: String = sqlx::query_scalar("SELECT pg_current_xact_id()::TEXT")
            .fetch_one(&mut *connection)
            .await?;
        if transaction != self.transaction {
            return Err(Error::PermissionDenied(
                "Queue completion transaction changed".into(),
            ));
        }
        self.fence.ensure_fresh_on(connection).await
    }
}

impl WorkQueueOutputRepository {
    /// Read only the accepted dispatch configuration. Completion never needs
    /// to reopen the item's sealed input values or reselect a current Key.
    pub async fn accepted_completion_config(
        conn: &mut PgConnection,
        queue: &crate::models::WorkQueue,
        execution: &crate::models::Execution,
        items: &[crate::models::WorkQueueItem],
    ) -> Result<AcceptedQueueCompletion> {
        use super::{
            execution_admission::ExecutionAdmissionRepository as Admission,
            runtime_authority::{RuntimeAuthorityRepository as Authority, RuntimeOwner},
            work_queue::WorkQueueItemRepository,
        };
        use crate::runtime_authority::{InvocationBinding, RequestAttribution};

        let denied =
            || Error::PermissionDenied("Accepted queue completion binding unavailable".into());
        // Serialize with delivery/reclamation using canonical item ID order.
        // The caller's read model alone cannot supply a completion lease.
        let mut locked = items.iter().collect::<Vec<_>>();
        locked.sort_unstable_by_key(|item| item.id);
        for expected in locked {
            let current = WorkQueueItemRepository::find_by_id_for_update(conn, expected.id)
                .await?
                .ok_or_else(denied)?;
            if current.queue != queue.id
                || current.status != WorkQueueItemStatus::Leased
                || current.leased_execution != Some(execution.id)
                || current.lease_token.is_none()
                || current.lease_token != expected.lease_token
                || current.attempt_count != expected.attempt_count
            {
                return Err(denied());
            }
        }
        let (record, fence) =
            Admission::require_with_fence(conn, RuntimeOwner::Execution(execution.id)).await?;
        let authority = &record.authority;
        let InvocationBinding::Queue {
            queue_ref,
            action_ref,
        } = &authority.binding
        else {
            return Err(denied());
        };
        let RequestAttribution::QueueBatch {
            queue_ref: attribution,
            items: accepted,
        } = &authority.attribution
        else {
            return Err(denied());
        };
        let actual: std::collections::BTreeSet<_> = items.iter().map(|item| item.id).collect();
        if queue.is_adhoc
            || queue_ref != &queue.r#ref
            || attribution != queue_ref
            || action_ref != &execution.action_ref
            || queue.pack != Some(authority.owner.pack_id)
            || items.is_empty()
            || actual.len() != items.len()
            || actual.iter().ne(accepted.keys())
            || items.iter().any(|item| {
                item.queue != queue.id
                    || item.queue_ref != *queue_ref
                    || item.leased_execution != Some(execution.id)
                    || item.lease_token.is_none()
            })
        {
            return Err(denied());
        }
        let policy = Authority::require_policy(conn, record.policy_revision).await?;
        let invocation = policy
            .policy
            .invocations
            .iter()
            .find(|candidate| {
                candidate.binding == authority.binding && candidate.source_id == Some(queue.id)
            })
            .ok_or_else(denied)?;
        let metadata: Option<(Value, String)> = sqlx::query_as(
            "SELECT metadata, metadata_digest FROM managed_dispatch_metadata_revision \
             WHERE policy_revision=$1 AND component_kind='queues' AND component_id=$2 FOR SHARE",
        )
        .bind(policy.id)
        .bind(queue.id)
        .fetch_optional(&mut *conn)
        .await?;
        let (metadata, digest) = metadata.ok_or_else(denied)?;
        if invocation.dispatch_template_digest.as_deref() != Some(digest.as_str())
            || crate::value_schema::digest(&metadata).map_err(|_| denied())? != digest
        {
            return Err(denied());
        }
        let config: crate::models::WorkQueueConfig =
            serde_json::from_value(metadata.get("config").cloned().ok_or_else(denied)?)?;
        let ack_version = config.ack_contract.map_or(1, |ack| ack.version);
        if ack_version < 1 {
            return Err(denied());
        }
        let transaction = sqlx::query_scalar("SELECT pg_current_xact_id()::TEXT")
            .fetch_one(&mut *conn)
            .await?;
        fence.ensure_fresh_on(conn).await?;
        Ok(AcceptedQueueCompletion {
            ack_version,
            retry_limit: config
                .dispatch
                .and_then(|dispatch| dispatch.retry_limit)
                .unwrap_or_default(),
            execution: execution.id,
            items: actual.into_iter().collect(),
            transaction,
            fence,
        })
    }

    /// Both successful acknowledgements and failure transitions must hold the
    /// closed producer, including failures that do not inspect its output.
    pub async fn require_closed_recorded_producer(
        conn: &mut PgConnection,
        accepted: &AcceptedQueueCompletion,
    ) -> Result<()> {
        use super::{
            execution_admission::ExecutionAdmissionRepository as Admission,
            runtime_authority::RuntimeOwner, ExecutionRepository, FindById,
        };
        let denied = || {
            Error::PermissionDenied("Recorded queue acknowledgement producer is unavailable".into())
        };
        accepted.ensure_fresh_on(conn).await?;
        let execution = accepted.execution;
        let admission = Admission::require(conn, RuntimeOwner::Execution(execution)).await?;
        let snapshot = ExecutionRepository::find_by_id(&mut *conn, execution)
            .await?
            .and_then(|row| row.executable_snapshot)
            .ok_or_else(denied)?
            .executable;
        let closed: Option<Id> = match (snapshot.action.workflow_def, snapshot.workflow_definition) {
            (None, None) => sqlx::query_scalar(
                "SELECT e.id FROM execution e JOIN worker_delivery_context c ON c.execution=e.id \
                 JOIN runtime_authority_execution_fence f ON f.execution=e.id AND f.admission=c.admission \
                 JOIN execution_runtime_read_fence r ON r.admission=f.admission \
                 WHERE e.id=$1 AND c.admission=$2 AND e.worker=c.worker AND c.attempt=f.attempt AND r.closed \
                 AND e.status IN ('completed','failed','cancelled','timeout','abandoned') \
                 FOR SHARE OF e,c,f,r",
            ).bind(execution).bind(admission.id).fetch_optional(&mut *conn).await?,
            (Some(definition), Some(snapshot)) if snapshot.id == definition => sqlx::query_scalar(
                "SELECT e.id FROM execution e JOIN workflow_execution w ON w.execution=e.id \
                 JOIN execution_runtime_read_fence r ON r.admission=$2 \
                 WHERE e.id=$1 AND w.workflow_def=$3 AND e.worker IS NULL AND r.closed \
                 AND e.status=w.status AND e.status IN ('completed','failed','cancelled','timeout','abandoned') \
                 AND NOT EXISTS(SELECT 1 FROM worker_delivery_context c WHERE c.execution=e.id) \
                 FOR SHARE OF e,w,r",
            ).bind(execution).bind(admission.id).bind(definition).fetch_optional(&mut *conn).await?,
            _ => return Err(denied()),
        };
        if closed.is_none() {
            return Err(denied());
        }
        Ok(())
    }

    /// Consume the recorded worker's closed result on the canonical queue
    /// control path. No requester IAM or delegated output credential is used.
    pub async fn recorded_ack_controls(
        conn: &mut PgConnection,
        accepted: &AcceptedQueueCompletion,
        key: Option<&str>,
    ) -> Result<(ProtectedQueueAck, InputReceiptFrontier)> {
        use super::{ExecutionRepository, FindById};
        let denied = || {
            Error::PermissionDenied("Recorded queue acknowledgement source is unavailable".into())
        };
        Self::require_closed_recorded_producer(conn, accepted).await?;
        let execution = accepted.execution;
        let mut origins = InputReceiptFrontier::default();
        let secrets =
            ExecutionSecretValueRepository::find_queue_ack_projection(conn, execution).await?;
        let result = ExecutionRepository::find_by_id(&mut *conn, execution)
            .await?
            .and_then(|row| row.result);
        let Some(result) = result else {
            return Ok((ProtectedQueueAck::Invalid, origins));
        };
        if secrets.is_empty() {
            return Ok((
                ProtectedQueueAck::validate(&result, &accepted.items, accepted.ack_version),
                origins,
            ));
        }
        let provenance = secrets
            .iter()
            .map(|secret| SecretProvenanceRow {
                entity_type: ENTITY_EXECUTION_RESULT.into(),
                entity_id: execution,
                json_path: secret.json_path.clone(),
                source_kind: secret.source_kind.clone(),
                source_ref: secret.source_ref.clone(),
            })
            .collect::<Vec<_>>();
        origins.prepare(conn, &provenance, key).await?;
        let value = if secrets.iter().any(|secret| secret.json_path.is_empty()) {
            result
        } else {
            serde_json::json!({"queue_ack": result.get("queue_ack").ok_or_else(denied)?})
        };
        let value =
            crate::secret_values::restore_secret_values(value, &secrets, key.ok_or_else(denied)?)?;
        let controls = ProtectedQueueAck::validate(&value, &accepted.items, accepted.ack_version);
        origins.ensure_fresh_on(conn).await?;
        Ok((controls, origins))
    }

    /// Caller holds the queue item's completion lease lock through commit.
    pub async fn prepare_item_metadata(
        conn: &mut PgConnection,
        execution: Id,
        index: usize,
        status: WorkQueueItemStatus,
        summary: Value,
        error: Option<Value>,
        key: Option<&str>,
    ) -> Result<PreparedQueueItemOutput> {
        let source_fields = ExecutionSecretValueRepository::find_stored_by_entity(
            &mut *conn,
            ENTITY_EXECUTION_RESULT,
            execution,
        )
        .await?;
        let base = format!("/queue_ack/items/{index}");
        let mut summary_sources = Vec::new();
        let mut error_sources = Vec::new();
        for field in ["summary", "error"] {
            let source = format!("{base}/{field}");
            for secret in &source_fields {
                if let Some(suffix) = protected_suffix(&secret.json_path, &source) {
                    let origin = SecretSource::Bound(SecretProvenance {
                        origins: vec![SecretOrigin::Entity {
                            entity_type: ENTITY_EXECUTION_RESULT.into(),
                            entity_id: execution,
                            path: secret.json_path.clone(),
                        }],
                        templates: Vec::new(),
                    });
                    summary_sources.push(SecretPathSource {
                        path: format!("/{field}{suffix}"),
                        source: origin.clone(),
                    });
                    if field == "error" {
                        if status == WorkQueueItemStatus::Failed
                            && summary.get("status").and_then(Value::as_str) == Some("retry")
                        {
                            error_sources.push(SecretPathSource {
                                path: format!("/retry_error{suffix}"),
                                source: origin,
                            });
                        } else {
                            error_sources.push(SecretPathSource {
                                path: suffix,
                                source: origin,
                            });
                        }
                    }
                }
            }
        }
        let (summary, summary_inputs) = redact_secret_path_sources(summary, &summary_sources);
        let (error, error_inputs) = if let Some(error) = error {
            let (value, inputs) = redact_secret_path_sources(error, &error_sources);
            (Some(value), inputs)
        } else {
            (None, Vec::new())
        };
        let seal = |inputs: Vec<crate::secret_values::SecretValueInput>| -> Result<_> {
            if inputs.is_empty() {
                Ok(Vec::new())
            } else {
                prepare_secret_values(
                    inputs,
                    key.ok_or_else(|| {
                        Error::validation("Queue acknowledgement encryption is unavailable")
                    })?,
                )
            }
        };
        let summary_fields = seal(summary_inputs)?;
        let error_fields = seal(error_inputs)?;
        Ok(PreparedQueueItemOutput {
            summary,
            error,
            summary_fields,
            error_fields,
            status,
        })
    }

    /// Persist after updating the public queue row, under the same held lease
    /// lock and transaction. Its mutation trigger removes superseded copies.
    pub async fn persist_item_metadata(
        conn: &mut PgConnection,
        item: Id,
        output: &PreparedQueueItemOutput,
    ) -> Result<()> {
        ExecutionSecretValueRepository::replace_entity_with_conn(
            conn,
            ENTITY_QUEUE_ITEM_ACK_SUMMARY,
            item,
            &output.summary_fields,
        )
        .await?;
        ExecutionSecretValueRepository::replace_entity_with_conn(
            conn,
            ENTITY_QUEUE_ITEM_LAST_ERROR,
            item,
            &output.error_fields,
        )
        .await?;
        let terminal_fields = if matches!(
            output.status,
            WorkQueueItemStatus::Completed | WorkQueueItemStatus::Skipped
        ) {
            &output.summary_fields
        } else {
            &output.error_fields
        };
        ExecutionSecretValueRepository::replace_entity_with_conn(
            conn,
            ENTITY_QUEUE_ITEM_RESULT,
            item,
            terminal_fields,
        )
        .await
    }
}

fn protected_suffix(secret: &str, value: &str) -> Option<String> {
    if secret.is_empty()
        || secret == value
        || value
            .strip_prefix(secret)
            .is_some_and(|suffix| suffix.starts_with('/'))
    {
        Some(String::new())
    } else {
        secret
            .strip_prefix(value)
            .filter(|suffix| suffix.starts_with('/'))
            .map(str::to_owned)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn maps_secret_parent_or_descendant_without_protecting_siblings() {
        assert_eq!(
            protected_suffix("/queue_ack", "/queue_ack/items/0/summary"),
            Some(String::new())
        );
        assert_eq!(
            protected_suffix(
                "/queue_ack/items/0/summary/token",
                "/queue_ack/items/0/summary"
            ),
            Some("/token".into())
        );
        assert_eq!(
            protected_suffix("/queue_ack/items/0/error", "/queue_ack/items/0/summary"),
            None
        );
        assert_eq!(
            protected_suffix("/queue_ack/items/1/summary", "/queue_ack/items/0/summary"),
            None
        );
    }
}
