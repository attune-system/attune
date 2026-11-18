//! The trusted action producer consumes the real prepared context and release
//! inventory. No local executable discovery, filesystem path or raw host plan
//! participates in image/command selection.
use super::*;
use attune_common::{
    pack_format::{Artifact, FileEntry, FileMode, Manifest, NativeArch},
    repositories::{execution_input::SelectedExecutionInput, workload_launch::RuntimeStageBinding},
    workload_launch::{
        FrozenContextRef, LaunchInvocation, LaunchManifest, LaunchPlatform, LaunchPurpose,
        LaunchRoot, LaunchSpec, LaunchTarget, PackAccess, ReservedLaunch, PACK_ROOT,
    },
};
use attune_worker::{executor::ControlExecutionPreparation, runtime::launch_planning};
use std::collections::BTreeMap;

/// Runtime inputs are supplied by trusted CP publication selection, never by a
/// host request. The ledger rechecks their settled Setup/image/engine/build
/// proof. A missing required runtime blocks preparation; no host fallback.
pub async fn freeze_action_launch(
    state: Arc<AppState>,
    source: &ClaimedActionSource,
    prepared: ControlExecutionPreparation,
    runtime: Option<(PinnedLaunchInput, RuntimeStageBinding)>,
) -> ApiResult<ReservedLaunch> {
    let context = prepared.context;
    let snapshot = prepared.input_snapshot.ok_or_else(unavailable)?;
    let execution = match source.target {
        LaunchTarget::Execution { execution, .. } => execution,
        _ => return Err(unavailable()),
    };
    if context.execution_id != execution
        || execution != source.execution.id
        || !context.secrets.is_empty()
        || context.code_path.is_none()
    {
        return Err(unavailable());
    }
    let release_pin = &source
        .execution
        .executable_snapshot
        .as_ref()
        .ok_or_else(unavailable)?
        .release;
    let release = PackReleaseRepository::find_by_id(&state.db, release_pin.id)
        .await?
        .ok_or_else(unavailable)?;
    if release.digest != release_pin.digest {
        return Err(unavailable());
    }
    let manifest: Manifest = serde_json::from_value(release.manifest).map_err(|_| unavailable())?;
    let action = &source
        .execution
        .executable_snapshot
        .as_ref()
        .ok_or_else(unavailable)?
        .executable
        .action;
    if manifest.pack.r#ref != action.pack_ref || context.action_ref != action.r#ref {
        return Err(unavailable());
    }
    let entry = format!("actions/{}", context.entry_point);
    let mut selected: BTreeMap<_, _> = context.env.into_iter().collect();
    // The preparer validates snapshot-selected overrides. Recheck reserved
    // context protection at the final merge before the context is frozen.
    for (key, value) in context.execution_env {
        attune_common::execution_env::validate_execution_env_var(&key, &value)?;
        selected.insert(key, value);
    }
    let (argv, env) = if let Some(config) = context
        .runtime_config_override
        .as_ref()
        .filter(|config| !config.interpreter.binary.is_empty())
    {
        require_file(&manifest.files, &entry, false)?;
        let argv = launch_planning::interpreted_argv(config, &entry, runtime.is_some())
            .map_err(|_| unavailable())?;
        let env = launch_planning::action_environment(config, &selected, &entry, runtime.is_some())
            .map_err(|_| unavailable())?;
        (argv, env)
    } else {
        if context.runtime_name.as_ref().is_some_and(|name| {
            attune_common::runtime_detection::normalize_runtime_name(name) != "native"
        }) || runtime.is_some()
        {
            return Err(unavailable());
        }
        if let Some(config) = &context.runtime_config_override {
            if launch_planning::needs_runtime(config) || !config.interpreter.args.is_empty() {
                return Err(unavailable());
            }
            selected = launch_planning::action_environment(config, &selected, &entry, false)
                .map_err(|_| unavailable())?;
        } else {
            selected
                .entry("PATH".into())
                .or_insert_with(|| "/usr/local/bin:/usr/bin:/bin".into());
        }
        (
            vec![native_program(
                &manifest,
                &action.r#ref,
                &entry,
                source.profile.engine.platform,
            )?],
            selected,
        )
    };
    let timeout_ms = context
        .timeout
        .and_then(|seconds| seconds.checked_mul(1000))
        .filter(|value| *value > 0)
        .ok_or_else(unavailable)?;
    let pack = materialize_action_pack(state.clone(), source).await?;
    let mut inputs = vec![pack];
    let expected_runtime = if let Some((input, binding)) = &runtime {
        if input.kind != LaunchInputKind::Runtime {
            return Err(unavailable());
        }
        let config = context
            .runtime_config_override
            .as_ref()
            .ok_or_else(unavailable)?;
        if binding.runtime_definition_sha256
            != Sha256Digest::of_bytes(&serde_json::to_vec(config).map_err(|_| unavailable())?)
        {
            return Err(unavailable());
        }
        inputs.push(input.clone());
        Some(binding)
    } else {
        None
    };
    let selected_input = SelectedExecutionInput::new(
        serde_json::to_value(context.parameters).map_err(|_| unavailable())?,
        snapshot,
    )?;
    let spec = LaunchSpec {
        manifest: LaunchManifest {
            // Ledger allocates these identities and replaces the placeholder
            // context with a randomized authenticated ciphertext reference.
            launch_id: 1,
            purpose: LaunchPurpose::Action,
            target: source.target.clone(),
            owner_identity_id: source.owner,
            engine_id: source.profile.engine.engine_id,
            engine_name_nonce: uuid::Uuid::new_v4(),
            image: source.profile.image.clone(),
            context: FrozenContextRef {
                id: 1,
                version: 1,
                ciphertext_sha256: Sha256Digest::of_bytes(&[]),
            },
            inputs,
            pack_access: PackAccess::ReadOnly,
            stage: None,
            timeout_ms: Some(timeout_ms),
            lease_deadline: chrono::Utc::now() + chrono::Duration::minutes(1),
        },
        invocation: LaunchInvocation {
            argv,
            env,
            working_root: LaunchRoot::Pack,
        },
    };
    Ledger::freeze_selected_execution(
        &state.db,
        source.admission,
        source.image,
        spec,
        state
            .config
            .security
            .encryption_key
            .as_deref()
            .ok_or_else(unavailable)?,
        expected_runtime,
        selected_input,
    )
    .await
    .map_err(Into::into)
}

