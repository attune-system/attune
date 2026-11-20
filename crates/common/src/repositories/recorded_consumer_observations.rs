//! Database-backed recorded consumer observations. This transport does not
//! install a consumer, alter membership, or supply execution authority.

use chrono::{DateTime, TimeDelta, Utc};
use serde::Deserialize;
use sqlx::{PgConnection, Postgres, Transaction};
use uuid::Uuid;

use crate::execution_authority::consumer_contract::{
    validate_consumer_contracts, ConsumerRole, ContractFreshnessPolicy, ContractSnapshot,
    ExpectedMember, ExpectedMembership, RecordedAuthorityConsumerContract,
    ValidatedConsumerContractSnapshot,
};
use crate::{Error, Result};

const OBSERVATION_WINDOW_SECONDS: i64 = 30;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DatabaseEvidence {
    expected: ExpectedMembership,
    observations: Vec<RecordedAuthorityConsumerContract>,
    now: DateTime<Utc>,
    revision: i64,
    caller: Option<ExpectedMember>,
}

/// Operator-supplied identity for this process boot. The database pool's
/// authenticated login must have been bound to the same identity.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecordedConsumerIdentity {
    snapshot: ContractSnapshot,
    instance: Uuid,
    boot: Uuid,
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
        RecordedConsumerObservations::report(
            connection,
            &self.snapshot,
            self.instance,
            self.boot,
            ready_role,
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
    ) -> Result<ValidatedConsumerContractSnapshot> {
        let evidence = RecordedConsumerObservations::read(transaction).await?;
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

    /// Use the member's dedicated login. Supplying the expected identity also
    /// prevents a service initialized with the wrong credentials from refreshing
    /// a different replica of the same role.
    pub async fn report(
        connection: &mut PgConnection,
        snapshot: &ContractSnapshot,
        instance: Uuid,
        boot: Uuid,
        ready_role: Option<ConsumerRole>,
    ) -> Result<()> {
        sqlx::query("SELECT report_recorded_consumer($1,$2,$3,$4)")
            .bind(serde_json::to_value(snapshot)?)
            .bind(instance)
            .bind(boot)
            .bind(ready_role.map(|role| i32::from(role.allowed_gate_mask())))
            .execute(connection)
            .await?;
        Ok(())
    }

    /// Active rollout and catalog remain locked through the caller's
    /// transaction. The observations must jointly satisfy the exact membership
    /// at the database's sampling time. The lease is not execution authority.
    pub async fn require_current(
        transaction: &mut Transaction<'_, Postgres>,
    ) -> Result<ValidatedConsumerContractSnapshot> {
        Self::read(transaction).await?.validate()
    }

    async fn read(transaction: &mut Transaction<'_, Postgres>) -> Result<DatabaseEvidence> {
        let evidence: Option<sqlx::types::Json<DatabaseEvidence>> =
            sqlx::query_scalar("SELECT current_recorded_consumer_evidence()")
                .fetch_one(&mut **transaction)
                .await?;
        evidence
            .map(|value| value.0)
            .filter(|value| value.revision > 0)
            .ok_or_else(|| Error::PermissionDenied("No compatible active rollout".into()))
    }
}

impl DatabaseEvidence {
    fn validate(self) -> Result<ValidatedConsumerContractSnapshot> {
        validate_consumer_contracts(
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
        })
    }
}
