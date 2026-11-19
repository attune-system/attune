//! Shared trusted-loop runtime discovery through the closed native backend.
//! Callers supply an installed input, not source authority: current release,
//! manifest, runtime metadata, repository epochs and exact worker incarnation
//! are verified here. No action/execution object or manager-local probe is used.
use attune_common::{
    child_process_environment::ChildProcessEnvironment,
    isolated_process::{ProcessControl, ProcessSpec, ResourceIdentity},
    models::{PackRelease, Runtime, RuntimeVersion},
    repositories::{
        runtime_probe_process::{
            RuntimeProbeProcessRepository as Probe, RuntimeProbeSource, VerifiedToolMap,
        },
        FindById,
    },
    workload_config::{WorkloadConfig, WorkloadStorageLimits},
    workload_launch::*,
    Error, Result,
};
use attune_workload_backend::configuration::ConfiguredBackend;
use sqlx::PgPool;
use std::{collections::BTreeSet, sync::Arc};
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

fn denied() -> Error {
    Error::PermissionDenied("Native runtime discovery unavailable".into())
}

/// One reusable driver retained by the existing worker/sensor loop. The caller
/// owns registration, heartbeat, captured admission policy and input acquisition.
pub struct NativeRuntimeProbeDriver<'a> {
    pool: PgPool,
    worker: i64,
    incarnation: Uuid,
    config: &'a WorkloadConfig,
    configured: &'a ConfiguredBackend,
    master: &'a str,
    child_environment: &'a ChildProcessEnvironment,
}
impl<'a> NativeRuntimeProbeDriver<'a> {
    pub fn new(
        pool: &PgPool,
        worker: i64,
        incarnation: Uuid,
        config: &'a WorkloadConfig,
        configured: &'a ConfiguredBackend,
        master: &'a str,
        child_environment: &'a ChildProcessEnvironment,
    ) -> Result<Self> {
        config.validate()?;
        if worker <= 0
            || incarnation.is_nil()
            || master.is_empty()
            || configured.runner != config.helper
        {
            return Err(denied());
        }
        Ok(Self {
            pool: pool.clone(),
            worker,
            incarnation,
            config,
            configured,
            master,
            child_environment,
        })
    }

