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
        if *engine_id != self.binding.engine_id || daemon_id != &self.binding.daemon_id {
            return Err(EngineError::OwnershipMismatch);
        }
        self.inspect(spec, Some(container_id))
            .await?
            .ok_or(EngineError::Transport)
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
        let id = if let Some((id, _)) = self.inspect(spec, None).await.map_err(common_error)? {
            id
        } else {
            if create == ProcessCreateDisposition::ReconcileOnly {
                return Err(common_error(EngineError::Transport));
            }
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
            .exact(&self.spec, &self.resource)
            .await
            .map_err(common_error)?;
        self.launcher.wait_id(&id).await.map_err(common_error)
    }
    async fn stop(&self, _: LaunchStopReason) -> attune_common::Result<LaunchExitOutcome> {
        self.writer.lock().await.take();
        let (id, _) = self
            .launcher
            .exact(&self.spec, &self.resource)
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
}
