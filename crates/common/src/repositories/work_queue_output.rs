//! Protected copies of action acknowledgement details into queue item state.
//! Controls are consumed internally; public metadata retains selective markers.

use serde_json::Value;
use sqlx::PgConnection;

use super::execution_secret_value::ExecutionSecretValueRepository;
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

impl WorkQueueOutputRepository {
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
