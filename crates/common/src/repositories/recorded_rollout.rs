//! Offline operator-owned expected deployment membership. No service registration,
//! observations, authority grants or readiness conversion are provided here.
use crate::{
    execution_authority::consumer_contract::{ConsumerRole, ExpectedMembership, CONTRACT_VERSION},
    Error, Result,
};
use serde_json::Value;
use sqlx::{PgConnection, PgPool};
use std::collections::BTreeSet;
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StagedRollout {
    pub id: i64,
    pub expected: ExpectedMembership,
    pub membership_digest: String,
    pub active: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActiveRollout {
    pub rollout: StagedRollout,
    pub revision: i64,
}

fn canonical(mut expected: ExpectedMembership) -> Result<ExpectedMembership> {
    let s = &expected.snapshot;
    let mut instances = BTreeSet::new();
    let mut boots = BTreeSet::new();
    let mut roles = BTreeSet::new();
    if s.contract_version != CONTRACT_VERSION
        || s.catalog_epoch <= 0
        || s.catalog_revision < 0
        || s.rollout_generation.is_nil()
        || s.release_digest.len() != 64
        || !s
            .release_digest
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        || expected.members.is_empty()
        || expected.members.len() > 4096
    {
        return Err(invalid());
    }
    for m in &expected.members {
        if m.instance.is_nil()
            || m.boot.is_nil()
            || m.required_mask != m.role.allowed_gate_mask()
            || !instances.insert(m.instance)
            || !boots.insert(m.boot)
        {
            return Err(invalid());
        }
        roles.insert(m.role);
    }
    if roles
        != BTreeSet::from([
            ConsumerRole::Api,
            ConsumerRole::Notifier,
            ConsumerRole::Executor,
            ConsumerRole::Worker,
        ])
    {
        return Err(invalid());
    }
    expected.members.sort_by_key(|m| m.instance);
    Ok(expected)
}
fn invalid() -> Error {
    Error::invalid_state("Invalid or conflicting staged deployment membership")
}

pub struct RecordedRolloutRepository;
impl RecordedRolloutRepository {
    /// The DB checks the authenticated session user, not SET ROLE, a GUC, JWT or
    /// a caller-supplied boolean. This seam is NOT called by runtime services.
    pub async fn stage(pool: &PgPool, expected: ExpectedMembership) -> Result<StagedRollout> {
        let expected = canonical(expected)?;
        let body = serde_json::to_value(&expected)?;
        let mut tx = pool.begin().await?;
        sqlx::query("SELECT stage_recorded_rollout($1)")
            .bind(&body)
            .execute(&mut *tx)
            .await?;
        let held = Self::load(&mut tx, expected.snapshot.rollout_generation)
            .await?
            .ok_or_else(invalid)?;
        if held.expected != expected {
            return Err(invalid());
        }
        tx.commit().await?;
        Ok(held)
    }
    pub async fn load(conn: &mut PgConnection, generation: Uuid) -> Result<Option<StagedRollout>> {
        let row:Option<(i64,Value,String,bool)>=sqlx::query_as("SELECT g.id,g.expected,g.membership_digest,EXISTS(SELECT 1 FROM recorded_rollout_active a WHERE a.generation=g.id) FROM recorded_rollout_generation g WHERE g.generation=$1 AND g.membership_digest=recorded_membership_digest(g.expected)")
            .bind(generation).fetch_optional(conn).await?;
        row.map(|(id, body, membership_digest, active)| {
            let expected = serde_json::from_value(body)?;
            Ok(StagedRollout {
                id,
                expected: canonical(expected)?,
                membership_digest,
                active,
            })
        })
        .transpose()
    }
    /// Absence is denial, not an empty membership or implicit local deployment.
    pub async fn active(conn: &mut PgConnection) -> Result<Option<ActiveRollout>> {
        type ActiveRow = (i64, Value, String, i64);
        let row: Option<ActiveRow> = sqlx::query_as("SELECT g.id,g.expected,g.membership_digest,a.revision FROM recorded_rollout_active a JOIN recorded_rollout_generation g ON g.id=a.generation WHERE a.singleton AND g.membership_digest=recorded_membership_digest(g.expected)").fetch_optional(conn).await?;
        row.map(|(id, body, membership_digest, revision)| {
            Ok(ActiveRollout {
                rollout: StagedRollout {
                    id,
                    expected: canonical(serde_json::from_value(body)?)?,
                    membership_digest,
                    active: true,
                },
                revision,
            })
        })
        .transpose()
    }
    pub async fn activate(
        pool: &PgPool,
        generation: Uuid,
        digest: &str,
        prior: Option<i64>,
        prior_revision: i64,
    ) -> Result<StagedRollout> {
        let mut tx = pool.begin().await?;
        let held = Self::load(&mut tx, generation).await?.ok_or_else(invalid)?;
        if held.membership_digest != digest {
            return Err(invalid());
        }
        sqlx::query("SELECT activate_recorded_rollout($1,$2,$3,$4)")
            .bind(generation)
            .bind(digest)
            .bind(prior)
            .bind(prior_revision)
            .execute(&mut *tx)
            .await?;
        let mut held = held;
        held.active = true;
        tx.commit().await?;
        Ok(held)
    }
}
