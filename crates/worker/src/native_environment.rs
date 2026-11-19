//! Existing coordinated native setup, shared by trusted worker and sensor loops.
//! Source/claim admission remains in PackEnvironmentManager and its repositories.
use attune_common::{
    isolated_process::ProcessSpec,
    models::RuntimeExecutionConfig,
    pack_environment::{
        PackEnvironmentManager, PackEnvironmentStatus, UpsertCoordinatedEnvironmentInput,
    },
    repositories::{
        execution_process::ExecutionProcessRepository as Effects,
        pack_environment_process::PackEnvironmentProcessRepository as Setup,
    },
    workload_config::WorkloadConfig,
    workload_launch::*,
    Error, Result,
};
use attune_workload_backend::configuration::ConfiguredBackend;
use sqlx::PgPool;
use tokio_util::sync::CancellationToken;

fn denied() -> Error {
    Error::PermissionDenied("Native runtime preparation unavailable".into())
}

pub struct NativeEnvironmentInstaller<'a> {
    pool: &'a PgPool,
    worker: i64,
    incarnation: uuid::Uuid,
    config: &'a WorkloadConfig,
    configured: &'a ConfiguredBackend,
    master: &'a str,
}
impl<'a> NativeEnvironmentInstaller<'a> {
    pub fn new(
        pool: &'a PgPool,
        worker: i64,
        incarnation: uuid::Uuid,
        config: &'a WorkloadConfig,
        configured: &'a ConfiguredBackend,
        master: &'a str,
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
            pool,
            worker,
            incarnation,
            config,
            configured,
            master,
        })
    }
    async fn finish_native_setup_removal(
        &self,
        spec: &ProcessSpec,
        resource: &attune_common::isolated_process::ResourceIdentity,
    ) -> Result<()> {
        if Setup::removal_recorded(self.pool, spec.intent, resource).await? {
            return Ok(());
        }
        Effects::request_stop(self.pool, spec.intent).await?;
        self.configured
            .backend
            .remove_stopped(spec, resource)
            .await?;
        Setup::acknowledge_removed(self.pool, spec.intent, Some(resource)).await
    }

    async fn settle_old_native_setups(&self, environment: i64, master: &str) -> Result<()> {
        let backend = &self.configured.backend;
        loop {
            let debt =
                Setup::unsettled_with_resources(self.pool, environment, None, master).await?;
            if debt.is_empty() {
                return Ok(());
            }
            for (spec, submitted, retained) in debt {
                if !submitted {
                    Effects::request_stop(self.pool, spec.intent).await?;
                    // The repository rechecks the create latch under its lock.
                    Setup::acknowledge_removed(self.pool, spec.intent, None).await?;
                    continue;
                }
                let newly_observed = retained.is_none();
                let resource=match retained {
                    Some(resource)=>resource,
                    None=>backend.launcher().prepare(&spec,attune_common::isolated_process::ProcessCreateDisposition::ReconcileOnly).await?,
                };
                let fence = async {
                    if newly_observed {
                        Effects::record_resource(self.pool, spec.intent, &resource).await?;
                    }
                    Effects::request_stop(self.pool, spec.intent).await
                };
                let (fenced, stopped) = tokio::join!(
                    fence,
                    backend.stop_retained(&spec, &resource, LaunchStopReason::SourceRevoked)
                );
                fenced?;
                let exit = stopped?;
                Effects::record_exit(self.pool, spec.intent, &resource, exit).await?;
                backend.remove_stopped(&spec, &resource).await?;
                Setup::acknowledge_removed(self.pool, spec.intent, Some(&resource)).await?;
            }
        }
    }

    pub async fn install(
        &self,
        input: UpsertCoordinatedEnvironmentInput<'_>,
        config: &RuntimeExecutionConfig,
        inputs: Vec<PinnedLaunchInput>,
        timeout: Option<u64>,
        cancel: Option<&CancellationToken>,
    ) -> Result<(PinnedLaunchInput, bool)> {
        if config.lifecycle.is_some() {
            return Err(denied());
        }
        let native = self.configured;
        let backend = &self.configured.backend;
        let master = self.master;
        let pack = inputs
            .iter()
            .find(|p| p.kind == LaunchInputKind::Pack)
            .ok_or_else(denied)?;
        // Only published configuration and immutable public build inputs. Never
        // hash rendered private environment, parameters or a SetupPlan.
        let engine = match &self.config.backend {
            attune_common::workload_config::WorkloadBackendConfig::Docker { binding, .. } => {
                Some(binding)
            }
            attune_common::workload_config::WorkloadBackendConfig::Kubernetes { .. } => None,
        };
        let mut public_binding = serde_json::json!({
            "image":self.config.image,"platform":self.config.platform,"engine":engine,"pack":pack,
            "runner":native.runner,"runtime":input.runtime_id,"version":input.runtime_version.map(|v|v.id),"config":config,
        });
        public_binding.sort_all_objects();
        let binding =
            attune_common::runtime_cache::sha256_bytes(&serde_json::to_vec(&public_binding)?);
        if let Some(pin) = Setup::published(self.pool, &binding).await? {
            if native.cache.open_input_archive(&pin).is_ok() {
                let (spec, resource) = Setup::retained_output(self.pool, &pin, master).await?;
                self.finish_native_setup_removal(&spec, &resource).await?;
                return Ok((pin, false));
            }
        }
        let manager = PackEnvironmentManager::new(self.pool.clone());
        let expected = manager
            .upsert_native_environment(input, &Sha256Digest::parse(&binding)?)
            .await?;
        if expected.status == PackEnvironmentStatus::Ready {
            manager
                .mark_coordinated_environment_outdated(&expected.env_key)
                .await?;
        }
        let worker = self.worker;
        let instance = self.incarnation;
        let claim_wait = std::time::Instant::now();
        let claimed = loop {
            if timeout.is_some_and(|seconds| claim_wait.elapsed().as_secs() >= seconds) {
                return Err(denied());
            }
            let current = manager
                .get_coordinated_environment(&expected.env_key)
                .await?
                .ok_or_else(denied)?;
            if current.manifest_checksum != expected.manifest_checksum
                || current.env_path != expected.env_path
            {
                return Err(denied());
            }
            if cancel.is_some_and(|t| t.is_cancelled()) {
                return Err(denied());
            }
            if let Some(pin) = Setup::published(self.pool, &binding).await? {
                if native.cache.open_input_archive(&pin).is_ok() {
                    let (spec, resource) = Setup::retained_output(self.pool, &pin, master).await?;
                    self.finish_native_setup_removal(&spec, &resource).await?;
                    return Ok((pin, false));
                }
            }
            if let Some(claim) = manager
                .claim_coordinated_environment(&expected, worker, 300)
                .await?
            {
                break claim;
            }
            tokio::time::sleep(std::time::Duration::from_millis(250)).await;
        };
        // New claim/nonce fences late publication by any previous installer.
        // Settle original resources before creating another generation.
        self.settle_old_native_setups(claimed.id, master).await?;
        let claim = claimed.claim_id.ok_or_else(denied)?;
        let plan = crate::runtime::launch_planning::setup_plan(config, &Default::default(), &[])
            .map_err(|_| denied())?;
        let plan = serde_json::to_string(&plan)?;
        let mut setup = ProcessSpec {
            intent: uuid::Uuid::new_v4(),
            purpose: LaunchPurpose::Setup,
            target: LaunchTarget::Setup {
                admission: claimed.id,
                attempt: claim,
            },
            image: self.config.image.clone(),
            platform: self.config.platform,
            inputs,
            pack_access: PackAccess::ReadOnly,
            stage: Some(PrivateStage {
                cache_key: binding,
                quota_bytes: self.config.storage.expanded_bytes,
                quota_files: self.config.storage.entries,
            }),
            timeout_ms: None,
            invocation: LaunchInvocation {
                argv: Vec::new(),
                env: Default::default(),
                working_root: LaunchRoot::Pack,
            },
        };
        setup.invocation = LaunchInvocation {
            argv: vec![
                crate::runtime::launch_planning::runner_program(setup.platform),
                "setup".into(),
            ],
            env: Default::default(),
            working_root: LaunchRoot::Pack,
        };
        setup.timeout_ms = timeout
            .map(|s| s.checked_mul(1000).ok_or_else(denied))
            .transpose()?;
        let setup =
            Setup::reserve(self.pool, &claimed, worker, instance, &setup, &plan, master).await?;
        let launcher = backend.launcher();
        let create = Setup::begin_create(self.pool, setup.intent, master).await?;
        let resource = launcher.prepare(&setup, create).await?;
        Effects::record_resource(self.pool, setup.intent, &resource).await?;
        let mut process = launcher.start(&setup, &resource).await?;
        if process.resource != resource {
            process
                .control
                .stop(LaunchStopReason::LaunchFailure)
                .await?;
            return Err(denied());
        }
        process.control = crate::executor::recording_process_control(
            process.control,
            self.pool.clone(),
            setup.intent,
            resource.clone(),
        );
        let payload = match Setup::consume_stdin(self.pool, setup.intent, &resource, master).await {
            Ok(value) => value,
            Err(error) => {
                process
                    .control
                    .stop(LaunchStopReason::SourceRevoked)
                    .await?;
                return Err(error);
            }
        };
        let cancel = cancel.cloned().unwrap_or_default().child_token();
        let future = crate::runtime::process_executor::execute_isolated_cancellable(
            process,
            payload,
            crate::runtime::process_executor::IsolatedCaptureOptions {
                timeout_secs: timeout,
                max_stdout_bytes: 16 * 1024,
                max_stderr_bytes: 16 * 1024,
                output_format: attune_common::models::OutputFormat::Text,
                output_policy: Default::default(),
                out_schema: None,
                cancel_token: Some(cancel.clone()),
                stdout_log_writer: None,
                stderr_log_writer: None,
            },
        );
        tokio::pin!(future);
        let mut renew = tokio::time::interval(std::time::Duration::from_secs(15));
        let result = loop {
            tokio::select! {
                result=&mut future=>break result.map_err(|_|denied())?,
                _=renew.tick()=>{
                    if !manager.renew_coordinated_environment_claim(&claimed.env_key,worker,claim,300).await.unwrap_or(false)
                        || Setup::validate_source(self.pool,setup.intent,master).await.is_err() {
                        cancel.cancel();let _=(&mut future).await;return Err(denied());
                    }
                }
            }
        };
        if result.exit_code != 0
            || result.error.is_some()
            || result.logs_incomplete
            || result.timed_out
        {
            manager
                .mark_coordinated_environment_failed(
                    &claimed.env_key,
                    worker,
                    claim,
                    "Native setup failed; private diagnostic output withheld",
                )
                .await?;
            return Err(denied());
        }
        // Wait/Remove proof is adapter-owned; archive bytes alone never grant Ready.
        let mut archive = backend.archive_stopped_stage(&setup, &resource).await?;
        let ceiling = self.config.storage;
        let cache = native.cache.clone();
        let id = claimed.id;
        let intent = setup.intent;
        let pin_id = (1..)
            .find(|id| !setup.inputs.iter().any(|p| p.pin_id == *id))
            .ok_or_else(denied)?;
        let pin = tokio::task::spawn_blocking(move || {
            use sha2::{Digest, Sha256};
            use std::io::{Read, Seek, SeekFrom};
            let policy = attune_common::workload_archive::ArchivePolicy {
                root: LaunchInputKind::Runtime,
                max_compressed_bytes: ceiling.compressed_bytes,
                max_expanded_bytes: ceiling.expanded_bytes,
                max_files: ceiling.entries,
            };
            let validated = attune_common::workload_archive::validate_zip(&mut archive, &policy)
                .map_err(|_| denied())?;
            archive.seek(SeekFrom::Start(0)).map_err(|_| denied())?;
            let mut hash = Sha256::new();
            let mut buffer = [0; 65536];
            let mut size = 0u64;
            loop {
                let n = archive.read(&mut buffer).map_err(|_| denied())?;
                if n == 0 {
                    break;
                }
                size += n as u64;
                if size > ceiling.compressed_bytes {
                    return Err(denied());
                }
                hash.update(&buffer[..n]);
            }
            let pin = PinnedLaunchInput {
                pin_id,
                kind: LaunchInputKind::Runtime,
                object_id: id,
                object_version: intent.to_string(),
                size_bytes: size,
                sha256: Sha256Digest::parse(
                    hash.finalize()
                        .iter()
                        .map(|byte| format!("{byte:02x}"))
                        .collect::<String>(),
                )?,
                max_expanded_bytes: validated.expanded_bytes,
                max_files: validated.files,
            };
            archive.seek(SeekFrom::Start(0)).map_err(|_| denied())?;
            cache
                .install_input(&pin, &mut archive)
                .map_err(|_| denied())?;
            Ok::<_, Error>(pin)
        })
        .await
        .map_err(|_| denied())??;
        Setup::publish(self.pool, setup.intent, &resource, &pin, master).await?;
        self.finish_native_setup_removal(&setup, &resource).await?;
        Ok((pin, true))
    }
}
