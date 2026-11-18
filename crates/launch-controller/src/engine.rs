//! Docker Engine API v1.47. Only a colocated Unix-socket engine is supported.
//! Cache mounts are generated from fixed controller configuration + admitted
//! immutable identifiers. No web request may deserialize an EngineCreateBody.
use attune_common::workload_launch::{
    EngineResourceEvidence, LaunchInputKind, LaunchManifest, LaunchSpec, PackAccess, Sha256Digest,
    RUNTIME_ROOT, SCRATCH_ROOT,
};
use reqwest::{header, Client, StatusCode, Url};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
    time::Duration,
};
use thiserror::Error;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

const API: &str = "v1.47";
const FRAME_LIMIT: usize = 16 * 1024 * 1024;
const RESPONSE_LIMIT: usize = 4 * 1024 * 1024;
const OWNER_LABEL: &str = "io.attune.launch.engine";
const LAUNCH_LABEL: &str = "io.attune.launch.id";
const NONCE_LABEL: &str = "io.attune.launch.nonce";
const DIGEST_LABEL: &str = "io.attune.launch.manifest";

#[derive(Debug, Error)]
pub enum EngineError {
    #[error("invalid controller engine policy: {0}")]
    Policy(&'static str),
    #[error("engine transport failed; resource settlement is uncertain")]
    Transport,
    #[error("engine operation rejected (HTTP {0})")]
    Rejected(u16),
    #[error("invalid engine response")]
    InvalidResponse,
    #[error("engine resource does not match admitted ownership or isolation policy")]
    OwnershipMismatch,
    #[error("engine I/O failed")]
    Io(#[from] std::io::Error),
}
pub type Result<T> = std::result::Result<T, EngineError>;

/// Operator ceiling, never supplied by a runner/pack request.
#[derive(Clone)]
pub struct EnginePolicy {
    pub engine_id: i64,
    pub cache_volume: String,
    pub network: String,
    pub allowed_images: BTreeSet<String>,
    pub memory_bytes: u64,
    pub pids_limit: i64,
    pub scratch_bytes: u64,
}
impl EnginePolicy {
    pub fn validate(&self) -> Result<()> {
        if self.engine_id <= 0
            || !engine_name(&self.cache_volume)
            || !engine_name(&self.network)
            || matches!(self.network.as_str(), "host" | "none")
            || self.allowed_images.is_empty()
            || self.memory_bytes < 32 * 1024 * 1024
            || self.memory_bytes > i64::MAX as u64
            || self.pids_limit < 16
            || self.scratch_bytes == 0
            || self.scratch_bytes > self.memory_bytes
        {
            return Err(EngineError::Policy(
                "invalid fixed engine/cache/network/resource ceiling",
            ));
        }
        Ok(())
    }
    pub fn prepare(&self, spec: &LaunchSpec, image_env: &[String]) -> Result<PreparedContainer> {
        self.validate()?;
        spec.validate_shape()
            .map_err(|_| EngineError::Policy("invalid admitted launch"))?;
        let manifest = &spec.manifest;
        if manifest.engine_id != self.engine_id
            || !self.allowed_images.contains(&manifest.image.reference)
        {
            return Err(EngineError::Policy(
                "image or engine exceeds operator ceiling",
            ));
        }
        if spec.invocation.argv.iter().any(|arg| arg.contains('\0'))
            || spec.invocation.argv.iter().map(String::len).sum::<usize>() > 1024 * 1024
        {
            return Err(EngineError::Policy("invalid invocation arguments"));
        }
        // Docker merges image Env with create Env. Explicitly clear every image
        // value before selected context is installed; ambient image credentials
        // and loader controls are never inherited by accident.
        let mut env = BTreeMap::new();
        for entry in image_env {
            let key = entry.split_once('=').map(|(key, _)| key).unwrap_or(entry);
            if !env_name(key) {
                return Err(EngineError::Policy("invalid image environment"));
            }
            env.insert(key.to_string(), String::new());
        }
        env.insert("PATH".into(), "/usr/local/bin:/usr/bin:/bin".into());
        for (key, value) in &spec.invocation.env {
            if !env_name(key) || forbidden_env(key) || value.contains('\0') {
                return Err(EngineError::Policy("forbidden invocation environment"));
            }
            env.insert(key.clone(), value.clone());
        }
        env.insert("HOME".into(), SCRATCH_ROOT.into());
        env.insert("TMPDIR".into(), SCRATCH_ROOT.into());
        env.insert("ATTUNE_ARTIFACTS_DIR".into(), "/tmp/artifacts".into());
        let env = env
            .into_iter()
            .map(|(key, value)| format!("{key}={value}"))
            .collect::<Vec<_>>();
        if env.iter().map(String::len).sum::<usize>() > 1024 * 1024 {
            return Err(EngineError::Policy(
                "invocation environment exceeds ceiling",
            ));
        }
        let public = self.public_plan(manifest)?;
        Ok(PreparedContainer {
            manifest: manifest.clone(),
            public: public.clone(),
            body: EngineCreateBody {
                image: manifest.image.reference.clone(),
                user: "1000:1000".into(),
                cmd: spec.invocation.argv.clone(),
                entrypoint: vec![String::new()],
                env,
                working_dir: spec.invocation.working_root.container_path().into(),
                open_stdin: true,
                stdin_once: true,
                attach_stdin: true,
                attach_stdout: true,
                attach_stderr: true,
                tty: false,
                labels: public.labels.clone(),
                host_config: public.host.clone(),
            },
        })
    }
    /// Does not require an allowed image or private invocation. Revocation must
    /// never prevent cleanup of an already admitted, owned engine resource.
    pub fn public_plan(&self, manifest: &LaunchManifest) -> Result<PublicPlan> {
        self.validate()?;
        manifest
            .validate_shape()
            .map_err(|_| EngineError::Policy("invalid public manifest"))?;
        if manifest.engine_id != self.engine_id {
            return Err(EngineError::OwnershipMismatch);
        }
        let mut mounts = Vec::new();
        for input in &manifest.inputs {
            let writable_copy = input.kind == LaunchInputKind::Pack
                && manifest.pack_access == PackAccess::CandidateCopy;
            let subpath = if writable_copy {
                candidate_subpath(manifest)
            } else {
                input_subpath(input.kind, &input.sha256)
            };
            mounts.push(VolumeMount::new(
                &self.cache_volume,
                subpath,
                input.kind.container_root(),
                !writable_copy,
            ));
        }
        if manifest.stage.is_some() {
            mounts.push(VolumeMount::new(
                &self.cache_volume,
                stage_subpath(manifest),
                RUNTIME_ROOT,
                false,
            ));
        }
        let admitted = manifest
            .admitted_digest()
            .map_err(|_| EngineError::Policy("invalid public manifest"))?;
        let labels = BTreeMap::from([
            (OWNER_LABEL.into(), manifest.engine_id.to_string()),
            (LAUNCH_LABEL.into(), manifest.launch_id.to_string()),
            (NONCE_LABEL.into(), manifest.engine_name_nonce.to_string()),
            (DIGEST_LABEL.into(), admitted.as_str().to_string()),
        ]);
        let tmpfs = BTreeMap::from([
            (
                SCRATCH_ROOT.into(),
                format!("rw,nosuid,nodev,size={},mode=1777", self.scratch_bytes),
            ),
            (
                "/tmp/artifacts".into(),
                format!(
                    "rw,nosuid,nodev,size={},uid=1000,gid=1000,mode=0700",
                    self.scratch_bytes
                ),
            ),
        ]);
        let host = HostPolicy {
            readonly_rootfs: true,
            cap_drop: vec!["ALL".into()],
            cap_add: Vec::new(),
            security_opt: vec!["no-new-privileges:true".into()],
            privileged: false,
            pid_mode: String::new(),
            ipc_mode: "private".into(),
            cgroupns_mode: "private".into(),
            network_mode: self.network.clone(),
            mounts,
            tmpfs,
            memory: self.memory_bytes,
            pids_limit: self.pids_limit,
            nano_cpus: 1_000_000_000,
            auto_remove: false,
            init: true,
        };
        let public = PublicEngineManifest {
            image: manifest.image.reference.clone(),
            user: "1000:1000".into(),
            labels: labels.clone(),
            host: host.clone(),
        };
        let public_digest = Sha256Digest::of_bytes(
            &serde_json::to_vec(&public).map_err(|_| EngineError::InvalidResponse)?,
        );
        Ok(PublicPlan {
            manifest: manifest.clone(),
            admitted,
            digest: public_digest,
            labels,
            host,
        })
    }
}
fn engine_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 255
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
}
fn env_name(value: &str) -> bool {
    let mut bytes = value.bytes();
    bytes
        .next()
        .is_some_and(|b| b.is_ascii_alphabetic() || b == b'_')
        && bytes.all(|b| b.is_ascii_alphanumeric() || b == b'_')
}
fn forbidden_env(value: &str) -> bool {
    let name = value.to_ascii_uppercase();
    [
        "ATTUNE_LAUNCH_",
        "ATTUNE__DATABASE__",
        "ATTUNE__MESSAGE_QUEUE__",
        "ATTUNE__SECURITY__",
        "DOCKER_",
        "KUBERNETES_",
    ]
    .iter()
    .any(|prefix| name.starts_with(prefix))
        || matches!(
            name.as_str(),
            "DATABASE_URL"
                | "MQ_URL"
                | "JWT_SECRET"
                | "ENCRYPTION_KEY"
                | "KUBECONFIG"
                | "ATTUNE_AGENT_BOOTSTRAP_TOKEN"
                | "ATTUNE__AGENT__BOOTSTRAP_TOKEN"
                | "ATTUNE_RUNTIME_LOG_CUSTODY_KEY"
        )
}
pub fn input_subpath(kind: LaunchInputKind, digest: &Sha256Digest) -> String {
    let kind = match kind {
        LaunchInputKind::Pack => "pack",
        LaunchInputKind::Runtime => "runtime",
        LaunchInputKind::Runner => "runner",
    };
    format!("inputs/{kind}/{}/body", digest.as_str())
}
pub fn stage_subpath(manifest: &LaunchManifest) -> String {
    format!("private/{}/runtime", manifest.engine_name_nonce.simple())
}
pub fn candidate_subpath(manifest: &LaunchManifest) -> String {
    format!("private/{}/pack", manifest.engine_name_nonce.simple())
}

// Docker request bodies are constructed only here. Deserialize is used solely
// for authenticated engine inspection responses, never runner/web request input.
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "PascalCase")]
struct VolumeMount {
    #[serde(rename = "Type")]
    kind: String,
    source: String,
    target: String,
    read_only: bool,
    volume_options: VolumeOptions,
}
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "PascalCase")]
struct VolumeOptions {
    no_copy: bool,
    subpath: String,
}
impl VolumeMount {
    fn new(source: &str, subpath: String, target: &str, read_only: bool) -> Self {
        Self {
            kind: "volume".into(),
            source: source.into(),
            target: target.into(),
            read_only,
            volume_options: VolumeOptions {
                no_copy: true,
                subpath,
            },
        }
    }
}
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "PascalCase")]
struct HostPolicy {
    readonly_rootfs: bool,
    cap_drop: Vec<String>,
    #[serde(default, deserialize_with = "null_vec")]
    cap_add: Vec<String>,
    security_opt: Vec<String>,
    privileged: bool,
    pid_mode: String,
    ipc_mode: String,
    #[serde(rename = "CgroupnsMode")]
    cgroupns_mode: String,
    network_mode: String,
    #[serde(default, deserialize_with = "null_vec")]
    mounts: Vec<VolumeMount>,
    tmpfs: BTreeMap<String, String>,
    memory: u64,
    pids_limit: i64,
    nano_cpus: u64,
    auto_remove: bool,
    init: bool,
}
#[derive(Serialize)]
#[serde(rename_all = "PascalCase")]
struct EngineCreateBody {
    image: String,
    cmd: Vec<String>,
    entrypoint: Vec<String>,
    env: Vec<String>,
    user: String,
    working_dir: String,
    open_stdin: bool,
    stdin_once: bool,
    attach_stdin: bool,
    attach_stdout: bool,
    attach_stderr: bool,
    tty: bool,
    labels: BTreeMap<String, String>,
    host_config: HostPolicy,
}
#[derive(Serialize)]
struct PublicEngineManifest {
    image: String,
    user: String,
    labels: BTreeMap<String, String>,
    host: HostPolicy,
}
/// Public immutable ownership policy, safe to reconstruct on cleanup claims.
#[derive(Clone)]
pub struct PublicPlan {
    manifest: LaunchManifest,
    admitted: Sha256Digest,
    digest: Sha256Digest,
    labels: BTreeMap<String, String>,
    host: HostPolicy,
}
impl PublicPlan {
    pub fn digest(&self) -> &Sha256Digest {
        &self.digest
    }
    pub fn name(&self) -> String {
        self.manifest.engine_name()
    }
}
/// Private prepared body contains scoped invocation secrets; deliberately no Debug.
pub struct PreparedContainer {
    manifest: LaunchManifest,
    public: PublicPlan,
    body: EngineCreateBody,
}
impl PreparedContainer {
    pub fn name(&self) -> String {
        self.manifest.engine_name()
    }
    pub fn public_digest(&self) -> &Sha256Digest {
        &self.public.digest
    }
}
#[derive(Clone, Debug)]
pub struct OwnedContainer {
    evidence: EngineResourceEvidence,
    name: String,
}
impl OwnedContainer {
    pub fn evidence(&self) -> &EngineResourceEvidence {
        &self.evidence
    }
    pub fn name(&self) -> &str {
        &self.name
    }
}
/// Unknown includes transport loss/5xx/name conflict. Never retry create blindly.
pub enum CreateOutcome {
    Created(OwnedContainer),
    /// An actual non-conflict 4xx response settles this submitted request.
    /// A caller must durably retain that acknowledgement before CP settlement.
    DefinitivelyRejected {
        status: u16,
    },
    ReconcileRequired,
}
#[derive(Clone, Debug)]
pub struct ExitStatus {
    pub exit_code: i32,
    pub oom_killed: bool,
}
#[derive(Clone)]
pub struct DockerEngine {
    control: Client,
    stream: Client,
}
impl DockerEngine {
    #[cfg(unix)]
    pub fn connect(socket: &Path) -> Result<Self> {
        if !socket.is_absolute() {
            return Err(EngineError::Policy("engine socket must be absolute"));
        }
        let builder = || {
            Client::builder()
                .unix_socket(socket.to_path_buf())
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .retry(reqwest::retry::never())
                .connect_timeout(Duration::from_secs(10))
        };
        Ok(Self {
            control: builder()
                .timeout(Duration::from_secs(30))
                .build()
                .map_err(|_| EngineError::Transport)?,
            stream: builder().build().map_err(|_| EngineError::Transport)?,
        })
    }
    fn url(segments: &[&str]) -> Url {
        let mut url = Url::parse("http://engine/").expect("constant engine URL");
        url.path_segments_mut()
            .expect("constant base supports path segments")
            .push(API)
            .extend(segments);
        url
    }
    pub async fn image_environment(&self, image: &str) -> Result<Vec<String>> {
        let response = self
            .control
            .get(Self::url(&["images", image, "json"]))
            .send()
            .await
            .map_err(|_| EngineError::Transport)?;
        let value: ImageInspection = decode(response).await?;
        if value
            .config
            .volumes
            .is_some_and(|volumes| !volumes.is_empty())
        {
            return Err(EngineError::Policy(
                "image-declared anonymous volumes are unsupported",
            ));
        }
        Ok(value.config.env.unwrap_or_default())
    }
    /// Require the daemon's ordinary builtin seccomp boundary. The controller
    /// never installs an unconfined profile or relaxes parent proc masks.
    pub async fn verify_daemon_policy(&self) -> Result<()> {
        #[derive(Deserialize)]
        #[serde(rename_all = "PascalCase")]
        struct Info {
            security_options: Vec<String>,
            #[serde(rename = "OSType")]
            os_type: String,
        }
        let info: Info = decode(
            self.control
                .get(Self::url(&["info"]))
                .send()
                .await
                .map_err(|_| EngineError::Transport)?,
        )
        .await?;
        if info.os_type != "linux"
            || !info
                .security_options
                .iter()
                .any(|value| value == "name=seccomp,profile=builtin")
        {
            return Err(EngineError::Policy(
                "Linux Engine with builtin seccomp is required",
            ));
        }
        Ok(())
    }
    pub async fn create_once(&self, prepared: &PreparedContainer) -> Result<CreateOutcome> {
        let response = match self
            .control
            .post(Self::url(&["containers", "create"]))
            .query(&[("name", prepared.name())])
            .json(&prepared.body)
            .send()
            .await
        {
            Ok(response) => response,
            Err(_) => return Ok(CreateOutcome::ReconcileRequired),
        };
        if response.status() == StatusCode::CONFLICT || response.status().is_server_error() {
            return Ok(CreateOutcome::ReconcileRequired);
        }
        if response.status().is_client_error() {
            return Ok(CreateOutcome::DefinitivelyRejected {
                status: response.status().as_u16(),
            });
        }
        if response.status() != StatusCode::CREATED {
            return Ok(CreateOutcome::ReconcileRequired);
        }
        let value: CreateResponse = match decode(response).await {
            Ok(value) => value,
            Err(_) => return Ok(CreateOutcome::ReconcileRequired),
        };
        if !resource_id(&value.id) {
            return Ok(CreateOutcome::ReconcileRequired);
        }
        Ok(CreateOutcome::Created(OwnedContainer {
            name: prepared.name(),
            evidence: EngineResourceEvidence {
                engine_id: prepared.manifest.engine_id,
                resource_id: value.id,
                engine_name_nonce: prepared.manifest.engine_name_nonce,
                admitted_digest: prepared.public.admitted.clone(),
                public_manifest_digest: prepared.public.digest.clone(),
            },
        }))
    }
    /// None is an observation, NOT definitive absence after unknown create.
    pub async fn reconcile(&self, plan: &PublicPlan) -> Result<Option<OwnedContainer>> {
        let response = self
            .control
            .get(Self::url(&["containers", &plan.name(), "json"]))
            .send()
            .await
            .map_err(|_| EngineError::Transport)?;
        if response.status() == StatusCode::NOT_FOUND {
            return Ok(None);
        }
        let value: ContainerInspection = decode(response).await?;
        if !resource_id(&value.id)
            || value.name != format!("/{}", plan.name())
            || !plan
                .labels
                .iter()
                .all(|(key, expected)| value.config.labels.get(key) == Some(expected))
            || value.config.image != plan.manifest.image.reference
            || value.config.user != "1000:1000"
            || value.config.tty
            || value.host_config != plan.host
        {
            return Err(EngineError::OwnershipMismatch);
        }
        Ok(Some(OwnedContainer {
            name: plan.name(),
            evidence: EngineResourceEvidence {
                engine_id: plan.manifest.engine_id,
                resource_id: value.id,
                engine_name_nonce: plan.manifest.engine_name_nonce,
                admitted_digest: plan.admitted.clone(),
                public_manifest_digest: plan.digest.clone(),
            },
        }))
    }
    /// Cleanup authority needs no image allowlist or private invocation. Select
    /// only the CP-derived nonce/name and compare the original recorded ID and
    /// ownership labels. Policy drift cannot reauthorize execution, but must not
    /// prevent removing this exact owned resource. A 404 remains non-settlement.
    pub async fn reconcile_cleanup(
        &self,
        manifest: &LaunchManifest,
        original: &EngineResourceEvidence,
    ) -> Result<Option<OwnedContainer>> {
        let admitted = manifest
            .admitted_digest()
            .map_err(|_| EngineError::OwnershipMismatch)?;
        if original.engine_id != manifest.engine_id
            || original.engine_name_nonce != manifest.engine_name_nonce
            || original.admitted_digest != admitted
            || !resource_id(&original.resource_id)
        {
            return Err(EngineError::OwnershipMismatch);
        }
        let response = self
            .control
            .get(Self::url(&["containers", &manifest.engine_name(), "json"]))
            .send()
            .await
            .map_err(|_| EngineError::Transport)?;
        if response.status() == StatusCode::NOT_FOUND {
            return Ok(None);
        }
        let value: ContainerInspection = decode(response).await?;
        let expected = [
            (OWNER_LABEL, manifest.engine_id.to_string()),
            (LAUNCH_LABEL, manifest.launch_id.to_string()),
            (NONCE_LABEL, manifest.engine_name_nonce.to_string()),
            (DIGEST_LABEL, admitted.as_str().to_string()),
        ];
        if value.id != original.resource_id
            || value.name != format!("/{}", manifest.engine_name())
            || !expected
                .iter()
                .all(|(key, expected)| value.config.labels.get(*key) == Some(expected))
        {
            return Err(EngineError::OwnershipMismatch);
        }
        Ok(Some(OwnedContainer {
            evidence: original.clone(),
            name: manifest.engine_name(),
        }))
    }
    pub async fn start_once(&self, container: &OwnedContainer) -> Result<()> {
        let response = self
            .control
            .post(Self::url(&[
                "containers",
                &container.evidence.resource_id,
                "start",
            ]))
            .send()
            .await
            .map_err(|_| EngineError::Transport)?;
        if response.status() != StatusCode::NO_CONTENT {
            return Err(EngineError::Rejected(response.status().as_u16()));
        }
        Ok(())
    }
    pub async fn attach(&self, container: &OwnedContainer) -> Result<reqwest::Upgraded> {
        let response = self
            .stream
            .post(Self::url(&[
                "containers",
                &container.evidence.resource_id,
                "attach",
            ]))
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
            .map_err(|_| EngineError::Transport)?;
        if response.status() != StatusCode::SWITCHING_PROTOCOLS {
            return Err(EngineError::Rejected(response.status().as_u16()));
        }
        response.upgrade().await.map_err(|_| EngineError::Transport)
    }
    pub async fn wait(&self, container: &OwnedContainer) -> Result<ExitStatus> {
        let response = self
            .stream
            .post(Self::url(&[
                "containers",
                &container.evidence.resource_id,
                "wait",
            ]))
            .query(&[("condition", "not-running")])
            .send()
            .await
            .map_err(|_| EngineError::Transport)?;
        let value: WaitResponse = decode(response).await?;
        if value.error.is_some() {
            return Err(EngineError::InvalidResponse);
        }
        let inspection: ContainerInspection = decode(
            self.control
                .get(Self::url(&[
                    "containers",
                    &container.evidence.resource_id,
                    "json",
                ]))
                .send()
                .await
                .map_err(|_| EngineError::Transport)?,
        )
        .await?;
        if inspection.state.running {
            return Err(EngineError::InvalidResponse);
        }
        Ok(ExitStatus {
            exit_code: value.status_code,
            oom_killed: inspection.state.oom_killed,
        })
    }
    /// Stop is followed by acknowledged wait. A timeout is never a stopped proof.
    pub async fn stop_and_wait(
        &self,
        container: &OwnedContainer,
        grace_secs: u32,
    ) -> Result<ExitStatus> {
        let response = self
            .control
            .post(Self::url(&[
                "containers",
                &container.evidence.resource_id,
                "stop",
            ]))
            .query(&[("t", grace_secs.min(30))])
            .timeout(Duration::from_secs(u64::from(grace_secs.min(30)) + 15))
            .send()
            .await
            .map_err(|_| EngineError::Transport)?;
        if !matches!(
            response.status(),
            StatusCode::NO_CONTENT | StatusCode::NOT_MODIFIED
        ) {
            return Err(EngineError::Rejected(response.status().as_u16()));
        }
        tokio::time::timeout(Duration::from_secs(30), self.wait(container))
            .await
            .map_err(|_| EngineError::Transport)?
    }
    pub async fn remove_stopped(&self, container: &OwnedContainer) -> Result<()> {
        let response = self
            .control
            .delete(Self::url(&["containers", &container.evidence.resource_id]))
            .query(&[("force", "false"), ("v", "false")])
            .send()
            .await
            .map_err(|_| EngineError::Transport)?;
        if response.status() != StatusCode::NO_CONTENT {
            return Err(EngineError::Rejected(response.status().as_u16()));
        }
        Ok(())
    }
}
fn null_vec<'de, D: serde::Deserializer<'de>, T: Deserialize<'de>>(
    deserializer: D,
) -> std::result::Result<Vec<T>, D::Error> {
    Ok(Option::<Vec<T>>::deserialize(deserializer)?.unwrap_or_default())
}
fn resource_id(id: &str) -> bool {
    id.len() == 64
        && id
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
async fn decode<T: serde::de::DeserializeOwned>(response: reqwest::Response) -> Result<T> {
    if !response.status().is_success() {
        return Err(EngineError::Rejected(response.status().as_u16()));
    }
    if response
        .content_length()
        .is_some_and(|length| length > RESPONSE_LIMIT as u64)
    {
        return Err(EngineError::InvalidResponse);
    }
    let mut response = response;
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| EngineError::Transport)? {
        if bytes.len().saturating_add(chunk.len()) > RESPONSE_LIMIT {
            return Err(EngineError::InvalidResponse);
        }
        bytes.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&bytes).map_err(|_| EngineError::InvalidResponse)
}
#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct ImageInspection {
    config: ImageConfig,
}
#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct ImageConfig {
    env: Option<Vec<String>>,
    volumes: Option<BTreeMap<String, serde_json::Value>>,
}
#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct CreateResponse {
    id: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct ContainerInspection {
    id: String,
    name: String,
    config: ObservedConfig,
    host_config: HostPolicy,
    state: ObservedState,
}
#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct ObservedConfig {
    image: String,
    user: String,
    labels: BTreeMap<String, String>,
    tty: bool,
}
#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct ObservedState {
    running: bool,
    #[serde(rename = "OOMKilled")]
    oom_killed: bool,
}
#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct WaitResponse {
    status_code: i32,
    error: Option<serde_json::Value>,
}

