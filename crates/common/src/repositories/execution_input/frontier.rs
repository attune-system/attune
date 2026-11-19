//! Native consumers retain only identities issued by their completed canonical
//! input walk. This is a flat source-lock projection, never a policy evaluator.
use super::super::{
    runtime_authority::{ApprovalEffectFence, InputAuthorityDependency as Dependency},
    runtime_target::InputOwnershipDependency as Ownership,
};
use crate::{Error, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::PgConnection;
use std::collections::BTreeSet;

fn denied() -> Error {
    Error::PermissionDenied("Recorded input frontier unavailable".into())
}

/// Stored only within the existing encrypted process context. No plaintext
/// parameter fingerprints, current IAM snapshots or independently issued grants.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct FrozenInputFrontier {
    sources: BTreeSet<Dependency>,
    deadline: Option<DateTime<Utc>>,
}
impl FrozenInputFrontier {
    pub(crate) fn empty() -> Self {
        Self {
            sources: BTreeSet::new(),
            deadline: None,
        }
    }

    pub(crate) fn from_held(fence: &ApprovalEffectFence) -> Self {
        Self {
            sources: fence.input_dependencies().clone(),
            deadline: fence.approval_deadline(),
        }
    }
    pub(crate) fn ensure_complete(&self) -> Result<()> {
        if self.sources.iter().any(|source| {
            matches!(
                source,
                Dependency::Incomplete
                    | Dependency::Ownership(
                        Ownership::Enforcement(_)
                            | Ownership::RuntimeVersion(_)
                            | Ownership::Component(
                                super::super::runtime_target::ComponentTarget::Runtime
                                    | super::super::runtime_target::ComponentTarget::Policy
                                    | super::super::runtime_target::ComponentTarget::Dashboard,
                                _
                            )
                    )
            )
        }) {
            return Err(denied());
        }
        Ok(())
    }
    pub(crate) fn deadline(&self) -> Option<DateTime<Utc>> {
        self.deadline
    }
    pub(crate) async fn retain(&self, conn: &mut PgConnection) -> Result<ApprovalEffectFence> {
        self.ensure_complete()?;
        super::super::schema_type::SchemaTypeRepository::lock_selection(conn).await?;
        let mut fence = ApprovalEffectFence::from_input_approval(self.deadline)?;
        for source in &self.sources {
            retain_dependency(conn, source).await?;
            fence = fence.with_input_dependency(source.clone());
        }
        fence.ensure_fresh_on(conn).await?;
        Ok(fence)
    }
    /// Called only AFTER the whole input/contributor/receipt source phase.
    pub(crate) fn epoch_sources(&self) -> Vec<(&'static str, i64)> {
        self.sources
            .iter()
            .filter_map(|source| match source {
                Dependency::Sponsor(id) => Some(("input_sponsor", *id)),
                Dependency::InputContributors(id) => Some(("input_contributors", *id)),
                Dependency::SecretBuffer(id) => Some(("input_secret_buffer", *id)),
                Dependency::ProvenanceBuffer(id) => Some(("input_provenance_buffer", *id)),
                Dependency::IdentityPresence(id) => Some(("input_identity_presence", *id)),
                Dependency::ExecutionPresence(id) => Some(("input_execution_presence", *id)),
                Dependency::WorkflowPresence(id) => Some(("input_workflow_presence", *id)),
                Dependency::Ownership(ownership) => ownership_epoch(ownership),
                Dependency::ArtifactVersion(id) => Some(("input_artifact_version", *id)),
                _ => None,
            })
            .collect()
    }
}

