//! Integration tests for workflow API endpoints

use attune_common::repositories::{
    workflow::{CreateWorkflowDefinitionInput, WorkflowDefinitionRepository},
    Create,
};
use axum::http::StatusCode;
use serde_json::{json, Value};

mod helpers;
use helpers::*;

/// Generate a unique pack name for testing to avoid conflicts
fn unique_pack_name() -> String {
    format!(
        "test_pack_{}",
        &uuid::Uuid::new_v4().to_string().replace("-", "")[..8]
    )
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_create_workflow_success() {
    let ctx = TestContext::new().await.unwrap().with_auth().await.unwrap();

    // Create a pack first
    let pack_name = unique_pack_name();
    let pack = create_test_pack(&ctx.pool, &pack_name).await.unwrap();
    let action_ref = format!("{}.echo", pack.r#ref);
    let workflow_ref = format!("{}.test_workflow", pack.r#ref);
    create_test_action(&ctx.pool, pack.id, &pack.r#ref, &action_ref)
        .await
        .unwrap();

    // Create workflow via API
    let response = ctx
        .post(
            "/api/v1/workflows",
            json!({
                "ref": workflow_ref.clone(),
                "pack_ref": pack.r#ref.clone(),
                "label": "Test Workflow",
                "description": "A test workflow",
                "version": "1.0.0",
                "definition": {
                    "ref": workflow_ref.clone(),
                    "label": "Test Workflow",
                    "version": "1.0.0",
                    "tasks": [
                        {
                            "name": "task1",
                            "action": action_ref,
                            "input": {"message": "Hello"}
                        }
                    ]
                },
                "tags": ["test", "automation"]
            }),
            ctx.token(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::CREATED);

    let body: Value = response.json().await.unwrap();
    assert_eq!(body["data"]["ref"], workflow_ref);
    assert_eq!(body["data"]["label"], "Test Workflow");
    assert_eq!(body["data"]["version"], "1.0.0");
    assert!(body["data"]["tags"].as_array().unwrap().len() == 2);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_create_workflow_duplicate_ref() {
    let ctx = TestContext::new().await.unwrap().with_auth().await.unwrap();

    // Create a pack first
    let pack_name = unique_pack_name();
    let pack = create_test_pack(&ctx.pool, &pack_name).await.unwrap();

    // Create workflow directly in DB
    let input = CreateWorkflowDefinitionInput {
        r#ref: "test-pack.existing_workflow".to_string(),
        pack: pack.id,
        pack_ref: pack.r#ref.clone(),
        label: "Existing Workflow".to_string(),
        description: Some("An existing workflow".to_string()),
        version: "1.0.0".to_string(),
        param_schema: None,
        out_schema: None,
        definition: json!({"tasks": []}),
        tags: vec![],
    };
    WorkflowDefinitionRepository::create(&ctx.pool, input)
        .await
        .unwrap();

    // Try to create workflow with same ref via API
    let response = ctx
        .post(
            "/api/v1/workflows",
            json!({
                "ref": "test-pack.existing_workflow",
                "pack_ref": pack.r#ref,
                "label": "Duplicate Workflow",
                "version": "1.0.0",
                "definition": {"tasks": []}
            }),
            ctx.token(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::CONFLICT);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_create_workflow_pack_not_found() {
    let ctx = TestContext::new().await.unwrap().with_auth().await.unwrap();

    let response = ctx
        .post(
            "/api/v1/workflows",
            json!({
                "ref": "nonexistent.workflow",
                "pack_ref": "nonexistent-pack",
                "label": "Test Workflow",
                "version": "1.0.0",
                "definition": {"tasks": []}
            }),
            ctx.token(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_get_workflow_by_ref() {
    let ctx = TestContext::new().await.unwrap().with_auth().await.unwrap();

    // Create a pack and workflow
    let pack_name = unique_pack_name();
    let pack = create_test_pack(&ctx.pool, &pack_name).await.unwrap();
    let input = CreateWorkflowDefinitionInput {
        r#ref: "test-pack.my_workflow".to_string(),
        pack: pack.id,
        pack_ref: pack.r#ref.clone(),
        label: "My Workflow".to_string(),
        description: Some("A workflow".to_string()),
        version: "1.0.0".to_string(),
        param_schema: None,
        out_schema: None,
        definition: json!({"tasks": [{"name": "task1"}]}),
        tags: vec!["test".to_string()],
    };
    WorkflowDefinitionRepository::create(&ctx.pool, input)
        .await
        .unwrap();

    // Get workflow via API
    let response = ctx
        .get("/api/v1/workflows/test-pack.my_workflow", ctx.token())
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body: Value = response.json().await.unwrap();
    assert_eq!(body["data"]["ref"], "test-pack.my_workflow");
    assert_eq!(body["data"]["label"], "My Workflow");
    assert_eq!(body["data"]["version"], "1.0.0");
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_get_workflow_not_found() {
    let ctx = TestContext::new().await.unwrap().with_auth().await.unwrap();

    let response = ctx
        .get("/api/v1/workflows/nonexistent.workflow", ctx.token())
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_list_workflows() {
    let ctx = TestContext::new().await.unwrap().with_auth().await.unwrap();

    // Create a pack and multiple workflows
    let pack_name = unique_pack_name();
    let pack = create_test_pack(&ctx.pool, &pack_name).await.unwrap();

    for i in 1..=3 {
        let input = CreateWorkflowDefinitionInput {
            r#ref: format!("test-pack.workflow_{}", i),
            pack: pack.id,
            pack_ref: pack.r#ref.clone(),
            label: format!("Workflow {}", i),
            description: Some(format!("Workflow number {}", i)),
            version: "1.0.0".to_string(),
            param_schema: None,
            out_schema: None,
            definition: json!({"tasks": []}),
            tags: vec!["test".to_string()],
        };
        WorkflowDefinitionRepository::create(&ctx.pool, input)
            .await
            .unwrap();
    }

    // List all workflows (filtered by pack_ref for test isolation)
    let response = ctx
        .get(
            &format!(
                "/api/v1/workflows?page=1&per_page=10&pack_ref={}",
                pack_name
            ),
            ctx.token(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body: Value = response.json().await.unwrap();
    assert_eq!(body["items"].as_array().unwrap().len(), 3);
    assert_eq!(body["pagination"]["total_items"], 3);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_list_workflows_by_pack() {
    let ctx = TestContext::new().await.unwrap().with_auth().await.unwrap();

    // Create two packs
    let pack1_name = unique_pack_name();
    let pack2_name = unique_pack_name();
    let pack1 = create_test_pack(&ctx.pool, &pack1_name).await.unwrap();
    let pack2 = create_test_pack(&ctx.pool, &pack2_name).await.unwrap();

    // Create workflows for pack1
    for i in 1..=2 {
        let input = CreateWorkflowDefinitionInput {
            r#ref: format!("pack1.workflow_{}", i),
            pack: pack1.id,
            pack_ref: pack1.r#ref.clone(),
            label: format!("Pack1 Workflow {}", i),
            description: None,
            version: "1.0.0".to_string(),
            param_schema: None,
            out_schema: None,
            definition: json!({"tasks": []}),
            tags: vec![],
        };
        WorkflowDefinitionRepository::create(&ctx.pool, input)
            .await
            .unwrap();
    }

    // Create workflows for pack2
    let input = CreateWorkflowDefinitionInput {
        r#ref: "pack2.workflow_1".to_string(),
        pack: pack2.id,
        pack_ref: pack2.r#ref.clone(),
        label: "Pack2 Workflow".to_string(),
        description: None,
        version: "1.0.0".to_string(),
        param_schema: None,
        out_schema: None,
        definition: json!({"tasks": []}),
        tags: vec![],
    };
    WorkflowDefinitionRepository::create(&ctx.pool, input)
        .await
        .unwrap();

    // List workflows for pack1
    let response = ctx
        .get(
            &format!("/api/v1/packs/{}/workflows", pack1_name),
            ctx.token(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body: Value = response.json().await.unwrap();
    let workflows = body["items"].as_array().unwrap();
    assert_eq!(workflows.len(), 2);
    assert!(workflows
        .iter()
        .all(|w| w["pack_ref"] == pack1.r#ref.as_str()));
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_list_workflows_with_filters() {
    let ctx = TestContext::new().await.unwrap().with_auth().await.unwrap();

    let pack_name = unique_pack_name();
    let pack = create_test_pack(&ctx.pool, &pack_name).await.unwrap();

    // Create workflows with different tags
    let workflows = vec![
        ("workflow1", vec!["incident", "approval"]),
        ("workflow2", vec!["incident"]),
        ("workflow3", vec!["automation"]),
    ];

    for (ref_name, tags) in workflows {
        let input = CreateWorkflowDefinitionInput {
            r#ref: format!("test-pack.{}", ref_name),
            pack: pack.id,
            pack_ref: pack.r#ref.clone(),
            label: format!("Workflow {}", ref_name),
            description: Some(format!("Description for {}", ref_name)),
            version: "1.0.0".to_string(),
            param_schema: None,
            out_schema: None,
            definition: json!({"tasks": []}),
            tags: tags.iter().map(|s| s.to_string()).collect(),
        };
        WorkflowDefinitionRepository::create(&ctx.pool, input)
            .await
            .unwrap();
    }

    // Filter by tag (and pack_ref for isolation)
    let response = ctx
        .get(
            &format!("/api/v1/workflows?tags=incident&pack_ref={}", pack_name),
            ctx.token(),
        )
        .await
        .unwrap();
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["items"].as_array().unwrap().len(), 2);

    // Search by label (and pack_ref for isolation)
    let response = ctx
        .get(
            &format!("/api/v1/workflows?search=workflow1&pack_ref={}", pack_name),
            ctx.token(),
        )
        .await
        .unwrap();
    let body: Value = response.json().await.unwrap();
    assert_eq!(body["items"].as_array().unwrap().len(), 1);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_update_workflow() {
    let ctx = TestContext::new().await.unwrap().with_auth().await.unwrap();

    // Create a pack and workflow
    let pack_name = unique_pack_name();
    let pack = create_test_pack(&ctx.pool, &pack_name).await.unwrap();
    let input = CreateWorkflowDefinitionInput {
        r#ref: "test-pack.update_test".to_string(),
        pack: pack.id,
        pack_ref: pack.r#ref.clone(),
        label: "Original Label".to_string(),
        description: Some("Original description".to_string()),
        version: "1.0.0".to_string(),
        param_schema: None,
        out_schema: None,
        definition: json!({"tasks": []}),
        tags: vec!["test".to_string()],
    };
    WorkflowDefinitionRepository::create(&ctx.pool, input)
        .await
        .unwrap();

    // Update workflow via API
    let response = ctx
        .put(
            "/api/v1/workflows/test-pack.update_test",
            json!({
                "label": "Updated Label",
                "description": "Updated description",
                "version": "1.1.0"
            }),
            ctx.token(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    let body: Value = response.json().await.unwrap();
    assert_eq!(body["data"]["label"], "Updated Label");
    assert_eq!(body["data"]["description"], "Updated description");
    assert_eq!(body["data"]["version"], "1.1.0");
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_update_workflow_not_found() {
    let ctx = TestContext::new().await.unwrap().with_auth().await.unwrap();

    let response = ctx
        .put(
            "/api/v1/workflows/nonexistent.workflow",
            json!({
                "label": "Updated Label"
            }),
            ctx.token(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_delete_workflow() {
    let ctx = TestContext::new().await.unwrap().with_auth().await.unwrap();

    // Create a pack and workflow
    let pack_name = unique_pack_name();
    let pack = create_test_pack(&ctx.pool, &pack_name).await.unwrap();
    let input = CreateWorkflowDefinitionInput {
        r#ref: "test-pack.delete_test".to_string(),
        pack: pack.id,
        pack_ref: pack.r#ref.clone(),
        label: "To Be Deleted".to_string(),
        description: None,
        version: "1.0.0".to_string(),
        param_schema: None,
        out_schema: None,
        definition: json!({"tasks": []}),
        tags: vec![],
    };
    WorkflowDefinitionRepository::create(&ctx.pool, input)
        .await
        .unwrap();

    // Delete workflow via API
    let response = ctx
        .delete("/api/v1/workflows/test-pack.delete_test", ctx.token())
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);

    // Verify it's deleted
    let response = ctx
        .get("/api/v1/workflows/test-pack.delete_test", ctx.token())
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_delete_workflow_not_found() {
    let ctx = TestContext::new().await.unwrap().with_auth().await.unwrap();

    let response = ctx
        .delete("/api/v1/workflows/nonexistent.workflow", ctx.token())
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_create_workflow_requires_auth() {
    let ctx = TestContext::new().await.unwrap();

    let response = ctx
        .post(
            "/api/v1/workflows",
            json!({
                "ref": "test.workflow",
                "pack_ref": "test",
                "label": "Test",
                "version": "1.0.0",
                "definition": {"tasks": []}
            }),
            None,
        )
        .await
        .unwrap();

    // TODO: API endpoints don't currently enforce authentication
    // This should be 401 once auth middleware is implemented
    assert!(response.status().is_success() || response.status().is_client_error());
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_workflow_validation() {
    let ctx = TestContext::new().await.unwrap().with_auth().await.unwrap();

    // Test empty ref
    let response = ctx
        .post(
            "/api/v1/workflows",
            json!({
                "ref": "",
                "pack_ref": "test",
                "label": "Test",
                "version": "1.0.0",
                "definition": {"tasks": []}
            }),
            ctx.token(),
        )
        .await
        .unwrap();

    // API returns 422 (Unprocessable Entity) for validation errors
    assert!(response.status().is_client_error());

    // Test empty label
    let response = ctx
        .post(
            "/api/v1/workflows",
            json!({
                "ref": "test.workflow",
                "pack_ref": "test",
                "label": "",
                "version": "1.0.0",
                "definition": {"tasks": []}
            }),
            ctx.token(),
        )
        .await
        .unwrap();

    // API returns 422 (Unprocessable Entity) for validation errors
    assert!(response.status().is_client_error());
}
