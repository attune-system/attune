//! Recorded queue consumers share the effect connection with source validation.
use super::*;
mod bulk_metadata;
use attune_common::repositories::runtime_target::{
    ComponentTarget, RuntimeTarget, RuntimeTargetRepository,
};
pub(super) use bulk_metadata::apply as bulk_metadata_apply;

pub(super) fn active(user: &AuthenticatedUser) -> ApiResult<bool> {
    crate::recorded_consumers::is_recorded(user)
}

pub(super) async fn dispatch_target(
    conn: &mut PgConnection,
    user: &AuthenticatedUser,
    action: i64,
) -> ApiResult<()> {
    let authority = user
        .recorded_runtime_authority(conn)
        .await?
        .ok_or_else(|| ApiError::Forbidden("Recorded authority unavailable".into()))?;
    let target = RuntimeTargetRepository::resolve(
        conn,
        RuntimeTarget::Component(ComponentTarget::Action, action),
        authority.credential_binding().sponsor_identity_id,
    )
    .await?
    .ok_or_else(|| ApiError::Forbidden("Dispatch target unavailable".into()))?;
    // Defining a dispatch target is not accepting a new item/execution. The
    // current action's enabled fence stays at the new-acceptance boundary.
    if !authority.allows_target(&target, RbacAction::Execute) {
        return Err(ApiError::Forbidden(
            "Dispatch target selection denied".into(),
        ));
    }
    Ok(())
}

pub(super) async fn creation(
    conn: &mut PgConnection,
    user: &AuthenticatedUser,
    pack: Option<i64>,
    reference: &str,
) -> ApiResult<()> {
    let authority = user
        .recorded_runtime_authority(conn)
        .await?
        .ok_or_else(|| ApiError::Forbidden("Recorded authority unavailable".into()))?;
    attune_common::repositories::runtime_read::RuntimeReadRepository::require_untainted(
        conn,
        authority.credential_binding().admission_id,
    )
    .await?;
    let target = RuntimeTargetRepository::resolve_queue_creation(
        conn,
        pack,
        reference,
        authority.credential_binding().sponsor_identity_id,
    )
    .await?
    .ok_or_else(|| ApiError::Forbidden("Queue owner unavailable".into()))?;
    if !authority.allows_new_effect(&target, RbacAction::Create) {
        return Err(ApiError::Forbidden("Queue creation denied".into()));
    }
    Ok(())
}

/// Metadata and item authority are distinct resources. No requester IAM lookup.
pub(super) async fn fence(
    conn: &mut PgConnection,
    user: &AuthenticatedUser,
    queue: &WorkQueue,
    item: Option<i64>,
    items: bool,
    action: RbacAction,
) -> ApiResult<Option<attune_common::repositories::runtime_authority::ApprovalEffectFence>> {
    if !active(user)? {
        return Ok(None);
    }
    let authority = user
        .recorded_runtime_authority(conn)
        .await?
        .ok_or_else(|| ApiError::Forbidden("Recorded queue authority unavailable".into()))?;
    if !matches!(action, RbacAction::Read | RbacAction::Decrypt) {
        attune_common::repositories::runtime_read::RuntimeReadRepository::require_untainted(
            conn,
            authority.credential_binding().admission_id,
        )
        .await?;
    }
    let metadata_mutation = !items && !matches!(action, RbacAction::Read | RbacAction::Decrypt);
    let current = if metadata_mutation {
        WorkQueueRepository::find_by_id_for_update(conn, queue.id).await?
    } else {
        WorkQueueRepository::find_by_id_for_share(conn, queue.id).await?
    }
    .ok_or_else(|| ApiError::NotFound("Queue unavailable".into()))?;
    if current.pack != queue.pack
        || current.is_adhoc != queue.is_adhoc
        || current.updated != queue.updated
        || current.r#ref != queue.r#ref
        || current.pack_ref != queue.pack_ref
        || current.reference_visibility != queue.reference_visibility
        || current.reference_allowed_pack_refs != queue.reference_allowed_pack_refs
    {
        return Err(ApiError::Conflict(
            "Queue authorization target changed; retry".into(),
        ));
    }
    if let Some(id) = item {
        let row = if matches!(action, RbacAction::Read | RbacAction::Decrypt) {
            WorkQueueItemRepository::find_by_queue_and_id(&mut *conn, queue.id, id).await?
        } else {
            WorkQueueItemRepository::find_by_id_for_update(conn, id).await?
        }
        .ok_or_else(|| ApiError::NotFound("Queue item unavailable".into()))?;
        if row.queue != queue.id {
            return Err(ApiError::NotFound("Queue item unavailable".into()));
        }
    }
    let target = match (items, item) {
        (true, Some(id)) => RuntimeTarget::QueueItem(id),
        (true, None) => RuntimeTarget::QueueItems(queue.id),
        (false, _) => RuntimeTarget::Component(ComponentTarget::Queue, queue.id),
    };
    let resolved = RuntimeTargetRepository::resolve(
        conn,
        target,
        authority.credential_binding().sponsor_identity_id,
    )
    .await?
    .ok_or_else(|| ApiError::Forbidden("Queue target unavailable".into()))?;
    // Existing items continue after ordinary action disablement. New acceptance
    // retains independent accepting-items and current-action fences.
    let permitted = authority.allows_target(&resolved, action);
    if !permitted {
        return Err(ApiError::Forbidden(
            "Recorded queue operation denied".into(),
        ));
    }
    authority.ensure_fresh_on(conn).await?;
    Ok(Some(authority.effect_fence().clone()))
}