fn require_file(files: &[FileEntry], path: &str, executable: bool) -> ApiResult<()> {
    attune_common::artifact_transport::ValidatedRelativePath::new(path)?;
    let mut matches = files.iter().filter(|file| file.path == path);
    let file = matches.next().ok_or_else(unavailable)?;
    if matches.next().is_some() || executable && file.mode != FileMode::Executable {
        return Err(unavailable());
    }
    Ok(())
}

fn native_program(
    manifest: &Manifest,
    action: &str,
    entry: &str,
    platform: LaunchPlatform,
) -> ApiResult<String> {
    let mut declarations = manifest.artifacts.iter().filter(|artifact| {
        artifact
            .component_refs()
            .iter()
            .any(|component| component == action)
    });
    let declaration = declarations.next();
    let path = if let Some(Artifact::Native { variants, .. }) = declaration {
        if declarations.next().is_some() {
            return Err(unavailable());
        }
        let arch = match platform {
            LaunchPlatform::LinuxAmd64 => NativeArch::Amd64,
            LaunchPlatform::LinuxArm64 => NativeArch::Arm64,
        };
        let mut variants = variants
            .iter()
            .filter(|variant| variant.target.arch == arch);
        let selected = variants.next().ok_or_else(unavailable)?;
        if variants.next().is_some() {
            return Err(unavailable());
        }
        selected.path.clone()
    } else if declaration.is_some() {
        return Err(unavailable());
    } else if manifest.files.iter().any(|file| file.path == entry) {
        entry.into()
    } else {
        format!("{entry}/linux-{}", platform.native_arch())
    };
    require_file(&manifest.files, &path, true)?;
    Ok(format!("{PACK_ROOT}/{path}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn manifest() -> Manifest {
        serde_json::from_value(serde_json::json!({
            "schema":"attune.pack.release/v1", "pack":{"ref":"owned","version":"1.0.0"},
            "requires":{"attune":"*","platform_catalog":"*","runtimes":[]},
            "dependencies":{}, "files":[], "artifacts":[], "evidence":{"sboms":[],"provenance":[]}
        }))
        .unwrap()
    }
    fn executable(path: &str) -> FileEntry {
        FileEntry {
            path: path.into(),
            size: 1,
            sha256: "00".repeat(32),
            mode: FileMode::Executable,
        }
    }
    #[test]
    fn native_selection_uses_admitted_engine_platform_and_pinned_executable_inventory() {
        let mut manifest = manifest();
        manifest.files = vec![
            executable("actions/bin/tool/linux-amd64"),
            executable("actions/bin/tool/linux-arm64"),
        ];
        for (platform, arch) in [
            (LaunchPlatform::LinuxAmd64, "amd64"),
            (LaunchPlatform::LinuxArm64, "arm64"),
        ] {
            assert_eq!(
                native_program(&manifest, "owned.action", "actions/bin/tool", platform).unwrap(),
                format!("{PACK_ROOT}/actions/bin/tool/linux-{arch}")
            );
        }
        manifest.files.pop();
        assert!(native_program(
            &manifest,
            "owned.action",
            "actions/bin/tool",
            LaunchPlatform::LinuxArm64
        )
        .is_err());
        manifest.files[0].mode = FileMode::Data;
        assert!(native_program(
            &manifest,
            "owned.action",
            "actions/bin/tool",
            LaunchPlatform::LinuxAmd64
        )
        .is_err());
        manifest.files = vec![executable("../outside")];
        assert!(native_program(
            &manifest,
            "owned.action",
            "../outside",
            LaunchPlatform::LinuxAmd64
        )
        .is_err());
    }
    #[test]
    fn canonical_artifact_binding_cannot_fall_through_to_an_unselected_entrypoint() {
        let mut manifest = manifest();
        let pinned = ".attune/artifacts/tool/linux-arm64-static";
        manifest.files = vec![
            executable(pinned),
            executable("actions/bin/tool/linux-amd64"),
        ];
        manifest.artifacts = serde_json::from_value(serde_json::json!([{
            "kind":"native","id":"tool","component_refs":["owned.action"],
            "variants":[{"id":"linux-arm64-static","path":pinned,"target":{"os":"linux","arch":"arm64","libc":"static"}}]
        }])).unwrap();
        assert_eq!(
            native_program(
                &manifest,
                "owned.action",
                "actions/bin/tool",
                LaunchPlatform::LinuxArm64
            )
            .unwrap(),
            format!("{PACK_ROOT}/{pinned}")
        );
        assert!(native_program(
            &manifest,
            "owned.action",
            "actions/bin/tool",
            LaunchPlatform::LinuxAmd64
        )
        .is_err());
        manifest.artifacts.push(manifest.artifacts[0].clone());
        assert!(native_program(
            &manifest,
            "owned.action",
            "actions/bin/tool",
            LaunchPlatform::LinuxArm64
        )
        .is_err());
    }
}
