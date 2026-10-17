//! Native input-to-output closure retains registry references, never flattened
//! parent grants or mutable replacements for the registry's captured parents.
use super::{
    execution_runtime_admission::ExecutionAdmissionRepository,
    execution_secret_value::ExecutionSecretValueRepository,
    recorded_input_origins::RecordedInputOriginRepository,
    runtime_authority::{ApprovalEffectFence, RuntimeAuthorityRepository, RuntimeOwner},
    runtime_read::{RuntimeReadRepository, ENTITY_RUNTIME_READ},
};
use crate::{
    models::Id,
    secret_provenance::{SecretOrigin, SecretProvenance},
    Error, Result,
};
use sqlx::PgConnection;
use std::collections::{BTreeMap, BTreeSet};

pub struct NativeInputClosure {
    origins: Vec<SecretOrigin>,
    effect_fence: ApprovalEffectFence,
    contributors: Vec<crate::delegation::DelegationAuthority>,
    receipts: Vec<super::inherited_input::ReceiptContext>,
    buffers: BTreeMap<(String, Id), Vec<crate::secret_values::StoredSecretValue>>,
}

impl NativeInputClosure {
    pub fn effect_fence(&self) -> &ApprovalEffectFence {
        &self.effect_fence
    }
    pub fn origins(&self) -> &[SecretOrigin] {
        &self.origins
    }
    pub fn ensure_fresh(&self) -> Result<()> {
        self.effect_fence.ensure_fresh()?;
        for receipt in &self.receipts {
            receipt.ensure_fresh()?;
        }
        Ok(())
    }
    pub async fn ensure_fresh_on(&self, conn: &mut PgConnection) -> Result<()> {
        self.effect_fence.ensure_fresh_on(conn).await?;
        for receipt in &self.receipts {
            receipt.ensure_fresh_on(conn).await?;
        }
        self.ensure_fresh()
    }

    /// Consume the exact source buffers prepared before capture/target locks.
    /// This phase never discovers a new receipt or reacquires source authority.
    pub async fn restore_prepared(
        &self,
        conn: &mut PgConnection,
        kind: &str,
        execution: Id,
        value: serde_json::Value,
        encryption_key: Option<&str>,
    ) -> Result<serde_json::Value> {
        let empty = Vec::new();
        let prepared = self
            .buffers
            .get(&(kind.to_owned(), execution))
            .unwrap_or(&empty);
        let actual =
            ExecutionSecretValueRepository::find_stored_by_entity(&mut *conn, kind, execution)
                .await?;
        if &actual != prepared {
            return Err(Error::PermissionDenied(
                "Native input buffer changed after source preparation".into(),
            ));
        }
        super::immutable_input_origin::bind_input_receipt_buffers(conn, kind, execution, prepared)
            .await?;
        let restored = if prepared.is_empty() {
            value
        } else {
            crate::secret_values::restore_secret_values(
                value,
                prepared,
                encryption_key.ok_or_else(|| {
                    Error::invalid_state("Native input decryption is unavailable")
                })?,
            )?
        };
        self.ensure_fresh_on(conn).await?;
        Ok(restored)
    }
    pub async fn authorize_delivery(
        &self,
        conn: &mut PgConnection,
        values: &[crate::secret_values::SecretValueInput],
    ) -> Result<()> {
        let mut origins = Vec::new();
        if values.len() > 256 {
            return Err(Error::validation("Native delivery exceeds its field bound"));
        }
        for value in values {
            if value.source_kind != "provenance" {
                continue;
            }
            let provenance: SecretProvenance =
                serde_json::from_str(value.source_ref.as_deref().ok_or_else(|| {
                    Error::PermissionDenied("Native delivery provenance unavailable".into())
                })?)?;
            origins.extend(provenance.origins.into_iter().filter(|origin|
                matches!(origin, SecretOrigin::Entity { entity_type, .. } if entity_type == ENTITY_RUNTIME_READ)));
        }
        RecordedInputOriginRepository::require_runtime_reads(conn, &self.contributors, &origins)
            .await?;
        self.ensure_fresh_on(conn).await
    }
}

