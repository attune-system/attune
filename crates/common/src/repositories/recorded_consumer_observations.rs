//! Database-backed recorded consumer observations. This transport does not
//! install a consumer, alter membership, or supply execution authority.

use chrono::{DateTime, TimeDelta, Utc};
use serde::Deserialize;
use sqlx::{PgConnection, PgPool, Postgres, Transaction};
use std::collections::{BTreeMap, BTreeSet};
use uuid::Uuid;

use crate::execution_authority::consumer_contract::{
    validate_consumer_contracts, ConsumerRole, ContractFreshnessPolicy, ContractSnapshot,
    ExpectedMember, ExpectedMembership, RecordedAuthorityConsumerContract,
    ValidatedConsumerContractSnapshot,
};
use crate::{Error, Result};

use super::runtime::{CreateWorkerInput, WorkerRepository};

const OBSERVATION_WINDOW_SECONDS: i64 = 30;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DatabaseEvidence {
    expected: ExpectedMembership,
    observations: Vec<RecordedAuthorityConsumerContract>,
    now: DateTime<Utc>,
    revision: i64,
    caller: Option<ExpectedMember>,
    worker_targets: Vec<WorkerTarget>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkerTarget {
    worker_id: crate::models::Id,
    instance: Uuid,
    boot: Uuid,
}

/// Validated rollout evidence with the exact eligible worker registrations.
/// Target matching does not replace the scheduler's normal placement checks.
pub struct RecordedRolloutReadiness {
    contract: ValidatedConsumerContractSnapshot,
    workers: BTreeMap<crate::models::Id, Uuid>,
}

impl RecordedRolloutReadiness {
    pub fn matches_worker(
        &self,
        worker: crate::models::Id,
        boot: Uuid,
        now: DateTime<Utc>,
    ) -> bool {
        now < self.contract.valid_until() && self.workers.get(&worker) == Some(&boot)
    }
}

/// Operator-supplied identity for this process boot. The database pool's
/// authenticated login must have been bound to the same identity.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecordedConsumerIdentity {
    snapshot: ContractSnapshot,
    instance: Uuid,
    boot: Uuid,
    #[serde(skip, default = "Uuid::new_v4")]
    process_claim: Uuid,
}

