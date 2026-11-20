//! Database-backed recorded consumer observations. This transport does not
//! install a consumer, alter membership, or supply execution authority.

use chrono::{DateTime, TimeDelta, Utc};
use serde::Deserialize;
use sqlx::{PgConnection, Postgres, Transaction};
use uuid::Uuid;

use crate::execution_authority::consumer_contract::{
    validate_consumer_contracts, ConsumerRole, ContractFreshnessPolicy, ContractSnapshot,
    ExpectedMembership, RecordedAuthorityConsumerContract, ValidatedConsumerContractSnapshot,
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
}

pub struct RecordedConsumerObservations;

impl RecordedConsumerObservations {
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
        let evidence: Option<sqlx::types::Json<DatabaseEvidence>> =
            sqlx::query_scalar("SELECT current_recorded_consumer_evidence()")
                .fetch_one(&mut **transaction)
                .await?;
        let evidence = evidence
            .map(|value| value.0)
            .filter(|value| value.revision > 0)
            .ok_or_else(|| Error::PermissionDenied("No compatible active rollout".into()))?;
        validate_consumer_contracts(
            Some(&evidence.expected),
            &evidence.observations,
            evidence.now,
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
