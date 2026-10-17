//! Source-first initial wait registration. Only this factory reserves wait IDs;
//! the actual wait/workflow FK is acquired later by the target binder.
use super::*;
use crate::models::{
    Execution, ExecutionStatus, WorkflowExecution, WorkflowTaskWait, WorkflowTaskWaitTarget,
};
use crate::repositories::FindById;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RegistrationRecipe {
    renderer: Id,
    workflow: Id,
    task_path: String,
    task_name: String,
    target_kind: String,
    target_id: Id,
    task_authority: SelectedRuntimeAuthority,
    selector: Option<Id>,
    source_admission: Id,
    source_version: Value,
}

// Policy selection issues a fresh non-nil attempt for each real selection.
// Idempotent registration compares the frozen capabilities, not those distinct
// attempts. The reservation/snapshot identity checks below still compare the
// factory's own exact authority, including its actual attempt.
fn same_selection(left: &SelectedRuntimeAuthority, right: &SelectedRuntimeAuthority) -> bool {
    !left.attempt.is_nil()
        && !right.attempt.is_nil()
        && left.policy_id == right.policy_id
        && left.binding == right.binding
        && left.owner == right.owner
        && left.target_release_id == right.target_release_id
        && left.target_release_digest == right.target_release_digest
        && left.selected_refs == right.selected_refs
        && left.grants == right.grants
        && left.attribution == right.attribution
}

pub struct PreparedWaitRegistration {
    transaction: String,
    reserved: Id,
    parent: Execution,
    workflow: WorkflowExecution,
    target: WorkflowTaskWaitTarget,
    recipe: RegistrationRecipe,
    fence: ApprovalEffectFence,
    observation: Option<PreparedWaitInput>,
}

pub struct RegisteredInitialWait {
    transaction: String,
    wait: WorkflowTaskWait,
    created: bool,
    released: Option<ReleasedWaitInput>,
    recipe_digest: String,
    fence: ApprovalEffectFence,
}

impl PreparedWaitRegistration {
    pub(crate) fn prepared_input(&self) -> Option<&PreparedWaitInput> {
        self.observation.as_ref()
    }
    pub fn state(&self) -> Option<&str> {
        self.observation
            .as_ref()
            .map(|input| input.source.state.as_str())
    }
    pub fn workflow(&self) -> Id {
        self.workflow.id
    }
    pub fn task_name(&self) -> &str {
        &self.recipe.task_name
    }
    pub fn bind_input(
        &self,
        session: &mut super::super::inherited_input::RenderSession,
    ) -> Result<()> {
        InheritedInputRepository::bind_prepared_wait_input(
            session,
            self.observation.as_ref().ok_or_else(denied)?,
        )
    }
    pub fn bind_outcome(
        &self,
        session: &mut super::super::inherited_input::RenderSession,
    ) -> Result<()> {
        InheritedInputRepository::bind_prepared_wait_outcome(
            session,
            self.observation.as_ref().ok_or_else(denied)?,
        )
    }
    pub async fn ensure_fresh_on(&self, conn: &mut PgConnection) -> Result<()> {
        require_transaction(conn, &self.transaction).await?;
        self.fence.ensure_fresh_on(conn).await
    }
    pub fn ensure_fresh(&self) -> Result<()> {
        self.fence.ensure_fresh()
    }

    async fn require_reservation(&self, conn: &mut PgConnection) -> Result<()> {
        let actual:Option<(Id,Option<Id>,Value,String,String)> = sqlx::query_as(
            "SELECT renderer,snapshot,recipe,recipe_digest,prepared_transaction FROM runtime_native_wait_reservation WHERE wait=$1"
        ).bind(self.reserved).fetch_optional(&mut *conn).await?;
        let (renderer, snapshot, recipe, digest, tx) = actual.ok_or_else(denied)?;
        bounded(&recipe)?;
        if renderer != self.recipe.renderer
            || snapshot != self.observation.as_ref().map(|input| input.snapshot)
            || tx != self.transaction
            || hash(&recipe)? != digest
            || recipe != serde_json::to_value(&self.recipe)?
        {
            return Err(denied());
        }
        Ok(())
    }

