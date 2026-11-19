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
        if config.lifecycle.is_some() {
            return self.prepare_isolated_recipe(context, spec, execution);
        }
        if action.launch.is_some()
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
        let mut selected: std::collections::BTreeMap<String, String> = context
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

    /// Called only after the owning action producer proves the exact release,
    /// source attempt and public-input origins. Recipes never enter the shared
    /// legacy environment installer or run a manager-side probe.
    pub(super) fn prepare_isolated_recipe(
        &self,
        context: &ExecutionContext,
        spec: &mut ProcessSpec,
        execution: &Execution,
    ) -> Result<()> {
        let config = context
            .runtime_config_override
            .as_ref()
            .ok_or_else(denied)?;
        let recipe = config.lifecycle.as_ref().ok_or_else(denied)?;
        recipe.validate().map_err(|_| denied())?;
        if !self
            .child_environment
            .admits_native_runtime_recipe(&recipe.digest().map_err(|_| denied())?)
        {
            return Err(denied());
        }
        let snapshot = execution.executable_snapshot.as_ref().ok_or_else(denied)?;
        let action = &snapshot.executable.action;
        if action.launch.is_some()
            || context.code.is_some()
            || action.parameter_delivery != attune_common::models::ParameterDelivery::Stdin
            || action.parameter_format != attune_common::models::ParameterFormat::Json
            || spec.pack_access != PackAccess::ReadOnly
            || spec.stage.is_some()
            || spec
                .inputs
                .iter()
                .any(|p| p.kind == LaunchInputKind::Runtime)
        {
            return Err(denied());
        }
        let relative =
            attune_common::artifact_transport::ValidatedRelativePath::new(&context.entry_point)?;
        spec.invocation.argv = crate::runtime::launch_planning::runtime_recipe_argv(
            spec.platform,
            recipe,
            "/usr/local/bin:/usr/bin:/bin",
            &format!("actions/{}", relative.as_str()),
            false,
            context.selected_runtime_version.as_deref(),
        )
        .map_err(|_| denied())?;
        let mut selected: std::collections::BTreeMap<String, String> = context
            .env
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        spec.invocation.env = crate::runtime::launch_planning::runtime_recipe_environment(&{
            selected.extend(
                context
                    .execution_env
                    .iter()
                    .map(|(k, v)| (k.clone(), v.clone())),
            );
            selected
        })
        .map_err(|_| denied())?;
        spec.invocation.working_root = LaunchRoot::Pack;
        spec.timeout_ms = Some(
            context
                .timeout
                .unwrap_or(recipe.launch.timeout_seconds)
                .checked_mul(1000)
                .filter(|v| *v > 0)
                .ok_or_else(denied)?,
        );
        Ok(())
    }

    /// Select and materialize the primary release through the existing action
    /// input path; the shared driver independently verifies its source proof.
    pub(crate) async fn probe_registered_native_runtime(
        &self,
        runtime: &attune_common::models::Runtime,
        version: Option<&attune_common::models::RuntimeVersion>,
        admitted: &std::collections::BTreeSet<String>,
    ) -> Result<attune_common::repositories::runtime_probe_process::VerifiedToolMap> {
        let native = self.native_preparation.as_ref().ok_or_else(denied)?;
        let backend = self.native_backend.as_ref().ok_or_else(denied)?;
        let master = self.secret_manager.encryption_key().ok_or_else(denied)?;
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
        let pin = self.prepare_native_pack(release).await?;
        let configured = attune_workload_backend::configuration::ConfiguredBackend {
            backend: backend.clone(),
            cache: native.cache.clone(),
            runner: native.runner.clone(),
        };
        crate::native_runtime_probe::NativeRuntimeProbeDriver::new(
            &self.pool,
            self.worker_id.load(Ordering::Acquire),
            *self.process_incarnation.get().ok_or_else(denied)?,
            &native.config,
            &configured,
            master,
            &self.child_environment,
        )?
        .verify(runtime, version, admitted, &pin)
        .await
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
            let debt =
                Setup::unsettled_with_resources(&self.pool, environment, None, master).await?;
            if debt.is_empty() {
                return Ok(());
            }
            for (spec, submitted, retained) in debt {
                if !submitted {
                    Effects::request_stop(&self.pool, spec.intent).await?;
                    // The repository rechecks the create latch under its lock.
                    Setup::acknowledge_removed(&self.pool, spec.intent, None).await?;
                    continue;
                }
                let newly_observed = retained.is_none();
                let resource=match retained {
                    Some(resource)=>resource,
                    None=>backend.launcher().prepare(&spec,attune_common::isolated_process::ProcessCreateDisposition::ReconcileOnly).await?,
                };
                let fence = async {
                    if newly_observed {
                        Effects::record_resource(&self.pool, spec.intent, &resource).await?;
                    }
                    Effects::request_stop(&self.pool, spec.intent).await
                };
                let (fenced, stopped) = tokio::join!(
                    fence,
                    backend.stop_retained(&spec, &resource, LaunchStopReason::SourceRevoked)
                );
                fenced?;
                let exit = stopped?;
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
        if action_spec.image != native.config.image
            || action_spec.platform != native.config.platform
        {
            return Err(denied());
        }
        let snapshot = execution.executable_snapshot.as_ref().ok_or_else(denied)?;
        let runtime = snapshot.executable.runtime.as_ref().ok_or_else(denied)?;
        let config = context
            .runtime_config_override
            .as_ref()
            .ok_or_else(denied)?;
        let version = match context.selected_runtime_version.as_deref() {
            Some(version) => Some(
                snapshot
                    .executable
                    .runtime_versions
                    .iter()
                    .find(|v| v.version == version)
                    .ok_or_else(denied)?,
            ),
            None => None,
        };
        let pack = attune_common::repositories::pack::PackRepository::find_by_ref(
            &self.pool,
            &snapshot.executable.action.pack_ref,
        )
        .await?
        .ok_or_else(denied)?;
        self.install_native_environment(
            UpsertCoordinatedEnvironmentInput {
                pack_id: pack.id,
                pack_ref: &pack.r#ref,
                runtime_id: runtime.id,
                runtime_ref: &runtime.r#ref,
                runtime_version: version,
                env_path: std::path::Path::new(RUNTIME_ROOT),
                manifest_checksum: Some(&snapshot.release.digest),
            },
            config,
            action_spec
                .inputs
                .iter()
                .filter(|p| p.kind != LaunchInputKind::Runtime)
                .cloned()
                .collect(),
            context.timeout,
            context.cancel_token.as_ref(),
        )
        .await
        .map(|(pin, _)| pin)
    }

    /// Existing startup/registration loops select the runtime/version and source.
    /// This shares the on-demand installer; it never probes manager executables.
    pub(crate) async fn prepare_registered_native_environment(
        &self,
        input: UpsertCoordinatedEnvironmentInput<'_>,
        config: &RuntimeExecutionConfig,
        release_digest: &str,
    ) -> Result<bool> {
        let native = self.native_preparation.as_ref().ok_or_else(denied)?;
        let pack = attune_common::repositories::pack::PackRepository::find_by_id(
            &self.pool,
            input.pack_id,
        )
        .await?
        .filter(|p| p.r#ref == input.pack_ref)
        .ok_or_else(denied)?;
        let release = attune_common::repositories::PackReleaseRepository::find_by_id(
            &self.pool,
            pack.active_release.ok_or_else(denied)?,
        )
        .await?
        .filter(|r| r.pack == pack.id && r.pack_ref == input.pack_ref && r.digest == release_digest)
        .ok_or_else(denied)?;
        let pack_pin = self.prepare_native_pack(release).await?;
        self.install_native_environment(
            UpsertCoordinatedEnvironmentInput {
                env_path: std::path::Path::new(RUNTIME_ROOT),
                manifest_checksum: Some(release_digest),
                ..input
            },
            config,
            vec![pack_pin, native.runner.clone()],
            None,
            None,
        )
        .await
        .map(|(_, installed)| installed)
    }

    async fn install_native_environment(
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
        let native = self.native_preparation.as_ref().ok_or_else(denied)?;
        let backend = self.native_backend.as_ref().ok_or_else(denied)?;
        let master = self.secret_manager.encryption_key().ok_or_else(denied)?;
        let pack = inputs
            .iter()
            .find(|p| p.kind == LaunchInputKind::Pack)
            .ok_or_else(denied)?;
        // Only published configuration and immutable public build inputs. Never
        // hash rendered private environment, parameters or a SetupPlan.
        let engine = match &native.config.backend {
            attune_common::workload_config::WorkloadBackendConfig::Docker { binding, .. } => {
                Some(binding)
            }
            attune_common::workload_config::WorkloadBackendConfig::Kubernetes { .. } => None,
        };
        let mut public_binding = serde_json::json!({
            "image":native.config.image,"platform":native.config.platform,"engine":engine,"pack":pack,
            "runner":native.runner,"runtime":input.runtime_id,"version":input.runtime_version.map(|v|v.id),"config":config,
        });
        public_binding.sort_all_objects();
        let binding =
            attune_common::runtime_cache::sha256_bytes(&serde_json::to_vec(&public_binding)?);
        if let Some(pin) = Setup::published(&self.pool, &binding).await? {
            if native.cache.open_input_archive(&pin).is_ok() {
                let (spec, resource) = Setup::retained_output(&self.pool, &pin, master).await?;
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
        let worker = self.worker_id.load(Ordering::Acquire);
        let instance = *self.process_incarnation.get().ok_or_else(denied)?;
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
            if let Some(pin) = Setup::published(&self.pool, &binding).await? {
                if native.cache.open_input_archive(&pin).is_ok() {
                    let (spec, resource) = Setup::retained_output(&self.pool, &pin, master).await?;
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
            image: native.config.image.clone(),
            platform: native.config.platform,
            inputs,
            pack_access: PackAccess::ReadOnly,
            stage: Some(PrivateStage {
                cache_key: binding,
                quota_bytes: native.config.storage.expanded_bytes,
                quota_files: native.config.storage.entries,
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
        Ok((pin, true))
    }
}