    /// Returns captured tools (an empty map for verified legacy availability).
    /// This does not publish capabilities or grant action completion authority.
    pub async fn verify(
        &self,
        runtime: &Runtime,
        version: Option<&RuntimeVersion>,
        admitted: &BTreeSet<String>,
        pack_pin: &PinnedLaunchInput,
    ) -> Result<VerifiedToolMap> {
        let native = self.configured;
        let backend = &native.backend;
        let master = self.master;
        loop {
            let debt = Probe::unsettled(&self.pool, self.worker, master).await?;
            if debt.is_empty() {
                break;
            }
            for (old, submitted, retained) in debt {
                if !submitted {
                    Probe::request_stop(&self.pool, old.intent).await?;
                    Probe::acknowledge_removed(&self.pool, old.intent, None).await?;
                    continue;
                }
                let newly_observed = retained.is_none();
                let resource = match retained {
                    Some(resource) => resource,
                    None => backend.launcher().prepare(&old,attune_common::isolated_process::ProcessCreateDisposition::ReconcileOnly).await?,
                };
                // A blocked DB fence cannot delay stopping an exact known writer.
                // Settlement/removal/replacement still require BOTH acknowledgements.
                let fence = async {
                    if newly_observed {
                        Probe::record_resource(&self.pool, old.intent, &resource).await?;
                    }
                    Probe::request_stop(&self.pool, old.intent).await
                };
                let (fenced, stopped) = tokio::join!(
                    fence,
                    backend.stop_retained(&old, &resource, LaunchStopReason::SourceRevoked)
                );
                fenced?;
                let exit = stopped?;
                Probe::record_exit(&self.pool, old.intent, &resource, exit).await?;
                backend.remove_stopped(&old, &resource).await?;
                Probe::acknowledge_removed(&self.pool, old.intent, Some(&resource)).await?;
            }
        }
        let config = version
            .map(|v| &v.execution_config)
            .unwrap_or(&runtime.execution_config);
        if let Some(version) = version {
            attune_common::runtime_lifecycle::validate_version_mode(
                &runtime.execution_config,
                &version.execution_config,
            )
            .map_err(|_| denied())?;
        }
        let recipe = admitted_recipe(config, self.child_environment, admitted)?;
        let definition = attune_common::runtime_detection::NativeRuntimeVerification {
            registered_version: version.is_some(),
            execution_config: config.clone(),
            distributions: version
                .map(|v| &v.distributions)
                .unwrap_or(&runtime.distributions)
                .clone(),
        };
        let pack = attune_common::repositories::pack::PackRepository::find_by_id(
            &self.pool,
            runtime.pack.ok_or_else(denied)?,
        )
        .await?
        .ok_or_else(denied)?;
        let release = attune_common::repositories::PackReleaseRepository::find_by_id(
            &self.pool,
            pack.active_release.ok_or_else(denied)?,
        )
        .await?
        .ok_or_else(denied)?;
        if release.pack != pack.id || release.pack_ref != pack.r#ref {
            return Err(denied());
        }
        let source = RuntimeProbeSource {
            worker: self.worker,
            incarnation: self.incarnation,
            runtime: runtime.id,
            runtime_version: version.map(|v| v.id),
            runtime_version_string: version.map(|v| v.version.clone()),
            provider_pack: pack.id,
            release: release.id,
            release_digest: release.digest.clone(),
            runtime_config: runtime.execution_config.clone(),
            version_config: version.map(|v| v.execution_config.clone()),
            runtime_distributions: runtime.distributions.clone(),
            version_distributions: version.map(|v| v.distributions.clone()),
        };
        validate_release_pin(&release, pack_pin)?;
        // Verify only the installed immutable ZIP, never a mutable unpacked
        // pack tree. Blocking archive work must not starve trusted heartbeats.
        let cache = native.cache.clone();
        let pin = pack_pin.clone();
        let retained = release.clone();
        let selected = source.clone();
        let runtime_ref = runtime.r#ref.clone();
        let limits = self.config.storage;
        tokio::task::spawn_blocking(move || {
            let mut archive = cache.open_input_archive(&pin).map_err(|_| denied())?;
            verify_release_archive(&mut archive, &retained, &pin, limits)?;
            crate::runtime::launch_planning::validate_runtime_release_metadata(
                archive,
                &runtime_ref,
                &selected.runtime_config,
                selected
                    .runtime_version_string
                    .as_deref()
                    .zip(selected.version_config.as_ref()),
                Some((
                    &selected.runtime_distributions,
                    selected.version_distributions.as_ref(),
                )),
            )
            .map_err(|_| denied())
        })
        .await
        .map_err(|_| denied())??;
        let attempt = uuid::Uuid::new_v4();
        let spec = ProcessSpec {
            intent: uuid::Uuid::new_v4(),
            purpose: LaunchPurpose::RuntimeProbe,
            // The repository replaces this with its genuine retained admission.
            target: LaunchTarget::RuntimeProbe {
                admission: runtime.id,
                attempt,
            },
            image: self.config.image.clone(),
            platform: self.config.platform,
            inputs: vec![pack_pin.clone(), native.runner.clone()],
            pack_access: PackAccess::ReadOnly,
            stage: None,
            timeout_ms: Some(120000),
            invocation: LaunchInvocation {
                argv: match &recipe {
                    Some(recipe) => crate::runtime::launch_planning::runtime_tool_probe_argv(
                        self.config.platform,
                        recipe,
                        "/usr/local/bin:/usr/bin:/bin",
                    ),
                    None => crate::runtime::launch_planning::runtime_verification_argv(
                        self.config.platform,
                        &definition,
                        "/usr/local/bin:/usr/bin:/bin",
                    ),
                }
                .map_err(|_| denied())?,
                env: Default::default(),
                working_root: LaunchRoot::Pack,
            },
        };
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(120);
        let spec = Probe::reserve(&self.pool, attempt, source, spec, master).await?;
        let launcher = backend.launcher();
        let disposition = Probe::begin_create(&self.pool, spec.intent, master).await?;
        let resource = tokio::time::timeout_at(deadline, launcher.prepare(&spec, disposition))
            .await
            .map_err(|_| denied())??;
        Probe::record_resource(&self.pool, spec.intent, &resource).await?;
        let mut process = tokio::time::timeout_at(deadline, launcher.start(&spec, &resource))
            .await
            .map_err(|_| denied())??;
        if process.resource != resource {
            return Err(denied());
        }
        process.control = Arc::new(RecordingProbeControl {
            inner: process.control,
            pool: self.pool.clone(),
            intent: spec.intent,
            resource: resource.clone(),
        });
        let stdin = Probe::consume_stdin(&self.pool, spec.intent, &resource, master).await?;
        let cancel = CancellationToken::new();
        let future = crate::runtime::process_executor::execute_isolated_cancellable(
            process,
            stdin,
            crate::runtime::process_executor::IsolatedCaptureOptions {
                timeout_secs: Some(120),
                max_stdout_bytes: crate::runtime::launch_planning::RUNTIME_TOOL_RESULT_BYTES,
                max_stderr_bytes: 16384,
                output_format: attune_common::models::OutputFormat::Text,
                output_policy: Default::default(),
                out_schema: None,
                cancel_token: Some(cancel.clone()),
                stdout_log_writer: None,
                stderr_log_writer: None,
            },
        );
        tokio::pin!(future);
        let mut check = tokio::time::interval(std::time::Duration::from_secs(1));
        let result = loop {
            tokio::select! {
                result=&mut future=>break result.map_err(|_|denied())?,
            _=tokio::time::sleep_until(deadline)=>{cancel.cancel();let _=(&mut future).await;return Err(denied());},
                _=check.tick()=>if Probe::validate_source(&self.pool,spec.intent,master).await.is_err(){cancel.cancel();let _=(&mut future).await;return Err(denied());}
            }
        };
        if result.exit_code != 0
            || result.error.is_some()
            || result.logs_incomplete
            || result.timed_out
            || result.stdout_truncated
            || result.stderr_truncated
        {
            return Err(denied());
        }
        let tools = serde_json::from_str(&result.stdout).map_err(|_| denied())?;
        Probe::complete(&self.pool, spec.intent, &resource, &tools, master).await?;
        // Positive exact removal follows captured receipt, never inferred 404.
        Probe::request_stop(&self.pool, spec.intent).await?;
        backend.remove_stopped(&spec, &resource).await?;
        Probe::acknowledge_removed(&self.pool, spec.intent, Some(&resource)).await?;
        Probe::tools(&self.pool, spec.intent, master).await
    }
}