    // Only the enclosing native source factory can use this entry to its common
    // observation builder. No caller can supply a row, observation or reserved ID.
    pub(super) async fn identity(
        &self,
        conn: &mut PgConnection,
        renderer: Id,
        path: &str,
        admission: &ExecutionAdmission,
        reserved: Id,
    ) -> Result<(Id, String, WorkflowTaskWaitTarget)> {
        self.ensure_fresh_on(conn).await?;
        if self.parent.id != renderer
            || self.recipe.task_path != path
            || self.reserved != reserved
            || serde_json::to_value(admission.authority())?
                != serde_json::to_value(&self.recipe.task_authority)?
        {
            return Err(denied());
        }
        Ok((self.workflow.id, self.recipe.task_name.clone(), self.target))
    }

    /// Target-only. A losing concurrent factory returns the actual original
    /// registration without finalizing its speculative input or child buffers.
    /// Its owning caller must roll back that losing transaction.
    pub async fn bind(self, conn: &mut PgConnection) -> Result<RegisteredInitialWait> {
        use super::super::{
            execution::ExecutionRepository as Executions,
            workflow::WorkflowExecutionRepository as Workflows,
            workflow_task_wait::WorkflowTaskWaitRepository as Waits,
        };
        self.ensure_fresh_on(conn).await?;
        self.require_reservation(conn).await?;
        let workflow = Workflows::find_by_id_for_update(&mut *conn, self.workflow.id)
            .await?
            .ok_or_else(denied)?;
        let parent = Executions::find_by_id(&mut *conn, self.parent.id)
            .await?
            .ok_or_else(denied)?;
        if workflow.execution != self.parent.id
            || workflow.status != ExecutionStatus::Running
            || workflow.paused
            || parent.status != ExecutionStatus::Running
        {
            return Err(denied());
        }
        self.fence.ensure_fresh_on(conn).await?;
        self.require_reservation(conn).await?;
        if let Some(wait) =
            Waits::find_by_workflow_task_for_update(conn, workflow.id, &self.recipe.task_name)
                .await?
        {
            if wait.target()? != self.target {
                return Err(denied());
            }
            let association: Option<(Id, String, Value, String)> = sqlx::query_as(
                "SELECT renderer,task_path,recipe,recipe_digest FROM runtime_native_wait_registration WHERE wait=$1"
            ).bind(wait.id).fetch_optional(&mut *conn).await?;
            let (renderer, path, recipe, expected) = association.ok_or_else(denied)?;
            bounded(&recipe)?;
            let original: RegistrationRecipe = serde_json::from_value(recipe.clone())?;
            // Source versions may advance after an original in-progress
            // registration. Identity, selector and frozen task authority may not.
            if hash(&recipe)? != expected
                || renderer != self.recipe.renderer
                || path != self.recipe.task_path
                || original.renderer != renderer
                || original.task_path != path
                || original.workflow != self.recipe.workflow
                || original.task_name != self.recipe.task_name
                || original.target_kind != self.recipe.target_kind
                || original.target_id != self.recipe.target_id
                || original.source_admission != self.recipe.source_admission
                || !same_selection(&original.task_authority, &self.recipe.task_authority)
            {
                return Err(denied());
            }
            if wait.state != crate::models::WorkflowTaskWaitState::Waiting {
                let release:Option<(Value,String,String)> = sqlx::query_as(
                    "SELECT s.body,s.body_digest,r.release_transaction FROM runtime_native_wait_release r JOIN runtime_native_input_snapshot s ON s.id=r.snapshot WHERE r.wait=$1"
                ).bind(wait.id).fetch_optional(&mut *conn).await?;
                let (body, digest, release_tx) = release.ok_or_else(denied)?;
                bounded(&body)?;
                if hash(&body)? != digest {
                    return Err(denied());
                }
                let body: Body = serde_json::from_value(body)?;
                let Source::Wait(source) = &body.source else {
                    return Err(denied());
                };
                if body.renderer != original.renderer
                    || body.execution != self.parent.id
                    || body.task_path != original.task_path
                    || source.wait != wait.id
                    || source.workflow != original.workflow
                    || source.task_name != original.task_name
                    || source.target_id != original.target_id
                    || source.target_kind != original.target_kind
                    || source.prepared_transaction != release_tx
                    || serde_json::to_value(&wait.state)?.as_str() != Some(source.state.as_str())
                    || source.source_admission != original.source_admission
                    || wait.result.as_ref() != Some(&source.raw_projection)
                    || !same_selection(&body.authority, &original.task_authority)
                {
                    return Err(denied());
                }
            }
            self.fence.ensure_fresh_on(conn).await?;
            return Ok(RegisteredInitialWait {
                transaction: self.transaction,
                wait,
                created: false,
                released: None,
                recipe_digest: expected,
                fence: self.fence,
            });
        }
        if serde_json::to_value(&workflow)? != serde_json::to_value(&self.workflow)?
            || serde_json::to_value(&parent)? != serde_json::to_value(&self.parent)?
        {
            return Err(denied());
        }
        let (inquiry, execution, queue) = match self.target {
            WorkflowTaskWaitTarget::Inquiry(id) => (Some(id), None, None),
            WorkflowTaskWaitTarget::Execution(id) => (None, Some(id), None),
            WorkflowTaskWaitTarget::WorkQueueItem(id) => (None, None, Some(id)),
        };
        sqlx::query("INSERT INTO workflow_task_wait(id,workflow_execution,task_name,kind,inquiry,target_execution,work_queue_item,active_work_queue_item,active_inquiry) VALUES($1,$2,$3,$4,$5,$6,$7,$7,$5)")
            .bind(self.reserved).bind(workflow.id).bind(&self.recipe.task_name).bind(self.target.kind())
            .bind(inquiry).bind(execution).bind(queue).execute(&mut *conn).await?;
        // Recheck after the last FK/unique target wait and before publication.
        self.fence.ensure_fresh_on(conn).await?;
        let recipe = serde_json::to_value(&self.recipe)?;
        bounded(&recipe)?;
        let recipe_digest = hash(&recipe)?;
        let (count,bytes):(i64,i64)=sqlx::query_as("SELECT count(*),COALESCE(sum(octet_length(recipe::TEXT)+octet_length(recipe_digest)+octet_length(task_path)+octet_length(registration_transaction)),0)::BIGINT FROM runtime_native_wait_registration WHERE renderer=$1")
            .bind(self.recipe.renderer).fetch_one(&mut *conn).await?;
        let charge:i64=sqlx::query_scalar("SELECT (octet_length($1::JSONB::TEXT)+octet_length($2::TEXT)+octet_length($3::TEXT)+octet_length(pg_current_xact_id()::TEXT))::BIGINT")
            .bind(&recipe).bind(&recipe_digest).bind(&self.recipe.task_path).fetch_one(&mut *conn).await?;
        if count >= 128
            || bytes
                .checked_add(charge)
                .is_none_or(|bytes| bytes > MAX_BYTES as i64)
        {
            return Err(denied());
        }
        sqlx::query("INSERT INTO runtime_native_wait_registration(wait,renderer,selector,task_path,recipe,recipe_digest,registration_transaction) VALUES($1,$2,$3,$4,$5,$6,pg_current_xact_id()::TEXT)")
            .bind(self.reserved).bind(self.recipe.renderer).bind(self.recipe.selector).bind(&self.recipe.task_path)
            .bind(recipe).bind(&recipe_digest).execute(&mut *conn).await?;
        let released = match self.observation {
            Some(input) => Some(NativeInputSnapshotRepository::release_wait(conn, input).await?),
            None => None,
        };
        sqlx::query("DELETE FROM runtime_native_wait_reservation WHERE wait=$1 AND prepared_transaction=pg_current_xact_id()::TEXT")
            .bind(self.reserved).execute(&mut *conn).await?;
        let wait = Waits::find_by_workflow_task(&mut *conn, workflow.id, &self.recipe.task_name)
            .await?
            .ok_or_else(denied)?;
        self.fence.ensure_fresh_on(conn).await?;
        Ok(RegisteredInitialWait {
            transaction: self.transaction,
            wait,
            created: true,
            released,
            recipe_digest,
            fence: self.fence,
        })
    }
}

