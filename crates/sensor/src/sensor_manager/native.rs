//! Native effects remain inside the existing trusted sensor lifecycle.
use super::*;
use attune_common::Error;
use attune_common::{
    isolated_process::{ProcessCreateDisposition, ProcessSpec, ResourceIdentity},
    repositories::sensor_workload::SensorWorkloadRepository as Ledger,
    workload_launch::*,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Context {
    intent: uuid::Uuid,
    target: LaunchTarget,
    image: PinnedImage,
    platform: LaunchPlatform,
    inputs: Vec<PinnedLaunchInput>,
    invocation: LaunchInvocation,
    bootstrap_json: String,
}
impl Context {
    fn spec(&self) -> ProcessSpec {
        ProcessSpec {
            intent: self.intent,
            purpose: LaunchPurpose::Sensor,
            target: self.target.clone(),
            image: self.image.clone(),
            platform: self.platform,
            inputs: self.inputs.clone(),
            invocation: self.invocation.clone(),
            pack_access: PackAccess::ReadOnly,
            stage: None,
            timeout_ms: None,
        }
    }
}

pub(super) async fn await_startup<T>(
    startup: impl Future<Output = Result<T>>,
    stopping: CancellationToken,
    native: bool,
    deadline: Option<tokio::time::Instant>,
    sensor_ref: &str,
) -> Result<T> {
    let bounded = async {
        if let Some(deadline) = deadline {
            match tokio::time::timeout_at(deadline, startup).await {
                Ok(result) => result,
                Err(_) => Err(anyhow!("Sensor {sensor_ref} startup timed out")),
            }
        } else {
            startup.await
        }
    };
    tokio::select! {
        biased;
        _ = stopping.cancelled(), if native => Err(anyhow!("Sensor {sensor_ref} startup cancelled by manager shutdown")),
        result = bounded => result,
    }
}

/// The original managed-start stage budget. Nested setup/discovery jobs keep
/// running under the manager's trackers after a caller drops its await, while
/// the same cancellation signal requests their owned Stop/capture joins.
#[derive(Clone)]
pub(super) struct NativeStartupStage {
    tasks: tokio_util::task::TaskTracker,
    attempt: tokio_util::task::TaskTracker,
    stopping: CancellationToken,
    deadline: tokio::time::Instant,
    failed: Arc<AtomicBool>,
}
impl NativeStartupStage {
    pub(super) fn new(
        tasks: tokio_util::task::TaskTracker,
        attempt: tokio_util::task::TaskTracker,
        stopping: CancellationToken,
        deadline: tokio::time::Instant,
        failed: Arc<AtomicBool>,
    ) -> Self {
        Self {
            tasks,
            attempt,
            stopping,
            deadline,
            failed,
        }
    }
    /// Seconds-only installer API, bounded independently by the exact stage
    /// deadline below. Rounding does not grant a new stage budget.
    fn remaining_seconds(&self) -> Result<u64> {
        let remaining = self
            .deadline
            .checked_duration_since(tokio::time::Instant::now())
            .filter(|remaining| !remaining.is_zero())
            .ok_or_else(|| anyhow!("Native sensor startup deadline elapsed"))?;
        Ok(remaining.as_secs() + u64::from(remaining.subsec_nanos() != 0))
    }
    async fn run<T, F>(
        &self,
        operation: impl FnOnce(CancellationToken) -> F + Send + 'static,
    ) -> Result<T>
    where
        T: Send + 'static,
        F: Future<Output = Result<T>> + Send + 'static,
    {
        self.remaining_seconds()?;
        if self.stopping.is_cancelled() {
            return Err(anyhow!("Native sensor startup cancelled"));
        }
        let cancel = self.stopping.child_token();
        // Dropping this caller signals the retained operation; it never aborts
        // the task that still owns prepare/start/Stop/capture acknowledgements.
        let _cancel_on_drop = cancel.clone().drop_guard();
        let deadline = self.deadline;
        let failed = self.failed.clone();
        let interrupted_failed = failed.clone();
        let task = self.tasks.spawn(self.attempt.track_future(async move {
            use futures::FutureExt;
            let result = std::panic::AssertUnwindSafe(async move {
                if cancel.is_cancelled() || tokio::time::Instant::now() >= deadline {
                    return Err(anyhow!(
                        "Native sensor startup interrupted before operation"
                    ));
                }
                let operation = operation(cancel.clone());
                tokio::pin!(operation);
                let completed = tokio::select! {
                    biased;
                    _ = cancel.cancelled() => None,
                    _ = tokio::time::sleep_until(deadline) => None,
                    result = &mut operation => Some(result),
                };
                match completed {
                    Some(result)
                        if cancel.is_cancelled() || tokio::time::Instant::now() >= deadline =>
                    {
                        if result.is_err() {
                            interrupted_failed.store(true, Ordering::Release);
                        }
                        cancel.cancel();
                        Err(anyhow!(
                            "Native sensor startup closed before accepting operation result"
                        ))
                    }
                    Some(result) => result,
                    None => {
                        cancel.cancel();
                        // Durable DB fencing may still be pending. Keep the
                        // original future, physical Stop and captures owned.
                        if operation.await.is_err() {
                            // A tracker join is not clean Stop/capture evidence.
                            // Preserve uncertainty even if the caller is gone.
                            interrupted_failed.store(true, Ordering::Release);
                        }
                        Err(anyhow!(
                            "Native sensor startup interrupted; operation joined"
                        ))
                    }
                }
            })
            .catch_unwind()
            .await;
            match result {
                Ok(result) => result,
                Err(_) => {
                    failed.store(true, Ordering::Release);
                    Err(anyhow!(
                        "Native sensor startup operation panicked; evidence retained"
                    ))
                }
            }
        }));
        task.await
            .map_err(|_| anyhow!("Native sensor startup operation join failed"))?
    }
}

/// Cleanup is queued synchronously on cancellation; the manager owns the task
/// through durable fencing, physical stop and every stream/waiter join.
struct NativeStartupCleanup {
    tasks: tokio_util::task::TaskTracker,
    attempt: tokio_util::task::TaskTracker,
    failed: Arc<AtomicBool>,
    cleanup: std::sync::Mutex<Option<futures::future::BoxFuture<'static, Result<()>>>>,
}
impl NativeStartupCleanup {
    fn new(
        tasks: tokio_util::task::TaskTracker,
        attempt: tokio_util::task::TaskTracker,
        failed: Arc<AtomicBool>,
        cleanup: impl Future<Output = Result<()>> + Send + 'static,
    ) -> Self {
        Self {
            tasks,
            attempt,
            failed,
            cleanup: std::sync::Mutex::new(Some(Box::pin(cleanup))),
        }
    }
    fn queue(&mut self) -> Option<JoinHandle<Result<()>>> {
        use futures::FutureExt;
        let cleanup = self
            .cleanup
            .get_mut()
            .expect("native cleanup owner")
            .take()?;
        let failed = self.failed.clone();
        Some(self.tasks.spawn(self.attempt.track_future(async move {
            let result = std::panic::AssertUnwindSafe(cleanup)
                .catch_unwind()
                .await
                .unwrap_or_else(|_| {
                    Err(anyhow!("Native sensor cleanup panicked; evidence retained"))
                });
            if result.is_err() {
                failed.store(true, Ordering::Release);
                warn!("Native sensor cleanup incomplete; exact evidence retained");
            }
            result
        })))
    }
    async fn stop(&mut self) -> Result<()> {
        self.queue()
            .ok_or_else(|| anyhow!("Native sensor cleanup already owned"))?
            .await?
    }
}
impl Drop for NativeStartupCleanup {
    fn drop(&mut self) {
        let _ = self.queue();
    }
}

struct NativeSensorContext {
    pool: PgPool,
    workload: Id,
    spec: ProcessSpec,
    resource: std::sync::OnceLock<ResourceIdentity>,
    backend: attune_workload_backend::configuration::NativeBackend,
    transport: Arc<dyn ArtifactFileTransport>,
    worker: Id,
    incarnation: uuid::Uuid,
}
#[derive(Default)]
struct NativeProcess {
    control: Option<Arc<dyn attune_common::isolated_process::ProcessControl>>,
    wait_task: Option<JoinHandle<attune_common::Result<LaunchExitOutcome>>>,
    stdout: Option<JoinHandle<()>>,
    stderr: Option<JoinHandle<()>>,
}
pub(super) struct NativeSensor {
    context: Arc<NativeSensorContext>,
    // Only synchronous transfers hold this lock, never IO or database work.
    process: Arc<std::sync::Mutex<NativeProcess>>,
    cleanup: NativeStartupCleanup,
}
impl NativeSensor {
    fn new(
        context: NativeSensorContext,
        tasks: tokio_util::task::TaskTracker,
        attempt: tokio_util::task::TaskTracker,
        failed: Arc<AtomicBool>,
    ) -> Self {
        let context = Arc::new(context);
        let process = Arc::new(std::sync::Mutex::new(NativeProcess::default()));
        let (cleanup_context, cleanup_process) = (context.clone(), process.clone());
        let cleanup = NativeStartupCleanup::new(tasks, attempt, failed, async move {
            let process =
                std::mem::take(&mut *cleanup_process.lock().expect("native process owner"));
            cleanup_context.stop(process).await
        });
        Self {
            context,
            process,
            cleanup,
        }
    }
    pub(super) async fn poll_exit(&mut self) -> Result<Option<LaunchExitOutcome>> {
        let task = {
            let mut process = self.process.lock().expect("native process owner");
            if !process
                .wait_task
                .as_ref()
                .is_some_and(|task| task.is_finished())
            {
                return Ok(None);
            }
            process.wait_task.take().expect("finished task")
        };
        let exit = task.await??;
        let context = &self.context;
        Ledger::record_native_exit(
            &context.pool,
            context.workload,
            context.spec.intent,
            context
                .resource
                .get()
                .ok_or_else(|| anyhow!("Native resource unconfirmed"))?,
            exit,
        )
        .await?;
        Ok(Some(exit))
    }
    pub(super) async fn settle_files(&self) -> Result<()> {
        let context = &self.context;
        settle_files(
            &context.pool,
            context.workload,
            &context.spec,
            context
                .resource
                .get()
                .ok_or_else(|| anyhow!("Native resource unconfirmed"))?,
            &context.backend,
            context.transport.as_ref(),
            context.worker,
            context.incarnation,
        )
        .await
    }
    pub(super) async fn stop(&mut self) -> Result<()> {
        self.cleanup.stop().await
    }
}
impl NativeSensorContext {
    async fn stop(&self, mut process: NativeProcess) -> Result<()> {
        let control = process.control.take();
        let stopped = async {
            let resource = match self.resource.get() {
                Some(resource) => resource.clone(),
                None => {
                    // Lost create ACK stays reconcile-only. A lookup error/404
                    // retains the intent and never authorizes recreation.
                    self.backend
                        .launcher()
                        .prepare(&self.spec, ProcessCreateDisposition::ReconcileOnly)
                        .await?
                }
            };
            let durable = async {
                Ledger::record_native_resource(
                    &self.pool,
                    self.workload,
                    self.spec.intent,
                    &resource,
                )
                .await?;
                Ledger::stop_native_effect(&self.pool, self.workload, self.spec.intent).await
            };
            let physical = async {
                if let Some(control) = control.as_ref() {
                    control.stop(LaunchStopReason::Cancelled).await
                } else {
                    self.backend
                        .stop_retained(&self.spec, &resource, LaunchStopReason::Cancelled)
                        .await
                }
            };
            let exit =
                attune_common::repositories::sensor_workload::native_effect::join_sensor_stop(
                    durable, physical,
                )
                .await?;
            Ledger::record_native_exit(
                &self.pool,
                self.workload,
                self.spec.intent,
                &resource,
                exit,
            )
            .await?;
            Ok(())
        };
        join_native_startup_stop(
            stopped,
            &mut process.wait_task,
            &mut process.stdout,
            &mut process.stderr,
        )
        .await
    }
}
async fn join_native_startup_stop(
    stopped: impl Future<Output = Result<()>>,
    waiter: &mut Option<JoinHandle<attune_common::Result<LaunchExitOutcome>>>,
    stdout: &mut Option<JoinHandle<()>>,
    stderr: &mut Option<JoinHandle<()>>,
) -> Result<()> {
    let (stopped, joined, stdout, stderr) = tokio::join!(
        stopped,
        join_owned_native_waiter(waiter),
        drain_sensor_capture(stdout, Duration::from_secs(30)),
        drain_sensor_capture(stderr, Duration::from_secs(30)),
    );
    // No early return abandons another stream or waiter. Aborted captures are
    // incomplete; only the capture writer itself can record an EOF receipt.
    stopped?;
    joined?;
    stdout?;
    stderr
}

async fn join_owned_native_waiter(
    waiter: &mut Option<JoinHandle<attune_common::Result<LaunchExitOutcome>>>,
) -> Result<()> {
    if let Some(task) = waiter.take() {
        task.abort();
        match task.await {
            Ok(_) => {}
            Err(error) if error.is_cancelled() => {}
            Err(error) => return Err(anyhow!("Native sensor waiter failed: {error}")),
        }
    }
    Ok(())
}

impl SensorManager {
    pub(super) async fn reconcile_native_sensor(&self, sensor: Id) -> Result<()> {
        let Some((workload, effect)) =
            Ledger::unsettled_native_for_sensor(&self.inner.db, sensor, self.inner.worker_instance)
                .await?
        else {
            return Ok(());
        };
        if !effect.create_submitted {
            // Cleanup needs no private invocation when the durable latch proves
            // no provider call was permitted. The repository rechecks it under
            // the operation lock; a concurrent/lost create ACK stays unresolved.
            Ledger::stop_native_effect(&self.inner.db, workload, effect.intent).await?;
            return Ledger::settle_unsubmitted_native_effect(
                &self.inner.db,
                workload,
                effect.intent,
            )
            .await
            .map_err(Into::into);
        }
        let native = self
            .inner
            .native
            .as_ref()
            .ok_or_else(|| anyhow!("Native backend unavailable"))?;
        let master = self
            .inner
            .encryption_key
            .as_deref()
            .ok_or_else(|| anyhow!("Native sensor encryption unavailable"))?;
        let clear = attune_common::crypto::decrypt(&effect.encrypted_context, master)?;
        let context: Context = serde_json::from_str(&clear)?;
        let spec = context.spec();
        if spec.intent != effect.intent
            || !matches!(spec.target,LaunchTarget::Sensor{sensor:subject,workload:id,generation,..}
            if subject==sensor && id==workload && generation==effect.generation)
        {
            return Err(anyhow!("Retained sensor context binding invalid"));
        }
        let (resource, exit) = match effect.resource {
            Some(resource) => {
                let exit =
                    attune_common::repositories::sensor_workload::native_effect::join_sensor_stop(
                        Ledger::stop_native_effect(&self.inner.db, workload, spec.intent),
                        native
                            .backend
                            .stop_retained(&spec, &resource, LaunchStopReason::Shutdown),
                    )
                    .await?;
                (resource, exit)
            }
            None => {
                Ledger::stop_native_effect(&self.inner.db, workload, spec.intent).await?;
                let resource = native
                    .backend
                    .launcher()
                    .prepare(&spec, ProcessCreateDisposition::ReconcileOnly)
                    .await?;
                Ledger::record_native_resource(&self.inner.db, workload, spec.intent, &resource)
                    .await?;
                let exit = native
                    .backend
                    .stop_retained(&spec, &resource, LaunchStopReason::Shutdown)
                    .await?;
                (resource, exit)
            }
        };
        Ledger::record_native_exit(&self.inner.db, workload, spec.intent, &resource, exit).await?;
        settle_files(
            &self.inner.db,
            workload,
            &spec,
            &resource,
            &native.backend,
            self.inner
                .api_client
                .artifact_transport(self.inner.artifact_transport.base_dir())?
                .as_ref(),
            self.inner.worker_id.load(Ordering::Acquire),
            self.inner.worker_instance,
        )
        .await
    }

    async fn prepare_native_pack(
        &self,
        release: attune_common::models::PackRelease,
    ) -> Result<attune_common::workload_launch::PinnedLaunchInput> {
        let native = self
            .inner
            .native
            .as_ref()
            .ok_or_else(|| anyhow!("Native pack input unavailable"))?;
        let operator = self
            .inner
            .workload_config
            .as_ref()
            .ok_or_else(|| anyhow!("Native config unavailable"))?;
        self.inner
            .pack_transport
            .sync_release(&release.pack_ref, release.id, &release.digest)
            .await?;
        let packs = std::path::PathBuf::from(&self.inner.packs_base_dir);
        let limits = operator.storage;
        let cache = native.cache.clone();
        let pin_id = if native.runner.pin_id == 1 { 2 } else { 1 };
        tokio::task::spawn_blocking(move || {
            attune_workload_backend::pack_input::install_release(
                &cache, release, &packs, limits, pin_id,
            )
        })
        .await
        .map_err(|_| anyhow!("Native release preparation did not complete"))?
        .map_err(Into::into)
    }

    /// Source authority is verified by the same driver used by worker discovery.
    pub(crate) async fn probe_native_runtime(
        &self,
        runtime: &attune_common::models::Runtime,
        version: Option<&attune_common::models::RuntimeVersion>,
        admitted: &std::collections::BTreeSet<String>,
    ) -> attune_common::Result<attune_common::repositories::runtime_probe_process::VerifiedToolMap>
    {
        self.probe_native_runtime_cancellable(runtime, version, admitted, None)
            .await
    }

    async fn probe_native_runtime_cancellable(
        &self,
        runtime: &attune_common::models::Runtime,
        version: Option<&attune_common::models::RuntimeVersion>,
        admitted: &std::collections::BTreeSet<String>,
        cancel: Option<&CancellationToken>,
    ) -> attune_common::Result<attune_common::repositories::runtime_probe_process::VerifiedToolMap>
    {
        let denied = || Error::PermissionDenied("Native sensor discovery unavailable".into());
        let configured = self.inner.native.as_ref().ok_or_else(denied)?;
        let config = self.inner.workload_config.as_ref().ok_or_else(denied)?;
        let master = self.inner.encryption_key.as_deref().ok_or_else(denied)?;
        let pack = attune_common::repositories::PackRepository::find_by_id(
            &self.inner.db,
            runtime.pack.ok_or_else(denied)?,
        )
        .await?
        .ok_or_else(denied)?;
        let release = attune_common::repositories::PackReleaseRepository::find_by_id(
            &self.inner.db,
            pack.active_release.ok_or_else(denied)?,
        )
        .await?
        .ok_or_else(denied)?;
        let pin = self
            .prepare_native_pack(release)
            .await
            .map_err(|_| denied())?;
        attune_worker::native_runtime_probe::NativeRuntimeProbeDriver::new(
            &self.inner.db,
            self.inner.worker_id.load(Ordering::Acquire),
            self.inner.worker_instance,
            config,
            configured,
            master,
            &self.inner.child_environment,
        )?
        .verify(runtime, version, admitted, &pin, cancel)
        .await
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) async fn start_native_sensor(
        &self,
        sensor: Sensor,
        workload: OwnedSensorWorkload,
        pack_revision: PackRevision,
        definition_updated: DateTime<Utc>,
        token: String,
        token_expires_at: DateTime<Utc>,
        config: &RuntimeExecutionConfig,
        runtime: &attune_common::models::Runtime,
        selected_version: Option<&str>,
        reset: bool,
        startup_stage: NativeStartupStage,
    ) -> Result<SensorInstance> {
        let native = self
            .inner
            .native
            .as_ref()
            .ok_or_else(|| anyhow!("Native sensor backend unavailable"))?;
        let operator = self
            .inner
            .workload_config
            .as_ref()
            .ok_or_else(|| anyhow!("Native sensor config unavailable"))?;
        let master = self
            .inner
            .encryption_key
            .as_deref()
            .ok_or_else(|| anyhow!("Native sensor encryption unavailable"))?;
        let (binding_id, binding, _) =
            attune_common::repositories::sensor_schema::SensorSchemaRepository::find(
                &self.inner.db,
                workload.workload_id,
                workload.generation,
            )
            .await?
            .ok_or_else(|| anyhow!("Sensor generation binding unavailable"))?;
        let sensitive = binding.requires_protected_logs()?;
        let mut connection = self.inner.db.acquire().await?;
        let triggers=attune_common::repositories::sensor_schema::SensorSchemaRepository::restore_initial_rules(
            &mut connection,binding_id,&binding,Some(master)).await?;
        drop(connection);
        let config_value = sensor
            .config
            .clone()
            .filter(|v| !v.is_null())
            .unwrap_or_else(|| serde_json::json!({}));
        let bootstrap = encode_bootstrap(&config_value, &triggers)?;
        let release = attune_common::repositories::PackReleaseRepository::find_by_id(
            &self.inner.db,
            pack_revision.release_id,
        )
        .await?
        .ok_or_else(|| anyhow!("Sensor release unavailable"))?;
        if release.digest != pack_revision.digest {
            return Err(anyhow!("Sensor release pin changed"));
        }
        let manager = self.clone();
        let pack = startup_stage
            .run(move |_cancel| async move {
                // Retain the blocking verified-cache write through startup cancellation too.
                manager.prepare_native_pack(release).await
            })
            .await?;
        let mut inputs = vec![pack, native.runner.clone()];
        let has_runtime =
            config.lifecycle.is_none() && attune_workload_backend::planning::needs_runtime(config);
        if has_runtime {
            let version = selected_version
                .map(|selected| {
                    binding
                        .executable
                        .runtime_versions
                        .iter()
                        .find(|v| v.version == selected)
                        .ok_or_else(|| anyhow!("Pinned sensor runtime version unavailable"))
                })
                .transpose()?;
            let remaining = startup_stage.remaining_seconds()?;
            let manager = self.clone();
            let (sensor, workload, runtime, version, revision, config, setup_inputs) = (
                sensor.clone(),
                workload.clone(),
                runtime.clone(),
                version.cloned(),
                pack_revision.clone(),
                config.clone(),
                inputs.clone(),
            );
            let (pin, _) = startup_stage
                .run(move |cancel| async move {
                    let native = manager
                        .inner
                        .native
                        .as_ref()
                        .ok_or_else(|| anyhow!("Native sensor backend unavailable"))?;
                    let operator = manager
                        .inner
                        .workload_config
                        .as_ref()
                        .ok_or_else(|| anyhow!("Native sensor config unavailable"))?;
                    let master = manager
                        .inner
                        .encryption_key
                        .as_deref()
                        .ok_or_else(|| anyhow!("Native sensor encryption unavailable"))?;
                    attune_worker::native_environment::NativeEnvironmentInstaller::new(
                        &manager.inner.db,
                        workload.worker_id,
                        workload.worker_instance,
                        operator,
                        native,
                        master,
                    )?
                    .install(
                        attune_common::pack_environment::UpsertCoordinatedEnvironmentInput {
                            pack_id: sensor.pack.ok_or_else(|| anyhow!("Missing sensor pack"))?,
                            pack_ref: sensor
                                .pack_ref
                                .as_deref()
                                .ok_or_else(|| anyhow!("Missing sensor pack ref"))?,
                            runtime_id: runtime.id,
                            runtime_ref: &runtime.r#ref,
                            runtime_version: version.as_ref(),
                            env_path: std::path::Path::new(RUNTIME_ROOT),
                            manifest_checksum: Some(&revision.digest),
                        },
                        &config,
                        setup_inputs,
                        Some(remaining),
                        Some(&cancel),
                    )
                    .await
                    .map_err(Into::into)
                })
                .await?;
            inputs.push(pin);
        }
        let entry = format!("sensors/{}", sensor.entrypoint);
        let argv = if let Some(recipe) = &config.lifecycle {
            recipe.validate()?;
            if !self
                .inner
                .child_environment
                .admits_native_runtime_recipe(&recipe.digest()?)
            {
                return Err(anyhow!("Native sensor recipe not admitted"));
            }
            let version = selected_version
                .map(|selected| {
                    binding
                        .executable
                        .runtime_versions
                        .iter()
                        .find(|v| v.version == selected)
                        .ok_or_else(|| anyhow!("Pinned sensor runtime version unavailable"))
                })
                .transpose()?;
            let admitted = self
                .inner
                .child_environment
                .native_runtime_recipe_digests()
                .map(str::to_owned)
                .collect();
            let (manager, probe_runtime, probe_version) =
                (self.clone(), runtime.clone(), version.cloned());
            startup_stage
                .run(move |cancel| async move {
                    manager
                        .probe_native_runtime_cancellable(
                            &probe_runtime,
                            probe_version.as_ref(),
                            &admitted,
                            Some(&cancel),
                        )
                        .await
                        .map_err(Into::into)
                })
                .await?;
            attune_workload_backend::planning::runtime_recipe_argv(
                operator.platform,
                recipe,
                "/usr/local/bin:/usr/bin:/bin",
                &entry,
                false,
                selected_version,
            )?
        } else if matches!(config.interpreter.binary.as_str(), "" | "native" | "none") {
            vec![attune_workload_backend::planning::native_program(
                &entry,
                pack_revision.path.join(&entry).is_dir(),
                operator.platform,
            )?]
        } else {
            attune_workload_backend::planning::interpreted_argv(
                config,
                &entry,
                has_runtime,
                PackAccess::ReadOnly,
            )?
        };
        let selected = BTreeMap::from([
            ("ATTUNE_API_URL".into(), self.inner.api_url.clone()),
            ("ATTUNE_API_TOKEN".into(), token),
            (
                "ATTUNE_PACK_REF".into(),
                sensor
                    .pack_ref
                    .clone()
                    .ok_or_else(|| anyhow!("Missing sensor pack"))?,
            ),
            ("ATTUNE_SENSOR_ID".into(), sensor.id.to_string()),
            ("ATTUNE_SENSOR_REF".into(), sensor.r#ref.clone()),
            (
                "ATTUNE_SENSOR_WORKLOAD_ID".into(),
                workload.workload_id.to_string(),
            ),
            (
                "ATTUNE_SENSOR_ASSIGNMENT_GENERATION".into(),
                workload.generation.to_string(),
            ),
            (
                "ATTUNE_SENSOR_TRIGGER_TYPES".into(),
                serde_json::to_string(&binding.triggers.keys().collect::<Vec<_>>())?,
            ),
            (
                "ATTUNE_ARTIFACTS_DIR".into(),
                attune_common::workload_artifact::ARTIFACTS_ROOT.into(),
            ),
            (
                "ATTUNE_ALLOW_INSECURE_NOTIFIER_WS".into(),
                self.inner.allow_insecure_notifier_ws.to_string(),
            ),
            ("ATTUNE_LOG_LEVEL".into(), self.inner.log_level.clone()),
            ("ATTUNE_LOG_FORMAT".into(), self.inner.log_format.clone()),
            (
                "ATTUNE_NOTIFIER_WS_URL".into(),
                self.inner.notifier_ws_url.clone(),
            ),
        ]);
        let environment = if let Some(recipe) = &config.lifecycle {
            selected_recipe_environment(recipe, &self.inner.child_environment, &selected)?
        } else {
            attune_workload_backend::planning::action_environment(
                config,
                &selected,
                &entry,
                has_runtime,
                PackAccess::ReadOnly,
            )?
        };
        let context = Context {
            bootstrap_json: String::from_utf8(bootstrap)
                .map_err(|_| anyhow!("Sensor bootstrap encoding invalid"))?,
            intent: uuid::Uuid::new_v4(),
            target: LaunchTarget::Sensor {
                sensor: sensor.id,
                workload: workload.workload_id,
                worker: workload.worker_id,
                worker_instance: workload.worker_instance,
                generation: workload.generation,
            },
            image: operator.image.clone(),
            platform: operator.platform,
            inputs,
            invocation: LaunchInvocation {
                argv,
                env: environment,
                working_root: if has_runtime {
                    LaunchRoot::Runtime
                } else {
                    LaunchRoot::Pack
                },
            },
        };
        Ledger::reserve_native_effect(
            &self.inner.db,
            sensor.id,
            workload.fence(),
            context.intent,
            &serde_json::to_string(&context)?,
            master,
        )
        .await?;
        let spec = context.spec();
        let mut log_config = self
            .inner
            .sensor_log_config
            .with_retention_overrides(sensor.log_retention_policy, sensor.log_retention_limit);
        log_config.generation_binding_id = Some(binding_id);
        let artifacts = crate::sensor_log::register_sensor_log_artifacts(
            &self.inner.db,
            &sensor.r#ref,
            self.inner.artifact_transport.as_ref(),
            &log_config,
        )
        .await?;
        let (stdout_mirror, stderr_mirror) =
            attune_common::runtime_log_mirror::select_mirror_sources(
                attune_common::runtime_log_mirror::RuntimeLogSource::Sensor {
                    sensor_id: sensor.id,
                    sensor_ref: sensor.r#ref.clone(),
                    pack_ref: sensor.pack_ref.clone().unwrap_or_default(),
                    worker_id: workload.worker_id,
                    worker_name: self.inner.worker_name.clone(),
                    worker_instance: workload.worker_instance,
                    workload_id: workload.workload_id,
                    assignment_generation: workload.generation,
                    process_id: None,
                },
                self.inner.mirror_runtime_stdout_to_stdio && !sensitive,
                self.inner.mirror_runtime_stderr_to_stdio && !sensitive,
            );
        let transport = self
            .inner
            .api_client
            .artifact_transport(self.inner.artifact_transport.base_dir())?;
        Ledger::consume_native_effect(
            &self.inner.db,
            sensor.id,
            workload.fence(),
            spec.intent,
            false,
        )
        .await?;
        let owned = NativeSensor::new(
            NativeSensorContext {
                pool: self.inner.db.clone(),
                workload: workload.workload_id,
                spec: spec.clone(),
                resource: std::sync::OnceLock::new(),
                backend: native.backend.clone(),
                transport,
                worker: workload.worker_id,
                incarnation: workload.worker_instance,
            },
            self.inner.native_startup_tasks.clone(),
            startup_stage.attempt.clone(),
            self.inner.native_cleanup_failed.clone(),
        );
        let launcher = native.backend.launcher();
        let resource = launcher
            .prepare(&spec, ProcessCreateDisposition::CreateOnce)
            .await?;
        owned
            .context
            .resource
            .set(resource.clone())
            .map_err(|_| anyhow!("Native resource already owned"))?;
        Ledger::record_native_resource(
            &self.inner.db,
            workload.workload_id,
            spec.intent,
            &resource,
        )
        .await?;
        let io = launcher.start(&spec, &resource).await?;
        {
            let mut process = owned.process.lock().expect("native process owner");
            process.control = Some(io.control.clone());
            let waiter = io.control.clone();
            process.wait_task = Some(tokio::spawn(async move { waiter.wait().await }));
        }
        let stdout = capture(
            io.stdout,
            self.inner.artifact_transport.clone(),
            log_config.clone(),
            self.inner.db.clone(),
            sensor.r#ref.clone(),
            artifacts.stdout,
            workload.workload_id,
            spec.intent,
            false,
            stdout_mirror,
        );
        let stderr = capture(
            io.stderr,
            self.inner.artifact_transport.clone(),
            log_config,
            self.inner.db.clone(),
            sensor.r#ref.clone(),
            artifacts.stderr,
            workload.workload_id,
            spec.intent,
            true,
            stderr_mirror,
        );
        {
            let mut process = owned.process.lock().expect("native process owner");
            process.stdout = Some(stdout);
            process.stderr = Some(stderr);
        }
        // Persistence may wait; the gated process and captures are already owned.
        self.persist_sensor_process_started(&sensor, None, reset)
            .await;
        Ledger::consume_native_effect(
            &self.inner.db,
            sensor.id,
            workload.fence(),
            spec.intent,
            true,
        )
        .await?;
        let payload = attune_common::workload_stdin::LaunchStdinPayload::for_sensor_bootstrap(
            context.bootstrap_json.into_bytes(),
        )?;
        io.control.write_stdin_once(payload).await?;
        let instance = SensorInstance {
            native: Some(owned),
            prepared_target: None,
            sensor_ref: sensor.r#ref.clone(),
            sensor,
            status: Arc::new(RwLock::new(SensorStatus {
                running: true,
                failed: false,
                failure_count: 0,
                last_poll: Some(Utc::now()),
            })),
            child_process: None,
            stdout_handle: None,
            stderr_handle: None,
            token_expires_at,
            token_rotation: Arc::new(SensorTokenRotation::default()),
            workload,
            pack_revision,
            definition_updated,
        };
        instance
            .token_rotation
            .generation_binding
            .store(binding_id, Ordering::Release);
        instance
            .token_rotation
            .sensitive_logs
            .store(sensitive, Ordering::Release);
        Ok(instance)
    }
}

fn selected_recipe_environment(
    recipe: &attune_common::runtime_lifecycle::RuntimeRecipe,
    captured: &ChildProcessEnvironment,
    context: &BTreeMap<String, String>,
) -> Result<BTreeMap<String, String>> {
    recipe.validate()?;
    if !captured.admits_native_runtime_recipe(&recipe.digest()?) {
        return Err(anyhow!("Native sensor recipe not admitted"));
    }
    let mut selected = context.clone();
    for name in &recipe.permitted_caller_env {
        if let Some(value) = captured.get(name) {
            selected.insert(
                name.clone(),
                value
                    .to_str()
                    .ok_or_else(|| anyhow!("Sensor caller environment is not UTF-8"))?
                    .to_owned(),
            );
        }
    }
    Ok(attune_workload_backend::planning::runtime_recipe_environment(&selected)?)
}

fn encode_bootstrap(config: &serde_json::Value, triggers: &impl Serialize) -> Result<Vec<u8>> {
    if !config.is_object() {
        return Err(anyhow!("Sensor configuration must be a flat object"));
    }
    let bytes = serde_json::to_vec(&serde_json::json!({"config":config,"triggers":triggers}))?;
    if bytes.len() > 32 * 1024 * 1024 {
        return Err(anyhow!(
            "Sensor bootstrap exceeds its 32 MiB delivery limit"
        ));
    }
    Ok(bytes)
}

#[allow(clippy::too_many_arguments)]
fn capture(
    mut reader: attune_common::isolated_process::ProcessReader,
    transport: Arc<dyn ArtifactFileTransport>,
    config: crate::sensor_log::SensorLogConfig,
    pool: PgPool,
    sensor_ref: String,
    target: crate::sensor_log::SensorLogArtifactTarget,
    workload: Id,
    intent: uuid::Uuid,
    stderr: bool,
    mirror_source: Option<attune_common::runtime_log_mirror::RuntimeLogSource>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let result: Result<()> = async {
            let mut writer = crate::sensor_log::RotatingLogWriter::new_versioned(
                transport,
                &sensor_ref,
                if stderr { "stderr" } else { "stdout" },
                config,
                pool.clone(),
                target,
            );
            use attune_common::runtime_log_mirror::{RuntimeLogMirror, RuntimeLogStream};
            let mut mirror = mirror_source.map(|source| {
                RuntimeLogMirror::new(
                    source,
                    if stderr {
                        RuntimeLogStream::Stderr
                    } else {
                        RuntimeLogStream::Stdout
                    },
                    None,
                )
            });
            let mut buffer = vec![0; 64 * 1024];
            loop {
                let count = reader.read(&mut buffer).await?;
                if count == 0 {
                    break;
                }
                writer.write_bytes(&buffer[..count]).await?;
                if mirror
                    .as_mut()
                    .is_some_and(|mirror| mirror.push(&buffer[..count]).is_err())
                {
                    mirror = None;
                    warn!("Native sensor mirror unavailable; artifact capture continues");
                }
            }
            if let Some(mirror) = mirror.as_mut() {
                if mirror.finish().is_err() {
                    warn!("Native sensor mirror finish failed");
                }
            }
            writer.ensure_writer().await?;
            writer.close().await?;
            Ledger::record_native_capture(&pool, workload, intent, stderr).await?;
            Ok(())
        }
        .await;
        if result.is_err() {
            warn!(
                sensor_ref,
                stderr, "Native sensor capture incomplete; retaining exact resource"
            );
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_sensor_bootstrap_preserves_types_and_uses_only_stdin_sections() {
        let config = serde_json::json!({"secret":"private-config","count":3,"enabled":true});
        let triggers = serde_json::json!([{"parameters":{"private":"rule-secret"}}]);
        let bytes = encode_bootstrap(&config, &triggers).unwrap();
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&bytes).unwrap(),
            serde_json::json!({"config":config,"triggers":triggers})
        );
        assert!(!bytes.ends_with(b"\n"));
        assert!(encode_bootstrap(&serde_json::json!([]), &triggers).is_err());
    }
    #[test]
    fn native_sensor_bootstrap_retains_32_mib_bound_without_echoing_denied_values() {
        let small = serde_json::json!({"value":"x".repeat(9*1024*1024)});
        assert!(encode_bootstrap(&small, &Vec::<String>::new()).is_ok());
        let oversized = serde_json::json!({"secret":"z".repeat(32*1024*1024)});
        let error = encode_bootstrap(&oversized, &Vec::<String>::new())
            .unwrap_err()
            .to_string();
        assert_eq!(error, "Sensor bootstrap exceeds its 32 MiB delivery limit");
    }
}

#[allow(clippy::too_many_arguments)]
async fn settle_files(
    pool: &PgPool,
    workload: Id,
    spec: &ProcessSpec,
    resource: &ResourceIdentity,
    backend: &attune_workload_backend::configuration::NativeBackend,
    transport: &dyn ArtifactFileTransport,
    worker: Id,
    incarnation: uuid::Uuid,
) -> Result<()> {
    if !matches!(spec.target,LaunchTarget::Sensor{workload:id,worker:owner,worker_instance,..}
        if id==workload && owner==worker && worker_instance==incarnation)
    {
        return Err(anyhow!(
            "Replacement sensor incarnation cannot publish retained files"
        ));
    }
    let effect = Ledger::native_effect(pool, workload)
        .await?
        .ok_or_else(|| anyhow!("Sensor effect unavailable"))?;
    if effect.intent != spec.intent || !effect.capture_complete() {
        return Err(anyhow!("Sensor capture receipts incomplete"));
    }
    let versions = Ledger::native_file_inventory(pool, workload, spec.intent).await?;
    for version in versions {
        let path = version
            .file_path
            .as_deref()
            .ok_or_else(|| anyhow!("Native sensor file path unavailable"))?;
        let bytes =
            if version.body_state == Some(attune_common::models::enums::ArtifactBodyState::Ready) {
                // API-ready bodies are immutable receipts; do not read plaintext or
                // require a guest staging file for direct uploads, including empty.
                let expected = version
                    .size_bytes
                    .and_then(|v| u64::try_from(v).ok())
                    .ok_or_else(|| anyhow!("Ready sensor file size unavailable"))?;
                acknowledge_ready_file(transport, path, expected).await?
            } else {
                let relative = attune_common::artifact_transport::ValidatedRelativePath::new(path)?;
                let source = backend
                    .read_stopped_artifact(spec, resource, &relative, i64::MAX as u64)
                    .await?
                    .ok_or_else(|| anyhow!("Allocated sensor file was not written"))?;
                publish_stopped_file(
                    transport,
                    source,
                    path,
                    version.content_type.as_deref(),
                    i64::MAX as u64,
                )
                .await?
            };
        Ledger::record_native_file_receipt(pool, workload, spec.intent, version.id, bytes).await?;
    }
    if !Ledger::native_files_complete(pool, workload, spec.intent).await? {
        return Err(anyhow!("Sensor file obligations incomplete"));
    }
    backend.remove_stopped(spec, resource).await?;
    Ledger::acknowledge_native_removed(pool, workload, spec.intent, worker, incarnation).await?;
    Ok(())
}

async fn publish_stopped_file(
    transport: &dyn ArtifactFileTransport,
    source: std::fs::File,
    path: &str,
    content_type: Option<&str>,
    max_bytes: u64,
) -> attune_common::Result<u64> {
    let (snapshot, size) = tokio::task::spawn_blocking(move || {
        use std::io::{Read, Seek};
        let io_error = |_| Error::io("Stopped artifact snapshot I/O failed");
        let mut source = source;
        source.rewind().map_err(io_error)?;
        let metadata = source.metadata().map_err(io_error)?;
        if !metadata.is_file() {
            return Err(Error::invalid_state(
                "Stopped artifact is not a regular file",
            ));
        }
        let expected = metadata.len();
        if expected > max_bytes {
            return Err(Error::validation(
                "Stopped artifact exceeds publication bounds",
            ));
        }
        let mut snapshot = tempfile::NamedTempFile::new().map_err(io_error)?;
        let copied = std::io::copy(
            &mut source.take(expected.saturating_add(1)),
            snapshot.as_file_mut(),
        )
        .map_err(io_error)?;
        if copied != expected {
            return Err(Error::invalid_state(
                "Stopped artifact changed before publication",
            ));
        }
        snapshot.as_file().sync_all().map_err(io_error)?;
        Ok::<_, Error>((snapshot, copied))
    })
    .await
    .map_err(|_| Error::invalid_state("Stopped artifact snapshot did not complete"))??;
    let uploaded = transport
        .write_file_from_path(path, snapshot.path(), content_type)
        .await
        .map_err(|_| {
            Error::retryable_transport("Native artifact upload acknowledgement pending")
        })?;
    if uploaded != size
        || transport.complete_file(path).await.map_err(|_| {
            Error::retryable_transport("Native artifact publication acknowledgement pending")
        })? != Some(size)
    {
        return Err(Error::invalid_state(
            "Artifact publication acknowledgement mismatch",
        ));
    }
    Ok(size)
}

async fn acknowledge_ready_file(
    transport: &dyn ArtifactFileTransport,
    path: &str,
    expected: u64,
) -> Result<u64> {
    if transport.complete_file(path).await? != Some(expected) {
        return Err(anyhow!("Ready sensor file receipt mismatch"));
    }
    Ok(expected)
}

#[cfg(test)]
mod file_receipt_tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[test]
    fn native_recipe_environment_uses_admitted_caller_snapshot_and_private_sensor_context() {
        let recipe: attune_common::runtime_lifecycle::RuntimeRecipe = serde_json::from_value(serde_json::json!({
            "tools":[{"name":"python","binaries":["python3"],"version_pattern":"(?<version>3[.]12)","primary":true}],
            "launch":{"command":["{tool_python}","{source_file}"]},
            "permitted_caller_env":["APP_REGION"]
        })).unwrap();
        let captured = ChildProcessEnvironment::from_parent(
            [
                (
                    "ATTUNE_RUNTIME_RECIPE_DIGESTS".into(),
                    recipe.digest().unwrap().into(),
                ),
                ("APP_REGION".into(), "region-a".into()),
                ("JWT_SECRET".into(), "manager-only-secret".into()),
                ("ATTUNE_API_TOKEN".into(), "manager-only-token".into()),
            ],
            &["APP_REGION".into()],
        )
        .unwrap()
        .with_workload_sandbox(
            attune_common::workload_sandbox::WorkloadSandbox::NativeEffectsRequired,
        );
        let context = BTreeMap::from([
            ("ATTUNE_SENSOR_ID".into(), "42".into()),
            ("ATTUNE_API_TOKEN".into(), "admitted-sensor-token".into()),
        ]);
        let selected = selected_recipe_environment(&recipe, &captured, &context).unwrap();
        assert_eq!(selected.len(), 3);
        assert_eq!(selected["APP_REGION"], "region-a");
        assert_eq!(selected["ATTUNE_API_TOKEN"], "admitted-sensor-token");
        assert!(!selected.contains_key("JWT_SECRET"));
        let denied = ChildProcessEnvironment::from_parent([], &[])
            .unwrap()
            .with_workload_sandbox(
                attune_common::workload_sandbox::WorkloadSandbox::NativeEffectsRequired,
            );
        assert!(selected_recipe_environment(&recipe, &denied, &context).is_err());
    }

    #[tokio::test]
    async fn ready_receipt_uses_only_completion_without_plaintext_or_local_staging() {
        for (header, expected, success) in [
            ("x-attune-size: 0\r\n", 0, true),
            ("x-attune-size: 7\r\n", 7, true),
            ("x-attune-size: 1\r\n", 0, false),
            ("", 0, false),
        ] {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let server = tokio::spawn(async move {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut request = Vec::new();
                while !request.ends_with(b"\r\n\r\n") {
                    let mut chunk = [0u8; 1024];
                    let count = stream.read(&mut chunk).await.unwrap();
                    assert!(
                        count > 0 && request.len() + count <= 8192,
                        "bounded HTTP fixture header"
                    );
                    request.extend_from_slice(&chunk[..count]);
                }
                let request = String::from_utf8_lossy(&request);
                assert!(
                    request.starts_with(
                        "POST /api/v1/internal/artifacts/complete/files/versions/42/output.txt "
                    ),
                    "{request}"
                );
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n{header}\r\n"
                );
                stream.write_all(response.as_bytes()).await.unwrap();
            });
            let transport = attune_common::artifact_transport::ApiTransport::new(
                &format!("http://{address}"),
                "owned-fixture-token",
                "/nonexistent-native-staging",
            )
            .unwrap();
            let result =
                acknowledge_ready_file(&transport, "files/versions/42/output.txt", expected).await;
            assert_eq!(result.is_ok(), success);
            server.await.unwrap();
        }
    }

    #[tokio::test]
    async fn failed_durable_fence_still_joins_owned_physical_stop() {
        use std::sync::atomic::{AtomicBool, Ordering};
        let started = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        let terminal = Arc::new(AtomicBool::new(false));
        let (signal, finish, stopped) = (started.clone(), release.clone(), terminal.clone());
        let task = tokio::spawn(async move {
            attune_common::repositories::sensor_workload::native_effect::join_sensor_stop(
                async { Err(Error::PermissionDenied("owned fence unavailable".into())) },
                async move {
                    signal.notify_one();
                    finish.notified().await;
                    stopped.store(true, Ordering::Release);
                    Ok(LaunchExitOutcome {
                        exit_code: 137,
                        oom_killed: false,
                    })
                },
            )
            .await
        });
        tokio::time::timeout(Duration::from_secs(5), started.notified())
            .await
            .unwrap();
        assert!(
            !task.is_finished(),
            "failed fencing must not cancel physical ownership"
        );
        release.notify_one();
        assert!(task.await.unwrap().is_err());
        assert!(terminal.load(Ordering::Acquire));
    }

    #[test]
    fn sensor_bootstrap_above_general_limit_is_sensor_only_and_private() {
        let bytes = encode_bootstrap(
            &serde_json::json!({"private":"x".repeat(9*1024*1024)}),
            &Vec::<String>::new(),
        )
        .unwrap();
        let payload =
            attune_common::workload_stdin::LaunchStdinPayload::for_sensor_bootstrap(bytes).unwrap();
        assert!(payload.validate_for(LaunchPurpose::Sensor).is_ok());
        assert!(payload.validate_for(LaunchPurpose::Action).is_err());
        assert!(serde_json::to_vec(&payload).is_err());
    }
}

