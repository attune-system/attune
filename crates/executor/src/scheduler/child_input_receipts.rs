//! Opaque input proofs owned by one workflow transaction and its publications.
use super::*;
use attune_common::repositories::{
    accepted_input::AcceptedInputRepository,
    declared_input_materialization::DeclaredInputMaterializationRepository as Materialize,
    execution_admission::ExecutionAdmissionRepository as Admission,
    inherited_input::ReceiptContext,
    native_input_origin::{NativeInputClosure, NativeInputOriginRepository},
    runtime_authority::{ApprovalEffectFence, RuntimeOwner},
};

pub(super) struct PreparedExecutionInputs {
    pub(super) execution: Execution,
    receipts: Vec<ReceiptContext>,
    source: Option<NativeInputClosure>,
}

impl PreparedExecutionInputs {
    pub(super) fn new(
        execution: Execution,
        receipts: Vec<ReceiptContext>,
        source: Option<NativeInputClosure>,
    ) -> Self {
        Self {
            execution,
            receipts,
            source,
        }
    }
}

struct ExecutionInputProofs {
    execution: i64,
    receipts: Vec<ReceiptContext>,
    source: Option<NativeInputClosure>,
    acceptance: Option<ApprovalEffectFence>,
}

#[derive(Default)]
pub(super) struct TransactionInputReceipts {
    executions: Vec<ExecutionInputProofs>,
}

impl TransactionInputReceipts {
    pub(super) fn retain(&mut self, prepared: PreparedExecutionInputs) -> Execution {
        let PreparedExecutionInputs {
            execution,
            receipts,
            source,
        } = prepared;
        self.executions.push(ExecutionInputProofs {
            execution: execution.id,
            receipts,
            source,
            acceptance: None,
        });
        execution
    }

    pub(super) fn contains(&self, execution: i64) -> bool {
        self.executions
            .iter()
            .any(|proof| proof.execution == execution)
    }

    /// Reused rows retain their original seal. This never calls a sealing API.
    pub(super) async fn retain_existing(
        &mut self,
        conn: &mut PgConnection,
        execution: &Execution,
        encryption_key: Option<&str>,
    ) -> Result<()> {
        anyhow::ensure!(
            !self.contains(execution.id),
            "Execution input proofs were already retained"
        );
        let recorded = Admission::find(conn, RuntimeOwner::Execution(execution.id)).await?;
        let (receipts, acceptance, source) = if recorded.is_some() {
            let key = encryption_key.ok_or_else(|| {
                anyhow::anyhow!("Accepted workflow input encryption is unavailable")
            })?;
            let acceptance =
                AcceptedInputRepository::revalidate_execution(conn, execution.id, key).await?;
            let receipts =
                Materialize::prepare_execution_input_receipts(conn, execution.id, key).await?;
            let source =
                NativeInputOriginRepository::capture_with_key(conn, execution.id, &[], Some(key))
                    .await?;
            (receipts, Some(acceptance), Some(source))
        } else {
            Self::require_legacy_input_source(conn, execution).await?;
            (Vec::new(), None, None)
        };
        self.executions.push(ExecutionInputProofs {
            execution: execution.id,
            receipts,
            source,
            acceptance,
        });
        Ok(())
    }

    async fn require_legacy_input_source(
        conn: &mut PgConnection,
        execution: &Execution,
    ) -> Result<()> {
        anyhow::ensure!(
            execution
                .executable_snapshot
                .as_ref()
                .is_some_and(|snapshot| { snapshot.executable.action.is_adhoc }),
            "Recorded child input admission is unavailable"
        );
        // An ad-hoc TARGET beneath a recorded/managed SOURCE still requires
        // admission. Mirror the production creation boundary, not its flag alone.
        if let Some(task) = &execution.workflow_task {
            let workflow =
                WorkflowExecutionRepository::find_by_id(&mut *conn, task.workflow_execution)
                    .await?
                    .ok_or_else(|| anyhow::anyhow!("Workflow input lineage is unavailable"))?;
            anyhow::ensure!(
                execution.parent == Some(workflow.execution),
                "Workflow input parent does not match its workflow"
            );
        }
        let parent_id = execution
            .parent
            .ok_or_else(|| anyhow::anyhow!("Workflow input parent is unavailable"))?;
        {
            let parent = ExecutionRepository::find_by_id(&mut *conn, parent_id)
                .await?
                .ok_or_else(|| anyhow::anyhow!("Workflow input parent is unavailable"))?;
            let parent_recorded = Admission::find(conn, RuntimeOwner::Execution(parent_id)).await?;
            let parent_adhoc = parent
                .executable_snapshot
                .as_ref()
                .map(|snapshot| snapshot.executable.action.is_adhoc)
                .ok_or_else(|| {
                    anyhow::anyhow!("Workflow input parent executable is unavailable")
                })?;
            anyhow::ensure!(
                parent_recorded.is_none() && parent_adhoc,
                "Recorded child input admission is unavailable"
            );
        }
        attune_common::delegation::require_execution_refs_for_share(
            conn,
            execution.executor,
            &execution.permission_set_refs,
        )
        .await?;
        Ok(())
    }