impl RecordedConsumerIdentity {
    pub fn from_environment() -> Result<Option<Self>> {
        let Some(path) = std::env::var_os("ATTUNE_RECORDED_CONSUMER_IDENTITY_FILE") else {
            return Ok(None);
        };
        let denied = || Error::validation("Invalid recorded consumer identity file");
        let body =
            crate::rollout_offline::read_private_with(std::path::Path::new(&path), 4096, || {})
                .map_err(|_| denied())?;
        let identity: Self = serde_json::from_slice(&body).map_err(|_| denied())?;
        let snapshot = &identity.snapshot;
        if snapshot.contract_version
            != crate::execution_authority::consumer_contract::CONTRACT_VERSION
            || snapshot.catalog_epoch != crate::platform_catalog::COMPATIBILITY_EPOCH
            || snapshot.catalog_revision != crate::platform_catalog::CATALOG_REVISION
            || snapshot.rollout_generation.is_nil()
            || identity.instance.is_nil()
            || identity.boot.is_nil()
            || snapshot.release_digest.len() != 64
            || !snapshot
                .release_digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(denied());
        }
        Ok(Some(identity))
    }

    pub async fn publish(
        &self,
        connection: &mut PgConnection,
        ready_role: Option<ConsumerRole>,
    ) -> Result<()> {
        RecordedConsumerObservations::write(connection, self, ready_role, None).await
    }

    /// Claim the deployment-assigned boot before touching the routing row.
    /// This stores no readiness and rolls back with unsuccessful registration.
    pub async fn reserve_worker_boot(&self, tx: &mut Transaction<'_, Postgres>) -> Result<Uuid> {
        sqlx::query("SELECT report_recorded_consumer($1,$2,$3,$4,NULL,NULL,TRUE)")
            .bind(serde_json::to_value(&self.snapshot)?)
            .bind(self.instance)
            .bind(self.boot)
            .bind(self.process_claim)
            .execute(&mut **tx)
            .await?;
        Ok(self.boot)
    }

    /// Register the deployment-assigned Action worker. A process that loses the
    /// boot claim never writes the existing routing row. Reservation alone
    /// remains invisible to readiness until real consumers report support.
    pub async fn register_worker(
        &self,
        pool: &PgPool,
        mut input: CreateWorkerInput,
    ) -> Result<(crate::models::Worker, Uuid)> {
        let capabilities = input
            .capabilities
            .get_or_insert_with(|| serde_json::json!({}))
            .as_object_mut()
            .ok_or_else(|| Error::validation("Worker capabilities must be an object"))?;
        capabilities.insert(
            crate::execution_authority::RECORDED_EXECUTION_CAPABILITY.into(),
            serde_json::json!({
            "snapshot": self.snapshot,
            "instance": self.instance,
            "boot": self.boot,
            }),
        );
        let mut tx = pool.begin().await?;
        let boot = self.reserve_worker_boot(&mut tx).await?;
        let role = crate::models::WorkerRole::Action;
        let registration =
            WorkerRepository::register_incarnation(&mut *tx, input, role, boot).await?;
        if registration.worker_role != role {
            return Err(Error::PermissionDenied(
                "Recorded worker registration conflicts with another worker role".into(),
            ));
        }
        tx.commit().await?;
        Ok((registration, boot))
    }

    /// Report installed worker support against this process's own registration.
    pub async fn publish_worker(
        &self,
        connection: &mut PgConnection,
        worker: crate::models::Id,
    ) -> Result<()> {
        RecordedConsumerObservations::write(
            connection,
            self,
            Some(ConsumerRole::Worker),
            Some(worker),
        )
        .await
    }

    /// Check the aggregate under the effect transaction and require this
    /// service's actual database session to be its exact admitted member.
    /// A reader-only login or a sibling boot cannot use the aggregate.
    pub async fn require_current(
        &self,
        transaction: &mut Transaction<'_, Postgres>,
        role: ConsumerRole,
    ) -> Result<RecordedRolloutReadiness> {
        let evidence =
            RecordedConsumerObservations::read(transaction, Some(self.process_claim)).await?;
        if evidence.expected.snapshot != self.snapshot
            || evidence.caller.as_ref().is_none_or(|caller| {
                caller.instance != self.instance
                    || caller.boot != self.boot
                    || caller.role != role
                    || caller.required_mask != role.allowed_gate_mask()
            })
        {
            return Err(Error::PermissionDenied(
                "Recorded consumer session does not match active rollout member".into(),
            ));
        }
        evidence.validate()
    }
}

pub struct RecordedConsumerObservations;