pub(crate) fn origin_epoch(kind: &str) -> bool {
    matches!(
        kind,
        "input_sponsor"
            | "input_contributors"
            | "input_secret_buffer"
            | "input_provenance_buffer"
            | "input_execution_presence"
            | "input_workflow_presence"
            | "input_key_owner"
            | "input_key_class"
            | "input_identity_presence"
            | "input_sensor_live"
            | "input_pack_owner"
            | "input_pack_primary"
            | "input_action_owner"
            | "input_sensor_owner"
            | "input_rule_owner"
            | "input_queue_owner"
            | "input_trigger_owner"
            | "input_execution_owner"
            | "input_queue_item_owner"
            | "input_inquiry_owner"
            | "input_artifact_owner"
            | "input_artifact_visibility"
            | "input_cache_owner"
            | "input_artifact_version"
    )
}

fn ownership_epoch(source: &Ownership) -> Option<(&'static str, i64)> {
    use super::super::runtime_target::ComponentTarget;
    match source {
        Ownership::Key(id) => Some(("input_key_owner", *id)),
        Ownership::KeyClassification(id) => Some(("input_key_class", *id)),
        Ownership::SensorLive(id) => Some(("input_sensor_live", *id)),
        Ownership::Pack(id) => Some(("input_pack_owner", *id)),
        Ownership::PackPrimaryOwner(id) => Some(("input_pack_primary", *id)),
        Ownership::ArtifactVisibility(id) => Some(("input_artifact_visibility", *id)),
        Ownership::Component(kind, id) => match kind {
            ComponentTarget::Action => Some(("input_action_owner", *id)),
            ComponentTarget::Sensor => Some(("input_sensor_owner", *id)),
            ComponentTarget::Rule => Some(("input_rule_owner", *id)),
            ComponentTarget::Queue => Some(("input_queue_owner", *id)),
            ComponentTarget::Trigger => Some(("input_trigger_owner", *id)),
            ComponentTarget::Runtime | ComponentTarget::Policy | ComponentTarget::Dashboard => None,
        },
        Ownership::Execution(id) => Some(("input_execution_owner", *id)),
        Ownership::Enforcement(_) => None,
        Ownership::QueueItem(id) => Some(("input_queue_item_owner", *id)),
        Ownership::Inquiry(id) => Some(("input_inquiry_owner", *id)),
        Ownership::Artifact(id) => Some(("input_artifact_owner", *id)),
        Ownership::CacheNamespace(id) => Some(("input_cache_owner", *id)),
        Ownership::RuntimeVersion(_) => None,
        Ownership::Release(_) | Ownership::Event(..) => None,
    }
}

