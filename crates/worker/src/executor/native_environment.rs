//! Coordinated legacy environment preparation through the same native effects.
//! Lifecycle recipes deliberately retain their separate private generations.
use super::*;
use attune_common::{
    isolated_process::ProcessSpec,
    pack_environment::{
        PackEnvironmentManager, PackEnvironmentStatus, UpsertCoordinatedEnvironmentInput,
    },
    repositories::{
        execution_process::ExecutionProcessRepository as Effects,
        pack_environment_process::PackEnvironmentProcessRepository as Setup,
    },
    workload_launch::*,
};

fn denied() -> Error {
    Error::PermissionDenied("Native runtime preparation unavailable".into())
}
impl ActionExecutor {
    pub(super) async fn prepare_isolated_interpreted(
        &self,
        context: &ExecutionContext,
        spec: &mut ProcessSpec,
        execution: &Execution,
    ) -> Result<()> {
        let started = std::time::Instant::now();
        let config = context
            .runtime_config_override
            .as_ref()
            .ok_or_else(denied)?;
        let snapshot = execution.executable_snapshot.as_ref().ok_or_else(denied)?;
        let action = &snapshot.executable.action;
        if config.lifecycle.is_some()
            || action.launch.is_some()
            || context.code.is_some()
            || action.parameter_delivery != attune_common::models::ParameterDelivery::Stdin
            || action.parameter_format != attune_common::models::ParameterFormat::Json
            || spec.pack_access != PackAccess::ReadOnly
        {
            return Err(denied());
        }
        let relative =
            attune_common::artifact_transport::ValidatedRelativePath::new(&context.entry_point)?;
        let file = format!("actions/{}", relative.as_str());
        let has_runtime = crate::runtime::launch_planning::needs_runtime(config);
        if has_runtime {
            let retained = spec
                .inputs
                .iter()
                .find(|pin| pin.kind == LaunchInputKind::Runtime)
                .cloned();
            if let Some(pin) = &retained {
                self.native_preparation
                    .as_ref()
                    .ok_or_else(denied)?
                    .cache
                    .open_input_archive(pin)
                    .map_err(|_| denied())?;
            }
            let pin = self
                .ensure_native_environment(context, spec, execution)
                .await?;
            match retained {
                Some(old) if old != pin => return Err(denied()),
                Some(_) => {}
                None => spec.inputs.push(pin),
            }
        }
        spec.invocation.argv = crate::runtime::launch_planning::interpreted_argv(
            config,
            &file,
            has_runtime,
            spec.pack_access,
        )
        .map_err(|_| denied())?;
        let mut selected = context
            .env
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect::<std::collections::BTreeMap<_, _>>();
        selected.extend(
            context
                .execution_env
                .iter()
                .map(|(k, v)| (k.clone(), v.clone())),
        );
        spec.invocation.env = crate::runtime::launch_planning::action_environment(
            config,
            &selected,
            &file,
            has_runtime,
            spec.pack_access,
        )
        .map_err(|_| denied())?;
        spec.invocation.working_root = if has_runtime {
            LaunchRoot::Runtime
        } else {
            LaunchRoot::Pack
        };
        spec.timeout_ms = context
            .timeout
            .map(|t| {
                t.checked_mul(1000)
                    .and_then(|ms| {
                        ms.checked_sub(u64::try_from(started.elapsed().as_millis()).ok()?)
                    })
                    .filter(|ms| *ms > 0)
                    .ok_or_else(denied)
            })
            .transpose()?;
        Ok(())
    }

    async fn finish_native_setup_removal(
        &self,
        spec: &ProcessSpec,
        resource: &attune_common::isolated_process::ResourceIdentity,
    ) -> Result<()> {
        if Setup::removal_recorded(&self.pool, spec.intent, resource).await? {
            return Ok(());
        }
        Effects::request_stop(&self.pool, spec.intent).await?;
        self.native_backend
            .as_ref()
            .ok_or_else(denied)?
            .remove_stopped(spec, resource)
            .await?;
        Setup::acknowledge_removed(&self.pool, spec.intent, Some(resource)).await
    }

