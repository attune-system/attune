//! Durable launch transition rules. The repository holds source/epoch admission
//! and the operation row through each mutation; this reducer never authorizes an
//! actor and never treats a provider timeout as absence.
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{workload_launch::*, Error, Result};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct LaunchOperation {
    pub status: LaunchStatus,
    pub engine_id: i64,
    pub engine_name_nonce: Uuid,
    pub create_nonce: Option<Uuid>,
    pub start_nonce: Option<Uuid>,
    pub lease_id: Uuid,
    pub lease_generation: i64,
    pub lease_purpose: LaunchLeasePurpose,
    pub lease_deadline: DateTime<Utc>,
}
fn denied() -> Error {
    Error::PermissionDenied("Launch operation unavailable".into())
}
impl LaunchOperation {
    pub fn reserved(manifest: &LaunchManifest) -> Result<Self> {
        Ok(Self {
            status: LaunchStatus {
                request: LaunchRequest {
                    launch_id: manifest.launch_id,
                    admitted_digest: manifest.admitted_digest()?,
                },
                state: LaunchState::Reserved,
                runtime_state: LaunchRuntimeState::Unstarted,
                exit_outcome: None,
                publication: if manifest.purpose == LaunchPurpose::Setup {
                    LaunchPublicationState::Pending
                } else {
                    LaunchPublicationState::NotRequired
                },
                resource: None,
                stop_reason: None,
            },
            engine_id: manifest.engine_id,
            engine_name_nonce: manifest.engine_name_nonce,
            create_nonce: None,
            start_nonce: None,
            lease_id: Uuid::new_v4(),
            lease_generation: 0,
            lease_purpose: LaunchLeasePurpose::Run,
            lease_deadline: manifest.lease_deadline,
        })
    }
    pub fn handle(&self) -> LaunchHandle {
        LaunchHandle {
            launch_id: self.status.request.launch_id,
            admitted_digest: self.status.request.admitted_digest.clone(),
            lease_id: self.lease_id,
            lease_generation: self.lease_generation,
            lease_deadline: self.lease_deadline,
            engine_id: self.engine_id,
            engine_name_nonce: self.engine_name_nonce,
        }
    }
    pub fn require_handle(&self, handle: &LaunchHandle, now: DateTime<Utc>) -> Result<()> {
        if self.handle() != *handle || self.lease_deadline <= now || self.lease_generation <= 0 {
            return Err(denied());
        }
        Ok(())
    }
    pub fn cleanup_only(&self) -> bool {
        matches!(
            self.status.state,
            LaunchState::CleanupRequired | LaunchState::StopRequired | LaunchState::Stopped
        )
    }
    pub fn publication_pending(&self) -> bool {
        self.status.state == LaunchState::Stopped
            && self.status.permits_stage_publication()
            && matches!(
                self.status.publication,
                LaunchPublicationState::Pending | LaunchPublicationState::Uploading
            )
    }
    pub fn claim(
        &mut self,
        deadline: DateTime<Utc>,
        now: DateTime<Utc>,
        same_cleanup_owner: bool,
    ) -> Result<LaunchLeasePurpose> {
        if (self.status.state == LaunchState::Stopped && !self.publication_pending())
            || deadline <= now
        {
            return Err(denied());
        }
        if self.lease_generation > 0
            && self.lease_deadline > now
            && !(same_cleanup_owner && self.cleanup_only())
        {
            // A live owner is never silently replaced, including during cleanup.
            return Err(denied());
        }
        if self.lease_generation > 0 && !self.cleanup_only() {
            self.stop(LaunchStopReason::LeaseExpired);
        }
        self.lease_generation = self.lease_generation.checked_add(1).ok_or_else(denied)?;
        self.lease_id = Uuid::new_v4();
        self.lease_deadline = deadline;
        self.lease_purpose = if self.publication_pending() {
            LaunchLeasePurpose::Publication
        } else if self.cleanup_only() {
            LaunchLeasePurpose::Cleanup
        } else {
            LaunchLeasePurpose::Run
        };
        Ok(self.lease_purpose)
    }
    pub fn renew(
        &mut self,
        handle: &LaunchHandle,
        deadline: DateTime<Utc>,
        now: DateTime<Utc>,
    ) -> Result<()> {
        self.require_handle(handle, now)?;
        if deadline <= now
            || deadline < self.lease_deadline
            || (self.status.state == LaunchState::Stopped && !self.publication_pending())
        {
            return Err(denied());
        }
        self.lease_deadline = deadline;
        Ok(())
    }
    pub fn create_intent(&mut self) -> Result<LaunchCreateIntent> {
        if self.status.state == LaunchState::Stopped {
            return Err(denied());
        }
        let disposition = if self.cleanup_only() {
            CreateDisposition::StopOnly
        } else if self.create_nonce.is_none() {
            self.create_nonce = Some(Uuid::new_v4());
            self.status.state = LaunchState::CreateIntent;
            CreateDisposition::CreateOnce
        } else {
            CreateDisposition::ReconcileOnly
        };
        // Cleanup of a never-submitted launch does not allocate a create nonce.
        Ok(LaunchCreateIntent {
            handle: self.handle(),
            create_nonce: self.create_nonce.unwrap_or(Uuid::nil()),
            disposition,
        })
    }
    pub fn start_intent(&mut self) -> Result<LaunchStartIntent> {
        if self.status.state == LaunchState::Stopped {
            return Err(denied());
        }
        let disposition = if self.cleanup_only() {
            StartDisposition::StopOnly
        } else if self.status.state != LaunchState::Created || self.status.resource.is_none() {
            return Err(denied());
        } else if self.start_nonce.is_none() {
            self.start_nonce = Some(Uuid::new_v4());
            self.status.runtime_state = LaunchRuntimeState::StartIntent;
            StartDisposition::StartOnce
        } else {
            StartDisposition::ReconcileOnly
        };
        Ok(LaunchStartIntent {
            handle: self.handle(),
            start_nonce: self.start_nonce.unwrap_or(Uuid::nil()),
            disposition,
        })
    }
    fn resource_matches(&self, resource: &EngineResourceEvidence) -> Result<()> {
        if resource.engine_id != self.engine_id
            || resource.engine_name_nonce != self.engine_name_nonce
            || resource.admitted_digest != self.status.request.admitted_digest
            || resource.resource_id.is_empty()
            || resource.resource_id.len() > 256
            || resource.resource_id.bytes().any(|b| b.is_ascii_control())
            || self
                .status
                .resource
                .as_ref()
                .is_some_and(|recorded| recorded != resource)
        {
            return Err(denied());
        }
        Ok(())
    }
    pub fn observe(&mut self, observation: &LaunchObservation) -> Result<()> {
        if self.status.state == LaunchState::Stopped {
            return Err(denied());
        }
        match observation {
            LaunchObservation::CreateUnknown { create_nonce } => {
                if self.create_nonce != Some(*create_nonce) {
                    return Err(denied());
                }
                if self.status.state == LaunchState::CreateIntent {
                    self.status.state = LaunchState::CreateUnknown;
                }
            }
            LaunchObservation::Created {
                create_nonce,
                resource,
            } => {
                if self.create_nonce != Some(*create_nonce) {
                    return Err(denied());
                }
                self.resource_matches(resource)?;
                self.status.resource = Some(resource.clone());
                if !self.cleanup_only() {
                    self.status.state = LaunchState::Created;
                }
            }
            LaunchObservation::StartUnknown {
                start_nonce,
                resource,
            }
            | LaunchObservation::Started {
                start_nonce,
                resource,
            }
            | LaunchObservation::Exited {
                start_nonce,
                resource,
                ..
            } => {
                if self.start_nonce != Some(*start_nonce)
                    || self.status.resource.as_ref() != Some(resource)
                {
                    return Err(denied());
                }
                self.resource_matches(resource)?;
                if let LaunchObservation::Exited {
                    exit_code,
                    oom_killed,
                    ..
                } = observation
                {
                    let outcome = LaunchExitOutcome {
                        exit_code: *exit_code,
                        oom_killed: *oom_killed,
                    };
                    if self.status.exit_outcome.is_some_and(|old| old != outcome) {
                        return Err(denied());
                    }
                    self.status.exit_outcome = Some(outcome);
                    if (outcome.exit_code != 0 || outcome.oom_killed)
                        && self.status.publication != LaunchPublicationState::NotRequired
                    {
                        self.status.publication = LaunchPublicationState::Rejected;
                    }
                    self.status.runtime_state = LaunchRuntimeState::Exited;
                    if self.status.state != LaunchState::StopRequired {
                        self.status.state = LaunchState::CleanupRequired;
                    }
                } else if self.status.runtime_state != LaunchRuntimeState::Exited {
                    match observation {
                        LaunchObservation::Started { .. } => {
                            self.status.runtime_state = LaunchRuntimeState::Running
                        }
                        _ if self.status.runtime_state == LaunchRuntimeState::StartIntent => {
                            self.status.runtime_state = LaunchRuntimeState::StartUnknown
                        }
                        _ => {}
                    }
                }
            }
        }
        self.status.validate_shape()
    }
    /// Closing authority precedes engine stop/wait/remove. The first reason is
    /// retained; successful exit remains reasonless unless explicitly revoked.
    pub fn stop(&mut self, reason: LaunchStopReason) {
        if matches!(
            self.status.publication,
            LaunchPublicationState::Pending | LaunchPublicationState::Uploading
        ) {
            self.status.publication = LaunchPublicationState::Rejected;
        }
        if self.status.stop_reason.is_none() {
            self.status.stop_reason = Some(reason);
        }
        if self.status.state != LaunchState::Stopped {
            self.status.state = LaunchState::StopRequired;
        }
    }
    pub fn acknowledge_removed(
        &mut self,
        acknowledgement: &StopLaunchAcknowledgement,
    ) -> Result<()> {
        if !self.cleanup_only() || self.lease_purpose != LaunchLeasePurpose::Cleanup {
            return Err(denied());
        }
        match &acknowledgement.create {
            ProviderCreateSettlement::NeverSubmitted => {
                if self.create_nonce.is_some()
                    || self.status.resource.is_some()
                    || acknowledgement.removed_resource.is_some()
                {
                    return Err(denied());
                }
            }
            ProviderCreateSettlement::DefinitiveAbsence { create_nonce } => {
                if self.create_nonce != Some(*create_nonce)
                    || self.status.resource.is_some()
                    || acknowledgement.removed_resource.is_some()
                {
                    return Err(denied());
                }
            }
            ProviderCreateSettlement::Created {
                create_nonce,
                resource,
            } => {
                if self.create_nonce != Some(*create_nonce)
                    || acknowledgement.removed_resource.as_ref() != Some(resource)
                {
                    return Err(denied());
                }
                self.resource_matches(resource)?;
                self.status.resource = Some(resource.clone());
            }
        }
        self.status.state = LaunchState::Stopped;
        self.status.validate_shape()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn operation() -> LaunchOperation {
        let digest = Sha256Digest::of_bytes(b"owned-launch-public-manifest");
        LaunchOperation {
            status: LaunchStatus {
                request: LaunchRequest {
                    launch_id: 1,
                    admitted_digest: digest,
                },
                state: LaunchState::Reserved,
                runtime_state: LaunchRuntimeState::Unstarted,
                exit_outcome: None,
                publication: LaunchPublicationState::NotRequired,
                resource: None,
                stop_reason: None,
            },
            engine_id: 2,
            engine_name_nonce: Uuid::new_v4(),
            create_nonce: None,
            start_nonce: None,
            lease_id: Uuid::new_v4(),
            lease_generation: 0,
            lease_purpose: LaunchLeasePurpose::Run,
            lease_deadline: time(30),
        }
    }
    fn time(seconds: i64) -> DateTime<Utc> {
        DateTime::from_timestamp(seconds, 0).unwrap()
    }
    fn resource(op: &LaunchOperation) -> EngineResourceEvidence {
        EngineResourceEvidence {
            engine_id: op.engine_id,
            resource_id: "owned-provider-resource".into(),
            engine_name_nonce: op.engine_name_nonce,
            admitted_digest: op.status.request.admitted_digest.clone(),
            public_manifest_digest: Sha256Digest::of_bytes(b"owned-original-engine-policy"),
        }
    }
    fn created(op: &mut LaunchOperation) -> EngineResourceEvidence {
        assert_eq!(
            op.claim(time(30), time(1), false).unwrap(),
            LaunchLeasePurpose::Run
        );
        let intent = op.create_intent().unwrap();
        assert_eq!(intent.disposition, CreateDisposition::CreateOnce);
        let resource = resource(op);
        op.observe(&LaunchObservation::Created {
            create_nonce: intent.create_nonce,
            resource: resource.clone(),
        })
        .unwrap();
        resource
    }
    #[test]
    fn create_and_start_authority_are_durable_one_shot_decisions() {
        let mut op = operation();
        op.claim(time(30), time(1), false).unwrap();
        let create = op.create_intent().unwrap();
        let mut restarted: LaunchOperation =
            serde_json::from_slice(&serde_json::to_vec(&op).unwrap()).unwrap();
        let retry = restarted.create_intent().unwrap();
        assert_eq!(retry.create_nonce, create.create_nonce);
        assert_eq!(retry.disposition, CreateDisposition::ReconcileOnly);
        let resource = resource(&restarted);
        restarted
            .observe(&LaunchObservation::Created {
                create_nonce: create.create_nonce,
                resource,
            })
            .unwrap();
        let start = restarted.start_intent().unwrap();
        assert_eq!(start.disposition, StartDisposition::StartOnce);
        let retry = restarted.start_intent().unwrap();
        assert_eq!(retry.start_nonce, start.start_nonce);
        assert_eq!(retry.disposition, StartDisposition::ReconcileOnly);
    }
    #[test]
    fn unknown_create_cannot_ack_never_submitted_or_restart_creation() {
        let mut op = operation();
        op.claim(time(30), time(1), false).unwrap();
        let intent = op.create_intent().unwrap();
        op.observe(&LaunchObservation::CreateUnknown {
            create_nonce: intent.create_nonce,
        })
        .unwrap();
        op.stop(LaunchStopReason::Cancelled);
        assert_eq!(
            op.create_intent().unwrap().disposition,
            CreateDisposition::StopOnly
        );
        op.claim(time(40), time(2), true).unwrap();
        let ack = StopLaunchAcknowledgement {
            handle: op.handle(),
            create: ProviderCreateSettlement::NeverSubmitted,
            removed_resource: None,
        };
        assert!(op.acknowledge_removed(&ack).is_err());
        assert_eq!(op.status.state, LaunchState::StopRequired);
        let ack = StopLaunchAcknowledgement {
            create: ProviderCreateSettlement::DefinitiveAbsence {
                create_nonce: intent.create_nonce,
            },
            ..ack
        };
        op.acknowledge_removed(&ack).unwrap();
        assert_eq!(op.status.state, LaunchState::Stopped);
        let resource = resource(&op);
        assert!(op
            .observe(&LaunchObservation::Created {
                create_nonce: intent.create_nonce,
                resource
            })
            .is_err());
    }
    #[test]
    fn normal_exit_is_historical_success_only_after_acknowledged_removal() {
        let mut op = operation();
        op.status.publication = LaunchPublicationState::Pending;
        let resource = created(&mut op);
        let start = op.start_intent().unwrap();
        op.observe(&LaunchObservation::Exited {
            start_nonce: start.start_nonce,
            resource: resource.clone(),
            exit_code: 0,
            oom_killed: false,
        })
        .unwrap();
        assert_eq!(op.status.state, LaunchState::CleanupRequired);
        assert_eq!(op.status.stop_reason, None);
        assert!(!op.status.permits_stage_publication());
        // A delayed Started callback cannot erase the terminal outcome.
        op.observe(&LaunchObservation::Started {
            start_nonce: start.start_nonce,
            resource: resource.clone(),
        })
        .unwrap();
        assert_eq!(op.status.runtime_state, LaunchRuntimeState::Exited);
        assert_eq!(
            op.start_intent().unwrap().disposition,
            StartDisposition::StopOnly
        );
        op.claim(time(40), time(2), true).unwrap();
        let mut ack = StopLaunchAcknowledgement {
            handle: op.handle(),
            create: ProviderCreateSettlement::Created {
                create_nonce: op.create_nonce.unwrap(),
                resource: resource.clone(),
            },
            removed_resource: None,
        };
        assert!(op.acknowledge_removed(&ack).is_err());
        ack.removed_resource = Some(resource);
        op.acknowledge_removed(&ack).unwrap();
        assert!(op.status.permits_stage_publication());
        // Late cancellation/revocation still prohibits publication of success.
        op.stop(LaunchStopReason::SourceRevoked);
        assert!(!op.status.permits_stage_publication());
    }
    #[test]
    fn removed_setup_retains_publication_only_recovery_without_engine_restart() {
        let mut op = operation();
        op.status.publication = LaunchPublicationState::Pending;
        let resource = created(&mut op);
        let start = op.start_intent().unwrap();
        op.observe(&LaunchObservation::Exited {
            start_nonce: start.start_nonce,
            resource: resource.clone(),
            exit_code: 0,
            oom_killed: false,
        })
        .unwrap();
        let run = op.handle();
        assert_eq!(
            op.claim(time(40), time(2), true).unwrap(),
            LaunchLeasePurpose::Cleanup
        );
        assert!(op.require_handle(&run, time(3)).is_err());
        op.acknowledge_removed(&StopLaunchAcknowledgement {
            handle: op.handle(),
            create: ProviderCreateSettlement::Created {
                create_nonce: op.create_nonce.unwrap(),
                resource: resource.clone(),
            },
            removed_resource: Some(resource),
        })
        .unwrap();
        let cleanup = op.handle();
        assert!(op.publication_pending());
        assert_eq!(
            op.claim(time(50), time(3), true).unwrap(),
            LaunchLeasePurpose::Publication
        );
        assert!(op.require_handle(&cleanup, time(4)).is_err());
        assert!(op.create_intent().is_err());
        assert!(op.start_intent().is_err());
        let mut restarted: LaunchOperation =
            serde_json::from_slice(&serde_json::to_vec(&op).unwrap()).unwrap();
        assert_eq!(
            restarted.claim(time(90), time(51), false).unwrap(),
            LaunchLeasePurpose::Publication
        );
        restarted.stop(LaunchStopReason::SourceRevoked);
        assert_eq!(
            restarted.status.publication,
            LaunchPublicationState::Rejected
        );
        assert!(!restarted.publication_pending());
        assert!(restarted.claim(time(100), time(91), false).is_err());
    }
    #[test]
    fn lease_handover_is_cleanup_only_and_denies_old_handle() {
        let mut op = operation();
        let resource = created(&mut op);
        let old = op.handle();
        assert!(op.claim(time(50), time(20), false).is_err());
        assert_eq!(
            op.claim(time(70), time(31), false).unwrap(),
            LaunchLeasePurpose::Cleanup
        );
        assert!(op.require_handle(&old, time(32)).is_err());
        assert!(op.require_handle(&op.handle(), time(32)).is_ok());
        assert_eq!(op.status.stop_reason, Some(LaunchStopReason::LeaseExpired));
        assert_eq!(op.status.resource, Some(resource));
        assert_eq!(
            op.create_intent().unwrap().disposition,
            CreateDisposition::StopOnly
        );
    }
    #[test]
    fn cancellation_before_submission_is_monotonic_and_has_no_create_authority() {
        let mut op = operation();
        op.stop(LaunchStopReason::Cancelled);
        assert_eq!(
            op.claim(time(30), time(1), false).unwrap(),
            LaunchLeasePurpose::Cleanup
        );
        let create = op.create_intent().unwrap();
        assert_eq!(create.create_nonce, Uuid::nil());
        assert_eq!(create.disposition, CreateDisposition::StopOnly);
        op.acknowledge_removed(&StopLaunchAcknowledgement {
            handle: op.handle(),
            create: ProviderCreateSettlement::NeverSubmitted,
            removed_resource: None,
        })
        .unwrap();
        assert!(op.create_intent().is_err());
        assert!(op.start_intent().is_err());
        assert!(op.claim(time(50), time(31), false).is_err());
    }
    #[test]
    fn resource_evidence_cannot_replace_engine_nonce_resource_or_original_policy() {
        let mut op = operation();
        let original = created(&mut op);
        let nonce = op.create_nonce.unwrap();
        let mut other = original.clone();
        other.engine_id += 1;
        assert!(op
            .observe(&LaunchObservation::Created {
                create_nonce: nonce,
                resource: other
            })
            .is_err());
        let mut changed = original.clone();
        changed.public_manifest_digest = Sha256Digest::of_bytes(b"changed-operator-policy");
        assert!(op
            .observe(&LaunchObservation::Created {
                create_nonce: nonce,
                resource: changed
            })
            .is_err());
        let mut changed = original.clone();
        changed.engine_name_nonce = Uuid::new_v4();
        assert!(op
            .observe(&LaunchObservation::Created {
                create_nonce: nonce,
                resource: changed
            })
            .is_err());
        let mut changed = original;
        changed.resource_id = "other-container".into();
        assert!(op
            .observe(&LaunchObservation::Created {
                create_nonce: nonce,
                resource: changed
            })
            .is_err());
    }
}
