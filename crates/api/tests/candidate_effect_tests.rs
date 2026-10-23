use attune_common::{
    auth::generate_integration_token,
    blob_store::{ObjectKey, ProviderVersion, StoredObject},
    models::PackInstallStatus,
    repositories::{
        candidate_effect::{
            CandidateArchiveBinding, CandidateEffectLease, CandidateEffectRepository,
        },
        object_maintenance::ObjectMaintenanceRepository,
        PackInstallRepository,
    },
};
use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use serde_json::{json, Value};
use tower::ServiceExt;
mod helpers;
use helpers::{Result, TestContext};

async fn context(ctx: &TestContext) -> Result<(i64, CandidateEffectLease, String)> {
    let repo = PackInstallRepository::new(ctx.pool.clone());
    let install = repo
        .create("candidate", "1.0.0", "install", None, None)
        .await?;
    let object = StoredObject {
        key: ObjectKey::new(format!("packs/candidates/{}.tar.gz", install.id))?,
        provider_version: ProviderVersion::from_stored("v:owned")?,
        size: 123,
        sha256: [0xab; 32],
    };
    let mut tx = ctx.pool.begin().await?;
    ObjectMaintenanceRepository::reserve_upload(&mut *tx, object.key.as_str(), "pack").await?;
    ObjectMaintenanceRepository::record_uploaded(
        &mut *tx,
        object.key.as_str(),
        object.provider_version.as_stored(),
        123,
    )
    .await?;
    assert!(PackInstallRepository::attach_candidate_object(&mut tx, install.id, &object).await?);
    tx.commit().await?;
    let archive = generate_integration_token()?;
    repo.claim_worker(install.id, 42, Some(&archive.hash))
        .await?
        .unwrap();
    let digest = "ab".repeat(32);
    let binding = CandidateArchiveBinding {
        key: object.key.as_str(),
        provider_version: object.provider_version.as_stored(),
        digest: &digest,
    };
    assert!(
        CandidateEffectRepository::open(&ctx.pool, install.id, 43, &archive.secret, &binding)
            .await
            .is_err()
    );
    assert!(
        CandidateEffectRepository::open(&ctx.pool, install.id, 42, "wrong-archive", &binding)
            .await
            .is_err()
    );
    assert!(CandidateEffectRepository::open(
        &ctx.pool,
        install.id,
        42,
        &archive.secret,
        &CandidateArchiveBinding {
            digest: &"cd".repeat(32),
            ..binding
        }
    )
    .await
    .is_err());
    for wrong in [
        CandidateArchiveBinding {
            key: "packs/candidates/wrong.tar.gz",
            ..binding
        },
        CandidateArchiveBinding {
            provider_version: "v:wrong",
            ..binding
        },
    ] {
        assert!(CandidateEffectRepository::open(
            &ctx.pool,
            install.id,
            42,
            &archive.secret,
            &wrong
        )
        .await
        .is_err());
    }
    let lease =
        CandidateEffectRepository::open(&ctx.pool, install.id, 42, &archive.secret, &binding)
            .await?;
    Ok((install.id, lease, archive.secret))
}

async fn effect(
    ctx: &TestContext,
    lease: &CandidateEffectLease,
    token: &str,
    body: Value,
) -> Result<(StatusCode, Value)> {
    let response = ctx
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri(format!("/api/v1/candidates/{}/effects", lease.id))
                .header("content-type", "application/json")
                .header("x-attune-candidate-effects", token)
                .body(Body::from(serde_json::to_vec(&body)?))?,
        )
        .await?;
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), 100_000).await?;
    Ok((
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    ))
}

