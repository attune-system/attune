//! Private, same-transaction ResultView reads. Never a Wait capability or a
//! persisted release. Each scope was issued from an owning view and declaration.
use super::*;
use std::collections::BTreeSet;

#[derive(Clone)]
pub(crate) struct IssuedResultRead {
    transaction: String,
    renderer: Id,
    snapshot: Id,
    source_digest: String,
    value_digest: String,
    scopes: BTreeSet<String>,
    fence: ApprovalEffectFence,
}

impl PreparedNativeResultView {
    pub(crate) fn prospective_result_reads(&self, scope: &str) -> Result<Vec<IssuedResultRead>> {
        self.ensure_fresh()?;
        // Archived views keep the existing released-only path. They cannot mint
        // an ephemeral read for a different preparation transaction.
        if self.archived {
            return Ok(Vec::new());
        }
        Ok(vec![self.issued_result_read(scope)?])
    }
    pub(crate) fn issued_result_read(&self, scope: &str) -> Result<IssuedResultRead> {
        self.ensure_fresh()?;
        if self.archived || self.source.transaction != self.transaction || self.snapshot <= 0 {
            return Err(denied());
        }
        Ok(IssuedResultRead {
            transaction: self.transaction.clone(),
            renderer: self.renderer,
            snapshot: self.snapshot,
            source_digest: hash(&serde_json::to_value(&self.source)?)?,
            value_digest: hash(&self.value)?,
            scopes: [scope.to_owned()].into_iter().collect(),
            fence: self.fence.clone(),
        })
    }

    pub(crate) async fn require_condition_scope(
        &self,
        conn: &mut PgConnection,
        path: &str,
    ) -> Result<()> {
        self.ensure_fresh_on(conn).await?;
        let child =
            RuntimeAuthorityRepository::require_retained_admission(conn, self.source.admission)
                .await?;
        let record =
            RuntimeAuthorityRepository::require_retained_admission(conn, self.renderer).await?;
        let InvocationBinding::WorkflowTask {
            workflow_ref,
            task_path,
            ..
        } = &child.authority.binding
        else {
            return Err(denied());
        };
        let graph = workflow_definition(conn, &record).await?;
        let task = canonical_task_path(&graph, task_path).ok_or_else(denied)?;
        let prefix = format!("/workflow{task}/next/");
        let suffix = path.strip_prefix(&prefix).ok_or_else(denied)?;
        let parts: Vec<_> = suffix.split('/').collect();
        if child.owner_id != self.source.execution
            || workflow_ref != record.authority.binding.action_ref()
            || !matches!(parts.as_slice(), [index,"when"] if index.parse::<usize>().is_ok())
            || graph
                .pointer(path.strip_prefix("/workflow").ok_or_else(denied)?)
                .is_none_or(Value::is_null)
        {
            return Err(denied());
        }
        Ok(())
    }
}

impl IssuedResultRead {
    pub(crate) fn snapshot(&self) -> Id {
        self.snapshot
    }
    pub(crate) fn merge(&mut self, other: &Self) -> Result<()> {
        if self.transaction != other.transaction
            || self.renderer != other.renderer
            || self.snapshot != other.snapshot
            || self.source_digest != other.source_digest
            || self.value_digest != other.value_digest
        {
            return Err(denied());
        }
        self.scopes.extend(other.scopes.iter().cloned());
        if self.scopes.len() > 256 {
            return Err(denied());
        }
        self.fence = self.fence.intersect(&other.fence);
        Ok(())
    }
    pub(crate) async fn require_stage(
        &self,
        conn: &mut PgConnection,
        snapshot: Id,
        renderer: Id,
        key: &str,
    ) -> Result<()> {
        require_transaction(conn, &self.transaction).await?;
        self.fence.ensure_fresh_on(conn).await?;
        let (body, value, _, _) = load(conn, snapshot, key).await?;
        let Source::ResultView(source) = &body.source else {
            return Err(denied());
        };
        if self.snapshot != snapshot
            || self.renderer != renderer
            || body.renderer != renderer
            || source.transaction != self.transaction
            || hash(&serde_json::to_value(source)?)? != self.source_digest
            || hash(&value)? != self.value_digest
        {
            return Err(denied());
        }
        result_view::validate_value(conn, snapshot, source, &value, key).await?;
        self.fence.ensure_fresh_on(conn).await
    }
    pub(crate) async fn evidence(
        &self,
        conn: &mut PgConnection,
        snapshot: Id,
        renderer: Id,
        declaration_path: &str,
        context_path: &str,
        key: &str,
    ) -> Result<SnapshotEvidence> {
        if !self.scopes.contains(declaration_path) {
            return Err(denied());
        }
        self.require_stage(conn, snapshot, renderer, key).await?;
        let mut evidence = NativeInputSnapshotRepository::evidence(
            conn,
            snapshot,
            renderer,
            declaration_path,
            context_path,
            key,
            true,
        )
        .await?;
        evidence.fence = evidence.fence.intersect(&self.fence);
        Ok(evidence)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    // These inert comparison fixtures never reach an issuer, load or DB gate.
    fn inert_read(scope: &str) -> IssuedResultRead {
        IssuedResultRead {
            transaction: "comparison-only".into(),
            renderer: 1,
            snapshot: 2,
            source_digest: "source".into(),
            value_digest: "value".into(),
            scopes: [scope.into()].into_iter().collect(),
            fence: ApprovalEffectFence::from_input_approval(None).unwrap(),
        }
    }
    #[test]
    fn merging_held_scopes_preserves_the_earliest_independent_deadline() {
        let mut first = inert_read("/workflow/tasks/0/next/0/when");
        let mut second = inert_read("$native/value");
        let deadline = chrono::Utc::now() + chrono::Duration::hours(1);
        second.fence = ApprovalEffectFence::from_input_approval(Some(deadline)).unwrap();
        first.merge(&second).unwrap();
        assert_eq!(first.scopes.len(), 2);
        assert_eq!(first.fence.approval_deadline(), Some(deadline));
    }
    #[test]
    fn mismatched_transaction_owner_snapshot_and_bytes_cannot_merge() {
        for field in 0..5 {
            let mut first = inert_read("$native/value");
            let mut other = inert_read("/workflow/tasks/0/next/0/when");
            match field {
                0 => other.transaction.push('x'),
                1 => other.renderer += 1,
                2 => other.snapshot += 1,
                3 => other.source_digest.push('x'),
                _ => other.value_digest.push('x'),
            }
            assert!(first.merge(&other).is_err());
        }
    }
}
