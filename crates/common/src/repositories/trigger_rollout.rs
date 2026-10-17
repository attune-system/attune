//! Current-contract rollout fence. Pinned emitters still target the live trigger.
use serde::Serialize;
use serde_json::Value;
use sqlx::PgConnection;

use super::{schema_type::SchemaTypeRepository, trigger::TriggerRepository};
use crate::{
    rbac::{Action, AuthorizationContext, Resource},
    runtime_authority::SelectedRuntimeAuthority,
    Error, Result,
};

#[derive(Debug, Default, Serialize)]
pub struct TriggerRolloutImpact {
    pub trigger_id: i64,
    pub execution_ids: Vec<i64>,
    pub workload_ids: Vec<i64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ordinary_confirmation_cannot_replace_explicit_incompatible_rollout_acknowledgement() {
        let impact = TriggerRolloutImpact {
            trigger_id: 9,
            execution_ids: vec![11],
            workload_ids: vec![4],
        };
        assert!(impact.require_drained_or_acknowledged(false).is_err());
        assert!(impact.require_drained_or_acknowledged(true).is_ok());
        assert!(TriggerRolloutImpact::default()
            .require_drained_or_acknowledged(false)
            .is_ok());
    }
}

impl TriggerRolloutImpact {
    pub fn requires_acknowledgement(&self) -> bool {
        !self.execution_ids.is_empty() || !self.workload_ids.is_empty()
    }

    pub fn require_drained_or_acknowledged(
        &self,
        acknowledge_incompatible_rollout: bool,
    ) -> Result<()> {
        if self.requires_acknowledgement() && !acknowledge_incompatible_rollout {
            return Err(Error::validation(format!(
                "Trigger contract rollout requires drain or explicit acknowledge_incompatible_rollout; affected execution IDs {:?}, workload IDs {:?}",
                self.execution_ids, self.workload_ids)));
        }
        Ok(())
    }
}

pub struct TriggerRolloutRepository;

impl TriggerRolloutRepository {
    /// Take before sensor/trigger/pack row locks. Execution admission and sensor
    /// generation capture take shared graph admission. Keep this through activation.
    pub async fn lock_activation(connection: &mut PgConnection) -> Result<()> {
        SchemaTypeRepository::lock_reclamation(connection).await
    }

    /// Call while holding exclusive graph admission, before changing projections.
    /// Any changed contract is conservatively incompatible. No universal schema
    /// compatibility proof is claimed. No payloads, configuration, or grants escape.
    pub async fn preflight(
        connection: &mut PgConnection,
        trigger_id: i64,
        proposed_out_schema: Option<&Value>,
        proposed_event_config_schema: Option<&Value>,
    ) -> Result<TriggerRolloutImpact> {
        let trigger = TriggerRepository::find_by_id_including_retired(&mut *connection, trigger_id)
            .await?
            .ok_or_else(|| Error::validation("Rollout trigger is unavailable"))?;
        let impact = TriggerRolloutImpact {
            trigger_id,
            ..Default::default()
        };
        let empty = serde_json::json!({});
        if trigger.out_schema.as_ref().unwrap_or(&empty) == proposed_out_schema.unwrap_or(&empty)
            && trigger.event_config_schema.as_ref().unwrap_or(&empty)
                == proposed_event_config_schema.unwrap_or(&empty)
        {
            return Ok(impact);
        }
        Self::preflight_changed_contract(connection, trigger_id).await
    }

    /// Candidate graph planners must also call this when a named dependency's
    /// resolved contract changes without changing the trigger's authored ref.
    pub async fn preflight_changed_contract(
        connection: &mut PgConnection,
        trigger_id: i64,
    ) -> Result<TriggerRolloutImpact> {
        let trigger = TriggerRepository::find_by_id_including_retired(&mut *connection, trigger_id)
            .await?
            .ok_or_else(|| Error::validation("Rollout trigger is unavailable"))?;
        let mut impact = TriggerRolloutImpact {
            trigger_id,
            ..Default::default()
        };
        let rows: Vec<(i64, Value)> = sqlx::query_as(
            "SELECT e.id, a.authority FROM runtime_authority_admission a JOIN execution e ON a.owner_kind = 'execution' AND a.owner_id = e.id JOIN pack_runtime_policy_revision p ON p.id = a.policy_revision WHERE a.revoked_at IS NULL AND p.revoked_at IS NULL AND e.status IN ('requested','scheduling','scheduled','running','canceling') ORDER BY e.id")
            .fetch_all(&mut *connection).await?;
        for (id, body) in rows {
            let authority: SelectedRuntimeAuthority = serde_json::from_value(body)
                .map_err(|_| Error::validation("Invalid retained emitter authority"))?;
            let mut context = AuthorizationContext::new(authority.owner.sponsor_identity_id);
            context.target_id = Some(trigger.id);
            context.target_ref = Some(trigger.r#ref.clone());
            context.pack_ref = trigger.pack_ref.clone();
            let target = super::runtime_target::RuntimeTargetRepository::resolve(
                connection,
                super::runtime_target::RuntimeTarget::EventTrigger(trigger.id),
                authority.owner.sponsor_identity_id,
            )
            .await?
            .ok_or_else(|| Error::validation("Rollout target ownership is unavailable"))?;
            context.managing_pack_id = target.pack_id();
            let approvals = authority
                .grants
                .iter()
                .filter_map(|grant| grant.approval_id())
                .collect();
            // Revoked/expired approvals may over-report. They never hide a live emitter.
            if authority.grants.iter().any(|grant| {
                grant.allows(
                    &approvals,
                    target.pack_id(),
                    Resource::Events,
                    Action::Create,
                    &context,
                )
            }) {
                impact.execution_ids.push(id);
            }
        }
        // Inspect admitted generations, not just the trigger's current sensor
        // relation, which may itself have changed since the process was admitted.
        impact.workload_ids = sqlx::query_scalar("SELECT w.id FROM sensor_workload w JOIN sensor_workload_assignment a ON a.workload = w.id JOIN sensor_generation_schema_binding b ON b.workload = a.workload AND b.generation = a.generation WHERE b.binding->'triggers' ? $1 AND a.worker IS NOT NULL AND a.lease_expires_at > clock_timestamp() ORDER BY w.id")
            .bind(&trigger.r#ref).fetch_all(connection).await?;
        Ok(impact)
    }
}