#[tokio::test]
async fn real_candidate_writes_and_events_never_reach_production_or_notifications() -> Result<()> {
    let ctx = TestContext::new().await?;
    use attune_common::repositories::{
        rule::{CreateRuleInput, RuleRepository},
        trigger::{CreateTriggerInput, TriggerRepository},
        Create,
    };
    let pack = helpers::create_test_pack(&ctx.pool, "candidate").await?;
    let trigger = TriggerRepository::create(
        &ctx.pool,
        CreateTriggerInput {
            r#ref: "candidate.isolated".into(),
            pack: Some(pack.id),
            pack_ref: Some("candidate".into()),
            label: "Production target with the same ref".into(),
            description: None,
            enabled: true,
            param_schema: None,
            event_config_schema: None,
            out_schema: None,
            sensor: None,
            sensor_ref: None,
            is_adhoc: false,
            reference_visibility: Default::default(),
            reference_allowed_pack_refs: vec![],
        },
    )
    .await?;
    let action = helpers::create_test_action(
        &ctx.pool,
        pack.id,
        "candidate",
        "candidate.production_action",
    )
    .await?;
    let rule = RuleRepository::create(
        &ctx.pool,
        CreateRuleInput {
            r#ref: "candidate.production_rule".into(),
            pack: pack.id,
            pack_ref: "candidate".into(),
            label: "Active production rule".into(),
            description: None,
            action: action.id,
            action_ref: action.r#ref,
            trigger: trigger.id,
            trigger_ref: trigger.r#ref,
            conditions: json!({}),
            action_params: json!({}),
            trigger_params: json!({}),
            trace_tag_template: None,
            permission_set_refs: None,
            enabled: true,
            is_adhoc: false,
            owner_identity: None,
        },
    )
    .await?;
    let (_install, lease, archive) = context(&ctx).await?;
    assert_eq!(
        ctx.get("/api/v1/keys", Some(&lease.token)).await?.status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        ctx.post("/auth/token-login", json!({"token":lease.token}), None)
            .await?
            .status(),
        StatusCode::UNAUTHORIZED
    );
    let mut listener = sqlx::postgres::PgListener::connect_with(&ctx.pool).await?;
    listener
        .listen_all(["event_created", "artifact_created", "execution_created"])
        .await?;
    let events_before: i64 = sqlx::query_scalar("SELECT count(*) FROM event")
        .fetch_one(&ctx.pool)
        .await?;
    let keys_before: i64 = sqlx::query_scalar("SELECT count(*) FROM key")
        .fetch_one(&ctx.pool)
        .await?;
    let put = json!({"operation":"put_key", "target":"candidate.local", "value":{"message":"real database write"}});
    assert_eq!(
        effect(&ctx, &lease, &archive, put.clone()).await?.0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        effect(&ctx, &lease, &lease.token, put).await?.0,
        StatusCode::OK
    );
    let (_, read) = effect(
        &ctx,
        &lease,
        &lease.token,
        json!({"operation":"get_key", "target":"candidate.local"}),
    )
    .await?;
    assert_eq!(read["data"]["message"], "real database write");
    assert_eq!(
        effect(
            &ctx,
            &lease,
            &lease.token,
            json!({"operation":"emit_event", "target":"candidate.production", "payload":{}})
        )
        .await?
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        effect(
            &ctx,
            &lease,
            &lease.token,
            json!({"operation":"put_trigger", "target":"core.production"})
        )
        .await?
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        effect(
            &ctx,
            &lease,
            &lease.token,
            json!({"operation":"put_trigger", "target":"candidate.isolated"})
        )
        .await?
        .0,
        StatusCode::OK
    );
    assert_eq!(effect(&ctx, &lease, &lease.token, json!({"operation":"emit_event", "target":"candidate.isolated", "payload":{"real":true}})).await?.0, StatusCode::OK);
    let (_, events) = effect(
        &ctx,
        &lease,
        &lease.token,
        json!({"operation":"list_events"}),
    )
    .await?;
    assert_eq!(events["data"][0]["payload"]["real"], true);
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM event")
            .fetch_one(&ctx.pool)
            .await?,
        events_before
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM key")
            .fetch_one(&ctx.pool)
            .await?,
        keys_before
    );
    // A notification sent after the committed effects is an ordering barrier, not a sleep.
    sqlx::query("SELECT pg_notify('event_created', 'candidate-test-barrier')")
        .execute(&ctx.pool)
        .await?;
    let notification =
        tokio::time::timeout(std::time::Duration::from_secs(5), listener.recv()).await??;
    assert_eq!(notification.payload(), "candidate-test-barrier");
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM enforcement WHERE rule = $1")
            .bind(rule.id)
            .fetch_one(&ctx.pool)
            .await?,
        0
    );
    assert!(
        sqlx::query_scalar::<_, bool>("SELECT enabled FROM trigger WHERE id = $1")
            .bind(trigger.id)
            .fetch_one(&ctx.pool)
            .await?
    );
    drop(listener);
    CandidateEffectRepository::close(&ctx.pool, lease.id).await?;
    ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn finished_failed_and_expired_candidates_have_no_reusable_authority() -> Result<()> {
    let ctx = TestContext::new().await?;
    for status in [
        PackInstallStatus::Succeeded,
        PackInstallStatus::Failed,
        PackInstallStatus::Activating,
    ] {
        let (install, lease, archive) = context(&ctx).await?;
        PackInstallRepository::new(ctx.pool.clone())
            .finish_running(install, status, None, None, None)
            .await?
            .unwrap();
        let mut activation = ctx.pool.begin().await?;
        let evidence = CandidateEffectRepository::require_closed_context(
            &mut activation,
            install,
            &CandidateArchiveBinding {
                key: &format!("packs/candidates/{install}.tar.gz"),
                provider_version: "v:owned",
                digest: &"ab".repeat(32),
            },
        )
        .await;
        assert_eq!(evidence.is_ok(), status == PackInstallStatus::Activating);
        activation.rollback().await?;
        assert_eq!(
            effect(
                &ctx,
                &lease,
                &lease.token,
                json!({"operation":"put_key", "target":"candidate.local", "value":"denied"})
            )
            .await?
            .0,
            StatusCode::FORBIDDEN
        );
        assert!(CandidateEffectRepository::open(
            &ctx.pool,
            install,
            42,
            &archive,
            &CandidateArchiveBinding {
                key: &format!("packs/candidates/{install}.tar.gz"),
                provider_version: "v:owned",
                digest: &"ab".repeat(32)
            }
        )
        .await
        .is_err());
    }
    let (_, lease, _) = context(&ctx).await?;
    sqlx::query("UPDATE pack_candidate_context SET expires_at = clock_timestamp() - INTERVAL '1 second' WHERE lease = $1").bind(lease.id).execute(&ctx.pool).await?;
    assert_eq!(
        effect(
            &ctx,
            &lease,
            &lease.token,
            json!({"operation":"list_events"})
        )
        .await?
        .0,
        StatusCode::FORBIDDEN
    );
    CandidateEffectRepository::close(&ctx.pool, lease.id).await?;
    ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn candidate_contexts_are_separate_and_close_purges_effects() -> Result<()> {
    let ctx = TestContext::new().await?;
    let (_, first, _) = context(&ctx).await?;
    let (_, second, _) = context(&ctx).await?;
    assert_eq!(
        effect(
            &ctx,
            &second,
            &first.token,
            json!({"operation":"put_key", "target":"candidate.local", "value":1})
        )
        .await?
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        effect(
            &ctx,
            &first,
            &first.token,
            json!({"operation":"put_trigger", "target":"candidate.local"})
        )
        .await?
        .0,
        StatusCode::OK
    );
    assert_eq!(
        effect(
            &ctx,
            &second,
            &second.token,
            json!({"operation":"emit_event", "target":"candidate.local", "payload":{}})
        )
        .await?
        .0,
        StatusCode::FORBIDDEN
    );
    CandidateEffectRepository::close(&ctx.pool, first.id).await?;
    assert_eq!(
        effect(
            &ctx,
            &first,
            &first.token,
            json!({"operation":"list_events"})
        )
        .await?
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM pack_candidate_resource WHERE context = $1"
        )
        .bind(first.id)
        .fetch_one(&ctx.pool)
        .await?,
        0
    );
    CandidateEffectRepository::close(&ctx.pool, second.id).await?;
    ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn candidate_archive_rebinding_budget_and_expiry_fail_closed() -> Result<()> {
    let ctx = TestContext::new().await?;
    let (install, lease, _) = context(&ctx).await?;
    sqlx::query("UPDATE pack_install SET candidate_digest = $2 WHERE id = $1")
        .bind(install)
        .bind("cd".repeat(32))
        .execute(&ctx.pool)
        .await?;
    assert_eq!(
        effect(
            &ctx,
            &lease,
            &lease.token,
            json!({"operation":"list_events"})
        )
        .await?
        .0,
        StatusCode::FORBIDDEN
    );
    CandidateEffectRepository::close(&ctx.pool, lease.id).await?;
    let (_, lease, _) = context(&ctx).await?;
    sqlx::query("UPDATE pack_candidate_context SET operations = 1024 WHERE lease = $1")
        .bind(lease.id)
        .execute(&ctx.pool)
        .await?;
    assert_eq!(
        effect(
            &ctx,
            &lease,
            &lease.token,
            json!({"operation":"list_events"})
        )
        .await?
        .0,
        StatusCode::FORBIDDEN
    );
    sqlx::query("UPDATE pack_candidate_context SET expires_at = clock_timestamp() - INTERVAL '1 second' WHERE lease = $1")
        .bind(lease.id).execute(&ctx.pool).await?;
    assert_eq!(
        CandidateEffectRepository::purge_expired(&ctx.pool, 100).await?,
        1
    );
    assert!(
        sqlx::query("UPDATE pack_candidate_context SET revoked = FALSE WHERE lease = $1")
            .bind(lease.id)
            .execute(&ctx.pool)
            .await
            .is_err()
    );
    ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn candidate_finish_committed_before_effect_admission_denies_the_waiting_write() -> Result<()>
{
    let ctx = TestContext::new().await?;
    let (install, lease, _) = context(&ctx).await?;
    let mut finish = ctx.pool.begin().await?;
    sqlx::query("SELECT id FROM pack_install WHERE id = $1 FOR UPDATE")
        .bind(install)
        .execute(&mut *finish)
        .await?;
    let app = ctx.app.clone();
    let request = Request::builder()
        .method("POST")
        .uri(format!("/api/v1/candidates/{}/effects", lease.id))
        .header("content-type", "application/json")
        .header("x-attune-candidate-effects", &lease.token)
        .body(Body::from(serde_json::to_vec(
            &json!({"operation":"put_key", "target":"candidate.race", "value":"must not commit"}),
        )?))?;
    let mut waiting = tokio::spawn(async move { app.oneshot(request).await.unwrap().status() });
    // Synchronize on the actual repository's database lock wait, not elapsed time.
    let barrier = tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            let blocked: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM pg_stat_activity WHERE datname = current_database() AND wait_event_type = 'Lock' AND query LIKE 'SELECT p.pack_ref FROM pack_install p JOIN pack_candidate_context%')")
                .fetch_one(&ctx.pool).await.unwrap();
            if blocked { break; }
            tokio::task::yield_now().await;
        }
    }).await;
    if let Err(error) = barrier {
        waiting.abort();
        let _ = waiting.await;
        finish.rollback().await?;
        ctx.cleanup().await?;
        return Err(error.into());
    }
    sqlx::query("UPDATE pack_install SET status = 'failed' WHERE id = $1")
        .bind(install)
        .execute(&mut *finish)
        .await?;
    finish.commit().await?;
    let result = tokio::time::timeout(std::time::Duration::from_secs(10), &mut waiting).await;
    let status = match result {
        Ok(status) => status?,
        Err(error) => {
            waiting.abort();
            let _ = waiting.await;
            ctx.cleanup().await?;
            return Err(error.into());
        }
    };
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert_eq!(
        sqlx::query_scalar::<_, i64>(
            "SELECT count(*) FROM pack_candidate_resource WHERE context = $1"
        )
        .bind(lease.id)
        .fetch_one(&ctx.pool)
        .await?,
        0
    );
    ctx.cleanup().await?;
    Ok(())
}
#[tokio::test]
async fn candidate_require_auth_authenticates_before_body_and_counts_only_effects() -> Result<()> {
    let ctx = TestContext::new().await?.with_auth().await?;
    let (install, lease, _) = context(&ctx).await?;
    let invalid_body = json!({"operation":"not_an_operation"});
    // A parsing-only guard would reach JSON extraction and return 422 here.
    assert_eq!(
        effect(&ctx, &lease, "arbitrary-header", invalid_body.clone())
            .await?
            .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        effect(&ctx, &lease, &lease.token, invalid_body).await?.0,
        StatusCode::UNPROCESSABLE_ENTITY
    );
    use attune_common::auth::jwt::{generate_token, TokenType};
    let user = ctx.user.as_ref().unwrap();
    for kind in [
        TokenType::Access,
        TokenType::Sensor,
        TokenType::Worker,
        TokenType::Execution,
    ] {
        let jwt = generate_token(user.id, &user.login, &ctx.state.jwt_config, kind)?;
        let jwt_only = ctx
            .app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(format!("/api/v1/candidates/{}/effects", lease.id))
                    .header("authorization", format!("Bearer {jwt}"))
                    .header("content-type", "application/json")
                    .body(Body::from("{\"operation\":\"list_events\"}"))?,
            )
            .await?;
        assert_eq!(jwt_only.status(), StatusCode::UNAUTHORIZED);
        assert!(matches!(
            effect(&ctx, &lease, &jwt, json!({"operation":"list_events"}))
                .await?
                .0,
            StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN
        ));
    }
    let proof = CandidateEffectRepository::authenticate(&ctx.pool, lease.id, &lease.token).await?;
    assert_eq!(
        sqlx::query_scalar::<_, i32>(
            "SELECT operations FROM pack_candidate_context WHERE lease = $1"
        )
        .bind(lease.id)
        .fetch_one(&ctx.pool)
        .await?,
        0
    );
    assert_eq!(
        effect(
            &ctx,
            &lease,
            &lease.token,
            json!({"operation":"put_trigger", "target":"candidate.only"})
        )
        .await?
        .0,
        StatusCode::OK
    );
    assert_eq!(
        sqlx::query_scalar::<_, i32>(
            "SELECT operations FROM pack_candidate_context WHERE lease = $1"
        )
        .bind(lease.id)
        .fetch_one(&ctx.pool)
        .await?,
        1
    );
    PackInstallRepository::new(ctx.pool.clone())
        .finish_running(install, PackInstallStatus::Failed, None, None, None)
        .await?
        .unwrap();
    assert!(CandidateEffectRepository::apply_authenticated(
        &ctx.pool,
        &proof,
        attune_common::repositories::candidate_effect::CandidateOperation::ListEvents
    )
    .await
    .is_err());
    ctx.cleanup().await?;
    Ok(())
}
