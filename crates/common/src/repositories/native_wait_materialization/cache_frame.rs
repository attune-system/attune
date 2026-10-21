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
    released: ReleaseOwner,
    published: OnceLock<Published>,
    native_sources: Vec<super::super::native_input_snapshot::NativeSourceEvidence>,
}
struct Published {
    workflow: WorkflowExecution,
    variables: Value,
    fields: Vec<crate::secret_values::PreparedSecretValue>,
    wait: Value,
    release_transaction: String,
}
enum ReleaseOwner {
    Registered(Arc<OnceLock<ReleasedWaitInput>>),
    Initial(Arc<OnceLock<super::super::native_input_snapshot::RegisteredInitialWait>>),
}
impl ReleaseOwner {
    fn held_deadline(&self) -> Option<chrono::DateTime<chrono::Utc>> {
        match self {
            Self::Registered(owner) => owner
                .get()
                .and_then(|released| released.binding().6.approval_deadline()),
            Self::Initial(owner) => owner.get().and_then(|bound| bound.held_deadline()),
        }
    }
    fn get(&self) -> Option<&ReleasedWaitInput> {
        match self {
            Self::Registered(owner) => owner.get(),
            Self::Initial(owner) => owner.get()?.released(),
        }
    }
}

impl Frame {
    pub(super) async fn require_sealed_state(
        &self,
        conn: &mut PgConnection,
    ) -> Result<(WorkflowExecution, Vec<StoredSecretValue>)> {
        self.require_published(conn).await?;
        let seal = self.published.get().ok_or_else(denied)?;
        let workflow = Workflows::find_by_id(&mut *conn, self.workflow.id)
            .await?
            .ok_or_else(denied)?;
        if serde_json::to_value(&workflow)? != serde_json::to_value(&seal.workflow)? {
            return Err(denied());
        }
        let fields =
            Secrets::find_stored_by_entity(&mut *conn, ENTITY_WORKFLOW_VARIABLES, self.parent.id)
                .await?;
        Ok((workflow, fields))
    }
    pub(super) fn source_identity(&self) -> (Id, Id) {
        (self.wait, self.snapshot)
    }
    pub(super) fn released_observation(&self) -> Result<&ReleasedWaitInput> {
        self.released.get().ok_or_else(denied)
    }
    pub(super) async fn new(
        conn: &mut PgConnection,
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
            native_sources: Self::hold_native_sources(conn, context, &program, key).await?,
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
            released: ReleaseOwner::Registered(released),
            published: OnceLock::new(),
        })
    }

    pub(super) async fn prospective(
        conn: &mut PgConnection,
        context: &Context,
        registration: &super::super::native_input_snapshot::PreparedWaitRegistration,
        program: Arc<super::staged::Program>,
        bound: Arc<OnceLock<super::super::native_input_snapshot::RegisteredInitialWait>>,
        key: &str,
    ) -> Result<Self> {
        let binding = registration
            .prepared_input()
            .ok_or_else(denied)?
            .outcome_binding()?;
        if binding.task_path != context.path
            || registration.workflow() != context.workflow.id
            || context.task.get("name").and_then(Value::as_str) != Some(registration.task_name())
            || registration.reserved_wait() <= 0
        {
            return Err(denied());
        }
        Ok(Self {
            native_sources: Self::hold_native_sources(conn, context, &program, key).await?,
            parent: context.parent.clone(),
            workflow: context.workflow.clone(),
            key_hash: crate::crypto::hash_encryption_key(key),
            transaction: binding.transaction.into(),
            wait: registration.reserved_wait(),
            task: registration.task_name().into(),
            snapshot: binding.snapshot,
            sources: context.sources.clone(),
            program,
            fence: registration.held_effect_fence().intersect(binding.fence),
            released: ReleaseOwner::Initial(bound),
            published: OnceLock::new(),
        })
    }

    async fn hold_native_sources(
        conn: &mut PgConnection,
        context: &Context,
        program: &super::staged::Program,
        key: &str,
    ) -> Result<Vec<super::super::native_input_snapshot::NativeSourceEvidence>> {
        let mut metadata = BTreeMap::new();
        for source in context
            .sources
            .fields
            .iter()
            .filter_map(|field| field.source.as_ref())
            .flat_map(|source| source.held_native_metadata())
            .chain(program.held_native_metadata())
        {
            if source.source_kind() == "cache"
                && metadata
                    .insert(source.snapshot(), source)
                    .is_some_and(|previous| previous != source)
            {
                return Err(denied());
            }
        }
        let mut held = vec![];
        for source in metadata.into_values() {
            let evidence = Snapshots::source_evidence(conn, source, key).await?;
            evidence
                .cache_read_policy()
                .ok_or_else(denied)?
                .ensure_fresh()?;
            held.push(evidence);
        }
        Ok(held)
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
                workflow: Workflows::find_by_id(&mut *conn, self.workflow.id)
                    .await?
                    .ok_or_else(denied)?,
                variables: variables.clone(),
                fields: fields.to_vec(),
                wait: business_wait(&wait)?,
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

impl Frame {
    pub(super) fn held_deadline(&self) -> Result<Option<chrono::DateTime<chrono::Utc>>> {
        let mut deadlines = vec![
            Ok(self.fence.approval_deadline()),
            self.sources.held_wait_deadline(),
            Ok(self.program.held_deadline()),
            Ok(self.released.held_deadline()),
        ];
        for source in &self.native_sources {
            deadlines.push(Ok(source
                .cache_read_policy()
                .ok_or_else(denied)?
                .fresh_until()));
        }
        earliest_held(deadlines)
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
        self.sources.ensure_wait_sources_on(conn).await?;
        for source in &self.native_sources {
            source
                .cache_read_policy()
                .ok_or_else(denied)?
                .ensure_fresh()?;
        }
        self.program.ensure_fresh_on(conn).await?;
        if let Some(released) = self.released.get() {
            released.ensure_fresh_on(conn).await?;
        }
        Ok(())
    }
    fn ensure_fresh(&self) -> Result<()> {
        self.fence.ensure_fresh()?;
        self.sources.ensure_wait_sources()?;
        for source in &self.native_sources {
            source
                .cache_read_policy()
                .ok_or_else(denied)?
                .ensure_fresh()?;
        }
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
        if business_wait(&wait)? != expected.wait
            || workflow.variables != expected.variables
            || fields != expected_fields
        {
            return Err(denied());
        }
        Ok(())
    }
}

// A delivery acknowledgement is protocol metadata, not a new observed outcome.
// The row timestamp also changes when that marker is acknowledged. All business
// fields, including source/target identity, state and result, stay exact.
fn business_wait(wait: &WorkflowTaskWait) -> Result<Value> {
    Ok(without_delivery_marker(serde_json::to_value(wait)?))
}

fn without_delivery_marker(mut value: Value) -> Value {
    if let Some(row) = value.as_object_mut() {
        row.remove("updated");
    }
    if let Some(result) = value.get_mut("result").and_then(Value::as_object_mut) {
        result.remove("_delivery_complete");
    }
    value
}

fn earliest_held(
    deadlines: impl IntoIterator<Item = Result<Option<chrono::DateTime<chrono::Utc>>>>,
) -> Result<Option<chrono::DateTime<chrono::Utc>>> {
    deadlines.into_iter().try_fold(None, |earliest, next| {
        Ok(earliest.into_iter().chain(next?).min())
    })
}

#[cfg(test)]
mod deadline_tests {
    use super::*;
    #[test]
    fn acknowledgement_does_not_change_business_observation() {
        let original = serde_json::json!({"id":1,"state":"failed","result":{"status":"failed","error":"original"}});
        let mut acknowledged = original.clone();
        acknowledged["result"]["_delivery_complete"] = serde_json::json!(true);
        assert_eq!(without_delivery_marker(acknowledged.clone()), original);
        acknowledged["result"]["error"] = serde_json::json!("changed");
        assert_ne!(without_delivery_marker(acknowledged), original);
    }
    #[test]
    fn independent_controls_and_sources_bound_no_child_frames() {
        let early = chrono::DateTime::from_timestamp(40, 0).unwrap();
        let late = chrono::DateTime::from_timestamp(90, 0).unwrap();
        assert_eq!(
            earliest_held([Ok(None), Ok(Some(late)), Ok(Some(early)), Ok(None)]).unwrap(),
            Some(early)
        );
        assert_eq!(earliest_held([Ok(None), Ok(None)]).unwrap(), None);
    }
    #[test]
    fn unsupported_source_deadline_is_not_known_unbounded() {
        assert!(earliest_held([Ok(None), Err(denied())]).is_err());
    }
}