    async fn settle_old_native_setups(&self, environment: i64, master: &str) -> Result<()> {
        let backend = self.native_backend.as_ref().ok_or_else(denied)?;
        loop {
            let debt = Setup::unsettled(&self.pool, environment, None, master).await?;
            if debt.is_empty() {
                return Ok(());
            }
            for (spec, submitted) in debt {
                // Close publication/input before any uncertain provider wait.
                Effects::request_stop(&self.pool, spec.intent).await?;
                if !submitted {
                    // Repository rechecks the first-submit bit under its lock;
                    // a racing submitted create can never become absence.
                    Setup::acknowledge_removed(&self.pool, spec.intent, None).await?;
                    continue;
                }
                let resource = match Effects::retained_resource(&self.pool, spec.intent).await? {
                    Some(resource) => resource,
                    None => {
                        let resource=backend.launcher().prepare(&spec,attune_common::isolated_process::ProcessCreateDisposition::ReconcileOnly).await?;
                        Effects::record_resource(&self.pool, spec.intent, &resource).await?;
                        resource
                    }
                };
                let exit = backend
                    .stop_retained(&spec, &resource, LaunchStopReason::SourceRevoked)
                    .await?;
                Effects::record_exit(&self.pool, spec.intent, &resource, exit).await?;
                backend.remove_stopped(&spec, &resource).await?;
                Setup::acknowledge_removed(&self.pool, spec.intent, Some(&resource)).await?;
            }
        }
    }

