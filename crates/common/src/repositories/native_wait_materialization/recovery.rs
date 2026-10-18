//! Prospective terminal recovery consumes the real reserved wait observation.
//! Registration/release is owned by initial.rs, not an invented wait row here.
use super::super::inherited_input::PreparedNativeWaitCompletionDestination;
use super::super::native_cache_materialization::NativeCacheRenderFrame;
use super::*;
use crate::repositories::Update;
use std::sync::Arc;

fn preparation_stage(error: Error, stage: &'static str) -> Error {
    match error {
        Error::PermissionDenied(message) => {
            Error::PermissionDenied(format!("Wait output [{stage}] denied: {message}"))
        }
        other => other,
    }
}

pub(super) struct Recovery {
    program: Arc<staged::Program>,
    frame: Arc<cache_frame::Frame>,
    completion: Option<Arc<PreparedNativeWaitCompletion>>,
    output: Option<PreparedNativeWaitCompletionDestination>,
    completions: Vec<Execution>,
    children: Vec<continuation::Recovery>,
    receipts: Vec<ReceiptContext>,
}
impl Recovery {
    pub(super) async fn prepare(
        conn: &mut PgConnection,
        context: &Context,
        observation: &Observation,
        bound: Arc<OnceLock<super::super::native_input_snapshot::RegisteredInitialWait>>,
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
            return Err(preparation_stage(denied(), "program-shape"));
        }
        let program = Arc::new(
            staged::Program::prepare(conn, context, observation, key)
                .await
                .map_err(|error| preparation_stage(error, "program"))?,
        );
        let Observation::Prospective(registration) = observation else {
            return Err(denied());
        };
        let frame = Arc::new(
            cache_frame::Frame::prospective(
                conn,
                context,
                registration,
                program.clone(),
                bound,
                key,
            )
            .await
            .map_err(|error| preparation_stage(error, "frame"))?,
        );
        let completion = if program.successors.is_empty() {
            Some(Arc::new(
                PreparedNativeWaitCompletion::prepare(
                    conn,
                    context,
                    observation,
                    program.clone(),
                    frame.clone(),
                    key,
                )
                .await
                .map_err(|error| preparation_stage(error, "completion-source"))?,
            ))
        } else {
            None
        };
        let output = match &completion {
            Some(source) => Some(
                Inputs::prepare_native_wait_completion(conn, source.clone(), key)
                    .await
                    .map_err(|error| preparation_stage(error, "output-proof"))?,
            ),
            None => None,
        };
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
            bytes
                .checked_add(serde_json::to_vec(&child.child.value)?.len())
                .ok_or_else(denied)
        })?;
        if bytes > 1024 * 1024 {
            return Err(denied());
        }
        let recovery = Self {
            program,
            frame,
            completion,
            output,
            completions: vec![],
            children,
            receipts: vec![],
        };
        recovery
            .ensure_fresh_on(conn)
            .await
            .map_err(|error| preparation_stage(error, "prepared-freshness"))?;
        Ok(recovery)
    }
    pub(super) fn completions(&self) -> &[Execution] {
        &self.completions
    }
    pub(super) async fn guard_original(
        &self,
        conn: &mut PgConnection,
        context: &Context,
    ) -> Result<()> {
        if let Some(output) = &self.output {
            super::completion_writer::require_original_targets(conn, context, output).await?;
        }
        Ok(())
    }
    pub(super) async fn apply(
        &mut self,
        conn: &mut PgConnection,
        context: &Context,
        original: &[StoredSecretValue],
        key: &str,
    ) -> Result<Vec<Execution>> {
        if let Some(source) = &self.completion {
            let (receipt, completed) = super::completion_writer::finish(
                conn,
                context,
                original,
                source,
                &self.program,
                &self.frame,
                self.output.as_ref().ok_or_else(denied)?,
                key,
            )
            .await?;
            self.receipts.push(receipt);
            self.completions.push(completed);
            self.ensure_fresh_on(conn).await?;
            return Ok(vec![]);
        }
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
        self.frame.ensure_fresh_on(conn).await?;
        if let Some(output) = &self.output {
            output.ensure_fresh_on(conn).await?;
        }
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
        self.frame.ensure_fresh()?;
        if let Some(output) = &self.output {
            output.ensure_fresh()?;
        }
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
