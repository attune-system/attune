//! Assignment delivery to the execution-only manager. This consumer admits a
//! HostActor directly; it never fabricates or returns a controller credential.
use super::*;
use crate::{
    repositories::{
        file_evidence_retention::FileEvidenceRetentionRepository as Graph,
        workload_host::WorkloadHostRepository,
    },
    workload_host::{AssignedAction, HostActor},
};

impl WorkloadLaunchRepository {
    /// Manager refreshes only its exact assigned channel. Controller sessions
    /// and launch leases stay in the separate trusted controller service.
    pub async fn host_io_grant(
        pool: &PgPool,
        actor: HostActor,
        request: &LaunchIoGrantRequest,
    ) -> Result<LaunchIoGrant> {
        let mut tx = pool.begin().await?;
        Graph::lock_capture(&mut tx).await?;
        let host_deadline = WorkloadHostRepository::admit(&mut tx, actor).await?;
        let snapshot = Self::record(&mut tx, &request.request, false).await?;
        let profile:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM workload_host_profile WHERE id=$1 AND role='action' AND image=$2 AND NOT disabled)")
            .bind(actor.profile).bind(snapshot.image).fetch_one(&mut *tx).await?;
        if !matches!(snapshot.manifest.target,LaunchTarget::Execution{worker,worker_instance,..}
            if worker==actor.worker && worker_instance==actor.instance)
            || snapshot.manifest.purpose != LaunchPurpose::Action
            || !profile
        {
            return Err(denied());
        }
        let capture = request.channel != LaunchIoChannel::Stdin;
        let (epochs, source_deadline) = Self::epochs_for(&mut tx, &snapshot, capture).await?;
        let mut record = Self::record(&mut tx, &request.request, true).await?;
        let now = Self::now(&mut tx).await?;
        record.operation.expire_capture(now);
        if !Self::same_epochs(&epochs, &record.source_epochs, capture)
            || (!capture && (record.operation.cleanup_only() || record.operation.stdin_authorized))
            || (capture && !record.operation.capture_pending())
        {
            return Err(denied());
        }
        let mut expires_at = (now + chrono::Duration::seconds(30))
            .min(host_deadline)
            .min(DateTime::from_timestamp(actor.expires_at, 0).ok_or_else(denied)?);
        if let Some(source) = source_deadline {
            expires_at = expires_at.min(source);
        }
        if let Some(capture) = record.operation.status.capture_deadline {
            expires_at = expires_at.min(capture);
        }
        let max_bytes = if capture {
            1024 * 1024
        } else {
            crate::workload_stdin::MAX_PRIVATE_STDIN_BYTES as u64
        };
        let token = new_token()?;
        sqlx::query("INSERT INTO workload_launch_io_grant(launch,channel,token_hash,max_bytes,expires_at) VALUES($1,$2,$3,$4,$5) ON CONFLICT(launch,channel) DO UPDATE SET token_hash=EXCLUDED.token_hash,max_bytes=EXCLUDED.max_bytes,expires_at=EXCLUDED.expires_at")
            .bind(record.id).bind(serde_json::to_value(request.channel)?.as_str().ok_or_else(denied)?)
            .bind(credential_hash(&token)).bind(max_bytes as i64).bind(expires_at).execute(&mut *tx).await?;
        WorkloadHostRepository::admit(&mut tx, actor).await?;
        if expires_at <= Self::now(&mut tx).await? {
            return Err(denied());
        }
        Self::save(&mut tx, &record).await?;
        tx.commit().await?;
        if expires_at <= Utc::now() {
            return Err(denied());
        }
        Ok(LaunchIoGrant {
            request: request.request.clone(),
            channel: request.channel,
            token,
            max_bytes,
            expires_at,
        })
    }
    pub async fn assigned_action(
        pool: &PgPool,
        actor: HostActor,
        known: &[i64],
        master: &str,
        max_stdout_bytes: u64,
        max_stderr_bytes: u64,
    ) -> Result<Option<AssignedAction>> {
        if known.len() > 1024
            || known.iter().any(|id| *id <= 0)
            || max_stdout_bytes == 0
            || max_stderr_bytes == 0
        {
            return Err(denied());
        }
        let mut tx = pool.begin().await?;
        Graph::lock_capture(&mut tx).await?;
        let host_deadline = WorkloadHostRepository::admit(&mut tx, actor).await?;
        // IDs are discovery only. Exact immutable target, source, image and
        // worker-incarnation checks follow before any private context leaves CP.
        let candidate:Option<(i64,String)>=sqlx::query_as(
            "SELECT l.id,l.admitted_digest FROM workload_launch l JOIN execution e ON e.id=(l.manifest#>>'{target,execution}')::bigint \
             WHERE l.manifest->>'purpose'='action' AND l.manifest#>>'{target,worker}'=$1 \
             AND l.manifest#>>'{target,worker_instance}'=$2 AND e.worker=$3 AND e.status='running' \
             AND NOT(e.id=ANY($4)) ORDER BY l.id LIMIT 1",
        ).bind(actor.worker.to_string()).bind(actor.instance.to_string()).bind(actor.worker)
            .bind(known).fetch_optional(&mut *tx).await?;
        let Some((launch_id, digest)) = candidate else {
            tx.commit().await?;
            return Ok(None);
        };
        let request = LaunchRequest {
            launch_id,
            admitted_digest: Sha256Digest::parse(digest)?,
        };
        let snapshot = Self::record(&mut tx, &request, false).await?;
        let execution = match snapshot.manifest.target {
            LaunchTarget::Execution {
                execution,
                worker,
                worker_instance,
                ..
            } if worker == actor.worker && worker_instance == actor.instance => execution,
            _ => return Err(denied()),
        };
        let context = Self::private_context(&snapshot, master)?;
        let expected = context
            .selected_input
            .as_ref()
            .ok_or_else(denied)?
            .public_snapshot()?;
        if expected.execution() != execution {
            return Err(denied());
        }
        Self::source(&mut tx, &snapshot.manifest.target, snapshot.owner).await?;
        let mut input =
            ExecutionInputRepository::restore(&mut tx, execution, Some(master), Some(&expected))
                .await?;
        let selected = context.stdin_json.as_deref().ok_or_else(denied)?;
        if &serde_json::from_str::<serde_json::Value>(selected).map_err(|_| denied())?
            != input.value()
        {
            return Err(denied());
        }
        input.ensure_fresh().await?;
        let (_, _, _, expiry) = input.into_parts();
        let (epochs, source_deadline) = Self::epochs(&mut tx, &snapshot).await?;
        let record = Self::record(&mut tx, &request, true).await?;
        if !Self::same_epochs(&epochs, &record.source_epochs, false)
            || record.encrypted_context != snapshot.encrypted_context
            || record.operation.cleanup_only()
            || record.operation.stdin_authorized
        {
            return Err(denied());
        }
        let (controller_origin,encrypted_dispatch):(String,String)=sqlx::query_as(
            "SELECT e.controller_url,l.encrypted_dispatch FROM workload_launch l \
             JOIN workload_host_profile p ON p.id=$2 AND p.image=l.image AND p.role='action' AND NOT p.disabled \
             JOIN workload_engine e ON e.id=l.engine WHERE l.id=$1",
        ).bind(record.id).bind(actor.profile).fetch_optional(&mut *tx).await?.ok_or_else(denied)?;
        let dispatch =
            LaunchToken::new(crypto::decrypt(&encrypted_dispatch, master).map_err(|_| denied())?)?;
        if credential_hash(&dispatch) != record.dispatch_hash {
            return Err(denied());
        }
        let now = Self::now(&mut tx).await?;
        let mut expires_at = (now + chrono::Duration::seconds(30))
            .min(host_deadline)
            .min(DateTime::from_timestamp(actor.expires_at, 0).ok_or_else(denied)?);
        if let Some(source) = source_deadline {
            expires_at = expires_at.min(source);
        }
        let mut issue = async |channel: LaunchIoChannel, max_bytes: u64| -> Result<LaunchIoGrant> {
            let token = new_token()?;
            sqlx::query("INSERT INTO workload_launch_io_grant(launch,channel,token_hash,max_bytes,expires_at) VALUES($1,$2,$3,$4,$5) ON CONFLICT(launch,channel) DO UPDATE SET token_hash=EXCLUDED.token_hash,max_bytes=EXCLUDED.max_bytes,expires_at=EXCLUDED.expires_at")
                .bind(record.id).bind(serde_json::to_value(channel)?.as_str().ok_or_else(denied)?)
                .bind(credential_hash(&token)).bind(max_bytes as i64).bind(expires_at).execute(&mut *tx).await?;
            Ok(LaunchIoGrant {
                request: request.clone(),
                channel,
                token,
                max_bytes,
                expires_at,
            })
        };
        let stdin_grant = issue(
            LaunchIoChannel::Stdin,
            crate::workload_stdin::MAX_PRIVATE_STDIN_BYTES as u64,
        )
        .await?;
        let stdout_grant = issue(LaunchIoChannel::Stdout, 1024 * 1024).await?;
        let stderr_grant = issue(LaunchIoChannel::Stderr, 1024 * 1024).await?;
        expiry.ensure_fresh_on(&mut tx).await?;
        WorkloadHostRepository::admit(&mut tx, actor).await?;
        if expires_at <= Self::now(&mut tx).await? {
            return Err(denied());
        }
        let assignment = AssignedAction {
            execution,
            launch: ReservedLaunch {
                request,
                dispatch_token: dispatch,
            },
            controller_origin,
            stdin_json: selected.into(),
            stdin_grant,
            stdout_grant,
            stderr_grant,
            max_stdout_bytes,
            max_stderr_bytes,
        };
        tx.commit().await?;
        expiry.ensure_fresh()?;
        if expires_at <= Utc::now() {
            return Err(denied());
        }
        Ok(Some(assignment))
    }
}
