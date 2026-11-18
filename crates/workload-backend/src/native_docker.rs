//! Direct Engine effects for the existing trusted worker; no controller server.
use super::*;
use async_trait::async_trait;
use attune_common::{
    isolated_process::{
        ProcessControl, ProcessCreateDisposition, ProcessIo, ProcessLauncher, ProcessSpec,
        ResourceIdentity,
    },
    workload_launch::{LaunchExitOutcome, LaunchInputKind, LaunchStopReason, PinnedLaunchInput},
    workload_stdin::LaunchStdinPayload,
};
use futures::{
    future::{BoxFuture, Shared},
    FutureExt,
};
use std::sync::Arc;
use std::{
    pin::Pin,
    task::{Context, Poll},
};
use tokio::{
    io::{AsyncWrite, ReadBuf},
    sync::Mutex,
};

#[derive(Clone)]
pub struct DockerLauncher {
    engine: DockerEngine,
    policy: EnginePolicy,
    binding: LaunchEngineBinding,
    cache: Arc<crate::cache::CacheStore>,
    bootstrap: PinnedLaunchInput,
}
impl DockerLauncher {
    /// Startup must separately prove the fixed named-volume binding using the
    /// owned sentinel probe. Neither cache path nor daemon identity comes from
    /// a pack or execution request.
    pub fn new(
        engine: DockerEngine,
        policy: EnginePolicy,
        binding: LaunchEngineBinding,
        cache: Arc<crate::cache::CacheStore>,
        bootstrap: PinnedLaunchInput,
    ) -> Result<Self> {
        policy.validate()?;
        if bootstrap.kind != LaunchInputKind::Runner || binding.engine_id != policy.engine_id {
            return Err(EngineError::OwnershipMismatch);
        }
        Ok(Self {
            engine,
            policy,
            binding,
            cache,
            bootstrap,
        })
    }
    fn name(spec: &ProcessSpec) -> String {
        format!("attune-{}", spec.intent.simple())
    }
    fn body(&self, spec: &ProcessSpec, image_env: &[String]) -> Result<EngineCreateBody> {
        let digest = crate::process_policy::public_digest(spec)
            .map_err(|_| EngineError::OwnershipMismatch)?;
        if spec.platform != self.binding.platform {
            return Err(EngineError::OwnershipMismatch);
        }
        let mut mounts = vec![VolumeMount::new(
            &self.policy.cache_volume,
            input_subpath(LaunchInputKind::Runner, &self.bootstrap.sha256),
            crate::kubernetes::BOOTSTRAP_ROOT,
            true,
        )];
        for pin in &spec.inputs {
            let candidate =
                pin.kind == LaunchInputKind::Pack && spec.pack_access == PackAccess::CandidateCopy;
            mounts.push(VolumeMount::new(
                &self.policy.cache_volume,
                if candidate {
                    format!("private/{}/pack", spec.intent.simple())
                } else {
                    input_subpath(pin.kind, &pin.sha256)
                },
                pin.kind.container_root(),
                !candidate,
            ));
        }
        if spec.stage.is_some() {
            mounts.push(VolumeMount::new(
                &self.policy.cache_volume,
                format!("private/{}/runtime", spec.intent.simple()),
                RUNTIME_ROOT,
                false,
            ));
        }
        mounts.push(VolumeMount::new(
            &self.policy.cache_volume,
            format!("private/{}/artifacts", spec.intent.simple()),
            "/tmp/artifacts",
            false,
        ));
        let mut env = BTreeMap::new();
        for value in image_env {
            let key = value.split_once('=').map_or(value.as_str(), |(k, _)| k);
            if !env_name(key) {
                return Err(EngineError::Policy("invalid image environment"));
            }
            env.insert(key.to_string(), String::new());
        }
        for (k, v) in [
            ("PATH", "/usr/local/bin:/usr/bin:/bin"),
            ("HOME", "/tmp"),
            ("TMPDIR", "/tmp"),
        ] {
            env.insert(k.into(), v.into());
        }
        Ok(EngineCreateBody {
            image: spec.image.reference.clone(),
            cmd: vec![
                format!(
                    "{}/linux-{}",
                    crate::kubernetes::BOOTSTRAP_ROOT,
                    spec.platform.native_arch()
                ),
                "gated".into(),
            ],
            entrypoint: vec![String::new()],
            env: env.into_iter().map(|(k, v)| format!("{k}={v}")).collect(),
            user: "1000:1000".into(),
            working_dir: "/tmp".into(),
            open_stdin: true,
            stdin_once: true,
            attach_stdin: true,
            attach_stdout: true,
            attach_stderr: true,
            tty: false,
            healthcheck: DisabledHealthcheck::new(),
            labels: BTreeMap::from([
                ("attune.io/process-intent".into(), spec.intent.to_string()),
                ("attune.io/process-policy".into(), digest.as_str().into()),
                (
                    "attune.io/process-engine".into(),
                    self.binding.daemon_id.clone(),
                ),
            ]),
            host_config: HostPolicy {
                readonly_rootfs: true,
                cap_drop: vec!["ALL".into()],
                cap_add: vec![],
                security_opt: vec!["no-new-privileges:true".into()],
                privileged: false,
                pid_mode: String::new(),
                ipc_mode: "private".into(),
                cgroupns_mode: "private".into(),
                network_mode: self.policy.network.clone(),
                mounts,
                tmpfs: BTreeMap::from([(
                    "/tmp".into(),
                    format!(
                        "rw,nosuid,nodev,size={},mode=1777",
                        self.policy.scratch_bytes
                    ),
                )]),
                memory: self.policy.memory_bytes,
                pids_limit: self.policy.pids_limit,
                nano_cpus: 1_000_000_000,
                auto_remove: false,
                init: false,
            },
        })
    }
    async fn inspect(
        &self,
        spec: &ProcessSpec,
        id: Option<&str>,
    ) -> Result<Option<(String, String)>> {
        self.engine
            .verify_engine_binding(&self.binding, &self.policy)
            .await?;
        let response = self
            .engine
            .control
            .get(DockerEngine::url(&[
                "containers",
                &Self::name(spec),
                "json",
            ]))
            .send()
            .await
            .map_err(|_| EngineError::Transport)?;
        if response.status() == StatusCode::NOT_FOUND {
            return Ok(None);
        }
        let value: serde_json::Value = decode(response).await?;
        let container = value["Id"].as_str().ok_or(EngineError::InvalidResponse)?;
        let expected = self.body(spec, &[])?;
        let observed: ContainerInspection =
            serde_json::from_value(value.clone()).map_err(|_| EngineError::InvalidResponse)?;
        if !resource_id(container)
            || id.is_some_and(|id| id != container)
            || observed.name != format!("/{}", Self::name(spec))
            || observed.config.image != spec.image.reference
            || observed.config.user != "1000:1000"
            || observed.config.tty
            || observed.config.healthcheck != expected.healthcheck
            || observed.host_config != expected.host_config
            || expected
                .labels
                .iter()
                .any(|(k, v)| observed.config.labels.get(k) != Some(v))
            || value["Config"]["Cmd"]
                != serde_json::to_value(&expected.cmd).map_err(|_| EngineError::InvalidResponse)?
            || value["Config"]["Entrypoint"]
                != serde_json::to_value(&expected.entrypoint)
                    .map_err(|_| EngineError::InvalidResponse)?
        {
            return Err(EngineError::OwnershipMismatch);
        }
        Ok(Some((
            container.into(),
            value["State"]["Status"]
                .as_str()
                .ok_or(EngineError::InvalidResponse)?
                .into(),
        )))
    }
    async fn exact(
        &self,
        spec: &ProcessSpec,
        resource: &ResourceIdentity,
    ) -> Result<(String, String)> {
        let ResourceIdentity::Docker {
            engine_id,
            daemon_id,
            container_id,
        } = resource
        else {
            return Err(EngineError::OwnershipMismatch);
        };
        if *engine_id != self.binding.engine_id
            || daemon_id != &self.binding.daemon_id
            || !resource_id(container_id)
        {
            return Err(EngineError::OwnershipMismatch);
        }
        self.inspect(spec, Some(container_id))
            .await?
            .ok_or(EngineError::Transport)
    }
    async fn reconcile_owned(&self, spec: &ProcessSpec) -> Result<Option<(String, String)>> {
        self.engine
            .verify_engine_binding(&self.binding, &self.policy)
            .await?;
        let response = self
            .engine
            .control
            .get(DockerEngine::url(&[
                "containers",
                &Self::name(spec),
                "json",
            ]))
            .send()
            .await
            .map_err(|_| EngineError::Transport)?;
        if response.status() == StatusCode::NOT_FOUND {
            return Ok(None);
        }
        let value: serde_json::Value = decode(response).await?;
        let id = value["Id"]
            .as_str()
            .filter(|id| resource_id(id))
            .ok_or(EngineError::InvalidResponse)?;
        let identity = ResourceIdentity::Docker {
            engine_id: self.binding.engine_id,
            daemon_id: self.binding.daemon_id.clone(),
            container_id: id.into(),
        };
        self.cleanup_exact(spec, &identity).await.map(Some)
    }
    async fn cleanup_exact(
        &self,
        spec: &ProcessSpec,
        resource: &ResourceIdentity,
    ) -> Result<(String, String)> {
        let ResourceIdentity::Docker {
            engine_id,
            daemon_id,
            container_id,
        } = resource
        else {
            return Err(EngineError::OwnershipMismatch);
        };
        if *engine_id != self.binding.engine_id
            || daemon_id != &self.binding.daemon_id
            || !resource_id(container_id)
        {
            return Err(EngineError::OwnershipMismatch);
        }
        self.engine
            .verify_engine_binding(&self.binding, &self.policy)
            .await?;
        let value: serde_json::Value = decode(
            self.engine
                .control
                .get(DockerEngine::url(&["containers", container_id, "json"]))
                .send()
                .await
                .map_err(|_| EngineError::Transport)?,
        )
        .await?;
        let digest = crate::process_policy::public_digest(spec)
            .map_err(|_| EngineError::OwnershipMismatch)?;
        if value["Id"].as_str() != Some(container_id)
            || value["Name"].as_str() != Some(&format!("/{}", Self::name(spec)))
            || value["Config"]["Labels"]["attune.io/process-intent"].as_str()
                != Some(&spec.intent.to_string())
            || value["Config"]["Labels"]["attune.io/process-policy"].as_str()
                != Some(digest.as_str())
            || value["Config"]["Labels"]["attune.io/process-engine"].as_str() != Some(daemon_id)
        {
            return Err(EngineError::OwnershipMismatch);
        }
        Ok((
            container_id.clone(),
            value["State"]["Status"]
                .as_str()
                .ok_or(EngineError::InvalidResponse)?
                .into(),
        ))
    }
    /// Caller has committed terminal/custody state and an empty or published
    /// ordinary-file inventory. This returns only positive Wait+Remove evidence;
    /// private staging stays owned until its separate receipt-gated cleanup.
    pub async fn remove_stopped(
        &self,
        spec: &ProcessSpec,
        resource: &ResourceIdentity,
    ) -> attune_common::Result<()> {
        let (id, state) = self
            .cleanup_exact(spec, resource)
            .await
            .map_err(common_error)?;
        if !matches!(state.as_str(), "created" | "exited") {
            return Err(common_error(EngineError::OwnershipMismatch));
        }
        self.wait_id(&id).await.map_err(common_error)?;
        let response = self
            .engine
            .control
            .delete(DockerEngine::url(&["containers", &id]))
            .query(&[("force", "false"), ("v", "false")])
            .send()
            .await
            .map_err(|_| common_error(EngineError::Transport))?;
        if response.status() != StatusCode::NO_CONTENT {
            return Err(common_error(EngineError::Rejected(
                response.status().as_u16(),
            )));
        }
        Ok(())
    }
    /// Read the fixed private Setup stage only after exact successful Wait.
    /// No Engine archive API or caller pathname; existing runtime publication
    /// and its receipt remain the trusted owning loop's responsibility.
    pub async fn archive_stopped_stage(
        &self,
        spec: &ProcessSpec,
        resource: &ResourceIdentity,
    ) -> attune_common::Result<std::fs::File> {
        self.cache
            .native_stage_limits(spec)
            .map_err(|_| common_error(EngineError::OwnershipMismatch))?;
        let (id, state) = self
            .cleanup_exact(spec, resource)
            .await
            .map_err(common_error)?;
        if state != "exited" {
            return Err(common_error(EngineError::OwnershipMismatch));
        }
        let outcome = self.wait_id(&id).await.map_err(common_error)?;
        if outcome.exit_code != 0 || outcome.oom_killed {
            return Err(common_error(EngineError::OwnershipMismatch));
        }
        let cache = self.cache.clone();
        let selected = spec.clone();
        tokio::task::spawn_blocking(move || cache.archive_native_stage(&selected))
            .await
            .map_err(|_| common_error(EngineError::Transport))?
            .map_err(|_| common_error(EngineError::OwnershipMismatch))
    }
    pub async fn read_stopped_artifact(
        &self,
        spec: &ProcessSpec,
        resource: &ResourceIdentity,
        relative: &attune_common::artifact_transport::ValidatedRelativePath,
        remaining: u64,
    ) -> attune_common::Result<Option<std::fs::File>> {
        crate::artifact_file::permits_files(spec)?;
        let (id, state) = self
            .cleanup_exact(spec, resource)
            .await
            .map_err(common_error)?;
        if state != "exited" {
            return Err(common_error(EngineError::OwnershipMismatch));
        }
        self.wait_id(&id).await.map_err(common_error)?;
        let store = self.cache.clone();
        let selected = spec.clone();
        let relative = relative.clone();
        tokio::task::spawn_blocking(move || {
            store.read_native_artifact(&selected, &relative, remaining)
        })
        .await
        .map_err(|_| common_error(EngineError::Transport))?
        .map_err(|_| common_error(EngineError::OwnershipMismatch))
    }
    async fn wait_id(&self, id: &str) -> Result<LaunchExitOutcome> {
        let response = self
            .engine
            .stream
            .post(DockerEngine::url(&["containers", id, "wait"]))
            .query(&[("condition", "not-running")])
            .send()
            .await
            .map_err(|_| EngineError::Transport)?;
        let outcome: WaitResponse = decode(response).await?;
        let inspected: StateInspection = decode(
            self.engine
                .control
                .get(DockerEngine::url(&["containers", id, "json"]))
                .send()
                .await
                .map_err(|_| EngineError::Transport)?,
        )
        .await?;
        if inspected.state.running {
            return Err(EngineError::InvalidResponse);
        }
        Ok(LaunchExitOutcome {
            exit_code: if outcome.error.is_some() && outcome.status_code == 0 {
                -1
            } else {
                outcome.status_code
            },
            oom_killed: inspected.state.oom_killed,
        })
    }
}
fn common_error(_: EngineError) -> attune_common::Error {
    attune_common::Error::validation(
        "Docker isolated process effect failed or has uncertain settlement",
    )
}
#[async_trait]
impl ProcessLauncher for DockerLauncher {
    async fn prepare(
        &self,
        spec: &ProcessSpec,
        create: ProcessCreateDisposition,
    ) -> attune_common::Result<ResourceIdentity> {
        if create == ProcessCreateDisposition::ReconcileOnly {
            let (id, _) = self
                .reconcile_owned(spec)
                .await
                .map_err(common_error)?
                .ok_or_else(|| common_error(EngineError::Transport))?;
            return Ok(ResourceIdentity::Docker {
                engine_id: self.binding.engine_id,
                daemon_id: self.binding.daemon_id.clone(),
                container_id: id,
            });
        }
        let id = if let Some((id, _)) = self.inspect(spec, None).await.map_err(common_error)? {
            id
        } else {
            let image_env = self
                .engine
                .image_environment_for_binding(&self.binding, &self.policy, &spec.image)
                .await
                .map_err(common_error)?;
            let cache = self.cache.clone();
            let selected = spec.clone();
            tokio::task::spawn_blocking(move || cache.prepare_native_private(&selected))
                .await
                .map_err(|_| common_error(EngineError::Transport))?
                .map_err(|_| common_error(EngineError::Policy("private staging unavailable")))?;
            let body = self.body(spec, &image_env).map_err(common_error)?;
            // One POST only. Conflict, timeout or lost response never resubmits.
            let response = self
                .engine
                .control
                .post(DockerEngine::url(&["containers", "create"]))
                .query(&[("name", Self::name(spec))])
                .json(&body)
                .send()
                .await
                .map_err(|_| common_error(EngineError::Transport))?;
            let created: CreateResponse = decode(response).await.map_err(common_error)?;
            self.inspect(spec, Some(&created.id))
                .await
                .map_err(common_error)?
                .ok_or_else(|| common_error(EngineError::Transport))?
                .0
        };
        Ok(ResourceIdentity::Docker {
            engine_id: self.binding.engine_id,
            daemon_id: self.binding.daemon_id.clone(),
            container_id: id,
        })
    }
    async fn start(
        &self,
        spec: &ProcessSpec,
        resource: &ResourceIdentity,
    ) -> attune_common::Result<ProcessIo> {
        crate::process_policy::validate_invocation(spec)?;
        if !self.policy.allowed_images.contains(&spec.image.reference) {
            return Err(common_error(EngineError::OwnershipMismatch));
        }
        let (id, state) = self.exact(spec, resource).await.map_err(common_error)?;
        if !matches!(state.as_str(), "created" | "running") {
            return Err(common_error(EngineError::OwnershipMismatch));
        }
        let response = self
            .engine
            .stream
            .post(DockerEngine::url(&["containers", &id, "attach"]))
            .query(&[
                ("stream", "1"),
                ("stdin", "1"),
                ("stdout", "1"),
                ("stderr", "1"),
            ])
            .header(header::CONNECTION, "Upgrade")
            .header(header::UPGRADE, "tcp")
            .send()
            .await
            .map_err(|_| common_error(EngineError::Transport))?;
        if response.status() != StatusCode::SWITCHING_PROTOCOLS {
            return Err(common_error(EngineError::Transport));
        }
        let attached = response
            .upgrade()
            .await
            .map_err(|_| common_error(EngineError::Transport))?;
        let (read, writer) = tokio::io::split(attached);
        let (out_tx, out_rx) = tokio::io::duplex(65536);
        let (err_tx, err_rx) = tokio::io::duplex(65536);
        let task = tokio::spawn(demultiplex(read, out_tx, err_tx));
        let engine = self.clone();
        let selected = spec.clone();
        let exact_resource = resource.clone();
        let terminal = async move {
            if !task.await.is_ok_and(|r| r.is_ok()) {
                return false;
            }
            // Daemon loss / premature attach closure cannot become completed
            // capture while the exact payload is still running.
            engine
                .exact(&selected, &exact_resource)
                .await
                .is_ok_and(|(_, s)| s == "exited")
        }
        .boxed()
        .shared();
        if state == "created" {
            let response = self
                .engine
                .control
                .post(DockerEngine::url(&["containers", &id, "start"]))
                .send()
                .await
                .map_err(|_| common_error(EngineError::Transport))?;
            if response.status() != StatusCode::NO_CONTENT {
                return Err(common_error(EngineError::Rejected(
                    response.status().as_u16(),
                )));
            }
        }
        Ok(ProcessIo {
            resource: resource.clone(),
            stdout: Box::pin(DockerReader {
                inner: out_rx,
                terminal: terminal.clone(),
            }),
            stderr: Box::pin(DockerReader {
                inner: err_rx,
                terminal,
            }),
            control: Arc::new(DockerControl {
                launcher: self.clone(),
                spec: spec.clone(),
                resource: resource.clone(),
                writer: Mutex::new(Some(Box::new(writer))),
                header: crate::runner_protocol::encode_start(&spec.invocation)
                    .map_err(|_| common_error(EngineError::Policy("invalid private context")))?,
            }),
        })
    }
}
struct DockerReader {
    inner: tokio::io::DuplexStream,
    terminal: Shared<BoxFuture<'static, bool>>,
}
impl AsyncRead for DockerReader {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let before = buf.filled().len();
        match Pin::new(&mut self.inner).poll_read(cx, buf) {
            Poll::Ready(Ok(())) if buf.filled().len() == before && buf.remaining() > 0 => {
                use std::future::Future;
                match Pin::new(&mut self.terminal).poll(cx) {
                    Poll::Ready(true) => Poll::Ready(Ok(())),
                    Poll::Ready(false) => {
                        Poll::Ready(Err(std::io::Error::other("Docker capture transport lost")))
                    }
                    Poll::Pending => Poll::Pending,
                }
            }
            other => other,
        }
    }
}
struct DockerControl {
    launcher: DockerLauncher,
    spec: ProcessSpec,
    resource: ResourceIdentity,
    writer: Mutex<Option<Box<dyn AsyncWrite + Unpin + Send>>>,
    header: Vec<u8>,
}
#[async_trait]
impl ProcessControl for DockerControl {
    async fn write_stdin_once(&self, payload: LaunchStdinPayload) -> attune_common::Result<()> {
        let (_, state) = self
            .launcher
            .exact(&self.spec, &self.resource)
            .await
            .map_err(common_error)?;
        let mut writer = self
            .writer
            .lock()
            .await
            .take()
            .ok_or_else(|| common_error(EngineError::Transport))?;
        if state != "running" {
            return Err(common_error(EngineError::Transport));
        }
        writer
            .write_all(&self.header)
            .await
            .map_err(|_| common_error(EngineError::Transport))?;
        writer
            .write_all(payload.as_bytes())
            .await
            .map_err(|_| common_error(EngineError::Transport))?;
        writer
            .shutdown()
            .await
            .map_err(|_| common_error(EngineError::Transport))
    }
    async fn wait(&self) -> attune_common::Result<LaunchExitOutcome> {
        let (id, _) = self
            .launcher
            .cleanup_exact(&self.spec, &self.resource)
            .await
            .map_err(common_error)?;
        self.launcher.wait_id(&id).await.map_err(common_error)
    }
    async fn stop(&self, _: LaunchStopReason) -> attune_common::Result<LaunchExitOutcome> {
        self.writer.lock().await.take();
        let (id, _) = self
            .launcher
            .cleanup_exact(&self.spec, &self.resource)
            .await
            .map_err(common_error)?;
        let response = self
            .launcher
            .engine
            .control
            .post(DockerEngine::url(&["containers", &id, "stop"]))
            .query(&[("t", "5")])
            .send()
            .await
            .map_err(|_| common_error(EngineError::Transport))?;
        if !matches!(
            response.status(),
            StatusCode::NO_CONTENT | StatusCode::NOT_MODIFIED
        ) {
            return Err(common_error(EngineError::Rejected(
                response.status().as_u16(),
            )));
        }
        tokio::time::timeout(Duration::from_secs(20), self.launcher.wait_id(&id))
            .await
            .map_err(|_| common_error(EngineError::Transport))?
            .map_err(common_error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture(socket: &Path) -> (DockerLauncher, ProcessSpec, tempfile::TempDir) {
        let root = tempfile::tempdir().unwrap();
        let old =
            crate::test_support::spec(format!("fixture.invalid/payload@sha256:{}", "a".repeat(64)));
        let policy = crate::test_support::policy(
            old.manifest.image.reference.clone(),
            "owned-network".into(),
        );
        let spec = ProcessSpec {
            intent: old.manifest.engine_name_nonce,
            purpose: old.manifest.purpose,
            target: old.manifest.target,
            image: old.manifest.image,
            platform: LaunchPlatform::LinuxAmd64,
            inputs: vec![],
            pack_access: PackAccess::ReadOnly,
            stage: None,
            timeout_ms: Some(1000),
            invocation: old.invocation,
        };
        let cache = crate::cache::CacheStore::open(
            root.path().join("cache"),
            crate::cache::CacheLimits {
                compressed_bytes: 1024,
                expanded_bytes: 4096,
                files: 32,
            },
        )
        .unwrap();
        let bootstrap = PinnedLaunchInput {
            pin_id: 1,
            kind: LaunchInputKind::Runner,
            object_id: 1,
            object_version: "public-fixture".into(),
            size_bytes: 1,
            sha256: Sha256Digest::of_bytes(b"public-helper"),
            max_expanded_bytes: 4096,
            max_files: 1,
        };
        (
            DockerLauncher::new(
                DockerEngine::connect(socket).unwrap(),
                policy,
                LaunchEngineBinding {
                    engine_id: 5,
                    daemon_id: "owned-daemon".into(),
                    platform: LaunchPlatform::LinuxAmd64,
                },
                Arc::new(cache),
                bootstrap,
            )
            .unwrap(),
            spec,
            root,
        )
    }
    #[test]
    fn private_invocation_cannot_enter_engine_metadata_or_change_fixed_mount_policy() {
        let path = tempfile::tempdir().unwrap();
        let (launcher, mut spec, _root) = fixture(&path.path().join("not-connected.sock"));
        spec.invocation.argv.push("owned-private-argument".into());
        spec.invocation
            .env
            .insert("SELECTED_TOKEN".into(), "owned-private-token".into());
        let body = launcher
            .body(
                &spec,
                &[
                    "INHERITED_SECRET=owned-image-token".into(),
                    "LD_PRELOAD=owned-image-loader".into(),
                ],
            )
            .unwrap();
        let json = serde_json::to_value(&body).unwrap();
        let serialized = serde_json::to_string(&json).unwrap();
        for secret in ["owned-private", "owned-image-token", "owned-image-loader"] {
            assert!(!serialized.contains(secret));
        }
        assert!(json["Env"]
            .as_array()
            .unwrap()
            .contains(&serde_json::json!("INHERITED_SECRET=")));
        assert_eq!(json["HostConfig"]["Privileged"], false);
        assert_eq!(json["HostConfig"]["ReadonlyRootfs"], true);
        assert_eq!(json["HostConfig"]["CapDrop"], serde_json::json!(["ALL"]));
        assert_eq!(json["HostConfig"]["CapAdd"], serde_json::json!([]));
        assert_eq!(json["HostConfig"]["Init"], false);
        assert_eq!(json["User"], "1000:1000");
        assert_eq!(json["Healthcheck"]["Test"], serde_json::json!(["NONE"]));
        let mounts = json["HostConfig"]["Mounts"].as_array().unwrap();
        assert!(mounts
            .iter()
            .all(|m| m["Type"] == "volume" && m["Source"] == "owned-fixture-cache"));
        let artifacts = mounts
            .iter()
            .find(|m| m["Target"] == "/tmp/artifacts")
            .unwrap();
        assert_eq!(artifacts["ReadOnly"], false);
        assert_eq!(
            artifacts["VolumeOptions"]["Subpath"],
            format!("private/{}/artifacts", spec.intent.simple())
        );
        let digest = crate::process_policy::public_digest(&spec).unwrap();
        spec.invocation.argv.clear();
        spec.invocation.env.clear();
        assert_eq!(digest, crate::process_policy::public_digest(&spec).unwrap());
    }
    #[tokio::test]
    async fn unknown_create_missing_inspection_is_read_only_and_never_posts_create() {
        let path = tempfile::tempdir().unwrap();
        let socket = path.path().join("owned.sock");
        let listener = tokio::net::UnixListener::bind(&socket).unwrap();
        let server = tokio::spawn(async move {
            let mut paths = Vec::new();
            for _ in 0..2 {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut request = Vec::new();
                let mut buf = [0; 1024];
                while !request.windows(4).any(|b| b == b"\r\n\r\n") {
                    let n = stream.read(&mut buf).await.unwrap();
                    assert!(n > 0);
                    request.extend_from_slice(&buf[..n]);
                    assert!(request.len() < 8192);
                }
                let request = String::from_utf8(request).unwrap();
                let line = request.lines().next().unwrap().to_string();
                assert!(line.starts_with("GET "));
                paths.push(line.clone());
                let (status, body) = if line.contains("/info ") {
                    (
                        "200 OK",
                        r#"{"ID":"owned-daemon","OSType":"linux","Architecture":"amd64","SecurityOptions":["name=seccomp,profile=builtin"]}"#,
                    )
                } else {
                    assert!(line.contains("/containers/attune-"));
                    ("404 Not Found", "{}")
                };
                stream.write_all(format!("HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).as_bytes()).await.unwrap();
            }
            paths
        });
        let (launcher, spec, _root) = fixture(&socket);
        assert!(launcher
            .prepare(&spec, ProcessCreateDisposition::ReconcileOnly)
            .await
            .is_err());
        let paths = server.await.unwrap();
        assert_eq!(paths.len(), 2);
        assert!(paths.iter().all(|p| !p.contains("/create")));
    }
    #[tokio::test]
    async fn cleanup_rejects_cross_daemon_and_malformed_selectors_before_engine_io() {
        let path = tempfile::tempdir().unwrap();
        let socket = path.path().join("owned-cleanup.sock");
        let listener = tokio::net::UnixListener::bind(&socket).unwrap();
        let (launcher, spec, _root) = fixture(&socket);
        for (engine_id, daemon_id, container_id) in [
            (6, "owned-daemon", "a".repeat(64)),
            (5, "other-daemon", "a".repeat(64)),
            (5, "owned-daemon", "../foreign-selector".into()),
        ] {
            let resource = ResourceIdentity::Docker {
                engine_id,
                daemon_id: daemon_id.into(),
                container_id,
            };
            let result = tokio::time::timeout(
                Duration::from_secs(1),
                launcher.remove_stopped(&spec, &resource),
            )
            .await
            .expect("rejection must precede transport");
            assert!(result.is_err());
        }
        assert!(
            tokio::time::timeout(Duration::from_millis(10), listener.accept())
                .await
                .is_err(),
            "rejected cleanup must not contact Engine"
        );
    }
    #[test]
    fn local_candidate_image_uses_exact_config_id_without_tag_or_registry_alias() {
        let path = tempfile::tempdir().unwrap();
        let (mut launcher, mut spec, _root) = fixture(&path.path().join("not-connected.sock"));
        spec.image.reference = format!("sha256:{}", spec.image.sha256.as_str());
        launcher.policy.allowed_images =
            std::collections::BTreeSet::from([spec.image.reference.clone()]);
        let body = launcher.body(&spec, &[]).unwrap();
        assert_eq!(body.image, spec.image.reference);
        spec.image.reference = format!("sha256:{}", "b".repeat(64));
        assert!(launcher.body(&spec, &[]).is_err());
    }
    #[tokio::test]
    async fn inspection_rejects_inherited_or_executable_healthchecks_before_attach() {
        for (health, accepted) in [
            (Some(serde_json::json!({"Test":["NONE"]})), true),
            (Some(serde_json::json!({"Test":[]})), false),
            (
                Some(serde_json::json!({"Test":["CMD-SHELL","owned-before-gate"]})),
                false,
            ),
            (Some(serde_json::Value::Null), false),
            (None, false),
        ] {
            let root = tempfile::tempdir().unwrap();
            let socket = root.path().join("owned-health-inspection.sock");
            let listener = tokio::net::UnixListener::bind(&socket).unwrap();
            let (launcher, spec, _cache) = fixture(&socket);
            let body = serde_json::to_value(launcher.body(&spec, &[]).unwrap()).unwrap();
            let mut config = body.clone();
            if let Some(health) = health {
                config["Healthcheck"] = health;
            } else {
                config.as_object_mut().unwrap().remove("Healthcheck");
            }
            let observed = serde_json::json!({"Id":"b".repeat(64),
                "Name":format!("/{}",DockerLauncher::name(&spec)),
                "Config":config,"HostConfig":body["HostConfig"],"State":{"Status":"created"}})
            .to_string();
            let server = tokio::spawn(async move {
                for index in 0..2 {
                    let (mut stream, _) = listener.accept().await.unwrap();
                    let mut request = Vec::new();
                    let mut buffer = [0; 1024];
                    while !request.windows(4).any(|b| b == b"\r\n\r\n") {
                        let count = stream.read(&mut buffer).await.unwrap();
                        assert!(count > 0);
                        request.extend_from_slice(&buffer[..count]);
                        assert!(request.len() < 8192);
                    }
                    let request = String::from_utf8(request).unwrap();
                    let first = request.lines().next().unwrap();
                    assert!(first.starts_with("GET "));
                    let response = if index == 0 {
                        assert!(first.contains("/info "));
                        r#"{"ID":"owned-daemon","OSType":"linux","Architecture":"amd64","SecurityOptions":["name=seccomp,profile=builtin"]}"#
                    } else {
                        assert!(first.contains("/containers/"));
                        observed.as_str()
                    };
                    stream.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response}", response.len()).as_bytes()).await.unwrap();
                }
            });
            let result =
                tokio::time::timeout(Duration::from_secs(5), launcher.inspect(&spec, None))
                    .await
                    .unwrap();
            server.await.unwrap();
            assert_eq!(result.is_ok(), accepted);
        }
    }
    #[tokio::test]
    async fn reconciliation_recovers_public_ownership_after_policy_change_without_run_permission() {
        let path = tempfile::tempdir().unwrap();
        let socket = path.path().join("owned-reconcile.sock");
        let listener = tokio::net::UnixListener::bind(&socket).unwrap();
        let (mut launcher, mut spec, _root) = fixture(&socket);
        let id = "b".repeat(64);
        let expected_id = id.clone();
        let digest = crate::process_policy::public_digest(&spec).unwrap();
        let body=serde_json::json!({"Id":id,"Name":format!("/{}",DockerLauncher::name(&spec)),"Config":{"Labels":{
            "attune.io/process-intent":spec.intent.to_string(),"attune.io/process-policy":digest.as_str(),"attune.io/process-engine":"owned-daemon"}},"State":{"Status":"exited"}}).to_string();
        let original_invocation = spec.invocation.clone();
        spec.invocation.argv.clear();
        spec.invocation.env.clear();
        launcher.policy.allowed_images.clear();
        launcher.policy.memory_bytes *= 2;
        launcher.bootstrap.sha256 = Sha256Digest::of_bytes(b"changed-public-helper");
        let server = tokio::spawn(async move {
            for _ in 0..4 {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut request = Vec::new();
                let mut buf = [0; 1024];
                while !request.windows(4).any(|b| b == b"\r\n\r\n") {
                    let n = stream.read(&mut buf).await.unwrap();
                    assert!(n > 0);
                    request.extend_from_slice(&buf[..n]);
                    assert!(request.len() < 8192);
                }
                let request = String::from_utf8(request).unwrap();
                let first = request.lines().next().unwrap();
                assert!(first.starts_with("GET "));
                let response = if first.contains("/info ") {
                    r#"{"ID":"owned-daemon","OSType":"linux","Architecture":"amd64","SecurityOptions":["name=seccomp,profile=builtin"]}"#
                } else {
                    assert!(first.contains("/containers/"));
                    body.as_str()
                };
                stream.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response}",response.len()).as_bytes()).await.unwrap();
            }
            listener
        });
        let resource = launcher
            .prepare(&spec, ProcessCreateDisposition::ReconcileOnly)
            .await
            .unwrap();
        assert!(
            matches!(&resource,ResourceIdentity::Docker{container_id,..} if container_id==&expected_id)
        );
        let listener = server.await.unwrap();
        spec.invocation = original_invocation;
        let denied = tokio::time::timeout(Duration::from_secs(1), launcher.start(&spec, &resource))
            .await
            .expect("new Run must deny before transport");
        assert!(denied.is_err());
        assert!(
            tokio::time::timeout(Duration::from_millis(10), listener.accept())
                .await
                .is_err()
        );
    }
}