pub(super) fn item_response(item: WorkQueueItem) -> WorkQueueItemResponse {
    let mut response = WorkQueueItemResponse::from(item);
    // A contribution proves neither linked-execution visibility nor whole-batch
    // result/log visibility. Never return runtime result/error copies here.
    response.requested_by_execution = None;
    response.requested_by_enforcement = None;
    response.leased_execution = None;
    response.lease_token = None;
    response.lease_expires_at = None;
    response.last_error = None;
    response.ack_summary = None;
    // Schema defaults and arbitrary metadata can contain copied plaintext.
    response.schema_binding = None;
    response.metadata = serde_json::Value::Null;
    response
}

pub(super) fn queue_response(queue: WorkQueue) -> WorkQueueResponse {
    let mut response = WorkQueueResponse::from(queue);
    response.action_params = attune_common::secret_values::redaction_marker();
    response.config = attune_common::secret_values::redaction_marker();
    response.item_schema = serde_json::json!({});
    response.resolved_dispatch_tuning = None;
    response
}

pub(super) async fn queues(
    state: &Arc<AppState>,
    user: &AuthenticatedUser,
    query: &WorkQueueQueryParams,
    pack_ref: Option<&str>,
) -> ApiResult<PaginatedResponse<WorkQueueSummary>> {
    let mut tx = state.db.begin().await?;
    let authority = user
        .recorded_runtime_authority(&mut tx)
        .await?
        .ok_or_else(|| ApiError::Forbidden("Recorded authority unavailable".into()))?;
    let pack = match pack_ref {
        Some(reference) => Some(
            PackRepository::find_by_ref(&mut *tx, reference)
                .await?
                .ok_or_else(|| ApiError::NotFound("Pack unavailable".into()))?
                .id,
        ),
        None => None,
    };
    let page = query.page.max(1);
    let per_page = query.per_page.clamp(1, 100);
    let result = attune_common::repositories::runtime_authority::recorded_collections::queues(
        &mut tx,
        &authority.queue_collection(),
        &WorkQueueSearchFilters {
            pack,
            enabled: query.enabled,
            is_adhoc: query.is_adhoc,
            search: query.search.clone(),
            limit: per_page,
            offset: page.saturating_sub(1).saturating_mul(per_page),
            ..Default::default()
        },
    )
    .await?;
    Ok(PaginatedResponse::new(
        result
            .rows
            .into_iter()
            .map(WorkQueueSummary::from)
            .collect(),
        &PaginationParams {
            page,
            page_size: per_page,
        },
        result.total,
    ))
}

