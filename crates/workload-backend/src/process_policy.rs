//! Public provider placement proof; private invocation bytes are never hashed.
use attune_common::{
    isolated_process::ProcessSpec,
    workload_launch::{LaunchInputKind, LaunchPurpose, PackAccess, Sha256Digest},
    Error, Result,
};
use std::collections::BTreeSet;

pub(crate) fn invalid() -> Error {
    Error::validation("Isolated process policy or resource evidence is invalid")
}
pub(crate) fn public_digest(spec: &ProcessSpec) -> Result<Sha256Digest> {
    if spec.intent.is_nil() || !pinned_image(&spec.image) || spec.inputs.len() > 3 {
        return Err(invalid());
    }
    let mut kinds = BTreeSet::new();
    for pin in &spec.inputs {
        if pin.pin_id <= 0
            || pin.object_id <= 0
            || pin.object_version.is_empty()
            || pin.size_bytes == 0
            || pin.max_expanded_bytes == 0
            || pin.max_files == 0
            || !kinds.insert(pin.kind.container_root())
        {
            return Err(invalid());
        }
    }
    if spec.stage.is_some()
        && (spec.purpose != LaunchPurpose::Setup
            || kinds.contains(LaunchInputKind::Runtime.container_root()))
    {
        return Err(invalid());
    }
    if spec.pack_access == PackAccess::CandidateCopy
        && (spec.purpose != LaunchPurpose::CandidateTest
            || !kinds.contains(LaunchInputKind::Pack.container_root()))
    {
        return Err(invalid());
    }
    if spec
        .stage
        .as_ref()
        .is_some_and(|s| s.quota_bytes == 0 || s.quota_files == 0)
    {
        return Err(invalid());
    }
    let bytes = serde_json::to_vec(&(
        spec.intent,
        spec.purpose,
        &spec.target,
        &spec.image,
        spec.platform,
        &spec.inputs,
        spec.pack_access,
        &spec.stage,
        spec.timeout_ms,
    ))
    .map_err(|_| invalid())?;
    Ok(Sha256Digest::of_bytes(&bytes))
}
pub(crate) fn pinned_image(image: &attune_common::workload_launch::PinnedImage) -> bool {
    image.reference == format!("sha256:{}", image.sha256.as_str()) || registry_image(image)
}
pub(crate) fn registry_image(image: &attune_common::workload_launch::PinnedImage) -> bool {
    image
        .reference
        .split_once("@sha256:")
        .is_some_and(|(name, digest)| {
            !name.is_empty()
                && name
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"./:_-".contains(&b))
                && !name.contains("://")
                && digest == image.sha256.as_str()
        })
}

