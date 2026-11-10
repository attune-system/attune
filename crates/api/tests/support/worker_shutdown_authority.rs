//! Authority boundary used by graceful worker shutdown: keeping the original
//! delivery permit alive does not waive current policy or attempt validation.
use super::*;

#[tokio::test]
async fn shutdown_delivery_keeps_policy_and_attempt_fences() -> Result<()> {
    use attune_common::{models::LogStreamBackend, repositories::log_stream::LogStreamRepository};
    let fixture = fixture(true).await?;
    let context = delivery(&fixture).await?;
    // Registration ownership is separate from the delivery context's instance.
    let incarnation = Uuid::new_v4();
    WorkerRepository::update(
        &fixture.ctx.pool,
        context.worker,
        attune_common::repositories::runtime::UpdateWorkerInput {
            meta: Some(json!({"registration_incarnation": incarnation.to_string()})),
            ..Default::default()
        },
    )
    .await?;
    let reason = format!("shutdown test {}", Uuid::new_v4());
    assert!(
        WorkerRepository::cordon_for_shutdown(
            &fixture.ctx.pool,
            context.worker,
            incarnation,
            &reason
        )
        .await?
    );
    WorkerRepository::clear_shutdown_cordon(
        &fixture.ctx.pool,
        context.worker,
        incarnation,
        &reason,
    )
    .await?;
    // Clearing is refused while active; the original owner can still deliver.
    let worker = WorkerRepository::get_by_id(&fixture.ctx.pool, context.worker).await?;
    assert!(worker.cordoned);
    assert_eq!(
        worker.status,
        Some(attune_common::models::WorkerStatus::Active)
    );
    let response = fixture
        .ctx
        .post(
            "/api/v1/artifacts",
            json!({
                "ref":"recorded_emitter.shutdown.stdout.log", "scope":"action",
                "owner":"recorded_emitter.emit", "type":"file_text"
            }),
            Some(&fixture.token),
        )
        .await?;
    assert_eq!(response.status(), StatusCode::CREATED);
    let artifact: Value = response.json().await?;
    let mut tx = fixture.ctx.pool.begin().await?;
    let (_, effect) = WorkerDeliveryRepository::require_launch_context(&mut tx, &context).await?;
    let version = ArtifactVersionRepository::create_log_pending_with_conn(
        &mut tx,
        artifact["data"]["id"].as_i64().unwrap(),
        "recorded_emitter.shutdown.stdout.log",
        LogStreamBackend::ObjectSegments,
        context.execution,
        json!({"stream":"stdout"}),
    )
    .await?;
    let stream = LogStreamRepository::create_with_backend_with_conn(
        &mut tx,
        version.id,
        LogStreamBackend::ObjectSegments,
        16,
        500,
    )
    .await?;
    let permit = WorkerDeliveryRepository::bind(
        &mut tx,
        &context,
        version.id,
        Some(stream.id),
        64,
        4,
        attune_common::rbac::Action::Update,
        &effect,
    )
    .await?;
    tx.commit().await?;
    let token =
        attune_common::worker_delivery::mint(&context, &permit, &fixture.ctx.state.jwt_config)?;
    let mut stale = context.clone();
    stale.worker_instance = Uuid::new_v4();
    let stale_token =
        attune_common::worker_delivery::mint(&stale, &permit, &fixture.ctx.state.jwt_config)?;
    let path = format!("/api/v1/internal/logs/{}/segments/0", version.id);
    assert_eq!(
        raw(
            &fixture,
            "PUT",
            &path,
            &stale_token,
            "text/plain",
            b"tail".to_vec()
        )
        .await?,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        raw(
            &fixture,
            "PUT",
            &path,
            &token,
            "text/plain",
            b"tail".to_vec()
        )
        .await?,
        StatusCode::CREATED
    );
    let seal_path = format!("/api/v1/internal/logs/{}/seal", version.id);
    assert_eq!(
        fixture
            .ctx
            .post(&seal_path, json!({}), Some(&token))
            .await?
            .status(),
        StatusCode::OK
    );
    // Even an idempotent seal retry must revalidate policy after shutdown's
    // successful final upload. No grace window authorizes a revoked owner.
    let mut tx = fixture.ctx.pool.begin().await?;
    RuntimeAuthorityRepository::revoke_policy(&mut tx, fixture.policy_id).await?;
    tx.commit().await?;
    assert_eq!(
        fixture
            .ctx
            .post(&seal_path, json!({}), Some(&token))
            .await?
            .status(),
        StatusCode::FORBIDDEN
    );
    WorkerRepository::update(
        &fixture.ctx.pool,
        context.worker,
        attune_common::repositories::runtime::UpdateWorkerInput {
            status: Some(attune_common::models::WorkerStatus::Inactive),
            ..Default::default()
        },
    )
    .await?;
    WorkerRepository::clear_shutdown_cordon(
        &fixture.ctx.pool,
        context.worker,
        incarnation,
        &reason,
    )
    .await?;
    assert!(
        !WorkerRepository::get_by_id(&fixture.ctx.pool, context.worker)
            .await?
            .cordoned
    );
    assert!(
        WorkerRepository::cordon_for_shutdown(
            &fixture.ctx.pool,
            context.worker,
            incarnation,
            &reason
        )
        .await?
    );
    // An operator can replace this invocation's marker while it is cordoned.
    WorkerRepository::set_cordoned(
        &fixture.ctx.pool,
        context.worker,
        true,
        Some("operator maintenance".into()),
        None,
    )
    .await?;
    assert!(
        !WorkerRepository::cordon_for_shutdown(
            &fixture.ctx.pool,
            context.worker,
            incarnation,
            &reason
        )
        .await?
    );
    WorkerRepository::clear_shutdown_cordon(
        &fixture.ctx.pool,
        context.worker,
        incarnation,
        &reason,
    )
    .await?;
    assert!(
        WorkerRepository::get_by_id(&fixture.ctx.pool, context.worker)
            .await?
            .cordoned
    );
    fixture.ctx.cleanup().await?;
    Ok(())
}
