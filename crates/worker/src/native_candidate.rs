//! Direct native candidate process effects. The existing TestExecutor retains
//! discovery, parser and policy; the existing handler owns install publication
//! and the opt-in candidate API lease. Nothing here launches a host process.
use attune_common::{
    auth::WorkerTokenProvider,
    child_process_environment::ChildProcessEnvironment,
    isolated_process::{ProcessReader, ProcessSpec},
    repositories::{
        candidate_process::{
            CandidateCapture, CandidateEffectsSource, CandidateProcessRepository as Ledger,
            CandidateSource,
        },
        execution_process::ExecutionProcessRepository as Effects,
    },
    test_executor::{
        CommandOutput, TestConfig, TestExecutor, TestProcessEffect, TestProcessInterruption,
        TestProcessRequest,
    },
    workload_config::WorkloadConfig,
    workload_launch::*,
    Error, Result,
};
use attune_workload_backend::configuration::{ConfiguredBackend, NativeBackend};
use attune_workload_backend::recipe_timeout::{self, RecipeTimeoutChallenge};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::File,
    io::{Read, Seek, SeekFrom, Write},
    path::{Component, Path, PathBuf},
    sync::Arc,
};
use tokio::io::AsyncWriteExt;
use uuid::Uuid;
fn denied() -> Error {
    Error::validation("Native candidate process unavailable or incomplete")
}
/// Selected immutable dependency input. The owning manager admits this exact
/// published runtime; tests receive only its read-only guest mount.
pub struct CandidateRuntime {
    pub pin: PinnedLaunchInput,
    pub interpreter: String,
}
pub struct NativeCandidateTests {
    pool: sqlx::PgPool,
    source: CandidateSource,
    config: WorkloadConfig,
    backend: NativeBackend,
    pack: PinnedLaunchInput,
    runtime: Option<CandidateRuntime>,
    testing: TestConfig,
    environment: ChildProcessEnvironment,
    master: String,
    api_url: String,
    // Private extracted metadata is for TestExecutor's existing validation only.
    // Payload effects always consume the digest-bound cache input, never this tree.
    _root: tempfile::TempDir,
    pack_dir: PathBuf,
}
impl NativeCandidateTests {
    #[allow(clippy::too_many_arguments)]
    pub async fn new(
        pool: sqlx::PgPool,
        config: WorkloadConfig,
        configured: ConfiguredBackend,
        worker: i64,
        instance: Uuid,
        install: i64,
        archive_token: &str,
        tokens: Arc<WorkerTokenProvider>,
        api_url: &str,
        master: String,
        environment: ChildProcessEnvironment,
        runtime: Option<CandidateRuntime>,
    ) -> Result<Arc<Self>> {
        config.validate()?;
        if configured.runner != config.helper {
            return Err(denied());
        }
        let source = Ledger::admit(&pool, install, worker, instance, archive_token).await?;
        let maximum = config.storage.compressed_bytes;
        let expected = u64::try_from(source.archive_size).map_err(|_| denied())?;
        if expected == 0 || expected > maximum {
            return Err(denied());
        }
        let root = tempfile::tempdir().map_err(|_| denied())?;
        let archive = tempfile::NamedTempFile::new_in(root.path()).map_err(|_| denied())?;
        let mut response = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(std::time::Duration::from_secs(120))
            .build()
            .map_err(|_| denied())?
            .get(format!(
                "{}/api/v1/internal/pack-installs/{install}/archive",
                api_url.trim_end_matches('/')
            ))
            .bearer_auth(tokens.token().map_err(|_| denied())?)
            .header("x-attune-pack-candidate-token", archive_token)
            .send()
            .await
            .map_err(|_| denied())?;
        if !response.status().is_success()
            || response.content_length().is_some_and(|s| s != expected)
            || response
                .headers()
                .get("x-attune-pack-candidate-sha256")
                .and_then(|v| v.to_str().ok())
                != Some(source.archive_digest.as_str())
        {
            return Err(denied());
        }
        let mut output = tokio::fs::File::from_std(archive.reopen().map_err(|_| denied())?);
        let mut size = 0u64;
        use sha2::Digest;
        let mut digest = sha2::Sha256::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| denied())? {
            size = size
                .checked_add(chunk.len() as u64)
                .filter(|s| *s <= expected)
                .ok_or_else(denied)?;
            digest.update(&chunk);
            output.write_all(&chunk).await.map_err(|_| denied())?;
        }
        output.sync_all().await.map_err(|_| denied())?;
        drop(output);
        let hash = digest
            .finalize()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>();
        if size != expected || hash != source.archive_digest {
            return Err(denied());
        }
        Ledger::validate_source(&pool, &source).await?;
        let cache = configured.cache.clone();
        let limits = config.storage;
        let pack_ref = source.pack_ref.clone();
        let directory = root.path().join("files");
        let id = source.install;
        let version = source.archive_digest.clone();
        let used = [
            config.helper.pin_id,
            runtime.as_ref().map(|r| r.pin.pin_id).unwrap_or(0),
        ];
        if used[0] == used[1] {
            return Err(denied());
        }
        let pack_pin_id = (1..=3).find(|id| !used.contains(id)).ok_or_else(denied)?;
        let identity = CandidateInputIdentity {
            install: id,
            pin_id: pack_pin_id,
            version,
        };
        let (root, pack, pack_dir) = tokio::task::spawn_blocking(move || {
            let (pin, dir) =
                convert_candidate(archive, &pack_ref, &directory, identity, limits, &cache)?;
            Ok::<_, Error>((root, pin, dir))
        })
        .await
        .map_err(|_| denied())??;
        let yaml = tokio::fs::read(pack_dir.join("pack.yaml"))
            .await
            .map_err(|_| denied())?;
        let metadata: serde_yaml_ng::Value =
            serde_yaml_ng::from_slice(&yaml).map_err(|_| denied())?;
        let testing: TestConfig =
            serde_yaml_ng::from_value(metadata.get("testing").cloned().ok_or_else(denied)?)
                .map_err(|_| denied())?;
        testing.validate_policy()?;
        if let Some(runtime) = &runtime {
            if runtime.pin.kind != LaunchInputKind::Runtime
                || !runtime.interpreter.starts_with(&format!("{RUNTIME_ROOT}/"))
            {
                return Err(denied());
            }
            attune_common::artifact_transport::ValidatedRelativePath::new(
                runtime
                    .interpreter
                    .strip_prefix(&format!("{RUNTIME_ROOT}/"))
                    .ok_or_else(denied)?,
            )?;
            configured
                .cache
                .open_input_archive(&runtime.pin)
                .map_err(|_| denied())?;
        }
        Ledger::validate_source(&pool, &source).await?;
        Ok(Arc::new(Self {
            pool,
            source,
            config,
            backend: configured.backend,
            pack,
            runtime,
            testing,
            environment,
            master,
            api_url: api_url.trim_end_matches('/').into(),
            _root: root,
            pack_dir: pack_dir.canonicalize().map_err(|_| denied())?,
        }))
    }
    pub fn pack_dir(&self) -> &Path {
        &self.pack_dir
    }
    pub fn testing(&self) -> &TestConfig {
        &self.testing
    }
    pub fn attach(self: &Arc<Self>, executor: TestExecutor) -> TestExecutor {
        executor.with_process_effect(self.clone())
    }
    /// Call after process_owner.shutdown, BEFORE lease close, terminal install
    /// updates or candidate-root cleanup. False/Err retains all owning resources.
    pub async fn settled(&self) -> Result<()> {
        Ledger::settle_unsubmitted(&self.pool, &self.source).await?;
        if !Ledger::settled(&self.pool, &self.source, &self.master).await? {
            return Err(denied());
        }
        Ok(())
    }
    /// The handler must retain this transaction through candidate lease close,
    /// result writes and install completion, then check fence freshness and
    /// COMMIT. `settled` alone authorizes cleanup, never result disclosure.
    pub async fn lock_result(
        &self,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    ) -> Result<attune_common::repositories::candidate_process::CandidateResultFence> {
        Ledger::lock_result(tx, &self.source, &self.master).await
    }
    async fn invoke(&self, request: TestProcessRequest) -> Result<CommandOutput> {
        let configured = self
            .testing
            .runners
            .get(&request.suite)
            .ok_or_else(denied)?;
        if serde_json::to_value(configured)? != serde_json::to_value(&request.runner)?
            || request.candidate_effects.is_some() != self.testing.candidate_api_effects
        {
            return Err(denied());
        }
        let entry = request.entry_point.to_str().ok_or_else(denied)?;
        if request
            .entry_point
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
            || request.entry_point
                != Path::new(&configured.entry_point)
                    .components()
                    .filter(|c| !matches!(c, Component::CurDir))
                    .collect::<PathBuf>()
        {
            return Err(denied());
        }
        let suite = self
            .testing
            .runners
            .keys()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .position(|v| v == &request.suite)
            .ok_or_else(denied)? as i64
            + 1;
        let mut source = self.source.clone();
        let candidate_effects = request.candidate_effects;
        if let Some(effect) = &candidate_effects {
            let prefix = format!("{}/api/v1/candidates/", self.api_url);
            let lease = effect
                .url
                .strip_prefix(&prefix)
                .and_then(|s| s.strip_suffix("/effects"))
                .and_then(|v| Uuid::parse_str(v).ok())
                .ok_or_else(denied)?;
            source.effects = Some(CandidateEffectsSource {
                lease,
                token_hash: attune_common::auth::hash_integration_token(&effect.token),
            });
        }
        let target = LaunchTarget::CandidateTest {
            install: source.install,
            test_job: suite,
            attempt: source.instance,
        };
        let retained = Ledger::retained(&self.pool, &source, &target, &self.master).await?;
        // Recover from the exact old spec before ordinary environment planning
        // replaces it. Never mint new evidence for an existing older intent.
        let challenge =
            retained_recipe_challenge(retained.as_ref(), request.runner.lifecycle.is_some())?;
        let mut env = BTreeMap::from([
            ("PATH".into(), "/usr/local/bin:/usr/bin:/bin".into()),
            ("HOME".into(), PACK_ROOT.into()),
            ("LANG".into(), "C.UTF-8".into()),
            ("LC_ALL".into(), "C.UTF-8".into()),
        ]);
        if let Some(effect) = candidate_effects {
            env.insert("ATTUNE_CANDIDATE_EFFECTS_URL".into(), effect.url);
            env.insert("ATTUNE_CANDIDATE_EFFECTS_TOKEN".into(), effect.token);
        }
        let guest = format!("{PACK_ROOT}/{entry}");
        let mut inputs = vec![self.pack.clone(), self.config.helper.clone()];
        if request.runner.lifecycle.is_none() {
            if let Some(runtime) = &self.runtime {
                inputs.push(runtime.pin.clone());
                let bin = Path::new(&runtime.interpreter)
                    .parent()
                    .and_then(Path::to_str)
                    .ok_or_else(denied)?;
                env.insert("PATH".into(), format!("{bin}:/usr/local/bin:/usr/bin:/bin"));
            }
        }
        let argv = if let Some(recipe) = &request.runner.lifecycle {
            if !self
                .environment
                .admits_native_runtime_recipe(&recipe.digest().map_err(|_| denied())?)
            {
                return Err(denied());
            }
            for name in &recipe.permitted_caller_env {
                if let Some(value) = self.environment.get(name) {
                    env.insert(name.clone(), value.to_str().ok_or_else(denied)?.to_owned());
                }
            }
            let argv = attune_workload_backend::planning::candidate_runtime_recipe_argv(
                self.config.platform,
                recipe,
                "/usr/local/bin:/usr/bin:/bin",
                entry,
                false,
                None,
            )
            .map_err(|_| denied())?;
            env = attune_workload_backend::planning::runtime_recipe_environment(&env)
                .map_err(|_| denied())?;
            argv
        } else {
            match request.runner.r#type.as_str() {
                "script" => vec![
                    if Path::new(entry).extension().and_then(|s| s.to_str()) == Some("sh") {
                        "/bin/sh"
                    } else {
                        "/bin/bash"
                    }
                    .into(),
                    guest,
                ],
                "unittest" | "pytest" => {
                    // Legacy Python tests require their private dependency
                    // environment. An image interpreter cannot substitute for
                    // the owning setup producer's selected immutable input.
                    let program = self
                        .runtime
                        .as_ref()
                        .ok_or_else(denied)?
                        .interpreter
                        .clone();
                    let mut args = vec![program, "-m".into(), request.runner.r#type.clone(), guest];
                    if request.runner.r#type == "pytest" {
                        args.push("-v".into())
                    }
                    args
                }
                _ => return Err(denied()),
            }
        };
        let timeout = request.runner.timeout.unwrap_or(300);
        let mut spec = ProcessSpec {
            intent: retained
                .as_ref()
                .map_or_else(Uuid::new_v4, |old| old.intent),
            purpose: LaunchPurpose::CandidateTest,
            target,
            image: self.config.image.clone(),
            platform: self.config.platform,
            inputs,
            pack_access: PackAccess::CandidateCopy,
            stage: None,
            timeout_ms: Some(
                timeout
                    .checked_mul(1000)
                    .filter(|t| *t > 0)
                    .ok_or_else(denied)?,
            ),
            invocation: LaunchInvocation {
                argv,
                env,
                working_root: LaunchRoot::Pack,
            },
        };
        // Recipe environment validation above excludes the reserved proof env;
        // only the fixed helper receives this private intent-bound challenge.
        recipe_timeout::bind_challenge(&mut spec, challenge)?;
        let spec = Ledger::reserve(&self.pool, &source, &spec, &self.master).await?;
        if let Some(c) = Ledger::captured(&self.pool, spec.intent, &self.master).await? {
            return command_output(c);
        }
        let started = std::time::Instant::now();
        let budget_started = tokio::time::Instant::now();
        let remaining = Ledger::remaining(&self.pool, spec.intent, &self.master).await?;
        let deadline = budget_started + remaining;
        let disposition = Ledger::begin_create(&self.pool, spec.intent, &self.master).await?;
        let launcher = self.backend.launcher();
        let resource = tokio::time::timeout_at(deadline, launcher.prepare(&spec, disposition))
            .await
            .map_err(|_| Error::Timeout("Candidate create acknowledgement uncertain".into()))??;
        use TestProcessInterruption as Interrupted;
        let mut streams = None;
        // Enclose every DB/attach/stdin/source wait after resource discovery.
        // Cancellation/deadline can therefore progress physical stop even if
        // one of those durable operations is blocked by a database row lock.
        let outcome = {
            let run = async {
                Effects::record_resource(&self.pool, spec.intent, &resource)
                    .await
                    .map_err(|_| Interrupted::ProviderFailure)?;
                Ledger::validate(&self.pool, spec.intent, &self.master)
                    .await
                    .map_err(|_| Interrupted::SourceRevoked)?;
                let io = launcher
                    .start(&spec, &resource)
                    .await
                    .map_err(|_| Interrupted::ProviderFailure)?;
                let control = io.control;
                streams = Some((
                    tokio::spawn(read_capture(io.stdout)),
                    tokio::spawn(read_capture(io.stderr)),
                ));
                let payload =
                    Ledger::consume_stdin(&self.pool, spec.intent, &resource, &self.master)
                        .await
                        .map_err(|_| Interrupted::InputUncertain)?;
                control
                    .write_stdin_once(payload)
                    .await
                    .map_err(|_| Interrupted::InputUncertain)?;
                let source_watch = async {
                    loop {
                        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                        if Ledger::validate(&self.pool, spec.intent, &self.master)
                            .await
                            .is_err()
                        {
                            break Interrupted::SourceRevoked;
                        }
                    }
                };
                tokio::select! {
                    result=control.wait()=>result.map_err(|_|Interrupted::ProviderFailure),
                    invalid=source_watch=>Err(invalid),
                }
            };
            tokio::select! {
                result=run=>result,
                _=request.cancellation.cancelled()=>Err(Interrupted::Cancelled),
                _=tokio::time::sleep_until(deadline)=>Err(Interrupted::TimedOut),
            }
        };
        let (fenced, exit, mut interruption) = match outcome {
            Ok(exit) => (
                Ok(()),
                Ok(exit),
                exit.oom_killed.then_some(Interrupted::OutOfMemory),
            ),
            Err(reason) => {
                let stop_reason = if reason == Interrupted::TimedOut {
                    LaunchStopReason::Timeout
                } else {
                    LaunchStopReason::Cancelled
                };
                let durable = async {
                    // Record the exact identity even when the interrupted
                    // record-resource COMMIT acknowledgement was lost.
                    Effects::record_resource(&self.pool, spec.intent, &resource).await?;
                    Effects::request_stop(&self.pool, spec.intent).await
                };
                let (fenced, stopped) = tokio::join!(
                    durable,
                    self.backend.stop_retained(&spec, &resource, stop_reason)
                );
                (fenced, stopped, Some(reason))
            }
        };
        // Both readers join even when either fence/physical stop fails. Missing
        // attach/capture remains incomplete; no EOF or empty output is invented.
        let (out, err) = if let Some((stdout, stderr)) = streams {
            tokio::join!(join_capture(stdout), join_capture(stderr))
        } else {
            (Err(denied()), Err(denied()))
        };
        let exit = exit?;
        fenced?;
        Effects::record_exit(&self.pool, spec.intent, &resource, exit).await?;
        let stdout = out?;
        let stderr = err?;
        Effects::request_stop(&self.pool, spec.intent).await?;
        if recipe_timeout::observed_timeout(&self.backend, &spec, &resource).await? {
            interruption = Some(Interrupted::TimedOut);
        }
        self.backend.remove_stopped(&spec, &resource).await?;
        Ledger::acknowledge_removed(&self.pool, spec.intent, &resource).await?;
        let capture = CandidateCapture {
            interruption,
            exit_code: exit.exit_code,
            stdout,
            stderr,
            duration_ms: started
                .elapsed()
                .as_millis()
                .try_into()
                .map_err(|_| denied())?,
        };
        Ledger::capture(&self.pool, spec.intent, &resource, &capture, &self.master).await?;
        command_output(capture)
    }
}
fn retained_recipe_challenge(
    retained: Option<&ProcessSpec>,
    recipe: bool,
) -> Result<Option<RecipeTimeoutChallenge>> {
    let challenge = retained
        .map(recipe_timeout::retained_challenge)
        .transpose()?
        .flatten();
    if recipe && retained.is_some() && challenge.is_none() {
        // Older retained intents may still need physical cleanup, but cannot
        // acquire new timeout evidence or restart by being reconstructed here.
        return Err(denied());
    }
    Ok(challenge)
}
#[async_trait::async_trait]
impl TestProcessEffect for NativeCandidateTests {
    async fn run(&self, request: TestProcessRequest) -> Result<CommandOutput> {
        self.invoke(request).await
    }
}
fn command_output(c: CandidateCapture) -> Result<CommandOutput> {
    Ok(CommandOutput {
        interruption: c.interruption,
        exit_code: c.exit_code,
        stdout: c.stdout,
        stderr: c.stderr,
        duration_ms: c.duration_ms,
    })
}
async fn read_capture(reader: ProcessReader) -> Result<String> {
    // Preserve the shared executor's declared bounded-prefix retention while
    // draining to actual EOF. A read error after the prefix still fails capture.
    TestExecutor::read_stream(reader)
        .await
        .map_err(|_| denied())
}
async fn join_capture(mut task: tokio::task::JoinHandle<Result<String>>) -> Result<String> {
    match tokio::time::timeout(std::time::Duration::from_secs(5), &mut task).await {
        Ok(result) => result.map_err(|_| denied())?,
        Err(_) => {
            task.abort();
            let _ = task.await;
            Err(denied())
        }
    }
}

