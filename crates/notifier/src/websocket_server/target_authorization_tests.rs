//! Real repository targets exercised through subscription, dispatch and dequeue.
use super::*;
use attune_common::{
    models::{
        ActionReferenceVisibility, ExecutionStatus, InquiryStatus, WorkQueueBatchMode,
        WorkQueueItemStatus, WorkQueueUpdateStrategy,
    },
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
    let neighbor = IdentityRepository::create(
        pool,
        CreateIdentityInput {
            login: "notifier_neighbor".into(),
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
            reference_visibility: ActionReferenceVisibility::Public,
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
            status: WorkQueueItemStatus::Queued,
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
            response_options: vec![attune_common::models::inquiry::InquiryResponseOption {
                r#ref: "continue".into(),
                label: "Continue".into(),
                style: attune_common::models::inquiry::InquiryResponseOptionStyle::Default,
                response: json!({}),
            }],
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
            neighbor.id,
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
                &WebSocketAuthContext::test_context(neighbor.id, TokenType::Access, vec![]),
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
    // Persisted inquiry participants need no ambient inquiry read grant.
    let participant = WebSocketAuthContext::test_context(owner.id, TokenType::Access, vec![]);
    assert!(
        notification_allowed_for_identity(&notification("inquiry", inquiry.id), &participant, pool)
            .await
    );
    assert!(
        human_delivery::allowed(
            pool,
            &participant,
            &ClientMessage::Notification(notification("inquiry", inquiry.id))
        )
        .await
    );
    let mut mismatched = notification("work_queue", queue.id);
    mismatched.notification_type = "inquiry_created".into();
    let global = WebSocketAuthContext::test_context(
        owner.id,
        TokenType::Access,
        vec![Grant {
            resource: Resource::Queues,
            actions: vec![Action::Read],
            constraints: None,
        }],
    );
    assert!(!notification_allowed_for_identity(&mismatched, &global, pool).await);
    assert!(
        !notification_allowed_for_identity(&notification("future", queue.id), &global, pool).await
    );
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
    // Private queue discovery requires management, not just ambient read.
    use attune_common::repositories::{work_queue::UpdateWorkQueueInput, Update};
    WorkQueueRepository::update(
        pool,
        queue.id,
        UpdateWorkQueueInput {
            reference_visibility: Some(ActionReferenceVisibility::Private),
            ..Default::default()
        },
    )
    .await?;
    assert!(
        !notification_allowed_for_identity(&notification("work_queue", queue.id), &global, pool)
            .await
    );
    assert!(
        !filter_allowed_for_identity(
            &SubscriptionFilter::Entity {
                entity_type: "work_queue".into(),
                entity_id: queue.id
            },
            owner.id,
            &TokenType::Access,
            &[],
            &[],
            &global.grants,
            pool
        )
        .await
    );
    let mut manager = global.clone();
    manager.grants[0].actions.push(Action::Update);
    manager.grants.push(Grant {
        resource: Resource::QueueItems,
        actions: vec![Action::Read],
        constraints: None,
    });
    assert!(
        notification_allowed_for_identity(&notification("work_queue", queue.id), &manager, pool)
            .await
    );
    assert!(
        notification_allowed_for_identity(
            &notification("work_queue_item", item.id),
            &manager,
            pool
        )
        .await
    );
    let only_items = WebSocketAuthContext::test_context(
        owner.id,
        TokenType::Access,
        vec![Grant {
            resource: Resource::QueueItems,
            actions: vec![Action::Read],
            constraints: None,
        }],
    );
    assert!(
        !notification_allowed_for_identity(
            &notification("work_queue_item", item.id),
            &only_items,
            pool
        )
        .await
    );
    use attune_common::repositories::inquiry::UpdateInquiryInput;
    InquiryRepository::update(
        pool,
        inquiry.id,
        UpdateInquiryInput {
            assigned_to: Some(neighbor.id),
            ..Default::default()
        },
    )
    .await?;
    let assignee = WebSocketAuthContext::test_context(neighbor.id, TokenType::Access, vec![]);
    assert!(
        notification_allowed_for_identity(&notification("inquiry", inquiry.id), &assignee, pool)
            .await
    );
    assert!(
        filter_allowed_for_identity(
            &SubscriptionFilter::Entity {
                entity_type: "inquiry".into(),
                entity_id: inquiry.id
            },
            neighbor.id,
            &TokenType::Access,
            &[],
            &[],
            &[],
            pool
        )
        .await
    );
    // Structural pack ownership and current declared-role membership.
    use attune_common::repositories::{
        identity::CreateIdentityRoleAssignmentInput,
        pack::{CreatePackInput, PackRepository},
        Patch,
    };
    let pack = PackRepository::create(
        pool,
        CreatePackInput {
            r#ref: "targets".into(),
            label: "Targets".into(),
            description: None,
            version: "1.0.0".into(),
            conf_schema: json!({}),
            config: json!({}),
            meta: json!({}),
            tags: vec![],
            runtime_deps: vec![],
            dependencies: vec![],
            is_standard: false,
            installers: json!({}),
        },
    )
    .await?;
    PackRepository::set_installed_by(pool, pack.id, owner.id).await?;
    WorkQueueRepository::update(
        pool,
        queue.id,
        UpdateWorkQueueInput {
            pack: Some(Patch::Set(pack.id)),
            pack_ref: Some(Patch::Set(pack.r#ref.clone())),
            is_adhoc: Some(false),
            ..Default::default()
        },
    )
    .await?;
    let owner_grant: Grant = serde_json::from_value(
        json!({"resource":"queues", "actions":["read","update"], "constraints":{"pack_owner":"self"}}),
    )?;
    let owned =
        WebSocketAuthContext::test_context(owner.id, TokenType::Access, vec![owner_grant.clone()]);
    let member = WebSocketAuthContext::test_context(
        neighbor.id,
        TokenType::Access,
        vec![owner_grant.clone()],
    );
    let queue_notification = notification("work_queue", queue.id);
    assert!(notification_allowed_for_identity(&queue_notification, &owned, pool).await);
    assert!(!notification_allowed_for_identity(&queue_notification, &member, pool).await);
    let mut connection = pool.acquire().await?;
    PackRepository::set_owner_roles(&mut connection, pack.id, vec!["target-maintainers".into()])
        .await?;
    drop(connection);
    let membership = IdentityRoleAssignmentRepository::create(
        pool,
        CreateIdentityRoleAssignmentInput {
            identity: neighbor.id,
            role: "target-maintainers".into(),
            source: "local".into(),
            managed: false,
        },
    )
    .await?;
    let member_set = PermissionSetRepository::create(
        pool,
        CreatePermissionSetInput {
            r#ref: "targets.owner".into(),
            pack: None,
            pack_ref: None,
            label: None,
            description: None,
            grants: serde_json::to_value(vec![owner_grant])?,
        },
    )
    .await?;
    PermissionAssignmentRepository::create(
        pool,
        CreatePermissionAssignmentInput {
            identity: neighbor.id,
            permset: member_set.id,
        },
    )
    .await?;
    assert!(notification_allowed_for_identity(&queue_notification, &member, pool).await);
    assert!(
        human_delivery::allowed(
            pool,
            &member,
            &ClientMessage::Notification(queue_notification.clone())
        )
        .await
    );
    IdentityRoleAssignmentRepository::delete(pool, membership.id).await?;
    assert!(
        !human_delivery::allowed(
            pool,
            &member,
            &ClientMessage::Notification(queue_notification)
        )
        .await
    );
    db.cleanup().await?;
    Ok(())
}
