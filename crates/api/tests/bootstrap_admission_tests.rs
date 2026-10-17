use attune_common::{
    repositories::{
        pack_release::{CreatePackReleaseInput, PackReleaseRepository},
        platform_catalog::PlatformCatalogRepository,
        rule_authority::RuleAuthorityRepository,
        runtime_authority::RuntimeAuthorityRepository,
    },
    runtime_authority::RuntimePolicy,
};
use axum::http::StatusCode;
use serde_json::json;
mod helpers;
use helpers::*;

#[tokio::test]
async fn bootstrap_requires_admitted_core_but_not_workers_and_rejects_catalog_skew() -> Result<()> {
    let ctx = TestContext::new().await?;
    PlatformCatalogRepository::reconcile(&ctx.pool).await?;
    assert_eq!(
        ctx.get("/health/ready", None).await?.status(),
        StatusCode::OK
    );
    let descriptor: serde_json::Value = ctx.get("/health/ready", None).await?.json().await?;
    assert_eq!(descriptor["bootstrap_admission"]["contract_version"], 1);
    assert_eq!(descriptor["bootstrap_admission"]["client_version"], 1);
    assert_eq!(descriptor["bootstrap_admission"]["enabled"], false);
    assert_eq!(
        descriptor["bootstrap_admission_protocol"],
        serde_json::Value::Null
    );
    assert_eq!(
        descriptor["pack_publication"],
        json!({
            "protocol_version": 1,
            "client_version": 1,
            "revision": "authenticated-pack-publication-1",
            "authorization": "registrar",
            "components": ["immutable_release", "atomic_deployment"],
        })
    );
    assert!(descriptor["bootstrap_admission"]["components"]
        .as_array()
        .unwrap()
        .contains(&json!("sensor_metadata")));
    assert_eq!(
        ctx.get("/health/bootstrap", None).await?.status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
    let pack = create_test_pack(&ctx.pool, "core").await?;
    activate_test_pack_release(&ctx.pool, &pack).await?;
    assert_eq!(
        ctx.get("/health/bootstrap", None).await?.status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
    let release = PackReleaseRepository::find_active_by_pack_ref(&ctx.pool, "core")
        .await?
        .unwrap();
    let mut tx = ctx.pool.begin().await?;
    RuleAuthorityRepository::capture_deployment(&mut tx, pack.id).await?;
    let owner = RuntimeAuthorityRepository::find_owner(&mut tx, pack.id)
        .await?
        .unwrap();
    let revision = RuntimeAuthorityRepository::capture_policy(
        &mut tx,
        &RuntimePolicy {
            owner,
            release_id: release.id,
            release_digest: release.digest.clone(),
            invocations: vec![],
            concrete_targets: Default::default(),
        },
    )
    .await?;
    RuntimeAuthorityRepository::activate_policy(&mut tx, revision.id).await?;
    tx.commit().await?;
    let response = ctx.get("/health/bootstrap", None).await?;
    assert_eq!(response.status(), StatusCode::OK);
    let ready: serde_json::Value = response.json().await?;
    assert_eq!(ready["core"]["release_id"], release.id);
    assert_eq!(ready["core"]["policy_revision"], revision.id);
    // Worker readiness remains a separate application-content gate.
    assert_eq!(
        ctx.get("/health/content", None).await?.status(),
        StatusCode::SERVICE_UNAVAILABLE
    );

    // A failed upgrade rolls back release and admission in the same transaction.
    let mut upgrade = ctx.pool.begin().await?;
    let replacement = PackReleaseRepository::create_or_get(
        &mut upgrade,
        CreatePackReleaseInput {
            pack: pack.id,
            pack_ref: "core".into(),
            version: "2.0.0".into(),
            digest: "b".repeat(64),
            object_key: "packs/blobs/core-upgrade.tar.gz".into(),
            provider_version: "e:upgrade".into(),
            content_path: "/packs/.releases/core-upgrade".into(),
            archive_size: 1,
            manifest: json!({}),
        },
    )
    .await?;
    PackReleaseRepository::activate(&mut upgrade, pack.id, replacement.id).await?;
    upgrade.rollback().await?;
    let response = ctx.get("/health/bootstrap", None).await?;
    assert_eq!(response.status(), StatusCode::OK);
    let ready: serde_json::Value = response.json().await?;
    assert_eq!(ready["core"]["release_id"], release.id);
    assert_eq!(ready["core"]["policy_revision"], revision.id);

    sqlx::query("UPDATE platform_catalog_state SET revision = 0 WHERE singleton")
        .execute(&ctx.pool)
        .await?;
    assert_eq!(
        ctx.get("/health/bootstrap", None).await?.status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
    ctx.cleanup().await?;
    Ok(())
}
