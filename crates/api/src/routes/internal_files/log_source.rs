//! Source owner for short prepare/final transactions.
use super::*;
use attune_common::repositories::{
    delegated_execution::{DelegatedExecutionRepository, LogProducerBinding},
    runtime_authority::RuntimeCredentialBinding,
    worker_delivery::WorkerDeliveryRepository,
};
use serde::{Deserialize, Serialize};

/// Historical data only. No credentials, deadlines, connection or mutation proof.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum Producer {
    Process {
        context: i64,
        binding: RuntimeCredentialBinding,
        worker: i64,
        instance: uuid::Uuid,
    },
    Delivery {
        context: i64,
        permit: i64,
        execution: i64,
        admission: i64,
        attempt: uuid::Uuid,
        worker: i64,
        instance: uuid::Uuid,
    },
    Delegated {
        binding: LogProducerBinding,
    },
    Sensor {
        sensor: i64,
        metadata_revision: Option<i64>,
        schema_binding: Option<(i64, String)>,
        log_generation: Option<(i64, String)>,
        fence: attune_common::models::SensorWorkloadFence,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum ProofBinding {
    Plain,
    Encrypted {
        authenticated_sha256: String,
        plaintext_cap: u64,
        ciphertext_cap: u64,
        manifest: attune_common::runtime_log_completion::FinalLogManifest,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HistoricalBinding {
    version: i64,
    stream: i64,
    producer: Producer,
    proof: ProofBinding,
}

impl HistoricalBinding {
    /// Server-derived projections for the durable selection repository, not grants.
    pub(crate) fn producer(&self) -> &Producer {
        &self.producer
    }
    pub(crate) fn proof(&self) -> &ProofBinding {
        &self.proof
    }
}

/// Must remain within this transaction, and be dropped before storage work.
/// HistoricalBinding is the only value allowed to cross a phase boundary.
pub struct Source<'db> {
    version: i64,
    artifact: i64,
    execution: Option<i64>,
    tx: sqlx::Transaction<'db, sqlx::Postgres>,
    producer: Producer,
    effect: crate::recorded_files::FileMutationProof,
}

fn denied() -> (StatusCode, String) {
    (
        StatusCode::FORBIDDEN,
        "Log source authority unavailable".into(),
    )
}

/// Call under shared graph admission BEFORE artifact/version/stream locks.
/// Acquiring capture is idempotent for an admitted caller; legacy paths enter too.
pub async fn acquire<'db>(
    state: &AppState,
    mut tx: sqlx::Transaction<'db, sqlx::Postgres>,
    user: &AuthenticatedUser,
    version: &attune_common::models::artifact_version::ArtifactVersion,
) -> Result<Source<'db>, (StatusCode, String)> {
    let result = acquire_binding(state, &mut tx, user, version).await;
    let (producer, effect) = match result {
        Ok(binding) => binding,
        Err(error) => {
            tx.rollback().await.map_err(map_sqlx_error)?;
            return Err(error);
        }
    };
    let mut source = Source {
        version: version.id,
        artifact: version.artifact,
        execution: version.execution,
        tx,
        producer,
        effect,
    };
    if let Err(error) = source.recheck().await {
        source.rollback().await?;
        return Err(error);
    }
    Ok(source)
}

