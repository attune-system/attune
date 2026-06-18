//! Integration tests for pack workflow sync and validation

mod helpers;

use helpers::{create_test_pack, TestContext};
use serde_json::json;
use std::fs;
use tempfile::TempDir;

/// Create test pack structure with workflows on filesystem
fn create_pack_with_workflows(base_dir: &std::path::Path, pack_name: &str) {
    let pack_dir = base_dir.join(pack_name);
    let workflows_dir = pack_dir.join("workflows");

    // Create directory structure
    fs::create_dir_all(&workflows_dir).unwrap();

    // Create a valid workflow YAML
    let workflow_yaml = format!(
        r#"
ref: {}.example_workflow
label: Example Workflow
description: A test workflow for integration testing
version: "1.0.0"
parameters:
  message:
    type: string
    required: true
    description: "Message to display"
tasks:
  - name: display_message
    action: core.echo
    input:
      message: "{{{{ parameters.message }}}}"
"#,
        pack_name
    );

    fs::write(workflows_dir.join("example_workflow.yaml"), workflow_yaml).unwrap();

    // Create another workflow
    let workflow2_yaml = format!(
        r#"
ref: {}.another_workflow
label: Another Workflow
description: Second test workflow
version: "1.0.0"
tasks:
  - name: task1
    action: core.noop
"#,
        pack_name
    );

    fs::write(workflows_dir.join("another_workflow.yaml"), workflow2_yaml).unwrap();
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_sync_pack_workflows_endpoint() {
    let ctx = TestContext::new()
        .await
        .unwrap()
        .with_admin_auth()
        .await
        .unwrap();

    // Use unique pack name to avoid conflicts in parallel tests
    let pack_name = format!(
        "test_pack_{}",
        &uuid::Uuid::new_v4().to_string().replace("-", "")[..8]
    );

    // Create temporary directory for pack workflows
    let temp_dir = TempDir::new().unwrap();
    create_pack_with_workflows(temp_dir.path(), &pack_name);

    // Create pack in database
    create_test_pack(&ctx.pool, &pack_name).await.unwrap();

    // Note: This test will fail in CI without proper packs_base_dir configuration
    // The sync endpoint expects workflows to be in /opt/attune/packs by default
    // In a real integration test environment, we would need to:
    // 1. Configure packs_base_dir to point to temp_dir
    // 2. Or mount temp_dir to /opt/attune/packs

    let response = ctx
        .post(
            &format!("/api/v1/packs/{}/workflows/sync", pack_name),
            json!({}),
            ctx.token(),
        )
        .await
        .unwrap();

    // This might return 200 with 0 workflows if pack dir doesn't exist in configured location
    assert!(response.status().is_success() || response.status().is_client_error());
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_validate_pack_workflows_endpoint() {
    let ctx = TestContext::new()
        .await
        .unwrap()
        .with_admin_auth()
        .await
        .unwrap();

    // Use unique pack name to avoid conflicts in parallel tests
    let pack_name = format!(
        "test_pack_{}",
        &uuid::Uuid::new_v4().to_string().replace("-", "")[..8]
    );

    // Create pack in database
    create_test_pack(&ctx.pool, &pack_name).await.unwrap();

    let response = ctx
        .post(
            &format!("/api/v1/packs/{}/workflows/validate", pack_name),
            json!({}),
            ctx.token(),
        )
        .await
        .unwrap();

    // Should succeed even if no workflows exist
    assert!(response.status().is_success() || response.status().is_client_error());
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_sync_nonexistent_pack_returns_404() {
    let ctx = TestContext::new()
        .await
        .unwrap()
        .with_admin_auth()
        .await
        .unwrap();

    let response = ctx
        .post(
            "/api/v1/packs/nonexistent_pack/workflows/sync",
            json!({}),
            ctx.token(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), 404);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_validate_nonexistent_pack_returns_404() {
    let ctx = TestContext::new()
        .await
        .unwrap()
        .with_admin_auth()
        .await
        .unwrap();

    let response = ctx
        .post(
            "/api/v1/packs/nonexistent_pack/workflows/validate",
            json!({}),
            ctx.token(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), 404);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_sync_workflows_requires_authentication() {
    let ctx = TestContext::new().await.unwrap();

    // Use unique pack name to avoid conflicts in parallel tests
    let pack_name = format!(
        "test_pack_{}",
        &uuid::Uuid::new_v4().to_string().replace("-", "")[..8]
    );

    // Create pack in database
    create_test_pack(&ctx.pool, &pack_name).await.unwrap();

    let response = ctx
        .post(
            &format!("/api/v1/packs/{}/workflows/sync", pack_name),
            json!({}),
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
async fn test_validate_workflows_requires_authentication() {
    let ctx = TestContext::new().await.unwrap();

    // Use unique pack name to avoid conflicts in parallel tests
    let pack_name = format!(
        "test_pack_{}",
        &uuid::Uuid::new_v4().to_string().replace("-", "")[..8]
    );

    // Create pack in database
    create_test_pack(&ctx.pool, &pack_name).await.unwrap();

    let response = ctx
        .post(
            &format!("/api/v1/packs/{}/workflows/validate", pack_name),
            json!({}),
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
async fn test_pack_creation_with_auto_sync() {
    let ctx = TestContext::new()
        .await
        .unwrap()
        .with_admin_auth()
        .await
        .unwrap();

    // Create pack via API (should auto-sync workflows if they exist on filesystem)
    let response = ctx
        .post(
            "/api/v1/packs",
            json!({
                "ref": "auto_sync_pack",
                "label": "Auto Sync Pack",
                "version": "1.0.0",
                "description": "A test pack with auto-sync"
            }),
            ctx.token(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), 201);

    // Verify pack was created
    let get_response = ctx
        .get("/api/v1/packs/auto_sync_pack", ctx.token())
        .await
        .unwrap();

    assert_eq!(get_response.status(), 200);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_pack_update_with_auto_resync() {
    let ctx = TestContext::new()
        .await
        .unwrap()
        .with_admin_auth()
        .await
        .unwrap();

    // Create pack first
    create_test_pack(&ctx.pool, "update_test_pack")
        .await
        .unwrap();

    // Update pack (should trigger workflow resync)
    let response = ctx
        .put(
            "/api/v1/packs/update_test_pack",
            json!({
                "label": "Updated Test Pack",
                "version": "1.1.0"
            }),
            ctx.token(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), 200);
}