pub(super) async fn items(
    state: &Arc<AppState>,
    user: &AuthenticatedUser,
    queue_ref: &str,
    query: &WorkQueueItemQueryParams,
) -> ApiResult<PaginatedResponse<WorkQueueItemResponse>> {
    let mut tx = state.db.begin().await?;
    let authority = user
        .recorded_runtime_authority(&mut tx)
        .await?
        .ok_or_else(|| ApiError::Forbidden("Recorded authority unavailable".into()))?;
    let queue = WorkQueueRepository::find_by_ref_including_retired(&mut *tx, queue_ref)
        .await?
        .ok_or_else(|| ApiError::NotFound("Queue unavailable".into()))?;
    let result = attune_common::repositories::runtime_authority::recorded_collections::queue_items(
        &mut tx,
        &authority.queue_item_collection(),
        &WorkQueueItemSearchFilters {
            queue: Some(queue.id),
            item_key: query.item_key.clone(),
            enqueue_source: query.enqueue_source.clone(),
            statuses: (!query.statuses.is_empty()).then_some(query.statuses.clone()),
            limit: query.limit(),
            offset: query.offset(),
            ..Default::default()
        },
        None,
    )
    .await?;
    Ok(PaginatedResponse::new(
        result.rows.into_iter().map(item_response).collect(),
        &PaginationParams {
            page: query.page,
            page_size: query.per_page,
        },
        result.total,
    ))
}

pub(super) async fn preview(
    state: &Arc<AppState>,
    user: &AuthenticatedUser,
    queue_ref: &str,
    request: &PreviewWorkQueueItemsRequest,
) -> ApiResult<PreviewWorkQueueItemsResponse> {
    let mut tx = state.db.begin().await?;
    let authority = user
        .recorded_runtime_authority(&mut tx)
        .await?
        .ok_or_else(|| ApiError::Forbidden("Recorded authority unavailable".into()))?;
    let queue = WorkQueueRepository::find_by_ref_including_retired(&mut *tx, queue_ref)
        .await?
        .ok_or_else(|| ApiError::NotFound("Queue unavailable".into()))?;
    let selector = selector_from_request(&request.selector);
    let result = map_selector_result(
        attune_common::repositories::runtime_authority::recorded_collections::queue_items(
            &mut tx,
            &authority.queue_item_collection(),
            &WorkQueueItemSearchFilters {
                queue: Some(queue.id),
                limit: request.limit.min(100),
                ..Default::default()
            },
            Some(&selector),
        )
        .await,
    )?;
    let items: Vec<_> = result.rows.into_iter().map(item_response).collect();
    Ok(PreviewWorkQueueItemsResponse {
        matched_count: result.total,
        preview_count: items.len(),
        items,
    })
}

pub(super) async fn payload_disclosure(
    conn: &mut PgConnection,
    user: &AuthenticatedUser,
    queue: &WorkQueue,
    item: &WorkQueueItem,
) -> ApiResult<()> {
    let effect_fence = fence(conn, user, queue, Some(item.id), true, RbacAction::Read).await?;
    fence(conn, user, queue, Some(item.id), true, RbacAction::Decrypt).await?;
    let authority = user
        .recorded_runtime_authority(conn)
        .await?
        .ok_or_else(|| ApiError::Forbidden("Recorded authority unavailable".into()))?;
    let rows = attune_common::repositories::execution_secret_value::ExecutionSecretValueRepository::find_bounded_provenance(
        &mut *conn, &[attune_common::secret_values::ENTITY_QUEUE_ITEM_PAYLOAD], &[item.id],
    ).await?;
    crate::recorded_disclosure::authorize_origins(conn, &authority, &rows).await?;
    if let Some(fence) = effect_fence {
        fence.ensure_fresh_on(conn).await?;
    }
    Ok(())
}

