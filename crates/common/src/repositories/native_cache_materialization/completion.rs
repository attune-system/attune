//! Finite, source-first cache completion evidence. No caller summary or batch
//! result can construct this capability. Initial EOF is bound only by B's writer.
use super::*;
use crate::{
    repositories::{
        inherited_input::PreparedWorkflowOutcome,
        native_input_origin::{NativeInputClosure, NativeInputOriginRepository as Native},
        native_input_snapshot::{NativeSnapshotStorageEvidence, NativeSourceEvidence},
        runtime_authority::ApprovalEffectFence,
        schema_contract::ActionSchemaBinding,
    },
    secret_values::{StoredSecretValue, ENTITY_EXECUTION_RESULT},
};
use std::sync::OnceLock;

const MAX_ATTEMPTS: usize = 255; // root plus attempts fit the native 256-source closure
const MAX_BYTES: usize = 1024 * 1024;

impl NativeCacheMaterializationRepository {
    /// IDs select persisted state, not caller evidence. Source phase only, with
    /// PinMutation held by the outer transaction before workflow/iteration locks.
    /// None means no genuine empty EOF/terminal complete inventory in this scope.
    pub async fn prepare_completion_source(
        conn: &mut PgConnection,
        root: Id,
        workflow: Id,
        iteration: Id,
        key: &str,
    ) -> Result<Option<SourceCacheCompletion>> {
        let parent = Executions::find_by_id(&mut *conn, root)
            .await?
            .ok_or_else(denied)?;
        let workflow = Workflows::find_by_id(&mut *conn, workflow)
            .await?
            .ok_or_else(denied)?;
        let row = Iterations::find_by_id(&mut *conn, iteration)
            .await?
            .ok_or_else(denied)?;
        if workflow.execution != root || row.workflow_execution != workflow.id {
            return Err(denied());
        }
        if parent.status != ExecutionStatus::Running
            || workflow.status != ExecutionStatus::Running
            || row.state != WorkflowCacheIterationState::Scanning
        {
            return Ok(None);
        }
        let snapshot = parent.executable_snapshot.as_ref().ok_or_else(denied)?;
        let definition = snapshot
            .executable
            .workflow_definition
            .as_ref()
            .ok_or_else(denied)?;
        if workflow.workflow_def != definition.id {
            return Err(denied());
        }
        let nodes = definition
            .definition
            .get("tasks")
            .and_then(Value::as_array)
            .ok_or_else(denied)?;
        let mut matching = nodes.iter().filter(|task| {
            task.get("name").and_then(Value::as_str) == Some(row.task_name.as_str())
        });
        let task = matching.next().ok_or_else(denied)?.clone();
        if matching.next().is_some() || task.get("iterate_cache").is_none_or(Value::is_null) {
            return Err(denied());
        }
        let action = task
            .get("action")
            .and_then(Value::as_str)
            .ok_or_else(denied)?;
        if contains_template(&task["action"])
            || task
                .get("permission_set_refs")
                .is_some_and(contains_template)
        {
            return Ok(None);
        }
        let path = Materialize::workflow_task_site_path(conn, root, &row.task_name, action).await?;
        let executable =
            Admissions::resolve_workflow_snapshot(conn, root, &row.task_name, action).await?;
        let refs = match task
            .get("permission_set_refs")
            .filter(|value| !value.is_null())
        {
            Some(value) => permission_refs(value.clone())?,
            None => executable
                .executable
                .action
                .default_execution_permission_set_refs
                .clone(),
        };
        // Recover original selector Controls from a genuine bound frontier, not
        // the newest equal-looking variable, current IAM or rerendered selector.
        let Some((controls, _, _)) =
            original_controls(conn, root, workflow.id, &path, &row, key).await?
        else {
            return Ok(None);
        };
        let closure = Native::capture_with_key(conn, root, &[], Some(key)).await?;
        let admission = Admissions::workflow_child_prepared(
            conn,
            root,
            &row.task_name,
            &executable,
            &refs,
            &closure,
        )
        .await?;
        let eof = Snapshots::prepare_cache_with_controls(
            conn,
            root,
            &path,
            &admission,
            row.id,
            MAX_BYTES,
            &controls.iter().collect::<Vec<_>>(),
            key,
        )
        .await?;
        if !eof.exhausted() || eof.count() != 0 {
            return Ok(None);
        }
        let Some(sources) = prepare_sources(conn, &parent, None, &workflow, key).await? else {
            return Ok(None);
        };
        let context = Context {
            revision: crate::value_schema::digest(&serde_json::to_value(&snapshot.executable)?)
                .map_err(|_| denied())?,
            bindings: BTreeMap::new(),
            parent,
            workflow,
            task,
            path,
            sources: Arc::new(sources),
        };
        SourceCacheCompletion::prepare(
            conn,
            &context,
            Some(&row),
            Arc::new(eof),
            Arc::new(controls),
            Arc::new(Vec::new()),
            None,
            key,
        )
        .await
    }
}