pub struct NativeInputOriginRepository;
impl NativeInputOriginRepository {
    pub async fn capture_workflow_with_key(
        conn: &mut PgConnection,
        consumer: Id,
        encryption_key: Option<&str>,
    ) -> Result<NativeInputClosure> {
        let sources: Vec<Id> = sqlx::query_scalar("SELECT id FROM execution WHERE parent = $1 AND result IS NOT NULL AND status IN ('completed','failed','timeout','cancelled','abandoned') ORDER BY id LIMIT 257")
            .bind(consumer).fetch_all(&mut *conn).await?;
        Self::capture_with_key(conn, consumer, &sources, encryption_key).await
    }

    /// Receipt source preparation and ordinary all-contributor proof precede
    /// the consumer mutation/read-capture fences acquired by `capture`.
    pub async fn capture_with_key(
        conn: &mut PgConnection,
        consumer: Id,
        sources: &[Id],
        encryption_key: Option<&str>,
    ) -> Result<NativeInputClosure> {
        ExecutionAdmissionRepository::require(conn, RuntimeOwner::Execution(consumer)).await?;
        let ids: BTreeSet<_> = sources.iter().copied().chain([consumer]).collect();
        if ids.len() > 256 {
            return Err(Error::validation(
                "Native input closure exceeds its source bound",
            ));
        }
        for source in &ids {
            if !super::execution::ExecutionRepository::is_in_execution_tree(
                &mut *conn, consumer, *source, true,
            )
            .await?
            {
                return Err(Error::PermissionDenied(
                    "Native source is outside its actual workflow tree".into(),
                ));
            }
        }
        let kinds = [
            crate::secret_values::ENTITY_EXECUTION_CONFIG,
            crate::secret_values::ENTITY_EXECUTION_RESULT,
            crate::secret_values::ENTITY_WORKFLOW_VARIABLES,
        ];
        let ids: Vec<_> = ids.into_iter().collect();
        let rows =
            ExecutionSecretValueRepository::find_bounded_provenance(&mut *conn, &kinds, &ids)
                .await?;
        if rows.len() > 256 || rows.iter().any(|row| row.source_ref.is_none()) {
            return Err(Error::validation(
                "Native input provenance exceeds its source bound",
            ));
        }
        let groups: BTreeSet<_> = rows
            .iter()
            .map(|row| (row.entity_type.clone(), row.entity_id))
            .collect();
        let mut buffers = BTreeMap::new();
        let mut receipts = Vec::new();
        let mut count = 0usize;
        for (kind, execution) in groups {
            let secrets =
                ExecutionSecretValueRepository::find_stored_by_entity(&mut *conn, &kind, execution)
                    .await?;
            count = count
                .checked_add(secrets.len())
                .ok_or_else(|| Error::validation("Native input buffer bound overflow"))?;
            if count > 256 {
                return Err(Error::validation(
                    "Native input buffers exceed their source bound",
                ));
            }
            let key = encryption_key.ok_or_else(|| {
                Error::invalid_state("Native input receipt decryption is unavailable")
            })?;
            let (ordinary, contexts) = super::immutable_input_origin::prepare_input_receipts(
                conn, &kind, execution, &secrets, key,
            )
            .await?;
            // Purpose-specific receipts are removed only from their exact source
            // field. Every remaining contributor origin still needs its proof.
            crate::key_access::authorize_execution_parameter_key_origins_with_conn(
                conn, consumer, None, &ordinary,
            )
            .await?;
            receipts.extend(contexts);
            if receipts.len() > 256 {
                return Err(Error::validation(
                    "Native input receipt closure exceeds its source bound",
                ));
            }
            buffers.insert((kind, execution), secrets);
        }
        // Empty parameters still validate ordinary accepted input/admission.
        crate::key_access::authorize_execution_parameter_key_origins_with_conn(
            conn,
            consumer,
            None,
            &[],
        )
        .await?;
        let mut closure = Self::capture(conn, consumer, sources).await?;
        closure.receipts = receipts;
        closure.buffers = buffers;
        closure.ensure_fresh_on(conn).await?;
        Ok(closure)
    }

