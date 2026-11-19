//! Shared trusted-loop runtime discovery through the closed native backend.
//! Callers supply an installed input, not source authority: current release,
//! manifest, runtime metadata, repository epochs and exact worker incarnation
//! are verified here. No action/execution object or manager-local probe is used.
use attune_common::{
    child_process_environment::ChildProcessEnvironment,
    isolated_process::{ProcessControl, ProcessIo, ProcessSpec, ResourceIdentity},
    models::{PackRelease, Runtime, RuntimeVersion},
    repositories::{
        runtime_probe_process::{
            RuntimeProbeProcessRepository as Probe, RuntimeProbeSource, VerifiedToolMap,
        },
        FindById,
    },
    workload_config::{WorkloadConfig, WorkloadStorageLimits},
    workload_launch::*,
    workload_stdin::LaunchStdinPayload,
    Error, Result,
};
use attune_workload_backend::configuration::ConfiguredBackend;
use sqlx::PgPool;
use std::{
    collections::BTreeSet,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};
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
    /// Shutdown signals `cancel` and retains this future through its join; a
    /// deadline closes launch admission without abandoning a pending ACK/fence.
    pub async fn verify(
        &self,
        runtime: &Runtime,
        version: Option<&RuntimeVersion>,
        admitted: &BTreeSet<String>,
        pack_pin: &PinnedLaunchInput,
        cancel: Option<&CancellationToken>,
    ) -> Result<VerifiedToolMap> {
        let cancel = cancel.cloned().unwrap_or_default().child_token();
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
        let start_submitted = AtomicBool::new(false);
        let options = NativePreparationOptions {
            cancel: &cancel,
            start_submitted: &start_submitted,
            deadline: Some(deadline),
            stdout_bytes: crate::runtime::launch_planning::RUNTIME_TOOL_RESULT_BYTES,
            stderr_bytes: 16384,
        };
        options.check_open()?;
        let spec = Probe::reserve(&self.pool, attempt, source, spec, master).await?;
        let (observed, resource_watch) = tokio::sync::watch::channel(None);
        let current = &spec;
        let operation = async move {
            options.check_open()?;
            let disposition = Probe::begin_create(&self.pool, current.intent, master).await?;
            options.check_open()?;
            let launcher = backend.launcher();
            let resource = launcher.prepare(current, disposition).await?;
            observed.send_replace(Some(resource.clone()));
            Probe::record_resource(&self.pool, current.intent, &resource).await?;
            options.check_open()?;
            options.start_submitted.store(true, Ordering::Release);
            let mut process = launcher.start(current, &resource).await?;
            if process.resource != resource {
                let _ = stop_prepared_process(process, options).await;
                return Err(denied());
            }
            process.control = Arc::new(RecordingProbeControl {
                inner: process.control,
                pool: self.pool.clone(),
                intent: current.intent,
                resource: resource.clone(),
            });
            if options.check_open().is_err() {
                let _ = stop_prepared_process(process, options).await;
                return Err(denied());
            }
            match Probe::consume_stdin(&self.pool, current.intent, &resource, master).await {
                Ok(stdin) => Ok((process, stdin)),
                Err(error) => {
                    let _ = stop_prepared_process(process, options).await;
                    Err(error)
                }
            }
        };
        let (process, stdin) =
            await_native_preparation(
                operation,
                resource_watch,
                options,
                Probe::request_stop(&self.pool, spec.intent),
                |resource, reason| async move {
                    backend.stop_retained(current, &resource, reason).await
                },
                |resource, exit| async move {
                    Probe::record_exit(&self.pool, current.intent, &resource, exit).await
                },
            )
            .await?;
        let resource = process.resource.clone();
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
        let mut check = tokio::time::interval(std::time::Duration::from_secs(1));
        let source_checks = async {
            loop {
                check.tick().await;
                Probe::validate_source(&self.pool, spec.intent, master).await?;
            }
        };
        let result = await_probe_execution(future, source_checks, deadline, &cancel).await?;
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

/// The owning service must retain this future through shutdown. Cancellation
/// closes local admission, but does not dispose an in-flight provider/DB future.
#[derive(Clone, Copy)]
pub(crate) struct NativePreparationOptions<'a> {
    pub cancel: &'a CancellationToken,
    // Monotonic local fact selector, never provider submission/absence evidence.
    pub start_submitted: &'a AtomicBool,
    pub deadline: Option<tokio::time::Instant>,
    pub stdout_bytes: usize,
    pub stderr_bytes: usize,
}
impl NativePreparationOptions<'_> {
    pub(crate) fn check_open(&self) -> Result<()> {
        if self.cancel.is_cancelled()
            || self
                .deadline
                .is_some_and(|d| d <= tokio::time::Instant::now())
        {
            self.cancel.cancel();
            return Err(denied());
        }
        Ok(())
    }
}

