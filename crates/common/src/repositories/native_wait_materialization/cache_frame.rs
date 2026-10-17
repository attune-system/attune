//! Immutable source frame. Only the owning wait writer may seal its publication.
use super::super::{
    inherited_input::RenderSession, native_cache_materialization::NativeCacheRenderFrame,
};
use super::*;
use std::sync::Arc;

pub(super) struct Frame {
    parent: Execution,
    workflow: WorkflowExecution,
    key_hash: String,
    transaction: String,
    wait: Id,
    task: String,
    snapshot: Id,
    sources: Arc<Sources>,
    program: Arc<super::staged::Program>,
    // Independent original observation fence, not a fence inferred from b.
    fence: ApprovalEffectFence,
    released: Arc<OnceLock<ReleasedWaitInput>>,
    published: OnceLock<Published>,
}
struct Published {
    variables: Value,
    fields: Vec<crate::secret_values::PreparedSecretValue>,
    wait: Value,
    release_transaction: String,
}

impl Frame {
    pub(super) fn new(
        context: &Context,
        wait: &WorkflowTaskWait,
        observation: &Observation,
        program: Arc<super::staged::Program>,
        released: Arc<OnceLock<ReleasedWaitInput>>,
        key: &str,
    ) -> Result<Self> {
        let binding = match observation {
            Observation::Pending(wait) => wait.outcome_binding()?,
            Observation::Released(wait) => wait.outcome_binding()?,
            Observation::Prospective(_) => return Err(denied()),
        };
        if binding.task_path != context.path || wait.workflow_execution != context.workflow.id {
            return Err(denied());
        }
        Ok(Self {
            parent: context.parent.clone(),
            workflow: context.workflow.clone(),
            key_hash: crate::crypto::hash_encryption_key(key),
            transaction: binding.transaction.into(),
            wait: wait.id,
            task: wait.task_name.clone(),
            snapshot: binding.snapshot,
            sources: context.sources.clone(),
            program,
            fence: binding.fence.clone(),
            released,
            published: OnceLock::new(),
        })
    }

    pub(super) async fn seal(
        &self,
        conn: &mut PgConnection,
        variables: &Value,
        fields: &[crate::secret_values::PreparedSecretValue],
    ) -> Result<()> {
        self.released
            .get()
            .ok_or_else(denied)?
            .ensure_fresh_on(conn)
            .await?;
        let wait = Waits::find_by_workflow_task(&mut *conn, self.workflow.id, &self.task)
            .await?
            .ok_or_else(denied)?;
        if wait.id != self.wait || wait.state == WorkflowTaskWaitState::Waiting {
            return Err(denied());
        }
        let release_transaction = self.release(conn).await?;
        self.published
            .set(Published {
                variables: variables.clone(),
                fields: fields.to_vec(),
                wait: serde_json::to_value(wait)?,
                release_transaction,
            })
            .map_err(|_| denied())?;
        self.require_published(conn).await
    }

    async fn release(&self, conn: &mut PgConnection) -> Result<String> {
        let (snapshot, transaction): (Id, String) = sqlx::query_as(
            "SELECT snapshot,release_transaction FROM runtime_native_wait_release WHERE wait=$1",
        )
        .bind(self.wait)
        .fetch_optional(&mut *conn)
        .await?
        .ok_or_else(denied)?;
        if snapshot != self.snapshot
            || self
                .released
                .get()
                .ok_or_else(denied)?
                .outcome_binding()?
                .snapshot
                != snapshot
        {
            return Err(denied());
        }
        Ok(transaction)
    }
}

#[async_trait::async_trait]
impl NativeCacheRenderFrame for Frame {
    fn execution(&self) -> Id {
        self.parent.id
    }
    async fn prepared_sources(
        &self,
        conn: &mut PgConnection,
        parent: &Execution,
        workflow: &WorkflowExecution,
        key: &str,
    ) -> Result<Option<Arc<Sources>>> {
        self.ensure_fresh_on(conn).await?;
        if serde_json::to_value(parent)? != serde_json::to_value(&self.parent)?
            || serde_json::to_value(workflow)? != serde_json::to_value(&self.workflow)?
            || crate::crypto::hash_encryption_key(key) != self.key_hash
        {
            return Err(denied());
        }
        Ok(Some(self.sources.clone()))
    }
    async fn bind(
        &self,
        conn: &mut PgConnection,
        session: &mut RenderSession,
        binding: &DeclaredInputBinding,
        _base: &Sources,
        key: &str,
    ) -> Result<()> {
        self.ensure_fresh_on(conn).await?;
        if crate::crypto::hash_encryption_key(key) != self.key_hash {
            return Err(denied());
        }
        // No owning wait input/outcome binding at a foreign cache declaration.
        // The only wait VALUE path is through genuine issued publications.
        self.program
            .bind(conn, &self.sources, session, binding, key)
            .await
    }
    fn attach_literal(&self, rendered: &mut RenderedReceiptContext, _base: &Sources) -> Result<()> {
        self.ensure_fresh()?;
        self.program.attach_literal(&self.sources, rendered)
    }
    async fn ensure_fresh_on(&self, conn: &mut PgConnection) -> Result<()> {
        let transaction: String = sqlx::query_scalar("SELECT pg_current_xact_id()::TEXT")
            .fetch_one(&mut *conn)
            .await?;
        if transaction != self.transaction {
            return Err(denied());
        }
        self.fence.ensure_fresh_on(conn).await?;
        self.program.ensure_fresh_on(conn).await?;
        if let Some(released) = self.released.get() {
            released.ensure_fresh_on(conn).await?;
        }
        Ok(())
    }
    fn ensure_fresh(&self) -> Result<()> {
        self.fence.ensure_fresh()?;
        self.program.ensure_fresh()?;
        if let Some(released) = self.released.get() {
            released.ensure_fresh()?;
        }
        Ok(())
    }
    async fn require_published(&self, conn: &mut PgConnection) -> Result<()> {
        let expected = self.published.get().ok_or_else(denied)?;
        self.ensure_fresh_on(conn).await?;
        if self.release(conn).await? != expected.release_transaction {
            return Err(denied());
        }
        let wait = Waits::find_by_workflow_task(&mut *conn, self.workflow.id, &self.task)
            .await?
            .ok_or_else(denied)?;
        let workflow = Workflows::find_by_id(&mut *conn, self.workflow.id)
            .await?
            .ok_or_else(denied)?;
        let mut fields =
            Secrets::find_stored_by_entity(&mut *conn, ENTITY_WORKFLOW_VARIABLES, self.parent.id)
                .await?
                .into_iter()
                .map(|field| {
                    Ok(crate::secret_values::PreparedSecretValue {
                        json_path: field.json_path,
                        encrypted_value: field.encrypted_value,
                        encryption_key_hash: field.encryption_key_hash.ok_or_else(denied)?,
                        source_kind: field.source_kind,
                        source_ref: field.source_ref,
                    })
                })
                .collect::<Result<Vec<_>>>()?;
        let mut expected_fields = expected.fields.clone();
        fields.sort_by(|a, b| a.json_path.cmp(&b.json_path));
        expected_fields.sort_by(|a, b| a.json_path.cmp(&b.json_path));
        if serde_json::to_value(wait)? != expected.wait
            || workflow.variables != expected.variables
            || fields != expected_fields
        {
            return Err(denied());
        }
        Ok(())
    }
}