/// Only the candidate API's actual gzip-TAR contract is accepted. No release
/// manifest or unpacked manager tree is used as input authority.
struct CandidateInputIdentity {
    install: i64,
    pin_id: i64,
    version: String,
}
fn convert_candidate(
    archive: tempfile::NamedTempFile,
    pack_ref: &str,
    directory: &Path,
    identity: CandidateInputIdentity,
    limits: attune_common::workload_config::WorkloadStorageLimits,
    cache: &attune_workload_backend::cache::CacheStore,
) -> Result<(PinnedLaunchInput, PathBuf)> {
    attune_common::schema::RefValidator::validate_pack_ref(pack_ref)?;
    std::fs::create_dir(directory).map_err(|_| denied())?;
    let mut zip_file = tempfile::NamedTempFile::new().map_err(|_| denied())?;
    let expanded_budget = limits
        .expanded_bytes
        .checked_add(limits.entries.checked_mul(8192).ok_or_else(denied)?)
        .and_then(|v| v.checked_add(1024 * 1024))
        .ok_or_else(denied)?;
    let decoder = flate2::read::GzDecoder::new(archive.reopen().map_err(|_| denied())?);
    let mut tar = tar::Archive::new(BudgetReader {
        inner: decoder,
        remaining: expanded_budget,
    });
    let mut zip = zip::ZipWriter::new(BudgetWriter {
        file: zip_file.as_file_mut(),
        maximum: limits.compressed_bytes,
    });
    let mut seen = BTreeSet::new();
    let mut entries = 0u64;
    let mut expanded = 0u64;
    for entry in tar.entries().map_err(|_| denied())? {
        let mut entry = entry.map_err(|_| denied())?;
        entries += 1;
        if entries > limits.entries {
            return Err(denied());
        }
        let path = entry.path().map_err(|_| denied())?.into_owned();
        if path
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
        {
            return Err(denied());
        }
        let relative = path.strip_prefix(pack_ref).map_err(|_| denied())?;
        let kind = entry.header().entry_type();
        if !kind.is_file() && !kind.is_dir() {
            return Err(denied());
        }
        if relative.as_os_str().is_empty() {
            if !kind.is_dir() {
                return Err(denied());
            }
            continue;
        }
        let name = relative.to_str().ok_or_else(denied)?.to_owned();
        if !seen.insert(name.clone()) {
            return Err(denied());
        }
        let target = directory.join(relative);
        if kind.is_dir() {
            std::fs::create_dir_all(&target).map_err(|_| denied())?;
            zip.add_directory(
                format!("{name}/"),
                zip::write::SimpleFileOptions::default().unix_permissions(0o755),
            )
            .map_err(|_| denied())?;
            continue;
        }
        let size = entry.size();
        expanded = expanded
            .checked_add(size)
            .filter(|v| *v <= limits.expanded_bytes)
            .ok_or_else(denied)?;
        if size > attune_common::config::PackUploadConfig::DEFAULT_MAX_PER_ENTRY_SIZE_BYTES {
            return Err(denied());
        }
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(|_| denied())?;
        }
        let mode = if entry.header().mode().map_err(|_| denied())? & 0o111 != 0 {
            0o755
        } else {
            0o644
        };
        zip.start_file(
            name,
            zip::write::SimpleFileOptions::default().unix_permissions(mode),
        )
        .map_err(|_| denied())?;
        let mut local = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&target)
            .map_err(|_| denied())?;
        let mut copied = 0u64;
        let mut chunk = [0; 65536];
        loop {
            let count = entry.read(&mut chunk).map_err(|_| denied())?;
            if count == 0 {
                break;
            }
            copied += count as u64;
            if copied > size {
                return Err(denied());
            }
            zip.write_all(&chunk[..count]).map_err(|_| denied())?;
            local.write_all(&chunk[..count]).map_err(|_| denied())?;
        }
        if copied != size {
            return Err(denied());
        }
        local.sync_all().map_err(|_| denied())?;
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(mode))
            .map_err(|_| denied())?;
    }
    zip.finish().map_err(|_| denied())?;
    zip_file.as_file().sync_all().map_err(|_| denied())?;
    let size = zip_file.as_file().metadata().map_err(|_| denied())?.len();
    zip_file
        .as_file_mut()
        .seek(SeekFrom::Start(0))
        .map_err(|_| denied())?;
    use sha2::Digest;
    let mut digest = sha2::Sha256::new();
    let mut chunk = [0; 65536];
    loop {
        let count = zip_file.read(&mut chunk).map_err(|_| denied())?;
        if count == 0 {
            break;
        }
        digest.update(&chunk[..count]);
    }
    let hash = digest
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>();
    let pin = PinnedLaunchInput {
        pin_id: identity.pin_id,
        kind: LaunchInputKind::Pack,
        object_id: identity.install,
        object_version: identity.version,
        size_bytes: size,
        sha256: Sha256Digest::parse(hash)?,
        max_expanded_bytes: limits.expanded_bytes,
        max_files: limits.entries,
    };
    zip_file
        .as_file_mut()
        .seek(SeekFrom::Start(0))
        .map_err(|_| denied())?;
    cache
        .install_input(&pin, zip_file.as_file_mut())
        .map_err(|_| denied())?;
    Ok((pin, directory.to_path_buf()))
}
struct BudgetReader<R> {
    inner: R,
    remaining: u64,
}
impl<R: Read> Read for BudgetReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        if self.remaining == 0 {
            let mut probe = [0];
            return if self.inner.read(&mut probe)? == 0 {
                Ok(0)
            } else {
                Err(std::io::Error::other("candidate archive budget exceeded"))
            };
        }
        let bound = buf
            .len()
            .min(self.remaining.try_into().unwrap_or(usize::MAX));
        let n = self.inner.read(&mut buf[..bound])?;
        self.remaining -= n as u64;
        Ok(n)
    }
}
struct BudgetWriter<'a> {
    file: &'a mut File,
    maximum: u64,
}
impl Write for BudgetWriter<'_> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        if self
            .file
            .stream_position()?
            .checked_add(buf.len() as u64)
            .is_none_or(|s| s > self.maximum)
        {
            return Err(std::io::Error::other("candidate ZIP budget exceeded"));
        }
        self.file.write(buf)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.file.flush()
    }
}
impl Seek for BudgetWriter<'_> {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        let p = self.file.seek(pos)?;
        if p > self.maximum {
            return Err(std::io::Error::other("candidate ZIP budget exceeded"));
        }
        Ok(p)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn candidate_recipe_recovery_retains_proof_and_denies_prechallenge_intents() {
        let mut spec = ProcessSpec {
            intent: Uuid::new_v4(),
            purpose: LaunchPurpose::CandidateTest,
            target: LaunchTarget::CandidateTest {
                install: 1,
                test_job: 1,
                attempt: Uuid::new_v4(),
            },
            image: PinnedImage {
                reference: format!("fixture.invalid/candidate@sha256:{}", "a".repeat(64)),
                sha256: Sha256Digest::parse("a".repeat(64)).unwrap(),
            },
            platform: LaunchPlatform::LinuxAmd64,
            inputs: [(1, LaunchInputKind::Pack), (2, LaunchInputKind::Runner)]
                .into_iter()
                .map(|(id, kind)| PinnedLaunchInput {
                    pin_id: id,
                    kind,
                    object_id: id,
                    object_version: "owned-pure-proof-fixture".into(),
                    size_bytes: 1,
                    sha256: Sha256Digest::of_bytes(b"owned"),
                    max_expanded_bytes: 1,
                    max_files: 1,
                })
                .collect(),
            pack_access: PackAccess::CandidateCopy,
            stage: None,
            timeout_ms: Some(1000),
            invocation: LaunchInvocation {
                argv: vec![
                    format!("{RUNNER_ROOT}/linux-amd64"),
                    "run-candidate-runtime-recipe".into(),
                ],
                env: BTreeMap::new(),
                working_root: LaunchRoot::Pack,
            },
        };
        assert!(retained_recipe_challenge(Some(&spec), true).is_err());
        let fresh = retained_recipe_challenge(None, true).unwrap();
        recipe_timeout::bind_challenge(&mut spec, fresh).unwrap();
        let old = spec.clone();
        let proof = retained_recipe_challenge(Some(&old), true).unwrap();
        spec.invocation.env.clear();
        recipe_timeout::bind_challenge(&mut spec, proof).unwrap();
        assert_eq!(spec.intent, old.intent);
        assert_eq!(spec.invocation, old.invocation);
        let proof = retained_recipe_challenge(Some(&old), true).unwrap();
        spec.intent = Uuid::new_v4();
        spec.invocation.env.clear();
        assert!(recipe_timeout::bind_challenge(&mut spec, proof).is_err());
        spec.invocation.argv = vec!["/bin/sh".into(), format!("{PACK_ROOT}/tests/run.sh")];
        assert!(retained_recipe_challenge(Some(&spec), false)
            .unwrap()
            .is_none());
    }
    fn archive(root: &Path, link: bool) -> tempfile::NamedTempFile {
        let file = tempfile::NamedTempFile::new_in(root).unwrap();
        let encoder =
            flate2::write::GzEncoder::new(file.reopen().unwrap(), flate2::Compression::fast());
        let mut tar = tar::Builder::new(encoder);
        let mut directory = tar::Header::new_gnu();
        directory.set_size(0);
        directory.set_entry_type(tar::EntryType::Directory);
        directory.set_mode(0o755);
        directory.set_cksum();
        tar.append_data(&mut directory, "candidate/empty", std::io::empty())
            .unwrap();
        for (path, bytes) in [
            (
                "candidate/pack.yaml",
                b"ref: candidate\nversion: 1.0.0\n".as_slice(),
            ),
            ("candidate/tests/run.sh", b"printf passed".as_slice()),
        ] {
            let mut header = tar::Header::new_gnu();
            header.set_size(bytes.len() as u64);
            header.set_mode(0o755);
            header.set_cksum();
            tar.append_data(&mut header, path, bytes).unwrap();
        }
        if link {
            let mut header = tar::Header::new_gnu();
            header.set_size(0);
            header.set_entry_type(tar::EntryType::Symlink);
            header.set_link_name("/etc/passwd").unwrap();
            header.set_cksum();
            tar.append_data(&mut header, "candidate/forbidden", std::io::empty())
                .unwrap();
        }
        tar.into_inner()
            .unwrap()
            .finish()
            .unwrap()
            .sync_all()
            .unwrap();
        file
    }
    fn limits() -> attune_common::workload_config::WorkloadStorageLimits {
        attune_common::workload_config::WorkloadStorageLimits {
            compressed_bytes: 16384,
            expanded_bytes: 32768,
            entries: 32,
            artifacts: Default::default(),
        }
    }
    fn cache(root: &Path) -> attune_workload_backend::cache::CacheStore {
        attune_workload_backend::cache::CacheStore::open(
            root.join("cache"),
            attune_workload_backend::cache::CacheLimits {
                compressed_bytes: 16384,
                expanded_bytes: 32768,
                files: 32,
            },
        )
        .unwrap()
    }
    // No process effects exist in these pure archive fixtures. Unseal only
    // their explicitly owned cache directories, then surface deletion errors.
    fn cleanup(root: tempfile::TempDir) {
        fn writable(path: &Path) {
            let metadata = std::fs::symlink_metadata(path).unwrap();
            assert!(!metadata.file_type().is_symlink());
            if metadata.is_dir() {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700)).unwrap();
                for entry in std::fs::read_dir(path).unwrap() {
                    writable(&entry.unwrap().path());
                }
            }
        }
        writable(root.path());
        root.close().unwrap();
    }
    #[test]
    fn actual_candidate_tar_contract_converts_deterministically_without_manifest() {
        let root = tempfile::tempdir().unwrap();
        let cache = cache(root.path());
        let input = archive(root.path(), false);
        let bytes = std::fs::read(input.path()).unwrap();
        let (first, dir) = convert_candidate(
            input,
            "candidate",
            &root.path().join("one"),
            CandidateInputIdentity {
                install: 1,
                pin_id: 1,
                version: "exact-version".into(),
            },
            limits(),
            &cache,
        )
        .unwrap();
        let input = tempfile::NamedTempFile::new_in(root.path()).unwrap();
        std::fs::write(input.path(), bytes).unwrap();
        let (second, _) = convert_candidate(
            input,
            "candidate",
            &root.path().join("two"),
            CandidateInputIdentity {
                install: 1,
                pin_id: 1,
                version: "exact-version".into(),
            },
            limits(),
            &cache,
        )
        .unwrap();
        assert_eq!(first, second);
        assert_eq!(
            std::fs::read(dir.join("tests/run.sh")).unwrap(),
            b"printf passed"
        );
        let mut archive = cache.open_input_archive(&first).unwrap();
        let body = cache.install_input(&first, &mut archive).unwrap();
        assert!(body.join("empty").is_dir());
        drop(archive);
        drop(cache);
        cleanup(root);
    }
    #[test]
    fn candidate_contract_rejects_link_wrong_root_and_non_tar_without_fallback() {
        for bad in ["link", "root", "zip"] {
            let root = tempfile::tempdir().unwrap();
            let cache = cache(root.path());
            let input = archive(root.path(), bad == "link");
            if bad == "zip" {
                std::fs::write(input.path(), b"PK\x03\x04not-the-api-contract").unwrap();
            }
            assert!(convert_candidate(
                input,
                if bad == "root" { "other" } else { "candidate" },
                &root.path().join("output"),
                CandidateInputIdentity {
                    install: 1,
                    pin_id: 1,
                    version: "version".into()
                },
                limits(),
                &cache
            )
            .is_err());
            drop(cache);
            cleanup(root);
        }
    }
    #[test]
    fn candidate_conversion_enforces_expansion_budget() {
        let root = tempfile::tempdir().unwrap();
        let cache = cache(root.path());
        let input = archive(root.path(), false);
        let mut limit = limits();
        limit.expanded_bytes = 1;
        assert!(convert_candidate(
            input,
            "candidate",
            &root.path().join("output"),
            CandidateInputIdentity {
                install: 1,
                pin_id: 1,
                version: "version".into()
            },
            limit,
            &cache
        )
        .is_err());
        drop(cache);
        cleanup(root);
    }
    struct CaptureFailure {
        remaining: usize,
    }
    impl tokio::io::AsyncRead for CaptureFailure {
        fn poll_read(
            mut self: std::pin::Pin<&mut Self>,
            _: &mut std::task::Context<'_>,
            buffer: &mut tokio::io::ReadBuf<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            if self.remaining == 0 {
                return std::task::Poll::Ready(Err(std::io::Error::other(
                    "lost capture after retained prefix",
                )));
            }
            let bytes = [b'x'; 8192];
            let size = self.remaining.min(buffer.remaining()).min(bytes.len());
            buffer.put_slice(&bytes[..size]);
            self.remaining -= size;
            std::task::Poll::Ready(Ok(()))
        }
    }
    #[tokio::test]
    async fn bounded_capture_drains_to_eof_and_preserves_known_interruption() {
        assert_eq!(
            read_capture(Box::pin(std::io::Cursor::new(b"owned".to_vec())))
                .await
                .unwrap(),
            "owned"
        );
        assert_eq!(
            read_capture(Box::pin(std::io::Cursor::new(vec![b'a'; 1024 * 1024 + 1])))
                .await
                .unwrap()
                .len(),
            1024 * 1024
        );
        assert_eq!(
            read_capture(Box::pin(std::io::Cursor::new(vec![0xff; 1024 * 1024 + 1])))
                .await
                .unwrap()
                .len(),
            3 * 1024 * 1024
        );
        assert!(read_capture(Box::pin(CaptureFailure {
            remaining: 1024 * 1024 + 1
        }))
        .await
        .is_err());
        let result = command_output(CandidateCapture {
            interruption: Some(TestProcessInterruption::Cancelled),
            exit_code: 0,
            stdout: "passed".into(),
            stderr: String::new(),
            duration_ms: 0,
        });
        assert!(result.unwrap().interruption.is_some());
    }
}
