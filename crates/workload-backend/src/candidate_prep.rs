//! One fixed helper fact: candidate Python preparation completed. No launch,
//! completion or cleanup authority is conveyed by this private receipt.
use crate::{configuration::NativeBackend, planning::*};
use attune_common::{
    artifact_transport::ValidatedRelativePath,
    isolated_process::{ProcessSpec, ResourceIdentity},
    workload_launch::{LaunchInputKind, LaunchPurpose, LaunchRoot, PackAccess, RUNNER_ROOT},
    Error, Result,
};
use std::io::Read;

#[derive(Clone)]
pub struct CandidatePrepChallenge {
    intent: uuid::Uuid,
    value: String,
}
impl std::fmt::Debug for CandidatePrepChallenge {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("CandidatePrepChallenge([redacted])")
    }
}
fn denied() -> Error {
    Error::PermissionDenied("Candidate Python preparation evidence unavailable".into())
}
fn valid(value: &str) -> bool {
    value.len() == CANDIDATE_PYTHON_PREP_CHALLENGE_BYTES
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn fixed_candidate(spec: &ProcessSpec) -> Result<bool> {
    if spec.invocation.argv.get(1).map(String::as_str) != Some("run-candidate-python") {
        return Ok(false);
    }
    if spec.invocation.argv.first()
        != Some(&format!(
            "{RUNNER_ROOT}/linux-{}",
            spec.platform.native_arch()
        ))
        || spec.purpose != LaunchPurpose::CandidateTest
        || spec.pack_access != PackAccess::CandidateCopy
        || spec.invocation.working_root != LaunchRoot::Pack
        || spec.stage.is_some()
        || spec.inputs.len() != 2
        || spec
            .inputs
            .iter()
            .filter(|p| p.kind == LaunchInputKind::Pack)
            .count()
            != 1
        || spec
            .inputs
            .iter()
            .filter(|p| p.kind == LaunchInputKind::Runner)
            .count()
            != 1
    {
        return Err(denied());
    }
    Ok(true)
}
pub fn retained_challenge(spec: &ProcessSpec) -> Result<Option<CandidatePrepChallenge>> {
    let Some(value) = spec.invocation.env.get(CANDIDATE_PYTHON_PREP_CHALLENGE_ENV) else {
        return Ok(None);
    };
    if !fixed_candidate(spec)? || !valid(value) {
        return Err(denied());
    }
    Ok(Some(CandidatePrepChallenge {
        intent: spec.intent,
        value: value.clone(),
    }))
}
/// After ordinary environment validation and before encrypted reservation.
pub fn bind_challenge(
    spec: &mut ProcessSpec,
    retained: Option<CandidatePrepChallenge>,
) -> Result<()> {
    if !fixed_candidate(spec)? {
        return if retained.is_some()
            || spec
                .invocation
                .env
                .contains_key(CANDIDATE_PYTHON_PREP_CHALLENGE_ENV)
        {
            Err(denied())
        } else {
            Ok(())
        };
    }
    if spec
        .invocation
        .env
        .contains_key(CANDIDATE_PYTHON_PREP_CHALLENGE_ENV)
        || retained
            .as_ref()
            .is_some_and(|proof| proof.intent != spec.intent)
    {
        return Err(denied());
    }
    let proof = retained.unwrap_or_else(|| CandidatePrepChallenge {
        intent: spec.intent,
        value: format!(
            "{}{}",
            uuid::Uuid::new_v4().simple(),
            uuid::Uuid::new_v4().simple()
        ),
    });
    spec.invocation
        .env
        .insert(CANDIDATE_PYTHON_PREP_CHALLENGE_ENV.into(), proof.value);
    Ok(())
}
fn verify(proof: &CandidatePrepChallenge, bytes: &[u8]) -> Result<()> {
    if bytes != proof.value.as_bytes() {
        return Err(denied());
    }
    Ok(())
}
/// Only after exact terminal observation, joined capture and a durable stop
/// fence. The selected-file adapter proves stopped writer/tree identity.
pub async fn observed_prepared(
    backend: &NativeBackend,
    spec: &ProcessSpec,
    resource: &ResourceIdentity,
) -> Result<bool> {
    let proof = retained_challenge(spec)?.ok_or_else(denied)?;
    let selected = ValidatedRelativePath::new(CANDIDATE_PYTHON_PREP_RECEIPT)?;
    let Some(mut file) = backend
        .read_stopped_artifact(
            spec,
            resource,
            &selected,
            CANDIDATE_PYTHON_PREP_CHALLENGE_BYTES as u64,
        )
        .await?
    else {
        return Ok(false);
    };
    let mut bytes = Vec::with_capacity(CANDIDATE_PYTHON_PREP_CHALLENGE_BYTES);
    Read::by_ref(&mut file)
        .take(CANDIDATE_PYTHON_PREP_CHALLENGE_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| denied())?;
    verify(&proof, &bytes)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn candidate_preparation_receipts_are_exact_and_redacted() {
        let proof = CandidatePrepChallenge {
            intent: uuid::Uuid::new_v4(),
            value: "a".repeat(64),
        };
        assert!(verify(&proof, proof.value.as_bytes()).is_ok());
        for bytes in [Vec::new(), vec![b'a'; 63], vec![b'a'; 65], vec![b'b'; 64]] {
            let error = verify(&proof, &bytes).unwrap_err().to_string();
            assert!(!error.contains(&proof.value));
        }
        assert!(!format!("{proof:?}").contains(&proof.value));
        for value in ["A".repeat(64), "g".repeat(64), "a".repeat(63)] {
            assert!(!valid(&value));
        }
    }
}
