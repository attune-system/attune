//! Ephemeral source read authority for PRIVATE same-transaction wait staging.
//! No ID-only/public constructor, serde form, cache exception or release fact.
use super::*;

#[derive(Clone)]
pub(crate) struct IssuedWaitRead {
    transaction: String,
    renderer: Id,
    snapshot: Id,
    wait: Id,
    task_path: String,
    source_digest: String,
    value_digest: String,
    fence: ApprovalEffectFence,
}
impl PreparedWaitInput {
    pub(crate) fn issued_reads(&self) -> Result<Vec<IssuedWaitRead>> {
        let mut reads = self.read_dependencies.clone();
        reads.push(self.issued_read()?);
        if reads.len() > 256 {
            return Err(denied());
        }
        Ok(reads)
    }
    pub(crate) fn issued_read(&self) -> Result<IssuedWaitRead> {
        self.fence.ensure_fresh()?;
        if self.source.prepared_transaction != self.transaction
            || self.source.wait <= 0
            || self.snapshot <= 0
        {
            return Err(denied());
        }
        Ok(IssuedWaitRead {
            transaction: self.transaction.clone(),
            renderer: self.renderer,
            snapshot: self.snapshot,
            wait: self.source.wait,
            task_path: self.task_path.clone(),
            source_digest: hash(&serde_json::to_value(&self.source)?)?,
            value_digest: hash(&self.value)?,
            fence: self.fence.clone(),
        })
    }
}
impl IssuedWaitRead {
    pub(crate) fn snapshot(&self) -> Id {
        self.snapshot
    }
    pub(crate) fn merge(&mut self, other: &Self) -> Result<()> {
        if self.snapshot != other.snapshot
            || self.wait != other.wait
            || self.renderer != other.renderer
            || self.transaction != other.transaction
            || self.task_path != other.task_path
            || self.source_digest != other.source_digest
            || self.value_digest != other.value_digest
        {
            return Err(denied());
        }
        self.fence = self.fence.intersect(&other.fence);
        Ok(())
    }
    async fn load(
        &self,
        conn: &mut PgConnection,
        snapshot: Id,
        renderer: Id,
        key: &str,
    ) -> Result<(Body, Value, usize)> {
        require_transaction(conn, &self.transaction).await?;
        self.fence.ensure_fresh_on(conn).await?;
        if self.snapshot != snapshot || self.renderer != renderer {
            return Err(denied());
        }
        let (body, value, bytes, _) = load(conn, snapshot, key).await?;
        let Source::Wait(source) = &body.source else {
            return Err(denied());
        };
        if body.renderer != renderer
            || body.task_path != self.task_path
            || source.wait != self.wait
            || source.prepared_transaction != self.transaction
            || hash(&serde_json::to_value(source)?)? != self.source_digest
            || hash(&value)? != self.value_digest
        {
            return Err(denied());
        }
        validate_wait_value(conn, snapshot, source, &value, key).await?;
        Ok((body, value, bytes))
    }
    /// Only record_prepared_inner's private staging path calls this. Real final
    /// recording/publication still requires the normalized wait release.
    pub(crate) async fn require_stage(
        &self,
        conn: &mut PgConnection,
        snapshot: Id,
        renderer: Id,
        key: &str,
    ) -> Result<()> {
        self.load(conn, snapshot, renderer, key).await?;
        Ok(())
    }
    /// The same canonical graph reader uses this ONLY when an opaque issued
    /// read was carried from binding/preparation. Bare IDs cannot request it.
    pub(crate) async fn evidence(
        &self,
        conn: &mut PgConnection,
        snapshot: Id,
        renderer: Id,
        declaration_path: &str,
        context_path: &str,
        key: &str,
    ) -> Result<SnapshotEvidence> {
        let (body, value, bytes) = self.load(conn, snapshot, renderer, key).await?;
        let Source::Wait(source) = &body.source else {
            return Err(denied());
        };
        if !in_task(&body.task_path, declaration_path) {
            return Err(denied());
        }
        let outcome = ["succeeded", "failed", "timed_out", "cancelled"]
            .iter()
            .any(|name| context_path == format!("/$outcome/{name}"));
        if outcome {
            if !wait_outcome_path(&body.task_path, declaration_path) {
                return Err(denied());
            }
            native_wait_outcome_values(source)?;
        } else if context_path != source.context_path
            && !context_path
                .strip_prefix(&source.context_path)
                .is_some_and(|suffix| suffix.starts_with('/'))
        {
            return Err(denied());
        }
        let record = RuntimeAuthorityRepository::require_retained_admission(conn, renderer).await?;
        if record.owner_id != body.execution {
            return Err(denied());
        }
        require_task(conn, &record, &body.task_path, &body.authority).await?;
        // Source phase only. Retain all independent original fences; do not
        // reacquire these authorities at target-side publication.
        let fence = self
            .fence
            .intersect(&RuntimeAuthorityRepository::retained_admission_fence(conn, renderer).await?)
            .intersect(
                &RuntimeAuthorityRepository::retained_admission_fence(
                    conn,
                    source.source_admission,
                )
                .await?,
            )
            .intersect(&ApprovalEffectFence::from_input_approval(
                body.approval_deadline,
            )?);
        fence.ensure_fresh_on(conn).await?;
        validate_wait_value(conn, snapshot, source, &value, key).await?;
        Ok(SnapshotEvidence {
            fence,
            origins: source.origins.clone(),
            parents: source.parents.clone(),
            metadata: metadata(snapshot, &body),
            bytes,
        })
    }
}
