//! Exact private input effect. The transaction owns source and origin locks,
//! epochs, operation and the one-shot marker through acknowledged COMMIT.
use super::*;
use crate::workload_stdin::AuthorizeLaunchStdinRequest;

fn exact_payload(selected: Option<&str>, payload: &[u8]) -> Result<()> {
    if selected.map(str::as_bytes).unwrap_or_default() != payload {
        return Err(denied());
    }
    Ok(())
}

impl WorkloadLaunchRepository {
    pub async fn authorize_stdin(
        pool: &PgPool,
        actor: ControllerActor,
        request: &AuthorizeLaunchStdinRequest,
        lease: &LaunchToken,
        grant: &LaunchToken,
        master: &str,
    ) -> Result<LaunchIoAuthorization> {
        let key = LaunchRequest {
            launch_id: request.handle.launch_id,
            admitted_digest: request.handle.admitted_digest.clone(),
        };
        let mut tx = pool.begin().await?;
        Self::actor(&mut tx, actor).await?;
        let snapshot = Self::record(&mut tx, &key, false).await?;
        if snapshot.engine != actor.engine || snapshot.manifest.purpose != LaunchPurpose::Action {
            return Err(denied());
        }
        let context = Self::private_context(&snapshot, master)?;
        exact_payload(context.stdin_json.as_deref(), request.payload.as_bytes())?;
        let expected = context
            .selected_input
            .as_ref()
            .ok_or_else(denied)?
            .public_snapshot()?;
        if !matches!(context.target, LaunchTarget::Execution { execution, .. } if execution==expected.execution())
        {
            return Err(denied());
        }
        // Match freeze's order: primary source -> input sources -> all epochs
        // -> operation -> grant. Never restore origins after locking operation.
        Self::source(&mut tx, &context.target, context.owner).await?;
        let mut input = ExecutionInputRepository::restore(
            &mut tx,
            expected.execution(),
            Some(master),
            Some(&expected),
        )
        .await?;
        let selected: serde_json::Value =
            serde_json::from_str(context.stdin_json.as_deref().ok_or_else(denied)?)
                .map_err(|_| denied())?;
        if &selected != input.value() {
            return Err(denied());
        }
        input.ensure_fresh().await?;
        let (_, _, _, expiry) = input.into_parts();
        let (epochs, source_deadline) = Self::epochs(&mut tx, &snapshot).await?;
        let mut record = Self::record(&mut tx, &key, true).await?;
        if !Self::same_epochs(&epochs, &record.source_epochs.0, false)
            || record.encrypted_context != snapshot.encrypted_context
        {
            return Err(denied());
        }
        let now = Self::now(&mut tx).await?;
        Self::require_lease(&record, actor, &request.handle, lease, now)?;
        if record.operation.cleanup_only()
            || record.operation.lease_purpose != LaunchLeasePurpose::Run
            || record.operation.status.runtime_state != LaunchRuntimeState::Running
            || record.operation.stdin_authorized
        {
            return Err(denied());
        }
        let permission: Option<(i64, DateTime<Utc>)> = sqlx::query_as(
            "SELECT max_bytes,expires_at FROM workload_launch_io_grant WHERE launch=$1 AND channel='stdin' AND token_hash=$2 AND expires_at>clock_timestamp() FOR SHARE",
        ).bind(record.id).bind(credential_hash(grant)).fetch_optional(&mut *tx).await?;
        let (max_bytes, grant_deadline) = permission.ok_or_else(denied)?;
        if request.payload.as_bytes().len() as u64
            > u64::try_from(max_bytes).map_err(|_| denied())?
        {
            return Err(denied());
        }
        let mut deadline = grant_deadline
            .min(record.operation.lease_deadline)
            .min(DateTime::from_timestamp(actor.expires_at, 0).ok_or_else(denied)?);
        if let Some(source) = source_deadline {
            deadline = deadline.min(source);
        }
        // Recheck after the operation/grant waits and before the durable effect.
        expiry.ensure_fresh_on(&mut tx).await?;
        Self::actor(&mut tx, actor).await?;
        if deadline <= Self::now(&mut tx).await? {
            return Err(denied());
        }
        record.operation.stdin_authorized = true;
        Self::save(&mut tx, &record).await?;
        expiry.ensure_fresh_on(&mut tx).await?;
        if deadline <= Self::now(&mut tx).await? {
            return Err(denied());
        }
        tx.commit().await?;
        expiry.ensure_fresh()?;
        if deadline <= Utc::now() {
            return Err(denied());
        }
        // Replays (including a lost COMMIT/HTTP ACK) now fail on the persisted
        // marker. Failure is never interpreted as permission for a second write.
        Ok(LaunchIoAuthorization {
            handle: request.handle.clone(),
            channel: LaunchIoChannel::Stdin,
            max_bytes: request.payload.as_bytes().len() as u64,
            expires_at: deadline,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn private_input_requires_exact_bytes_without_json_or_chunk_equivalence() {
        let selected = r#"{"plan":"original"}"#;
        assert!(exact_payload(Some(selected), selected.as_bytes()).is_ok());
        for changed in [
            br#"{"plan":"changed!"}"#.as_slice(),
            b"{ \"plan\":\"original\"}",
            b"original",
            b"",
        ] {
            assert!(exact_payload(Some(selected), changed).is_err());
        }
        assert!(exact_payload(None, b"").is_ok());
        assert!(exact_payload(None, b"{}").is_err());
    }
}