pub(crate) async fn stop_prepared_process(
    process: ProcessIo,
    options: NativePreparationOptions<'_>,
) -> crate::runtime::RuntimeResult<crate::runtime::ExecutionResult> {
    crate::runtime::process_executor::stop_isolated(
        process,
        crate::runtime::process_executor::IsolatedCaptureOptions {
            timeout_secs: None,
            max_stdout_bytes: options.stdout_bytes,
            max_stderr_bytes: options.stderr_bytes,
            output_format: attune_common::models::OutputFormat::Text,
            output_policy: Default::default(),
            out_schema: None,
            cancel_token: Some(options.cancel.clone()),
            stdout_log_writer: None,
            stderr_log_writer: None,
        },
    )
    .await
}

// Domain repositories remain with each producer; only ownership of their
// existing operations is shared. No new resource can be selected by this helper.
pub(crate) async fn await_native_preparation<Stop, StopFuture, Record, RecordFuture>(
    operation: impl std::future::Future<Output = Result<(ProcessIo, LaunchStdinPayload)>>,
    mut observed: tokio::sync::watch::Receiver<Option<ResourceIdentity>>,
    options: NativePreparationOptions<'_>,
    fence: impl std::future::Future<Output = Result<()>>,
    stop: Stop,
    record: Record,
) -> Result<(ProcessIo, LaunchStdinPayload)>
where
    Stop: FnOnce(ResourceIdentity, LaunchStopReason) -> StopFuture,
    StopFuture: std::future::Future<Output = Result<LaunchExitOutcome>>,
    Record: FnOnce(ResourceIdentity, LaunchExitOutcome) -> RecordFuture,
    RecordFuture: std::future::Future<Output = Result<()>>,
{
    let mut operation = Box::pin(operation);
    let completed = tokio::select! {
        biased;
        _ = options.cancel.cancelled() => None,
        _ = async {
            match options.deadline {
                Some(deadline) => tokio::time::sleep_until(deadline).await,
                None => std::future::pending().await,
            }
        } => None,
        result = &mut operation => Some(result),
    };
    let completed = match completed {
        Some(Ok(prepared)) if options.check_open().is_ok() => return Ok(prepared),
        other => other,
    };
    let reason = if options
        .deadline
        .is_some_and(|d| d <= tokio::time::Instant::now())
    {
        LaunchStopReason::Timeout
    } else if options.cancel.is_cancelled() {
        LaunchStopReason::Cancelled
    } else {
        LaunchStopReason::LaunchFailure
    };
    options.cancel.cancel();
    let original = async {
        let result = match completed {
            Some(result) => result,
            None => operation.as_mut().await,
        };
        // It has completed, so release its observation sender even if the
        // future's completed state retains captures until destruction.
        drop(operation);
        if let Ok((process, _unused_payload)) = result {
            // Includes a late Start/consume ACK after an earlier retained Stop.
            // The actual returned control and both captures still have owners.
            let _ = stop_prepared_process(process, options).await;
        }
    };
    let physical = async {
        loop {
            let resource = observed.borrow_and_update().clone();
            if let Some(resource) = resource {
                let exit = stop(resource.clone(), reason).await?;
                return Ok::<_, Error>(Some((resource, exit)));
            }
            if observed.changed().await.is_err() {
                // No exact ACK is not absence. The submitted ledger stays debt.
                return Ok(None);
            }
        }
    };
    // Keep all three futures live. In particular, a row lock cannot prevent
    // physical Stop, and a late provider ACK cannot escape the original owner.
    let (_, fenced, stopped) = tokio::join!(original, fence, physical);
    if let Ok(Some((resource, exit))) = stopped {
        // Resource recording has now joined; this is only a factual exit record,
        // never a capture receipt, removal or publication acknowledgement.
        if !options.start_submitted.load(Ordering::Acquire) {
            let recorded = record(resource, exit).await;
            fenced?;
            recorded?;
        } else {
            // A prior retained Stop cannot stand in for a late attachment's
            // Stop. Its Recording*Control alone records that positive exit.
            fenced?;
        }
    } else {
        fenced?;
        stopped?;
    }
    Err(denied())
}