/// Original admitted attempt, including superseded attempts needed to prove rank.
/// Constructible only while authenticating the complete inventory below.
pub struct NativeCacheCompletionAttempt {
    execution: Execution,
    contract: ActionSchemaBinding,
    fields: Vec<StoredSecretValue>,
    config_fields: Vec<StoredSecretValue>,
    inputs: Vec<ReceiptContext>,
    membership: NativeSnapshotStorageEvidence,
    source: NativeSourceEvidence,
    outcome: Option<PreparedWorkflowOutcome>,
    latest: bool,
}
impl NativeCacheCompletionAttempt {
    pub fn execution(&self) -> &Execution {
        &self.execution
    }
    pub fn contract(&self) -> &ActionSchemaBinding {
        &self.contract
    }
    pub fn result_fields(&self) -> &[StoredSecretValue] {
        &self.fields
    }
    pub fn config_fields(&self) -> &[StoredSecretValue] {
        &self.config_fields
    }
    pub fn input_receipts(&self) -> &[ReceiptContext] {
        &self.inputs
    }
    pub fn cache_membership(&self) -> &NativeSnapshotStorageEvidence {
        &self.membership
    }
    pub fn native_source(&self) -> &NativeSourceEvidence {
        &self.source
    }
    pub fn outcome(&self) -> Option<&PreparedWorkflowOutcome> {
        self.outcome.as_ref()
    }
    pub fn is_latest(&self) -> bool {
        self.latest
    }
}

/// B-owned authenticated source, distinct from an aggregate VALUE/output recipe.
/// No Clone, Deserialize, caller DTO constructor or mutable evidence accessors.
pub struct SourceCacheCompletion {
    transaction: String,
    parent: Execution,
    workflow: WorkflowExecution,
    task_path: String,
    task_name: String,
    original: Option<WorkflowCacheIteration>,
    eof: Arc<PreparedCacheInput>,
    bound_iteration: OnceLock<WorkflowCacheIteration>,
    terminal_iteration: OnceLock<WorkflowCacheIteration>,
    contract: ActionSchemaBinding,
    variables: Vec<StoredSecretValue>,
    root_config: Vec<StoredSecretValue>,
    root_result: Vec<StoredSecretValue>,
    attempts: Vec<NativeCacheCompletionAttempt>,
    controls: Arc<Vec<ReceiptContext>>,
    issued: Arc<Vec<PreparedInputSource>>,
    original_controls: Vec<ReceiptContext>,
    original_source: Option<NativeSourceEvidence>,
    frame: Option<Arc<dyn NativeCacheRenderFrame>>,
    closure: NativeInputClosure,
    fences: Vec<ApprovalEffectFence>,
    failed: bool,
}

impl SourceCacheCompletion {
    pub fn execution(&self) -> &Execution {
        &self.parent
    }
    pub fn workflow(&self) -> &WorkflowExecution {
        &self.workflow
    }
    pub fn task_path(&self) -> &str {
        &self.task_path
    }
    pub fn task_name(&self) -> &str {
        &self.task_name
    }
    pub fn original_iteration(&self) -> Option<&WorkflowCacheIteration> {
        self.original.as_ref()
    }
    pub fn eof(&self) -> &PreparedCacheInput {
        &self.eof
    }
    pub fn bound_iteration(&self) -> Option<Id> {
        self.bound_iteration.get().map(|row| row.id)
    }
    pub fn bound_frontier(&self) -> Option<&WorkflowCacheIteration> {
        self.bound_iteration.get()
    }
    pub fn terminal_frontier(&self) -> Option<&WorkflowCacheIteration> {
        self.terminal_iteration.get()
    }
    pub fn contract(&self) -> &ActionSchemaBinding {
        &self.contract
    }
    pub fn variable_fields(&self) -> &[StoredSecretValue] {
        &self.variables
    }
    pub fn root_config_fields(&self) -> &[StoredSecretValue] {
        &self.root_config
    }
    pub fn root_result_fields(&self) -> &[StoredSecretValue] {
        &self.root_result
    }
    pub fn source_origins(&self) -> &[crate::secret_provenance::SecretOrigin] {
        self.closure.origins()
    }
    pub fn attempts(&self) -> &[NativeCacheCompletionAttempt] {
        &self.attempts
    }
    pub fn controls(&self) -> &[ReceiptContext] {
        &self.controls
    }
    pub fn issued_controls(&self) -> &[PreparedInputSource] {
        &self.issued
    }
    pub fn original_controls(&self) -> &[ReceiptContext] {
        &self.original_controls
    }
    pub fn original_native_source(&self) -> Option<&NativeSourceEvidence> {
        self.original_source.as_ref()
    }
    pub fn has_failed_latest_attempt(&self) -> bool {
        self.failed
    }
    pub fn transaction(&self) -> &str {
        &self.transaction
    }

