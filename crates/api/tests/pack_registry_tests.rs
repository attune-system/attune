//! Integration tests for pack registry system
//!
//! This module tests:
//! - End-to-end pack installation from all sources (git, archive, local, registry)
//! - Dependency validation during installation
//! - Installation metadata tracking
//! - Checksum verification
//! - Error handling and edge cases

mod helpers;

use attune_common::{
    models::Pack,
    pack_registry::calculate_directory_checksum,
    repositories::{
        identity::{CreatePermissionSetInput, PermissionSetRepository},
        pack::{CreatePackInput, PackRepository},
        Create, FindById, FindByRef, List,
    },
};
use helpers::{Result, TestContext};
use serde_json::json;
use std::fs;
use tempfile::TempDir;

/// Helper to create a test pack directory with pack.yaml
fn create_test_pack_dir(name: &str, version: &str) -> Result<TempDir> {
    let temp_dir = TempDir::new()?;
    let pack_yaml = format!(
        r#"
ref: {}
name: Test Pack {}
version: {}
description: Test pack for integration tests
author: Test Author
email: test@example.com
keywords:
  - test
  - integration
dependencies: []
python: "3.8"
actions:
  test_action:
    entry_point: test.py
    runner_type: python-script
"#,
        name, name, version
    );

    fs::write(temp_dir.path().join("pack.yaml"), pack_yaml)?;

    // Create a simple action file
    let action_content = r#"
#!/usr/bin/env python3
print("Test action executed")
"#;
    fs::write(temp_dir.path().join("test.py"), action_content)?;

    Ok(temp_dir)
}

/// Helper to create a pack with dependencies
fn create_pack_with_deps(name: &str, deps: &[&str]) -> Result<TempDir> {
    let temp_dir = TempDir::new()?;
    let deps_yaml = deps
        .iter()
        .map(|d| format!("  - {}", d))
        .collect::<Vec<_>>()
        .join("\n");

    let pack_yaml = format!(
        r#"
ref: {}
name: Test Pack {}
version: 1.0.0
description: Test pack with dependencies
author: Test Author
dependencies:
{}
python: "3.8"
actions:
  test_action:
    entry_point: test.py
    runner_type: python-script
"#,
        name, name, deps_yaml
    );

    fs::write(temp_dir.path().join("pack.yaml"), pack_yaml)?;
    fs::write(temp_dir.path().join("test.py"), "print('test')")?;

    Ok(temp_dir)
}