    pub async fn capture_workflow(
        conn: &mut PgConnection,
        consumer: Id,
    ) -> Result<NativeInputClosure> {
        let sources: Vec<Id> = sqlx::query_scalar("SELECT id FROM execution WHERE parent = $1 AND result IS NOT NULL AND status IN ('completed','failed','timeout','cancelled','abandoned') ORDER BY id LIMIT 257")
            .bind(consumer).fetch_all(&mut *conn).await?;
        Self::capture(conn, consumer, &sources).await
    }
    /// Call before restoring sources and retain the transaction through output
    /// persistence. Every source is explicit; no requester/source partitioning.
    pub async fn capture(
        conn: &mut PgConnection,
        consumer: Id,
        sources: &[Id],
    ) -> Result<NativeInputClosure> {
        let admission =
            ExecutionAdmissionRepository::require(conn, RuntimeOwner::Execution(consumer)).await?;
        let mut effect_fence =
            RuntimeAuthorityRepository::retained_admission_fence(conn, admission.id).await?;
        let contributors =
            ExecutionAdmissionRepository::input_authorities(conn, consumer, None).await?;
        let sources: BTreeSet<_> = sources.iter().copied().chain([consumer]).collect();
        if sources.len() > 256 {
            return Err(Error::validation(
                "Native input closure exceeds its source bound",
            ));
        }
        let mut admissions = BTreeSet::new();
        // Validate all sources before capture/target locks. Historical terminal
        // sources remain usable; no current human IAM or fresh API selection.
        for source in &sources {
            if let Some(record) =
                ExecutionAdmissionRepository::find(conn, RuntimeOwner::Execution(*source)).await?
            {
                effect_fence = effect_fence.intersect(
                    &RuntimeAuthorityRepository::retained_admission_fence(conn, record.id).await?,
                );
                admissions.insert(record.id);
            }
        }
        // Serialize this native producer before taking read-capture fences. An
        // SDK reader locks its source execution before its capture fence; taking
        // the consumer mutation lock here prevents an inverse upgrade at final
        // output publication while such a reader waits on our capture fence.
        super::execution::ExecutionRepository::find_by_id_for_update(conn, consumer)
            .await?
            .ok_or_else(|| Error::PermissionDenied("Native consumer unavailable".into()))?;
        for admission in admissions {
            sqlx::query("INSERT INTO execution_runtime_read_fence(admission) VALUES ($1) ON CONFLICT DO NOTHING")
                .bind(admission).execute(&mut *conn).await?;
            sqlx::query_scalar::<_, bool>(
                "SELECT closed FROM execution_runtime_read_fence WHERE admission = $1 FOR SHARE",
            )
            .bind(admission)
            .fetch_one(&mut *conn)
            .await?;
        }
        let sources: Vec<_> = sources.into_iter().collect();
        let mut origins = RuntimeReadRepository::references_for_executions(&mut *conn, &sources)
            .await?
            .into_values()
            .flatten()
            .collect::<Vec<_>>();
        let rows = ExecutionSecretValueRepository::find_bounded_provenance(
            &mut *conn,
            &[
                crate::secret_values::ENTITY_EXECUTION_CONFIG,
                crate::secret_values::ENTITY_EXECUTION_RESULT,
                crate::secret_values::ENTITY_WORKFLOW_VARIABLES,
            ],
            &sources,
        )
        .await?;
        if rows.len() > 256 {
            return Err(Error::validation(
                "Native input provenance exceeds its row bound",
            ));
        }
        for row in rows {
            if row.source_kind != "provenance" {
                continue;
            }
            let provenance: SecretProvenance =
                serde_json::from_str(row.source_ref.as_deref().ok_or_else(|| {
                    Error::PermissionDenied("Native input provenance is unavailable".into())
                })?)?;
            origins.extend(provenance.origins.into_iter().filter(|origin|
                matches!(origin, SecretOrigin::Entity { entity_type, .. } if entity_type == ENTITY_RUNTIME_READ)));
        }
        origins.sort_by_key(|origin| match origin {
            SecretOrigin::Entity { entity_id, .. } => *entity_id,
            _ => 0,
        });
        origins.dedup();
        RecordedInputOriginRepository::require_runtime_reads(conn, &contributors, &origins).await?;
        effect_fence.ensure_fresh_on(conn).await?;
        Ok(NativeInputClosure {
            origins,
            effect_fence,
            contributors,
            receipts: Vec::new(),
            buffers: BTreeMap::new(),
        })
    }
}