    pub async fn ensure_fresh_on(&self, conn: &mut PgConnection) -> Result<()> {
        self.require_transaction(conn).await?;
        self.eof.ensure_fresh_on(conn).await?;
        self.closure.ensure_fresh_on(conn).await?;
        for fence in &self.fences {
            fence.ensure_fresh_on(conn).await?;
        }
        for control in self.controls.iter() {
            control.ensure_fresh_on(conn).await?;
        }
        for control in &self.original_controls {
            control.ensure_fresh_on(conn).await?;
        }
        for control in self.issued.iter() {
            control.ensure_fresh_on(conn).await?;
        }
        if let Some(frame) = &self.frame {
            frame.ensure_fresh_on(conn).await?;
        }
        for attempt in &self.attempts {
            for input in &attempt.inputs {
                input.ensure_fresh_on(conn).await?;
            }
        }
        self.ensure_fresh()
    }

    pub fn ensure_fresh(&self) -> Result<()> {
        self.eof.ensure_fresh()?;
        self.closure.ensure_fresh()?;
        for fence in &self.fences {
            fence.ensure_fresh()?;
        }
        for control in self.controls.iter() {
            control.ensure_fresh()?;
        }
        for control in &self.original_controls {
            control.ensure_fresh()?;
        }
        for control in self.issued.iter() {
            control.ensure_fresh()?;
        }
        if let Some(frame) = &self.frame {
            frame.ensure_fresh()?;
        }
        for attempt in &self.attempts {
            attempt
                .source
                .cache_read_policy()
                .ok_or_else(denied)?
                .ensure_fresh()?;
            for input in &attempt.inputs {
                input.ensure_fresh()?;
            }
        }
        if let Some(source) = &self.original_source {
            source
                .cache_read_policy()
                .ok_or_else(denied)?
                .ensure_fresh()?;
        }
        Ok(())
    }

    /// Equality reads only, no source discovery/authorization under target locks.
    pub async fn require_current(&self, conn: &mut PgConnection) -> Result<()> {
        let parent = Executions::find_by_id(&mut *conn, self.parent.id)
            .await?
            .ok_or_else(denied)?;
        let workflow = Workflows::find_by_id(&mut *conn, self.workflow.id)
            .await?
            .ok_or_else(denied)?;
        let fields =
            Secrets::find_stored_by_entity(&mut *conn, ENTITY_WORKFLOW_VARIABLES, self.parent.id)
                .await?;
        if !same(&parent, &self.parent)?
            || !same(&workflow, &self.workflow)?
            || fields != self.variables
        {
            return Err(denied());
        }
        self.require_root_fields(conn).await?;
        self.require_frontier_and_attempts(conn).await
    }

    async fn require_transaction(&self, conn: &mut PgConnection) -> Result<()> {
        let actual: String = sqlx::query_scalar("SELECT pg_current_xact_id()::TEXT")
            .fetch_one(&mut *conn)
            .await?;
        if actual != self.transaction {
            return Err(denied());
        }
        Ok(())
    }

    // Checkpoint has already compared its authentic frame's authorized workflow
    // publication buffers. Do not replace those with a caller-supplied baseline.
    pub(super) async fn require_frontier_and_attempts(
        &self,
        conn: &mut PgConnection,
    ) -> Result<()> {
        self.ensure_fresh_on(conn).await?;
        self.require_root_fields(conn).await?;
        let current =
            Iterations::find_by_workflow_task(&mut *conn, self.workflow.id, &self.task_name)
                .await?;
        if !same(&current, &self.original)? {
            return Err(denied());
        }
        self.require_attempts(conn).await
    }

