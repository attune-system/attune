//! Integration tests for Inquiry repository
//!
//! These tests verify CRUD operations, queries, and constraints
//! for the Inquiry repository.

mod helpers;

use attune_common::{
    models::enums::InquiryStatus,
    repositories::{
        inquiry::{CreateInquiryInput, InquiryRepository, UpdateInquiryInput},
        Create, Delete, FindById, List, Update,
    },
    Error,
};
use chrono::{Duration, Utc};
use helpers::*;
use serde_json::json;

// ============================================================================
// CREATE Tests
// ============================================================================

#[tokio::test]
#[ignore = "integration test — requires database"]
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
        execution: execution.id,
        prompt: "Approve deployment?".to_string(),
        response_schema: None,
        assigned_to: None,
        status: InquiryStatus::Pending,
        response: None,
        timeout_at: None,
    };

    let inquiry = InquiryRepository::create(&pool, input).await.unwrap();

    assert!(inquiry.id > 0);
    assert_eq!(inquiry.execution, execution.id);
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
#[ignore = "integration test — requires database"]
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
        "type": "object",
        "properties": {
            "approved": {"type": "boolean"},
            "reason": {"type": "string"}
        },
        "required": ["approved"]
    });

    let input = CreateInquiryInput {
        execution: execution.id,
        prompt: "Approve this action?".to_string(),
        response_schema: Some(response_schema.clone()),
        assigned_to: None,
        status: InquiryStatus::Pending,
        response: None,
        timeout_at: None,
    };

    let inquiry = InquiryRepository::create(&pool, input).await.unwrap();

    assert_eq!(inquiry.response_schema, Some(response_schema));
}

#[tokio::test]
#[ignore = "integration test — requires database"]
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
        execution: execution.id,
        prompt: "Time-sensitive approval".to_string(),
        response_schema: None,
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
#[ignore = "integration test — requires database"]
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
        execution: execution.id,
        prompt: "Review and approve".to_string(),
        response_schema: None,
        assigned_to: Some(identity.id),
        status: InquiryStatus::Pending,
        response: None,
        timeout_at: None,
    };

    let inquiry = InquiryRepository::create(&pool, input).await.unwrap();

    assert_eq!(inquiry.assigned_to, Some(identity.id));
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_create_inquiry_with_invalid_execution_fails() {
    let pool = create_test_pool().await.unwrap();

    // Try to create inquiry with non-existent execution ID
    let input = CreateInquiryInput {
        execution: 99999,
        prompt: "Test prompt".to_string(),
        response_schema: None,
        assigned_to: None,
        status: InquiryStatus::Pending,
        response: None,
        timeout_at: None,
    };

    let result = InquiryRepository::create(&pool, input).await;

    assert!(result.is_err());
    // Foreign key constraint violation
}

// ============================================================================
// READ Tests
// ============================================================================

#[tokio::test]
#[ignore = "integration test — requires database"]
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
        .with_response_schema(json!({"type": "boolean"}))
        .create(&pool)
        .await
        .unwrap();

    let found = InquiryRepository::find_by_id(&pool, created_inquiry.id)
        .await
        .unwrap();

    assert!(found.is_some());
    let inquiry = found.unwrap();
    assert_eq!(inquiry.id, created_inquiry.id);
    assert_eq!(inquiry.execution, created_inquiry.execution);
    assert_eq!(inquiry.prompt, created_inquiry.prompt);
    assert_eq!(inquiry.status, created_inquiry.status);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_find_inquiry_by_id_not_found() {
    let pool = create_test_pool().await.unwrap();

    let result = InquiryRepository::find_by_id(&pool, 99999).await.unwrap();

    assert!(result.is_none());
}

#[tokio::test]
#[ignore = "integration test — requires database"]
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
#[ignore = "integration test — requires database"]
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
#[ignore = "integration test — requires database"]
async fn test_list_inquiries_empty() {
    let pool = create_test_pool().await.unwrap();

    let inquiries = InquiryRepository::list(&pool).await.unwrap();
    // May have inquiries from other tests, just verify we can list without error
    drop(inquiries);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
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
#[ignore = "integration test — requires database"]
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
#[ignore = "integration test — requires database"]
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
#[ignore = "integration test — requires database"]
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
#[ignore = "integration test — requires database"]
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
#[ignore = "integration test — requires database"]
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
#[ignore = "integration test — requires database"]
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
#[ignore = "integration test — requires database"]
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
#[ignore = "integration test — requires database"]
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
#[ignore = "integration test — requires database"]
async fn test_delete_inquiry_not_found() {
    let pool = create_test_pool().await.unwrap();

    let deleted = InquiryRepository::delete(&pool, 99999).await.unwrap();

    assert!(!deleted);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_delete_execution_cascades_to_inquiries() {
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

    // Delete the execution - should cascade to inquiries
    use attune_common::repositories::Delete;
    ExecutionRepository::delete(&pool, execution.id)
        .await
        .unwrap();

    // Verify the deleted execution's inquiry is gone.
    let found1 = InquiryRepository::find_by_id(&pool, inquiry1.id)
        .await
        .unwrap();
    assert!(found1.is_none());

    // Unrelated inquiry should remain.
    let found2 = InquiryRepository::find_by_id(&pool, inquiry2.id)
        .await
        .unwrap();
    assert_eq!(found2.unwrap().execution, execution2.id);
}

// ============================================================================
// SPECIALIZED QUERY Tests
// ============================================================================

#[tokio::test]
#[ignore = "integration test — requires database"]
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
#[ignore = "integration test — requires database"]
async fn test_find_inquiries_by_execution() {
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

    let inquiries = InquiryRepository::find_by_execution(&pool, execution1.id)
        .await
        .unwrap();

    assert_eq!(inquiries.len(), 1);
    for inquiry in &inquiries {
        assert_eq!(inquiry.execution, execution1.id);
    }
}

// ============================================================================
// TIMESTAMP Tests
// ============================================================================

#[tokio::test]
#[ignore = "integration test — requires database"]
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

    // Update and verify timestamp changed
    tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;

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
#[ignore = "integration test — requires database"]
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
        "type": "object",
        "properties": {
            "severity": {
                "type": "string",
                "enum": ["low", "medium", "high", "critical"]
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
            "approval": {"type": "boolean"}
        },
        "required": ["severity", "approval"]
    });

    let inquiry = InquiryFixture::new_unique(execution.id, "Complex schema")
        .with_response_schema(complex_schema.clone())
        .create(&pool)
        .await
        .unwrap();

    assert_eq!(inquiry.response_schema, Some(complex_schema));
}