    /// Call only with the transaction in which these contexts were prepared.
    pub(super) async fn ensure_fresh_on(&self, conn: &mut PgConnection) -> Result<()> {
        for proof in &self.executions {
            if let Some(source) = &proof.source {
                source.ensure_fresh_on(conn).await?;
            }
            if let Some(acceptance) = &proof.acceptance {
                acceptance.ensure_fresh_on(conn).await?;
            }
            for receipt in &proof.receipts {
                receipt.ensure_fresh_on(conn).await?;
            }
        }
        Ok(())
    }

    pub(super) fn ensure_fresh(&self) -> Result<()> {
        for proof in &self.executions {
            Self::ensure_execution_proof_fresh(proof)?;
        }
        Ok(())
    }

    /// Check again for EACH child, after waits for earlier publications.
    pub(super) fn ensure_execution_fresh(&self, execution: i64) -> Result<()> {
        let proof = self
            .executions
            .iter()
            .find(|proof| proof.execution == execution)
            .ok_or_else(|| anyhow::anyhow!("Publication has no retained execution input proofs"))?;
        Self::ensure_execution_proof_fresh(proof)
    }

    pub(super) fn publication_wait_deadline(
        &self,
        execution: i64,
    ) -> Result<Option<chrono::DateTime<chrono::Utc>>> {
        let proof = self
            .executions
            .iter()
            .find(|proof| proof.execution == execution)
            .ok_or_else(|| anyhow::anyhow!("Publication has no retained execution input proofs"))?;
        Self::ensure_execution_proof_fresh(proof)?;
        let mut candidates = Vec::new();
        if let Some(acceptance) = &proof.acceptance {
            candidates.extend(acceptance.approval_deadline());
        }
        if let Some(source) = &proof.source {
            candidates.extend(source.publication_wait_deadline()?);
        }
        for receipt in &proof.receipts {
            candidates.extend(receipt.publication_wait_deadline()?);
        }
        Ok(candidates.into_iter().min())
    }

    fn ensure_execution_proof_fresh(proof: &ExecutionInputProofs) -> Result<()> {
        if let Some(source) = &proof.source {
            source.ensure_fresh()?;
        }
        if let Some(acceptance) = &proof.acceptance {
            acceptance.ensure_fresh()?;
        }
        for receipt in &proof.receipts {
            receipt.ensure_fresh()?;
        }
        Ok(())
    }

    /// Append only AFTER checking and committing the other transaction. Its
    /// transaction-bound contexts must not be checked on a later transaction.
    pub(super) fn append_committed(&mut self, mut other: Self) {
        self.executions.append(&mut other.executions);
    }
}

/// Source-only preparation at an actual workflow transaction commit owner.
/// Pending DTOs remain plain, cloneable transport data, never proof owners.
pub(super) struct PreparedWorkflowInputPublications {
    inputs: TransactionInputReceipts,
    waits: Vec<std::sync::Arc<NativeWaitCheckpoint>>,
}

/// Transport bookkeeping and opaque frame custody. Message IDs are not proofs.
/// There is intentionally no owning IntoIterator or generic Extend impl:
/// recursive merges must use extend(Self), which preserves deferred owner IDs.
#[derive(Debug, Clone, Default)]
pub(super) struct WorkflowPublications {
    messages: Vec<PendingExecutionRequested>,
    input_owners: std::collections::BTreeSet<i64>,
    waits: Vec<std::sync::Arc<NativeWaitCheckpoint>>,
}

impl WorkflowPublications {
    pub(super) fn push(&mut self, pending: PendingExecutionRequested) {
        self.input_owners.insert(pending.execution_id);
        self.messages.push(pending);
    }

    pub(super) fn register_created(&mut self, execution: i64) {
        self.input_owners.insert(execution);
    }

    pub(super) fn extend(&mut self, other: Self) {
        self.input_owners.extend(other.input_owners);
        self.messages.extend(other.messages);
        self.waits.extend(other.waits);
    }

    pub(super) fn retain_wait(&mut self, wait: NativeWaitCheckpoint) {
        self.waits.push(std::sync::Arc::new(wait));
    }

    pub(super) fn owner_ids(&self) -> impl Iterator<Item = i64> + '_ {
        self.input_owners.iter().copied()
    }

    /// Consume messages ONLY after the commit owner has prepared and retained
    /// its opaque input proofs. This is not a recursive merge operation.
    pub(super) fn into_messages_after_preparation(self) -> Vec<PendingExecutionRequested> {
        self.messages
    }
}