    async fn require_attempts(&self, conn: &mut PgConnection) -> Result<()> {
        let rows =
            Executions::native_cache_attempt_inventory(conn, self.workflow.id, &self.task_name)
                .await?;
        if rows.len() != self.attempts.len() {
            return Err(denied());
        }
        for (row, held) in rows.iter().zip(&self.attempts) {
            if !same(row, &held.execution)? {
                return Err(denied());
            }
            let fields =
                Secrets::find_stored_by_entity(&mut *conn, ENTITY_EXECUTION_RESULT, row.id).await?;
            if fields != held.fields {
                return Err(denied());
            }
            let config = Secrets::find_stored_by_entity(
                &mut *conn,
                crate::secret_values::ENTITY_EXECUTION_CONFIG,
                row.id,
            )
            .await?;
            if config != held.config_fields {
                return Err(denied());
            }
        }
        Ok(())
    }

    // Invoked immediately after the real EOF frontier is bound by the checkpoint.
    // No public setter can bless a new iteration, publication or inventory.
    pub(super) async fn accept_bound_eof(
        &self,
        conn: &mut PgConnection,
        row: &WorkflowCacheIteration,
        workflow: &WorkflowExecution,
        fields: &[StoredSecretValue],
    ) -> Result<()> {
        self.ensure_fresh_on(conn).await?;
        self.require_root_fields(conn).await?;
        self.require_attempts(conn).await?;
        // These are the writer's already-compared authentic frame buffers from
        // before the frontier wait, never a public expected-state argument.
        let actual_parent = Executions::find_by_id(&mut *conn, self.parent.id)
            .await?
            .ok_or_else(denied)?;
        let actual_workflow = Workflows::find_by_id(&mut *conn, self.workflow.id)
            .await?
            .ok_or_else(denied)?;
        let actual_fields =
            Secrets::find_stored_by_entity(&mut *conn, ENTITY_WORKFLOW_VARIABLES, self.parent.id)
                .await?;
        if !same(&actual_parent, &self.parent)?
            || workflow.id != self.workflow.id
            || !same(&actual_workflow, workflow)?
            || actual_fields.as_slice() != fields
        {
            return Err(denied());
        }
        if let Some(original) = &self.original {
            if !same(original, row)? {
                return Err(denied());
            }
        } else {
            let initial = self.eof.initial_create_input()?;
            if row.workflow_execution != initial.workflow_execution
                || row.task_name != initial.task_name
                || row.namespace != initial.namespace
                || row.generation != initial.generation
                || row.page_size != initial.page_size
                || row.batch_size != initial.batch_size
                || row.concurrency != initial.concurrency
                || row.state != WorkflowCacheIterationState::Scanning
                || row.last_external_id.is_some()
                || row.next_batch_index != 0
                || row.scanned_count != 0
                || row.dispatched_count != 0
                || row.completed_at.is_some()
                || row.error_summary.is_some()
                || !self.attempts.is_empty()
            {
                return Err(denied());
            }
        }
        let actual = Iterations::find_by_id(&mut *conn, row.id)
            .await?
            .ok_or_else(denied)?;
        if !same(&actual, row)? {
            return Err(denied());
        }
        self.bound_iteration
            .set(row.clone())
            .map_err(|_| denied())?;
        Ok(())
    }

    async fn require_root_fields(&self, conn: &mut PgConnection) -> Result<()> {
        for (kind, held) in [
            (
                crate::secret_values::ENTITY_EXECUTION_CONFIG,
                &self.root_config,
            ),
            (ENTITY_EXECUTION_RESULT, &self.root_result),
        ] {
            let actual = Secrets::find_stored_by_entity(&mut *conn, kind, self.parent.id).await?;
            if &actual != held {
                return Err(denied());
            }
        }
        Ok(())
    }

    /// Pure comparison for H's later finite recipe. It grants no output VALUE.
    pub async fn require_terminal_frontier(&self, conn: &mut PgConnection) -> Result<()> {
        self.ensure_fresh_on(conn).await?;
        let expected = self.terminal_iteration.get().ok_or_else(denied)?;
        let actual = Iterations::find_by_id(&mut *conn, expected.id)
            .await?
            .ok_or_else(denied)?;
        if !same(expected, &actual)? {
            return Err(denied());
        }
        self.require_attempts(conn).await
    }

