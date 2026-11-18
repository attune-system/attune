//! In-process effects used by the trusted worker and sensor loops.
//!
//! The caller owns admission, attempts, origins, retries, encryption, custody
//! and completion. An adapter owns only an exact Pod/container and its streams.
//! No HTTP principal, channel grant or domain scheduler crosses this seam.
use crate::{
    workload_launch::{
        LaunchExitOutcome, LaunchInvocation, LaunchPlatform, LaunchPurpose, LaunchStopReason,
        LaunchTarget, PackAccess, PinnedImage, PinnedLaunchInput, PrivateStage,
    },
    workload_stdin::LaunchStdinPayload,
    Result,
};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::{fmt, pin::Pin, sync::Arc};
use tokio::io::AsyncRead;
use uuid::Uuid;

/// These names are backend policy, never selectors supplied by pack code.
pub const PAYLOAD_CONTAINER: &str = "payload";
pub const TRANSFER_CONTAINER: &str = "transfer";

/// Private, already-admitted execution material. The trusted loop persists its
/// encrypted snapshot and exact attempt binding before calling the adapter.
/// This deliberately has no general serializer or public plaintext digest.
#[derive(Clone)]
pub struct ProcessSpec {
    pub intent: Uuid,
    pub purpose: LaunchPurpose,
    pub target: LaunchTarget,
    pub image: PinnedImage,
    pub platform: LaunchPlatform,
    pub inputs: Vec<PinnedLaunchInput>,
    pub pack_access: PackAccess,
    pub stage: Option<PrivateStage>,
    pub timeout_ms: Option<u64>,
    pub invocation: LaunchInvocation,
}
impl fmt::Debug for ProcessSpec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ProcessSpec")
            .field("intent", &self.intent)
            .field("purpose", &self.purpose)
            .field("platform", &self.platform)
            .field("input_count", &self.inputs.len())
            .field("invocation", &self.invocation)
            .finish_non_exhaustive()
    }
}

/// Persisted provider evidence, not permission to select another resource.
/// Backends must verify the recorded nonce/ownership before every effect.
/// Kubernetes API operations locate by name AND verify UID; fixed container
/// names above are not fields that a caller can change.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ResourceIdentity {
    Docker {
        engine_id: i64,
        daemon_id: String,
        container_id: String,
    },
    Kubernetes {
        cluster: String,
        namespace: String,
        pod_name: String,
        pod_uid: String,
    },
}

pub type ProcessReader = Pin<Box<dyn AsyncRead + Send>>;

/// Separate non-TTY streams attached while the payload is still held at its
/// fixed Runner gate. Adapter buffering must be bounded. Stream loss is an
/// incomplete capture, never invented EOF or permission to replay stdin.
pub struct ProcessIo {
    pub resource: ResourceIdentity,
    pub stdout: ProcessReader,
    pub stderr: ProcessReader,
    pub control: Arc<dyn ProcessControl>,
}

#[async_trait]
pub trait ProcessControl: Send + Sync {
    /// The caller commits its exact-input/attempt one-shot fence FIRST. This
    /// opens the fixed gate and forwards these same bytes, including empty
    /// input, without a newline, re-read or chunk substitution. All subsequent
    /// calls fail, including after an uncertain/lost acknowledgement.
    async fn write_stdin_once(&self, payload: LaunchStdinPayload) -> Result<()>;

    /// Observing exit never implies log custody, publication or completion.
    async fn wait(&self) -> Result<LaunchExitOutcome>;

    /// Terminate the exact payload, including one still held at the gate, and
    /// await termination. Retain helper/private staging for receipt-gated file
    /// publication. Errors/404/timeouts do not prove provider absence.
    async fn stop(&self, reason: LaunchStopReason) -> Result<LaunchExitOutcome>;
}

/// Durable native-effect decision committed before a provider request. A
/// resource lookup returning 404 never changes ReconcileOnly back to CreateOnce.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProcessCreateDisposition {
    CreateOnce,
    ReconcileOnly,
}

#[async_trait]
pub trait ProcessLauncher: Send + Sync {
    /// Create/recover only the recorded intent, with pack execution held at a
    /// credential-free fixed gate. Unknown create settlement is reconciled;
    /// absence after a timed-out create is not authority for a duplicate.
    async fn prepare(
        &self,
        spec: &ProcessSpec,
        create: ProcessCreateDisposition,
    ) -> Result<ResourceIdentity>;

    /// Caller has durably recorded this exact resource. Attach separate streams
    /// before returning; this MUST NOT release pack execution. An already-exited
    /// resource is not restarted. Inherited image/service env is cleared before
    /// the admitted invocation context is installed by the Runner.
    async fn start(&self, spec: &ProcessSpec, resource: &ResourceIdentity) -> Result<ProcessIo>;
}