impl std::ops::Deref for WorkflowPublications {
    type Target = [PendingExecutionRequested];

    fn deref(&self) -> &Self::Target {
        &self.messages
    }
}

impl<'a> IntoIterator for &'a WorkflowPublications {
    type Item = &'a PendingExecutionRequested;
    type IntoIter = std::slice::Iter<'a, PendingExecutionRequested>;

    fn into_iter(self) -> Self::IntoIter {
        self.messages.iter()
    }
}

impl PreparedWorkflowInputPublications {
    /// Transport coherence only, not input authority or permission delegation.
    pub(super) fn require_pending_matches_execution(
        execution: &Execution,
        request: &PendingExecutionRequested,
    ) -> Result<()> {
        anyhow::ensure!(
            execution.status == ExecutionStatus::Requested
                && execution.id == request.execution_id
                && execution.action == Some(request.action_id)
                && execution.action_ref == request.action_ref
                && execution.parent == Some(request.parent_id)
                && execution.enforcement == request.enforcement_id
                && execution.config == request.config
                && execution.pack_release == request.release_id
                && execution.pack_release_digest == request.release_digest,
            "Pending workflow publication differs from its persisted execution"
        );
        Ok(())
    }

    /// The standalone fanout reuse path must not combine an old row with new
    /// caller-selected action/parent metadata. The collector uses the SAME check.
    pub(super) fn prepare_pending_data(
        execution: &Execution,
        action_id: i64,
        action_ref: &str,
        parent: &Execution,
    ) -> Result<PendingExecutionRequested> {
        let request = PendingExecutionRequested {
            execution_id: execution.id,
            action_id,
            action_ref: action_ref.into(),
            parent_id: parent.id,
            enforcement_id: parent.enforcement,
            config: execution.config.clone(),
            release_id: execution.pack_release,
            release_digest: execution.pack_release_digest.clone(),
        };
        Self::require_pending_matches_execution(execution, &request)?;
        Ok(request)
    }

    pub(super) async fn prepare(
        conn: &mut PgConnection,
        pending: &WorkflowPublications,
        encryption_key: Option<&str>,
    ) -> Result<Self> {
        let mut inputs = TransactionInputReceipts::default();
        let owner_ids: Vec<i64> = pending.owner_ids().collect();
        let mut executions = std::collections::BTreeMap::new();
        // The batch size limits individual indexed lookups, not legal fanout.
        // An empty collection performs no query. No retained-table scan occurs.
        for ids in owner_ids.chunks(256) {
            anyhow::ensure!(
                ids.iter().all(|id| *id > 0),
                "Invalid workflow input owner ID"
            );
            let rows = ExecutionRepository::find_by_ids(&mut *conn, ids).await?;
            anyhow::ensure!(
                rows.len() == ids.len(),
                "Tracked workflow input owner is unavailable"
            );
            for row in rows {
                anyhow::ensure!(
                    ids.binary_search(&row.id).is_ok(),
                    "Unexpected workflow input owner"
                );
                anyhow::ensure!(
                    executions.insert(row.id, row).is_none(),
                    "Duplicate workflow input owner row"
                );
            }
        }
        for request in pending {
            let execution = executions
                .get(&request.execution_id)
                .ok_or_else(|| anyhow::anyhow!("Pending workflow input owner is unavailable"))?;
            Self::require_pending_matches_execution(execution, request)?;
        }
        // Every ID, including deferred owners with no message, reopens actual
        // original input authority once. Duplicate payloads were ALL checked.
        for execution in executions.into_values() {
            inputs
                .retain_existing(conn, &execution, encryption_key)
                .await?;
        }
        Ok(Self {
            inputs,
            waits: pending.waits.clone(),
        })
    }

    pub(super) async fn ensure_fresh_on(&self, conn: &mut PgConnection) -> Result<()> {
        for wait in &self.waits {
            wait.ensure_fresh_on(conn).await?;
        }
        self.inputs.ensure_fresh_on(conn).await
    }

    pub(super) fn ensure_fresh(&self) -> Result<()> {
        for wait in &self.waits {
            wait.ensure_fresh()?;
        }
        self.inputs.ensure_fresh()
    }

    pub(super) fn ensure_child_fresh(&self, execution: i64) -> Result<()> {
        for wait in &self.waits {
            wait.ensure_fresh()?;
        }
        self.inputs.ensure_execution_fresh(execution)
    }

    pub(super) fn publication_wait_deadline(
        &self,
        execution: i64,
    ) -> Result<Option<chrono::DateTime<chrono::Utc>>> {
        self.inputs.publication_wait_deadline(execution)
    }
}
