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
        identity::{
            CreatePermissionAssignmentInput, CreatePermissionSetInput, IdentityRepository,
            PermissionAssignmentRepository, PermissionSetRepository,
        },
        pack::{CreatePackInput, PackRepository},
        Create, FindById, FindByRef, List,
    },
};
use helpers::{Result, TestContext};
use serde_json::json;
use std::fs;
use std::time::Duration;
use tempfile::TempDir;

const STANDARD_INDEX_URL: &str = attune_common::pack_registry::STANDARD_PACK_INDEX_URL;

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

async fn register_pack_index_user(ctx: &TestContext, grants: serde_json::Value) -> Result<String> {
    let login = format!("pack_index_{}", uuid::Uuid::new_v4().simple());
    let response = ctx
        .post(
            "/auth/register",
            json!({
                "login": login,
                "password": "TestPassword123!",
                "display_name": "Pack index authorization test",
            }),
            None,
        )
        .await?;
    assert!(response.status().is_success());
    let body: serde_json::Value = response.json().await?;
    let token = body["data"]["access_token"]
        .as_str()
        .expect("registration access token")
        .to_string();
    let identity = IdentityRepository::find_by_login(&ctx.pool, &login)
        .await?
        .expect("registered identity");
    let permission_set = PermissionSetRepository::create(
        &ctx.pool,
        CreatePermissionSetInput {
            r#ref: format!("test.pack_index_{}", uuid::Uuid::new_v4().simple()),
            pack: None,
            pack_ref: None,
            label: Some("Pack index authorization test".to_string()),
            description: None,
            grants,
        },
    )
    .await?;
    PermissionAssignmentRepository::create(
        &ctx.pool,
        CreatePermissionAssignmentInput {
            identity: identity.id,
            permset: permission_set.id,
        },
    )
    .await?;
    attune_api::authz::AuthorizationService::invalidate_identity_authz_cache(identity.id).await;
    attune_api::authz::AuthorizationService::invalidate_permission_set_caches().await;
    Ok(token)
}