    async fn ensure_native_environment(
        &self,
        context: &ExecutionContext,
        action_spec: &ProcessSpec,
        execution: &Execution,
    ) -> Result<PinnedLaunchInput> {
        let native = self.native_preparation.as_ref().ok_or_else(denied)?;
        let backend = self.native_backend.as_ref().ok_or_else(denied)?;
        let master = self.secret_manager.encryption_key().ok_or_else(denied)?;
        let snapshot = execution.executable_snapshot.as_ref().ok_or_else(denied)?;
        let runtime = snapshot.executable.runtime.as_ref().ok_or_else(denied)?;
        let config = context
            .runtime_config_override
            .as_ref()
            .ok_or_else(denied)?;
        if config.lifecycle.is_some() {
            return Err(denied());
        }
        let version = match context.selected_runtime_version.as_deref() {
            Some(v) => Some(
                snapshot
                    .executable
                    .runtime_versions
                    .iter()
                    .find(|r| r.version == v)
                    .ok_or_else(denied)?,
            ),
            None => None,
        };
        let pack = action_spec
            .inputs
            .iter()
            .find(|p| p.kind == LaunchInputKind::Pack)
            .ok_or_else(denied)?;
        let pack_row = attune_common::repositories::pack::PackRepository::find_by_ref(
            &self.pool,
            &snapshot.executable.action.pack_ref,
        )
        .await?
        .ok_or_else(denied)?;
        // Only published configuration and immutable public build inputs. Never
        // hash rendered private environment, parameters or a SetupPlan.
        let engine = match &native.config.backend {
            attune_common::workload_config::WorkloadBackendConfig::Docker { binding, .. } => {
                Some(binding)
            }
            attune_common::workload_config::WorkloadBackendConfig::Kubernetes { .. } => None,
        };
        let binding = attune_common::runtime_cache::sha256_bytes(&serde_json::to_vec(
            &serde_json::json!({
                "image":action_spec.image,"platform":action_spec.platform,"engine":engine,"pack":pack,
                "runner":native.runner,"runtime":runtime.id,"version":version.map(|v|v.id),"config":config,
            }),
        )?);
        if let Some(pin) = Setup::published(&self.pool, &binding).await? {
            if native.cache.open_input_archive(&pin).is_ok() {
                let (spec, resource) = Setup::retained_output(&self.pool, &pin, master).await?;
                self.finish_native_setup_removal(&spec, &resource).await?;
                return Ok(pin);
            }
        }
        let manager = PackEnvironmentManager::new(self.pool.clone());
        let expected = manager
            .upsert_native_environment(
                UpsertCoordinatedEnvironmentInput {
                    pack_id: pack_row.id,
                    pack_ref: &pack_row.r#ref,
                    runtime_id: runtime.id,
                    runtime_ref: &runtime.r#ref,
                    runtime_version: version,
                    env_path: std::path::Path::new(RUNTIME_ROOT),
                    manifest_checksum: Some(&snapshot.release.digest),
                },
                &Sha256Digest::parse(&binding)?,
            )
            .await?;
        if expected.status == PackEnvironmentStatus::Ready {
            manager
                .mark_coordinated_environment_outdated(&expected.env_key)
                .await?;
        }
        let worker = self.worker_id.load(Ordering::Acquire);
        let instance = *self.process_incarnation.get().ok_or_else(denied)?;
        let claim_wait = std::time::Instant::now();
        let claimed = loop {
            if claim_wait.elapsed().as_secs() >= context.timeout.unwrap_or(300) {
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
            if context
                .cancel_token
                .as_ref()
                .is_some_and(|t| t.is_cancelled())
            {
                return Err(denied());
            }
            if let Some(pin) = Setup::published(&self.pool, &binding).await? {
                if native.cache.open_input_archive(&pin).is_ok() {
                    let (spec, resource) = Setup::retained_output(&self.pool, &pin, master).await?;
                    self.finish_native_setup_removal(&spec, &resource).await?;
                    return Ok(pin);
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
        let mut setup = action_spec.clone();
        setup.intent = uuid::Uuid::new_v4();
        setup.purpose = LaunchPurpose::Setup;
        setup.target = LaunchTarget::Setup {
            admission: claimed.id,
            attempt: claim,
        };
        setup.stage = Some(PrivateStage {
            cache_key: binding,
            quota_bytes: native.config.storage.expanded_bytes,
            quota_files: native.config.storage.entries,
        });
        setup.invocation = LaunchInvocation {
            argv: vec![
                crate::runtime::launch_planning::runner_program(setup.platform),
                "setup".into(),
            ],
            env: Default::default(),
            working_root: LaunchRoot::Pack,
        };
        setup.timeout_ms = context
            .timeout
            .map(|s| s.checked_mul(1000).ok_or_else(denied))
            .transpose()?;
        let setup = Setup::reserve(
            &self.pool, &claimed, worker, instance, &setup, &plan, master,
        )
        .await?;
        let launcher = backend.launcher();
        let create = Setup::begin_create(&self.pool, setup.intent, master).await?;
        let resource = launcher.prepare(&setup, create).await?;
        Effects::record_resource(&self.pool, setup.intent, &resource).await?;
        let mut process = launcher.start(&setup, &resource).await?;
        if process.resource != resource {
            process
                .control
                .stop(LaunchStopReason::LaunchFailure)
                .await?;
            return Err(denied());
        }
        process.control = Arc::new(RecordingProcessControl {
            inner: process.control,
            pool: self.pool.clone(),
            intent: setup.intent,
            resource: resource.clone(),
        });
        let payload = match Setup::consume_stdin(&self.pool, setup.intent, &resource, master).await
        {
            Ok(value) => value,
            Err(error) => {
                process
                    .control
                    .stop(LaunchStopReason::SourceRevoked)
                    .await?;
                return Err(error);
            }
        };
        let cancel = context
            .cancel_token
            .clone()
            .unwrap_or_default()
            .child_token();
        let future = crate::runtime::process_executor::execute_isolated_cancellable(
            process,
            payload,
            crate::runtime::process_executor::IsolatedCaptureOptions {
                timeout_secs: context.timeout,
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
                        || Setup::validate_source(&self.pool,setup.intent,master).await.is_err() {
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
        let ceiling = native.config.storage;
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
        Setup::publish(&self.pool, setup.intent, &resource, &pin, master).await?;
        self.finish_native_setup_removal(&setup, &resource).await?;
        Ok(pin)
    }
}