impl RecordedConsumerObservations {
    /// Check access on the API's admitted queue writes, runtime provenance and
    /// exact artifact delivery paths before reporting the installed consumer.
    pub async fn api_access_available(connection: &mut PgConnection) -> Result<bool> {
        sqlx::query_scalar(
            "SELECT NOT EXISTS (
                SELECT 1
                FROM (
                    VALUES
                      ('execution_authority_selection', ARRAY['SELECT'], TRUE),
                      ('runtime_authority_admission', ARRAY['SELECT','INSERT'], TRUE),
                      ('runtime_authority_execution_fence', ARRAY['SELECT'], TRUE),
                      ('runtime_input_receipt', ARRAY['SELECT','INSERT'], TRUE),
                      ('execution_secret_value', ARRAY['SELECT','INSERT','DELETE'], TRUE),
                      ('execution_runtime_read_fence', ARRAY['SELECT','INSERT'], TRUE),
                      ('execution_runtime_read_origin', ARRAY['SELECT','INSERT'], TRUE),
                      ('execution_key_read_origin', ARRAY['SELECT','INSERT'], FALSE),
                      ('key', ARRAY['SELECT','INSERT','UPDATE','DELETE'], TRUE),
                      ('work_queue', ARRAY['SELECT'], TRUE),
                      ('work_queue_item', ARRAY['SELECT','INSERT','UPDATE'], TRUE),
                      ('work_queue_item_runtime_admission', ARRAY['SELECT','INSERT'], TRUE),
                      ('work_queue_item_configuration_input', ARRAY['SELECT','INSERT'], TRUE),
                      ('recorded_execution_input_authority', ARRAY['SELECT','INSERT'], TRUE),
                      ('artifact', ARRAY['SELECT','INSERT','UPDATE','DELETE'], TRUE),
                      ('artifact_version', ARRAY['SELECT','INSERT','UPDATE'], TRUE),
                      ('artifact_runtime_owner_binding', ARRAY['SELECT','INSERT'], TRUE),
                      ('artifact_file_body', ARRAY['SELECT','INSERT','UPDATE'], TRUE),
                      ('artifact_file_origin', ARRAY['SELECT','INSERT'], TRUE),
                      ('worker_delivery_context', ARRAY['SELECT'], TRUE),
                      ('worker_delivery_permit', ARRAY['SELECT'], TRUE),
                      ('log_stream', ARRAY['SELECT','INSERT','UPDATE'], TRUE)
                ) AS required(name, privileges, locked)
                CROSS JOIN LATERAL unnest(required.privileges) AS privilege(name)
                WHERE to_regclass(required.name) IS NULL
                    OR NOT has_table_privilege(current_user, to_regclass(required.name), privilege.name)
                    OR required.locked AND NOT has_any_column_privilege(
                        current_user, to_regclass(required.name), 'UPDATE'
                    )
            )",
        )
        .fetch_one(connection)
        .await
        .map_err(Into::into)
    }

    /// Rights used by dispatch, native workflow effects and protected item
    /// completion. The subscription proof is verified independently in the
    /// executor that runs those paths.
    pub async fn executor_access_available(connection: &mut PgConnection) -> Result<bool> {
        sqlx::query_scalar(
            "SELECT NOT EXISTS (
                SELECT 1
                FROM (
                    VALUES
                      ('execution', ARRAY['SELECT','INSERT','UPDATE'], TRUE),
                      ('execution_authority_selection', ARRAY['SELECT','INSERT'], TRUE),
                      ('runtime_authority_admission', ARRAY['SELECT','INSERT'], TRUE),
                      ('runtime_accepted_batch_manifest', ARRAY['SELECT','INSERT'], FALSE),
                      ('runtime_accepted_batch_item', ARRAY['SELECT','INSERT'], FALSE),
                      ('recorded_queue_execution_input', ARRAY['SELECT','INSERT'], TRUE),
                      ('runtime_input_receipt', ARRAY['SELECT','INSERT'], TRUE),
                      ('execution_secret_value', ARRAY['SELECT','INSERT','DELETE'], TRUE),
                      ('execution_runtime_read_fence', ARRAY['SELECT','INSERT','UPDATE'], TRUE),
                      ('worker_delivery_context', ARRAY['SELECT'], TRUE),
                      ('workflow_execution', ARRAY['SELECT','INSERT','UPDATE'], TRUE),
                      ('workflow_task_wait', ARRAY['SELECT','INSERT','UPDATE'], TRUE),
                      ('work_queue', ARRAY['SELECT'], TRUE),
                      ('work_queue_item', ARRAY['SELECT','UPDATE'], TRUE),
                      ('work_queue_dispatch', ARRAY['SELECT','INSERT','UPDATE'], TRUE),
                      ('work_queue_item_configuration_input', ARRAY['SELECT'], TRUE)
                ) AS required(name, privileges, locked)
                CROSS JOIN LATERAL unnest(required.privileges) AS privilege(name)
                WHERE to_regclass(required.name) IS NULL
                    OR NOT has_table_privilege(current_user, to_regclass(required.name), privilege.name)
                    OR required.locked AND NOT has_any_column_privilege(
                        current_user, to_regclass(required.name), 'UPDATE'
                    )
            )",
        )
        .fetch_one(connection)
        .await
        .map_err(Into::into)
    }

    /// Check the database access used by the notifier's recorded source and
    /// target fences before renewing its observation. PostgreSQL row locks
    /// require UPDATE on at least one column even when the query is a read.
    pub async fn notifier_access_available(connection: &mut PgConnection) -> Result<bool> {
        let reads = [
            "artifact_version",
            "recorded_execution_input_authority",
            "recorded_queue_execution_input",
            "recorded_queue_item_input",
            "runtime",
            "runtime_accepted_batch_item",
            "runtime_accepted_batch_manifest",
            "runtime_input_only_approval",
            "runtime_standard_owner_binding",
            "runtime_version",
            "work_queue_item_ad_hoc_dispatch",
            "work_queue_item_runtime_admission",
            "workflow_definition",
        ];
        let locked_reads = [
            "action",
            "artifact",
            "artifact_runtime_owner_binding",
            "enforcement",
            "event",
            "execution",
            "identity",
            "managed_executable_metadata_revision",
            "pack",
            "pack_release",
            "pack_release_executable",
            "pack_runtime_identity",
            "pack_runtime_policy_revision",
            "rule",
            "runtime_admission_dependency",
            "runtime_authority_admission",
            "runtime_authority_execution_fence",
            "runtime_external_approval",
            "runtime_workflow_policy_dependency",
            "sensor",
            "trigger",
            "work_queue",
            "work_queue_item",
        ];
        let access = sqlx::query_scalar(
            "SELECT NOT EXISTS (
                SELECT 1
                FROM (
                    SELECT to_regclass(name) AS relation, FALSE AS locks FROM unnest($1::text[]) name
                    UNION ALL
                    SELECT to_regclass(name) AS relation, TRUE AS locks FROM unnest($2::text[]) name
                ) AS required
                WHERE required.relation IS NULL
                    OR NOT has_table_privilege(current_user, required.relation, 'SELECT')
                    OR required.locks AND NOT has_any_column_privilege(
                        current_user, required.relation, 'UPDATE'
                    )
            )
            AND COALESCE((
                SELECT has_any_column_privilege(current_user, relation, 'UPDATE')
                FROM (SELECT to_regclass('work_queue_item_configuration_input') AS relation) input
                WHERE relation IS NOT NULL
                    AND NOT EXISTS (
                        SELECT 1
                        FROM unnest(ARRAY['id', 'item', 'policy_revision', 'configuration_revision',
                            'binding', 'binding_digest', 'source_receipts']) AS column_name
                        WHERE NOT has_column_privilege(current_user, relation, column_name, 'SELECT')
                    )
            ), FALSE)",
        )
        .bind(&reads[..])
        .bind(&locked_reads[..])
        .fetch_one(connection)
        .await?;
        Ok(access)
    }

    async fn write(
        connection: &mut PgConnection,
        identity: &RecordedConsumerIdentity,
        ready_role: Option<ConsumerRole>,
        worker: Option<crate::models::Id>,
    ) -> Result<()> {
        sqlx::query("SELECT report_recorded_consumer($1,$2,$3,$4,$5,$6)")
            .bind(serde_json::to_value(&identity.snapshot)?)
            .bind(identity.instance)
            .bind(identity.boot)
            .bind(identity.process_claim)
            .bind(ready_role.map(|role| i32::from(role.allowed_gate_mask())))
            .bind(worker)
            .execute(connection)
            .await?;
        Ok(())
    }

    /// Active rollout and catalog remain locked through the caller's
    /// transaction. The observations must jointly satisfy the exact membership
    /// at the database's sampling time. The lease is not execution authority.
    pub async fn require_current(
        transaction: &mut Transaction<'_, Postgres>,
    ) -> Result<RecordedRolloutReadiness> {
        Self::read(transaction, None).await?.validate()
    }

    async fn read(
        transaction: &mut Transaction<'_, Postgres>,
        process_claim: Option<Uuid>,
    ) -> Result<DatabaseEvidence> {
        let evidence: Option<sqlx::types::Json<DatabaseEvidence>> =
            sqlx::query_scalar("SELECT current_recorded_consumer_evidence($1)")
                .bind(process_claim)
                .fetch_one(&mut **transaction)
                .await?;
        evidence
            .map(|value| value.0)
            .filter(|value| value.revision > 0)
            .ok_or_else(|| Error::PermissionDenied("No compatible active rollout".into()))
    }
}

