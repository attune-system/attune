//! Prospective terminal recovery consumes the real reserved wait observation.
//! Registration/release is owned by initial.rs, not an invented wait row here.
use super::*;
use crate::repositories::Update;

pub(super) struct Recovery {
    program: staged::Program,
    children: Vec<continuation::Recovery>,
    receipts: Vec<ReceiptContext>,
}
impl Recovery {
    pub(super) async fn prepare(
        conn: &mut PgConnection,
        context: &Context,
        observation: &Observation,
        key: &str,
    ) -> Result<Self> {
        let graph = &context
            .parent
            .executable_snapshot
            .as_ref()
            .ok_or_else(denied)?
            .executable
            .workflow_definition
            .as_ref()
            .ok_or_else(denied)?
            .definition;
        if !continuation::supported_program(&context.task, graph)? {
            return Err(denied());
        }
        let program = staged::Program::prepare(conn, context, observation, key).await?;
        if program.successors.is_empty() {
            return Err(Error::InvalidState(
                "Native wait without a selected recovery requires its finite output recipe".into(),
            ));
        }
        let mut children = vec![];
        for name in &program.successors {
            if !continuation::ordinary(continuation::unique_task(graph, name)?) {
                return Err(denied());
            }
            children.push(
                continuation::prepare_recovery_from_program(conn, context, name, &program, key)
                    .await?,
            );
        }
        let bytes = children.iter().try_fold(program.bytes()?, |bytes, child| {
            Ok::<_, Error>(
                bytes
                    .checked_add(serde_json::to_vec(&child.child.value)?.len())
                    .ok_or_else(denied)?,
            )
        })?;
        if bytes > 1024 * 1024 {
            return Err(denied());
        }
        let recovery = Self {
            program,
            children,
            receipts: vec![],
        };
        recovery.ensure_fresh_on(conn).await?;
        Ok(recovery)
    }
    pub(super) async fn apply(
        &mut self,
        conn: &mut PgConnection,
        context: &Context,
        original: &[StoredSecretValue],
        key: &str,
    ) -> Result<Vec<Execution>> {
        // The caller has already bound/released its exact positive reservation.
        // A's public finalization still checks all normalized transitive releases.
        let (variables, _) = self
            .program
            .publish(
                conn,
                context.workflow.id,
                context.parent.id,
                context.workflow.variables.clone(),
                original,
                key,
            )
            .await?;
        let name = context
            .task
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(denied)?;
        let mut current = context.workflow.current_tasks.clone();
        current.retain(|task| task != name);
        let mut pending = vec![];
        for child in &mut self.children {
            pending.push(
                continuation::write_recovery_child(
                    conn,
                    &mut child.child,
                    self.program.controls(),
                    &mut self.receipts,
                    key,
                )
                .await?,
            );
            if !current.contains(&child.name) {
                current.push(child.name.clone());
            }
        }
        let mut completed = context.workflow.completed_tasks.clone();
        if !completed.iter().any(|task| task == name) {
            completed.push(name.into());
        }
        let failed = context
            .workflow
            .failed_tasks
            .iter()
            .filter(|task| task.as_str() != name)
            .cloned()
            .collect();
        Workflows::update(
            &mut *conn,
            context.workflow.id,
            super::super::workflow::UpdateWorkflowExecutionInput {
                variables: Some(variables),
                current_tasks: Some(current),
                completed_tasks: Some(completed),
                failed_tasks: Some(failed),
                ..Default::default()
            },
        )
        .await?;
        self.ensure_fresh_on(conn).await?;
        Ok(pending)
    }
    pub(super) async fn ensure_fresh_on(&self, conn: &mut PgConnection) -> Result<()> {
        self.program.ensure_fresh_on(conn).await?;
        for child in &self.children {
            child.fence.ensure_fresh_on(conn).await?;
            for field in &child.child.fields {
                field.rendered.ensure_fresh_on(conn).await?;
            }
        }
        for receipt in &self.receipts {
            receipt.ensure_fresh_on(conn).await?;
        }
        Ok(())
    }
    pub(super) fn ensure_fresh(&self) -> Result<()> {
        self.program.ensure_fresh()?;
        for child in &self.children {
            child.fence.ensure_fresh()?;
            for field in &child.child.fields {
                field.rendered.ensure_fresh()?;
            }
        }
        for receipt in &self.receipts {
            receipt.ensure_fresh()?;
        }
        Ok(())
    }
}
