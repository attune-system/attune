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
