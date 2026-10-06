//! Integration tests for Inquiry repository
//!
//! These tests verify CRUD operations, queries, and constraints
//! for the Inquiry repository.

mod helpers;

use attune_common::{
    models::{
        enums::InquiryStatus,
        inquiry::{InquiryResponseOption, InquiryResponseOptionStyle},
    },
    repositories::{
        inquiry::{
            CreateInquiryInput, InquiryRepository, InquirySearchFilters, UpdateInquiryInput,
        },
        Create, Delete, FindById, List, Update,
    },
    Error,
};
use chrono::{Duration, Utc};
use helpers::*;
use serde_json::json;

fn response_options(response: serde_json::Value) -> Vec<InquiryResponseOption> {
    vec![InquiryResponseOption {
        r#ref: "continue".to_string(),
        label: "Continue".to_string(),
        style: InquiryResponseOptionStyle::Default,
        response,
    }]
}

// ============================================================================
// CREATE Tests
// ============================================================================

#[tokio::test]
async fn test_create_inquiry_minimal() {
    let pool = create_test_pool().await.unwrap();

    // Create pack, action, and execution
    let pack = PackFixture::new_unique("inquiry_pack")
        .create(&pool)
        .await
        .unwrap();

    let action = ActionFixture::new_unique(pack.id, &pack.r#ref, "action")
        .create(&pool)
        .await
        .unwrap();

    // Create execution for inquiry
    use attune_common::repositories::execution::{CreateExecutionInput, ExecutionRepository};
    let execution = ExecutionRepository::create(
        &pool,
        CreateExecutionInput {
            action: Some(action.id),
            action_ref: action.r#ref.clone(),
            config: None,
            env_vars: None,
            parent: None,
            enforcement: None,
            executor: None,
            permission_set_refs: Vec::new(),
            artifact_retention_policy: None,
            artifact_retention_limit: None,
            worker_selector: None,
            worker_tolerations: None,
            worker_affinity: None,
            worker: None,
            status: attune_common::models::enums::ExecutionStatus::Requested,
            trace_tag: None,
            result: None,
            workflow_task: None,
            timeout_seconds: None,
        },
    )
    .await
    .unwrap();

    // Create inquiry with minimal fields
    let input = CreateInquiryInput {
        created_by_execution: execution.id,
        prompt: "Approve deployment?".to_string(),
        response_schema: None,
        response_options: response_options(json!({})),
        assigned_to: None,
        status: InquiryStatus::Pending,
        response: None,
        timeout_at: None,
    };

    let inquiry = InquiryRepository::create(&pool, input).await.unwrap();

    assert!(inquiry.id > 0);
    assert_eq!(inquiry.created_by_execution, execution.id);
    assert_eq!(inquiry.prompt, "Approve deployment?");
    assert_eq!(inquiry.response_schema, None);
    assert_eq!(inquiry.assigned_to, None);
    assert_eq!(inquiry.status, InquiryStatus::Pending);
    assert_eq!(inquiry.response, None);
    assert_eq!(inquiry.timeout_at, None);
    assert_eq!(inquiry.responded_at, None);
    assert!(inquiry.created.timestamp() > 0);
    assert!(inquiry.updated.timestamp() > 0);
}

#[tokio::test]
async fn test_create_inquiry_with_response_schema() {
    let pool = create_test_pool().await.unwrap();

    let pack = PackFixture::new_unique("schema_pack")
        .create(&pool)
        .await
        .unwrap();

    let action = ActionFixture::new_unique(pack.id, &pack.r#ref, "action")
        .create(&pool)
        .await
        .unwrap();

    use attune_common::repositories::execution::{CreateExecutionInput, ExecutionRepository};
    let execution = ExecutionRepository::create(
        &pool,
        CreateExecutionInput {
            action: Some(action.id),
            action_ref: action.r#ref.clone(),
            config: None,
            env_vars: None,
            parent: None,
            enforcement: None,
            executor: None,
            permission_set_refs: Vec::new(),
            artifact_retention_policy: None,
            artifact_retention_limit: None,
            worker_selector: None,
            worker_tolerations: None,
            worker_affinity: None,
            worker: None,
            status: attune_common::models::enums::ExecutionStatus::Requested,
            trace_tag: None,
            result: None,
            workflow_task: None,
            timeout_seconds: None,
        },
    )
    .await
    .unwrap();

    let response_schema = json!({
        "approved": {"type": "boolean", "required": true},
        "reason": {"type": "string"}
    });

    let input = CreateInquiryInput {
        created_by_execution: execution.id,
        prompt: "Approve this action?".to_string(),
        response_schema: Some(response_schema.clone()),
        response_options: response_options(json!({"approved": true})),
        assigned_to: None,
        status: InquiryStatus::Pending,
        response: None,
        timeout_at: None,
    };

    let inquiry = InquiryRepository::create(&pool, input).await.unwrap();

    assert_eq!(inquiry.response_schema, Some(response_schema));
}

#[tokio::test]
async fn test_create_inquiry_with_timeout() {
    let pool = create_test_pool().await.unwrap();

    let pack = PackFixture::new_unique("timeout_pack")
        .create(&pool)
        .await
        .unwrap();

    let action = ActionFixture::new_unique(pack.id, &pack.r#ref, "action")
        .create(&pool)
        .await
        .unwrap();

    use attune_common::repositories::execution::{CreateExecutionInput, ExecutionRepository};
    let execution = ExecutionRepository::create(
        &pool,
        CreateExecutionInput {
            action: Some(action.id),
            action_ref: action.r#ref.clone(),
            config: None,
            env_vars: None,
            parent: None,
            enforcement: None,
            executor: None,
            permission_set_refs: Vec::new(),
            artifact_retention_policy: None,
            artifact_retention_limit: None,
            worker_selector: None,
            worker_tolerations: None,
            worker_affinity: None,
            worker: None,
            status: attune_common::models::enums::ExecutionStatus::Requested,
            trace_tag: None,
            result: None,
            workflow_task: None,
            timeout_seconds: None,
        },
    )
    .await
    .unwrap();

    let timeout_at = Utc::now() + Duration::hours(1);

    let input = CreateInquiryInput {
        created_by_execution: execution.id,
        prompt: "Time-sensitive approval".to_string(),
        response_schema: None,
        response_options: response_options(json!({})),
        assigned_to: None,
        status: InquiryStatus::Pending,
        response: None,
        timeout_at: Some(timeout_at),
    };

    let inquiry = InquiryRepository::create(&pool, input).await.unwrap();

    assert!(inquiry.timeout_at.is_some());
    let saved_timeout = inquiry.timeout_at.unwrap();
    // Allow for small timestamp differences (within 1 second)
    assert!((saved_timeout.timestamp() - timeout_at.timestamp()).abs() < 1);
}

#[tokio::test]
async fn test_create_inquiry_with_assigned_user() {
    let pool = create_test_pool().await.unwrap();

    let pack = PackFixture::new_unique("assigned_pack")
        .create(&pool)
        .await
        .unwrap();

    let action = ActionFixture::new_unique(pack.id, &pack.r#ref, "action")
        .create(&pool)
        .await
        .unwrap();

    use attune_common::repositories::execution::{CreateExecutionInput, ExecutionRepository};
    let execution = ExecutionRepository::create(
        &pool,
        CreateExecutionInput {
            action: Some(action.id),
            action_ref: action.r#ref.clone(),
            config: None,
            env_vars: None,
            parent: None,
            enforcement: None,
            executor: None,
            permission_set_refs: Vec::new(),
            artifact_retention_policy: None,
            artifact_retention_limit: None,
            worker_selector: None,
            worker_tolerations: None,
            worker_affinity: None,
            worker: None,
            status: attune_common::models::enums::ExecutionStatus::Requested,
            trace_tag: None,
            result: None,
            workflow_task: None,
            timeout_seconds: None,
        },
    )
    .await
    .unwrap();

    // Create an identity to assign to
    use attune_common::repositories::identity::{CreateIdentityInput, IdentityRepository};
    let identity = IdentityRepository::create(
        &pool,
        CreateIdentityInput {
            login: format!("approver_{}", unique_test_id()),
            display_name: Some("Approver User".to_string()),
            attributes: json!({"email": format!("approver_{}@example.com", unique_test_id())}),
            password_hash: None,
        },
    )
    .await
    .unwrap();

    let input = CreateInquiryInput {
        created_by_execution: execution.id,
        prompt: "Review and approve".to_string(),
        response_schema: None,
        response_options: response_options(json!({})),
        assigned_to: Some(identity.id),
        status: InquiryStatus::Pending,
        response: None,
        timeout_at: None,
    };

    let inquiry = InquiryRepository::create(&pool, input).await.unwrap();

    assert_eq!(inquiry.assigned_to, Some(identity.id));
}

#[tokio::test]
async fn test_create_inquiry_allows_dangling_execution_reference() {
    let pool = create_test_pool().await.unwrap();

    // Try to create inquiry with non-existent execution ID
    let input = CreateInquiryInput {
        created_by_execution: 99999,
        prompt: "Test prompt".to_string(),
        response_schema: None,
        response_options: response_options(json!({})),
        assigned_to: None,
        status: InquiryStatus::Pending,
        response: None,
        timeout_at: None,
    };

    let inquiry = InquiryRepository::create(&pool, input).await.unwrap();

    assert_eq!(inquiry.created_by_execution, 99999);
}

#[tokio::test]
async fn test_workflow_inquiry_idempotency_response_and_wait_release() {
    use attune_common::{
        models::{
            enums::{ExecutionStatus, WorkflowTaskWaitState},
            execution::WorkflowTaskMetadata,
            workflow::WorkflowTaskWaitTarget,
            ActionReferenceVisibility, WorkQueueBatchMode, WorkQueueItemStatus,
            WorkQueueUpdateStrategy,
        },
        repositories::{
            execution::{CreateExecutionInput, ExecutionRepository},
            inquiry::CreateWorkflowInquiryInput,
            work_queue::{
                CreateWorkQueueInput, CreateWorkQueueItemInput, WorkQueueItemRepository,
                WorkQueueRepository,
            },
            workflow::{
                CreateWorkflowDefinitionInput, CreateWorkflowExecutionInput,
                WorkflowDefinitionRepository, WorkflowExecutionRepository,
            },
            workflow_task_wait::{CreateWorkflowTaskWaitInput, WorkflowTaskWaitRepository},
        },
    };

    let pool = create_test_pool().await.unwrap();
    let pack = PackFixture::new_unique("workflow_inquiry")
        .create(&pool)
        .await
        .unwrap();
    let workflow_action = ActionFixture::new_unique(pack.id, &pack.r#ref, "workflow")
        .create(&pool)
        .await
        .unwrap();
    let creator_action = ActionFixture::new_unique(pack.id, &pack.r#ref, "request_approval")
        .create(&pool)
        .await
        .unwrap();
    let parent = ExecutionRepository::create(
        &pool,
        CreateExecutionInput {
            action: Some(workflow_action.id),
            action_ref: workflow_action.r#ref.clone(),
            status: ExecutionStatus::Running,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let definition = WorkflowDefinitionRepository::create(
        &pool,
        CreateWorkflowDefinitionInput {
            r#ref: format!("{}.approval", pack.r#ref),
            pack: pack.id,
            pack_ref: pack.r#ref.clone(),
            label: "Approval".to_string(),
            description: None,
            version: "1.0.0".to_string(),
            param_schema: None,
            out_schema: None,
            definition: json!({}),
            tags: Vec::new(),
        },
    )
    .await
    .unwrap();
    let workflow = WorkflowExecutionRepository::create(
        &pool,
        CreateWorkflowExecutionInput {
            execution: parent.id,
            workflow_def: definition.id,
            task_graph: json!({}),
            variables: json!({}),
            status: ExecutionStatus::Running,
        },
    )
    .await
    .unwrap();
    let creator = ExecutionRepository::create(
        &pool,
        CreateExecutionInput {
            action: Some(creator_action.id),
            action_ref: creator_action.r#ref,
            parent: Some(parent.id),
            status: ExecutionStatus::Completed,
            workflow_task: Some(WorkflowTaskMetadata {
                workflow_execution: workflow.id,
                task_name: "request_approval".to_string(),
                triggered_by: None,
                task_index: None,
                task_batch: None,
                retry_count: 0,
                max_retries: 2,
                next_retry_at: None,
                timeout_seconds: None,
                timed_out: false,
                duration_ms: Some(1),
                started_at: Some(Utc::now()),
                completed_at: Some(Utc::now()),
            }),
            ..Default::default()
        },
    )
    .await
    .unwrap();

    let assignee = attune_common::repositories::identity::IdentityRepository::create(
        &pool,
        attune_common::repositories::identity::CreateIdentityInput {
            login: format!("assignee_{}", unique_test_id()),
            display_name: Some("Approver".to_string()),
            attributes: json!({}),
            password_hash: None,
        },
    )
    .await
    .unwrap();

    let create_input = CreateWorkflowInquiryInput {
        created_by_execution: creator.id,
        purpose: "approval".to_string(),
        prompt: "Approve deployment?".to_string(),
        response_schema: Some(json!({"approved": {"type": "boolean", "required": true}})),
        response_options: vec![InquiryResponseOption {
            r#ref: "approve".to_string(),
            label: "Approve".to_string(),
            style: InquiryResponseOptionStyle::Positive,
            response: json!({"approved": true}),
        }],
        assigned_to: Some(assignee.id),
        timeout_seconds: Some(3600),
    };
    let mut conn = pool.acquire().await.unwrap();
    let inquiry =
        InquiryRepository::create_workflow_inquiry_idempotent(&mut conn, create_input.clone())
            .await
            .unwrap();
    let duplicate =
        InquiryRepository::create_workflow_inquiry_idempotent(&mut conn, create_input.clone())
            .await
            .unwrap();
    assert_eq!(duplicate.id, inquiry.id);
    assert_eq!(inquiry.workflow_execution, Some(workflow.id));
    assert_eq!(
        inquiry.workflow_task_name.as_deref(),
        Some("request_approval")
    );
    assert_eq!(inquiry.action_attempt_family, Some(creator.id));

    let conflicting = InquiryRepository::create_workflow_inquiry_idempotent(
        &mut conn,
        CreateWorkflowInquiryInput {
            prompt: "Different prompt".to_string(),
            ..create_input.clone()
        },
    )
    .await;
    assert!(matches!(conflicting, Err(Error::AlreadyExists { .. })));

    let conflicting = InquiryRepository::create_workflow_inquiry_idempotent(
        &mut conn,
        CreateWorkflowInquiryInput {
            response_options: vec![InquiryResponseOption {
                r#ref: "reject".to_string(),
                label: "Reject".to_string(),
                style: InquiryResponseOptionStyle::Destructive,
                response: json!({"approved": false}),
            }],
            ..create_input
        },
    )
    .await;
    assert!(matches!(conflicting, Err(Error::AlreadyExists { .. })));

    let wait = WorkflowTaskWaitRepository::create_or_get(
        &mut conn,
        CreateWorkflowTaskWaitInput {
            workflow_execution: workflow.id,
            task_name: "deploy".to_string(),
            target: WorkflowTaskWaitTarget::Inquiry(inquiry.id),
        },
    )
    .await
    .unwrap();
    assert_eq!(wait.state, WorkflowTaskWaitState::Waiting);
    assert_eq!(
        wait.target().unwrap(),
        WorkflowTaskWaitTarget::Inquiry(inquiry.id)
    );
    assert!(
        ExecutionRepository::is_in_execution_tree(&pool, parent.id, parent.id, true)
            .await
            .unwrap()
    );
    assert!(
        !ExecutionRepository::is_in_execution_tree(&pool, parent.id, parent.id, false)
            .await
            .unwrap()
    );
    assert!(
        ExecutionRepository::is_in_execution_tree(&pool, parent.id, creator.id, false)
            .await
            .unwrap()
    );
    let waits = WorkflowTaskWaitRepository::list_by_execution(&pool, parent.id)
        .await
        .unwrap();
    assert_eq!(waits.len(), 1);
    assert_eq!(waits[0].id, wait.id);
    assert!(
        WorkflowTaskWaitRepository::list_by_execution(&pool, creator.id)
            .await
            .unwrap()
            .is_empty()
    );

    let identity = attune_common::repositories::identity::IdentityRepository::create(
        &pool,
        attune_common::repositories::identity::CreateIdentityInput {
            login: format!("responder_{}", unique_test_id()),
            display_name: Some("Responder".to_string()),
            attributes: json!({}),
            password_hash: None,
        },
    )
    .await
    .unwrap();
    let responded = InquiryRepository::respond_pending(
        &pool,
        inquiry.id,
        json!({"approved": true}),
        identity.id,
        Some(json!({"provider": "test", "actor": "external-1"})),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(responded.status, InquiryStatus::Responded);
    assert_eq!(responded.responded_by, Some(identity.id));

    let contexts = InquiryRepository::find_contexts_by_ids(&pool, &[inquiry.id])
        .await
        .unwrap();
    assert_eq!(contexts.len(), 1);
    assert_eq!(contexts[0].workflow_root_execution_id, Some(parent.id));
    assert_eq!(
        contexts[0].assigned_to_login.as_deref(),
        Some(assignee.login.as_str())
    );
    assert_eq!(
        contexts[0].assigned_to_display_name.as_deref(),
        Some("Approver")
    );
    assert_eq!(
        contexts[0].responded_by_login.as_deref(),
        Some(identity.login.as_str())
    );
    assert_eq!(
        contexts[0].responded_by_display_name.as_deref(),
        Some("Responder")
    );

    for filters in [
        InquirySearchFilters {
            workflow_action_ref: Some(workflow_action.r#ref.clone()),
            limit: 10,
            ..Default::default()
        },
        InquirySearchFilters {
            workflow_pack_ref: Some(pack.r#ref.clone()),
            limit: 10,
            ..Default::default()
        },
    ] {
        let result = InquiryRepository::search(&pool, &filters).await.unwrap();
        assert_eq!(result.total, 1);
        assert_eq!(result.rows[0].id, inquiry.id);
    }
    let result = InquiryRepository::search(
        &pool,
        &InquirySearchFilters {
            workflow_action_ref: Some(format!("{}.other", pack.r#ref)),
            limit: 10,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(result.total, 0);

    assert!(InquiryRepository::respond_pending(
        &pool,
        inquiry.id,
        json!({"approved": false}),
        identity.id,
        None,
    )
    .await
    .unwrap()
    .is_none());

    let released = WorkflowTaskWaitRepository::transition_waiting(
        &pool,
        wait.id,
        WorkflowTaskWaitState::Released,
        None,
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(released.state, WorkflowTaskWaitState::Released);
    assert!(released.resolved_at.is_some());
    assert!(released.released_at.is_some());

    ExecutionRepository::create(
        &pool,
        CreateExecutionInput {
            action_ref: "test.deploy".to_string(),
            parent: Some(parent.id),
            status: ExecutionStatus::Requested,
            workflow_task: Some(WorkflowTaskMetadata {
                workflow_execution: workflow.id,
                task_name: "deploy".to_string(),
                triggered_by: None,
                task_index: None,
                task_batch: None,
                retry_count: 0,
                max_retries: 0,
                next_retry_at: None,
                timeout_seconds: None,
                timed_out: false,
                duration_ms: None,
                started_at: None,
                completed_at: None,
            }),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert!(WorkflowTaskWaitRepository::find_resolvable(&pool, 100)
        .await
        .unwrap()
        .iter()
        .any(|candidate| candidate.id == wait.id));

    let execution_wait = WorkflowTaskWaitRepository::create_or_get(
        &mut conn,
        CreateWorkflowTaskWaitInput {
            workflow_execution: workflow.id,
            task_name: "after_request".to_string(),
            target: WorkflowTaskWaitTarget::Execution(creator.id),
        },
    )
    .await
    .unwrap();
    assert_eq!(
        execution_wait.target().unwrap(),
        WorkflowTaskWaitTarget::Execution(creator.id)
    );
    assert!(matches!(
        WorkflowTaskWaitRepository::create_or_get(
            &mut conn,
            CreateWorkflowTaskWaitInput {
                workflow_execution: workflow.id,
                task_name: "after_request".to_string(),
                target: WorkflowTaskWaitTarget::Inquiry(inquiry.id),
            },
        )
        .await,
        Err(Error::InvalidState(_))
    ));
    assert!(WorkflowTaskWaitRepository::find_reconcilable_by_target(
        &pool,
        WorkflowTaskWaitTarget::Execution(creator.id),
    )
    .await
    .unwrap()
    .iter()
    .any(|candidate| candidate.id == execution_wait.id));
    assert!(WorkflowTaskWaitRepository::find_resolvable(&pool, 100)
        .await
        .unwrap()
        .iter()
        .any(|candidate| candidate.id == execution_wait.id));
    assert!(matches!(
        ExecutionRepository::delete(&pool, creator.id).await,
        Err(Error::InvalidState(_))
    ));

    let disappearing_execution = ExecutionRepository::create(
        &pool,
        CreateExecutionInput {
            action_ref: "test.disappearing".to_string(),
            parent: Some(parent.id),
            status: ExecutionStatus::Completed,
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let disappearing_wait = WorkflowTaskWaitRepository::create_or_get(
        &mut conn,
        CreateWorkflowTaskWaitInput {
            workflow_execution: workflow.id,
            task_name: "after_disappearing_execution".to_string(),
            target: WorkflowTaskWaitTarget::Execution(disappearing_execution.id),
        },
    )
    .await
    .unwrap();
    sqlx::query("DELETE FROM execution WHERE id = $1")
        .bind(disappearing_execution.id)
        .execute(&pool)
        .await
        .unwrap();
    assert!(WorkflowTaskWaitRepository::find_resolvable(&pool, 100)
        .await
        .unwrap()
        .iter()
        .any(|candidate| candidate.id == disappearing_wait.id));

    let queue = WorkQueueRepository::create(
        &pool,
        CreateWorkQueueInput {
            r#ref: format!("{}.wait-target", pack.r#ref),
            pack: Some(pack.id),
            pack_ref: Some(pack.r#ref.clone()),
            is_adhoc: false,
            label: "Wait target".to_string(),
            description: None,
            enabled: true,
            accepting_new_items: true,
            dispatch_action: None,
            dispatch_action_ref: "test.dispatch".to_string(),
            default_priority: 0,
            allow_pending_update: false,
            update_strategy: WorkQueueUpdateStrategy::Replace,
            batch_mode: WorkQueueBatchMode::Single,
            item_schema: json!({}),
            action_params: json!({}),
            trace_tag_template: None,
            permission_set_refs: None,
            config: json!({}),
            reference_visibility: ActionReferenceVisibility::Public,
            reference_allowed_pack_refs: Vec::new(),
        },
    )
    .await
    .unwrap();
    let queue_item = WorkQueueItemRepository::create(
        &pool,
        CreateWorkQueueItemInput {
            queue: queue.id,
            queue_ref: queue.r#ref.clone(),
            item_key: None,
            priority: 0,
            status: WorkQueueItemStatus::Skipped,
            payload: json!({}),
            metadata: json!({}),
            trace_tag: None,
            enqueue_source: "test".to_string(),
            requested_by_identity: None,
            requested_by_execution: Some(parent.id),
            requested_by_enforcement: None,
            leased_execution: None,
            lease_token: None,
            lease_expires_at: None,
            attempt_count: 0,
            last_error: None,
            ack_summary: None,
        },
    )
    .await
    .unwrap();
    let queue_wait = WorkflowTaskWaitRepository::create_or_get(
        &mut conn,
        CreateWorkflowTaskWaitInput {
            workflow_execution: workflow.id,
            task_name: "after_queue".to_string(),
            target: WorkflowTaskWaitTarget::WorkQueueItem(queue_item.id),
        },
    )
    .await
    .unwrap();
    assert!(WorkflowTaskWaitRepository::find_resolvable(&pool, 100)
        .await
        .unwrap()
        .iter()
        .any(|candidate| candidate.id == queue_wait.id));
    assert!(matches!(
        WorkQueueItemRepository::delete(&pool, queue_item.id).await,
        Err(Error::InvalidState(_))
    ));
    assert!(matches!(
        WorkQueueItemRepository::delete_if_statuses(
            &pool,
            queue_item.id,
            &[WorkQueueItemStatus::Skipped],
        )
        .await,
        Err(Error::InvalidState(_))
    ));
    assert!(matches!(
        WorkQueueRepository::delete(&pool, queue.id).await,
        Err(Error::InvalidState(_))
    ));
    WorkflowTaskWaitRepository::transition_waiting(
        &pool,
        queue_wait.id,
        WorkflowTaskWaitState::Failed,
        Some(json!({"code": "queue_item_failed"})),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(WorkQueueItemRepository::delete(&pool, queue_item.id)
        .await
        .unwrap());
    assert!(WorkQueueRepository::delete(&pool, queue.id).await.unwrap());

    let terminal_wait = WorkflowTaskWaitRepository::create_or_get(
        &mut conn,
        CreateWorkflowTaskWaitInput {
            workflow_execution: workflow.id,
            task_name: "verify_timeout_delivery".to_string(),
            target: WorkflowTaskWaitTarget::Inquiry(inquiry.id),
        },
    )
    .await
    .unwrap();
    WorkflowTaskWaitRepository::transition_waiting(
        &pool,
        terminal_wait.id,
        WorkflowTaskWaitState::TimedOut,
        Some(json!({"code": "inquiry_timeout"})),
    )
    .await
    .unwrap()
    .unwrap();
    let resolvable = WorkflowTaskWaitRepository::find_resolvable(&pool, 100)
        .await
        .unwrap();
    assert!(resolvable.iter().any(|wait| wait.id == terminal_wait.id));
    assert!(
        WorkflowTaskWaitRepository::mark_terminal_delivery_complete(&pool, terminal_wait.id)
            .await
            .unwrap()
    );
    let resolvable = WorkflowTaskWaitRepository::find_resolvable(&pool, 100)
        .await
        .unwrap();
    assert!(!resolvable.iter().any(|wait| wait.id == terminal_wait.id));

    let pending_inquiry = InquiryRepository::create_workflow_inquiry_idempotent(
        &mut conn,
        CreateWorkflowInquiryInput {
            created_by_execution: creator.id,
            purpose: "cancellation".to_string(),
            prompt: "Cancel this inquiry".to_string(),
            response_schema: None,
            response_options: response_options(json!({})),
            assigned_to: None,
            timeout_seconds: None,
        },
    )
    .await
    .unwrap();
    let pending_wait = WorkflowTaskWaitRepository::create_or_get(
        &mut conn,
        CreateWorkflowTaskWaitInput {
            workflow_execution: workflow.id,
            task_name: "cancelled_task".to_string(),
            target: WorkflowTaskWaitTarget::Inquiry(pending_inquiry.id),
        },
    )
    .await
    .unwrap();
    drop(conn);

    let cancelled_workflow = WorkflowExecutionRepository::cancel_with_prerequisites(
        &pool,
        workflow.id,
        "cancelled by test",
        Some((ExecutionStatus::Canceling, None)),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(cancelled_workflow.status, ExecutionStatus::Cancelled);
    assert_eq!(
        ExecutionRepository::find_by_id(&pool, parent.id)
            .await
            .unwrap()
            .unwrap()
            .status,
        ExecutionStatus::Canceling
    );
    assert_eq!(
        InquiryRepository::find_by_id(&pool, pending_inquiry.id)
            .await
            .unwrap()
            .unwrap()
            .status,
        InquiryStatus::Cancelled
    );
    assert_eq!(
        WorkflowTaskWaitRepository::find_by_workflow_task(
            &pool,
            workflow.id,
            &pending_wait.task_name,
        )
        .await
        .unwrap()
        .unwrap()
        .state,
        WorkflowTaskWaitState::Cancelled
    );
}

// ============================================================================
// READ Tests
// ============================================================================

#[tokio::test]
async fn test_find_inquiry_by_id() {
    let pool = create_test_pool().await.unwrap();

    let pack = PackFixture::new_unique("find_pack")
        .create(&pool)
        .await
        .unwrap();

    let action = ActionFixture::new_unique(pack.id, &pack.r#ref, "action")
        .create(&pool)
        .await
        .unwrap();

    use attune_common::repositories::execution::{CreateExecutionInput, ExecutionRepository};
    let execution = ExecutionRepository::create(
        &pool,
        CreateExecutionInput {
            action: Some(action.id),
            action_ref: action.r#ref.clone(),
            config: None,
            env_vars: None,
            parent: None,
            enforcement: None,
            executor: None,
            permission_set_refs: Vec::new(),
            artifact_retention_policy: None,
            artifact_retention_limit: None,
            worker_selector: None,
            worker_tolerations: None,
            worker_affinity: None,
            worker: None,
            status: attune_common::models::enums::ExecutionStatus::Requested,
            trace_tag: None,
            result: None,
            workflow_task: None,
            timeout_seconds: None,
        },
    )
    .await
    .unwrap();

    let created_inquiry = InquiryFixture::new_unique(execution.id, "Find me")
        .with_response_schema(json!({"approved": {"type": "boolean", "required": true}}))
        .with_response_options(response_options(json!({"approved": true})))
        .create(&pool)
        .await
        .unwrap();

    let found = InquiryRepository::find_by_id(&pool, created_inquiry.id)
        .await
        .unwrap();

    assert!(found.is_some());
    let inquiry = found.unwrap();
    assert_eq!(inquiry.id, created_inquiry.id);
    assert_eq!(
        inquiry.created_by_execution,
        created_inquiry.created_by_execution
    );
    assert_eq!(inquiry.prompt, created_inquiry.prompt);
    assert_eq!(inquiry.status, created_inquiry.status);
}

#[tokio::test]
async fn test_find_inquiry_by_id_not_found() {
    let pool = create_test_pool().await.unwrap();

    let result = InquiryRepository::find_by_id(&pool, 99999).await.unwrap();

    assert!(result.is_none());
}

#[tokio::test]
async fn test_get_inquiry_by_id() {
    let pool = create_test_pool().await.unwrap();

    let pack = PackFixture::new_unique("get_pack")
        .create(&pool)
        .await
        .unwrap();

    let action = ActionFixture::new_unique(pack.id, &pack.r#ref, "action")
        .create(&pool)
        .await
        .unwrap();

    use attune_common::repositories::execution::{CreateExecutionInput, ExecutionRepository};
    let execution = ExecutionRepository::create(
        &pool,
        CreateExecutionInput {
            action: Some(action.id),
            action_ref: action.r#ref.clone(),
            config: None,
            env_vars: None,
            parent: None,
            enforcement: None,
            executor: None,
            permission_set_refs: Vec::new(),
            artifact_retention_policy: None,
            artifact_retention_limit: None,
            worker_selector: None,
            worker_tolerations: None,
            worker_affinity: None,
            worker: None,
            status: attune_common::models::enums::ExecutionStatus::Requested,
            trace_tag: None,
            result: None,
            workflow_task: None,
            timeout_seconds: None,
        },
    )
    .await
    .unwrap();

    let created_inquiry = InquiryFixture::new_unique(execution.id, "Get me")
        .create(&pool)
        .await
        .unwrap();

    let inquiry = InquiryRepository::get_by_id(&pool, created_inquiry.id)
        .await
        .unwrap();

    assert_eq!(inquiry.id, created_inquiry.id);
}

#[tokio::test]
async fn test_get_inquiry_by_id_not_found() {
    let pool = create_test_pool().await.unwrap();

    let result = InquiryRepository::get_by_id(&pool, 99999).await;

    assert!(result.is_err());
    assert!(matches!(result.unwrap_err(), Error::NotFound { .. }));
}

// ============================================================================
// LIST Tests
// ============================================================================

#[tokio::test]
async fn test_list_inquiries_empty() {
    let pool = create_test_pool().await.unwrap();

    let inquiries = InquiryRepository::list(&pool).await.unwrap();
    // May have inquiries from other tests, just verify we can list without error
    drop(inquiries);
}

#[tokio::test]
async fn test_list_inquiries() {
    let pool = create_test_pool().await.unwrap();

    let pack = PackFixture::new_unique("list_pack")
        .create(&pool)
        .await
        .unwrap();

    let action = ActionFixture::new_unique(pack.id, &pack.r#ref, "action")
        .create(&pool)
        .await
        .unwrap();

    use attune_common::repositories::execution::{CreateExecutionInput, ExecutionRepository};
    let _execution = ExecutionRepository::create(
        &pool,
        CreateExecutionInput {
            action: Some(action.id),
            action_ref: action.r#ref.clone(),
            config: None,
            env_vars: None,
            parent: None,
            enforcement: None,
            executor: None,
            permission_set_refs: Vec::new(),
            artifact_retention_policy: None,
            artifact_retention_limit: None,
            worker_selector: None,
            worker_tolerations: None,
            worker_affinity: None,
            worker: None,
            status: attune_common::models::enums::ExecutionStatus::Requested,
            trace_tag: None,
            result: None,
            workflow_task: None,
            timeout_seconds: None,
        },
    )
    .await
    .unwrap();

    let before_count = InquiryRepository::list(&pool).await.unwrap().len();

    // Create multiple inquiries on distinct executions.
    let mut created_ids = vec![];
    for i in 0..3 {
        let execution = ExecutionRepository::create(
            &pool,
            CreateExecutionInput {
                action: Some(action.id),
                action_ref: action.r#ref.clone(),
                config: None,
                env_vars: None,
                parent: None,
                enforcement: None,
                executor: None,
                permission_set_refs: Vec::new(),
                artifact_retention_policy: None,
                artifact_retention_limit: None,
                worker_selector: None,
                worker_tolerations: None,
                worker_affinity: None,
                worker: None,
                status: attune_common::models::enums::ExecutionStatus::Requested,
                trace_tag: None,
                result: None,
                workflow_task: None,
                timeout_seconds: None,
            },
        )
        .await
        .unwrap();

        let inquiry = InquiryFixture::new_unique(execution.id, &format!("Inquiry {}", i))
            .create(&pool)
            .await
            .unwrap();
        created_ids.push(inquiry.id);
    }

    let inquiries = InquiryRepository::list(&pool).await.unwrap();

    assert!(inquiries.len() >= before_count + 3);
    // Verify our inquiries are in the list
    let our_inquiries: Vec<_> = inquiries
        .iter()
        .filter(|i| created_ids.contains(&i.id))
        .collect();
    assert_eq!(our_inquiries.len(), 3);
}

// ============================================================================
// UPDATE Tests
// ============================================================================

#[tokio::test]
async fn test_update_inquiry_status() {
    let pool = create_test_pool().await.unwrap();

    let pack = PackFixture::new_unique("update_pack")
        .create(&pool)
        .await
        .unwrap();

    let action = ActionFixture::new_unique(pack.id, &pack.r#ref, "action")
        .create(&pool)
        .await
        .unwrap();

    use attune_common::repositories::execution::{CreateExecutionInput, ExecutionRepository};
    let execution = ExecutionRepository::create(
        &pool,
        CreateExecutionInput {
            action: Some(action.id),
            action_ref: action.r#ref.clone(),
            config: None,
            env_vars: None,
            parent: None,
            enforcement: None,
            executor: None,
            permission_set_refs: Vec::new(),
            artifact_retention_policy: None,
            artifact_retention_limit: None,
            worker_selector: None,
            worker_tolerations: None,
            worker_affinity: None,
            worker: None,
            status: attune_common::models::enums::ExecutionStatus::Requested,
            trace_tag: None,
            result: None,
            workflow_task: None,
            timeout_seconds: None,
        },
    )
    .await
    .unwrap();

    let inquiry = InquiryFixture::new_unique(execution.id, "Update status")
        .with_status(InquiryStatus::Pending)
        .create(&pool)
        .await
        .unwrap();

    let input = UpdateInquiryInput {
        status: Some(InquiryStatus::Responded),
        response: None,
        responded_at: None,
        assigned_to: None,
    };

    let updated = InquiryRepository::update(&pool, inquiry.id, input)
        .await
        .unwrap();

    assert_eq!(updated.id, inquiry.id);
    assert_eq!(updated.status, InquiryStatus::Responded);
    assert!(updated.updated > inquiry.updated);
}

#[tokio::test]
async fn test_update_inquiry_status_transitions() {
    let pool = create_test_pool().await.unwrap();

    let pack = PackFixture::new_unique("transitions_pack")
        .create(&pool)
        .await
        .unwrap();

    let action = ActionFixture::new_unique(pack.id, &pack.r#ref, "action")
        .create(&pool)
        .await
        .unwrap();

    use attune_common::repositories::execution::{CreateExecutionInput, ExecutionRepository};
    let execution = ExecutionRepository::create(
        &pool,
        CreateExecutionInput {
            action: Some(action.id),
            action_ref: action.r#ref.clone(),
            config: None,
            env_vars: None,
            parent: None,
            enforcement: None,
            executor: None,
            permission_set_refs: Vec::new(),
            artifact_retention_policy: None,
            artifact_retention_limit: None,
            worker_selector: None,
            worker_tolerations: None,
            worker_affinity: None,
            worker: None,
            status: attune_common::models::enums::ExecutionStatus::Requested,
            trace_tag: None,
            result: None,
            workflow_task: None,
            timeout_seconds: None,
        },
    )
    .await
    .unwrap();

    let inquiry = InquiryFixture::new_unique(execution.id, "Transitions")
        .create(&pool)
        .await
        .unwrap();

    // Test status transitions: Pending -> Responded
    let updated = InquiryRepository::update(
        &pool,
        inquiry.id,
        UpdateInquiryInput {
            status: Some(InquiryStatus::Responded),
            response: None,
            responded_at: None,
            assigned_to: None,
        },
    )
    .await
    .unwrap();
    assert_eq!(updated.status, InquiryStatus::Responded);

    // Test status transition: Responded -> Cancelled (although unusual)
    let updated = InquiryRepository::update(
        &pool,
        inquiry.id,
        UpdateInquiryInput {
            status: Some(InquiryStatus::Cancelled),
            response: None,
            responded_at: None,
            assigned_to: None,
        },
    )
    .await
    .unwrap();
    assert_eq!(updated.status, InquiryStatus::Cancelled);

    // Test Timeout status
    let updated = InquiryRepository::update(
        &pool,
        inquiry.id,
        UpdateInquiryInput {
            status: Some(InquiryStatus::Timeout),
            response: None,
            responded_at: None,
            assigned_to: None,
        },
    )
    .await
    .unwrap();
    assert_eq!(updated.status, InquiryStatus::Timeout);
}

#[tokio::test]
async fn test_update_inquiry_response() {
    let pool = create_test_pool().await.unwrap();

    let pack = PackFixture::new_unique("response_pack")
        .create(&pool)
        .await
        .unwrap();

    let action = ActionFixture::new_unique(pack.id, &pack.r#ref, "action")
        .create(&pool)
        .await
        .unwrap();

    use attune_common::repositories::execution::{CreateExecutionInput, ExecutionRepository};
    let execution = ExecutionRepository::create(
        &pool,
        CreateExecutionInput {
            action: Some(action.id),
            action_ref: action.r#ref.clone(),
            config: None,
            env_vars: None,
            parent: None,
            enforcement: None,
            executor: None,
            permission_set_refs: Vec::new(),
            artifact_retention_policy: None,
            artifact_retention_limit: None,
            worker_selector: None,
            worker_tolerations: None,
            worker_affinity: None,
            worker: None,
            status: attune_common::models::enums::ExecutionStatus::Requested,
            trace_tag: None,
            result: None,
            workflow_task: None,
            timeout_seconds: None,
        },
    )
    .await
    .unwrap();

    let inquiry = InquiryFixture::new_unique(execution.id, "Get response")
        .create(&pool)
        .await
        .unwrap();

    let response = json!({
        "approved": true,
        "reason": "Looks good to me"
    });

    let input = UpdateInquiryInput {
        status: None,
        response: Some(response.clone()),
        responded_at: None,
        assigned_to: None,
    };

    let updated = InquiryRepository::update(&pool, inquiry.id, input)
        .await
        .unwrap();

    assert_eq!(updated.response, Some(response));
}

#[tokio::test]
async fn test_update_inquiry_with_response_and_status() {
    let pool = create_test_pool().await.unwrap();

    let pack = PackFixture::new_unique("both_pack")
        .create(&pool)
        .await
        .unwrap();

    let action = ActionFixture::new_unique(pack.id, &pack.r#ref, "action")
        .create(&pool)
        .await
        .unwrap();

    use attune_common::repositories::execution::{CreateExecutionInput, ExecutionRepository};
    let execution = ExecutionRepository::create(
        &pool,
        CreateExecutionInput {
            action: Some(action.id),
            action_ref: action.r#ref.clone(),
            config: None,
            env_vars: None,
            parent: None,
            enforcement: None,
            executor: None,
            permission_set_refs: Vec::new(),
            artifact_retention_policy: None,
            artifact_retention_limit: None,
            worker_selector: None,
            worker_tolerations: None,
            worker_affinity: None,
            worker: None,
            status: attune_common::models::enums::ExecutionStatus::Requested,
            trace_tag: None,
            result: None,
            workflow_task: None,
            timeout_seconds: None,
        },
    )
    .await
    .unwrap();

    let inquiry = InquiryFixture::new_unique(execution.id, "Complete")
        .create(&pool)
        .await
        .unwrap();

    let response = json!({"decision": "approved"});
    let responded_at = Utc::now();

    let input = UpdateInquiryInput {
        status: Some(InquiryStatus::Responded),
        response: Some(response.clone()),
        responded_at: Some(responded_at),
        assigned_to: None,
    };

    let updated = InquiryRepository::update(&pool, inquiry.id, input)
        .await
        .unwrap();

    assert_eq!(updated.status, InquiryStatus::Responded);
    assert_eq!(updated.response, Some(response));
    assert!(updated.responded_at.is_some());
}

#[tokio::test]
async fn test_update_inquiry_assignment() {
    let pool = create_test_pool().await.unwrap();

    let pack = PackFixture::new_unique("assign_pack")
        .create(&pool)
        .await
        .unwrap();

    let action = ActionFixture::new_unique(pack.id, &pack.r#ref, "action")
        .create(&pool)
        .await
        .unwrap();

    use attune_common::repositories::execution::{CreateExecutionInput, ExecutionRepository};
    let execution = ExecutionRepository::create(
        &pool,
        CreateExecutionInput {
            action: Some(action.id),
            action_ref: action.r#ref.clone(),
            config: None,
            env_vars: None,
            parent: None,
            enforcement: None,
            executor: None,
            permission_set_refs: Vec::new(),
            artifact_retention_policy: None,
            artifact_retention_limit: None,
            worker_selector: None,
            worker_tolerations: None,
            worker_affinity: None,
            worker: None,
            status: attune_common::models::enums::ExecutionStatus::Requested,
            trace_tag: None,
            result: None,
            workflow_task: None,
            timeout_seconds: None,
        },
    )
    .await
    .unwrap();

    let inquiry = InquiryFixture::new_unique(execution.id, "Reassign")
        .create(&pool)
        .await
        .unwrap();

    // Create an identity to assign to
    use attune_common::repositories::identity::{CreateIdentityInput, IdentityRepository};
    let identity = IdentityRepository::create(
        &pool,
        CreateIdentityInput {
            login: format!("new_approver_{}", unique_test_id()),
            display_name: Some("New Approver".to_string()),
            password_hash: None,
            attributes: json!({"email": format!("new_approver_{}@example.com", unique_test_id())}),
        },
    )
    .await
    .unwrap();

    let input = UpdateInquiryInput {
        status: None,
        response: None,
        responded_at: None,
        assigned_to: Some(identity.id),
    };

    let updated = InquiryRepository::update(&pool, inquiry.id, input)
        .await
        .unwrap();

    assert_eq!(updated.assigned_to, Some(identity.id));
}

#[tokio::test]
async fn test_update_inquiry_no_changes() {
    let pool = create_test_pool().await.unwrap();

    let pack = PackFixture::new_unique("nochange_pack")
        .create(&pool)
        .await
        .unwrap();

    let action = ActionFixture::new_unique(pack.id, &pack.r#ref, "action")
        .create(&pool)
        .await
        .unwrap();

    use attune_common::repositories::execution::{CreateExecutionInput, ExecutionRepository};
    let execution = ExecutionRepository::create(
        &pool,
        CreateExecutionInput {
            action: Some(action.id),
            action_ref: action.r#ref.clone(),
            config: None,
            env_vars: None,
            parent: None,
            enforcement: None,
            executor: None,
            permission_set_refs: Vec::new(),
            artifact_retention_policy: None,
            artifact_retention_limit: None,
            worker_selector: None,
            worker_tolerations: None,
            worker_affinity: None,
            worker: None,
            status: attune_common::models::enums::ExecutionStatus::Requested,
            trace_tag: None,
            result: None,
            workflow_task: None,
            timeout_seconds: None,
        },
    )
    .await
    .unwrap();

    let inquiry = InquiryFixture::new_unique(execution.id, "No change")
        .create(&pool)
        .await
        .unwrap();

    let input = UpdateInquiryInput {
        status: None,
        response: None,
        responded_at: None,
        assigned_to: None,
    };

    let result = InquiryRepository::update(&pool, inquiry.id, input)
        .await
        .unwrap();

    // Should return existing inquiry without updating
    assert_eq!(result.id, inquiry.id);
    assert_eq!(result.status, inquiry.status);
}

#[tokio::test]
async fn test_update_inquiry_not_found() {
    let pool = create_test_pool().await.unwrap();

    let input = UpdateInquiryInput {
        status: Some(InquiryStatus::Responded),
        response: None,
        responded_at: None,
        assigned_to: None,
    };

    let result = InquiryRepository::update(&pool, 99999, input).await;

    // When updating non-existent entity with changes, SQLx returns RowNotFound error
    assert!(result.is_err());
}

// ============================================================================
// DELETE Tests
// ============================================================================

#[tokio::test]
async fn test_delete_inquiry() {
    let pool = create_test_pool().await.unwrap();

    let pack = PackFixture::new_unique("delete_pack")
        .create(&pool)
        .await
        .unwrap();

    let action = ActionFixture::new_unique(pack.id, &pack.r#ref, "action")
        .create(&pool)
        .await
        .unwrap();

    use attune_common::repositories::execution::{CreateExecutionInput, ExecutionRepository};
    let execution = ExecutionRepository::create(
        &pool,
        CreateExecutionInput {
            action: Some(action.id),
            action_ref: action.r#ref.clone(),
            config: None,
            env_vars: None,
            parent: None,
            enforcement: None,
            executor: None,
            permission_set_refs: Vec::new(),
            artifact_retention_policy: None,
            artifact_retention_limit: None,
            worker_selector: None,
            worker_tolerations: None,
            worker_affinity: None,
            worker: None,
            status: attune_common::models::enums::ExecutionStatus::Requested,
            trace_tag: None,
            result: None,
            workflow_task: None,
            timeout_seconds: None,
        },
    )
    .await
    .unwrap();

    let inquiry = InquiryFixture::new_unique(execution.id, "Delete me")
        .create(&pool)
        .await
        .unwrap();

    let deleted = InquiryRepository::delete(&pool, inquiry.id).await.unwrap();

    assert!(deleted);

    // Verify it's gone
    let found = InquiryRepository::find_by_id(&pool, inquiry.id)
        .await
        .unwrap();
    assert!(found.is_none());
}

#[tokio::test]
async fn test_delete_inquiry_not_found() {
    let pool = create_test_pool().await.unwrap();

    let deleted = InquiryRepository::delete(&pool, 99999).await.unwrap();

    assert!(!deleted);
}

#[tokio::test]
async fn test_delete_execution_preserves_inquiry_reference() {
    let pool = create_test_pool().await.unwrap();

    let pack = PackFixture::new_unique("cascade_pack")
        .create(&pool)
        .await
        .unwrap();

    let action = ActionFixture::new_unique(pack.id, &pack.r#ref, "action")
        .create(&pool)
        .await
        .unwrap();

    use attune_common::repositories::execution::{CreateExecutionInput, ExecutionRepository};
    let execution = ExecutionRepository::create(
        &pool,
        CreateExecutionInput {
            action: Some(action.id),
            action_ref: action.r#ref.clone(),
            config: None,
            env_vars: None,
            parent: None,
            enforcement: None,
            executor: None,
            permission_set_refs: Vec::new(),
            artifact_retention_policy: None,
            artifact_retention_limit: None,
            worker_selector: None,
            worker_tolerations: None,
            worker_affinity: None,
            worker: None,
            status: attune_common::models::enums::ExecutionStatus::Requested,
            trace_tag: None,
            result: None,
            workflow_task: None,
            timeout_seconds: None,
        },
    )
    .await
    .unwrap();

    // Create one inquiry for this execution.
    let inquiry1 = InquiryFixture::new_unique(execution.id, "First")
        .create(&pool)
        .await
        .unwrap();

    // Keep a second inquiry on a different execution to ensure unrelated rows survive.
    let execution2 = ExecutionRepository::create(
        &pool,
        CreateExecutionInput {
            action: Some(action.id),
            action_ref: action.r#ref.clone(),
            config: None,
            env_vars: None,
            parent: None,
            enforcement: None,
            executor: None,
            permission_set_refs: Vec::new(),
            artifact_retention_policy: None,
            artifact_retention_limit: None,
            worker_selector: None,
            worker_tolerations: None,
            worker_affinity: None,
            worker: None,
            status: attune_common::models::enums::ExecutionStatus::Requested,
            trace_tag: None,
            result: None,
            workflow_task: None,
            timeout_seconds: None,
        },
    )
    .await
    .unwrap();

    let inquiry2 = InquiryFixture::new_unique(execution2.id, "Second")
        .create(&pool)
        .await
        .unwrap();

    // Inquiries can outlive execution retention. The inquiry intentionally
    // keeps its plain BIGINT reference after the execution is removed.
    use attune_common::repositories::Delete;
    ExecutionRepository::delete(&pool, execution.id)
        .await
        .unwrap();

    // The deleted execution's inquiry remains available for audit/history.
    let found1 = InquiryRepository::find_by_id(&pool, inquiry1.id)
        .await
        .unwrap();
    assert_eq!(found1.unwrap().created_by_execution, execution.id);

    // Unrelated inquiry should remain.
    let found2 = InquiryRepository::find_by_id(&pool, inquiry2.id)
        .await
        .unwrap();
    assert_eq!(found2.unwrap().created_by_execution, execution2.id);
}

// ============================================================================
// SPECIALIZED QUERY Tests
// ============================================================================

#[tokio::test]
async fn test_find_inquiries_by_status() {
    let pool = create_test_pool().await.unwrap();

    let pack = PackFixture::new_unique("status_query_pack")
        .create(&pool)
        .await
        .unwrap();

    let action = ActionFixture::new_unique(pack.id, &pack.r#ref, "action")
        .create(&pool)
        .await
        .unwrap();

    use attune_common::repositories::execution::{CreateExecutionInput, ExecutionRepository};
    let execution = ExecutionRepository::create(
        &pool,
        CreateExecutionInput {
            action: Some(action.id),
            action_ref: action.r#ref.clone(),
            config: None,
            env_vars: None,
            parent: None,
            enforcement: None,
            executor: None,
            permission_set_refs: Vec::new(),
            artifact_retention_policy: None,
            artifact_retention_limit: None,
            worker_selector: None,
            worker_tolerations: None,
            worker_affinity: None,
            worker: None,
            status: attune_common::models::enums::ExecutionStatus::Requested,
            trace_tag: None,
            result: None,
            workflow_task: None,
            timeout_seconds: None,
        },
    )
    .await
    .unwrap();

    // Create inquiries with different statuses on distinct executions.
    let inq1 = InquiryFixture::new_unique(execution.id, "Pending 1")
        .with_status(InquiryStatus::Pending)
        .create(&pool)
        .await
        .unwrap();

    let execution2 = ExecutionRepository::create(
        &pool,
        CreateExecutionInput {
            action: Some(action.id),
            action_ref: action.r#ref.clone(),
            config: None,
            env_vars: None,
            parent: None,
            enforcement: None,
            executor: None,
            permission_set_refs: Vec::new(),
            artifact_retention_policy: None,
            artifact_retention_limit: None,
            worker_selector: None,
            worker_tolerations: None,
            worker_affinity: None,
            worker: None,
            status: attune_common::models::enums::ExecutionStatus::Requested,
            trace_tag: None,
            result: None,
            workflow_task: None,
            timeout_seconds: None,
        },
    )
    .await
    .unwrap();
    let inq2 = InquiryFixture::new_unique(execution2.id, "Responded")
        .with_status(InquiryStatus::Responded)
        .create(&pool)
        .await
        .unwrap();

    let execution3 = ExecutionRepository::create(
        &pool,
        CreateExecutionInput {
            action: Some(action.id),
            action_ref: action.r#ref.clone(),
            config: None,
            env_vars: None,
            parent: None,
            enforcement: None,
            executor: None,
            permission_set_refs: Vec::new(),
            artifact_retention_policy: None,
            artifact_retention_limit: None,
            worker_selector: None,
            worker_tolerations: None,
            worker_affinity: None,
            worker: None,
            status: attune_common::models::enums::ExecutionStatus::Requested,
            trace_tag: None,
            result: None,
            workflow_task: None,
            timeout_seconds: None,
        },
    )
    .await
    .unwrap();
    let inq3 = InquiryFixture::new_unique(execution3.id, "Pending 2")
        .with_status(InquiryStatus::Pending)
        .create(&pool)
        .await
        .unwrap();

    let pending_inquiries = InquiryRepository::find_by_status(&pool, InquiryStatus::Pending)
        .await
        .unwrap();

    // Filter to only our test inquiries
    let our_pending: Vec<_> = pending_inquiries
        .iter()
        .filter(|i| i.id == inq1.id || i.id == inq3.id)
        .collect();
    assert_eq!(our_pending.len(), 2);
    for inquiry in &our_pending {
        assert_eq!(inquiry.status, InquiryStatus::Pending);
    }

    let responded_inquiries = InquiryRepository::find_by_status(&pool, InquiryStatus::Responded)
        .await
        .unwrap();

    // Verify our responded inquiry is in the list
    let our_responded: Vec<_> = responded_inquiries
        .iter()
        .filter(|i| i.id == inq2.id)
        .collect();
    assert_eq!(our_responded.len(), 1);
}

#[tokio::test]
async fn test_find_inquiries_by_creator_execution() {
    let pool = create_test_pool().await.unwrap();

    let pack = PackFixture::new_unique("exec_query_pack")
        .create(&pool)
        .await
        .unwrap();

    let action = ActionFixture::new_unique(pack.id, &pack.r#ref, "action")
        .create(&pool)
        .await
        .unwrap();

    use attune_common::repositories::execution::{CreateExecutionInput, ExecutionRepository};
    let execution1 = ExecutionRepository::create(
        &pool,
        CreateExecutionInput {
            action: Some(action.id),
            action_ref: action.r#ref.clone(),
            config: None,
            env_vars: None,
            parent: None,
            enforcement: None,
            executor: None,
            permission_set_refs: Vec::new(),
            artifact_retention_policy: None,
            artifact_retention_limit: None,
            worker_selector: None,
            worker_tolerations: None,
            worker_affinity: None,
            worker: None,
            status: attune_common::models::enums::ExecutionStatus::Requested,
            trace_tag: None,
            result: None,
            workflow_task: None,
            timeout_seconds: None,
        },
    )
    .await
    .unwrap();

    let execution2 = ExecutionRepository::create(
        &pool,
        CreateExecutionInput {
            action: Some(action.id),
            action_ref: action.r#ref.clone(),
            config: None,
            env_vars: None,
            parent: None,
            enforcement: None,
            executor: None,
            permission_set_refs: Vec::new(),
            artifact_retention_policy: None,
            artifact_retention_limit: None,
            worker_selector: None,
            worker_tolerations: None,
            worker_affinity: None,
            worker: None,
            status: attune_common::models::enums::ExecutionStatus::Requested,
            trace_tag: None,
            result: None,
            workflow_task: None,
            timeout_seconds: None,
        },
    )
    .await
    .unwrap();

    InquiryFixture::new_unique(execution1.id, "Exec1 inquiry")
        .create(&pool)
        .await
        .unwrap();
    InquiryFixture::new_unique(execution2.id, "Exec2 inquiry")
        .create(&pool)
        .await
        .unwrap();

    let inquiries = InquiryRepository::find_by_created_by_execution(&pool, execution1.id)
        .await
        .unwrap();

    assert_eq!(inquiries.len(), 1);
    for inquiry in &inquiries {
        assert_eq!(inquiry.created_by_execution, execution1.id);
    }
}

// ============================================================================
// TIMESTAMP Tests
// ============================================================================

#[tokio::test]
async fn test_inquiry_timestamps_auto_managed() {
    let pool = create_test_pool().await.unwrap();

    let pack = PackFixture::new_unique("timestamp_pack")
        .create(&pool)
        .await
        .unwrap();

    let action = ActionFixture::new_unique(pack.id, &pack.r#ref, "action")
        .create(&pool)
        .await
        .unwrap();

    use attune_common::repositories::execution::{CreateExecutionInput, ExecutionRepository};
    let execution = ExecutionRepository::create(
        &pool,
        CreateExecutionInput {
            action: Some(action.id),
            action_ref: action.r#ref.clone(),
            config: None,
            env_vars: None,
            parent: None,
            enforcement: None,
            executor: None,
            permission_set_refs: Vec::new(),
            artifact_retention_policy: None,
            artifact_retention_limit: None,
            worker_selector: None,
            worker_tolerations: None,
            worker_affinity: None,
            worker: None,
            status: attune_common::models::enums::ExecutionStatus::Requested,
            trace_tag: None,
            result: None,
            workflow_task: None,
            timeout_seconds: None,
        },
    )
    .await
    .unwrap();

    let inquiry = InquiryFixture::new_unique(execution.id, "Timestamps")
        .create(&pool)
        .await
        .unwrap();

    let created_time = inquiry.created;
    let updated_time = inquiry.updated;

    assert!(created_time.timestamp() > 0);
    assert_eq!(created_time, updated_time);

    let updated_time = updated_time - Duration::seconds(1);
    set_updated_for_test(&pool, "inquiry", inquiry.id, updated_time).await;

    let input = UpdateInquiryInput {
        status: Some(InquiryStatus::Responded),
        response: None,
        responded_at: None,
        assigned_to: None,
    };

    let updated = InquiryRepository::update(&pool, inquiry.id, input)
        .await
        .unwrap();

    assert_eq!(updated.created, created_time); // created unchanged
    assert!(updated.updated > updated_time); // updated changed
}

// ============================================================================
// JSON SCHEMA Tests
// ============================================================================

#[tokio::test]
async fn test_inquiry_complex_response_schema() {
    let pool = create_test_pool().await.unwrap();

    let pack = PackFixture::new_unique("schema_complex_pack")
        .create(&pool)
        .await
        .unwrap();

    let action = ActionFixture::new_unique(pack.id, &pack.r#ref, "action")
        .create(&pool)
        .await
        .unwrap();

    use attune_common::repositories::execution::{CreateExecutionInput, ExecutionRepository};
    let execution = ExecutionRepository::create(
        &pool,
        CreateExecutionInput {
            action: Some(action.id),
            action_ref: action.r#ref.clone(),
            config: None,
            env_vars: None,
            parent: None,
            enforcement: None,
            executor: None,
            permission_set_refs: Vec::new(),
            artifact_retention_policy: None,
            artifact_retention_limit: None,
            worker_selector: None,
            worker_tolerations: None,
            worker_affinity: None,
            worker: None,
            status: attune_common::models::enums::ExecutionStatus::Requested,
            trace_tag: None,
            result: None,
            workflow_task: None,
            timeout_seconds: None,
        },
    )
    .await
    .unwrap();

    let complex_schema = json!({
        "severity": {
            "type": "string",
            "enum": ["low", "medium", "high", "critical"],
            "required": true
        },
        "impact_analysis": {
            "type": "object",
            "properties": {
                "affected_systems": {
                    "type": "array",
                    "items": {"type": "string"}
                },
                "estimated_downtime": {"type": "number"}
            }
        },
        "approval": {"type": "boolean", "required": true}
    });

    let inquiry = InquiryFixture::new_unique(execution.id, "Complex schema")
        .with_response_schema(complex_schema.clone())
        .with_response_options(response_options(json!({
            "severity": "low",
            "approval": true
        })))
        .create(&pool)
        .await
        .unwrap();

    assert_eq!(inquiry.response_schema, Some(complex_schema));
}