impl RegisteredInitialWait {
    pub fn wait(&self) -> &WorkflowTaskWait {
        &self.wait
    }
    pub fn created(&self) -> bool {
        self.created
    }
    pub fn released(&self) -> Option<&ReleasedWaitInput> {
        self.released.as_ref()
    }
    pub async fn ensure_fresh_on(&self, conn: &mut PgConnection) -> Result<()> {
        require_transaction(conn, &self.transaction).await?;
        let actual: Option<(Value, String)> = sqlx::query_as(
            "SELECT recipe,recipe_digest FROM runtime_native_wait_registration WHERE wait=$1",
        )
        .bind(self.wait.id)
        .fetch_optional(&mut *conn)
        .await?;
        let (recipe, digest) = actual.ok_or_else(denied)?;
        bounded(&recipe)?;
        if digest != self.recipe_digest || hash(&recipe)? != digest {
            return Err(denied());
        }
        let wait =
            super::super::workflow_task_wait::WorkflowTaskWaitRepository::find_by_workflow_task(
                &mut *conn,
                self.wait.workflow_execution,
                &self.wait.task_name,
            )
            .await?
            .ok_or_else(denied)?;
        if serde_json::to_value(wait)? != serde_json::to_value(&self.wait)? {
            return Err(denied());
        }
        if let Some(released) = &self.released {
            released.ensure_fresh_on(conn).await?;
        }
        self.fence.ensure_fresh_on(conn).await
    }
    pub fn ensure_fresh(&self) -> Result<()> {
        if let Some(released) = &self.released {
            released.ensure_fresh()?;
        }
        self.fence.ensure_fresh()
    }
}