    // Only B's real successful terminal writer can seal the returned row. Do not
    // expose an expected frontier setter to H/A or any caller-supplied frame.
    pub(super) async fn record_terminal_frontier(
        &self,
        conn: &mut PgConnection,
        row: WorkflowCacheIteration,
    ) -> Result<()> {
        let mut expected = self.bound_iteration.get().ok_or_else(denied)?.clone();
        if self.failed
            || row.state != WorkflowCacheIterationState::Completed
            || row.completed_at.is_none()
            || row.error_summary.is_some()
        {
            return Err(denied());
        }
        expected.state = WorkflowCacheIterationState::Completed;
        expected.completed_at = row.completed_at;
        expected.updated = row.updated;
        if !same(&expected, &row)? {
            return Err(denied());
        }
        self.terminal_iteration.set(row).map_err(|_| denied())?;
        self.require_terminal_frontier(conn).await
    }

    pub(super) async fn prepare(
        conn: &mut PgConnection,
        context: &Context,
        original: Option<&WorkflowCacheIteration>,
        eof: Arc<PreparedCacheInput>,
        controls: Arc<Vec<ReceiptContext>>,
        issued: Arc<Vec<PreparedInputSource>>,
        frame: Option<Arc<dyn NativeCacheRenderFrame>>,
        key: &str,
    ) -> Result<Option<Self>> {
        if !eof.exhausted() || eof.count() != 0 {
            return Ok(None);
        }
        let (original_controls, original_source, original_fence) = if let Some(row) = original {
            let Some((controls, source, fence)) = original_controls(
                conn,
                context.parent.id,
                context.workflow.id,
                &context.path,
                row,
                key,
            )
            .await?
            else {
                return Ok(None);
            };
            (controls, Some(source), Some(fence))
        } else {
            (Vec::new(), None, None)
        };
        let rows = Executions::native_cache_attempt_inventory(
            conn,
            context.workflow.id,
            context
                .task
                .get("name")
                .and_then(Value::as_str)
                .ok_or_else(denied)?,
        )
        .await?;
        if rows.len() > MAX_ATTEMPTS {
            return Err(denied());
        }
        let task_name = context
            .task
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(denied)?
            .to_owned();
        let contract = original_contract(conn, &context.parent).await?;
        let variables = Secrets::find_stored_by_entity(
            &mut *conn,
            ENTITY_WORKFLOW_VARIABLES,
            context.parent.id,
        )
        .await?;
        let root_config = Secrets::find_stored_by_entity(
            &mut *conn,
            crate::secret_values::ENTITY_EXECUTION_CONFIG,
            context.parent.id,
        )
        .await?;
        let root_result =
            Secrets::find_stored_by_entity(&mut *conn, ENTITY_EXECUTION_RESULT, context.parent.id)
                .await?;
        let mut bytes = 0;
        charge(&mut bytes, &context.parent)?;
        charge(&mut bytes, &context.workflow)?;
        charge(&mut bytes, &contract)?;
        charge_fields(&mut bytes, &variables)?;
        charge_fields(&mut bytes, &root_config)?;
        charge_fields(&mut bytes, &root_result)?;
        charge(&mut bytes, &original)?;
        if controls.len() + issued.len() > 64 {
            return Err(denied());
        }
        for control in controls.iter() {
            charge_receipt(&mut bytes, control)?;
        }
        for control in &original_controls {
            charge_receipt(&mut bytes, control)?;
        }
        // A's issued Control constructor already enforces bounded immutable
        // ancestry. Include its retained canonical value in the aggregate budget.
        for control in issued.iter() {
            charge(&mut bytes, control.value())?;
        }
        let mut seen = BTreeSet::new();
        let mut attempts = Vec::new();
        let mut fences = original_fence.into_iter().collect::<Vec<_>>();
        if let Some(source) = &original_source {
            bytes = bytes
                .checked_add(source.retained_bytes())
                .ok_or_else(denied)?;
            if bytes > MAX_BYTES {
                return Err(denied());
            }
        }
        let mut inventory = BTreeMap::new();
        let mut failed = false;
        for row in rows {
            let metadata = row.workflow_task.as_ref().ok_or_else(denied)?;
            let index = metadata.task_index.ok_or_else(denied)?;
            if row.parent != Some(context.parent.id)
                || metadata.workflow_execution != context.workflow.id
                || metadata.task_name != task_name
                || index < 0
                || metadata.retry_count != row.retry_count
                || row.retry_count < 0
            {
                return Err(denied());
            }
            let latest = seen.insert(index);
            if latest && !terminal(row.status) {
                return Ok(None);
            }
            if latest && row.status != ExecutionStatus::Completed {
                failed = true;
            }
            let admitted = Admissions::require(conn, RuntimeOwner::Execution(row.id)).await?;
            fences.push(Authority::retained_admission_fence(conn, admitted.id).await?);
            let path = Materialize::workflow_task_site_path(
                conn,
                context.parent.id,
                &task_name,
                &row.action_ref,
            )
            .await?;
            if path != context.path {
                return Err(denied());
            }
            let child_contract = original_contract(conn, &row).await?;
            // This observes an original accepted producer, not a new delivery to
            // a terminal execution. Keep the canonical acceptance and its
            // independent fence, including empty-parameter contributor ceilings.
            let (config_fields, inputs) =
                super::super::accepted_execution_source::prepare_observation(
                    conn,
                    &row,
                    &child_contract,
                    key,
                )
                .await?;
            fences.push(
                super::super::accepted_input::AcceptedInputRepository::revalidate_execution(
                    conn, row.id, key,
                )
                .await?,
            );
            let mut native = BTreeSet::new();
            for receipt in &inputs {
                charge_receipt(&mut bytes, receipt)?;
                for source in receipt.native_source_metadata() {
                    if source.source_kind() == "cache" {
                        native.insert(source.snapshot());
                    }
                }
            }
            if native.len() != 1 {
                return Err(denied());
            }
            let membership =
                Snapshots::storage_evidence(conn, *native.first().ok_or_else(denied)?).await?;
            let source = Snapshots::source_evidence(conn, membership.metadata(), key).await?;
            let cache = membership.cache().ok_or_else(denied)?;
            let iteration = original.ok_or_else(denied)?;
            if cache.iteration() != iteration.id
                || cache.workflow() != context.workflow.id
                || cache.namespace() != iteration.namespace
                || cache.generation() != iteration.generation
                || cache.ordinal() != i64::from(index)
                || cache.page_size() != iteration.page_size
                || cache.batch_size() != iteration.batch_size
                || cache.entries().is_empty()
                || metadata.task_batch
                    != Some(i32::try_from(cache.entries().len()).map_err(|_| denied())?)
            {
                return Err(denied());
            }
            let identity = (
                cache.before().map(str::to_owned),
                cache.after().map(str::to_owned),
                cache.entries().len(),
                cache
                    .entries()
                    .iter()
                    .map(|entry| {
                        (
                            entry.id(),
                            entry.external_id().to_owned(),
                            entry.source_checksum().map(str::to_owned),
                        )
                    })
                    .collect::<Vec<_>>(),
            );
            if let Some(previous) = inventory.insert(index, identity.clone()) {
                if previous != identity {
                    return Err(denied());
                }
            }
            charge(&mut bytes, membership.encrypted_projection())?;
            let fields =
                Secrets::find_stored_by_entity(&mut *conn, ENTITY_EXECUTION_RESULT, row.id).await?;
            let restored = crate::secret_values::restore_secret_values(
                row.result.clone().unwrap_or(Value::Null),
                &fields,
                key,
            )?;
            if !crate::secret_values::redacted_paths(&restored).is_empty() {
                return Err(denied());
            }
            charge(&mut bytes, &restored)?;
            charge_fields(&mut bytes, &fields)?;
            charge_fields(&mut bytes, &config_fields)?;
            charge(&mut bytes, &row)?;
            charge(&mut bytes, &child_contract)?;
            let outcome = if matches!(
                row.status,
                ExecutionStatus::Completed | ExecutionStatus::Failed | ExecutionStatus::Timeout
            ) {
                Some(Inputs::prepare_outcome(conn, context.parent.id, row.id).await?)
            } else {
                None
            }; // Cancelled/Abandoned are cache-specific terminal statuses, never ordinary Outcome tokens.
            attempts.push(NativeCacheCompletionAttempt {
                execution: row,
                contract: child_contract,
                fields,
                config_fields,
                inputs,
                membership,
                source,
                outcome,
                latest,
            });
        }
        let mut after = None;
        let mut scanned = 0_i64;
        for (ordinal, (index, (before, end, count, _))) in inventory.iter().enumerate() {
            if i64::from(*index) != i64::try_from(ordinal).map_err(|_| denied())?
                || before != &after
                || end.is_none()
            {
                return Err(denied());
            }
            after = end.clone();
            scanned = scanned
                .checked_add(i64::try_from(*count).map_err(|_| denied())?)
                .ok_or_else(denied)?;
        }
        if let Some(row) = original {
            let count = i64::try_from(inventory.len()).map_err(|_| denied())?;
            if row.next_batch_index != count
                || row.dispatched_count != count
                || row.scanned_count != scanned
                || row.last_external_id != after
                || row.state != WorkflowCacheIterationState::Scanning
                || row.completed_at.is_some()
                || row.error_summary.is_some()
                || eof.index() != count
                || eof.after_external_id() != after.as_deref()
            {
                return Err(denied());
            }
        } else if !attempts.is_empty() || eof.index() != 0 {
            return Err(denied());
        }
        let ids = attempts
            .iter()
            .map(|attempt| attempt.execution.id)
            .collect::<Vec<_>>();
        let closure = measure(
            Op::NativeClosure,
            Native::capture_with_key(conn, context.parent.id, &ids, Some(key)),
        )
        .await?;
        let mut receipt_roots = Vec::new();
        for receipt in controls
            .iter()
            .chain(&original_controls)
            .chain(attempts.iter().flat_map(|attempt| attempt.inputs.iter()))
        {
            receipt_roots.push(receipt);
        }
        measure(
            Op::CompletionAccounting,
            charge_graph(conn, &mut bytes, &receipt_roots, &issued, eof.snapshot, key),
        )
        .await
        .map_err(|error| preparation_stage(error, "completion-graph-accounting"))?;
        let source = Self {
            transaction: sqlx::query_scalar("SELECT pg_current_xact_id()::TEXT")
                .fetch_one(&mut *conn)
                .await?,
            parent: context.parent.clone(),
            workflow: context.workflow.clone(),
            task_path: context.path.clone(),
            task_name,
            original: original.cloned(),
            eof,
            bound_iteration: OnceLock::new(),
            terminal_iteration: OnceLock::new(),
            contract,
            variables,
            root_config,
            root_result,
            attempts,
            controls,
            issued,
            original_controls,
            original_source,
            frame,
            closure,
            fences,
            failed,
        };
        source.require_current(conn).await?;
        Ok(Some(source))
    }
}