#[cfg(all(test, unix))]
#[path = "../../tests/support/native_provider.rs"]
mod provider_tests;

#[cfg(test)]
mod startup_ownership_tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;
    use tokio_util::task::TaskTracker;

    struct Joined(Arc<AtomicUsize>);
    impl Drop for Joined {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::AcqRel);
        }
    }
    struct CleanupFixture {
        physical: Arc<Notify>,
        durable: Arc<Notify>,
        joined: Arc<AtomicUsize>,
        failed: Arc<AtomicBool>,
        owner: NativeStartupCleanup,
    }
    impl CleanupFixture {
        async fn new(tasks: TaskTracker, attempt: TaskTracker, fail_fence: bool) -> Self {
            let physical = Arc::new(Notify::new());
            let durable = Arc::new(Notify::new());
            let joined = Arc::new(AtomicUsize::new(0));
            let failed = Arc::new(AtomicBool::new(false));
            let waiter_ready = Arc::new(Notify::new());
            let waiter = {
                let (ready, joined) = (waiter_ready.clone(), joined.clone());
                tokio::spawn(async move {
                    let _joined = Joined(joined);
                    ready.notify_one();
                    std::future::pending::<attune_common::Result<LaunchExitOutcome>>().await
                })
            };
            waiter_ready.notified().await;
            let streams = Arc::new(tokio::sync::Semaphore::new(0));
            let capture = || {
                let (streams, joined) = (streams.clone(), joined.clone());
                tokio::spawn(async move {
                    let permit = streams.acquire().await.unwrap();
                    permit.forget();
                    joined.fetch_add(1, Ordering::AcqRel);
                })
            };
            let (stdout, stderr) = (capture(), capture());
            let (fence_release, signal) = (durable.clone(), physical.clone());
            let owner = NativeStartupCleanup::new(tasks, attempt, failed.clone(), async move {
                let stopped = async {
                    attune_common::repositories::sensor_workload::native_effect::join_sensor_stop(
                        async {
                            fence_release.notified().await;
                            if fail_fence {
                                Err(Error::PermissionDenied("owned fence unavailable".into()))
                            } else {
                                Ok(())
                            }
                        },
                        async {
                            // A pure process-control double, not provider proof.
                            streams.add_permits(2);
                            signal.notify_one();
                            Ok(LaunchExitOutcome {
                                exit_code: 137,
                                oom_killed: false,
                            })
                        },
                    )
                    .await?;
                    Ok(())
                };
                join_native_startup_stop(
                    stopped,
                    &mut Some(waiter),
                    &mut Some(stdout),
                    &mut Some(stderr),
                )
                .await
            });
            Self {
                physical,
                durable,
                joined,
                failed,
                owner,
            }
        }
    }

    async fn observed(signal: &Notify) {
        timeout(Duration::from_secs(5), signal.notified())
            .await
            .expect("owned signal");
    }
    async fn captures_joined(joined: &AtomicUsize) {
        timeout(Duration::from_secs(5), async {
            while joined.load(Ordering::Acquire) != 3 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("all owned waiter/capture tasks joined");
    }

    #[tokio::test]
    async fn startup_error_timeout_and_caller_cancel_keep_cleanup_owned_through_db_stall() {
        for mode in ["error", "timeout", "caller-cancel"] {
            let tasks = TaskTracker::new();
            let attempt = TaskTracker::new();
            let fixture = CleanupFixture::new(tasks.clone(), attempt.clone(), false).await;
            let CleanupFixture {
                physical,
                durable,
                joined,
                failed,
                owner,
            } = fixture;
            let pending = Arc::new(Notify::new());
            let persistence = pending.clone();
            let ready = Arc::new(Notify::new());
            let entered = ready.clone();
            let start = async move {
                let _owner = owner;
                entered.notify_one();
                persistence.notified().await;
                Err::<(), _>(anyhow!("startup persistence failed"))
            };
            let deadline = if mode == "timeout" {
                Duration::ZERO
            } else {
                Duration::from_secs(60)
            };
            let caller = tokio::spawn(await_startup(
                start,
                CancellationToken::new(),
                true,
                Some(tokio::time::Instant::now() + deadline),
                "fixture",
            ));
            observed(&ready).await;
            if mode == "caller-cancel" {
                caller.abort();
                assert!(caller.await.unwrap_err().is_cancelled());
            } else {
                if mode == "error" {
                    pending.notify_one();
                }
                assert!(caller.await.unwrap().is_err());
            }
            observed(&physical).await;
            captures_joined(&joined).await;
            attempt.close();
            tasks.close();
            let waiting = tokio::spawn(async move {
                attempt.wait().await;
                tasks.wait().await;
            });
            assert!(
                !waiting.is_finished(),
                "DB uncertainty must not report a completed join"
            );
            durable.notify_one();
            timeout(Duration::from_secs(5), waiting)
                .await
                .unwrap()
                .unwrap();
            assert!(!failed.load(Ordering::Acquire));
        }
    }

    #[tokio::test]
    async fn cancelled_cleanup_waiter_does_not_abort_physical_stop_or_capture_joins() {
        let tasks = TaskTracker::new();
        let attempt = TaskTracker::new();
        let fixture = CleanupFixture::new(tasks.clone(), attempt.clone(), true).await;
        let CleanupFixture {
            physical,
            durable,
            joined,
            failed,
            mut owner,
        } = fixture;
        let caller = tokio::spawn(async move { owner.stop().await });
        observed(&physical).await;
        caller.abort();
        assert!(caller.await.unwrap_err().is_cancelled());
        captures_joined(&joined).await;
        durable.notify_one();
        attempt.close();
        tasks.close();
        timeout(Duration::from_secs(5), async {
            attempt.wait().await;
            tasks.wait().await;
        })
        .await
        .unwrap();
        assert!(
            failed.load(Ordering::Acquire),
            "uncertain ledger must remain failed"
        );
    }

    #[tokio::test]
    async fn capture_join_failure_cannot_become_complete_cleanup() {
        let tasks = TaskTracker::new();
        let attempt = TaskTracker::new();
        let failed = Arc::new(AtomicBool::new(false));
        let stream = tokio::spawn(std::future::pending::<()>());
        stream.abort();
        let mut owner =
            NativeStartupCleanup::new(tasks.clone(), attempt.clone(), failed.clone(), async {
                join_native_startup_stop(async { Ok(()) }, &mut None, &mut Some(stream), &mut None)
                    .await
            });
        assert!(owner.stop().await.is_err());
        attempt.close();
        tasks.close();
        attempt.wait().await;
        tasks.wait().await;
        assert!(failed.load(Ordering::Acquire));
    }

    pub(super) fn lazy_manager() -> (SensorManager, PgPool) {
        // Lazy pool + local transports: no DB/API requests or service setup.
        let pool = sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgres://unused:unused@unused.invalid/unused")
            .unwrap();
        let manager = SensorManager::new(
            pool.clone(),
            SensorManagerConfig {
                native: None,
                workload_config: None,
                encryption_key: None,
                child_environment: ChildProcessEnvironment::from_parent([], &[]).unwrap(),
                api_url: "http://unused.invalid".into(),
                worker_token_provider: None,
                notifier_ws_url: "ws://unused.invalid".into(),
                allow_insecure_notifier_ws: false,
                mq_url: String::new(),
                log_level: "warn".into(),
                log_format: "json".into(),
                packs_base_dir: "/unused".into(),
                runtime_envs_dir: "/unused".into(),
                pack_transport: Arc::new(attune_common::pack_transport::VolumePackTransport::new(
                    "/unused",
                )),
                artifact_transport: Arc::new(
                    attune_common::artifact_transport::VolumeTransport::new("/unused"),
                ),
                sensor_log_config: Default::default(),
                mirror_runtime_stdout_to_stdio: false,
                mirror_runtime_stderr_to_stdio: false,
                worker_name: "ownership-fixture".into(),
            },
        );
        (manager, pool)
    }

    #[tokio::test]
    async fn manager_shutdown_cancels_and_joins_unregistered_startup_owners() {
        let (manager, pool) = lazy_manager();
        *manager.inner.running.write().await = true;
        let tasks = manager.inner.native_startup_tasks.clone();
        let attempt = TaskTracker::new();
        let fixture = CleanupFixture::new(tasks.clone(), attempt.clone(), false).await;
        let CleanupFixture {
            physical,
            durable,
            joined,
            failed,
            owner,
        } = fixture;
        let ready = Arc::new(Notify::new());
        let entered = ready.clone();
        let startup_manager = manager.clone();
        let startup = tokio::spawn(async move {
            let _gate = startup_manager.inner.lifecycle_gate.read().await;
            let stopping = startup_manager
                .inner
                .native_startup_stop
                .read()
                .await
                .clone();
            let result = await_startup(
                async move {
                    let _owner = owner;
                    entered.notify_one();
                    std::future::pending::<Result<()>>().await
                },
                stopping,
                true,
                Some(tokio::time::Instant::now() + Duration::from_secs(60)),
                "fixture",
            )
            .await;
            attempt.close();
            attempt.wait().await;
            result
        });
        observed(&ready).await;
        assert!(manager.inner.sensors.read().await.is_empty());
        let shutdown = tokio::spawn(async move { manager.stop().await });
        observed(&physical).await;
        captures_joined(&joined).await;
        assert!(!shutdown.is_finished());
        durable.notify_one();
        assert!(timeout(Duration::from_secs(5), startup)
            .await
            .unwrap()
            .unwrap()
            .is_err());
        timeout(Duration::from_secs(5), shutdown)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(tasks.is_closed() && tasks.is_empty());
        assert!(!failed.load(Ordering::Acquire));
        pool.close().await;
    }
}

#[cfg(test)]
mod nested_startup_ownership_tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;
    use tokio_util::task::TaskTracker;

    #[derive(Default)]
    struct Acknowledgements {
        entered: Notify,
        late_prepare: Notify,
        stop_requested: Notify,
        stop_ack: Notify,
        capture_ack: Notify,
        durable_ack: Notify,
        stopped: Notify,
        captured: Notify,
        calls: AtomicUsize,
        finished: AtomicBool,
        dropped: AtomicBool,
        accepted_cache: AtomicBool,
        token: std::sync::Mutex<Option<CancellationToken>>,
    }
    struct OperationLifetime(Arc<Acknowledgements>);
    impl Drop for OperationLifetime {
        fn drop(&mut self) {
            self.0.dropped.store(true, Ordering::Release);
        }
    }
    async fn controlled_nested_operation(
        state: Arc<Acknowledgements>,
        cancel: CancellationToken,
        window: &'static str,
        uncertain_stop: bool,
        uncertain_capture: bool,
    ) -> Result<()> {
        let _lifetime = OperationLifetime(state.clone());
        state.calls.fetch_add(1, Ordering::AcqRel);
        *state.token.lock().unwrap() = Some(cancel.clone());
        state.entered.notify_one();
        if window == "prepare" {
            // The original provider call remains owned until its late ACK.
            state.late_prepare.notified().await;
        }
        cancel.cancelled().await;
        state.stop_requested.notify_one();
        let (stop, capture, ()) = tokio::join!(
            async {
                state.stop_ack.notified().await;
                state.stopped.notify_one();
                if uncertain_stop {
                    Err(anyhow!("controlled Stop ACK uncertain"))
                } else {
                    Ok(())
                }
            },
            async {
                state.capture_ack.notified().await;
                state.captured.notify_one();
                if uncertain_capture {
                    Err(anyhow!("controlled capture ACK uncertain"))
                } else {
                    Ok(())
                }
            },
            async {
                state.durable_ack.notified().await;
            },
        );
        state.finished.store(true, Ordering::Release);
        stop?;
        capture?;
        // Deliberately return a late success after cancellation. The actual
        // startup wrapper must not expose it as an accepted verification.
        Ok(())
    }
    async fn observed(signal: &Notify) {
        timeout(Duration::from_secs(5), signal.notified())
            .await
            .expect("owned nested-operation signal");
    }
    fn stage(deadline: tokio::time::Instant, stopping: CancellationToken) -> NativeStartupStage {
        NativeStartupStage::new(
            TaskTracker::new(),
            TaskTracker::new(),
            stopping,
            deadline,
            Arc::new(AtomicBool::new(false)),
        )
    }
    async fn release_and_join(stage: &NativeStartupStage, state: &Acknowledgements, window: &str) {
        assert_eq!(state.calls.load(Ordering::Acquire), 1);
        assert!(!state.finished.load(Ordering::Acquire));
        assert!(!state.dropped.load(Ordering::Acquire));
        assert_eq!(
            stage.tasks.len(),
            1,
            "cancelled caller lost its nested owner"
        );
        if window == "prepare" {
            state.late_prepare.notify_one();
        }
        observed(&state.stop_requested).await;
        // Known physical stop/capture progress independently of a pending DB
        // fence. Neither acknowledgement ends ownership of the other awaits.
        state.stop_ack.notify_one();
        observed(&state.stopped).await;
        state.capture_ack.notify_one();
        observed(&state.captured).await;
        assert!(!state.finished.load(Ordering::Acquire));
        assert!(!state.dropped.load(Ordering::Acquire));
        assert_eq!(stage.tasks.len(), 1);
        state.durable_ack.notify_one();
        stage.attempt.close();
        stage.tasks.close();
        timeout(Duration::from_secs(5), async {
            stage.attempt.wait().await;
            stage.tasks.wait().await;
        })
        .await
        .expect("retained nested future joined");
        assert!(state.finished.load(Ordering::Acquire));
        assert!(state.dropped.load(Ordering::Acquire));
        assert!(!state.accepted_cache.load(Ordering::Acquire));
        assert_eq!(
            state.calls.load(Ordering::Acquire),
            1,
            "original operation was replayed"
        );
    }
    fn caller(
        stage: NativeStartupStage,
        state: Arc<Acknowledgements>,
        kind: &'static str,
        window: &'static str,
        uncertain_stop: bool,
        uncertain_capture: bool,
    ) -> JoinHandle<Result<()>> {
        tokio::spawn(async move {
            let (deadline, stopping) = (stage.deadline, stage.stopping.clone());
            let operation_state = state.clone();
            let result = await_startup(
                stage.run(move |cancel| {
                    controlled_nested_operation(
                        operation_state,
                        cancel,
                        window,
                        uncertain_stop,
                        uncertain_capture,
                    )
                }),
                stopping,
                true,
                Some(deadline),
                kind,
            )
            .await;
            if result.is_ok() {
                state.accepted_cache.store(true, Ordering::Release);
            }
            result
        })
    }

    #[tokio::test]
    async fn startup_wrapper_retains_nested_install_and_probe_on_shutdown_and_caller_drop() {
        for kind in ["installation", "discovery"] {
            for window in ["prepare", "resource-record", "running", "post-execution"] {
                for mode in ["shutdown", "caller-drop"] {
                    let stage = stage(
                        tokio::time::Instant::now() + Duration::from_secs(60),
                        CancellationToken::new(),
                    );
                    let state = Arc::new(Acknowledgements::default());
                    let caller = caller(
                        stage.clone(),
                        state.clone(),
                        kind,
                        window,
                        window == "resource-record",
                        window == "running",
                    );
                    observed(&state.entered).await;
                    if mode == "caller-drop" {
                        caller.abort();
                        assert!(caller.await.unwrap_err().is_cancelled());
                    } else {
                        stage.stopping.cancel();
                        assert!(caller.await.unwrap().is_err());
                    }
                    let token = state.token.lock().unwrap().clone().unwrap();
                    timeout(Duration::from_secs(5), token.cancelled())
                        .await
                        .unwrap();
                    release_and_join(&stage, &state, window).await;
                    assert_eq!(
                        stage.failed.load(Ordering::Acquire),
                        window == "resource-record" || window == "running",
                        "joined uncertain outcome lost from shutdown latch"
                    );
                }
            }
        }
    }

    #[tokio::test]
    async fn startup_wrapper_deadline_retains_nested_acks_without_resetting_budget() {
        for kind in ["installation", "discovery"] {
            // This exercises the real deadline, not a sleep used as readiness.
            let deadline = tokio::time::Instant::now() + Duration::from_secs(1);
            let stage = stage(deadline, CancellationToken::new());
            let state = Arc::new(Acknowledgements::default());
            let caller = caller(stage.clone(), state.clone(), kind, "prepare", false, false);
            observed(&state.entered).await;
            assert!(caller.await.unwrap().is_err());
            assert_eq!(stage.deadline, deadline);
            release_and_join(&stage, &state, "prepare").await;
            let calls = Arc::new(AtomicUsize::new(0));
            let count = calls.clone();
            assert!(stage
                .run(move |_| async move {
                    count.fetch_add(1, Ordering::AcqRel);
                    Ok(())
                })
                .await
                .is_err());
            assert_eq!(
                calls.load(Ordering::Acquire),
                0,
                "later nested call reset the expired stage budget"
            );
        }
    }

    #[tokio::test]
    async fn closed_stage_never_enters_nested_producer_or_accepts_verification() {
        for expired in [true, false] {
            let stop = CancellationToken::new();
            let now = tokio::time::Instant::now();
            let deadline = if expired {
                now
            } else {
                stop.cancel();
                now + Duration::from_secs(60)
            };
            let stage = stage(deadline, stop);
            let entered = Arc::new(AtomicBool::new(false));
            let called = entered.clone();
            assert!(stage
                .run(move |_| async move {
                    called.store(true, Ordering::Release);
                    Ok(())
                })
                .await
                .is_err());
            assert!(!entered.load(Ordering::Acquire));
            assert!(
                !stage.failed.load(Ordering::Acquire),
                "proven no-effect cancellation latched failure"
            );
            assert!(stage.tasks.is_empty() && stage.attempt.is_empty());
            stage.tasks.close();
            stage.attempt.close();
            stage.tasks.wait().await;
            stage.attempt.wait().await;
        }
    }

    #[tokio::test]
    async fn queued_nested_owner_survives_caller_drop_before_producer_is_polled() {
        let stage = stage(
            tokio::time::Instant::now() + Duration::from_secs(60),
            CancellationToken::new(),
        );
        let entered = Arc::new(AtomicBool::new(false));
        let called = entered.clone();
        let mut awaiting = Box::pin(stage.run(move |_| async move {
            called.store(true, Ordering::Release);
            Ok(())
        }));
        std::future::poll_fn(|cx| {
            assert!(awaiting.as_mut().poll(cx).is_pending());
            std::task::Poll::Ready(())
        })
        .await;
        assert_eq!(stage.tasks.len(), 1);
        drop(awaiting);
        assert!(!entered.load(Ordering::Acquire));
        stage.tasks.close();
        stage.attempt.close();
        timeout(Duration::from_secs(5), async {
            stage.tasks.wait().await;
            stage.attempt.wait().await;
        })
        .await
        .unwrap();
        assert!(
            !entered.load(Ordering::Acquire),
            "producer ran after its queued caller closed"
        );
        assert!(
            !stage.failed.load(Ordering::Acquire),
            "unpolled producer was treated as uncertain cleanup"
        );
    }

    #[tokio::test]
    async fn manager_stop_preserves_joined_nested_failure_after_caller_disappears() {
        for failure in ["stop", "capture", "completed-then-closed"] {
            let (manager, pool) = super::startup_ownership_tests::lazy_manager();
            let stage = NativeStartupStage::new(
                manager.inner.native_startup_tasks.clone(),
                TaskTracker::new(),
                manager.inner.native_startup_stop.read().await.clone(),
                tokio::time::Instant::now() + Duration::from_secs(60),
                manager.inner.native_cleanup_failed.clone(),
            );
            if failure == "completed-then-closed" {
                assert!(stage
                    .run(|cancel| async move {
                        cancel.cancel();
                        Err::<(), _>(anyhow!(
                            "controlled uncertain result observed while closing"
                        ))
                    })
                    .await
                    .is_err());
                stage.tasks.close();
                stage.attempt.close();
                stage.tasks.wait().await;
                stage.attempt.wait().await;
            } else {
                let state = Arc::new(Acknowledgements::default());
                let caller = caller(
                    stage.clone(),
                    state.clone(),
                    "installation",
                    "running",
                    failure == "stop",
                    failure == "capture",
                );
                observed(&state.entered).await;
                caller.abort();
                assert!(caller.await.unwrap_err().is_cancelled());
                let token = state.token.lock().unwrap().clone().unwrap();
                timeout(Duration::from_secs(5), token.cancelled())
                    .await
                    .unwrap();
                release_and_join(&stage, &state, "running").await;
            }
            assert!(stage.tasks.is_empty() && stage.attempt.is_empty());
            assert!(manager.inner.sensors.read().await.is_empty());
            assert!(manager.inner.native_cleanup_failed.load(Ordering::Acquire));
            assert!(
                manager.stop().await.is_err(),
                "empty trackers erased uncertain nested outcome"
            );
            pool.close().await;
        }
    }
}