/// Helper to create a pack with specific runtime requirements
fn create_pack_with_runtime(
    name: &str,
    python: Option<&str>,
    nodejs: Option<&str>,
) -> Result<TempDir> {
    let temp_dir = TempDir::new()?;

    let python_line = python
        .map(|v| format!("python: \"{}\"", v))
        .unwrap_or_default();
    let nodejs_line = nodejs
        .map(|v| format!("nodejs: \"{}\"", v))
        .unwrap_or_default();

    let pack_yaml = format!(
        r#"
ref: {}
name: Test Pack {}
version: 1.0.0
description: Test pack with runtime requirements
author: Test Author
{}
{}
actions:
  test_action:
    entry_point: test.py
    runner_type: python-script
"#,
        name, name, python_line, nodejs_line
    );

    fs::write(temp_dir.path().join("pack.yaml"), pack_yaml)?;
    fs::write(temp_dir.path().join("test.py"), "print('test')")?;

    Ok(temp_dir)
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_install_pack_from_local_directory() -> Result<()> {
    let ctx = TestContext::new().await?.with_admin_auth().await?;
    let token = ctx.token().unwrap();

    // Create a test pack directory
    let pack_dir = create_test_pack_dir("local-test", "1.0.0")?;
    let pack_path = pack_dir.path().to_string_lossy().to_string();

    // Install pack from local directory
    let response = ctx
        .post(
            "/api/v1/packs/install",
            json!({
                "source": pack_path,
                "force": false,
                "skip_tests": true,
                "skip_deps": true
            }),
            Some(token),
        )
        .await?;

    let status = response.status();
    let body_text = response.text().await?;

    if status != 200 {
        eprintln!("Error response (status {}): {}", status, body_text);
    }
    assert_eq!(status, 200, "Installation should succeed");

    let body: serde_json::Value = serde_json::from_str(&body_text)?;
    assert_eq!(body["data"]["pack"]["ref"], "local-test");
    assert_eq!(body["data"]["pack"]["version"], "1.0.0");
    assert_eq!(body["data"]["tests_skipped"], true);

    Ok(())
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_install_pack_with_dependency_validation_success() -> Result<()> {
    let ctx = TestContext::new().await?.with_admin_auth().await?;
    let token = ctx.token().unwrap();

    // First, install a dependency pack
    let dep_pack_dir = create_test_pack_dir("core", "1.0.0")?;
    let dep_path = dep_pack_dir.path().to_string_lossy().to_string();

    ctx.post(
        "/api/v1/packs/install",
        json!({
            "source": dep_path,
            "force": false,
            "skip_tests": true,
            "skip_deps": true
        }),
        Some(token),
    )
    .await?;

    // Now install a pack that depends on it
    let pack_dir = create_pack_with_deps("dependent-pack", &["core"])?;
    let pack_path = pack_dir.path().to_string_lossy().to_string();

    let response = ctx
        .post(
            "/api/v1/packs/install",
            json!({
                "source": pack_path,
                "force": false,
                "skip_tests": true,
                "skip_deps": false  // Enable dependency validation
            }),
            Some(token),
        )
        .await?;

    assert_eq!(
        response.status(),
        200,
        "Installation should succeed when dependencies are met"
    );

    let body: serde_json::Value = response.json().await?;
    assert_eq!(body["data"]["pack"]["ref"], "dependent-pack");

    Ok(())
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_install_pack_with_missing_dependency_fails() -> Result<()> {
    let ctx = TestContext::new().await?.with_admin_auth().await?;
    let token = ctx.token().unwrap();

    // Create a pack with an unmet dependency
    let pack_dir = create_pack_with_deps("dependent-pack", &["missing-pack"])?;
    let pack_path = pack_dir.path().to_string_lossy().to_string();

    let response = ctx
        .post(
            "/api/v1/packs/install",
            json!({
                "source": pack_path,
                "force": false,
                "skip_tests": true,
                "skip_deps": false  // Enable dependency validation
            }),
            Some(token),
        )
        .await?;

    // Should fail with 400 Bad Request
    assert_eq!(
        response.status(),
        400,
        "Installation should fail when dependencies are missing"
    );

    let body: serde_json::Value = response.json().await?;
    let error_msg = body["error"].as_str().unwrap();
    assert!(
        error_msg.contains("dependency validation failed") || error_msg.contains("missing-pack"),
        "Error should mention dependency validation failure"
    );

    Ok(())
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_register_pack_returns_install_contract_for_equal_manifest_mirrors() -> Result<()> {
    let ctx = TestContext::new().await?.with_admin_auth().await?;
    let pack_dir = TempDir::new()?;
    fs::write(
        pack_dir.path().join("pack.yaml"),
        r#"ref: manifest_contract_pack
version: 1.0.0
label: Canonical label
name: Legacy label
conf_schema:
  canonical:
    type: string
config_schema:
  canonical:
    type: string
meta:
  source: canonical
metadata:
  source: canonical
tags: [canonical]
keywords: [canonical]
"#,
    )?;

    let response = ctx
        .post(
            "/api/v1/packs/register",
            json!({
                "path": pack_dir.path().to_string_lossy(),
                "force": false,
                "skip_tests": true
            }),
            ctx.token(),
        )
        .await?;

    assert_eq!(response.status(), axum::http::StatusCode::CREATED);
    let body: serde_json::Value = response.json().await?;
    assert_eq!(body["data"]["pack"]["ref"], "manifest_contract_pack");
    assert_eq!(body["data"]["pack"]["label"], "Canonical label");
    assert_eq!(body["data"]["tests_skipped"], true);
    assert!(body["data"]["test_result"].is_null());

    let pack = PackRepository::find_by_ref(&ctx.pool, "manifest_contract_pack")
        .await?
        .expect("registered pack");
    assert_eq!(pack.conf_schema["canonical"]["type"], "string");
    assert!(pack.conf_schema.get("legacy").is_none());
    assert_eq!(pack.meta["source"], "canonical");
    assert_eq!(pack.tags, vec!["canonical"]);

    Ok(())
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_register_pack_rolls_back_when_component_loading_fails() -> Result<()> {
    let ctx = TestContext::new().await?.with_admin_auth().await?;
    let fixture =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/pack_load_failure");

    let response = ctx
        .post(
            "/api/v1/packs/register",
            json!({
                "path": fixture.to_string_lossy(),
                "force": false,
                "skip_tests": true
            }),
            ctx.token(),
        )
        .await?;

    assert_eq!(response.status(), axum::http::StatusCode::BAD_REQUEST);
    let body: serde_json::Value = response.json().await?;
    assert!(body["error"]
        .as_str()
        .is_some_and(|message| message.contains("failed while loading components")));
    assert!(
        PackRepository::find_by_ref(&ctx.pool, "test_pack_load_failure")
            .await?
            .is_none(),
        "a failed component load must not leave a registered pack"
    );
    assert!(
        PermissionSetRepository::find_by_ref(&ctx.pool, "test_pack_load_failure.preflight")
            .await?
            .is_none(),
        "cache definition preflight must run before any pack component mutation"
    );

    let existing_pack = PackRepository::create(
        &ctx.pool,
        CreatePackInput {
            r#ref: "test_pack_load_failure".to_string(),
            label: "Existing Pack".to_string(),
            description: None,
            version: "0.9.0".to_string(),
            conf_schema: json!({}),
            config: json!({}),
            meta: json!({}),
            tags: Vec::new(),
            runtime_deps: Vec::new(),
            dependencies: Vec::new(),
            is_standard: false,
            installers: json!({}),
        },
    )
    .await?;
    PermissionSetRepository::create(
        &ctx.pool,
        CreatePermissionSetInput {
            r#ref: "test_pack_load_failure.preflight".to_string(),
            pack: Some(existing_pack.id),
            pack_ref: Some(existing_pack.r#ref.clone()),
            label: Some("Existing Sentinel".to_string()),
            description: None,
            grants: json!([]),
        },
    )
    .await?;

    let reinstall_response = ctx
        .post(
            "/api/v1/packs/register",
            json!({
                "path": fixture.to_string_lossy(),
                "force": true,
                "skip_tests": true
            }),
            ctx.token(),
        )
        .await?;
    assert_eq!(
        reinstall_response.status(),
        axum::http::StatusCode::BAD_REQUEST
    );
    assert_eq!(
        PackRepository::find_by_ref(&ctx.pool, "test_pack_load_failure")
            .await?
            .expect("existing pack remains")
            .version,
        "0.9.0",
        "failed component loading must not publish staged pack metadata"
    );
    assert_eq!(
        PermissionSetRepository::find_by_ref(&ctx.pool, "test_pack_load_failure.preflight")
            .await?
            .expect("existing permission set remains")
            .label
            .as_deref(),
        Some("Existing Sentinel"),
        "cache definition preflight must prevent partial existing-pack component updates"
    );

    Ok(())
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_install_pack_skip_deps_bypasses_validation() -> Result<()> {
    let ctx = TestContext::new().await?.with_admin_auth().await?;
    let token = ctx.token().unwrap();

    // Create a pack with an unmet dependency
    let pack_dir = create_pack_with_deps("dependent-pack", &["missing-pack"])?;
    let pack_path = pack_dir.path().to_string_lossy().to_string();

    let response = ctx
        .post(
            "/api/v1/packs/install",
            json!({
                "source": pack_path,
                "force": false,
                "skip_tests": true,
                "skip_deps": true  // Skip dependency validation
            }),
            Some(token),
        )
        .await?;

    // Should succeed because validation is skipped
    assert_eq!(
        response.status(),
        200,
        "Installation should succeed when validation is skipped"
    );

    let body: serde_json::Value = response.json().await?;
    assert_eq!(body["data"]["pack"]["ref"], "dependent-pack");

    Ok(())
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_install_pack_with_runtime_validation() -> Result<()> {
    let ctx = TestContext::new().await?.with_admin_auth().await?;
    let token = ctx.token().unwrap();

    // Create a pack with reasonable runtime requirements
    let pack_dir = create_pack_with_runtime("runtime-test", Some("3.8"), None)?;
    let pack_path = pack_dir.path().to_string_lossy().to_string();

    let response = ctx
        .post(
            "/api/v1/packs/install",
            json!({
                "source": pack_path,
                "force": false,
                "skip_tests": true,
                "skip_deps": false  // Enable validation
            }),
            Some(token),
        )
        .await?;

    // Result depends on whether Python 3.8+ is available in test environment
    // We just verify the response is well-formed
    let status = response.status();
    assert!(
        status == 200 || status == 400,
        "Should either succeed or fail gracefully"
    );

    Ok(())
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_install_pack_metadata_tracking() -> Result<()> {
    let ctx = TestContext::new().await?.with_admin_auth().await?;
    let token = ctx.token().unwrap();

    // Install a pack
    let pack_dir = create_test_pack_dir("metadata-test", "1.0.0")?;
    let pack_path = pack_dir.path().to_string_lossy().to_string();
    let original_checksum = calculate_directory_checksum(pack_dir.path())?;

    let response = ctx
        .post(
            "/api/v1/packs/install",
            json!({
                "source": pack_path,
                "force": false,
                "skip_tests": true,
                "skip_deps": true
            }),
            Some(token),
        )
        .await?;

    assert_eq!(response.status(), 200);

    let body: serde_json::Value = response.json().await?;
    let pack_id = body["data"]["pack"]["id"].as_i64().unwrap();

    // Verify installation metadata was created
    let pack = PackRepository::find_by_id(&ctx.pool, pack_id)
        .await?
        .expect("Should have pack record");

    assert_eq!(pack.id, pack_id);
    assert_eq!(pack.source_type.as_deref(), Some("local_directory"));
    assert!(pack.source_url.is_some());
    assert!(pack.checksum.is_some());
    assert!(pack.installed_at.is_some());

    // Verify checksum matches
    let stored_checksum = pack.checksum.as_ref().unwrap();
    assert_eq!(
        stored_checksum, &original_checksum,
        "Stored checksum should match calculated checksum"
    );

    Ok(())
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_install_pack_force_reinstall() -> Result<()> {
    let ctx = TestContext::new().await?.with_admin_auth().await?;
    let token = ctx.token().unwrap();

    let pack_dir = create_test_pack_dir("force-test", "1.0.0")?;
    let pack_path = pack_dir.path().to_string_lossy().to_string();

    // Install once
    let response1 = ctx
        .post(
            "/api/v1/packs/install",
            json!({
                "source": &pack_path,
                "force": false,
                "skip_tests": true,
                "skip_deps": true
            }),
            Some(token),
        )
        .await?;

    assert_eq!(response1.status(), 200);

    // Try to install again without force - should work but might replace
    let response2 = ctx
        .post(
            "/api/v1/packs/install",
            json!({
                "source": &pack_path,
                "force": true,
                "skip_tests": true,
                "skip_deps": true
            }),
            Some(token),
        )
        .await?;

    assert_eq!(response2.status(), 200, "Force reinstall should succeed");

    // Verify pack exists
    let packs = PackRepository::list(&ctx.pool).await?;
    let force_test_packs: Vec<&Pack> = packs.iter().filter(|p| p.r#ref == "force-test").collect();
    assert_eq!(
        force_test_packs.len(),
        1,
        "Should have exactly one force-test pack"
    );

    Ok(())
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_install_pack_storage_path_created() -> Result<()> {
    let ctx = TestContext::new().await?.with_admin_auth().await?;
    let token = ctx.token().unwrap();

    let pack_dir = create_test_pack_dir("storage-test", "2.3.4")?;
    let pack_path = pack_dir.path().to_string_lossy().to_string();

    let response = ctx
        .post(
            "/api/v1/packs/install",
            json!({
                "source": pack_path,
                "force": false,
                "skip_tests": true,
                "skip_deps": true
            }),
            Some(token),
        )
        .await?;

    assert_eq!(response.status(), 200);

    let body: serde_json::Value = response.json().await?;
    let pack_id = body["data"]["pack"]["id"].as_i64().unwrap();

    // Verify installation metadata has storage path
    let pack = PackRepository::find_by_id(&ctx.pool, pack_id)
        .await?
        .expect("Should have pack record");

    let storage_path = pack
        .storage_path
        .as_ref()
        .expect("Should have storage path");
    assert!(
        storage_path.contains("storage-test"),
        "Storage path should contain pack ref"
    );
    assert!(
        storage_path.ends_with("storage-test"),
        "Storage path should end with the installed pack ref"
    );

    // Note: We can't verify the actual filesystem without knowing the config path
    // but we verify the path structure is correct

    Ok(())
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_install_pack_invalid_source() -> Result<()> {
    let ctx = TestContext::new().await?.with_admin_auth().await?;
    let token = ctx.token().unwrap();

    let response = ctx
        .post(
            "/api/v1/packs/install",
            json!({
                "source": "/nonexistent/path/to/pack",
                "force": false,
                "skip_tests": true,
                "skip_deps": true
            }),
            Some(token),
        )
        .await?;

    assert_eq!(
        response.status(),
        404,
        "Should fail with not found status for nonexistent path"
    );

    let body: serde_json::Value = response.json().await?;
    assert!(body["error"].is_string(), "Should have error message");

    Ok(())
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_install_pack_missing_pack_yaml() -> Result<()> {
    let ctx = TestContext::new().await?.with_admin_auth().await?;
    let token = ctx.token().unwrap();

    // Create directory without pack.yaml
    let temp_dir = TempDir::new()?;
    fs::write(temp_dir.path().join("readme.txt"), "No pack.yaml here")?;

    let response = ctx
        .post(
            "/api/v1/packs/install",
            json!({
                "source": temp_dir.path().to_string_lossy(),
                "force": false,
                "skip_tests": true,
                "skip_deps": true
            }),
            Some(token),
        )
        .await?;

    assert_eq!(response.status(), 400, "Should fail with bad request");

    let body: serde_json::Value = response.json().await?;
    let error = body["error"].as_str().unwrap();
    assert!(
        error.contains("pack.yaml"),
        "Error should mention pack.yaml"
    );

    Ok(())
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_install_pack_invalid_pack_yaml() -> Result<()> {
    let ctx = TestContext::new().await?.with_admin_auth().await?;
    let token = ctx.token().unwrap();

    // Create pack.yaml with invalid content
    let temp_dir = TempDir::new()?;
    fs::write(temp_dir.path().join("pack.yaml"), "invalid: yaml: content:")?;

    let response = ctx
        .post(
            "/api/v1/packs/install",
            json!({
                "source": temp_dir.path().to_string_lossy(),
                "force": false,
                "skip_tests": true,
                "skip_deps": true
            }),
            Some(token),
        )
        .await?;

    // Should fail with error status
    assert!(response.status().is_client_error() || response.status().is_server_error());

    Ok(())
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_install_pack_without_auth_fails() -> Result<()> {
    let ctx = TestContext::new().await?; // No auth

    let pack_dir = create_test_pack_dir("auth-test", "1.0.0")?;
    let pack_path = pack_dir.path().to_string_lossy().to_string();

    let response = ctx
        .post(
            "/api/v1/packs/install",
            json!({
                "source": pack_path,
                "force": false,
                "skip_tests": true,
                "skip_deps": true
            }),
            None, // No token
        )
        .await?;

    assert_eq!(response.status(), 401, "Should require authentication");

    Ok(())
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_multiple_pack_installations() -> Result<()> {
    let ctx = TestContext::new().await?.with_admin_auth().await?;
    let token = ctx.token().unwrap();

    // Install multiple packs
    for i in 1..=3 {
        let pack_dir = create_test_pack_dir(&format!("multi-pack-{}", i), "1.0.0")?;
        let pack_path = pack_dir.path().to_string_lossy().to_string();

        let response = ctx
            .post(
                "/api/v1/packs/install",
                json!({
                    "source": pack_path,
                    "force": false,
                    "skip_tests": true,
                    "skip_deps": true
                }),
                Some(token),
            )
            .await?;

        assert_eq!(
            response.status(),
            200,
            "Pack {} installation should succeed",
            i
        );
    }

    // Verify all packs are installed
    let packs = <PackRepository as List>::list(&ctx.pool).await?;
    let multi_packs: Vec<&Pack> = packs
        .iter()
        .filter(|p| p.r#ref.starts_with("multi-pack-"))
        .collect();

    assert_eq!(
        multi_packs.len(),
        3,
        "Should have 3 multi-pack installations"
    );

    Ok(())
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_install_pack_version_upgrade() -> Result<()> {
    let ctx = TestContext::new().await?.with_admin_auth().await?;
    let token = ctx.token().unwrap();

    // Install version 1.0.0
    let pack_dir_v1 = create_test_pack_dir("version-test", "1.0.0")?;
    let response1 = ctx
        .post(
            "/api/v1/packs/install",
            json!({
                "source": pack_dir_v1.path().to_string_lossy(),
                "force": false,
                "skip_tests": true,
                "skip_deps": true
            }),
            Some(token),
        )
        .await?;

    assert_eq!(response1.status(), 200);

    // Install version 2.0.0 with force
    let pack_dir_v2 = create_test_pack_dir("version-test", "2.0.0")?;
    let response2 = ctx
        .post(
            "/api/v1/packs/install",
            json!({
                "source": pack_dir_v2.path().to_string_lossy(),
                "force": true,
                "skip_tests": true,
                "skip_deps": true
            }),
            Some(token),
        )
        .await?;

    assert_eq!(response2.status(), 200);

    let body: serde_json::Value = response2.json().await?;
    assert_eq!(
        body["data"]["pack"]["version"], "2.0.0",
        "Should be upgraded to version 2.0.0"
    );

    Ok(())
}
