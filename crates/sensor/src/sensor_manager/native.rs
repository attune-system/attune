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

pub(super) struct NativeSensor {
    pool: PgPool,
    workload: Id,
    spec: ProcessSpec,
    resource: ResourceIdentity,
    backend: attune_workload_backend::configuration::NativeBackend,
    transport: Arc<dyn ArtifactFileTransport>,
    worker: Id,
    incarnation: uuid::Uuid,
    control: Arc<dyn attune_common::isolated_process::ProcessControl>,
    wait_task: Option<JoinHandle<attune_common::Result<LaunchExitOutcome>>>,
}
impl NativeSensor {
    pub(super) async fn poll_exit(&mut self) -> Result<Option<LaunchExitOutcome>> {
        if !self
            .wait_task
            .as_ref()
            .is_some_and(|task| task.is_finished())
        {
            return Ok(None);
        }
        let exit = self.wait_task.take().expect("finished task").await??;
        Ledger::record_native_exit(
            &self.pool,
            self.workload,
            self.spec.intent,
            &self.resource,
            exit.clone(),
        )
        .await?;
        Ok(Some(exit))
    }
    pub(super) async fn settle_files(&self) -> Result<()> {
        settle_files(
            &self.pool,
            self.workload,
            &self.spec,
            &self.resource,
            &self.backend,
            self.transport.as_ref(),
            self.worker,
            self.incarnation,
        )
        .await
    }
    pub(super) async fn stop(&mut self) -> Result<()> {
        // A DB wait must not keep the known exact payload alive. Both owned
        // futures join before terminal evidence or publication can proceed.
        let exit = attune_common::repositories::sensor_workload::native_effect::join_sensor_stop(
            Ledger::stop_native_effect(&self.pool, self.workload, self.spec.intent),
            self.control.stop(LaunchStopReason::Cancelled),
        )
        .await?;
        Ledger::record_native_exit(
            &self.pool,
            self.workload,
            self.spec.intent,
            &self.resource,
            exit,
        )
        .await?;
        if let Some(task) = self.wait_task.take() {
            task.abort();
            let _ = task.await;
        }
        Ok(())
    }
}
impl SensorManager {
    pub(super) async fn reconcile_native_sensor(&self, sensor: Id) -> Result<()> {
        let Some((workload, effect)) =
            Ledger::unsettled_native_for_sensor(&self.inner.db, sensor, self.inner.worker_instance)
                .await?
        else {
            return Ok(());
        };
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
                if !effect.create_submitted {
                    return Err(anyhow!("Sensor intent retained before create; capture/publication settlement required"));
                }
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
        .verify(runtime, version, admitted, &pin)
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
        let pack = self.prepare_native_pack(release).await?;
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
            let (pin, _) = attune_worker::native_environment::NativeEnvironmentInstaller::new(
                &self.inner.db,
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
                    runtime_version: version,
                    env_path: std::path::Path::new(RUNTIME_ROOT),
                    manifest_checksum: Some(&pack_revision.digest),
                },
                config,
                inputs.clone(),
                None,
                None,
            )
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
            self.probe_native_runtime(runtime, version, &admitted)
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
                "/opt/attune/artifacts".into(),
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
        Ledger::consume_native_effect(
            &self.inner.db,
            sensor.id,
            workload.fence(),
            spec.intent,
            false,
        )
        .await?;
        let launcher = native.backend.launcher();
        let resource = launcher
            .prepare(&spec, ProcessCreateDisposition::CreateOnce)
            .await?;
        Ledger::record_native_resource(
            &self.inner.db,
            workload.workload_id,
            spec.intent,
            &resource,
        )
        .await?;
        let io = launcher.start(&spec, &resource).await?;
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
        self.persist_sensor_process_started(&sensor, None, reset)
            .await;
        let waiter = io.control.clone();
        let wait_task = tokio::spawn(async move { waiter.wait().await });
        let instance = SensorInstance {
            native: Some(NativeSensor {
                pool: self.inner.db.clone(),
                workload: workload.workload_id,
                spec,
                resource,
                backend: native.backend.clone(),
                transport: self
                    .inner
                    .api_client
                    .artifact_transport(self.inner.artifact_transport.base_dir())?,
                worker: workload.worker_id,
                incarnation: workload.worker_instance,
                control: io.control,
                wait_task: Some(wait_task),
            }),
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
            stdout_handle: Some(stdout),
            stderr_handle: Some(stderr),
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