async fn charge_graph(
    conn: &mut PgConnection,
    bytes: &mut usize,
    roots: &[&ReceiptContext],
    issued: &[PreparedInputSource],
    eof: Id,
    key: &str,
) -> Result<()> {
    let (charge, _) = Inputs::prepared_source_graph_bounds(conn, roots, issued, key).await?;
    *bytes = bytes.checked_add(charge).ok_or_else(denied)?;
    if *bytes > MAX_BYTES {
        return Err(denied());
    }
    // The EOF is owned by this preparation, not a receipt-selected source.
    // Its ancestry was charged through the authenticated Controls above.
    {
        let size: i64 = sqlx::query_scalar("SELECT octet_length(body::TEXT)::BIGINT + octet_length(encrypted_value::TEXT)::BIGINT + octet_length(body_digest)::BIGINT + octet_length(encryption_key_hash)::BIGINT FROM runtime_native_input_snapshot WHERE id=$1")
            .bind(eof).fetch_one(&mut *conn).await?;
        *bytes = bytes
            .checked_add(usize::try_from(size).map_err(|_| denied())?)
            .ok_or_else(denied)?;
        if *bytes > MAX_BYTES {
            return Err(denied());
        }
    }
    Ok(())
}

async fn original_controls(
    conn: &mut PgConnection,
    root: Id,
    workflow: Id,
    path: &str,
    row: &WorkflowCacheIteration,
    key: &str,
) -> Result<
    Option<(
        Vec<ReceiptContext>,
        NativeSourceEvidence,
        ApprovalEffectFence,
    )>,