async fn acquire_binding(
    state: &AppState,
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    user: &AuthenticatedUser,
    version: &attune_common::models::artifact_version::ArtifactVersion,
) -> Result<(Producer, crate::recorded_files::FileMutationProof), (StatusCode, String)> {
    attune_common::repositories::file_evidence_retention::FileEvidenceRetentionRepository::lock_capture(tx)
        .await.map_err(map_transport_error)?;
    let sensor = if user.claims.token_type == TokenType::Sensor {
        let reference = scoped_metadata_value(user, "sensor_ref", "Sensor")?;
        let row = SensorRepository::find_by_ref(&mut **tx, reference)
            .await
            .map_err(map_transport_error)?
            .ok_or_else(denied)?;
        let fence = user.sensor_workload_fence().map_err(|_| denied())?;
        let workload = SensorWorkloadRepository::lock_current_fence_workload(tx, row.id, fence)
            .await
            .map_err(map_transport_error)?
            .ok_or_else(denied)?;
        if workload.sensor != row.id
            || user
                .claims
                .sensor_pack_ref()
                .ok()
                .is_some_and(|pack| row.pack_ref.as_deref() != Some(pack))
        {
            return Err(denied());
        }
        let path = version.file_path.as_deref().ok_or_else(denied)?;
        if !ArtifactVersionRepository::file_path_owned_by_sensor(&mut **tx, path, reference)
            .await
            .map_err(map_transport_error)?
        {
            return Err(denied());
        }
        attune_common::repositories::sensor_schema::SensorSchemaRepository::lock_log_source(
            tx,
            fence.workload_id,
            fence.generation,
            version.id,
        )
        .await
        .map_err(map_transport_error)?;
        let metadata_revision = attune_common::repositories::managed_sensor_metadata::ManagedSensorMetadataRepository::revision_for_generation(
            tx, fence.workload_id, fence.generation).await.map_err(map_transport_error)?;
        let schema_binding =
            attune_common::repositories::sensor_schema::SensorSchemaRepository::find(
                &mut **tx,
                fence.workload_id,
                fence.generation,
            )
            .await
            .map_err(map_transport_error)?
            .map(|(id, binding, _)| binding.binding_digest().map(|digest| (id, digest)))
            .transpose()
            .map_err(map_transport_error)?;
        let log_generation = attune_common::repositories::sensor_schema::SensorSchemaRepository::find_log_generation(
            &mut **tx, version.id).await.map_err(map_transport_error)?;
        if log_generation
            .as_ref()
            .is_some_and(|log| Some(log) != schema_binding.as_ref())
        {
            return Err(denied());
        }
        Some(Producer::Sensor {
            sensor: row.id,
            metadata_revision,
            schema_binding,
            log_generation,
            fence,
        })
    } else {
        None
    };
    let delegated = if user.claims.token_type == TokenType::Worker
        && attune_common::worker_delivery::credential(&user.claims)
            .map_err(map_transport_error)?
            .is_none()
    {
        let execution = version.execution.ok_or_else(denied)?;
        let native = user.claims.sub == "0"
            && scoped_metadata_value(user, "worker_id", "Worker")? == "executor"
            && version
                .meta
                .as_ref()
                .and_then(|m| m.get("kind"))
                .and_then(|v| v.as_str())
                == Some("workflow_log");
        Some(Producer::Delegated {
            binding: DelegatedExecutionRepository::lock_log_producer(
                tx,
                execution,
                if native {
                    None
                } else {
                    Some(pack_candidate_worker_id(user)?)
                },
            )
            .await
            .map_err(map_transport_error)?,
        })
    } else {
        None
    };
    let delegated = if user.claims.token_type == TokenType::Execution
        && !crate::recorded_consumers::is_recorded(user).map_err(map_api_error)?
    {
        let (execution, _) = DelegatedExecutionRepository::validate_claims(tx, &user.claims)
            .await
            .map_err(map_transport_error)?
            .ok_or_else(denied)?;
        if version.execution != Some(execution) {
            return Err(denied());
        }
        let row = ExecutionRepository::find_by_id(&mut **tx, execution)
            .await
            .map_err(map_transport_error)?
            .ok_or_else(denied)?;
        Some(Producer::Delegated {
            binding: DelegatedExecutionRepository::lock_log_producer(
                tx,
                execution,
                Some(row.worker.ok_or_else(denied)?),
            )
            .await
            .map_err(map_transport_error)?,
        })
    } else {
        delegated
    };
    let effect = authorize_version_mutation_source(state, tx, user, version).await?;
    let producer = if let Some(authority) = &effect.process {
        {
            let context = attune_common::repositories::worker_delivery::WorkerDeliveryRepository::for_execution(tx, authority.credential_binding().execution_id)
                .await.map_err(map_transport_error)?.ok_or_else(denied)?;
            Producer::Process {
                context: WorkerDeliveryRepository::lock_log_context(tx, &context)
                    .await
                    .map_err(map_transport_error)?,
                binding: authority.credential_binding(),
                worker: context.worker,
                instance: context.worker_instance,
            }
        }
    } else if let Some((context, permit, _)) = &effect.worker {
        Producer::Delivery {
            context: WorkerDeliveryRepository::lock_log_context(tx, context)
                .await
                .map_err(map_transport_error)?,
            permit: WorkerDeliveryRepository::lock_log_permit(tx, permit)
                .await
                .map_err(map_transport_error)?,
            execution: context.execution,
            admission: context.admission,
            attempt: context.attempt,
            worker: context.worker,
            instance: context.worker_instance,
        }
    } else {
        sensor.or(delegated).ok_or_else(denied)?
    };
    Ok((producer, effect))
}

impl<'db> Source<'db> {
    /// Complete this short phase and repeat pure freshness after commit waits.
    pub async fn commit(mut self) -> Result<(), (StatusCode, String)> {
        if let Err(error) = self.recheck().await {
            self.rollback().await?;
            return Err(error);
        }
        // Unknown acknowledgement is distinct from acknowledged commit followed
        // by a pure freshness failure. Only the former may be reconciled.
        self.tx.commit().await.map_err(|_| {
            (
                StatusCode::SERVICE_UNAVAILABLE,
                "Log close commit acknowledgement uncertain; retry same input".into(),
            )
        })?;
        self.effect.ensure_fresh().map_err(map_api_error)
    }

    pub async fn rollback(self) -> Result<(), (StatusCode, String)> {
        self.tx.rollback().await.map_err(map_sqlx_error)
    }

