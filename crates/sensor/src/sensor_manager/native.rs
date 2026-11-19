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
}
impl Context {
    fn spec(self) -> ProcessSpec {
        ProcessSpec {
            intent: self.intent,
            purpose: LaunchPurpose::Sensor,
            target: self.target,
            image: self.image,
            platform: self.platform,
            inputs: self.inputs,
            invocation: self.invocation,
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
    pub(super) async fn stop(&mut self) -> Result<()> {
        // Run is durably fenced before the provider call, including after revoke.
        Ledger::stop_native_effect(&self.pool, self.workload, self.spec.intent).await?;
        let exit = self.control.stop(LaunchStopReason::Cancelled).await?;
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
        // Capture/file publication is an independent obligation. Do not remove
        // a resource or discard its row merely because the payload exited.
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
        Ledger::stop_native_effect(&self.inner.db, workload, spec.intent).await?;
        if !effect.create_submitted {
            return Err(anyhow!(
                "Sensor intent retained before create; capture/publication settlement required"
            ));
        }
        let resource = match effect.resource {
            Some(resource) => resource,
            None => {
                let resource = native
                    .backend
                    .launcher()
                    .prepare(&spec, ProcessCreateDisposition::ReconcileOnly)
                    .await?;
                Ledger::record_native_resource(&self.inner.db, workload, spec.intent, &resource)
                    .await?;
                resource
            }
        };
        let exit = native
            .backend
            .stop_retained(&spec, &resource, LaunchStopReason::Shutdown)
            .await?;
        Ledger::record_native_exit(&self.inner.db, workload, spec.intent, &resource, exit).await?;
        // Missing capture and file receipts are never invented by restart.
        Err(anyhow!(
            "Stopped sensor retained pending capture/file publication receipts"
        ))
    }

    async fn prepare_native_pack(
        &self,
        release: attune_common::models::PackRelease,
    ) -> Result<attune_common::workload_launch::PinnedLaunchInput> {
        use attune_common::{
            pack_registry::storage::PackReleaseManifest,
            workload_archive::ArchivePolicy,
            workload_launch::{LaunchInputKind, PinnedLaunchInput, Sha256Digest},
            workload_pack_input::{release_zip, ReleaseInputPin},
        };
        let denied = || Error::PermissionDenied("Native pack input unavailable".into());
        let native = self.inner.native.as_ref().ok_or_else(denied)?;
        let manifest: PackReleaseManifest =
            serde_json::from_value(release.manifest).map_err(|_| denied())?;
        if manifest.version != release.version || manifest.pack_ref != release.pack_ref {
            return Err(denied().into());
        }
        // Existing pack transport materializes this primary immutable archive
        // for both volume and API managers. Never repack an unpacked tree.
        let archive = attune_common::pack_transport::release_cache_path(
            &self.inner.packs_base_dir,
            &release.pack_ref,
            &release.digest,
        )?
        .parent()
        .ok_or_else(denied)?
        .join("pack.tar.gz");
        let source = ReleaseInputPin {
            pack_ref: release.pack_ref,
            size_bytes: u64::try_from(release.archive_size).map_err(|_| denied())?,
            sha256: Sha256Digest::parse(&release.digest)?,
        };
        let ceiling = self
            .inner
            .workload_config
            .as_ref()
            .ok_or_else(denied)?
            .storage;
        let runner = native.runner.clone();
        let cache = native.cache.clone();
        let release_id = release.id;
        let release_digest = release.digest;
        let pack = tokio::task::spawn_blocking(move || {
            use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
            let file = std::fs::OpenOptions::new()
                .read(true)
                .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
                .open(archive)
                .map_err(|_| Error::validation("Pinned release archive unavailable"))?;
            let metadata = file
                .metadata()
                .map_err(|_| Error::validation("Pinned release archive unavailable"))?;
            if !metadata.is_file()
                || metadata.nlink() != 1
                || metadata.mode() & 0o022 != 0
                || !(metadata.uid() == 0 || metadata.uid() == unsafe { libc::geteuid() })
            {
                return Err(Error::PermissionDenied(
                    "Pinned release archive ownership invalid".into(),
                ));
            }
            let prepared = release_zip(
                file,
                &source,
                &manifest,
                attune_common::pack_format::Limits {
                    compressed_bytes: ceiling.compressed_bytes,
                    extracted_bytes: ceiling.expanded_bytes,
                    file_bytes: ceiling.expanded_bytes,
                    entries: ceiling.entries,
                    ..Default::default()
                },
                &ArchivePolicy {
                    root: LaunchInputKind::Pack,
                    max_compressed_bytes: ceiling.compressed_bytes,
                    max_expanded_bytes: ceiling.expanded_bytes,
                    max_files: ceiling.entries,
                },
            )?;
            let pin = PinnedLaunchInput {
                // Pin IDs distinguish slots inside this one plan. Object ID and
                // version come exclusively from the retained release record;
                // the public Runner's operator pin is preserved unchanged.
                pin_id: if runner.pin_id == 1 { 2 } else { 1 },
                kind: LaunchInputKind::Pack,
                object_id: release_id,
                object_version: release_digest,
                size_bytes: prepared.size_bytes(),
                sha256: prepared.sha256().clone(),
                max_expanded_bytes: prepared.expanded_bytes(),
                max_files: prepared.entries(),
            };
            let mut body = prepared.into_file();
            cache
                .install_input(&pin, body.as_file_mut())
                .map_err(|_| Error::validation("Pinned release ZIP installation failed"))?;
            Ok::<_, Error>(pin)
        })
        .await
        .map_err(|_| Error::invalid_state("Native release preparation did not complete"))??;
        Ok(pack)
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
        // The shared private-generation producer must integrate before recipes
        // and environment setup can be admitted. Never run their local effects.
        if attune_workload_backend::planning::needs_runtime(config) {
            return Err(anyhow!(
                "Sensor runtime requires the native private-generation producer"
            ));
        }
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
        // This constructor currently enforces the shared Runner bound. Fail
        // before resource creation if the purpose-bound 32 MiB hook is absent.
        let payload = attune_common::workload_stdin::LaunchStdinPayload::from_bytes(bootstrap)?;
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
        let entry = format!("sensors/{}", sensor.entrypoint);
        let argv = if matches!(config.interpreter.binary.as_str(), "" | "native" | "none") {
            vec![attune_workload_backend::planning::native_program(
                &entry,
                pack_revision.path.join(&entry).is_dir(),
                operator.platform,
            )?]
        } else {
            attune_workload_backend::planning::interpreted_argv(
                config,
                &entry,
                false,
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
        let environment = attune_workload_backend::planning::action_environment(
            config,
            &selected,
            &entry,
            false,
            PackAccess::ReadOnly,
        )?;
        let context = Context {
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
            inputs: vec![pack, native.runner.clone()],
            invocation: LaunchInvocation {
                argv,
                env: environment,
                working_root: LaunchRoot::Pack,
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
