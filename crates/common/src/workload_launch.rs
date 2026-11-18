//! Canonical v1 control-plane/launcher contract. Callers submit only admitted
//! IDs and digests. Images, invocation, mounts and ownership are resolved by the
//! control plane; this module exposes no Docker HostConfig or caller host path.
//!
//! The public manifest binds a random-nonce-encrypted frozen context reference,
//! never an unsalted digest of plaintext arguments, environment or parameters.
//! Invocation material is released only to the admitted controller/custodian.
//! Launch credentials and per-stream keys never enter a pack invocation.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Deserializer, Serialize};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, fmt};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::{Error, Result};

pub const LAUNCH_DISPATCH_HEADER: &str = "x-attune-launch-dispatch";
pub const LAUNCH_LEASE_HEADER: &str = "x-attune-launch-lease";
pub const LAUNCH_OUTPUT_HEADER: &str = "x-attune-launch-output";
pub const LAUNCH_IO_HEADER: &str = "x-attune-launch-io";
pub const LAUNCH_ARCHIVE_CONTENT_TYPE: &str = "application/zip";
pub const LAUNCH_IO_NEXT_OFFSET_HEADER: &str = "x-attune-launch-next-offset";
pub const LAUNCH_IO_EOF_HEADER: &str = "x-attune-launch-eof";
pub const LAUNCH_IO_TRUNCATED_HEADER: &str = "x-attune-launch-truncated-bytes";
pub const PACK_ROOT: &str = "/opt/attune/pack";
pub const RUNTIME_ROOT: &str = "/opt/attune/runtime";
pub const RUNNER_ROOT: &str = "/opt/attune/runner";
pub const SCRATCH_ROOT: &str = "/tmp";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, ToSchema)]
#[serde(transparent)]
#[schema(value_type = String)]
pub struct Sha256Digest(String);
impl Sha256Digest {
    pub fn parse(value: impl Into<String>) -> Result<Self> {
        let value = value.into();
        if value.len() != 64
            || !value
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(Error::validation("Expected a lowercase SHA-256 digest"));
        }
        Ok(Self(value))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
    pub fn of_bytes(bytes: &[u8]) -> Self {
        Self(
            Sha256::digest(bytes)
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect(),
        )
    }
}
impl<'de> Deserialize<'de> for Sha256Digest {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        Self::parse(String::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum LaunchPurpose {
    Action,
    Sensor,
    Setup,
    RuntimeProbe,
    CandidateTest,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum LaunchTarget {
    Execution {
        execution: i64,
        worker: i64,
        worker_instance: Uuid,
        attempt: Uuid,
    },
    Sensor {
        sensor: i64,
        workload: i64,
        worker: i64,
        worker_instance: Uuid,
        generation: i64,
    },
    Setup {
        admission: i64,
        attempt: Uuid,
    },
    RuntimeProbe {
        admission: i64,
        attempt: Uuid,
    },
    CandidateTest {
        install: i64,
        test_job: i64,
        attempt: Uuid,
    },
}

/// Admission request contains references only. The actor and target determine
/// every image, input pin, owner, invocation and permitted engine server-side.
#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ReserveLaunchRequest {
    pub purpose: LaunchPurpose,
    pub target: LaunchTarget,
}

/// The complete request accepted by the engine-facing launch transport.
/// Authorization is carried separately; an admitted digest is not a credential.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct LaunchRequest {
    pub launch_id: i64,
    pub admitted_digest: Sha256Digest,
}

/// Engine identity is immutable across controller restarts. Replacing a daemon
/// or changing its native platform requires a new engine ID; runtime cache
/// compatibility is never inferred from a multiarch index digest alone.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub enum LaunchPlatform {
    #[serde(rename = "linux/amd64")]
    LinuxAmd64,
    #[serde(rename = "linux/arm64")]
    LinuxArm64,
}
impl LaunchPlatform {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::LinuxAmd64 => "linux/amd64",
            Self::LinuxArm64 => "linux/arm64",
        }
    }
    pub fn native_arch(self) -> &'static str {
        match self {
            Self::LinuxAmd64 => "amd64",
            Self::LinuxArm64 => "arm64",
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct LaunchEngineBinding {
    pub engine_id: i64,
    pub daemon_id: String,
    pub platform: LaunchPlatform,
}
impl LaunchEngineBinding {
    pub fn validate_shape(&self) -> Result<()> {
        if self.engine_id <= 0
            || self.daemon_id.is_empty()
            || self.daemon_id.len() > 256
            || self.daemon_id.bytes().any(|byte| !byte.is_ascii_graphic())
        {
            return Err(Error::validation("Invalid admitted engine binding"));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct PinnedImage {
    pub reference: String,
    pub sha256: Sha256Digest,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct FrozenContextRef {
    pub id: i64,
    pub version: i64,
    pub ciphertext_sha256: Sha256Digest,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum LaunchInputKind {
    Pack,
    Runtime,
    Runner,
}
impl LaunchInputKind {
    pub fn container_root(self) -> &'static str {
        match self {
            Self::Pack => PACK_ROOT,
            Self::Runtime => RUNTIME_ROOT,
            Self::Runner => RUNNER_ROOT,
        }
    }
}

/// The body is fetched through the exact launch/pin endpoint, not a supplied
/// URL or tar. Body format is canonical ZIP (application/zip), with no fallback
/// reader. Size/hash and expanded quotas are verified before publication.
/// Runner input contains public immutable helper bytes, never credentials.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct PinnedLaunchInput {
    pub pin_id: i64,
    pub kind: LaunchInputKind,
    pub object_id: i64,
    pub object_version: String,
    pub size_bytes: u64,
    pub sha256: Sha256Digest,
    pub max_expanded_bytes: u64,
    pub max_files: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct PrivateStage {
    pub cache_key: String,
    pub quota_bytes: u64,
    pub quota_files: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum PackAccess {
    ReadOnly,
    CandidateCopy,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum LaunchRoot {
    Pack,
    Runtime,
    Scratch,
    Stage,
}
impl LaunchRoot {
    pub fn container_path(self) -> &'static str {
        match self {
            Self::Pack => PACK_ROOT,
            Self::Runtime | Self::Stage => RUNTIME_ROOT,
            Self::Scratch => SCRATCH_ROOT,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct LaunchManifest {
    pub launch_id: i64,
    pub purpose: LaunchPurpose,
    pub target: LaunchTarget,
    pub owner_identity_id: i64,
    pub engine_id: i64,
    pub engine_name_nonce: Uuid,
    pub image: PinnedImage,
    pub context: FrozenContextRef,
    pub inputs: Vec<PinnedLaunchInput>,
    pub pack_access: PackAccess,
    pub stage: Option<PrivateStage>,
    pub timeout_ms: Option<u64>,
    /// Initial admission deadline. Runtime renewals change the separate lease,
    /// never this immutable manifest or its digest (notably for managed sensors).
    pub lease_deadline: DateTime<Utc>,
}
impl LaunchManifest {
    pub fn admitted_digest(&self) -> Result<Sha256Digest> {
        self.validate_shape()?;
        Ok(Sha256Digest::of_bytes(&serde_json::to_vec(self).map_err(
            |_| Error::validation("Launch manifest cannot be encoded"),
        )?))
    }
    pub fn engine_name(&self) -> String {
        format!("attune-launch-{}", self.engine_name_nonce.simple())
    }
    pub fn validate_shape(&self) -> Result<()> {
        let invalid = || Error::validation("Invalid launch manifest");
        if self.launch_id <= 0
            || self.owner_identity_id <= 0
            || self.engine_id <= 0
            || self.engine_name_nonce.is_nil()
            || self.context.id <= 0
            || self.context.version <= 0
            || self.timeout_ms == Some(0)
        {
            return Err(invalid());
        }
        let purpose_matches = matches!(
            (self.purpose, &self.target),
            (LaunchPurpose::Action, LaunchTarget::Execution { .. })
                | (LaunchPurpose::Sensor, LaunchTarget::Sensor { .. })
                | (LaunchPurpose::Setup, LaunchTarget::Setup { .. })
                | (
                    LaunchPurpose::RuntimeProbe,
                    LaunchTarget::RuntimeProbe { .. }
                )
                | (
                    LaunchPurpose::CandidateTest,
                    LaunchTarget::CandidateTest { .. }
                )
        );
        if !purpose_matches {
            return Err(invalid());
        }
        match &self.target {
            LaunchTarget::Execution {
                execution,
                worker,
                worker_instance,
                attempt,
            } => {
                if *execution <= 0 || *worker <= 0 || worker_instance.is_nil() || attempt.is_nil() {
                    return Err(invalid());
                }
            }
            LaunchTarget::Sensor {
                sensor,
                workload,
                worker,
                worker_instance,
                generation,
            } => {
                if *sensor <= 0
                    || *workload <= 0
                    || *worker <= 0
                    || *generation <= 0
                    || worker_instance.is_nil()
                {
                    return Err(invalid());
                }
            }
            LaunchTarget::Setup { admission, attempt }
            | LaunchTarget::RuntimeProbe { admission, attempt } => {
                if *admission <= 0 || attempt.is_nil() {
                    return Err(invalid());
                }
            }
            LaunchTarget::CandidateTest {
                install,
                test_job,
                attempt,
            } => {
                if *install <= 0 || *test_job <= 0 || attempt.is_nil() {
                    return Err(invalid());
                }
            }
        }
        let pinned_ref = format!("sha256:{}", self.image.sha256.as_str());
        if self.image.reference != pinned_ref
            && !self.image.reference.ends_with(&format!("@{pinned_ref}"))
        {
            return Err(invalid());
        }
        if self.image.reference.contains("://")
            || self.image.reference.matches('@').count() > 1
            || !self
                .image
                .reference
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"./:_@-".contains(&b))
        {
            return Err(invalid());
        }
        let mut kinds = Vec::new();
        let mut pins = std::collections::BTreeSet::new();
        for input in &self.inputs {
            if input.pin_id <= 0
                || input.object_id <= 0
                || input.object_version.is_empty()
                || input.object_version.len() > 1024
                || input
                    .object_version
                    .bytes()
                    .any(|byte| byte.is_ascii_control())
                || input.size_bytes == 0
                || input.max_expanded_bytes == 0
                || input.max_files == 0
                || kinds.contains(&input.kind)
                || !pins.insert(input.pin_id)
            {
                return Err(invalid());
            }
            kinds.push(input.kind);
        }
        if self.purpose == LaunchPurpose::Setup {
            let stage = self.stage.as_ref().ok_or_else(invalid)?;
            if stage.cache_key.is_empty()
                || stage.cache_key.len() > 128
                || !stage
                    .cache_key
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
                || stage.quota_bytes == 0
                || stage.quota_files == 0
                || kinds.contains(&LaunchInputKind::Runtime)
            {
                return Err(invalid());
            }
        } else if self.stage.is_some() {
            return Err(invalid());
        }
        if (self.purpose == LaunchPurpose::CandidateTest)
            != (self.pack_access == PackAccess::CandidateCopy)
        {
            return Err(invalid());
        }
        if self.purpose == LaunchPurpose::CandidateTest && !kinds.contains(&LaunchInputKind::Pack) {
            return Err(invalid());
        }
        if self.purpose != LaunchPurpose::Sensor && self.timeout_ms.is_none() {
            return Err(invalid());
        }
        Ok(())
    }
}

/// Private CP materialization of the encrypted context. Never accept this from
/// a runner request, hash it into a public digest, or include its values in logs.
/// Per-stream keys, engine credentials and reusable enrollment are not fields.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct LaunchInvocation {
    pub argv: Vec<String>,
    pub env: BTreeMap<String, String>,
    pub working_root: LaunchRoot,
}
impl fmt::Debug for LaunchInvocation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LaunchInvocation")
            .field("argc", &self.argv.len())
            .field("env_count", &self.env.len())
            .field("working_root", &self.working_root)
            .finish_non_exhaustive()
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
#[schema(as = WorkloadLaunchSpec)]
pub struct LaunchSpec {
    pub manifest: LaunchManifest,
    pub invocation: LaunchInvocation,
}
impl LaunchSpec {
    pub fn validate_shape(&self) -> Result<()> {
        self.manifest.validate_shape()?;
        let invalid = || Error::validation("Invalid private launch invocation");
        if self.invocation.argv.is_empty() || self.invocation.argv[0].is_empty() {
            return Err(invalid());
        }
        let has_input = |kind| self.manifest.inputs.iter().any(|input| input.kind == kind);
        match self.invocation.working_root {
            LaunchRoot::Stage if self.manifest.purpose != LaunchPurpose::Setup => {
                return Err(invalid())
            }
            LaunchRoot::Runtime
                if !has_input(LaunchInputKind::Runtime) && self.manifest.stage.is_none() =>
            {
                return Err(invalid())
            }
            LaunchRoot::Pack if !has_input(LaunchInputKind::Pack) => return Err(invalid()),
            _ => {}
        }
        if matches!(
            self.manifest.purpose,
            LaunchPurpose::Setup | LaunchPurpose::RuntimeProbe
        ) && self
            .invocation
            .env
            .keys()
            .any(|name| name.eq_ignore_ascii_case("ATTUNE_API_TOKEN"))
        {
            return Err(invalid());
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum LaunchState {
    Reserved,
    CreateIntent,
    CreateUnknown,
    Created,
    /// Exited normally; creation is settled but removal is still required.
    CleanupRequired,
    StopRequired,
    Stopped,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum LaunchRuntimeState {
    Unstarted,
    StartIntent,
    StartUnknown,
    Running,
    Exited,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum LaunchLeasePurpose {
    Run,
    Cleanup,
    /// Successful removed Setup only; no engine/invocation/input authority.
    Publication,
    /// Finite retained stdout/stderr only after engine removal.
    Capture,
}

/// Identity only. The secret lease credential travels in LAUNCH_LEASE_HEADER.
/// Engine resource IDs are evidence and never accepted as caller selectors.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct LaunchHandle {
    pub launch_id: i64,
    pub admitted_digest: Sha256Digest,
    pub lease_id: Uuid,
    pub lease_generation: i64,
    pub lease_deadline: DateTime<Utc>,
    pub engine_id: i64,
    pub engine_name_nonce: Uuid,
}

#[derive(Clone, Serialize, ToSchema)]
#[serde(transparent)]
#[schema(value_type = String)]
pub struct LaunchToken(String);
impl LaunchToken {
    pub fn new(value: String) -> Result<Self> {
        if value.is_empty() || value.len() > 16384 || value.bytes().any(|b| !b.is_ascii_graphic()) {
            return Err(Error::validation("Invalid launch credential"));
        }
        Ok(Self(value))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
impl<'de> Deserialize<'de> for LaunchToken {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        Self::new(String::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}
impl fmt::Debug for LaunchToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("LaunchToken([redacted])")
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ReservedLaunch {
    pub request: LaunchRequest,
    pub dispatch_token: LaunchToken,
}
/// Controller restart discovery is engine-scoped by authentication. Complete a
/// bounded cursor round, then begin a new round; unfinished rows are not consumed.
#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ReconcileLaunchesRequest {
    pub after: Option<i64>,
    pub limit: u32,
}
#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ReconcileLaunchesPage {
    pub items: Vec<ReservedLaunch>,
    pub next_cursor: Option<i64>,
}
#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct LaunchLease {
    pub handle: LaunchHandle,
    pub purpose: LaunchLeasePurpose,
    pub token: LaunchToken,
}
#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
pub enum ClaimedLaunch {
    Active {
        spec: Box<LaunchSpec>,
        lease: LaunchLease,
    },
    Cleanup {
        manifest: Box<LaunchManifest>,
        /// Recorded original evidence remains usable after source/image revocation.
        status: LaunchStatus,
        lease: LaunchLease,
    },
    Capture {
        manifest: Box<LaunchManifest>,
        status: LaunchStatus,
        lease: LaunchLease,
    },
    Publication {
        manifest: Box<LaunchManifest>,
        status: LaunchStatus,
        lease: LaunchLease,
    },
    Stopped {
        status: LaunchStatus,
    },
}
#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct LaunchLeaseRequest {
    pub handle: LaunchHandle,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum CreateDisposition {
    CreateOnce,
    ReconcileOnly,
    StopOnly,
}
#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct LaunchCreateIntent {
    pub handle: LaunchHandle,
    pub create_nonce: Uuid,
    pub disposition: CreateDisposition,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum StartDisposition {
    StartOnce,
    ReconcileOnly,
    StopOnly,
}
#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct LaunchStartIntent {
    pub handle: LaunchHandle,
    pub start_nonce: Uuid,
    pub disposition: StartDisposition,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct EngineResourceEvidence {
    pub engine_id: i64,
    pub resource_id: String,
    pub engine_name_nonce: Uuid,
    pub admitted_digest: Sha256Digest,
    /// Digest of the validated public engine manifest, excluding env/argv/stdin.
    pub public_manifest_digest: Sha256Digest,
}
#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum LaunchObservation {
    CreateUnknown {
        create_nonce: Uuid,
    },
    Created {
        create_nonce: Uuid,
        resource: EngineResourceEvidence,
    },
    StartUnknown {
        start_nonce: Uuid,
        resource: EngineResourceEvidence,
    },
    Started {
        start_nonce: Uuid,
        resource: EngineResourceEvidence,
    },
    Exited {
        start_nonce: Uuid,
        resource: EngineResourceEvidence,
        exit_code: i32,
        oom_killed: bool,
    },
}
#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ObserveLaunchRequest {
    pub handle: LaunchHandle,
    pub observation: LaunchObservation,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum LaunchStopReason {
    Cancelled,
    Timeout,
    SourceRevoked,
    LeaseExpired,
    Shutdown,
    LaunchFailure,
}
#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct StopLaunchRequest {
    pub request: LaunchRequest,
    pub reason: LaunchStopReason,
}
#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ProviderCreateSettlement {
    NeverSubmitted,
    DefinitiveAbsence {
        create_nonce: Uuid,
    },
    Created {
        create_nonce: Uuid,
        resource: EngineResourceEvidence,
    },
}
/// DefinitiveAbsence requires settled provider ownership, not a timeout/404.
/// Unknown creates remain StopRequired until owned resource settlement is proved.
/// A created resource requires Wait and successful Remove before acknowledgement.
/// Mere exit/stop is insufficient: an extant container could regain its RW stage.
#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct StopLaunchAcknowledgement {
    pub handle: LaunchHandle,
    pub create: ProviderCreateSettlement,
    pub removed_resource: Option<EngineResourceEvidence>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct LaunchExitOutcome {
    pub exit_code: i32,
    pub oom_killed: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum LaunchPublicationState {
    NotRequired,
    Pending,
    Uploading,
    Published,
    Rejected,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum LaunchCaptureState {
    Pending,
    Complete,
    Incomplete,
}
#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct LaunchStatus {
    pub request: LaunchRequest,
    pub state: LaunchState,
    pub runtime_state: LaunchRuntimeState,
    pub exit_outcome: Option<LaunchExitOutcome>,
    pub publication: LaunchPublicationState,
    pub capture: LaunchCaptureState,
    pub capture_deadline: Option<DateTime<Utc>>,
    pub resource: Option<EngineResourceEvidence>,
    pub stop_reason: Option<LaunchStopReason>,
}

impl LaunchStatus {
    /// Historical success survives removal, without implying live authority.
    pub fn permits_stage_publication(&self) -> bool {
        self.validate_shape().is_ok()
            && matches!(
                self.publication,
                LaunchPublicationState::Pending
                    | LaunchPublicationState::Uploading
                    | LaunchPublicationState::Published
            )
            && self.state == LaunchState::Stopped
            && self.stop_reason.is_none()
            && self.exit_outcome
                == Some(LaunchExitOutcome {
                    exit_code: 0,
                    oom_killed: false,
                })
    }
    pub fn validate_shape(&self) -> Result<()> {
        if self.request.launch_id <= 0
            || (self.runtime_state == LaunchRuntimeState::Exited) != self.exit_outcome.is_some()
            || (self.runtime_state == LaunchRuntimeState::Exited && self.resource.is_none())
            || (self.state == LaunchState::Stopped
                && self.capture == LaunchCaptureState::Pending
                && self.capture_deadline.is_none())
            || (self.state == LaunchState::CleanupRequired
                && (self.runtime_state != LaunchRuntimeState::Exited || self.stop_reason.is_some()))
            || (self.state == LaunchState::StopRequired && self.stop_reason.is_none())
            || self
                .resource
                .as_ref()
                .is_some_and(|resource| resource.admitted_digest != self.request.admitted_digest)
        {
            return Err(Error::validation("Invalid launch status"));
        }
        Ok(())
    }
}

/// Raw stdin/stdout/stderr are distinct from encoded/published log artifacts.
/// These credentials stay in non-pack managers/custodians, not invocation envs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum LaunchIoChannel {
    Stdin,
    Stdout,
    Stderr,
}
#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct LaunchIoGrantRequest {
    pub request: LaunchRequest,
    pub channel: LaunchIoChannel,
}
#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct LaunchIoGrant {
    pub request: LaunchRequest,
    pub channel: LaunchIoChannel,
    pub token: LaunchToken,
    pub max_bytes: u64,
    pub expires_at: DateTime<Utc>,
}
/// Controller authenticates with its CP credential and current lease, forwarding
/// the manager's exact channel credential in LAUNCH_IO_HEADER before byte effects.
#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct AuthorizeLaunchIoRequest {
    pub request: LaunchRequest,
    pub channel: LaunchIoChannel,
    pub offset: u64,
    pub bytes: u64,
}
#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct LaunchIoAuthorization {
    pub handle: LaunchHandle,
    pub channel: LaunchIoChannel,
    pub max_bytes: u64,
    pub expires_at: DateTime<Utc>,
}
/// A finite raw-byte read permits credential refresh between reads for sensors.
/// Tokens are headers, never query parameters. Missing bytes are capture failure.
#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct LaunchIoReadQuery {
    pub admitted_digest: Sha256Digest,
    pub offset: u64,
    pub max_bytes: u32,
}
#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct LaunchOutputBodyQuery {
    pub admitted_digest: Sha256Digest,
}
#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct LaunchStageResult {
    pub output_id: i64,
    pub object_id: i64,
    pub object_version: String,
    pub size_bytes: u64,
    pub sha256: Sha256Digest,
    pub validation_receipt: i64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum LaunchOutputKind {
    Stdout,
    Stderr,
    RuntimeResult,
    CandidateResult,
    StageArchive,
}
#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ReserveLaunchOutput {
    pub handle: LaunchHandle,
    pub kind: LaunchOutputKind,
    pub size_bytes: u64,
    /// StageArchive requires successful Exited + Stopped (settled create/removal),
    /// never CleanupRequired or StopRequired. Its body is canonical ZIP.
    /// For protected output this is the ciphertext body hash, not plaintext.
    pub sha256: Sha256Digest,
}
#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct LaunchOutputGrant {
    pub output_id: i64,
    pub token: LaunchToken,
    pub expires_at: DateTime<Utc>,
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn runner_request_rejects_engine_configuration_and_paths() {
        let json = serde_json::json!({"launch_id":1,"admitted_digest":"a".repeat(64),"HostConfig":{"Privileged":true}});
        assert!(serde_json::from_value::<LaunchRequest>(json).is_err());
    }
    #[test]
    fn private_invocation_and_credentials_are_not_debug_values() {
        let invocation = LaunchInvocation {
            argv: vec!["owned-secret-argument".into()],
            env: BTreeMap::from([("APP_KEY".into(), "owned-secret-value".into())]),
            working_root: LaunchRoot::Scratch,
        };
        let debug = format!("{invocation:?}");
        assert!(!debug.contains("owned-secret"));
        assert!(!format!(
            "{:?}",
            LaunchToken::new("owned-secret-token".into()).unwrap()
        )
        .contains("owned-secret"));
    }
    #[test]
    fn credential_deserialization_cannot_bypass_validation() {
        for value in ["", "with space", "with\ncontrol", "with\u{0085}control"] {
            assert!(serde_json::from_value::<LaunchToken>(serde_json::json!(value)).is_err());
        }
        assert!(
            serde_json::from_value::<LaunchToken>(serde_json::json!("a".repeat(16385))).is_err()
        );
    }
    #[test]
    fn stage_publication_requires_removed_success_and_preserves_cancellation() {
        let manifest = manifest();
        let resource = EngineResourceEvidence {
            engine_id: manifest.engine_id,
            resource_id: "owned-engine-resource".into(),
            engine_name_nonce: manifest.engine_name_nonce,
            admitted_digest: manifest.admitted_digest().unwrap(),
            public_manifest_digest: Sha256Digest::of_bytes(b"owned-policy"),
        };
        let mut status = LaunchStatus {
            request: LaunchRequest {
                launch_id: manifest.launch_id,
                admitted_digest: resource.admitted_digest.clone(),
            },
            state: LaunchState::CleanupRequired,
            publication: LaunchPublicationState::Pending,
            capture: LaunchCaptureState::Pending,
            capture_deadline: Some(Utc::now() + chrono::Duration::seconds(60)),
            runtime_state: LaunchRuntimeState::Exited,
            exit_outcome: Some(LaunchExitOutcome {
                exit_code: 0,
                oom_killed: false,
            }),
            resource: Some(resource),
            stop_reason: None,
        };
        assert!(status.validate_shape().is_ok());
        assert!(!status.permits_stage_publication());
        status.state = LaunchState::Stopped;
        assert!(status.permits_stage_publication());
        status.stop_reason = Some(LaunchStopReason::Cancelled);
        assert!(!status.permits_stage_publication());
        status.state = LaunchState::StopRequired;
        assert!(status.validate_shape().is_ok());
        status.stop_reason = None;
        assert!(status.validate_shape().is_err());
    }
    fn manifest() -> LaunchManifest {
        let hash = Sha256Digest::of_bytes(b"owned-public-fixture");
        LaunchManifest {
            launch_id: 1,
            purpose: LaunchPurpose::Action,
            target: LaunchTarget::Execution {
                execution: 2,
                worker: 3,
                worker_instance: Uuid::new_v4(),
                attempt: Uuid::new_v4(),
            },
            owner_identity_id: 4,
            engine_id: 5,
            engine_name_nonce: Uuid::new_v4(),
            image: PinnedImage {
                reference: format!("sha256:{}", hash.as_str()),
                sha256: hash.clone(),
            },
            context: FrozenContextRef {
                id: 6,
                version: 1,
                ciphertext_sha256: hash.clone(),
            },
            inputs: vec![PinnedLaunchInput {
                pin_id: 7,
                kind: LaunchInputKind::Pack,
                object_id: 8,
                object_version: "immutable-version".into(),
                size_bytes: 10,
                sha256: hash,
                max_expanded_bytes: 100,
                max_files: 10,
            }],
            pack_access: PackAccess::ReadOnly,
            stage: None,
            timeout_ms: Some(1000),
            lease_deadline: Utc::now() + chrono::Duration::minutes(1),
        }
    }
    #[test]
    fn setup_cannot_mount_a_published_runtime_as_its_writable_stage() {
        let mut manifest = manifest();
        manifest.purpose = LaunchPurpose::Setup;
        manifest.target = LaunchTarget::Setup {
            admission: 9,
            attempt: Uuid::new_v4(),
        };
        manifest.stage = Some(PrivateStage {
            cache_key: "owned-stage".into(),
            quota_bytes: 100,
            quota_files: 10,
        });
        let mut runtime = manifest.inputs[0].clone();
        runtime.kind = LaunchInputKind::Runtime;
        runtime.pin_id = 10;
        manifest.inputs.push(runtime);
        assert!(manifest.validate_shape().is_err());
        manifest.inputs.pop();
        assert!(manifest.validate_shape().is_ok());
        let mut spec = LaunchSpec {
            manifest,
            invocation: LaunchInvocation {
                argv: vec!["/bin/true".into()],
                env: BTreeMap::new(),
                working_root: LaunchRoot::Stage,
            },
        };
        assert!(spec.validate_shape().is_ok());
        spec.invocation
            .env
            .insert("ATTUNE_API_TOKEN".into(), "owned-forbidden-token".into());
        assert!(spec.validate_shape().is_err());
    }
    #[test]
    fn writable_pack_copy_cannot_authorize_normal_action_or_runtime_publication() {
        let mut manifest = manifest();
        manifest.pack_access = PackAccess::CandidateCopy;
        assert!(manifest.validate_shape().is_err());
        manifest.purpose = LaunchPurpose::CandidateTest;
        manifest.target = LaunchTarget::CandidateTest {
            install: 9,
            test_job: 10,
            attempt: Uuid::new_v4(),
        };
        assert!(manifest.validate_shape().is_ok());
        manifest.stage = Some(PrivateStage {
            cache_key: "forbidden-publication".into(),
            quota_bytes: 100,
            quota_files: 10,
        });
        assert!(manifest.validate_shape().is_err());
    }
}
