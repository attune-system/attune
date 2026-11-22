//! Private HTTP protocol between the API and Attune workload managers.

use crate::models::{
    ExecutionStatus, Id, ManagerExecutionAssignment, Runtime, RuntimeVersion, WorkerRole,
};
use crate::runtime_lifecycle::PreparationScope;
use crate::{Error, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;
use uuid::Uuid;

pub const MANAGER_CONTROL_PATH: &str = "/api/v1/internal/managers";
pub const PACK_JOB_CLAIM_HEADER: &str = "x-attune-manager-pack-claim";
pub const PACK_JOB_HEADER: &str = "x-attune-manager-pack-job";
pub const WORK_SCOPE_HEADER: &str = "x-attune-manager-work";
pub const SENSOR_GENERATION_HEADER: &str = "x-attune-manager-generation";
pub const SESSION_TTL_SECONDS: i32 = 120;
pub const MAX_MANAGER_LOG_PLAINTEXT: usize = 16 * 1024 * 1024;

#[derive(Clone, Copy, Debug)]
pub enum PackTransferScope {
    PackJob { install: Id, claim: Uuid },
    Action { assignment: Id },
    Sensor { workload: Id, generation: i64 },
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ManagerEnrollmentGrant {
    pub role: WorkerRole,
    pub pool: String,
    /// Lowercase hexadecimal SHA-256 of a random, dedicated enrollment secret.
    pub secret_sha256: String,
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
    #[serde(default)]
    pub taints: Vec<crate::scheduling::WorkerTaint>,
}

impl ManagerEnrollmentGrant {
    pub fn validate_all(grants: &[Self]) -> Result<()> {
        let mut scopes = BTreeSet::new();
        let mut digests = BTreeSet::new();
        for grant in grants {
            crate::scheduling::validate_label_map("manager pool labels", &grant.labels)?;
            crate::scheduling::validate_taints(&grant.taints)?;
            if !valid_manager_name(&grant.pool)
                || grant.labels.contains_key("managed_pool")
                || grant.secret_sha256.len() != 64
                || !grant
                    .secret_sha256
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
                || !scopes.insert((grant.role as u8, &grant.pool))
                || !digests.insert(&grant.secret_sha256)
            {
                return Err(Error::configuration(
                    "Invalid or duplicate workload manager enrollment grant",
                ));
            }
        }
        Ok(())
    }
}

pub fn valid_manager_name(value: &str) -> bool {
    matches!(value.len(), 1..=63)
        && value.as_bytes()[0].is_ascii_alphanumeric()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ManagerEnrollment {
    pub role: WorkerRole,
    pub pool: String,
    pub name: String,
    /// Generate once per process start and reuse when retrying enrollment.
    pub instance: Uuid,
}

#[derive(Deserialize, Serialize)]
pub struct ManagerSession {
    pub id: Id,
    pub instance: Uuid,
    pub expires_at: DateTime<Utc>,
    pub token: String,
}

#[derive(Deserialize, Serialize)]
pub struct ManagerRuntimeCatalog {
    pub revision: i64,
    pub runtimes: Vec<Runtime>,
    pub versions: Vec<RuntimeVersion>,
    pub object_preparation: bool,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ManagerRuntimeReport {
    pub revision: i64,
    pub sequence: i64,
    pub runtimes: BTreeMap<Id, RuntimeProbe>,
    pub recipe_digests: BTreeSet<String>,
    pub preparation_scope: Option<PreparationScope>,
    pub runtime_filter: Option<Vec<String>>,
    #[serde(default)]
    pub pack_jobs_only: bool,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeProbe {
    pub default_available: bool,
    pub recipe_digest: Option<String>,
    pub primary_version: Option<String>,
    pub versions: BTreeSet<Id>,
}

#[derive(Deserialize, Serialize)]
pub struct RuntimeReportReceipt {
    pub worker: Id,
    pub revision: i64,
    pub sequence: i64,
}

#[derive(Deserialize, Serialize)]
pub struct PackJobOffer {
    pub install: Id,
    pub attempt: i32,
}

#[derive(Deserialize, Serialize)]
#[serde(tag = "source", rename_all = "snake_case")]
pub enum PackJobSource {
    Candidate { digest: String },
    Release { id: Id, digest: String },
}

#[derive(Deserialize, Serialize)]
pub struct ClaimedPackJob {
    pub install: Id,
    pub pack_ref: String,
    pub version: String,
    pub source: PackJobSource,
    pub preparation: Option<crate::repositories::runtime_preparation::PreparationJob>,
    pub python: Option<Runtime>,
    pub effects: Option<PackJobEffects>,
}

#[derive(Deserialize, Serialize)]
pub struct PackJobEffects {
    pub id: Uuid,
    pub token: String,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ClaimPackJob {
    pub attempt: i32,
    pub request: Uuid,
}

#[derive(Deserialize, Serialize)]
#[serde(tag = "result", rename_all = "snake_case", deny_unknown_fields)]
pub enum FinishPackJob {
    Tests {
        summary: crate::models::PackTestResult,
    },
    Prepared {
        receipt: crate::runtime_lifecycle::InstalledPreparation,
    },
    Failed,
}

#[derive(Deserialize, Serialize)]
pub struct WorkOffer {
    pub assignment: Id,
    pub execution: Id,
    pub pack_release: Id,
    pub pack_release_digest: String,
    pub deadline: DateTime<Utc>,
}

impl From<ManagerExecutionAssignment> for WorkOffer {
    fn from(row: ManagerExecutionAssignment) -> Self {
        Self {
            assignment: row.id,
            execution: row.execution,
            pack_release: row.pack_release,
            pack_release_digest: row.pack_release_digest,
            deadline: row.deadline,
        }
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ClaimWork {
    pub request: Uuid,
}

/// An immutable execution descriptor plus plaintext inputs for this one
/// attempt. Keep this out of diagnostics and the manager's inherited environment.
#[derive(Deserialize, Serialize)]
pub struct ActionLaunch {
    pub execution: Id,
    pub release: Id,
    pub digest: String,
    pub action: crate::models::Action,
    pub runtime: Runtime,
    pub version: Option<RuntimeVersion>,
    pub prepared: Option<crate::runtime_lifecycle::InstalledPreparation>,
    pub manifest: Option<crate::pack_format::Manifest>,
    pub schema: crate::repositories::schema_contract::ActionSchemaBinding,
    pub parameters: serde_json::Map<String, serde_json::Value>,
    pub secret_paths: Vec<String>,
    pub environment: std::collections::HashMap<String, String>,
    pub trace_tag: Option<String>,
    pub rule: Option<String>,
    pub trigger: Option<String>,
    pub timeout_seconds: u64,
    pub max_stdout_bytes: usize,
    pub max_stderr_bytes: usize,
    pub child_token: Option<String>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ActionProgram {
    pub request: Uuid,
    pub selected: crate::action_cli::SelectedProgram,
}

#[derive(Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LogChannel {
    Stdout,
    Stderr,
}

impl LogChannel {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Stdout => "stdout",
            Self::Stderr => "stderr",
        }
    }
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct OpenLog {
    pub request: Uuid,
    pub channel: LogChannel,
}

#[derive(Deserialize, Serialize)]
pub struct ActionLog {
    pub version: Id,
    pub stream: Id,
    pub worker: Id,
    pub attempt: Uuid,
    pub key: String,
    pub max_plaintext: usize,
    pub segment_bytes: usize,
    pub capture_token: String,
    pub recoverable: bool,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ResumeLog {
    pub custody: Uuid,
}

#[derive(Deserialize, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum LogResume {
    Waiting,
    Upload { token: String },
    Closed,
}

#[derive(Deserialize, Serialize)]
pub struct SensorLaunch {
    pub fence: crate::models::SensorWorkloadFence,
    pub executable: crate::models::SensorExecutableSnapshot,
    pub config: serde_json::Value,
    pub triggers: serde_json::Value,
    pub version: Option<RuntimeVersion>,
    pub prepared: Option<crate::runtime_lifecycle::InstalledPreparation>,
    pub token: String,
    pub token_expiry: String,
}

#[derive(Clone, Copy, Deserialize, Serialize)]
#[serde(tag = "state", rename_all = "snake_case", deny_unknown_fields)]
pub enum SensorProcessReport {
    Running { pid: i32 },
    Stopped,
    Failed { exit_code: Option<i32> },
}

impl SensorProcessReport {
    pub fn outcome(self) -> Option<&'static str> {
        match self {
            Self::Running { .. } => None,
            Self::Stopped => Some("stopped"),
            Self::Failed { .. } => Some("failed"),
        }
    }
}

#[derive(Deserialize, Serialize)]
pub struct SensorLog {
    pub version: Id,
    pub rotation_bytes: u64,
    pub rotates: bool,
    pub segment_bytes: usize,
}

#[derive(Deserialize, Serialize)]
pub struct SensorLease {
    pub expires: Option<DateTime<Utc>>,
}

#[derive(Deserialize, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum WorkLease {
    Running { deadline: DateTime<Utc> },
    Cancel,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct FinishWork {
    pub status: ExecutionStatus,
    pub result: serde_json::Map<String, serde_json::Value>,
}

#[derive(Deserialize, Serialize)]
pub struct WorkCompletion {
    pub status: ExecutionStatus,
}

/// A pooled manager transport. Credentials are passed per request and never
/// forwarded across redirects or through an environment-selected HTTP proxy.
#[derive(Clone)]
pub struct ManagerControlClient {
    client: reqwest::Client,
    api_url: String,
    base_url: String,
}

impl ManagerControlClient {
    pub fn new(api_url: &str, allow_plain_http: bool) -> Result<Self> {
        let url = url::Url::parse(api_url)
            .map_err(|_| Error::configuration("Invalid manager API URL"))?;
        if url.host().is_none()
            || url.fragment().is_some()
            || url.query().is_some()
            || !url.username().is_empty()
            || url.password().is_some()
            || !(url.scheme() == "https" || allow_plain_http && url.scheme() == "http")
        {
            return Err(Error::configuration("Invalid manager API URL or scheme"));
        }
        let client = crate::http_client::build_http_client(|| {
            reqwest::Client::builder()
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .connect_timeout(Duration::from_secs(5))
                .timeout(Duration::from_secs(10))
        })
        .map_err(|_| Error::configuration("Could not create manager API client"))?;
        let api_url = url.as_str().trim_end_matches('/').to_owned();
        Ok(Self {
            client,
            base_url: format!("{api_url}{MANAGER_CONTROL_PATH}"),
            api_url,
        })
    }

    pub fn api_url(&self) -> &str {
        &self.api_url
    }

    pub fn pack_transport(
        &self,
        session: tokio::sync::watch::Receiver<std::sync::Arc<ManagerSession>>,
        scope: PackTransferScope,
        packs_base_dir: &str,
    ) -> crate::pack_transport::ApiPackTransport {
        crate::pack_transport::ApiPackTransport::for_manager(
            self.api_url(),
            self.client.clone(),
            session,
            scope,
            packs_base_dir,
        )
    }

    pub async fn enroll(
        &self,
        secret: &str,
        enrollment: &ManagerEnrollment,
    ) -> Result<ManagerSession> {
        let session: ManagerSession = checked_response(
            self.client
                .post(format!("{}/enroll", self.base_url))
                .bearer_auth(secret)
                .json(enrollment),
        )
        .await?
        .json()
        .await
        .map_err(|_| invalid_response())?;
        if session.instance != enrollment.instance || session.id <= 0 {
            return Err(invalid_response());
        }
        Ok(session)
    }

    pub async fn renew(&self, current: &ManagerSession) -> Result<ManagerSession> {
        let session: ManagerSession = checked_response(
            self.client
                .post(format!("{}/session", self.base_url))
                .bearer_auth(&current.token),
        )
        .await?
        .json()
        .await
        .map_err(|_| invalid_response())?;
        if session.id != current.id || session.instance != current.instance {
            return Err(invalid_response());
        }
        Ok(session)
    }

    pub async fn withdraw(&self, current: &ManagerSession) -> Result<()> {
        checked_response(
            self.client
                .delete(format!("{}/session", self.base_url))
                .bearer_auth(&current.token),
        )
        .await?;
        Ok(())
    }

    pub async fn wait_work(&self, session: &ManagerSession) -> Result<Option<WorkOffer>> {
        checked_response(
            self.client
                .get(format!("{}/work", self.base_url))
                .bearer_auth(&session.token),
        )
        .await?
        .json()
        .await
        .map_err(|_| invalid_response())
    }

    /// Retain `request` across retries for the same local launch attempt.
    pub async fn claim_work(
        &self,
        session: &ManagerSession,
        offer: &WorkOffer,
        request: Uuid,
    ) -> Result<WorkOffer> {
        let claimed: WorkOffer = checked_response(
            self.work_request(session, offer.assignment, "claim")
                .json(&ClaimWork { request }),
        )
        .await?
        .json()
        .await
        .map_err(|_| invalid_response())?;
        if claimed.assignment != offer.assignment
            || claimed.execution != offer.execution
            || claimed.pack_release != offer.pack_release
            || claimed.pack_release_digest != offer.pack_release_digest
        {
            return Err(invalid_response());
        }
        Ok(claimed)
    }

    pub async fn renew_work(&self, session: &ManagerSession, assignment: Id) -> Result<WorkLease> {
        checked_response(self.work_request(session, assignment, "lease"))
            .await?
            .json()
            .await
            .map_err(|_| invalid_response())
    }

    pub async fn launch_action(
        &self,
        session: &ManagerSession,
        offer: &WorkOffer,
        request: Uuid,
    ) -> Result<ActionLaunch> {
        let launch: ActionLaunch = checked_response(
            self.work_request(session, offer.assignment, "launch")
                .json(&ClaimWork { request }),
        )
        .await?
        .json()
        .await
        .map_err(|_| invalid_response())?;
        if launch.execution != offer.execution
            || launch.release != offer.pack_release
            || launch.digest != offer.pack_release_digest
        {
            return Err(invalid_response());
        }
        Ok(launch)
    }

    pub async fn record_action_program(
        &self,
        session: &ManagerSession,
        assignment: Id,
        request: Uuid,
        selected: &crate::action_cli::SelectedProgram,
    ) -> Result<()> {
        checked_response(
            self.work_request(session, assignment, "program")
                .json(&ActionProgram {
                    request,
                    selected: selected.clone(),
                }),
        )
        .await?;
        Ok(())
    }

    /// Renew the lease and wait briefly for cancellation, without polling the
    /// API while the action keeps running.
    pub async fn watch_work(&self, session: &ManagerSession, assignment: Id) -> Result<WorkLease> {
        checked_response(self.work_request(session, assignment, "lease?wait=true"))
            .await?
            .json()
            .await
            .map_err(|_| invalid_response())
    }

    pub async fn finish_work(
        &self,
        session: &ManagerSession,
        assignment: Id,
        body: &FinishWork,
    ) -> Result<WorkCompletion> {
        checked_response(self.work_request(session, assignment, "finish").json(body))
            .await?
            .json()
            .await
            .map_err(|_| invalid_response())
    }

    pub async fn drain_work(&self, session: &ManagerSession) -> Result<()> {
        checked_response(
            self.client
                .post(format!("{}/work/drain", self.base_url))
                .bearer_auth(&session.token),
        )
        .await?;
        Ok(())
    }

    pub async fn open_action_log(
        &self,
        session: &ManagerSession,
        assignment: Id,
        body: &OpenLog,
    ) -> Result<ActionLog> {
        checked_response(self.work_request(session, assignment, "logs").json(body))
            .await?
            .json()
            .await
            .map_err(|_| invalid_response())
    }

    pub async fn prepare_action_log(
        &self,
        session: &ManagerSession,
        version: Id,
        body: &crate::repositories::log_delivery::PrepareLogDelivery,
    ) -> Result<()> {
        checked_response(
            self.client
                .post(format!("{}/logs/{version}/prepare", self.base_url))
                .bearer_auth(&session.token)
                .json(body),
        )
        .await?;
        Ok(())
    }

    pub async fn resume_log(
        &self,
        session: &ManagerSession,
        version: Id,
        custody: Uuid,
    ) -> Result<LogResume> {
        checked_response(
            self.client
                .post(format!("{}/logs/{version}/resume", self.base_url))
                .bearer_auth(&session.token)
                .json(&ResumeLog { custody }),
        )
        .await?
        .json()
        .await
        .map_err(|_| invalid_response())
    }

    pub fn artifact_transport(
        &self,
        token: &str,
        local_dir: &str,
    ) -> crate::artifact_transport::ApiTransport {
        crate::artifact_transport::ApiTransport::scoped(
            self.api_url(),
            self.client.clone(),
            token,
            local_dir,
        )
    }

    pub async fn available_sensors(&self, session: &ManagerSession) -> Result<Vec<Id>> {
        checked_response(
            self.client
                .get(format!("{}/sensors", self.base_url))
                .bearer_auth(&session.token),
        )
        .await?
        .json()
        .await
        .map_err(|_| invalid_response())
    }

    pub async fn start_sensor(
        &self,
        session: &ManagerSession,
        sensor: Id,
        request: Uuid,
    ) -> Result<SensorLaunch> {
        let launch: SensorLaunch = checked_response(
            self.client
                .post(format!("{}/sensors/{sensor}/start", self.base_url))
                .bearer_auth(&session.token)
                .json(&ClaimWork { request }),
        )
        .await?
        .json()
        .await
        .map_err(|_| invalid_response())?;
        if launch.executable.sensor.id != sensor || launch.fence.worker_instance != session.instance
        {
            return Err(invalid_response());
        }
        Ok(launch)
    }

    pub async fn report_sensor_process(
        &self,
        session: &ManagerSession,
        fence: crate::models::SensorWorkloadFence,
        state: SensorProcessReport,
    ) -> Result<()> {
        checked_response(
            self.client
                .post(format!(
                    "{}/sensor-workloads/{}/{}/process",
                    self.base_url, fence.workload_id, fence.generation,
                ))
                .bearer_auth(&session.token)
                .json(&state),
        )
        .await?;
        Ok(())
    }

    pub async fn open_sensor_log(
        &self,
        session: &ManagerSession,
        fence: crate::models::SensorWorkloadFence,
        body: &OpenLog,
    ) -> Result<SensorLog> {
        checked_response(
            self.client
                .post(format!(
                    "{}/sensor-workloads/{}/{}/logs",
                    self.base_url, fence.workload_id, fence.generation,
                ))
                .bearer_auth(&session.token)
                .json(body),
        )
        .await?
        .json()
        .await
        .map_err(|_| invalid_response())
    }

    pub async fn renew_sensor(
        &self,
        session: &ManagerSession,
        fence: crate::models::SensorWorkloadFence,
    ) -> Result<SensorLease> {
        checked_response(self.sensor_request(reqwest::Method::POST, session, fence))
            .await?
            .json()
            .await
            .map_err(|_| invalid_response())
    }

    pub async fn release_sensor(
        &self,
        session: &ManagerSession,
        fence: crate::models::SensorWorkloadFence,
    ) -> Result<()> {
        checked_response(self.sensor_request(reqwest::Method::DELETE, session, fence)).await?;
        Ok(())
    }

    fn sensor_request(
        &self,
        method: reqwest::Method,
        session: &ManagerSession,
        fence: crate::models::SensorWorkloadFence,
    ) -> reqwest::RequestBuilder {
        self.client
            .request(
                method,
                format!(
                    "{}/sensor-workloads/{}/{}",
                    self.base_url, fence.workload_id, fence.generation
                ),
            )
            .bearer_auth(&session.token)
    }

    /// Supply the previously applied revision for a cheap unchanged check.
    pub async fn runtime_catalog(
        &self,
        session: &ManagerSession,
        since: Option<i64>,
    ) -> Result<Option<ManagerRuntimeCatalog>> {
        let mut url = format!("{}/runtime", self.base_url);
        if let Some(revision) = since {
            url.push_str(&format!("?since={revision}"));
        }
        checked_response(self.client.get(url).bearer_auth(&session.token))
            .await?
            .json()
            .await
            .map_err(|_| invalid_response())
    }

    pub async fn report_runtime(
        &self,
        session: &ManagerSession,
        report: &ManagerRuntimeReport,
    ) -> Result<RuntimeReportReceipt> {
        checked_response(
            self.client
                .post(format!("{}/runtime", self.base_url))
                .bearer_auth(&session.token)
                .json(report),
        )
        .await?
        .json()
        .await
        .map_err(|_| invalid_response())
    }

    pub async fn wait_pack_job(&self, session: &ManagerSession) -> Result<Option<PackJobOffer>> {
        checked_response(
            self.client
                .get(format!("{}/pack-jobs", self.base_url))
                .bearer_auth(&session.token),
        )
        .await?
        .json()
        .await
        .map_err(|_| invalid_response())
    }

    pub async fn claim_pack_job(
        &self,
        session: &ManagerSession,
        offer: &PackJobOffer,
        request: Uuid,
    ) -> Result<ClaimedPackJob> {
        let job: ClaimedPackJob = checked_response(
            self.client
                .post(format!(
                    "{}/pack-jobs/{}/claim",
                    self.base_url, offer.install
                ))
                .bearer_auth(&session.token)
                .json(&ClaimPackJob {
                    attempt: offer.attempt,
                    request,
                }),
        )
        .await?
        .json()
        .await
        .map_err(|_| invalid_response())?;
        if job.install != offer.install {
            return Err(invalid_response());
        }
        Ok(job)
    }

    pub async fn renew_pack_job(
        &self,
        session: &ManagerSession,
        install: Id,
        claim: Uuid,
    ) -> Result<Option<DateTime<Utc>>> {
        checked_response(self.pack_job_request(session, install, claim, "lease"))
            .await?
            .json()
            .await
            .map_err(|_| invalid_response())
    }

    pub async fn finish_pack_job(
        &self,
        session: &ManagerSession,
        install: Id,
        claim: Uuid,
        result: &FinishPackJob,
    ) -> Result<()> {
        checked_response(
            self.pack_job_request(session, install, claim, "finish")
                .timeout(Duration::from_secs(60))
                .json(result),
        )
        .await?;
        Ok(())
    }

    fn pack_job_request(
        &self,
        session: &ManagerSession,
        install: Id,
        claim: Uuid,
        operation: &str,
    ) -> reqwest::RequestBuilder {
        self.client
            .post(format!("{}/pack-jobs/{install}/{operation}", self.base_url))
            .bearer_auth(&session.token)
            .header(PACK_JOB_CLAIM_HEADER, claim.to_string())
    }

    fn work_request(
        &self,
        session: &ManagerSession,
        assignment: Id,
        operation: &str,
    ) -> reqwest::RequestBuilder {
        self.client
            .post(format!("{}/work/{assignment}/{operation}", self.base_url))
            .bearer_auth(&session.token)
    }
}

async fn checked_response(request: reqwest::RequestBuilder) -> Result<reqwest::Response> {
    let response = request
        .send()
        .await
        .map_err(|_| Error::RetryableTransport("Manager API request failed".into()))?;
    match response.status() {
        status if status.is_success() => Ok(response),
        reqwest::StatusCode::UNAUTHORIZED | reqwest::StatusCode::FORBIDDEN => Err(
            Error::PermissionDenied("Manager API credential denied".into()),
        ),
        reqwest::StatusCode::CONFLICT => Err(Error::invalid_state("Manager API state conflict")),
        status if status.is_server_error() || status == reqwest::StatusCode::TOO_MANY_REQUESTS => {
            Err(Error::RetryableTransport(format!(
                "Manager API temporarily unavailable ({status})"
            )))
        }
        status => Err(Error::external_service(format!(
            "Manager API request rejected ({status})"
        ))),
    }
}

fn invalid_response() -> Error {
    Error::RetryableTransport("Invalid manager API response".into())
}
