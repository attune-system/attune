//! Deployment diagnostics against an owned API/database fixture.
mod helpers;

use attune_common::repositories::{pack::PackRepository, FindByRef, PackInstallRepository};
use axum::http::StatusCode;
use helpers::{Result, TestContext};
use serde_json::json;

#[tokio::test]
async fn deployment_syntax_errors_identify_manifest_without_exposing_values() -> Result<()> {
    let ctx = TestContext::new().await?.with_admin_auth().await?;
    let source = tempfile::tempdir()?;
    std::fs::write(source.path().join("pack.yaml"), "ref: [SECRET_CANARY\n")?;
    let response = ctx
        .post(
            "/api/v1/packs/register",
            json!({"path": source.path(), "skip_tests": true}),
            ctx.token(),
        )
        .await?;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body: serde_json::Value = response.json().await?;
    assert_eq!(body["details"]["diagnostics"][0]["path"], "pack.yaml");
    assert!(body["details"]["diagnostics"][0]["line"].is_number());
    assert!(!body.to_string().contains("SECRET_CANARY"));
    ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn deployment_metadata_errors_have_authored_file_and_field() -> Result<()> {
    let ctx = TestContext::new().await?.with_admin_auth().await?;
    let source = tempfile::tempdir()?;
    std::fs::write(
        source.path().join("pack.yaml"),
        "ref: diagnostic_fields\nversion: 1.0.0\n",
    )?;
    std::fs::create_dir(source.path().join("actions"))?;
    std::fs::write(
        source.path().join("actions/run.yaml"),
        "ref: diagnostic_fields.run\nrunner_type: native\nentry_point: run\ntimeout_seconds: 0\n",
    )?;
    let response = ctx
        .post(
            "/api/v1/packs/register",
            json!({"path": source.path(), "skip_tests": true}),
            ctx.token(),
        )
        .await?;
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let body: serde_json::Value = response.json().await?;
    assert!(body["details"]["diagnostics"]
        .as_array()
        .unwrap()
        .iter()
        .any(|diagnostic| diagnostic["path"] == "actions/run.yaml"
            && diagnostic["metadata_path"] == "/timeout_seconds"));
    assert!(PackRepository::find_by_ref(&ctx.pool, "diagnostic_fields")
        .await?
        .is_none());
    ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn deployment_activation_history_retains_the_actual_component_failure() -> Result<()> {
    let ctx = TestContext::new().await?.with_pack_install_auth().await?;
    attune_common::repositories::platform_catalog::PlatformCatalogRepository::reconcile(&ctx.pool)
        .await?;
    let source = tempfile::tempdir()?;
    std::fs::write(
        source.path().join("pack.yaml"),
        "ref: diagnostic_history\nversion: 1.0.0\n",
    )?;
    std::fs::create_dir(source.path().join("actions"))?;
    std::fs::write(source.path().join("actions/run.yaml"), "ref: diagnostic_history.run\nrunner_type: shell\nentry_point: run.sh\nlog_retention_policy: not_a_policy\n")?;
    std::fs::write(source.path().join("actions/run.sh"), "#!/bin/sh\nexit 0\n")?;
    let response = ctx
        .post(
            "/api/v1/packs/register",
            json!({"path": source.path(), "skip_tests": true}),
            ctx.token(),
        )
        .await?;
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let body: serde_json::Value = response.json().await?;
    let id = body["details"]["install_id"]
        .as_i64()
        .expect("activation attempt identity");
    let install = PackInstallRepository::new(ctx.pool.clone())
        .find_by_id(id)
        .await?
        .unwrap();
    assert_eq!(install.status, "failed");
    assert_eq!(install.diagnostic_details, Some(body["details"].clone()));
    assert_eq!(install.error_message.as_deref(), body["error"].as_str());
    assert_ne!(
        install.error_message.as_deref(),
        Some("Pack activation was interrupted")
    );
    assert!(PackRepository::find_by_ref(&ctx.pool, "diagnostic_history")
        .await?
        .is_none());
    ctx.cleanup().await?;
    Ok(())
}
