//! Coordinated legacy environment preparation through the same native effects.
//! Lifecycle recipes deliberately retain their separate private generations.
use super::*;
use attune_common::{
    isolated_process::ProcessSpec, pack_environment::UpsertCoordinatedEnvironmentInput,
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
        let native = self.native_preparation.as_ref().ok_or_else(denied)?;
        let configured = attune_workload_backend::configuration::ConfiguredBackend {
            backend: self.native_backend.as_ref().ok_or_else(denied)?.clone(),
            cache: native.cache.clone(),
            runner: native.runner.clone(),
        };
        crate::native_environment::NativeEnvironmentInstaller::new(
            &self.pool,
            self.worker_id.load(Ordering::Acquire),
            *self.process_incarnation.get().ok_or_else(denied)?,
            &native.config,
            &configured,
            self.secret_manager.encryption_key().ok_or_else(denied)?,
        )?
        .install(input, config, inputs, timeout, cancel)
        .await
    }
}