struct RecordingProbeControl {
    inner: Arc<dyn ProcessControl>,
    pool: PgPool,
    intent: Uuid,
    resource: ResourceIdentity,
}
#[async_trait::async_trait]
impl ProcessControl for RecordingProbeControl {
    async fn write_stdin_once(
        &self,
        payload: attune_common::workload_stdin::LaunchStdinPayload,
    ) -> Result<()> {
        self.inner.write_stdin_once(payload).await
    }
    async fn wait(&self) -> Result<LaunchExitOutcome> {
        let outcome = self.inner.wait().await?;
        Probe::record_exit(&self.pool, self.intent, &self.resource, outcome).await?;
        Ok(outcome)
    }
    async fn stop(&self, reason: LaunchStopReason) -> Result<LaunchExitOutcome> {
        let (fenced, stopped) = tokio::join!(
            Probe::request_stop(&self.pool, self.intent),
            self.inner.stop(reason)
        );
        let outcome = stopped?;
        let recorded = Probe::record_exit(&self.pool, self.intent, &self.resource, outcome).await;
        fenced?;
        recorded?;
        Ok(outcome)
    }
}
fn admitted_recipe(
    config: &serde_json::Value,
    environment: &ChildProcessEnvironment,
    admitted: &BTreeSet<String>,
) -> Result<Option<attune_common::runtime_lifecycle::RuntimeRecipe>> {
    let recipe = config
        .get("lifecycle")
        .cloned()
        .map(serde_json::from_value::<attune_common::runtime_lifecycle::RuntimeRecipe>)
        .transpose()
        .map_err(|_| denied())?;
    if let Some(recipe) = &recipe {
        let digest = recipe.digest().map_err(|_| denied())?;
        if !admitted.contains(&digest) || !environment.admits_native_runtime_recipe(&digest) {
            return Err(denied());
        }
    }
    Ok(recipe)
}

fn validate_release_pin(release: &PackRelease, pin: &PinnedLaunchInput) -> Result<()> {
    if pin.kind != LaunchInputKind::Pack
        || pin.object_id != release.id
        || pin.object_version != release.digest
        || release.id <= 0
        || Sha256Digest::parse(&release.digest).is_err()
    {
        return Err(denied());
    }
    Ok(())
}