> {
    let first: Option<Id> = sqlx::query_scalar("SELECT snapshot FROM runtime_native_cache_frontier WHERE bound_iteration=$1 AND ordinal=0 ORDER BY snapshot LIMIT 1")
        .bind(row.id).fetch_optional(&mut *conn).await?;
    let Some(first) = first else {
        return Ok(None);
    };
    let evidence = Snapshots::storage_evidence(conn, first).await?;
    let source = Snapshots::source_evidence(conn, evidence.metadata(), key).await?;
    let root_admission = Authority::execution_admission(conn, root)
        .await?
        .ok_or_else(denied)?;
    if source.renderer_admission() != root_admission.id {
        return Err(denied());
    }
    // Authenticate the original frontier's own retained task authority even for
    // empty EOF, which has no admitted batch child. Use the actual retained
    // namespace site and the page's real ordinal. This issues no render, authored
    // declaration, child outcome, Boolean VALUE or result receipt.
    let original = Snapshots::evidence(
        conn,
        first,
        source.renderer_admission(),
        &format!("{path}/iterate_cache/namespace"),
        "/index",
        key,
        false,
    )
    .await?;
    let cache = evidence.cache().ok_or_else(denied)?;
    if cache.iteration() != row.id
        || cache.workflow() != workflow
        || cache.namespace() != row.namespace
        || cache.generation() != row.generation
        || cache.ordinal() != 0
        || cache.before().is_some()
    {
        return Err(denied());
    }
    let mut controls = Vec::new();
    for (id, site) in evidence.receipt_edges() {
        if site.execution != root
            || site.kind != InputEntityKind::WorkflowVariables
            || site.purpose != InputSitePurpose::Control
            || !["namespace", "generation", "owner_type", "owner_ref"]
                .iter()
                .any(|field| site.path == format!("{path}/iterate_cache/{field}"))
        {
            return Err(denied());
        }
        let origin = crate::secret_provenance::SecretOrigin::Entity {
            entity_type: format!("declared_input:{}:control", site.kind.as_str()),
            entity_id: *id,
            path: site.path.clone(),
        };
        controls.push(Inputs::receipt_context(conn, site, &origin, key).await?);
    }
    Ok(Some((controls, source, original.fence)))
}