pub(super) async fn bulk_apply(
    state: &Arc<AppState>,
    user: &AuthenticatedUser,
    queue: &WorkQueue,
    request: &ApplyWorkQueueItemsRequest,
    action: RbacAction,
) -> ApiResult<ApplyWorkQueueItemsResponse> {
    let mut tx = state.db.begin().await?;
    let effect_fence = fence(&mut tx, user, queue, None, true, action).await?;
    // Counts/selectors disclose item metadata independently of mutation rights.
    fence(&mut tx, user, queue, None, true, RbacAction::Read).await?;
    let editors = if request.operation == WorkQueueItemBulkOperation::PatchPayload {
        Some(queue_payload_editors(&mut tx, user).await?)
    } else {
        None
    };
    let selector = selector_from_request(&request.selector);
    let mut selected = map_selector_result(
        WorkQueueItemRepository::list_selected_mutable(&mut *tx, queue.id, &selector).await,
    )?;
    selected.sort_by_key(|item| item.id);
    let matched_count = selected.len() as u64;
    let mut affected_count = 0;
    let mut preview = Vec::new();
    for candidate in selected {
        fence(&mut tx, user, queue, Some(candidate.id), true, action).await?;
        fence(
            &mut tx,
            user,
            queue,
            Some(candidate.id),
            true,
            RbacAction::Read,
        )
        .await?;
        let item = WorkQueueItemRepository::find_by_id_for_update(&mut tx, candidate.id)
            .await?
            .ok_or_else(|| ApiError::Conflict("Selected item disappeared".into()))?;
        if !WorkQueueItemRepository::is_mutable_pending_status(item.status) {
            continue;
        }
        if preview.len() < request.preview_limit.min(100) as usize {
            preview.push(item_response(item.clone()));
        }
        let input = match request.operation {
            WorkQueueItemBulkOperation::Cancel => UpdateWorkQueueItemInput {
                status: Some(attune_common::models::WorkQueueItemStatus::Cancelled),
                ..Default::default()
            },
            WorkQueueItemBulkOperation::Reprioritize => UpdateWorkQueueItemInput {
                priority: request.priority,
                ..Default::default()
            },
            WorkQueueItemBulkOperation::PatchPayload => {
                if queue.is_adhoc {
                    attune_common::repositories::ad_hoc_queue_admission::AdHocQueueAdmissionRepository::require_payload_binding(&mut tx, item.id).await?;
                }
                attune_common::repositories::recorded_input_authority::RecordedInputAuthorityRepository::authorize_queue_payload_edit(&mut tx, item.id, editors.as_deref().unwrap_or_default()).await?;
                let original = restore_queue_payload(state, user, &mut tx, queue, &item).await?;
                let merged = merge_patch_value(
                    original,
                    request
                        .payload_patch
                        .as_ref()
                        .ok_or_else(|| ApiError::BadRequest("Missing payload patch".into()))?,
                );
                let (payload, secrets) =
                    prepare_queue_payload(state, &admitted_item_schema(&item)?, &merged)?;
                attune_common::repositories::execution_secret_value::ExecutionSecretValueRepository::replace_entity_with_conn(&mut tx, attune_common::secret_values::ENTITY_QUEUE_ITEM_PAYLOAD, item.id, &secrets).await?;
                UpdateWorkQueueItemInput {
                    payload: Some(payload),
                    ..Default::default()
                }
            }
        };
        if WorkQueueItemRepository::update_if_statuses(
            &mut *tx,
            item.id,
            WorkQueueItemRepository::mutable_pending_statuses(),
            input,
        )
        .await?
        .is_some()
        {
            affected_count += 1;
        }
    }
    if let Some(fence) = effect_fence {
        fence.ensure_fresh_on(&mut tx).await?;
    }
    tx.commit().await?;
    emit_queue_items_bulk_audit(
        state,
        user,
        queue,
        request,
        matched_count,
        affected_count,
        matched_count.saturating_sub(affected_count),
    );
    Ok(ApplyWorkQueueItemsResponse {
        operation: request.operation,
        matched_count,
        affected_count,
        skipped_count: matched_count.saturating_sub(affected_count),
        preview_count: preview.len(),
        items: preview,
    })
}
