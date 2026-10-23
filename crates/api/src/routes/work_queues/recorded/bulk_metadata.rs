//! Cancel/reprioritize only. B retains payload editing and input admission.
use super::*;
use attune_common::repositories::runtime_authority::recorded_collections::bulk::{
    apply_metadata, MetadataOperation,
};

pub(in crate::routes::work_queues) async fn apply(
    state: &Arc<AppState>,
    user: &AuthenticatedUser,
    queue_ref: &str,
    request: &ApplyWorkQueueItemsRequest,
) -> ApiResult<ApplyWorkQueueItemsResponse> {
    let operation = match request.operation {
        WorkQueueItemBulkOperation::Cancel => MetadataOperation::Cancel,
        WorkQueueItemBulkOperation::Reprioritize => MetadataOperation::Reprioritize(
            request
                .priority
                .ok_or_else(|| ApiError::BadRequest("Missing priority".into()))?,
        ),
        WorkQueueItemBulkOperation::PatchPayload => {
            return Err(ApiError::BadRequest("Not a metadata operation".into()))
        }
    };
    let mut tx = state.db.begin().await?;
    // Exactly one actual-transaction source validation, before capture and target
    // protection. The repository carries this authority to its final SQL fence.
    let authority = user
        .recorded_runtime_authority(&mut tx)
        .await?
        .ok_or_else(|| ApiError::Forbidden("Recorded authority unavailable".into()))?;
    // Retirement stops new acceptance, not control of already accepted items.
    // Resolve only after source validation, on the effect connection too.
    let queue = WorkQueueRepository::find_by_ref_including_retired(&mut *tx, queue_ref)
        .await?
        .ok_or_else(|| ApiError::NotFound(format!("Work queue '{}' not found", queue_ref)))?;
    let selector = selector_from_request(&request.selector);
    let result = match apply_metadata(
        &mut tx,
        &authority,
        &queue,
        &selector,
        operation,
        request.preview_limit,
    )
    .await
    {
        // Selector-drift conflicts must be retryable, not partial successes.
        Err(attune_common::Error::InvalidState(message)) => {
            return Err(ApiError::Conflict(message))
        }
        Err(attune_common::Error::Database(sqlx::Error::Database(error)))
            if is_jsonpath_database_error(error.as_ref()) =>
        {
            return Err(ApiError::BadRequest(
                "Invalid SQL/JSONPath queue item selector".into(),
            ));
        }
        other => map_selector_result(other)?,
    };
    let credential = authority.credential_binding();
    let items = result
        .preview
        .into_iter()
        .map(item_response)
        .collect::<Vec<_>>();
    authority.ensure_fresh_on(&mut tx).await?;
    tx.commit().await?;
    // Cancellation permits a read-tainted process because it writes only a fixed
    // state. Never persist its arbitrary selector text, variable keys or values.
    use attune_common::audit::{AuditCategory, AuditEventBuilder, AuditOutcome};
    state.audit_emitter.emit(
        AuditEventBuilder::new(
            AuditCategory::Admin,
            "queue_items.bulk_operation.applied",
            AuditOutcome::Success,
        )
        .resource("work_queue")
        .resource_id(queue.id)
        .actor_identity(credential.sponsor_identity_id)
        .actor_token_type("execution")
        .with_details(serde_json::json!({
            "operation": request.operation,
            "execution_id": credential.execution_id,
            "admission_id": credential.admission_id,
            "queue_id": queue.id,
            "matched_count": result.matched_count,
            "affected_count": result.matched_count,
            "skipped_count": 0,
        }))
        .build(),
    );
    Ok(ApplyWorkQueueItemsResponse {
        operation: request.operation,
        matched_count: result.matched_count,
        affected_count: result.matched_count,
        skipped_count: 0,
        preview_count: items.len(),
        items,
    })
}
