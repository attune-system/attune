//! Real repository targets exercised through subscription, dispatch and dequeue.
use super::*;
use attune_common::{
    models::*,
    repositories::{
        execution::CreateExecutionInput,
        identity::{
            CreateIdentityInput, CreatePermissionAssignmentInput, CreatePermissionSetInput,
            PermissionAssignmentRepository,
        },
        inquiry::CreateInquiryInput,
        work_queue::{CreateWorkQueueInput, CreateWorkQueueItemInput},
        Create, Delete,
    },
    test_database::TestDatabase,
};
use serde_json::json;

fn notification(kind: &str, id: i64) -> Notification {
    Notification {
        notification_type: format!("{kind}_created"),
        entity_type: kind.into(),
        entity_id: id,
        payload: json!({}),
        user_id: None,
        timestamp: chrono::Utc::now(),
    }
}
fn scoped(resource: Resource, reference: &str) -> Grant {
    Grant {
        resource,
        actions: vec![Action::Read],
        constraints: Some(GrantConstraints {
            refs: Some(vec![reference.into()]),
            ..Default::default()
        }),
    }
}

#[tokio::test]
async fn notifier_targets_scope_unknown_and_revocation() -> Result<()> {
    let config = Config::load_from_file(&format!(
        "{}/../../config.test.yaml",
        env!("CARGO_MANIFEST_DIR")
    ))?;
    let db = TestDatabase::create(&config.database)
        .await?
        .with_cleanup_on_drop();
    let pool = db.pool();
    let owner = IdentityRepository::create(
        pool,
        CreateIdentityInput {
            login: "notifier_target_owner".into(),
            display_name: None,
            password_hash: None,
            attributes: json!({}),
        },
    )
    .await?;
    let queue = WorkQueueRepository::create(
        pool,
        CreateWorkQueueInput {
            r#ref: "targets.inbox".into(),
            pack: None,
            pack_ref: None,
            is_adhoc: true,
            label: "Inbox".into(),
            description: None,
            enabled: true,
            accepting_new_items: true,
            dispatch_action: None,
            dispatch_action_ref: "targets.run".into(),
            default_priority: 0,
            allow_pending_update: false,
            update_strategy: WorkQueueUpdateStrategy::MergePatch,
            batch_mode: WorkQueueBatchMode::Batch,
            item_schema: json!({}),
            action_params: json!({}),
            trace_tag_template: None,
            permission_set_refs: None,
            config: json!({}),
            reference_visibility: ActionReferenceVisibility::Private,
            reference_allowed_pack_refs: vec![],
        },
    )
    .await?;
    let item = WorkQueueItemRepository::create(
        pool,
        CreateWorkQueueItemInput {
            queue: queue.id,
            queue_ref: queue.r#ref.clone(),
            item_key: None,
            priority: 0,
            status: WorkQueueItemStatus::Pending,
            payload: json!({}),
            metadata: json!({}),
            trace_tag: None,
            enqueue_source: "test".into(),
            requested_by_identity: None,
            requested_by_execution: None,
            requested_by_enforcement: None,
            leased_execution: None,
            lease_token: None,
            lease_expires_at: None,
            attempt_count: 0,
            last_error: None,
            ack_summary: None,
        },
    )
    .await?;
    let execution = ExecutionRepository::create(
        pool,
        CreateExecutionInput {
            action_ref: "targets.run".into(),
            executor: Some(owner.id),
            status: ExecutionStatus::Running,
            ..Default::default()
        },
    )
    .await?;
    let inquiry = InquiryRepository::create(
        pool,
        CreateInquiryInput {
            created_by_execution: execution.id,
            prompt: "test".into(),
            response_schema: None,
            response_options: vec![],
            assigned_to: None,
            status: InquiryStatus::Pending,
            response: None,
            timeout_at: None,
        },
    )
    .await?;
    for (kind, id, resource, reference) in [
        (
            "work_queue",
            queue.id,
            Resource::Queues,
            queue.r#ref.clone(),
        ),
        (
            "work_queue_item",
            item.id,
            Resource::QueueItems,
            queue.r#ref.clone(),
        ),
        (
            "inquiry",
            inquiry.id,
            Resource::Inquiries,
            format!("inquiry:{}", inquiry.id),
        ),
    ] {
        let n = notification(kind, id);
        let permitted = WebSocketAuthContext::test_context(
            owner.id,
            TokenType::Access,
            vec![scoped(resource, &reference)],
        );
        let denied = WebSocketAuthContext::test_context(
            owner.id,
            TokenType::Access,
            vec![scoped(resource, "neighbor.other")],
        );
        for filter in [
            SubscriptionFilter::Entity {
                entity_type: kind.into(),
                entity_id: id,
            },
            SubscriptionFilter::EntityType(kind.into()),
            SubscriptionFilter::NotificationType(n.notification_type.clone()),
        ] {
            assert!(
                filter_allowed_for_identity(
                    &filter,
                    owner.id,
                    &TokenType::Access,
                    &[],
                    &[],
                    &permitted.grants,
                    pool
                )
                .await
            );
        }
        assert!(
            notification_allowed_for_identity(&n, &permitted, pool).await,
            "authorized {kind}"
        );
        assert!(
            !notification_allowed_for_identity(&n, &denied, pool).await,
            "neighbor {kind}"
        );
        assert!(
            !notification_allowed_for_identity(
                &n,
                &WebSocketAuthContext::test_context(owner.id, TokenType::Access, vec![]),
                pool
            )
            .await
        );
        // All/user filters cannot bypass the production dispatch authorization.
        let manager = SubscriberManager::new();
        let (tx, mut rx) = mpsc::unbounded_channel();
        manager.register("denied".into(), Arc::new(denied), tx);
        manager.subscribe(&"denied".into(), SubscriptionFilter::All);
        dispatch_notification(&manager, pool, n.clone()).await;
        assert!(rx.try_recv().is_err());
    }
    let grant = scoped(Resource::Queues, &queue.r#ref);
    let set = PermissionSetRepository::create(
        pool,
        CreatePermissionSetInput {
            r#ref: "targets.observe".into(),
            pack: None,
            pack_ref: None,
            label: None,
            description: None,
            grants: serde_json::to_value(vec![grant.clone()])?,
        },
    )
    .await?;
    let assignment = PermissionAssignmentRepository::create(
        pool,
        CreatePermissionAssignmentInput {
            identity: owner.id,
            permset: set.id,
        },
    )
    .await?;
    let auth = WebSocketAuthContext::test_context(owner.id, TokenType::Access, vec![grant]);
    let message = ClientMessage::Notification(notification("work_queue", queue.id));
    assert!(human_delivery::allowed(pool, &auth, &message).await);
    PermissionAssignmentRepository::delete(pool, assignment.id).await?;
    assert!(
        !human_delivery::allowed(pool, &auth, &message).await,
        "queued frame after revocation"
    );
    let unknown = notification("future_execution", queue.id);
    assert!(!notification_allowed_for_identity(&unknown, &auth, pool).await);
    assert!(!human_delivery::allowed(pool, &auth, &ClientMessage::Notification(unknown)).await);
    for filter in [
        SubscriptionFilter::EntityType("future".into()),
        SubscriptionFilter::Entity {
            entity_type: "future".into(),
            entity_id: queue.id,
        },
        SubscriptionFilter::NotificationType("future_execution_created".into()),
    ] {
        assert!(
            !filter_allowed_for_identity(
                &filter,
                owner.id,
                &TokenType::Access,
                &[],
                &["admin".into()],
                &[],
                pool
            )
            .await
        );
    }
    db.cleanup().await?;
    Ok(())
}