// A pending source query must not suspend execution's deadline/cancellation polling.
async fn await_probe_execution(
    execution: impl std::future::Future<
        Output = crate::runtime::RuntimeResult<crate::runtime::ExecutionResult>,
    >,
    source_checks: impl std::future::Future<Output = Result<()>>,
    deadline: tokio::time::Instant,
    cancel: &CancellationToken,
) -> Result<crate::runtime::ExecutionResult> {
    tokio::pin!(execution);
    let completed = {
        tokio::pin!(source_checks);
        tokio::select! {
            result = &mut execution => Some(result),
            _ = tokio::time::sleep_until(deadline) => None,
            _ = &mut source_checks => None,
        }
    };
    match completed {
        Some(result) => result.map_err(|_| denied()),
        None => {
            // Drop the pending source query before joining durable Stop/capture.
            // Cancellation alone never proves either has settled.
            cancel.cancel();
            let _ = execution.await;
            Err(denied())
        }
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

#[cfg(test)]
pub(crate) mod preparation_tests {
    use super::*;
    use std::{
        pin::Pin,
        sync::atomic::{AtomicBool, AtomicUsize, Ordering},
        task::{Context, Poll},
        time::Duration,
    };
    use tokio::{
        io::{AsyncRead, ReadBuf},
        sync::{mpsc, oneshot},
    };

    pub(crate) struct StopRequest {
        pub reason: LaunchStopReason,
        pub ack: oneshot::Sender<bool>,
    }
    pub(crate) struct Control {
        pub writes: AtomicUsize,
        pub readers_dropped: Arc<AtomicUsize>,
        stops: mpsc::UnboundedSender<StopRequest>,
    }
    #[async_trait::async_trait]
    impl ProcessControl for Control {
        async fn write_stdin_once(&self, _: LaunchStdinPayload) -> Result<()> {
            self.writes.fetch_add(1, Ordering::SeqCst);
            Err(denied())
        }
        async fn wait(&self) -> Result<LaunchExitOutcome> {
            panic!("a closed startup gate must not enter invocation wait")
        }
        async fn stop(&self, reason: LaunchStopReason) -> Result<LaunchExitOutcome> {
            let (ack, joined) = oneshot::channel();
            self.stops.send(StopRequest { reason, ack }).unwrap();
            if !joined.await.unwrap() {
                return Err(Error::execution("Uncertain exact Stop acknowledgement"));
            }
            Ok(LaunchExitOutcome {
                exit_code: 0,
                oom_killed: false,
            })
        }
    }
    struct PendingReader(Arc<AtomicUsize>);
    impl AsyncRead for PendingReader {
        fn poll_read(
            self: Pin<&mut Self>,
            _: &mut Context<'_>,
            _: &mut ReadBuf<'_>,
        ) -> Poll<std::io::Result<()>> {
            Poll::Pending
        }
    }
    impl Drop for PendingReader {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }
    pub(crate) fn control_fixture() -> (
        ResourceIdentity,
        Arc<Control>,
        mpsc::UnboundedReceiver<StopRequest>,
    ) {
        let (stops, receiver) = mpsc::unbounded_channel();
        let resource = ResourceIdentity::Kubernetes {
            cluster: "owned-fixture".into(),
            namespace: "owned".into(),
            pod_name: "preparing-native-process".into(),
            pod_uid: Uuid::new_v4().to_string(),
        };
        (
            resource,
            Arc::new(Control {
                writes: AtomicUsize::new(0),
                readers_dropped: Arc::new(AtomicUsize::new(0)),
                stops,
            }),
            receiver,
        )
    }
    pub(crate) fn process(resource: ResourceIdentity, control: Arc<Control>) -> ProcessIo {
        ProcessIo {
            resource,
            stdout: Box::pin(PendingReader(control.readers_dropped.clone())),
            stderr: Box::pin(PendingReader(control.readers_dropped.clone())),
            control,
        }
    }
    pub(crate) async fn next_stop(
        receiver: &mut mpsc::UnboundedReceiver<StopRequest>,
    ) -> StopRequest {
        tokio::time::timeout(Duration::from_secs(1), receiver.recv())
            .await
            .unwrap()
            .unwrap()
    }
    #[derive(Clone, Copy, PartialEq)]
    enum BlockedPhase {
        Record,
        Start,
        Consume,
    }

    async fn blocked_phase(phase: BlockedPhase, stopped_fact: bool) {
        let (resource, control, mut stops) = control_fixture();
        let cancel = CancellationToken::new();
        let child = cancel.child_token();
        let (blocked, waiting) = oneshot::channel();
        let (operation_ack, operation_wait) = oneshot::channel();
        let (fencing, fence_waiting) = oneshot::channel();
        let (fence_ack, fence_wait) = oneshot::channel();
        let original_joined = Arc::new(AtomicBool::new(false));
        let fence_joined = Arc::new(AtomicBool::new(false));
        let exit_recorded = Arc::new(AtomicBool::new(false));
        let starts = Arc::new(AtomicUsize::new(0));
        let task = {
            let control = control.clone();
            let original_joined = original_joined.clone();
            let fence_joined = fence_joined.clone();
            let exit_recorded = exit_recorded.clone();
            let starts = starts.clone();
            tokio::spawn(async move {
                let start_submitted = AtomicBool::new(false);
                let options = NativePreparationOptions {
                    cancel: &child,
                    start_submitted: &start_submitted,
                    deadline: None,
                    stdout_bytes: 1024,
                    stderr_bytes: 1024,
                };
                let (observed, receiver) = tokio::sync::watch::channel(None);
                let source_resource = resource.clone();
                let source_control = control.clone();
                let joined = original_joined.clone();
                let operation = async move {
                    let result = async {
                        observed.send_replace(Some(source_resource.clone()));
                        let attached = if phase == BlockedPhase::Consume {
                            options.start_submitted.store(true, Ordering::Release);
                            starts.fetch_add(1, Ordering::SeqCst);
                            Some(process(source_resource.clone(), source_control.clone()))
                        } else {
                            None
                        };
                        if phase == BlockedPhase::Start {
                            options.start_submitted.store(true, Ordering::Release);
                            starts.fetch_add(1, Ordering::SeqCst);
                        }
                        blocked.send(()).unwrap();
                        operation_wait.await.unwrap();
                        if phase == BlockedPhase::Record {
                            options.check_open()?;
                            starts.fetch_add(1, Ordering::SeqCst);
                        }
                        let process =
                            attached.unwrap_or_else(|| process(source_resource, source_control));
                        Ok((
                            process,
                            LaunchStdinPayload::from_bytes(b"{}".to_vec()).unwrap(),
                        ))
                    }
                    .await;
                    joined.store(true, Ordering::SeqCst);
                    result
                };
                let physical = control.clone();
                let factual_control = control.clone();
                let recorded_fence = fence_joined.clone();
                await_native_preparation(
                    operation,
                    receiver,
                    options,
                    async {
                        fencing.send(()).unwrap();
                        fence_wait.await.unwrap();
                        fence_joined.store(true, Ordering::SeqCst);
                        Ok(())
                    },
                    move |exact, reason| async move {
                        assert_eq!(exact, resource);
                        physical.stop(reason).await
                    },
                    move |_, _| async move {
                        assert!(original_joined.load(Ordering::SeqCst));
                        assert!(recorded_fence.load(Ordering::SeqCst));
                        if phase != BlockedPhase::Record {
                            assert_eq!(factual_control.readers_dropped.load(Ordering::SeqCst), 2);
                        }
                        exit_recorded.store(true, Ordering::SeqCst);
                        Ok(())
                    },
                )
                .await
            })
        };
        supervision_tests::signal(waiting).await;
        cancel.cancel();
        supervision_tests::signal(fence_waiting).await;
        let stop = next_stop(&mut stops).await;
        assert_eq!(stop.reason, LaunchStopReason::Cancelled);
        assert!(!original_joined.load(Ordering::SeqCst));
        assert!(!fence_joined.load(Ordering::SeqCst));
        assert!(
            !task.is_finished(),
            "physical Stop must not drop the original DB/provider future"
        );
        stop.ack.send(stopped_fact).unwrap();
        operation_ack.send(()).unwrap();
        if phase != BlockedPhase::Record {
            // A late Start/consume result still owns its actual control/streams,
            // independently of both the first Stop ACK and the blocked fence.
            let late = next_stop(&mut stops).await;
            assert_eq!(late.reason, LaunchStopReason::Cancelled);
            late.ack.send(false).unwrap();
        }
        assert!(!task.is_finished());
        fence_ack.send(()).unwrap();
        assert!(task.await.unwrap().is_err());
        assert_eq!(control.writes.load(Ordering::SeqCst), 0);
        assert_eq!(
            starts.load(Ordering::SeqCst),
            usize::from(phase != BlockedPhase::Record)
        );
        assert_eq!(
            exit_recorded.load(Ordering::SeqCst),
            stopped_fact && phase == BlockedPhase::Record
        );
        assert_eq!(
            control.readers_dropped.load(Ordering::SeqCst),
            if phase == BlockedPhase::Record { 0 } else { 2 }
        );
    }

    #[tokio::test(start_paused = true)]
    async fn known_resource_stops_while_resource_record_and_fence_are_blocked() {
        blocked_phase(BlockedPhase::Record, true).await;
    }
    #[tokio::test(start_paused = true)]
    async fn late_start_ack_joins_control_and_both_captures_before_durable_fence() {
        blocked_phase(BlockedPhase::Start, true).await;
    }
    #[tokio::test(start_paused = true)]
    async fn blocked_stdin_consumption_stops_without_delivery_or_replay() {
        blocked_phase(BlockedPhase::Consume, true).await;
    }
    #[tokio::test(start_paused = true)]
    async fn uncertain_known_stop_retains_debt_without_recording_an_exit() {
        blocked_phase(BlockedPhase::Record, false).await;
    }

    async fn late_prepare(known_ack: bool) {
        let (resource, control, mut stops) = control_fixture();
        let expected_resource = resource.clone();
        let (preparing, preparation_waiting) = oneshot::channel();
        let (prepare_ack, prepare_wait) = oneshot::channel::<Result<ResourceIdentity>>();
        let (record_ack, record_wait) = oneshot::channel();
        let (fencing, fence_waiting) = oneshot::channel();
        let (fence_ack, fence_wait) = oneshot::channel();
        let starts = Arc::new(AtomicUsize::new(0));
        let recorded = Arc::new(AtomicBool::new(false));
        let task = {
            let starts = starts.clone();
            let recorded = recorded.clone();
            let control = control.clone();
            tokio::spawn(async move {
                let cancel = CancellationToken::new();
                let start_submitted = AtomicBool::new(false);
                let options = NativePreparationOptions {
                    cancel: &cancel,
                    start_submitted: &start_submitted,
                    deadline: Some(tokio::time::Instant::now() + Duration::from_secs(120)),
                    stdout_bytes: 1024,
                    stderr_bytes: 1024,
                };
                let (observed, receiver) = tokio::sync::watch::channel(None);
                let operation = async move {
                    preparing.send(()).unwrap();
                    let resource = prepare_wait.await.unwrap()?;
                    observed.send_replace(Some(resource));
                    record_wait.await.unwrap();
                    options.check_open()?;
                    starts.fetch_add(1, Ordering::SeqCst);
                    Err(denied())
                };
                await_native_preparation(
                    operation,
                    receiver,
                    options,
                    async {
                        fencing.send(()).unwrap();
                        fence_wait.await.unwrap();
                        Ok(())
                    },
                    move |resource, reason| async move {
                        assert_eq!(resource, expected_resource);
                        control.stop(reason).await
                    },
                    move |_, _| async move {
                        recorded.store(true, Ordering::SeqCst);
                        Ok(())
                    },
                )
                .await
            })
        };
        supervision_tests::signal(preparation_waiting).await;
        tokio::time::advance(Duration::from_secs(120)).await;
        supervision_tests::signal(fence_waiting).await;
        assert!(matches!(
            stops.try_recv(),
            Err(mpsc::error::TryRecvError::Empty)
        ));
        assert!(
            !task.is_finished(),
            "timeout must retain the submitted create future"
        );
        if known_ack {
            prepare_ack.send(Ok(resource)).unwrap();
            let stopped = next_stop(&mut stops).await;
            assert_eq!(stopped.reason, LaunchStopReason::Timeout);
            assert!(
                !task.is_finished(),
                "late ACK must retain blocked resource recording"
            );
            stopped.ack.send(true).unwrap();
            record_ack.send(()).unwrap();
        } else {
            prepare_ack
                .send(Err(Error::execution("Unknown create acknowledgement")))
                .unwrap();
            drop(record_ack);
        }
        fence_ack.send(()).unwrap();
        assert!(task.await.unwrap().is_err());
        assert_eq!(starts.load(Ordering::SeqCst), 0);
        assert_eq!(control.writes.load(Ordering::SeqCst), 0);
        assert_eq!(recorded.load(Ordering::SeqCst), known_ack);
    }

    #[tokio::test(start_paused = true)]
    async fn late_prepare_ack_after_deadline_is_stopped_without_starting() {
        late_prepare(true).await;
    }
    #[tokio::test(start_paused = true)]
    async fn unknown_create_after_deadline_does_not_invent_resource_or_exit() {
        late_prepare(false).await;
    }

    #[tokio::test(start_paused = true)]
    async fn uninterrupted_preparation_returns_exact_input_without_opening_gate() {
        let (resource, control, mut stops) = control_fixture();
        let expected = b"{\"private\":\"unchanged\"}";
        let token = CancellationToken::new();
        let start_submitted = AtomicBool::new(false);
        let options = NativePreparationOptions {
            cancel: &token,
            start_submitted: &start_submitted,
            deadline: None,
            stdout_bytes: 1024,
            stderr_bytes: 1024,
        };
        let (observed, receiver) = tokio::sync::watch::channel(None);
        let attached = process(resource.clone(), control.clone());
        let prepared = await_native_preparation(
            async move {
                observed.send_replace(Some(resource));
                Ok((
                    attached,
                    LaunchStdinPayload::from_bytes(expected.to_vec()).unwrap(),
                ))
            },
            receiver,
            options,
            async { panic!("ready preparation must not fence Stop") },
            |_, _| async { panic!("ready preparation must not stop") },
            |_, _| async { panic!("ready preparation must not record an exit") },
        )
        .await
        .unwrap();
        assert_eq!(prepared.1.as_bytes(), expected);
        assert_eq!(control.writes.load(Ordering::SeqCst), 0);
        let cleanup = tokio::spawn(async move {
            let token = CancellationToken::new();
            let start_submitted = AtomicBool::new(false);
            stop_prepared_process(
                prepared.0,
                NativePreparationOptions {
                    cancel: &token,
                    start_submitted: &start_submitted,
                    deadline: None,
                    stdout_bytes: 1024,
                    stderr_bytes: 1024,
                },
            )
            .await
        });
        next_stop(&mut stops).await.ack.send(true).unwrap();
        assert!(!cleanup.await.unwrap().unwrap().is_success());
        assert_eq!(control.readers_dropped.load(Ordering::SeqCst), 2);
        assert_eq!(control.writes.load(Ordering::SeqCst), 0);
    }
}

// The setup tests reuse this process double; both supervise the real isolated
// executor, with physical Stop observable before its durable acknowledgement.
#[cfg(test)]
pub(crate) mod supervision_tests {
    use super::*;
    use attune_common::{isolated_process::ProcessIo, workload_stdin::LaunchStdinPayload};
    use std::{
        sync::{
            atomic::{AtomicUsize, Ordering},
            Mutex,
        },
        time::Duration,
    };
    use tokio::sync::oneshot;

    struct Control {
        waiting: Mutex<Option<oneshot::Sender<()>>>,
        stopped: Mutex<Option<oneshot::Sender<LaunchStopReason>>>,
        stop_ack: Mutex<Option<oneshot::Receiver<bool>>>,
        writes: Arc<AtomicUsize>,
    }

    #[async_trait::async_trait]
    impl ProcessControl for Control {
        async fn write_stdin_once(&self, payload: LaunchStdinPayload) -> Result<()> {
            assert_eq!(payload.as_bytes(), b"{}");
            assert_eq!(self.writes.fetch_add(1, Ordering::SeqCst), 0);
            Ok(())
        }

        async fn wait(&self) -> Result<LaunchExitOutcome> {
            self.waiting
                .lock()
                .unwrap()
                .take()
                .unwrap()
                .send(())
                .unwrap();
            std::future::pending().await
        }

        async fn stop(&self, reason: LaunchStopReason) -> Result<LaunchExitOutcome> {
            self.stopped
                .lock()
                .unwrap()
                .take()
                .unwrap()
                .send(reason)
                .unwrap();
            let ack = self.stop_ack.lock().unwrap().take().unwrap();
            if !ack.await.unwrap() {
                return Err(Error::execution("Uncertain stopped-writer acknowledgement"));
            }
            Ok(LaunchExitOutcome {
                exit_code: 0,
                oom_killed: false,
            })
        }
    }

    pub(crate) struct PendingProcess {
        pub process: ProcessIo,
        pub waiting: oneshot::Receiver<()>,
        pub stopped: oneshot::Receiver<LaunchStopReason>,
        pub stop_ack: oneshot::Sender<bool>,
        pub writes: Arc<AtomicUsize>,
    }

    pub(crate) fn pending_process() -> PendingProcess {
        let (waiting_tx, waiting) = oneshot::channel();
        let (stopped_tx, stopped) = oneshot::channel();
        let (stop_ack, ack) = oneshot::channel();
        let writes = Arc::new(AtomicUsize::new(0));
        PendingProcess {
            process: ProcessIo {
                resource: ResourceIdentity::Kubernetes {
                    cluster: "owned-fixture".into(),
                    namespace: "owned".into(),
                    pod_name: "pending-native-process".into(),
                    pod_uid: Uuid::new_v4().to_string(),
                },
                stdout: Box::pin(std::io::Cursor::new(Vec::<u8>::new())),
                stderr: Box::pin(std::io::Cursor::new(Vec::<u8>::new())),
                control: Arc::new(Control {
                    waiting: Mutex::new(Some(waiting_tx)),
                    stopped: Mutex::new(Some(stopped_tx)),
                    stop_ack: Mutex::new(Some(ack)),
                    writes: writes.clone(),
                }),
            },
            waiting,
            stopped,
            stop_ack,
            writes,
        }
    }

    pub(crate) async fn execute(
        process: ProcessIo,
        cancel: CancellationToken,
        timeout_secs: Option<u64>,
    ) -> crate::runtime::RuntimeResult<crate::runtime::ExecutionResult> {
        crate::runtime::process_executor::execute_isolated_cancellable(
            process,
            LaunchStdinPayload::from_bytes(b"{}".to_vec()).unwrap(),
            crate::runtime::process_executor::IsolatedCaptureOptions {
                timeout_secs,
                max_stdout_bytes: 1024,
                max_stderr_bytes: 1024,
                output_format: attune_common::models::OutputFormat::Text,
                output_policy: Default::default(),
                out_schema: None,
                cancel_token: Some(cancel),
                stdout_log_writer: None,
                stderr_log_writer: None,
            },
        )
        .await
    }

    pub(crate) async fn signal<T>(receiver: oneshot::Receiver<T>) -> T {
        tokio::time::timeout(Duration::from_secs(1), receiver)
            .await
            .expect("supervision stopped polling while a database check was pending")
            .expect("supervisor dropped without settling")
    }

    #[tokio::test(start_paused = true)]
    async fn pending_source_check_does_not_suspend_probe_deadline_or_stop_join() {
        let PendingProcess {
            process,
            waiting,
            stopped,
            stop_ack,
            writes,
        } = pending_process();
        let cancel = CancellationToken::new();
        let (checking_tx, checking) = oneshot::channel();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(120);
        // The absolute probe budget includes reserve/create/start time.
        tokio::time::advance(Duration::from_secs(60)).await;
        let task = tokio::spawn(async move {
            let checks = async {
                checking_tx.send(()).unwrap();
                std::future::pending::<Result<()>>().await
            };
            await_probe_execution(
                execute(process, cancel.clone(), Some(120)),
                checks,
                deadline,
                &cancel,
            )
            .await
        });
        signal(checking).await;
        signal(waiting).await;
        tokio::time::advance(Duration::from_secs(60)).await;
        assert_eq!(signal(stopped).await, LaunchStopReason::Cancelled);
        assert!(
            !task.is_finished(),
            "Stop/capture must remain joined until acknowledged"
        );
        stop_ack.send(true).unwrap();
        assert!(task.await.unwrap().is_err());
        assert_eq!(writes.load(Ordering::SeqCst), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn failed_source_check_cancels_probe_and_waits_for_uncertain_stop() {
        let PendingProcess {
            process,
            waiting,
            stopped,
            stop_ack,
            writes,
        } = pending_process();
        let cancel = CancellationToken::new();
        let (revoke, revoked) = oneshot::channel();
        let task = tokio::spawn(async move {
            let checks = async {
                revoked.await.unwrap();
                Err(denied())
            };
            await_probe_execution(
                execute(process, cancel.clone(), Some(120)),
                checks,
                tokio::time::Instant::now() + Duration::from_secs(120),
                &cancel,
            )
            .await
        });
        signal(waiting).await;
        revoke.send(()).unwrap();
        assert_eq!(signal(stopped).await, LaunchStopReason::Cancelled);
        assert!(!task.is_finished());
        stop_ack.send(false).unwrap();
        assert!(task.await.unwrap().is_err());
        assert_eq!(writes.load(Ordering::SeqCst), 1);
    }
}
