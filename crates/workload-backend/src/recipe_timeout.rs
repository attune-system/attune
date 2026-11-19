//! Private evidence for a fixed Runner's final-child timeout. This conveys no
//! launch, input, publication or cleanup authority. The owning manager freezes
//! the challenge in its encrypted invocation before admitting any effects.
use crate::{
    configuration::NativeBackend,
    planning::{
        RUNTIME_RECIPE_TIMEOUT_CHALLENGE_BYTES, RUNTIME_RECIPE_TIMEOUT_CHALLENGE_ENV,
        RUNTIME_RECIPE_TIMEOUT_RECEIPT,
    },
};
use attune_common::{
    artifact_transport::ValidatedRelativePath,
    isolated_process::{ProcessSpec, ResourceIdentity},
    workload_launch::{LaunchInputKind, LaunchPurpose, LaunchRoot, PackAccess, RUNNER_ROOT},
    Error, Result,
};
use std::io::Read;

fn unavailable() -> Error {
    Error::PermissionDenied("Native recipe timeout evidence unavailable".into())
}

/// Manager-only state. Never serialize this separately, log it, or pass it to
/// the untrusted final child. The fixed Runner consumes its reserved env entry.
#[derive(Clone)]
pub struct RecipeTimeoutChallenge {
    intent: uuid::Uuid,
    value: String,
}
impl std::fmt::Debug for RecipeTimeoutChallenge {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("RecipeTimeoutChallenge([redacted])")
    }
}

fn valid(value: &str) -> bool {
    value.len() == RUNTIME_RECIPE_TIMEOUT_CHALLENGE_BYTES
        && value
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn fixed_recipe(spec: &ProcessSpec) -> Result<bool> {
    let access = match spec.invocation.argv.get(1).map(String::as_str) {
        Some("run-runtime-recipe")
            if matches!(spec.purpose, LaunchPurpose::Action | LaunchPurpose::Sensor) =>
        {
            PackAccess::ReadOnly
        }
        Some("run-candidate-runtime-recipe") if spec.purpose == LaunchPurpose::CandidateTest => {
            PackAccess::CandidateCopy
        }
        Some("run-runtime-recipe" | "run-candidate-runtime-recipe") => return Err(unavailable()),
        _ => return Ok(false),
    };
    if spec.invocation.argv.first()
        != Some(&format!(
            "{RUNNER_ROOT}/linux-{}",
            spec.platform.native_arch()
        ))
        || spec.invocation.working_root != LaunchRoot::Pack
        || spec.pack_access != access
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
        return Err(unavailable());
    }
    Ok(true)
}

/// Capture from an already-retained exact spec BEFORE ordinary environment
/// planning replaces its env. A fresh unprepared spec has no challenge.
pub fn retained_challenge(spec: &ProcessSpec) -> Result<Option<RecipeTimeoutChallenge>> {
    let Some(value) = spec
        .invocation
        .env
        .get(RUNTIME_RECIPE_TIMEOUT_CHALLENGE_ENV)
    else {
        return Ok(None);
    };
    if !fixed_recipe(spec)? || !valid(value) {
        return Err(unavailable());
    }
    Ok(Some(RecipeTimeoutChallenge {
        intent: spec.intent,
        value: value.clone(),
    }))
}

/// Call AFTER ordinary selected-environment validation and BEFORE the durable
/// encrypted-spec reservation. Recovery preserves the original challenge.
pub fn bind_challenge(
    spec: &mut ProcessSpec,
    retained: Option<RecipeTimeoutChallenge>,
) -> Result<()> {
    if !fixed_recipe(spec)? {
        return if retained.is_some()
            || spec
                .invocation
                .env
                .contains_key(RUNTIME_RECIPE_TIMEOUT_CHALLENGE_ENV)
        {
            Err(unavailable())
        } else {
            Ok(())
        };
    }
    if spec
        .invocation
        .env
        .contains_key(RUNTIME_RECIPE_TIMEOUT_CHALLENGE_ENV)
    {
        return Err(unavailable());
    }
    if retained
        .as_ref()
        .is_some_and(|proof| proof.intent != spec.intent)
    {
        return Err(unavailable());
    }
    let challenge = retained.unwrap_or_else(|| {
        // UUIDv4 uses OS randomness; two provide a fresh private challenge
        // without introducing another credential or dependency.
        RecipeTimeoutChallenge {
            intent: spec.intent,
            value: format!(
                "{}{}",
                uuid::Uuid::new_v4().simple(),
                uuid::Uuid::new_v4().simple()
            ),
        }
    });
    spec.invocation
        .env
        .insert(RUNTIME_RECIPE_TIMEOUT_CHALLENGE_ENV.into(), challenge.value);
    Ok(())
}

fn verify_receipt(expected: &RecipeTimeoutChallenge, bytes: &[u8]) -> Result<()> {
    if bytes != expected.value.as_bytes() {
        return Err(unavailable());
    }
    Ok(())
}