#[tokio::test]
#[ignore = "integration test - requires database"]
async fn constrained_pack_grants_cannot_read_or_administer_global_pack_indices() -> Result<()> {
    let ctx = TestContext::new().await?;
    let token = register_pack_index_user(
        &ctx,
        json!([
            {"resource": "packs", "actions": ["read"], "constraints": {"pack_refs": ["core"]}},
            {"resource": "packs", "actions": ["configure"], "constraints": {"owner": "any"}},
            {"resource": "packs", "actions": ["configure"], "constraints": {"owner": "none"}},
            {"resource": "packs", "actions": ["configure"], "constraints": {"owner": "self"}},
            {"resource": "packs", "actions": ["configure"], "constraints": {"pack_refs": ["core"]}},
            {"resource": "packs", "actions": ["configure"], "constraints": {"owner_types": ["pack"]}},
            {"resource": "packs", "actions": ["configure"], "constraints": {"owner_refs": ["core"]}},
            {"resource": "packs", "actions": ["configure"], "constraints": {"refs": ["core"]}},
            {"resource": "packs", "actions": ["configure"], "constraints": {"ids": [1]}},
            {"resource": "packs", "actions": ["configure"], "constraints": {"attributes": {"team": "platform"}}}
        ]),
    )
    .await?;

    let standard_id: i64 = sqlx::query_scalar("SELECT id FROM pack_registry_index WHERE url = $1")
        .bind(STANDARD_INDEX_URL)
        .fetch_one(&ctx.pool)
        .await?;

    let list = ctx.get("/api/v1/pack-indices", Some(&token)).await?;
    assert_eq!(list.status(), axum::http::StatusCode::FORBIDDEN);
    let browse = ctx.get("/api/v1/pack-indices/packs", Some(&token)).await?;
    assert_eq!(browse.status(), axum::http::StatusCode::FORBIDDEN);
    let get = ctx
        .get("/api/v1/pack-indices/packs/core", Some(&token))
        .await?;
    assert_eq!(get.status(), axum::http::StatusCode::FORBIDDEN);

    let include_disabled = ctx
        .get(
            "/api/v1/pack-indices/packs?include_disabled=true",
            Some(&token),
        )
        .await?;
    assert_eq!(include_disabled.status(), axum::http::StatusCode::FORBIDDEN);
    let create = ctx
        .post(
            "/api/v1/pack-indices",
            json!({"url": "https://raw.githubusercontent.com/example/index.json"}),
            Some(&token),
        )
        .await?;
    assert_eq!(create.status(), axum::http::StatusCode::FORBIDDEN);
    let update = ctx
        .put(
            &format!("/api/v1/pack-indices/{standard_id}"),
            json!({"name": "Unauthorized update"}),
            Some(&token),
        )
        .await?;
    assert_eq!(update.status(), axum::http::StatusCode::FORBIDDEN);
    let delete = ctx
        .delete(&format!("/api/v1/pack-indices/{standard_id}"), Some(&token))
        .await?;
    assert_eq!(delete.status(), axum::http::StatusCode::FORBIDDEN);

    let mut denial_details: Option<serde_json::Value> = None;
    for _ in 0..40 {
        denial_details = sqlx::query_scalar(
            "SELECT details FROM audit_event WHERE event_type = 'rbac.denied' AND details ->> 'scope' = 'global_pack_index' ORDER BY created DESC LIMIT 1",
        )
        .fetch_optional(&ctx.pool)
        .await?;
        if denial_details.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(
        denial_details.expect("global pack index denial audit")["reason"],
        "unconstrained_grant_required"
    );

    Ok(())
}

#[tokio::test]
#[ignore = "integration test - requires database"]
async fn pack_index_update_waits_for_the_mutation_advisory_lock() -> Result<()> {
    let ctx = TestContext::new().await?.with_admin_auth().await?;
    let standard_id: i64 = sqlx::query_scalar("SELECT id FROM pack_registry_index WHERE url = $1")
        .bind(STANDARD_INDEX_URL)
        .fetch_one(&ctx.pool)
        .await?;
    let mut lock_tx = ctx.pool.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind("pack_registry_index_mutation")
        .execute(&mut *lock_tx)
        .await?;

    let blocked = tokio::time::timeout(
        Duration::from_millis(200),
        ctx.put(
            &format!("/api/v1/pack-indices/{standard_id}"),
            json!({"name": "Serialized update"}),
            None,
        ),
    )
    .await;
    assert!(
        blocked.is_err(),
        "update bypassed the mutation advisory lock"
    );
    lock_tx.rollback().await?;

    let response = ctx
        .put(
            &format!("/api/v1/pack-indices/{standard_id}"),
            json!({"name": "Serialized update"}),
            None,
        )
        .await?;
    assert_eq!(response.status(), axum::http::StatusCode::OK);

    Ok(())
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn standard_index_without_headers_does_not_require_encryption_key() -> Result<()> {
    let ctx = TestContext::new_without_registry_encryption_key()
        .await?
        .with_admin_auth()
        .await?;

    let response = ctx.get("/api/v1/pack-indices", None).await?;
    assert!(response.status().is_success());
    let body: serde_json::Value = response.json().await?;
    let standard = body["data"]
        .as_array()
        .and_then(|indices| {
            indices
                .iter()
                .find(|index| index["url"] == STANDARD_INDEX_URL)
        })
        .expect("standard index response");
    assert_eq!(standard["headers"], json!({}));

    let persisted: serde_json::Value =
        sqlx::query_scalar("SELECT headers FROM pack_registry_index WHERE url = $1")
            .bind(STANDARD_INDEX_URL)
            .fetch_one(&ctx.pool)
            .await?;
    assert_eq!(persisted, json!({}));

    Ok(())
}

#[tokio::test]
#[ignore = "integration test - requires database"]
async fn successful_pack_index_mutations_emit_redacted_audits() -> Result<()> {
    let ctx = TestContext::new().await?.with_admin_auth().await?;
    let create_secret = "create-registry-secret";
    let update_secret = "update-registry-secret";
    let response = ctx
        .post(
            "/api/v1/pack-indices",
            json!({
                "name": "Audit registry",
                "url": "https://raw.githubusercontent.com/attune-system/index/audit-test/index.json",
                "headers": {"Authorization": create_secret}
            }),
            None,
        )
        .await?;
    assert_eq!(response.status(), axum::http::StatusCode::CREATED);
    let body: serde_json::Value = response.json().await?;
    let id = body["data"]["id"].as_i64().expect("created registry id");
    assert_eq!(body["data"]["headers"]["Authorization"], "[REDACTED]");

    let response = ctx
        .put(
            &format!("/api/v1/pack-indices/{id}"),
            json!({
                "name": "Updated audit registry",
                "headers": {
                    "Authorization": "[REDACTED]",
                    "X-Api-Key": update_secret
                }
            }),
            None,
        )
        .await?;
    assert_eq!(response.status(), axum::http::StatusCode::OK);
    let response = ctx
        .delete(&format!("/api/v1/pack-indices/{id}"), None)
        .await?;
    assert_eq!(response.status(), axum::http::StatusCode::OK);

    ctx.flush_audit().await?;
    let events: Vec<(String, serde_json::Value)> = sqlx::query_as(
        r#"
        SELECT event_type, details
        FROM audit_event
        WHERE resource_type = 'pack_registry_index' AND resource_id = $1
        ORDER BY created, id
        "#,
    )
    .bind(id)
    .fetch_all(&ctx.pool)
    .await?;
    assert_eq!(
        events
            .iter()
            .map(|event| event.0.as_str())
            .collect::<Vec<_>>(),
        vec![
            "pack.registry_index.created",
            "pack.registry_index.updated",
            "pack.registry_index.deleted"
        ]
    );
    for (_, details) in events {
        let serialized = details.to_string();
        assert!(!serialized.contains(create_secret));
        assert!(!serialized.contains(update_secret));
        assert!(details.get("headers").is_none());
        assert_eq!(details["headers_configured"], true);
    }

    Ok(())
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn registry_id_only_loads_the_selected_enabled_managed_row() -> Result<()> {
    let ctx = TestContext::new_without_registry_encryption_key()
        .await?
        .with_admin_auth()
        .await?;
    let selected_id: i64 = sqlx::query_scalar(
        r#"
        INSERT INTO pack_registry_index (name, url, position, enabled, headers)
        VALUES ('Selected', 'https://registry.attune.example.com/index.json', 10, TRUE, '{}'::jsonb)
        RETURNING id
        "#,
    )
    .fetch_one(&ctx.pool)
    .await?;
    sqlx::query(
        r#"
        INSERT INTO pack_registry_index (name, url, position, enabled, headers)
        VALUES ('Unrelated broken secret', 'not-a-valid-url', 11, TRUE, '"not-ciphertext"'::jsonb)
        "#,
    )
    .execute(&ctx.pool)
    .await?;

    let response = ctx
        .post(
            "/api/v1/packs/install",
            json!({
                "source": "definitely_missing_pack",
                "registry_id": selected_id,
                "skip_tests": true,
                "skip_deps": true
            }),
            None,
        )
        .await?;
    let body = response.text().await?;
    assert!(!body.contains("encryption_key"), "{body}");
    assert!(!body.contains("not-a-valid-url"), "{body}");

    let browse = ctx
        .get(
            &format!("/api/v1/pack-indices/packs?registry_id={selected_id}"),
            None,
        )
        .await?;
    let browse_body = browse.text().await?;
    assert!(!browse_body.contains("encryption_key"), "{browse_body}");
    assert!(!browse_body.contains("not-a-valid-url"), "{browse_body}");

    let disabled_id: i64 = sqlx::query_scalar(
        r#"
        INSERT INTO pack_registry_index (name, url, position, enabled, headers)
        VALUES ('Disabled', 'https://registry.attune.example.com/disabled.json', 12, FALSE, '{}'::jsonb)
        RETURNING id
        "#,
    )
    .fetch_one(&ctx.pool)
    .await?;
    let disabled = ctx
        .post(
            "/api/v1/packs/install",
            json!({"source": "missing", "registry_id": disabled_id}),
            None,
        )
        .await?;
    assert_eq!(disabled.status(), axum::http::StatusCode::BAD_REQUEST);
    let unknown = ctx
        .post(
            "/api/v1/packs/install",
            json!({"source": "missing", "registry_id": i64::MAX}),
            None,
        )
        .await?;
    assert_eq!(unknown.status(), axum::http::StatusCode::BAD_REQUEST);

    Ok(())
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn disabled_registry_preserves_outbound_host_denial_for_direct_installs() -> Result<()> {
    let ctx = TestContext::new_with_disabled_pack_registry()
        .await?
        .with_admin_auth()
        .await?;

    let response = ctx
        .post(
            "/api/v1/packs/install",
            json!({"source": "https://github.com/attune-packs/ansible.git"}),
            None,
        )
        .await?;
    assert!(!response.status().is_success());
    let body: serde_json::Value = response.json().await?;
    assert!(body.to_string().contains("not explicitly approved"));

    Ok(())
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn direct_remote_install_requires_explicit_deployment_opt_in() -> Result<()> {
    let ctx = TestContext::new().await?.with_admin_auth().await?;

    let response = ctx
        .post(
            "/api/v1/packs/install",
            json!({"source": "https://github.com/attacker/pack.git"}),
            None,
        )
        .await?;
    assert_eq!(response.status(), axum::http::StatusCode::BAD_REQUEST);
    let body: serde_json::Value = response.json().await?;
    assert!(body
        .to_string()
        .contains("allow_unverified_direct_remote_installs"));

    Ok(())
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn direct_remote_query_credentials_are_rejected_without_echo() -> Result<()> {
    let ctx = TestContext::new_with_unverified_direct_remote_installs()
        .await?
        .with_admin_auth()
        .await?;

    let response = ctx
        .post(
            "/api/v1/packs/install",
            json!({"source": "https://github.com/attacker/pack.git?token=super-secret"}),
            None,
        )
        .await?;
    assert_eq!(response.status(), axum::http::StatusCode::BAD_REQUEST);
    let body = response.text().await?;
    assert!(!body.contains("super-secret"));

    Ok(())
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
    let provenance = &body["data"]["provenance"];
    assert_eq!(provenance["artifact_type"], "local_directory");
    assert_eq!(provenance["artifact_url"], pack_path);
    assert_eq!(provenance["registry_id"], serde_json::Value::Null);
    assert_eq!(provenance["registry_url"], serde_json::Value::Null);
    assert_eq!(provenance["fallback_occurred"], false);
    assert_eq!(provenance["checksum_verified"], false);

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
    let canonical_checksum = format!("sha256:{original_checksum}");
    assert_eq!(stored_checksum, &canonical_checksum);
    assert_eq!(
        pack.installers["installation_provenance"]["artifact_url"],
        pack_path
    );
    assert_eq!(
        pack.installers["installation_provenance"]["checksum"],
        canonical_checksum
    );

    let mut audit_details = None;
    for _ in 0..40 {
        audit_details = sqlx::query_scalar(
            "SELECT details FROM audit_event WHERE event_type = 'pack.installed' AND resource_id = $1 ORDER BY created DESC LIMIT 1",
        )
        .bind(pack_id)
        .fetch_optional(&ctx.pool)
        .await?;
        if audit_details.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let audit_details: serde_json::Value = audit_details.expect("pack install audit event");
    assert_eq!(
        audit_details["provenance"]["artifact_type"],
        "local_directory"
    );
    assert_eq!(audit_details["provenance"]["checksum"], canonical_checksum);
    assert!(audit_details.get("headers").is_none());

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
async fn install_only_permission_cannot_replace_existing_pack() -> Result<()> {
    let ctx = TestContext::new().await?.with_pack_install_auth().await?;
    let pack_dir = create_test_pack_dir("replacement_guard", "2.0.0")?;
    PackRepository::create(
        &ctx.pool,
        CreatePackInput {
            r#ref: "replacement_guard".to_string(),
            label: "Existing pack".to_string(),
            description: None,
            version: "1.0.0".to_string(),
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

    let response = ctx
        .post(
            "/api/v1/packs/install",
            json!({
                "source": pack_dir.path().to_string_lossy(),
                "force": true,
                "skip_tests": true
            }),
            None,
        )
        .await?;
    assert_eq!(response.status(), axum::http::StatusCode::FORBIDDEN);
    let persisted = PackRepository::find_by_ref(&ctx.pool, "replacement_guard")
        .await?
        .expect("existing pack");
    assert_eq!(persisted.version, "1.0.0");

    Ok(())
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn force_reinstall_preserves_ownerless_pack_ownership() -> Result<()> {
    let ctx = TestContext::new().await?.with_admin_auth().await?;
    let pack_dir = create_test_pack_dir("ownership_guard", "2.0.0")?;
    PackRepository::create(
        &ctx.pool,
        CreatePackInput {
            r#ref: "ownership_guard".to_string(),
            label: "Existing ownerless pack".to_string(),
            description: None,
            version: "1.0.0".to_string(),
            conf_schema: json!({}),
            config: json!({}),
            meta: json!({}),
            tags: Vec::new(),
            runtime_deps: Vec::new(),
            dependencies: Vec::new(),
            is_standard: false,
            installers: json!({"custom_installer": {"enabled": true}}),
        },
    )
    .await?;

    let response = ctx
        .post(
            "/api/v1/packs/install",
            json!({
                "source": pack_dir.path().to_string_lossy(),
                "force": true,
                "skip_tests": true
            }),
            None,
        )
        .await?;
    assert!(response.status().is_success());
    let persisted = PackRepository::find_by_ref(&ctx.pool, "ownership_guard")
        .await?
        .expect("reinstalled pack");
    assert_eq!(persisted.version, "2.0.0");
    assert_eq!(persisted.installed_by, None);
    assert_eq!(persisted.installers["custom_installer"]["enabled"], true);
    assert!(persisted.installers["installation_provenance"].is_object());

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
