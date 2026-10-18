//! Actual wait failure writer. A owns the distinct recipe; J owns the one
//! release/publication/terminal operation and never accepts an output JSON proof.
use super::super::inherited_input::PreparedNativeWaitCompletionDestination;
use super::*;
use crate::repositories::Update;
use std::sync::Arc;

pub(super) async fn require_original_targets(
    conn: &mut PgConnection,
    context: &Context,
    output: &PreparedNativeWaitCompletionDestination,
) -> Result<()> {
    output.ensure_fresh_on(conn).await?;
    let workflow = Workflows::find_by_id_for_update(&mut *conn, context.workflow.id)
        .await?
        .ok_or_else(denied)?;
    let parent = Executions::find_by_id_for_update(&mut *conn, context.parent.id)
        .await?
        .ok_or_else(denied)?;
    if serde_json::to_value(workflow)? != serde_json::to_value(&context.workflow)?
        || serde_json::to_value(parent)? != serde_json::to_value(&context.parent)?
    {
        return Err(denied());
    }
    output.require_original_rows(conn).await
}

pub(super) async fn finish(
    conn: &mut PgConnection,
    context: &Context,
    original: &[StoredSecretValue],
    source: &PreparedNativeWaitCompletion,
    program: &Arc<staged::Program>,
    frame: &Arc<cache_frame::Frame>,
    output: &PreparedNativeWaitCompletionDestination,
    key: &str,
) -> Result<(ReceiptContext, Execution)> {
    source.require_writer(program, frame)?;
    if !program.successors.is_empty() {
        return Err(denied());
    }
    let (variables, fields) = program
        .publish(
            conn,
            context.workflow.id,
            context.parent.id,
            context.workflow.variables.clone(),
            original,
            key,
        )
        .await?;
    let mut current = context.workflow.current_tasks.clone();
    current.retain(|task| task != source.task_name());
    let mut failed = context.workflow.failed_tasks.clone();
    if !failed.iter().any(|task| task == source.task_name()) {
        failed.push(source.task_name().into());
    }
    let projected = Workflows::update(
        &mut *conn,
        context.workflow.id,
        super::super::workflow::UpdateWorkflowExecutionInput {
            variables: Some(variables.clone()),
            current_tasks: Some(current),
            failed_tasks: Some(failed),
            ..Default::default()
        },
    )
    .await?;
    // Even with no list/cache child, this is the exact original completion
    // frame held by A's Arc token. No replacement map/baseline can seal it.
    frame.seal(conn, &variables, &fields).await?;
    let (sealed_workflow, _) = frame.require_sealed_state(conn).await?;
    if serde_json::to_value(&sealed_workflow)? != serde_json::to_value(&projected)? {
        return Err(denied());
    }
    let receipt =
        Inputs::record_native_wait_completion(conn, output, &projected, output.value(), key)
            .await?;
    let sources = output.secret_path_sources(&receipt)?;
    let (result, secrets) =
        crate::secret_values::redact_secret_path_sources(output.value().clone(), &sources);
    let encrypted = crate::secret_values::prepare_secret_values(secrets, key)?;
    if encrypted.len() > 256 || serde_json::to_vec(&encrypted)?.len() > 1024 * 1024 {
        return Err(denied());
    }
    Secrets::replace_entity_with_conn(
        conn,
        crate::secret_values::ENTITY_EXECUTION_RESULT,
        context.parent.id,
        &encrypted,
    )
    .await?;
    output.ensure_fresh_on(conn).await?;
    source.ensure_fresh_on(conn).await?;
    receipt.ensure_fresh_on(conn).await?;
    // Only the official root receipt and its persisted ciphertext permit the
    // terminal writes. All three unhandled outcomes remain workflow Failed.
    super::super::runtime_read::RuntimeReadRepository::close_execution(conn, context.parent.id)
        .await?;
    Workflows::update(
        &mut *conn,
        context.workflow.id,
        super::super::workflow::UpdateWorkflowExecutionInput {
            status: Some(output.terminal_status()),
            error_message: Some(output.error_message().into()),
            ..Default::default()
        },
    )
    .await?;
    let completed = Executions::update(
        &mut *conn,
        context.parent.id,
        super::super::execution::UpdateExecutionInput {
            status: Some(output.terminal_status()),
            result: Some(result),
            ..Default::default()
        },
    )
    .await?;
    output.ensure_fresh_on(conn).await?;
    source.ensure_fresh_on(conn).await?;
    receipt.ensure_fresh_on(conn).await?;
    Ok((receipt, completed))
}