impl DatabaseEvidence {
    fn validate(self) -> Result<RecordedRolloutReadiness> {
        let contract = validate_consumer_contracts(
            Some(&self.expected),
            &self.observations,
            self.now,
            ContractFreshnessPolicy {
                window: TimeDelta::seconds(OBSERVATION_WINDOW_SECONDS),
                max_clock_skew: TimeDelta::zero(),
            },
        )
        .map_err(|error| {
            Error::PermissionDenied(format!("Recorded consumer evidence denied: {error:?}"))
        })?;
        let mut expected_workers: BTreeSet<_> = self
            .observations
            .iter()
            .filter(|member| member.role == ConsumerRole::Worker)
            .map(|member| (member.instance, member.boot))
            .collect();
        let mut workers = BTreeMap::new();
        for target in self.worker_targets {
            if target.worker_id <= 0
                || !expected_workers.remove(&(target.instance, target.boot))
                || workers.insert(target.worker_id, target.boot).is_some()
            {
                return Err(Error::PermissionDenied(
                    "Invalid recorded worker targets".into(),
                ));
            }
        }
        if !expected_workers.is_empty() {
            return Err(Error::PermissionDenied(
                "Recorded worker targets are incomplete".into(),
            ));
        }
        Ok(RecordedRolloutReadiness { contract, workers })
    }
}
