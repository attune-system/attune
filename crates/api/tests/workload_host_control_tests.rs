mod helpers;
use attune_common::{
    workload_host::{self, EnrollmentActor, RegisterWorkloadHost, RegisteredWorkloadHost},
    workload_session::{RenewWorkloadSession, WorkloadSession},
};
use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
};
use helpers::{Result, TestContext};
use serde::Serialize;
use serde_json::{json, Value};
use tower::ServiceExt;
use uuid::Uuid;
async fn post<T: Serialize>(
    ctx: &TestContext,
    path: &str,
    token: Option<&str>,
    body: &T,
) -> Result<(StatusCode, Value)> {
    let mut request = Request::post(path).header("content-type", "application/json");
    if let Some(token) = token {
        request = request.header("authorization", format!("Bearer {token}"));
    }
    let response = ctx
        .app
        .clone()
        .oneshot(request.body(Body::from(serde_json::to_vec(body)?))?)
        .await?;
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 1024 * 1024).await?;
    Ok((status, serde_json::from_slice(&bytes)?))
}
#[tokio::test]
async fn enrollment_sessions_are_scoped_and_registration_handover_permanently_fences_old_tokens(
) -> Result<()> {
    attune_common::auth::install_crypto_provider();
    let ctx = TestContext::new().await?.with_admin_auth().await?;
    let identity = ctx.user.as_ref().unwrap().id;
    let engine:i64=sqlx::query_scalar("INSERT INTO workload_engine(pool,controller_url,daemon_id,platform) VALUES($1,'https://owned-controller.invalid','owned-daemon','linux/amd64') RETURNING id")
        .bind(format!("owned-{}",Uuid::new_v4())).fetch_one(&ctx.pool).await?;
    let digest = attune_common::workload_launch::Sha256Digest::of_bytes(b"owned-host-image");
    let image:i64=sqlx::query_scalar("INSERT INTO workload_execution_image(engine,reference,sha256,platform) VALUES($1,$2,$3,'linux/amd64') RETURNING id")
        .bind(engine).bind(format!("sha256:{}",digest.as_str())).bind(digest.as_str()).fetch_one(&ctx.pool).await?;
    let profile:i64=sqlx::query_scalar("INSERT INTO workload_host_profile(image,role,capabilities,max_concurrent) VALUES($1,'action','{}'::jsonb,1) RETURNING id")
        .bind(image).fetch_one(&ctx.pool).await?;
    let enrollment:i64=sqlx::query_scalar("INSERT INTO workload_host_enrollment(profile,identity,worker_name,expires_at) VALUES($1,$2,$3,clock_timestamp()+interval '1 hour') RETURNING id")
        .bind(profile).bind(identity).bind(format!("owned-host-{}",Uuid::new_v4())).fetch_one(&ctx.pool).await?;
    let enrollment = workload_host::mint_enrollment(
        EnrollmentActor {
            identity,
            enrollment,
            profile,
            generation: 0,
            expires_at: chrono::Utc::now().timestamp() + 1800,
        },
        &ctx.state.jwt_config,
    )?;
    let request = RegisterWorkloadHost {
        instance: Uuid::new_v4(),
        custody_instance: Uuid::new_v4(),
    };
    assert_eq!(
        post(&ctx, "/api/v1/workload-hosts/register", None, &request)
            .await?
            .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        post(
            &ctx,
            "/api/v1/workload-hosts/register",
            ctx.token(),
            &request
        )
        .await?
        .0,
        StatusCode::FORBIDDEN
    );
    let (status, value) = post(
        &ctx,
        "/api/v1/workload-hosts/register",
        Some(enrollment.as_str()),
        &request,
    )
    .await?;
    assert_eq!(status, StatusCode::OK, "{value}");
    let first: RegisteredWorkloadHost = serde_json::from_value(value)?;
    assert_eq!(first.instance, request.instance);
    assert_eq!(first.profile.engine.engine_id, engine);
    assert_eq!(first.profile.image.sha256, digest);
    let (status, value) = post(
        &ctx,
        "/api/v1/workload-hosts/session",
        Some(first.session.token.as_str()),
        &RenewWorkloadSession {},
    )
    .await?;
    assert_eq!(status, StatusCode::OK, "{value}");
    let _: WorkloadSession = serde_json::from_value(value)?;
    assert_eq!(
        post(
            &ctx,
            "/api/v1/workload-hosts/session",
            Some(enrollment.as_str()),
            &json!({})
        )
        .await?
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        post(
            &ctx,
            "/api/v1/workload-hosts/register",
            Some(first.session.token.as_str()),
            &request
        )
        .await?
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        post(
            &ctx,
            "/api/v1/internal/log-delivery/control",
            Some(first.session.token.as_str()),
            &json!({})
        )
        .await?
        .0,
        StatusCode::FORBIDDEN
    );
    let next = RegisterWorkloadHost {
        instance: Uuid::new_v4(),
        custody_instance: Uuid::new_v4(),
    };
    let (status, value) = post(
        &ctx,
        "/api/v1/workload-hosts/register",
        Some(enrollment.as_str()),
        &next,
    )
    .await?;
    assert_eq!(status, StatusCode::OK, "{value}");
    let second: RegisteredWorkloadHost = serde_json::from_value(value)?;
    assert_eq!(first.worker, second.worker);
    assert!(second.registration_generation > first.registration_generation);
    assert_eq!(
        post(
            &ctx,
            "/api/v1/workload-hosts/session",
            Some(first.session.token.as_str()),
            &json!({})
        )
        .await?
        .0,
        StatusCode::FORBIDDEN
    );
    // Even restoring the original UUID cannot restore its old credential epoch.
    let (status, value) = post(
        &ctx,
        "/api/v1/workload-hosts/register",
        Some(enrollment.as_str()),
        &request,
    )
    .await?;
    assert_eq!(status, StatusCode::OK, "{value}");
    let restored: RegisteredWorkloadHost = serde_json::from_value(value)?;
    assert!(restored.registration_generation > second.registration_generation);
    assert_eq!(
        post(
            &ctx,
            "/api/v1/workload-hosts/session",
            Some(first.session.token.as_str()),
            &json!({})
        )
        .await?
        .0,
        StatusCode::FORBIDDEN
    );
    for disabled in [true, false] {
        sqlx::query("UPDATE workload_host_profile SET disabled=$2 WHERE id=$1")
            .bind(profile)
            .bind(disabled)
            .execute(&ctx.pool)
            .await?;
    }
    assert_eq!(
        post(
            &ctx,
            "/api/v1/workload-hosts/session",
            Some(restored.session.token.as_str()),
            &json!({})
        )
        .await?
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        post(
            &ctx,
            "/api/v1/workload-hosts/register",
            Some(enrollment.as_str()),
            &request
        )
        .await?
        .0,
        StatusCode::FORBIDDEN,
        "withdrawal requires a fresh incarnation"
    );
    ctx.cleanup().await?;
    Ok(())
}