fn terminal(status: ExecutionStatus) -> bool {
    matches!(
        status,
        ExecutionStatus::Completed
            | ExecutionStatus::Failed
            | ExecutionStatus::Timeout
            | ExecutionStatus::Cancelled
            | ExecutionStatus::Abandoned
    )
}
fn same<T: serde::Serialize>(left: &T, right: &T) -> Result<bool> {
    Ok(serde_json::to_value(left)? == serde_json::to_value(right)?)
}
fn charge<T: serde::Serialize + ?Sized>(bytes: &mut usize, value: &T) -> Result<()> {
    *bytes = bytes
        .checked_add(serde_json::to_vec(value)?.len())
        .ok_or_else(denied)?;
    if *bytes > MAX_BYTES {
        return Err(denied());
    }
    Ok(())
}
fn charge_fields(bytes: &mut usize, fields: &[StoredSecretValue]) -> Result<()> {
    if fields.len() > 256 {
        return Err(denied());
    }
    for field in fields {
        charge(
            bytes,
            &(
                &field.json_path,
                &field.encrypted_value,
                &field.encryption_key_hash,
                &field.source_kind,
                &field.source_ref,
            ),
        )?;
    }
    Ok(())
}
fn charge_receipt(bytes: &mut usize, receipt: &ReceiptContext) -> Result<()> {
    charge(bytes, receipt.accepted_value())?;
    charge(bytes, receipt.site())?;
    for source in receipt.source_receipts() {
        charge(
            bytes,
            &(source.receipt(), source.retained_by(), source.site()),
        )?;
    }
    for source in receipt.native_source_metadata() {
        charge(
            bytes,
            &(
                source.snapshot(),
                source.source_kind(),
                source.target(),
                source.read_version(),
                source.source_site(),
            ),
        )?;
    }
    Ok(())
}
async fn original_contract(
    conn: &mut PgConnection,
    execution: &Execution,
) -> Result<ActionSchemaBinding> {
    let binding = Schemas::find(conn, BindingOwner::Execution, execution.id)
        .await?
        .ok_or_else(denied)?;
    binding.validate()?;
    let snapshot = execution.executable_snapshot.as_ref().ok_or_else(denied)?;
    if execution.action != Some(snapshot.executable.action.id)
        || execution.action_ref != snapshot.executable.action.r#ref
        || execution.pack_release != Some(snapshot.release.id)
        || execution.pack_release_digest.as_deref() != Some(snapshot.release.digest.as_str())
        || binding.input.source
            != (crate::value_schema::SourceIdentity::Released {
                component_ref: execution.action_ref.clone(),
                release_id: snapshot.release.id,
                release_digest: snapshot.release.digest.clone(),
            })
    {
        return Err(denied());
    }
    Ok(binding)
}