/// Lock only an already-issued source identity. Immutable proof records keep
/// their existing irreversible revocation, deadline and retention semantics.
pub(crate) async fn retain_dependency(conn: &mut PgConnection, source: &Dependency) -> Result<()> {
    let (query, id) = match source {
        Dependency::Incomplete => return Err(denied()),
        Dependency::IdentityPresence(id) => ("SELECT id FROM identity WHERE id=$1 FOR SHARE",*id),
        Dependency::ExecutionPresence(id) => ("SELECT id FROM execution WHERE id=$1 FOR SHARE",*id),
        Dependency::WorkflowPresence(id) => ("SELECT execution FROM workflow_execution WHERE execution=$1 FOR SHARE",*id),
        Dependency::InputContributors(id) => ("SELECT id FROM execution WHERE id=$1 FOR SHARE",*id),
        Dependency::EventSchema(id) => ("SELECT event FROM event_schema_binding WHERE event=$1 FOR SHARE",*id),
        Dependency::DelegatedOutput(id) => ("SELECT execution FROM execution_authority_selection WHERE execution=$1 AND mode='delegated' AND explicit AND output_grants IS NOT NULL FOR SHARE",*id),
        Dependency::Sponsor(id) => (
            "SELECT p.id FROM pack p JOIN pack_runtime_identity b ON b.pack=p.id JOIN identity i ON i.id=b.identity WHERE p.id=$1 AND NOT i.frozen FOR SHARE OF p,b,i", *id),
        Dependency::Policy(id) => ("SELECT id FROM pack_runtime_policy_revision WHERE id=$1 AND revoked_at IS NULL FOR SHARE",*id),
        Dependency::Admission(id) => ("SELECT id FROM runtime_authority_admission WHERE id=$1 AND revoked_at IS NULL FOR SHARE",*id),
        Dependency::Approval(id) => ("SELECT id FROM runtime_external_approval WHERE id=$1 AND revoked_at IS NULL AND (expires_at IS NULL OR expires_at>clock_timestamp()) FOR SHARE",*id),
        Dependency::DeclaredReceipt(id) => ("SELECT id FROM runtime_declared_input_receipt WHERE id=$1 FOR SHARE",*id),
        Dependency::Receipt(id) => ("SELECT id FROM runtime_input_receipt WHERE id=$1 FOR SHARE",*id),
        Dependency::RuntimeRead(id) => ("SELECT id FROM execution_runtime_read_origin WHERE id=$1 FOR SHARE",*id),
        Dependency::Configuration(id) => ("SELECT id FROM component_configuration_revision WHERE id=$1 FOR SHARE",*id),
        Dependency::SensorBinding(id) => ("SELECT id FROM sensor_generation_schema_binding WHERE id=$1 FOR SHARE",*id),
        Dependency::SensorDelivery(id) => ("SELECT id FROM sensor_rule_config_delivery WHERE id=$1 FOR SHARE",*id),
        Dependency::SensorMetadata(id) => ("SELECT id FROM managed_sensor_metadata_revision WHERE id=$1 AND revoked_at IS NULL FOR SHARE",*id),
        Dependency::SensorRuntime(id) => ("SELECT id FROM sensor WHERE id=$1 AND NOT EXISTS(SELECT 1 FROM managed_sensor_runtime_state WHERE sensor=$1 AND revoked_at IS NOT NULL) FOR SHARE",*id),
        Dependency::EventOrigin(id) => ("SELECT id FROM event_origin_snapshot WHERE id=$1 FOR SHARE",*id),
        Dependency::ExecutionSchema(id) => ("SELECT entity_id FROM runtime_schema_binding WHERE entity_kind='execution' AND entity_id=$1 FOR SHARE",*id),
        Dependency::InquirySchema(id) => ("SELECT entity_id FROM inquiry_schema_binding WHERE entity_id=$1 FOR SHARE",*id),
        Dependency::ProvenanceBuffer(id) => ("SELECT id FROM execution_secret_value WHERE id=$1 FOR SHARE",*id),
        Dependency::SecretBuffer(id) => ("SELECT id FROM execution_secret_value WHERE id=$1 FOR SHARE",*id),
        Dependency::ArtifactVersion(id) => ("SELECT id FROM artifact_version WHERE id=$1 AND (body_state IS NULL OR body_state NOT IN ('pending','deleting','cleanup_claimed')) FOR SHARE",*id),
        Dependency::Ownership(Ownership::SensorLive(id)) => ("SELECT id FROM sensor WHERE id=$1 AND retired_at IS NULL FOR SHARE",*id),
        Dependency::Ownership(ownership) => {
            if let Ownership::Event(id, created) = ownership {
                let found: Option<i64> = sqlx::query_scalar("SELECT id FROM event WHERE id=$1 AND created=$2 FOR SHARE")
                    .bind(id).bind(created).fetch_optional(conn).await?;
                return found.filter(|found| found == id).map(|_| ()).ok_or_else(denied);
            }
            let (table,id) = match ownership {
                Ownership::Pack(id) | Ownership::PackPrimaryOwner(id) => ("pack",id),
                Ownership::Component(kind,id) => (kind.metadata().0,id),
                Ownership::Key(id) | Ownership::KeyClassification(id) => ("key",id),
                Ownership::Execution(id) => ("execution",id),
                Ownership::Enforcement(id) => ("enforcement",id),
                Ownership::QueueItem(id) => ("work_queue_item",id),
                Ownership::Inquiry(id) => ("inquiry",id),
                Ownership::Artifact(id) | Ownership::ArtifactVisibility(id) => ("artifact",id),
                Ownership::CacheNamespace(id) => ("cache_namespace",id),
                Ownership::RuntimeVersion(id) => ("runtime_version",id),
                Ownership::Release(id) => ("pack_release",id),
                Ownership::SensorLive(_) | Ownership::Event(..) => unreachable!(),
            };
            let found: Option<i64> = sqlx::query_scalar(&format!("SELECT id FROM {table} WHERE id=$1 FOR SHARE"))
                .bind(id).fetch_optional(conn).await?;
            return found.filter(|found| found == id).map(|_| ()).ok_or_else(denied);
        }
    };
    if id <= 0 {
        return Err(denied());
    }
    let found: Option<i64> = sqlx::query_scalar(query)
        .bind(id)
        .fetch_optional(conn)
        .await?;
    found
        .filter(|found| *found == id)
        .map(|_| ())
        .ok_or_else(denied)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn input_evidence_frontier_preserves_exact_sources_and_earliest_deadline() {
        let later = Utc::now() + chrono::Duration::minutes(20);
        let earlier = later - chrono::Duration::minutes(10);
        let contributor = ApprovalEffectFence::from_input_approval(Some(later))
            .unwrap()
            .with_input_dependency(Dependency::InputContributors(11));
        let selected = ApprovalEffectFence::from_input_approval(Some(earlier))
            .unwrap()
            .with_input_dependency(Dependency::Sponsor(12))
            .with_input_dependency(Dependency::Approval(13))
            .with_input_dependency(Dependency::ProvenanceBuffer(14));
        let frozen = FrozenInputFrontier::from_held(&contributor.intersect(&selected));
        frozen.ensure_complete().unwrap();
        assert_eq!(frozen.deadline(), Some(earlier));
        assert_eq!(
            frozen.sources,
            BTreeSet::from([
                Dependency::InputContributors(11),
                Dependency::Sponsor(12),
                Dependency::Approval(13),
                Dependency::ProvenanceBuffer(14),
            ])
        );
        let epochs = frozen.epoch_sources();
        assert!(epochs.contains(&("input_sponsor", 12)));
        assert!(epochs.contains(&("input_contributors", 11)));
        assert!(epochs.contains(&("input_provenance_buffer", 14)));
        // Immutable approvals retain their original active-row/deadline proof;
        // there is no invented epoch on the admission/receipt graph.
        assert!(!epochs.iter().any(|(_, id)| *id == 13));
        let encoded = serde_json::to_string(&frozen).unwrap();
        let restored: FrozenInputFrontier = serde_json::from_str(&encoded).unwrap();
        assert!(restored == frozen);
    }

    #[test]
    fn input_evidence_missing_or_unsupported_capture_stays_incomplete() {
        let incomplete = ApprovalEffectFence::from_input_approval(None)
            .unwrap()
            .with_input_dependency(Dependency::Incomplete);
        assert!(FrozenInputFrontier::from_held(&incomplete)
            .ensure_complete()
            .is_err());
        let unsupported = ApprovalEffectFence::from_input_approval(None)
            .unwrap()
            .with_input_dependency(Dependency::Ownership(Ownership::Enforcement(21)));
        assert!(FrozenInputFrontier::from_held(&unsupported)
            .ensure_complete()
            .is_err());
    }

    #[test]
    fn input_evidence_api_filter_preserves_every_existing_guard() {
        for unchanged in [
            "owner",
            "execution",
            "worker",
            "execution_input",
            "sensor",
            "input_unrelated_existing_guard",
        ] {
            assert!(!origin_epoch(unchanged));
        }
        for origin in [
            "input_sponsor",
            "input_contributors",
            "input_key_owner",
            "input_provenance_buffer",
        ] {
            assert!(origin_epoch(origin));
        }
    }
}