/// Docker's non-TTY attach stream is multiplexed; headers never enter user logs.
pub async fn demultiplex<R: AsyncRead + Unpin, W: AsyncWrite + Unpin, E: AsyncWrite + Unpin>(
    mut source: R,
    mut stdout: W,
    mut stderr: E,
) -> Result<()> {
    loop {
        let mut header = [0u8; 8];
        let count = source.read(&mut header[..1]).await?;
        if count == 0 {
            break;
        }
        source.read_exact(&mut header[1..]).await?;
        if header[1..4] != [0, 0, 0] || !matches!(header[0], 1 | 2) {
            return Err(EngineError::InvalidResponse);
        }
        let size = u32::from_be_bytes(header[4..8].try_into().expect("fixed header")) as usize;
        if size > FRAME_LIMIT {
            return Err(EngineError::InvalidResponse);
        }
        let mut remaining = size;
        let mut buffer = [0u8; 65536];
        while remaining > 0 {
            let take = remaining.min(buffer.len());
            source.read_exact(&mut buffer[..take]).await?;
            if header[0] == 2 {
                stderr.write_all(&buffer[..take]).await?;
            } else {
                stdout.write_all(&buffer[..take]).await?;
            }
            remaining -= take;
        }
    }
    stdout.shutdown().await?;
    stderr.shutdown().await?;
    Ok(())
}

#[cfg(test)]
#[path = "engine_tests.rs"]
mod tests;