/// Match one primary runtime-v1 inventory. A caller-supplied pin/cache receipt
/// authenticates ZIP bytes, not their release authority; the retained immutable
/// manifest must independently match every regular file, including executability.
fn verify_release_archive(
    archive: &mut std::fs::File,
    release: &PackRelease,
    pin: &PinnedLaunchInput,
    limits: WorkloadStorageLimits,
) -> Result<()> {
    use attune_common::{
        pack_registry::storage::PackReleaseManifest,
        workload_archive::{self, ArchiveEntryKind, ArchivePolicy},
    };
    use sha2::{Digest, Sha256};
    use std::{
        collections::BTreeMap,
        io::{Read, Seek, SeekFrom},
    };
    validate_release_pin(release, pin)?;
    if pin.size_bytes > limits.compressed_bytes
        || pin.max_expanded_bytes > limits.expanded_bytes
        || pin.max_files > limits.entries
    {
        return Err(denied());
    }
    let manifest: PackReleaseManifest =
        serde_json::from_value(release.manifest.clone()).map_err(|_| denied())?;
    if manifest.format_version != 1
        || manifest.pack_ref != release.pack_ref
        || manifest.version != release.version
    {
        return Err(denied());
    }
    let policy = ArchivePolicy {
        root: LaunchInputKind::Pack,
        max_compressed_bytes: pin.size_bytes,
        max_expanded_bytes: pin.max_expanded_bytes,
        max_files: pin.max_files,
    };
    let inventory = workload_archive::validate_zip(archive, &policy).map_err(|_| denied())?;
    if manifest.files.len() != inventory.entries.len() {
        return Err(denied());
    }
    let mut expected = BTreeMap::new();
    for file in &manifest.files {
        Sha256Digest::parse(&file.sha256).map_err(|_| denied())?;
        if expected.insert(file.path.as_str(), file).is_some() {
            return Err(denied());
        }
    }
    for entry in &inventory.entries {
        let file = expected.get(entry.name.as_str()).ok_or_else(denied)?;
        if entry.size_bytes != file.size
            || entry.kind
                != (ArchiveEntryKind::File {
                    executable: file.executable,
                })
        {
            return Err(denied());
        }
    }
    {
        let mut zip = zip::ZipArchive::new(&mut *archive).map_err(|_| denied())?;
        for file in &manifest.files {
            let mut entry = zip.by_name(&file.path).map_err(|_| denied())?;
            let mut hash = Sha256::new();
            let mut bytes = 0u64;
            let mut buffer = [0u8; 64 * 1024];
            loop {
                let count = entry.read(&mut buffer).map_err(|_| denied())?;
                if count == 0 {
                    break;
                }
                bytes = bytes
                    .checked_add(count as u64)
                    .filter(|bytes| *bytes <= file.size)
                    .ok_or_else(denied)?;
                hash.update(&buffer[..count]);
            }
            let hash = hash
                .finalize()
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>();
            if bytes != file.size || hash != file.sha256 {
                return Err(denied());
            }
        }
    }
    archive.seek(SeekFrom::Start(0)).map_err(|_| denied())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use attune_common::{
        pack_registry::storage::{PackReleaseFile, PackReleaseManifest},
        workload_config::WorkloadStorageLimits,
        workload_sandbox::WorkloadSandbox,
    };
    use std::io::{Read, Seek, SeekFrom, Write};

    fn limits() -> WorkloadStorageLimits {
        WorkloadStorageLimits {
            compressed_bytes: 1024 * 1024,
            expanded_bytes: 1024 * 1024,
            entries: 32,
            artifacts: Default::default(),
        }
    }
    fn archive(script: &[u8], executable: bool) -> std::fs::File {
        let mut file = tempfile::tempfile().unwrap();
        {
            let mut zip = zip::ZipWriter::new(&mut file);
            for (path, bytes, exec) in [
                (
                    "runtimes/owned.yaml",
                    b"ref: owned.tool\nexecution_config: {}\n".as_slice(),
                    false,
                ),
                ("tools/probe", script, executable),
            ] {
                zip.start_file(
                    path,
                    zip::write::SimpleFileOptions::default().unix_permissions(if exec {
                        0o755
                    } else {
                        0o644
                    }),
                )
                .unwrap();
                zip.write_all(bytes).unwrap();
            }
            zip.finish().unwrap();
        }
        file.seek(SeekFrom::Start(0)).unwrap();
        file
    }
    fn fixture() -> (PackRelease, PinnedLaunchInput, std::fs::File) {
        let body = b"owned source executable";
        let files = [
            (
                "runtimes/owned.yaml",
                b"ref: owned.tool\nexecution_config: {}\n".as_slice(),
                false,
            ),
            ("tools/probe", body.as_slice(), true),
        ]
        .into_iter()
        .map(|(path, body, executable)| PackReleaseFile {
            path: path.into(),
            size: body.len() as u64,
            sha256: Sha256Digest::of_bytes(body).as_str().into(),
            executable,
        })
        .collect();
        let manifest = PackReleaseManifest {
            format_version: 1,
            pack_ref: "owned".into(),
            version: "1.0.0".into(),
            files,
        };
        let release = PackRelease {
            id: 42,
            pack: 7,
            pack_ref: "owned".into(),
            version: "1.0.0".into(),
            digest: "a".repeat(64),
            object_key: Some("owned-object".into()),
            provider_version: Some("owned-version".into()),
            content_path: "/owned/retained".into(),
            archive_size: 1,
            manifest: serde_json::to_value(manifest).unwrap(),
            created: chrono::Utc::now(),
            inactive_since: None,
        };
        let mut file = archive(body, true);
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes).unwrap();
        file.seek(SeekFrom::Start(0)).unwrap();
        let pin = PinnedLaunchInput {
            pin_id: 2,
            kind: LaunchInputKind::Pack,
            object_id: release.id,
            object_version: release.digest.clone(),
            size_bytes: bytes.len() as u64,
            sha256: Sha256Digest::of_bytes(&bytes),
            max_expanded_bytes: 1024,
            max_files: 32,
        };
        (release, pin, file)
    }
    #[test]
    fn exact_primary_release_inventory_is_required_not_an_asserted_pin() {
        let (release, pin, mut file) = fixture();
        verify_release_archive(&mut file, &release, &pin, limits()).unwrap();
        for change in 0..7 {
            let mut wrong = release.clone();
            match change {
                0 => wrong.id += 1,
                1 => wrong.digest = "b".repeat(64),
                2 => wrong.version = "2.0.0".into(),
                3 => wrong.manifest["files"][1]["sha256"] = serde_json::json!("c".repeat(64)),
                4 => wrong.manifest["files"][1]["size"] = serde_json::json!(1),
                5 => wrong.manifest["files"][1]["executable"] = serde_json::json!(false),
                _ => wrong.manifest["files"][1]["path"] = serde_json::json!("tools/other"),
            }
            file.seek(SeekFrom::Start(0)).unwrap();
            assert!(
                verify_release_archive(&mut file, &wrong, &pin, limits()).is_err(),
                "mutation{change}"
            );
        }
    }
    #[test]
    fn copied_runtime_yaml_cannot_authorize_substituted_tool_bytes() {
        let (release, mut pin, _) = fixture();
        let mut forged = archive(b"evil! source executable", true);
        let mut bytes = Vec::new();
        forged.read_to_end(&mut bytes).unwrap();
        forged.seek(SeekFrom::Start(0)).unwrap();
        // Even a self-consistent installed ZIP pin with the genuine release IDs
        // and exact runtime YAML cannot replace another file in that release.
        pin.sha256 = Sha256Digest::of_bytes(&bytes);
        pin.size_bytes = bytes.len() as u64;
        assert!(verify_release_archive(&mut forged, &release, &pin, limits()).is_err());
    }
    #[test]
    fn extra_or_missing_inventory_and_wrong_pin_kind_are_rejected() {
        let (release, mut pin, mut file) = fixture();
        pin.kind = LaunchInputKind::Runtime;
        assert!(verify_release_archive(&mut file, &release, &pin, limits()).is_err());
        pin.kind = LaunchInputKind::Pack;
        for extra in [true, false] {
            let mut wrong = release.clone();
            let files = wrong.manifest["files"].as_array_mut().unwrap();
            if extra {
                files.push(files[0].clone());
            } else {
                files.pop();
            }
            file.seek(SeekFrom::Start(0)).unwrap();
            assert!(verify_release_archive(&mut file, &wrong, &pin, limits()).is_err());
        }
    }
    #[test]
    fn caller_admission_never_replaces_captured_native_recipe_policy() {
        let recipe:attune_common::runtime_lifecycle::RuntimeRecipe=serde_json::from_value(serde_json::json!({"tools":[{"name":"owned","binaries":["owned"],"primary":true,"version_pattern":"(?<version>1)"}],"launch":{"command":["{tool_owned}"]}})).unwrap();
        let digest = recipe.digest().unwrap();
        let config = serde_json::json!({"lifecycle":recipe});
        let admitted = BTreeSet::from([digest.clone()]);
        let captured = ChildProcessEnvironment::from_parent(
            [("ATTUNE_RUNTIME_RECIPE_DIGESTS".into(), digest.into())],
            &[],
        )
        .unwrap();
        assert!(admitted_recipe(&config, &captured, &admitted).is_err());
        let native = captured.with_workload_sandbox(WorkloadSandbox::NativeEffectsRequired);
        assert!(admitted_recipe(&config, &native, &BTreeSet::new()).is_err());
        assert!(admitted_recipe(&config, &native, &admitted)
            .unwrap()
            .is_some());
        assert!(
            admitted_recipe(&serde_json::json!({}), &native, &BTreeSet::new())
                .unwrap()
                .is_none()
        );
    }
}