impl NativeInputSnapshotRepository {
    /// Recover the original registered selector, never evaluate its expression
    /// again merely because an in-progress target has subsequently completed.
    pub(crate) async fn initial_wait_selector(
        conn: &mut PgConnection,
        renderer: Id,
        task_path: &str,
        wait: Id,
        key: &str,
    ) -> Result<Option<Option<ReceiptContext>>> {
        let saved:Option<(Id,String,Value,String,Option<Id>)> = sqlx::query_as(
            "SELECT renderer,task_path,recipe,recipe_digest,selector FROM runtime_native_wait_registration WHERE wait=$1"
        ).bind(wait).fetch_optional(&mut *conn).await?;
        let Some((admission, path, body, digest, selector)) = saved else {
            return Ok(None);
        };
        bounded(&body)?;
        let recipe: RegistrationRecipe = serde_json::from_value(body.clone())?;
        let record = DeclaredInputRepository::require_source(conn, renderer).await?;
        let actual =
            super::super::workflow_task_wait::WorkflowTaskWaitRepository::find_by_workflow_task(
                &mut *conn,
                recipe.workflow,
                &recipe.task_name,
            )
            .await?
            .ok_or_else(denied)?;
        let belongs: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM workflow_execution WHERE id=$1 AND execution=$2)",
        )
        .bind(recipe.workflow)
        .bind(renderer)
        .fetch_one(&mut *conn)
        .await?;
        if hash(&body)? != digest
            || !belongs
            || admission != record.id
            || recipe.renderer != record.id
            || path != task_path
            || recipe.task_path != path
            || recipe.selector != selector
            || actual.id != wait
            || actual.target()?.id() != recipe.target_id
            || serde_json::to_value(actual.target()?.kind())?.as_str()
                != Some(recipe.target_kind.as_str())
        {
            return Err(denied());
        }
        let Some(receipt) = selector else {
            return Ok(Some(None));
        };
        let site = InputSite {
            execution: renderer,
            kind: super::super::inherited_input::InputEntityKind::WorkflowVariables,
            purpose: InputSitePurpose::Control,
            path: format!("{task_path}/wait_for/{}", recipe.target_kind),
        };
        let origin = SecretOrigin::Entity {
            entity_type: "declared_input:workflow_variables:control".into(),
            entity_id: receipt,
            path: site.path.clone(),
        };
        let control = InheritedInputRepository::receipt_context(conn, &site, &origin, key).await?;
        control
            .require_declared_control(conn, record.id, &site.path)
            .await?;
        Ok(Some(Some(control)))
    }

    /// Prepare the real registration before inserting any wait/workflow FK.
    /// The reserved positive ID is internal and cannot be chosen by the caller.
    pub async fn prepare_initial_wait(
        conn: &mut PgConnection,
        renderer: Id,
        workflow: Id,
        task_path: &str,
        admission: &ExecutionAdmission,
        target: WorkflowTaskWaitTarget,
        target_control: Option<&ReceiptContext>,
        key: &str,
    ) -> Result<PreparedWaitRegistration> {
        use super::super::{
            execution::ExecutionRepository as Executions,
            workflow::WorkflowExecutionRepository as Workflows,
        };
        let record = DeclaredInputRepository::require_source(conn, renderer).await?;
        require_task(conn, &record, task_path, admission.authority()).await?;
        let parent = Executions::find_by_id(&mut *conn, renderer)
            .await?
            .ok_or_else(denied)?;
        let workflow = Workflows::find_by_id(&mut *conn, workflow)
            .await?
            .ok_or_else(denied)?;
        if parent.status != ExecutionStatus::Running
            || workflow.execution != renderer
            || workflow.status != ExecutionStatus::Running
            || workflow.paused
            || target.id() <= 0
        {
            return Err(denied());
        }
        let accepted_parent =
            super::super::accepted_input::AcceptedInputRepository::revalidate_execution(
                conn, renderer, key,
            )
            .await?;
        let task = task_definition(conn, &record, task_path).await?;
        let task_name = task
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(denied)?
            .to_owned();
        let authored: crate::workflow::TaskWaitFor =
            serde_json::from_value(task.get("wait_for").cloned().ok_or_else(denied)?)?;
        let (kind, selector) = authored.target();
        let actual_kind = match target {
            WorkflowTaskWaitTarget::Inquiry(_) => "inquiry",
            WorkflowTaskWaitTarget::Execution(_) => "execution",
            WorkflowTaskWaitTarget::WorkQueueItem(_) => "work_queue_item",
        };
        if kind != actual_kind {
            return Err(denied());
        }
        let mut fence = RuntimeAuthorityRepository::retained_admission_fence(conn, record.id)
            .await?
            .intersect(admission.effect_fence())
            .intersect(&accepted_parent);
        let selector_receipt = if selector.as_i64() == Some(target.id()) {
            if target_control.is_some() {
                return Err(denied());
            }
            None
        } else {
            if !selector.as_str().is_some_and(|value| value.contains("{{")) {
                return Err(denied());
            }
            let control = target_control.ok_or_else(denied)?;
            control
                .require_declared_control(conn, record.id, &format!("{task_path}/wait_for/{kind}"))
                .await?;
            if control.site().execution != renderer
                || control.accepted_value() != &json!(target.id())
            {
                return Err(denied());
            }
            control.ensure_fresh_on(conn).await?;
            fence = fence.intersect(control.accepted_fence());
            Some(
                InheritedInputRepository::receipt_source_site(conn, &control.origin())
                    .await?
                    .0,
            )
        };
        let runtime_target = match target {
            WorkflowTaskWaitTarget::Inquiry(id) => RuntimeTarget::Inquiry(id),
            WorkflowTaskWaitTarget::Execution(id) => RuntimeTarget::Execution(id),
            WorkflowTaskWaitTarget::WorkQueueItem(id) => RuntimeTarget::QueueItem(id),
        };
        let resolved = RuntimeTargetRepository::resolve(
            conn,
            runtime_target,
            admission.authority().owner.sponsor_identity_id,
        )
        .await?
        .ok_or_else(denied)?;
        require_selected_read(
            conn,
            admission.authority(),
            &resolved,
            crate::rbac::Action::Read,
        )
        .await?;
        // Authorize first, then hold the actual source stable through binding.
        let (source_execution, terminal, source_version) = match target {
            WorkflowTaskWaitTarget::Execution(id) => {
                sqlx::query("SELECT id FROM execution WHERE id=$1 FOR SHARE")
                    .bind(id)
                    .fetch_optional(&mut *conn)
                    .await?
                    .ok_or_else(denied)?;
                let source = Executions::find_by_id(&mut *conn, id)
                    .await?
                    .ok_or_else(denied)?;
                if !Executions::is_in_execution_tree(&mut *conn, renderer, id, false).await? {
                    return Err(denied());
                }
                (
                    id,
                    matches!(
                        source.status,
                        ExecutionStatus::Completed
                            | ExecutionStatus::Failed
                            | ExecutionStatus::Timeout
                            | ExecutionStatus::Cancelled
                            | ExecutionStatus::Abandoned
                    ),
                    json!({"updated":source.updated,"status":source.status}),
                )
            }
            WorkflowTaskWaitTarget::Inquiry(id) => {
                sqlx::query("SELECT id FROM inquiry WHERE id=$1 FOR SHARE")
                    .bind(id)
                    .fetch_optional(&mut *conn)
                    .await?
                    .ok_or_else(denied)?;
                let source = super::super::inquiry::InquiryRepository::find_by_id(&mut *conn, id)
                    .await?
                    .ok_or_else(denied)?;
                if source.workflow_execution != Some(workflow.id) {
                    return Err(denied());
                }
                (
                    source.created_by_execution,
                    matches!(
                        source.status,
                        crate::models::InquiryStatus::Responded
                            | crate::models::InquiryStatus::Timeout
                            | crate::models::InquiryStatus::Cancelled
                    ),
                    json!({"updated":source.updated,"status":source.status}),
                )
            }
            WorkflowTaskWaitTarget::WorkQueueItem(id) => {
                sqlx::query("SELECT id FROM work_queue_item WHERE id=$1 FOR SHARE")
                    .bind(id)
                    .fetch_optional(&mut *conn)
                    .await?
                    .ok_or_else(denied)?;
                let source =
                    super::super::work_queue::WorkQueueItemRepository::find_by_id(&mut *conn, id)
                        .await?
                        .ok_or_else(denied)?;
                let requester = source.requested_by_execution.ok_or_else(denied)?;
                if !Executions::is_in_execution_tree(&mut *conn, renderer, requester, true).await? {
                    return Err(denied());
                }
                (
                    requester,
                    matches!(
                        source.status,
                        crate::models::WorkQueueItemStatus::Completed
                            | crate::models::WorkQueueItemStatus::Failed
                            | crate::models::WorkQueueItemStatus::Skipped
                            | crate::models::WorkQueueItemStatus::Cancelled
                    ),
                    json!({"updated":source.updated,"status":source.status}),
                )
            }
        };
        let source_record = RuntimeAuthorityRepository::execution_admission(conn, source_execution)
            .await?
            .ok_or_else(denied)?;
        fence = fence.intersect(
            &RuntimeAuthorityRepository::retained_admission_fence(conn, source_record.id).await?,
        );
        let reserved: Id =
            sqlx::query_scalar("SELECT nextval(pg_get_serial_sequence('workflow_task_wait','id'))")
                .fetch_one(&mut *conn)
                .await?;
        if reserved <= 0 {
            return Err(denied());
        }
        let workflow_id = workflow.id;
        let mut prepared = PreparedWaitRegistration {
            transaction: transaction(conn).await?,
            reserved,
            parent,
            workflow,
            target,
            recipe: RegistrationRecipe {
                renderer: record.id,
                workflow: workflow_id,
                task_path: task_path.into(),
                task_name,
                target_kind: kind.into(),
                target_id: target.id(),
                task_authority: admission.authority().clone(),
                selector: selector_receipt,
                source_admission: source_record.id,
                source_version,
            },
            fence,
            observation: None,
        };
        if terminal {
            prepared.observation = Some(
                Self::prepare_wait_inner(
                    conn,
                    renderer,
                    task_path,
                    admission,
                    reserved,
                    target_control,
                    key,
                    Some(&prepared),
                )
                .await?,
            );
        }
        if let Some(observation) = &prepared.observation {
            // The shared observer can acquire independent ciphertext/provenance
            // fences beyond registration eligibility. Retain the complete
            // observation deadline in the owning registration too, including
            // source/target waits before release_wait consumes the observation.
            prepared.fence = prepared.fence.intersect(&observation.fence);
        }
        let recipe = serde_json::to_value(&prepared.recipe)?;
        bounded(&recipe)?;
        sqlx::query("INSERT INTO runtime_native_input_snapshot_budget(renderer_admission) VALUES($1) ON CONFLICT DO NOTHING")
            .bind(prepared.recipe.renderer).execute(&mut *conn).await?;
        sqlx::query("SELECT renderer_admission FROM runtime_native_input_snapshot_budget WHERE renderer_admission=$1 FOR UPDATE")
            .bind(prepared.recipe.renderer).fetch_one(&mut *conn).await?;
        let (count,bytes):(i64,i64)=sqlx::query_as("SELECT count(*),COALESCE(sum(octet_length(recipe::TEXT)+octet_length(recipe_digest)+octet_length(prepared_transaction)),0)::BIGINT FROM runtime_native_wait_reservation WHERE renderer=$1")
            .bind(prepared.recipe.renderer).fetch_one(&mut *conn).await?;
        let digest = hash(&recipe)?;
        let charge:i64=sqlx::query_scalar("SELECT (octet_length($1::JSONB::TEXT)+octet_length($2::TEXT)+octet_length(pg_current_xact_id()::TEXT))::BIGINT")
            .bind(&recipe).bind(&digest).fetch_one(&mut *conn).await?;
        if count >= 128
            || bytes
                .checked_add(charge)
                .is_none_or(|bytes| bytes > MAX_BYTES as i64)
        {
            return Err(denied());
        }
        sqlx::query("INSERT INTO runtime_native_wait_reservation(wait,renderer,snapshot,recipe,recipe_digest,prepared_transaction) VALUES($1,$2,$3,$4,$5,pg_current_xact_id()::TEXT)")
            .bind(prepared.reserved).bind(prepared.recipe.renderer).bind(prepared.observation.as_ref().map(|input|input.snapshot))
            .bind(&recipe).bind(digest).execute(&mut *conn).await?;
        prepared.require_reservation(conn).await?;
        prepared.ensure_fresh_on(conn).await?;
        Ok(prepared)
    }
}
