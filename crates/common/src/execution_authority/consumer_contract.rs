//! Pure recorded-consumer protocol validation. No production membership source,
//! advertisement, transport, readiness conversion or authority grant lives here.
//! A valid descriptor does not prove that its emitter installed its consumers.

use chrono::{DateTime, TimeDelta, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use uuid::Uuid;

pub const CONTRACT_VERSION: u32 = 1;
const MAX_MEMBERS: usize = 4096;

pub const EXECUTION_ADMISSION: u16 = 1;
pub const API_CONSUMERS: u16 = 1 << 1;
pub const ARTIFACT_DISCLOSURE: u16 = 1 << 2;
pub const RUNTIME_READ_PROVENANCE: u16 = 1 << 3;
pub const NOTIFIER_DELIVERY: u16 = 1 << 4;
pub const WORKER_COMPLETION: u16 = 1 << 5;
pub const NATIVE_EXECUTOR_CONSUMERS: u16 = 1 << 6;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConsumerRole {
    Api,
    Notifier,
    Executor,
    Worker,
}

impl ConsumerRole {
    /// Protocol ownership, not advertised support. Shared responsibilities must
    /// be installed by every owning role, not borrowed from another descriptor.
    pub const fn allowed_gate_mask(self) -> u16 {
        match self {
            Self::Api => {
                EXECUTION_ADMISSION | API_CONSUMERS | ARTIFACT_DISCLOSURE | RUNTIME_READ_PROVENANCE
            }
            Self::Notifier => NOTIFIER_DELIVERY,
            Self::Executor => EXECUTION_ADMISSION | NATIVE_EXECUTOR_CONSUMERS,
            Self::Worker => RUNTIME_READ_PROVENANCE | WORKER_COMPLETION,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContractSnapshot {
    pub contract_version: u32,
    pub catalog_epoch: i32,
    pub catalog_revision: i32,
    pub release_digest: String,
    pub rollout_generation: Uuid,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExpectedMember {
    pub role: ConsumerRole,
    pub instance: Uuid,
    pub boot: Uuid,
    pub required_mask: u16,
}

/// Must come from an independently authoritative rollout owner. This module
/// has no production constructor or repository that can supply that evidence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExpectedMembership {
    pub snapshot: ContractSnapshot,
    pub members: Vec<ExpectedMember>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RecordedAuthorityConsumerContract {
    pub role: ConsumerRole,
    pub instance: Uuid,
    pub boot: Uuid,
    pub snapshot: ContractSnapshot,
    pub gate_mask: u16,
    pub observed: DateTime<Utc>,
    pub expires: DateTime<Utc>,
}

/// Explicit collection policy; there is no production/default policy here.
/// Clock skew is bounded independently and cannot expand a cached lease.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContractFreshnessPolicy {
    pub window: TimeDelta,
    pub max_clock_skew: TimeDelta,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContractValidationError {
    MissingMembership,
    EmptyMembership,
    InvalidMembership,
    InvalidSnapshot,
    UnsupportedContract,
    MissingMember,
    UnexpectedMember,
    DuplicateMember,
    WrongRole,
    WrongBoot,
    SnapshotMismatch,
    GateOverclaim,
    MissingGates,
    InvalidFreshnessWindow,
    FutureObservation,
    InvalidLifetime,
    Stale,
    SnapshotDrift,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CanonicalMembership {
    snapshot: ContractSnapshot,
    members: BTreeMap<Uuid, ExpectedMember>,
}

/// A checked protocol snapshot, not execution authority or production readiness.
/// In particular there is no conversion to `RecordedConsumerReadiness`.
#[derive(Debug, Clone)]
pub struct ValidatedConsumerContractSnapshot {
    membership: CanonicalMembership,
    valid_until: DateTime<Utc>,
    policy: ContractFreshnessPolicy,
}

impl ValidatedConsumerContractSnapshot {
    pub fn valid_until(&self) -> DateTime<Utc> {
        self.valid_until
    }

    /// Reusing this cached snapshot requires the same expected members, boots,
    /// release and generation, with a fresh complete observation set. Callers
    /// can resolve a replacement snapshot separately. This does not change the
    /// lifetime or consumption rules of retained execution/input authority.
    /// Fresh observations may renew expiry; reordering does not change identity.
    pub fn revalidate(
        &self,
        expected: Option<&ExpectedMembership>,
        observations: &[RecordedAuthorityConsumerContract],
        now: DateTime<Utc>,
        policy: ContractFreshnessPolicy,
    ) -> Result<Self, ContractValidationError> {
        let current = validate_consumer_contracts(expected, observations, now, policy)?;
        if current.membership != self.membership || current.policy != self.policy {
            return Err(ContractValidationError::SnapshotDrift);
        }
        Ok(current)
    }
}

pub fn validate_consumer_contracts(
    expected: Option<&ExpectedMembership>,
    observations: &[RecordedAuthorityConsumerContract],
    now: DateTime<Utc>,
    policy: ContractFreshnessPolicy,
) -> Result<ValidatedConsumerContractSnapshot, ContractValidationError> {
    use ContractValidationError as E;
    let expected = expected.ok_or(E::MissingMembership)?;
    if expected.members.is_empty() {
        return Err(E::EmptyMembership);
    }
    if expected.members.len() > MAX_MEMBERS || observations.len() > MAX_MEMBERS {
        return Err(E::InvalidMembership);
    }
    if policy.window <= TimeDelta::zero()
        || policy.max_clock_skew < TimeDelta::zero()
        || policy.max_clock_skew > policy.window
    {
        return Err(E::InvalidFreshnessWindow);
    }
    let cache_deadline = now
        .checked_add_signed(policy.window)
        .ok_or(E::InvalidFreshnessWindow)?;
    validate_snapshot(&expected.snapshot)?;
    let mut members = BTreeMap::new();
    let mut roles = BTreeSet::new();
    for member in &expected.members {
        if member.instance.is_nil()
            || member.boot.is_nil()
            || member.required_mask != member.role.allowed_gate_mask()
        {
            return Err(E::InvalidMembership);
        }
        if members.insert(member.instance, member.clone()).is_some() {
            return Err(E::DuplicateMember);
        }
        roles.insert(member.role);
    }
    if roles
        != BTreeSet::from([
            ConsumerRole::Api,
            ConsumerRole::Notifier,
            ConsumerRole::Executor,
            ConsumerRole::Worker,
        ])
    {
        return Err(E::InvalidMembership);
    }
    let mut seen = BTreeSet::new();
    let mut valid_until = None;
    for observation in observations {
        if !seen.insert(observation.instance) {
            return Err(E::DuplicateMember);
        }
        let member = members
            .get(&observation.instance)
            .ok_or(E::UnexpectedMember)?;
        if observation.role != member.role {
            return Err(E::WrongRole);
        }
        if observation.boot != member.boot {
            return Err(E::WrongBoot);
        }
        validate_snapshot(&observation.snapshot)?;
        if observation.snapshot != expected.snapshot {
            return Err(E::SnapshotMismatch);
        }
        let allowed = observation.role.allowed_gate_mask();
        if observation.gate_mask & !allowed != 0 {
            return Err(E::GateOverclaim);
        }
        if observation.gate_mask != member.required_mask {
            return Err(E::MissingGates);
        }
        if observation.observed.signed_duration_since(now) > policy.max_clock_skew {
            return Err(E::FutureObservation);
        }
        if observation.expires <= observation.observed
            || observation
                .expires
                .signed_duration_since(observation.observed)
                > policy.window
        {
            return Err(E::InvalidLifetime);
        }
        if observation.expires <= now
            || now.signed_duration_since(observation.observed) > policy.window
        {
            return Err(E::Stale);
        }
        let deadline = observation.expires.min(cache_deadline);
        valid_until = Some(valid_until.map_or(deadline, |held: DateTime<Utc>| held.min(deadline)));
    }
    if seen.len() != members.len() {
        return Err(E::MissingMember);
    }
    Ok(ValidatedConsumerContractSnapshot {
        membership: CanonicalMembership {
            snapshot: expected.snapshot.clone(),
            members,
        },
        valid_until: valid_until.ok_or(E::MissingMember)?,
        policy,
    })
}

fn validate_snapshot(snapshot: &ContractSnapshot) -> Result<(), ContractValidationError> {
    if snapshot.contract_version != CONTRACT_VERSION {
        return Err(ContractValidationError::UnsupportedContract);
    }
    if snapshot.catalog_epoch <= 0
        || snapshot.catalog_revision < 0
        || snapshot.rollout_generation.is_nil()
        || snapshot.release_digest.len() != 64
        || !snapshot
            .release_digest
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(ContractValidationError::InvalidSnapshot);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ContractValidationError as E;

    fn freshness() -> ContractFreshnessPolicy {
        ContractFreshnessPolicy {
            window: TimeDelta::seconds(30),
            max_clock_skew: TimeDelta::seconds(2),
        }
    }

    struct Fixture {
        membership: ExpectedMembership,
        observations: Vec<RecordedAuthorityConsumerContract>,
        now: DateTime<Utc>,
    }

    impl Fixture {
        fn new() -> Self {
            let now = DateTime::from_timestamp(2_000_000_000, 0).unwrap();
            let snapshot = ContractSnapshot {
                contract_version: CONTRACT_VERSION,
                catalog_epoch: 1,
                catalog_revision: 2,
                release_digest: "a".repeat(64),
                rollout_generation: Uuid::from_u128(999),
            };
            // Two API replicas: one successful LB response cannot cover both.
            let members = [
                ConsumerRole::Api,
                ConsumerRole::Api,
                ConsumerRole::Notifier,
                ConsumerRole::Executor,
                ConsumerRole::Worker,
            ]
            .into_iter()
            .enumerate()
            .map(|(index, role)| ExpectedMember {
                role,
                instance: Uuid::from_u128(index as u128 + 1),
                boot: Uuid::from_u128(index as u128 + 101),
                required_mask: role.allowed_gate_mask(),
            })
            .collect::<Vec<_>>();
            let observations = members
                .iter()
                .map(|member| RecordedAuthorityConsumerContract {
                    role: member.role,
                    instance: member.instance,
                    boot: member.boot,
                    snapshot: snapshot.clone(),
                    gate_mask: member.role.allowed_gate_mask(),
                    observed: now - TimeDelta::seconds(1),
                    expires: now + TimeDelta::seconds(20),
                })
                .collect();
            Self {
                membership: ExpectedMembership { snapshot, members },
                observations,
                now,
            }
        }

        fn validate(&self) -> Result<ValidatedConsumerContractSnapshot, E> {
            validate_consumer_contracts(
                Some(&self.membership),
                &self.observations,
                self.now,
                freshness(),
            )
        }
    }

    #[test]
    fn membership_cannot_be_inferred_from_observations_or_health() {
        let mut f = Fixture::new();
        assert_eq!(
            validate_consumer_contracts(None, &f.observations, f.now, freshness()).unwrap_err(),
            E::MissingMembership
        );
        f.membership.members.clear();
        assert_eq!(f.validate().unwrap_err(), E::EmptyMembership);
        let mut f = Fixture::new();
        f.observations.clear();
        assert_eq!(f.validate().unwrap_err(), E::MissingMember);
        let mut f = Fixture::new();
        f.membership
            .members
            .retain(|member| member.role != ConsumerRole::Worker);
        assert_eq!(f.validate().unwrap_err(), E::InvalidMembership);
    }

    #[test]
    fn every_expected_replica_requires_exactly_one_descriptor() {
        let mut f = Fixture::new();
        assert_eq!(
            f.validate().unwrap().valid_until(),
            f.now + TimeDelta::seconds(20)
        );
        f.observations.remove(1);
        assert_eq!(f.validate().unwrap_err(), E::MissingMember);
        let mut f = Fixture::new();
        let mut unexpected = f.observations[0].clone();
        unexpected.instance = Uuid::from_u128(90);
        f.observations.push(unexpected);
        assert_eq!(f.validate().unwrap_err(), E::UnexpectedMember);
        let mut f = Fixture::new();
        f.observations.push(f.observations[0].clone());
        assert_eq!(f.validate().unwrap_err(), E::DuplicateMember);
        f.observations.last_mut().unwrap().boot = Uuid::from_u128(900);
        assert_eq!(f.validate().unwrap_err(), E::DuplicateMember);
        let mut f = Fixture::new();
        f.membership.members.push(f.membership.members[0].clone());
        assert_eq!(f.validate().unwrap_err(), E::DuplicateMember);
    }

    #[test]
    fn local_gate_ownership_cannot_be_borrowed_or_overclaimed() {
        let mut f = Fixture::new();
        let worker = f.observations.last_mut().unwrap();
        assert_eq!(
            worker.gate_mask,
            RUNTIME_READ_PROVENANCE | WORKER_COMPLETION
        );
        worker.gate_mask |= API_CONSUMERS;
        assert_eq!(f.validate().unwrap_err(), E::GateOverclaim);
        let mut f = Fixture::new();
        f.observations[0].gate_mask |= 1 << 15;
        assert_eq!(f.validate().unwrap_err(), E::GateOverclaim);
        let mut f = Fixture::new();
        f.observations.last_mut().unwrap().gate_mask &= !WORKER_COMPLETION;
        assert_eq!(f.validate().unwrap_err(), E::MissingGates);
        let mut f = Fixture::new();
        f.observations.last_mut().unwrap().role = ConsumerRole::Api;
        assert_eq!(f.validate().unwrap_err(), E::WrongRole);
        for (index, gate) in [
            (0, EXECUTION_ADMISSION),
            (3, EXECUTION_ADMISSION),
            (0, RUNTIME_READ_PROVENANCE),
            (4, RUNTIME_READ_PROVENANCE),
        ] {
            let mut f = Fixture::new();
            f.observations[index].gate_mask &= !gate;
            assert_eq!(
                f.validate().unwrap_err(),
                E::MissingGates,
                "another role's overlapping bit cannot satisfy this member"
            );
        }
        let mut f = Fixture::new();
        f.membership.members[0].required_mask &= !EXECUTION_ADMISSION;
        assert_eq!(
            f.validate().unwrap_err(),
            E::InvalidMembership,
            "expected membership cannot shrink a role's fixed obligations"
        );
    }

    #[test]
    fn restart_and_mixed_release_catalog_generation_are_denied() {
        let mut f = Fixture::new();
        f.observations[0].boot = Uuid::from_u128(900);
        assert_eq!(f.validate().unwrap_err(), E::WrongBoot);
        for change in 0..4 {
            let mut f = Fixture::new();
            let snapshot = &mut f.observations[0].snapshot;
            match change {
                0 => snapshot.release_digest = "b".repeat(64),
                1 => snapshot.catalog_epoch += 1,
                2 => snapshot.catalog_revision += 1,
                _ => snapshot.rollout_generation = Uuid::from_u128(1000),
            }
            assert_eq!(f.validate().unwrap_err(), E::SnapshotMismatch);
        }
        let mut f = Fixture::new();
        f.observations[0].snapshot.contract_version += 1;
        assert_eq!(f.validate().unwrap_err(), E::UnsupportedContract);
        f.membership.snapshot.contract_version += 1;
        assert_eq!(f.validate().unwrap_err(), E::UnsupportedContract);
    }

    #[test]
    fn stale_future_and_unbounded_observations_deny() {
        let mut f = Fixture::new();
        f.observations[0].expires = f.now;
        assert_eq!(f.validate().unwrap_err(), E::Stale);
        let mut f = Fixture::new();
        f.observations[0].observed = f.now + TimeDelta::seconds(3);
        assert_eq!(f.validate().unwrap_err(), E::FutureObservation);
        let mut f = Fixture::new();
        f.observations[0].expires = f.observations[0].observed;
        assert_eq!(f.validate().unwrap_err(), E::InvalidLifetime);
        let mut f = Fixture::new();
        f.observations[0].expires = f.now + TimeDelta::minutes(5);
        assert_eq!(f.validate().unwrap_err(), E::InvalidLifetime);
        let f = Fixture::new();
        assert_eq!(
            validate_consumer_contracts(
                Some(&f.membership),
                &f.observations,
                f.now,
                ContractFreshnessPolicy {
                    window: TimeDelta::zero(),
                    max_clock_skew: TimeDelta::zero()
                }
            )
            .unwrap_err(),
            E::InvalidFreshnessWindow
        );
        assert_eq!(
            validate_consumer_contracts(
                Some(&f.membership),
                &f.observations,
                f.now + TimeDelta::seconds(20),
                freshness()
            )
            .unwrap_err(),
            E::Stale
        );
    }

    #[test]
    fn bounded_clock_skew_never_expands_cache_or_contract_lifetime() {
        let mut f = Fixture::new();
        for observation in &mut f.observations {
            observation.observed = f.now + TimeDelta::seconds(2);
            observation.expires = f.now + TimeDelta::seconds(32);
        }
        assert_eq!(
            f.validate().unwrap().valid_until(),
            f.now + TimeDelta::seconds(30)
        );
        f.observations[0].expires = f.now + TimeDelta::seconds(10);
        assert_eq!(
            f.validate().unwrap().valid_until(),
            f.now + TimeDelta::seconds(10)
        );
        for skew in [TimeDelta::seconds(-1), TimeDelta::seconds(31)] {
            assert_eq!(
                validate_consumer_contracts(
                    Some(&f.membership),
                    &f.observations,
                    f.now,
                    ContractFreshnessPolicy {
                        window: TimeDelta::seconds(30),
                        max_clock_skew: skew
                    }
                )
                .unwrap_err(),
                E::InvalidFreshnessWindow
            );
        }
        let accepted = f.validate().unwrap();
        assert_eq!(
            accepted
                .revalidate(
                    Some(&f.membership),
                    &f.observations,
                    f.now,
                    ContractFreshnessPolicy {
                        window: TimeDelta::seconds(60),
                        max_clock_skew: TimeDelta::seconds(2)
                    }
                )
                .unwrap_err(),
            E::SnapshotDrift
        );
        assert_eq!(
            accepted
                .revalidate(
                    Some(&f.membership),
                    &f.observations,
                    f.now + TimeDelta::seconds(10),
                    freshness()
                )
                .unwrap_err(),
            E::Stale
        );
    }

    #[test]
    fn admission_snapshot_cannot_drift_at_dispatch_or_worker_replacement() {
        for change in 0..4 {
            let mut f = Fixture::new();
            let accepted = f.validate().unwrap();
            match change {
                0 => {
                    f.membership.snapshot.catalog_revision += 1;
                    for observation in &mut f.observations {
                        observation.snapshot.catalog_revision += 1;
                    }
                }
                1 => {
                    f.membership.snapshot.rollout_generation = Uuid::from_u128(1000);
                    for observation in &mut f.observations {
                        observation.snapshot.rollout_generation = Uuid::from_u128(1000);
                    }
                }
                2 => {
                    f.membership.members.last_mut().unwrap().boot = Uuid::from_u128(1000);
                    f.observations.last_mut().unwrap().boot = Uuid::from_u128(1000);
                }
                _ => {
                    f.membership.members.last_mut().unwrap().instance = Uuid::from_u128(1000);
                    f.observations.last_mut().unwrap().instance = Uuid::from_u128(1000);
                }
            }
            f.validate().unwrap(); // The new rollout is internally coherent.
            assert_eq!(
                accepted
                    .revalidate(Some(&f.membership), &f.observations, f.now, freshness())
                    .unwrap_err(),
                E::SnapshotDrift
            );
        }
    }

    #[test]
    fn complete_fresh_reobservation_can_renew_same_snapshot_in_any_order() {
        let mut f = Fixture::new();
        let accepted = f.validate().unwrap();
        f.now += TimeDelta::seconds(10);
        for observation in &mut f.observations {
            observation.observed = f.now;
            observation.expires = f.now + TimeDelta::seconds(25);
        }
        f.observations.reverse();
        f.membership.members.reverse();
        let renewed = accepted
            .revalidate(Some(&f.membership), &f.observations, f.now, freshness())
            .unwrap();
        assert_eq!(renewed.valid_until(), f.now + TimeDelta::seconds(25));
        f.observations.clear();
        assert_eq!(
            renewed
                .revalidate(Some(&f.membership), &f.observations, f.now, freshness())
                .unwrap_err(),
            E::MissingMember
        );
    }

    #[test]
    fn malformed_identity_snapshot_and_serialization_have_no_defaults() {
        let mut f = Fixture::new();
        f.membership.members[0].instance = Uuid::nil();
        assert_eq!(f.validate().unwrap_err(), E::InvalidMembership);
        for digest in [String::new(), "A".repeat(64), "g".repeat(64)] {
            let mut f = Fixture::new();
            f.observations[0].snapshot.release_digest = digest;
            assert_eq!(f.validate().unwrap_err(), E::InvalidSnapshot);
        }
        let f = Fixture::new();
        let body = serde_json::to_value(&f.observations[0]).unwrap();
        for field in ["role", "boot", "snapshot", "expires", "gate_mask"] {
            let mut omitted = body.clone();
            omitted.as_object_mut().unwrap().remove(field);
            assert!(serde_json::from_value::<RecordedAuthorityConsumerContract>(omitted).is_err());
        }
        let mut wrong = body.clone();
        wrong["role"] = serde_json::json!("all_services");
        assert!(serde_json::from_value::<RecordedAuthorityConsumerContract>(wrong).is_err());
        let mut extra = body;
        extra["ready"] = serde_json::json!(true);
        assert!(serde_json::from_value::<RecordedAuthorityConsumerContract>(extra).is_err());
        let mut f = Fixture::new();
        f.membership
            .members
            .resize(MAX_MEMBERS + 1, f.membership.members[0].clone());
        assert_eq!(f.validate().unwrap_err(), E::InvalidMembership);
    }
}