    pub(super) async fn publish_evidence(
        &mut self,
        state: &AppState,
        user: &AuthenticatedUser,
        version: &attune_common::models::artifact_version::ArtifactVersion,
        stream: &LogStream,
        headers: &HeaderMap,
        truncated: bool,
    ) -> Result<(), (StatusCode, String)> {
        let proof = encrypted_log_seal_proof(
            &mut self.tx,
            state,
            user,
            version,
            stream,
            headers,
            &self.effect,
        )
        .await?;
        self.recheck().await?;
        if let Some(proof) = proof {
            if let Some((context, permit)) = &proof.recorded {
                attune_common::repositories::runtime_log_completion::RuntimeLogCompletionRepository::record(
                    &mut self.tx, context, permit, &proof.manifest, &proof.authenticated, truncated).await.map_err(map_transport_error)?;
            }
            ArtifactVersionRepository::set_encrypted_log_manifest(
                &mut self.tx,
                version.id,
                &proof.authenticated,
            )
            .await
            .map_err(map_transport_error)?;
        }
        Ok(())
    }

    /// Access only this phase's transaction for target row locks/reloads.
    pub fn transaction(&mut self) -> &mut sqlx::Transaction<'db, sqlx::Postgres> {
        &mut self.tx
    }

    /// Run after target locks/waits, immediately before every prepare/final effect.
    pub async fn recheck(&mut self) -> Result<(), (StatusCode, String)> {
        if let Producer::Sensor {
            sensor,
            fence,
            metadata_revision,
            ref schema_binding,
            ref log_generation,
        } = self.producer
        {
            let revision = attune_common::repositories::managed_sensor_metadata::ManagedSensorMetadataRepository::revision_for_generation(
                &mut self.tx, fence.workload_id, fence.generation).await.map_err(map_transport_error)?;
            let schema = attune_common::repositories::sensor_schema::SensorSchemaRepository::find(
                &mut *self.tx,
                fence.workload_id,
                fence.generation,
            )
            .await
            .map_err(map_transport_error)?
            .map(|(id, binding, _)| binding.binding_digest().map(|digest| (id, digest)))
            .transpose()
            .map_err(map_transport_error)?;
            let log = attune_common::repositories::sensor_schema::SensorSchemaRepository::find_log_generation(
                &mut *self.tx, self.version).await.map_err(map_transport_error)?;
            if revision != metadata_revision || &schema != schema_binding || &log != log_generation
            {
                return Err(denied());
            }
            SensorWorkloadRepository::recheck_locked_fence(&mut self.tx, sensor, fence)
                .await
                .map_err(map_transport_error)?;
        }
        self.effect
            .ensure_fresh_on(&mut self.tx)
            .await
            .map_err(map_api_error)
    }

    /// Target stage; the caller then locks/reloads version and stream and calls bind.
    pub async fn authorize_target(
        &mut self,
        user: &AuthenticatedUser,
        version: &attune_common::models::artifact_version::ArtifactVersion,
    ) -> Result<(), (StatusCode, String)> {
        if version.id != self.version
            || version.artifact != self.artifact
            || version.execution != self.execution
        {
            return Err(denied());
        }
        authorize_version_mutation_target(&mut self.tx, user, version, &self.effect).await?;
        if let Producer::Sensor { .. } = self.producer {
            let reference = scoped_metadata_value(user, "sensor_ref", "Sensor")?;
            if !ArtifactVersionRepository::file_path_owned_by_sensor(
                &mut *self.tx,
                version.file_path.as_deref().ok_or_else(denied)?,
                reference,
            )
            .await
            .map_err(map_transport_error)?
            {
                return Err(denied());
            }
        }
        self.recheck().await
    }

    /// Authenticates the existing v1 final manifest, checks current caps and binds
    /// exact authenticated input without persisting a bearer token or secret key.
    #[allow(clippy::too_many_arguments)]
    pub async fn bind(
        &mut self,
        state: &AppState,
        user: &AuthenticatedUser,
        version: &attune_common::models::artifact_version::ArtifactVersion,
        stream: &LogStream,
        headers: &HeaderMap,
        expected: Option<&HistoricalBinding>,
    ) -> Result<HistoricalBinding, (StatusCode, String)> {
        if version.id != self.version
            || version.artifact != self.artifact
            || version.execution != self.execution
            || stream.artifact_version != version.id
        {
            return Err(denied());
        }
        check_delivery_bounds(
            &mut self.tx,
            user,
            version.id,
            Some(stream.id),
            attune_common::repositories::worker_delivery::DeliveryOperation::Seal {
                sequences: stream.next_sequence,
                bytes: stream.total_bytes,
            },
        )
        .await?;
        let encrypted = encrypted_log_seal_proof(
            &mut self.tx,
            state,
            user,
            version,
            stream,
            headers,
            &self.effect,
        )
        .await?;
        let proof = match encrypted {
            None => ProofBinding::Plain,
            Some(proof) => ProofBinding::Encrypted {
                plaintext_cap: proof.plaintext_cap,
                ciphertext_cap: proof.ciphertext_cap,
                authenticated_sha256: hex_digest(
                    &Sha256::digest(proof.authenticated.as_bytes()).into(),
                ),
                manifest: proof.manifest,
            },
        };
        let binding = HistoricalBinding {
            version: version.id,
            stream: stream.id,
            producer: self.producer.clone(),
            proof,
        };
        if expected.is_some_and(|expected| expected != &binding) {
            return Err((
                StatusCode::CONFLICT,
                "Log source/proof binding changed".into(),
            ));
        }
        self.recheck().await?;
        Ok(binding)
    }
}