/// Call only after joining capture and observing the exact terminal resource.
/// The existing selected-file reader enforces stopped-writer/removal evidence.
/// Absence means no helper timeout receipt; an unsafe, forged or lost tree is
/// unknown evidence, not permission to infer timeout from any numeric exit.
pub async fn observed_timeout(
    backend: &NativeBackend,
    spec: &ProcessSpec,
    resource: &ResourceIdentity,
) -> Result<bool> {
    let Some(expected) = retained_challenge(spec)? else {
        return Ok(false);
    };
    let selected = ValidatedRelativePath::new(RUNTIME_RECIPE_TIMEOUT_RECEIPT)?;
    let Some(mut file) = backend
        .read_stopped_artifact(
            spec,
            resource,
            &selected,
            RUNTIME_RECIPE_TIMEOUT_CHALLENGE_BYTES as u64,
        )
        .await?
    else {
        return Ok(false);
    };
    let mut bytes = Vec::with_capacity(RUNTIME_RECIPE_TIMEOUT_CHALLENGE_BYTES);
    Read::by_ref(&mut file)
        .take(RUNTIME_RECIPE_TIMEOUT_CHALLENGE_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| unavailable())?;
    verify_receipt(&expected, &bytes)?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use attune_common::workload_launch::{LaunchPlatform, PinnedLaunchInput, Sha256Digest};

    fn recipe() -> ProcessSpec {
        let old =
            crate::test_support::spec(format!("fixture.invalid/payload@sha256:{}", "a".repeat(64)));
        let mut spec = ProcessSpec {
            intent: old.manifest.engine_name_nonce,
            purpose: LaunchPurpose::Action,
            target: old.manifest.target,
            image: old.manifest.image,
            platform: LaunchPlatform::LinuxAmd64,
            inputs: Vec::new(),
            pack_access: PackAccess::ReadOnly,
            stage: None,
            timeout_ms: Some(1000),
            invocation: old.invocation,
        };
        spec.invocation.argv = vec![
            format!("{RUNNER_ROOT}/linux-amd64"),
            "run-runtime-recipe".into(),
        ];
        spec.invocation.working_root = LaunchRoot::Pack;
        for (id, kind) in [(1, LaunchInputKind::Runner), (2, LaunchInputKind::Pack)] {
            spec.inputs.push(PinnedLaunchInput {
                pin_id: id,
                kind,
                object_id: id,
                object_version: "owned-public-source".into(),
                size_bytes: 1,
                sha256: Sha256Digest::of_bytes(b"public-input"),
                max_expanded_bytes: 1,
                max_files: 1,
            });
        }
        spec
    }

    #[test]
    fn recovery_preserves_challenge_but_new_attempts_receive_distinct_values() {
        let mut first = recipe();
        assert!(retained_challenge(&first).unwrap().is_none());
        bind_challenge(&mut first, None).unwrap();
        let frame = crate::runner_protocol::encode_start(&first.invocation).unwrap();
        let decoded = crate::runner_protocol::read_start(frame.as_slice()).unwrap();
        assert_eq!(decoded.env, first.invocation.env);
        crate::process_policy::validate_invocation(&first).unwrap();
        let expected = first.invocation.env[RUNTIME_RECIPE_TIMEOUT_CHALLENGE_ENV].clone();
        assert!(valid(&expected));
        let retained = retained_challenge(&first).unwrap();
        assert!(!format!("{retained:?}").contains(&expected));
        first.invocation.env.clear();
        bind_challenge(&mut first, retained).unwrap();
        assert_eq!(
            first.invocation.env[RUNTIME_RECIPE_TIMEOUT_CHALLENGE_ENV],
            expected
        );
        let mut next = recipe();
        assert!(bind_challenge(&mut next, retained_challenge(&first).unwrap()).is_err());
        bind_challenge(&mut next, None).unwrap();
        assert_ne!(
            next.invocation.env[RUNTIME_RECIPE_TIMEOUT_CHALLENGE_ENV],
            expected
        );
    }

    #[test]
    fn challenge_is_restricted_to_the_fixed_helper_and_matching_purpose() {
        for mutation in 0..4 {
            let mut spec = recipe();
            match mutation {
                0 => spec.invocation.argv[0] = "/opt/attune/pack/forged-helper".into(),
                1 => spec.purpose = LaunchPurpose::RuntimeProbe,
                2 => spec.pack_access = PackAccess::CandidateCopy,
                _ => {
                    spec.inputs.pop().unwrap();
                }
            }
            assert!(bind_challenge(&mut spec, None).is_err());
            assert!(!spec
                .invocation
                .env
                .contains_key(RUNTIME_RECIPE_TIMEOUT_CHALLENGE_ENV));
        }
        let mut candidate = recipe();
        candidate.invocation.argv[1] = "run-candidate-runtime-recipe".into();
        candidate.purpose = LaunchPurpose::CandidateTest;
        candidate.pack_access = PackAccess::CandidateCopy;
        bind_challenge(&mut candidate, None).unwrap();
        crate::runner_protocol::encode_start(&candidate.invocation).unwrap();
        crate::process_policy::validate_invocation(&candidate).unwrap();
        candidate.purpose = LaunchPurpose::Setup;
        assert!(crate::process_policy::validate_invocation(&candidate).is_err());
    }

    #[test]
    fn ordinary_commands_never_receive_the_helper_challenge() {
        let mut spec = recipe();
        bind_challenge(&mut spec, None).unwrap();
        let retained = retained_challenge(&spec).unwrap();
        spec.invocation.argv[1] = "exec".into();
        assert!(retained_challenge(&spec).is_err());
        assert!(crate::runner_protocol::encode_start(&spec.invocation).is_err());
        spec.invocation.env.clear();
        assert!(bind_challenge(&mut spec, retained).is_err());
        bind_challenge(&mut spec, None).unwrap();
        assert!(!spec
            .invocation
            .env
            .contains_key(RUNTIME_RECIPE_TIMEOUT_CHALLENGE_ENV));
    }

    #[test]
    fn forged_truncated_and_extra_receipt_bytes_are_unknown_and_redacted() {
        let expected = RecipeTimeoutChallenge {
            intent: uuid::Uuid::new_v4(),
            value: "a".repeat(64),
        };
        assert!(verify_receipt(&expected, expected.value.as_bytes()).is_ok());
        for bytes in [Vec::new(), vec![b'a'; 63], vec![b'a'; 65], vec![b'b'; 64]] {
            let error = verify_receipt(&expected, &bytes).unwrap_err().to_string();
            assert!(!error.contains(&expected.value));
        }
    }
}