pub(crate) fn validate_invocation(spec: &ProcessSpec) -> Result<()> {
    use attune_common::workload_launch::LaunchRoot;
    if matches!(
        spec.purpose,
        LaunchPurpose::Setup | LaunchPurpose::RuntimeProbe
    ) && spec.invocation.env.contains_key("ATTUNE_API_TOKEN")
    {
        return Err(invalid());
    }
    let has = |kind| spec.inputs.iter().any(|pin| pin.kind == kind);
    // The codec accepts only the exact control for each fixed helper. Bind
    // each to its purpose, input set and Pack access before provider effects.
    crate::recipe_timeout::retained_challenge(spec)?;
    crate::candidate_prep::retained_challenge(spec)?;
    match spec.invocation.working_root {
        LaunchRoot::Pack if !has(LaunchInputKind::Pack) => return Err(invalid()),
        LaunchRoot::Runtime if !has(LaunchInputKind::Runtime) && spec.stage.is_none() => {
            return Err(invalid())
        }
        LaunchRoot::Stage if spec.purpose != LaunchPurpose::Setup || spec.stage.is_none() => {
            return Err(invalid())
        }
        _ => {}
    }
    crate::runner_protocol::encode_start(&spec.invocation).map_err(|_| invalid())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use attune_common::workload_launch::{
        LaunchPlatform, LaunchRoot, LaunchTarget, PinnedLaunchInput, RUNNER_ROOT,
    };
    use std::io::{Cursor, Read};

    fn candidate(platform: LaunchPlatform) -> ProcessSpec {
        let original =
            crate::test_support::spec(format!("fixture.invalid/payload@sha256:{}", "a".repeat(64)));
        let mut spec = ProcessSpec {
            intent: original.manifest.engine_name_nonce,
            purpose: LaunchPurpose::CandidateTest,
            target: LaunchTarget::CandidateTest {
                install: 1,
                test_job: 1,
                attempt: uuid::Uuid::new_v4(),
            },
            image: original.manifest.image,
            platform,
            inputs: Vec::new(),
            pack_access: PackAccess::CandidateCopy,
            stage: None,
            timeout_ms: Some(1000),
            invocation: original.invocation,
        };
        spec.invocation.argv = vec![
            format!("{RUNNER_ROOT}/linux-{}", platform.native_arch()),
            "run-candidate-python".into(),
        ];
        spec.invocation.working_root = LaunchRoot::Pack;
        for (id, kind) in [(1, LaunchInputKind::Runner), (2, LaunchInputKind::Pack)] {
            spec.inputs.push(PinnedLaunchInput {
                pin_id: id,
                kind,
                object_id: id,
                object_version: "owned-public-input".into(),
                size_bytes: 1,
                sha256: Sha256Digest::of_bytes(b"public-input"),
                max_expanded_bytes: 1,
                max_files: 1,
            });
        }
        spec
    }

    #[test]
    fn candidate_preparation_control_roundtrips_without_consuming_application_stdin() {
        for platform in [LaunchPlatform::LinuxAmd64, LaunchPlatform::LinuxArm64] {
            let mut spec = candidate(platform);
            let public = public_digest(&spec).unwrap();
            crate::candidate_prep::bind_challenge(&mut spec, None).unwrap();
            validate_invocation(&spec).unwrap();
            assert_eq!(public_digest(&spec).unwrap(), public);
            for input in [b"".as_slice(), br#"{"owned":"exact one-shot input"}"#] {
                let mut frame = crate::runner_protocol::encode_start(&spec.invocation).unwrap();
                frame.extend_from_slice(input);
                let mut cursor = Cursor::new(frame);
                let decoded = crate::runner_protocol::read_start(&mut cursor).unwrap();
                assert_eq!(decoded, spec.invocation);
                let mut rest = Vec::new();
                cursor.read_to_end(&mut rest).unwrap();
                assert_eq!(rest, input);
            }
        }
    }

    #[test]
    fn candidate_preparation_control_cannot_escape_its_fixed_process_policy() {
        use crate::planning::CANDIDATE_PYTHON_PREP_CHALLENGE_ENV;
        for mutation in 0..6 {
            let mut spec = candidate(LaunchPlatform::LinuxAmd64);
            crate::candidate_prep::bind_challenge(&mut spec, None).unwrap();
            let private = spec.invocation.env[CANDIDATE_PYTHON_PREP_CHALLENGE_ENV].clone();
            match mutation {
                0 => spec.purpose = LaunchPurpose::Action,
                1 => spec.pack_access = PackAccess::ReadOnly,
                2 => spec.invocation.working_root = LaunchRoot::Scratch,
                3 => spec.invocation.argv[0] = "/opt/attune/pack/forged-helper".into(),
                4 => spec.invocation.argv[1] = "exec".into(),
                _ => {
                    spec.inputs.pop().unwrap();
                }
            }
            let error = validate_invocation(&spec).unwrap_err();
            assert!(!error.to_string().contains(&private));
        }
        for (name, value) in [
            (
                CANDIDATE_PYTHON_PREP_CHALLENGE_ENV.to_owned(),
                "A".repeat(64),
            ),
            (
                CANDIDATE_PYTHON_PREP_CHALLENGE_ENV.to_owned(),
                "a".repeat(63),
            ),
            (
                CANDIDATE_PYTHON_PREP_CHALLENGE_ENV.to_lowercase(),
                "a".repeat(64),
            ),
            ("ATTUNE_LAUNCH_UNRECOGNIZED".into(), "a".repeat(64)),
        ] {
            let mut spec = candidate(LaunchPlatform::LinuxAmd64);
            spec.invocation.env.insert(name, value);
            assert!(crate::runner_protocol::encode_start(&spec.invocation).is_err());
        }
    }

    fn combined_candidate(platform: LaunchPlatform) -> ProcessSpec {
        let mut spec = candidate(platform);
        let runtime: serde_json::Value =
            serde_yaml_ng::from_str(include_str!("../../../packs/core/runtimes/python.yaml"))
                .unwrap();
        let plan = crate::planning::CandidatePythonPlan {
            version: 1,
            config: serde_json::from_value(runtime["execution_config"].clone()).unwrap(),
            runner: "runtime".into(),
            entry: "tests/owned.py".into(),
            timeout_ms: 30_000,
            recipe: Some(serde_json::from_value(serde_json::json!({
                "tools":[{"name":"tool","binaries":["tool"],"version_pattern":"(?<version>1)","primary":true}],
                "launch":{"command":["{tool_tool}","{source_file}"]}
            })).unwrap()),
        };
        spec.invocation.argv = crate::planning::candidate_python_argv(platform, &plan).unwrap();
        spec
    }

    #[test]
    fn combined_candidate_controls_roundtrip_exact_frame_and_application_input() {
        for platform in [LaunchPlatform::LinuxAmd64, LaunchPlatform::LinuxArm64] {
            let mut spec = combined_candidate(platform);
            let public = public_digest(&spec).unwrap();
            crate::candidate_prep::bind_challenge(&mut spec, None).unwrap();
            crate::recipe_timeout::bind_challenge(&mut spec, None).unwrap();
            validate_invocation(&spec).unwrap();
            assert_eq!(public_digest(&spec).unwrap(), public);
            for input in [b"".as_slice(), br#"{"owned":"exact original input"}"#] {
                let mut frame = crate::runner_protocol::encode_start(&spec.invocation).unwrap();
                frame.extend_from_slice(input);
                let mut cursor = Cursor::new(frame);
                assert_eq!(
                    crate::runner_protocol::read_start(&mut cursor).unwrap(),
                    spec.invocation
                );
                let mut remaining = Vec::new();
                cursor.read_to_end(&mut remaining).unwrap();
                assert_eq!(remaining, input);
            }
        }
    }

    #[test]
    fn combined_candidate_timeout_control_requires_exact_plan_and_process_policy() {
        for mutation in 0..7 {
            let mut spec = combined_candidate(LaunchPlatform::LinuxAmd64);
            crate::candidate_prep::bind_challenge(&mut spec, None).unwrap();
            crate::recipe_timeout::bind_challenge(&mut spec, None).unwrap();
            match mutation {
                0 => {
                    let mut plan = crate::planning::CandidatePythonPlan::from_arguments(
                        &spec.invocation.argv[2..],
                    )
                    .unwrap();
                    plan.runner = "pytest".into();
                    plan.recipe = None;
                    spec.invocation.argv =
                        crate::planning::candidate_python_argv(spec.platform, &plan).unwrap();
                }
                1 => spec.invocation.argv.truncate(2),
                2 => spec.purpose = LaunchPurpose::Action,
                3 => spec.pack_access = PackAccess::ReadOnly,
                4 => {
                    spec.inputs.pop().unwrap();
                }
                5 => spec.invocation.argv[0] = "/opt/attune/pack/forged-helper".into(),
                _ => spec.invocation.working_root = LaunchRoot::Scratch,
            }
            assert!(validate_invocation(&spec).is_err());
        }
    }
}
