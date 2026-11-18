//! Real repository boundaries for the trusted in-process callsink. No host
//! enrollment/controller principal or provider service is manufactured here.
mod helpers;
use attune_common::{
    isolated_process::{ProcessSpec, ResourceIdentity},
    repositories::{
        delegated_execution::DelegatedExecutionRepository,
        execution::ExecutionRepository,
        execution_input::{ExecutionInputRepository, SelectedExecutionInput},
        execution_process::ExecutionProcessRepository as Effects,
        FindById,
    },
    workload_launch::{
        LaunchExitOutcome, LaunchInvocation, LaunchPlatform, LaunchPurpose, LaunchRoot,
        LaunchTarget, PackAccess, PinnedImage, Sha256Digest,
    },
};
use helpers::{Result, TestContext};
use serde_json::json;
use uuid::Uuid;

async fn admitted(
    ctx: &TestContext,
) -> Result<(ProcessSpec, SelectedExecutionInput, ResourceIdentity)> {
    let owner = ctx.user.as_ref().unwrap().id;
    let token = helpers::delegated_source::token(ctx, owner, &["standard".into()]).await?;
    let claims = attune_common::auth::jwt::validate_token(&token, &ctx.state.jwt_config)?;
    let execution = claims.metadata.unwrap()["execution_id"].as_i64().unwrap();
    let row = ExecutionRepository::find_by_id(&ctx.pool, execution)
        .await?
        .unwrap();
    let worker = row.worker.unwrap();
    let instance = Uuid::new_v4();
    sqlx::query("UPDATE worker SET meta=COALESCE(meta,'{}'::jsonb)||jsonb_build_object('registration_incarnation',$2::text) WHERE id=$1")
        .bind(worker).bind(instance.to_string()).execute(&ctx.pool).await?;
    attune_common::repositories::runtime::WorkerRepository::heartbeat_incarnation(
        &ctx.pool, worker, instance,
    )
    .await?;
    sqlx::query("UPDATE execution SET timeout_seconds=300,config=$2 WHERE id=$1")
        .bind(execution)
        .bind(json!({"value":"owned-private-parameters"}))
        .execute(&ctx.pool)
        .await?;
    let mut tx = ctx.pool.begin().await?;
    let attempt = DelegatedExecutionRepository::current_attempt(&mut tx, execution).await?;
    let input = ExecutionInputRepository::restore(
        &mut tx,
        execution,
        ctx.state.config.security.encryption_key.as_deref(),
        None,
    )
    .await?;
    let (value, _, snapshot, expiry) = input.into_parts();
    tx.commit().await?;
    expiry.ensure_fresh()?;
    let image = Sha256Digest::of_bytes(b"owned-process-image");
    Ok((
        ProcessSpec {
            intent: Uuid::new_v4(),
            purpose: LaunchPurpose::Action,
            target: LaunchTarget::Execution {
                execution,
                worker,
                worker_instance: instance,
                attempt,
            },
            image: PinnedImage {
                reference: format!("sha256:{}", image.as_str()),
                sha256: image,
            },
            platform: LaunchPlatform::LinuxAmd64,
            inputs: Vec::new(),
            pack_access: PackAccess::ReadOnly,
            stage: None,
            timeout_ms: Some(300_000),
            invocation: LaunchInvocation {
                argv: vec!["/opt/attune/pack/owned".into()],
                env: [(
                    "OWNED_PRIVATE_ENV".into(),
                    "owned-private-environment".into(),
                )]
                .into(),
                working_root: LaunchRoot::Pack,
            },
        },
        SelectedExecutionInput::new(value, snapshot)?,
        ResourceIdentity::Kubernetes {
            cluster: "owned-cluster".into(),
            namespace: "owned-namespace".into(),
            pod_name: format!("owned-{}", Uuid::new_v4().simple()),
            pod_uid: Uuid::new_v4().to_string(),
        },
    ))
}

#[tokio::test]
async fn exact_process_input_is_encrypted_and_consumed_once_across_recovery() -> Result<()> {
    attune_common::auth::install_crypto_provider();
    let ctx = TestContext::new().await?.with_admin_auth().await?;
    let (spec, selected, resource) = admitted(&ctx).await?;
    let owner = ctx.user.as_ref().unwrap().id;
    let master = ctx.state.config.security.encryption_key.as_deref().unwrap();
    Effects::reserve(&ctx.pool, &spec, owner, &selected, master).await?;
    assert_eq!(
        Effects::begin_create(&ctx.pool, spec.intent, master).await?,
        attune_common::isolated_process::ProcessCreateDisposition::CreateOnce
    );
    // A recovered caller cannot receive another first-submit decision when an
    // earlier provider request has no recorded outcome/resource.
    let absent: bool =
        sqlx::query_scalar("SELECT resource IS NULL FROM execution_process_effect WHERE intent=$1")
            .bind(spec.intent)
            .fetch_one(&ctx.pool)
            .await?;
    assert!(absent);
    assert_eq!(
        Effects::begin_create(&ctx.pool, spec.intent, master).await?,
        attune_common::isolated_process::ProcessCreateDisposition::ReconcileOnly
    );
    Effects::record_resource(&ctx.pool, spec.intent, &resource).await?;
    let ciphertext: String = sqlx::query_scalar(
        "SELECT encrypted_context FROM execution_process_effect WHERE intent=$1",
    )
    .bind(spec.intent)
    .fetch_one(&ctx.pool)
    .await?;
    assert!(!ciphertext.contains("owned-private-parameters"));
    assert!(!ciphertext.contains("owned-private-environment"));
    let input = Effects::consume_stdin(&ctx.pool, spec.intent, &resource, master).await?;
    assert_eq!(input.as_bytes(), br#"{"value":"owned-private-parameters"}"#);
    // A different connection/recovered caller cannot interpret a lost ACK as a
    // fresh write, even though the original immutable plan remains available.
    let retained = Effects::retained_spec(&ctx.pool, spec.intent, master).await?;
    assert_eq!(retained.intent, spec.intent);
    assert_eq!(
        Effects::begin_create(&ctx.pool, spec.intent, master).await?,
        attune_common::isolated_process::ProcessCreateDisposition::ReconcileOnly
    );
    assert!(
        Effects::consume_stdin(&ctx.pool, spec.intent, &resource, master)
            .await
            .is_err()
    );
    assert!(Effects::reserve(&ctx.pool, &spec, owner, &selected, master)
        .await
        .is_err());
    ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn process_gate_rejects_changed_pod_uid_stop_and_revoke_restore() -> Result<()> {
    attune_common::auth::install_crypto_provider();
    let ctx = TestContext::new().await?.with_admin_auth().await?;
    let master = ctx.state.config.security.encryption_key.as_deref().unwrap();
    let owner = ctx.user.as_ref().unwrap().id;
    let (spec, selected, resource) = admitted(&ctx).await?;
    Effects::reserve(&ctx.pool, &spec, owner, &selected, master).await?;
    assert_eq!(
        Effects::begin_create(&ctx.pool, spec.intent, master).await?,
        attune_common::isolated_process::ProcessCreateDisposition::CreateOnce
    );
    Effects::record_resource(&ctx.pool, spec.intent, &resource).await?;
    let mut changed = resource.clone();
    if let ResourceIdentity::Kubernetes { pod_uid, .. } = &mut changed {
        *pod_uid = Uuid::new_v4().to_string();
    }
    assert!(Effects::record_resource(&ctx.pool, spec.intent, &changed)
        .await
        .is_err());
    assert!(
        Effects::consume_stdin(&ctx.pool, spec.intent, &changed, master)
            .await
            .is_err()
    );
    sqlx::query("UPDATE identity SET frozen=TRUE WHERE id=$1")
        .bind(owner)
        .execute(&ctx.pool)
        .await?;
    sqlx::query("UPDATE identity SET frozen=FALSE WHERE id=$1")
        .bind(owner)
        .execute(&ctx.pool)
        .await?;
    assert!(
        Effects::consume_stdin(&ctx.pool, spec.intent, &resource, master)
            .await
            .is_err()
    );
    // Restoration permits a separate newly admitted operation, never revival.
    let (new, selected, new_resource) = admitted(&ctx).await?;
    Effects::reserve(&ctx.pool, &new, owner, &selected, master).await?;
    assert_eq!(
        Effects::begin_create(&ctx.pool, new.intent, master).await?,
        attune_common::isolated_process::ProcessCreateDisposition::CreateOnce
    );
    Effects::record_resource(&ctx.pool, new.intent, &new_resource).await?;
    Effects::request_stop(&ctx.pool, new.intent).await?;
    assert!(
        Effects::consume_stdin(&ctx.pool, new.intent, &new_resource, master)
            .await
            .is_err()
    );
    let (fresh, selected, fresh_resource) = admitted(&ctx).await?;
    Effects::reserve(&ctx.pool, &fresh, owner, &selected, master).await?;
    assert_eq!(
        Effects::begin_create(&ctx.pool, fresh.intent, master).await?,
        attune_common::isolated_process::ProcessCreateDisposition::CreateOnce
    );
    Effects::record_resource(&ctx.pool, fresh.intent, &fresh_resource).await?;
    assert!(
        Effects::consume_stdin(&ctx.pool, fresh.intent, &fresh_resource, master)
            .await
            .is_ok()
    );
    ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn native_exit_closes_runtime_before_completion_and_commit_replay_is_historical() -> Result<()>
{
    use attune_common::{
        models::ExecutionStatus, repositories::runtime_read::RuntimeReadRepository,
    };
    attune_common::auth::install_crypto_provider();
    let ctx = TestContext::new().await?.with_admin_auth().await?;
    let (spec, selected, resource) = admitted(&ctx).await?;
    let master = ctx.state.config.security.encryption_key.as_deref().unwrap();
    let owner = ctx.user.as_ref().unwrap().id;
    let LaunchTarget::Execution {
        execution, worker, ..
    } = spec.target
    else {
        unreachable!()
    };
    Effects::reserve(&ctx.pool, &spec, owner, &selected, master).await?;
    Effects::begin_create(&ctx.pool, spec.intent, master).await?;
    Effects::record_resource(&ctx.pool, spec.intent, &resource).await?;
    Effects::consume_stdin(&ctx.pool, spec.intent, &resource, master).await?;
    let token = helpers::delegated_source::execution_token(&ctx, execution).await?;
    let claims = attune_common::auth::jwt::validate_token(&token, &ctx.state.jwt_config)?;
    let mut tx = ctx.pool.begin().await?;
    assert!(
        DelegatedExecutionRepository::validate_claims(&mut tx, &claims)
            .await?
            .is_some()
    );
    tx.commit().await?;
    // A caller cannot fabricate a terminal result from a live native process.
    let mut tx = ctx.pool.begin().await?;
    assert!(Effects::lock_completion(&mut tx, spec.intent, master)
        .await
        .is_err());
    tx.rollback().await?;
    let exit = LaunchExitOutcome {
        exit_code: 0,
        oom_killed: false,
    };
    Effects::record_exit(&ctx.pool, spec.intent, &resource, exit).await?;
    Effects::record_exit(&ctx.pool, spec.intent, &resource, exit).await?;
    assert!(Effects::record_exit(
        &ctx.pool,
        spec.intent,
        &resource,
        LaunchExitOutcome {
            exit_code: 1,
            oom_killed: false
        }
    )
    .await
    .is_err());
    let mut tx = ctx.pool.begin().await?;
    assert!(
        DelegatedExecutionRepository::validate_claims(&mut tx, &claims)
            .await
            .is_err()
    );
    tx.rollback().await?;
    assert_eq!(
        ExecutionRepository::find_by_id(&ctx.pool, execution)
            .await?
            .unwrap()
            .status,
        ExecutionStatus::Running
    );

    // Closure and terminal state roll back together on cancellation/error.
    let mut tx = ctx.pool.begin().await?;
    assert!(Effects::lock_completion(&mut tx, spec.intent, master)
        .await?
        .is_none());
    RuntimeReadRepository::close_execution(&mut tx, execution).await?;
    ExecutionRepository::transition_worker(
        &mut *tx,
        execution,
        Some(worker),
        ExecutionStatus::Completed,
        Some(json!({"exit_code":0,"data":"original"})),
    )
    .await?;
    tx.rollback().await?;
    assert_eq!(
        ExecutionRepository::find_by_id(&ctx.pool, execution)
            .await?
            .unwrap()
            .status,
        ExecutionStatus::Running
    );
    let mut tx = ctx.pool.begin().await?;
    assert!(Effects::lock_completion(&mut tx, spec.intent, master)
        .await?
        .is_none());
    RuntimeReadRepository::close_execution(&mut tx, execution).await?;
    ExecutionRepository::transition_worker(
        &mut *tx,
        execution,
        Some(worker),
        ExecutionStatus::Completed,
        Some(json!({"exit_code":0,"data":"original"})),
    )
    .await?;
    tx.commit().await?;
    let count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM execution_history WHERE entity_id=$1")
            .bind(execution)
            .fetch_one(&ctx.pool)
            .await?;
    // Lose the original completion ACK; recovery reads its exact committed
    // result without closing another attempt or replaying any result writes.
    let mut tx = ctx.pool.begin().await?;
    let historical = Effects::lock_completion(&mut tx, spec.intent, master)
        .await?
        .ok_or("Missing completion receipt")?;
    assert_eq!(historical.execution.status, ExecutionStatus::Completed);
    assert_eq!(historical.execution.result.unwrap()["data"], "original");
    tx.commit().await?;
    let after: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM execution_history WHERE entity_id=$1")
            .bind(execution)
            .fetch_one(&ctx.pool)
            .await?;
    assert_eq!(count, after);
    ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn replacement_registration_cannot_reuse_an_existing_native_attempt() -> Result<()> {
    attune_common::auth::install_crypto_provider();
    let ctx = TestContext::new().await?.with_admin_auth().await?;
    let (spec, selected, resource) = admitted(&ctx).await?;
    let master = ctx.state.config.security.encryption_key.as_deref().unwrap();
    let owner = ctx.user.as_ref().unwrap().id;
    let LaunchTarget::Execution {
        execution, worker, ..
    } = spec.target
    else {
        unreachable!()
    };
    Effects::reserve(&ctx.pool, &spec, owner, &selected, master).await?;
    Effects::begin_create(&ctx.pool, spec.intent, master).await?;
    Effects::record_resource(&ctx.pool, spec.intent, &resource).await?;
    Effects::consume_stdin(&ctx.pool, spec.intent, &resource, master).await?;
    let token = helpers::delegated_source::execution_token(&ctx, execution).await?;
    let claims = attune_common::auth::jwt::validate_token(&token, &ctx.state.jwt_config)?;
    let replacement = Uuid::new_v4();
    sqlx::query("UPDATE worker SET meta=meta||jsonb_build_object('registration_incarnation',$2::text) WHERE id=$1")
        .bind(worker).bind(replacement.to_string()).execute(&ctx.pool).await?;
    attune_common::repositories::runtime::WorkerRepository::heartbeat_incarnation(
        &ctx.pool,
        worker,
        replacement,
    )
    .await?;
    let mut tx = ctx.pool.begin().await?;
    assert!(
        DelegatedExecutionRepository::validate_claims(&mut tx, &claims)
            .await
            .is_err()
    );
    tx.rollback().await?;
    let mut next = spec.clone();
    next.intent = Uuid::new_v4();
    if let LaunchTarget::Execution {
        worker_instance, ..
    } = &mut next.target
    {
        *worker_instance = replacement;
    }
    assert!(Effects::reserve(&ctx.pool, &next, owner, &selected, master)
        .await
        .is_err());
    let count:i64=sqlx::query_scalar("SELECT COUNT(*) FROM execution_process_effect WHERE target->>'kind'='execution' AND (target->>'execution')::BIGINT=$1")
        .bind(execution).fetch_one(&ctx.pool).await?;
    assert_eq!(count, 1);
    ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn retired_controller_scope_cannot_fall_through_to_ambient_iam() -> Result<()> {
    use attune_common::auth::jwt::{Claims, TokenType};
    use axum::{
        body::Body,
        http::{Request, StatusCode},
    };
    use tower::ServiceExt;
    attune_common::auth::install_crypto_provider();
    let ctx = TestContext::new().await?.with_admin_auth().await?;
    // Even an active administrator subject cannot widen this retired machine
    // purpose into ordinary API access after the relay route is removed.
    let now = chrono::Utc::now().timestamp();
    let claims = Claims {
        sub: ctx.user.as_ref().unwrap().id.to_string(),
        login: "retired-controller".into(),
        iat: now,
        exp: now + 60,
        token_type: TokenType::Worker,
        scope: Some("workload_controller".into()),
        metadata: Some(json!({"controller":1,"engine":1,"incarnation":Uuid::new_v4()})),
    };
    for scope in [
        "workload_controller",
        "workload_host",
        "workload_host_enrollment",
        "unknown_worker_scope",
    ] {
        let claims = Claims {
            scope: Some(scope.into()),
            ..claims.clone()
        };
        let token = jsonwebtoken::encode(
            &jsonwebtoken::Header::default(),
            &claims,
            &jsonwebtoken::EncodingKey::from_secret(ctx.state.jwt_config.secret.as_bytes()),
        )?;
        let response = ctx
            .app
            .clone()
            .oneshot(
                Request::get("/api/v1/packs")
                    .header("authorization", format!("Bearer {token}"))
                    .body(Body::empty())?,
            )
            .await?;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }
    for route in [
        "/api/v1/workload-hosts/register",
        "/api/v1/workload-hosts/session",
        "/api/v1/workload-hosts/custody/reserve",
        "/api/v1/workload-hosts/custody/prepare",
        "/api/v1/workload-hosts/custody/cancel-absent",
    ] {
        let response = ctx
            .app
            .clone()
            .oneshot(
                Request::post(route)
                    .header("authorization", format!("Bearer {}", ctx.token().unwrap()))
                    .header("content-type", "application/json")
                    .body(Body::from("{}"))?,
            )
            .await?;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }
    let response = ctx
        .app
        .clone()
        .oneshot(
            Request::post("/api/v1/workload-launches/1/resolve")
                .header("authorization", format!("Bearer {}", ctx.token().unwrap()))
                .header("content-type", "application/json")
                .body(Body::from(r#"{"launch_id":1,"admitted_digest":"retired"}"#))?,
        )
        .await?;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    ctx.cleanup().await?;
    Ok(())
}

async fn ordinary_file_version(
    ctx: &TestContext,
    execution: i64,
) -> Result<attune_common::models::artifact_version::ArtifactVersion> {
    use attune_common::{
        models::{ArtifactType, ArtifactVisibility, OwnerType, RetentionPolicyType},
        repositories::artifact::{
            ArtifactRepository, ArtifactVersionRepository, CreateArtifactInput,
        },
    };
    let row = ExecutionRepository::find_by_id(&ctx.pool, execution)
        .await?
        .ok_or("execution")?;
    let reference = format!("{}.owned-{}", row.action_ref, Uuid::new_v4().simple());
    let mut tx = ctx.pool.begin().await?;
    let artifact = ArtifactRepository::create_or_get(
        &mut tx,
        CreateArtifactInput {
            r#ref: reference.clone(),
            scope: OwnerType::Action,
            owner: row.action_ref,
            r#type: ArtifactType::FileBinary,
            visibility: ArtifactVisibility::Private,
            classification: Default::default(),
            retention_policy: RetentionPolicyType::Versions,
            retention_limit: 5,
            name: None,
            description: None,
            content_type: Some("application/octet-stream".into()),
            data: None,
        },
    )
    .await?;
    tx.commit().await?;
    Ok(ArtifactVersionRepository::create_file_backed(
        &ctx.pool,
        artifact.id,
        &reference,
        "application/octet-stream".into(),
        Some(execution),
        None,
        None,
    )
    .await?)
}
async fn file_upload_admission(
    ctx: &TestContext,
    token: &str,
    path: &str,
) -> Result<axum::http::StatusCode> {
    use axum::{body::Body, http::Request};
    use tower::ServiceExt;
    let response=ctx.app.clone().oneshot(Request::post(format!("/api/v1/internal/artifacts/direct-upload/{path}"))
        .header("authorization",format!("Bearer {token}")).header("content-type","application/json")
        .body(Body::from(serde_json::to_vec(&json!({"size_bytes":0,"sha256":Sha256Digest::of_bytes(b"").as_str(),"content_type":"application/octet-stream"}))?))?).await?;
    Ok(response.status())
}
#[tokio::test]
async fn native_file_publication_keeps_revocation_permanent_after_exit() -> Result<()> {
    use attune_common::{
        auth::WorkerTokenProvider, models::enums::ArtifactBodyState,
        repositories::artifact::ArtifactVersionRepository,
    };
    use axum::http::StatusCode;
    attune_common::auth::install_crypto_provider();
    let ctx = TestContext::new().await?.with_admin_auth().await?;
    let (spec, selected, resource) = admitted(&ctx).await?;
    let master = ctx.state.config.security.encryption_key.as_deref().unwrap();
    let owner = ctx.user.as_ref().unwrap().id;
    let LaunchTarget::Execution {
        execution, worker, ..
    } = spec.target
    else {
        unreachable!()
    };
    Effects::reserve(&ctx.pool, &spec, owner, &selected, master).await?;
    Effects::begin_create(&ctx.pool, spec.intent, master).await?;
    Effects::record_resource(&ctx.pool, spec.intent, &resource).await?;
    Effects::consume_stdin(&ctx.pool, spec.intent, &resource, master).await?;
    let version = ordinary_file_version(&ctx, execution).await?;
    Effects::record_exit(
        &ctx.pool,
        spec.intent,
        &resource,
        LaunchExitOutcome {
            exit_code: 0,
            oom_killed: false,
        },
    )
    .await?;
    let token = WorkerTokenProvider::new(
        owner,
        worker.to_string(),
        ctx.state.jwt_config.as_ref().clone(),
    )
    .token()?;
    let path = version.file_path.as_deref().unwrap();
    assert_eq!(
        file_upload_admission(&ctx, &token, path).await?,
        StatusCode::OK
    );
    let wrong_worker = WorkerTokenProvider::new(
        owner,
        (worker + 1000).to_string(),
        ctx.state.jwt_config.as_ref().clone(),
    )
    .token()?;
    assert_eq!(
        file_upload_admission(&ctx, &wrong_worker, path).await?,
        StatusCode::FORBIDDEN
    );
    sqlx::query("UPDATE identity SET frozen=TRUE WHERE id=$1")
        .bind(owner)
        .execute(&ctx.pool)
        .await?;
    sqlx::query("UPDATE identity SET frozen=FALSE WHERE id=$1")
        .bind(owner)
        .execute(&ctx.pool)
        .await?;
    assert_eq!(
        file_upload_admission(&ctx, &token, path).await?,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        ArtifactVersionRepository::find_by_id(&ctx.pool, version.id)
            .await?
            .unwrap()
            .body_state,
        Some(ArtifactBodyState::Pending)
    );
    let (new, selected, new_resource) = admitted(&ctx).await?;
    let LaunchTarget::Execution {
        execution, worker, ..
    } = new.target
    else {
        unreachable!()
    };
    Effects::reserve(&ctx.pool, &new, owner, &selected, master).await?;
    Effects::begin_create(&ctx.pool, new.intent, master).await?;
    Effects::record_resource(&ctx.pool, new.intent, &new_resource).await?;
    Effects::consume_stdin(&ctx.pool, new.intent, &new_resource, master).await?;
    let version = ordinary_file_version(&ctx, execution).await?;
    Effects::record_exit(
        &ctx.pool,
        new.intent,
        &new_resource,
        LaunchExitOutcome {
            exit_code: 0,
            oom_killed: false,
        },
    )
    .await?;
    let token = WorkerTokenProvider::new(
        owner,
        worker.to_string(),
        ctx.state.jwt_config.as_ref().clone(),
    )
    .token()?;
    assert_eq!(
        file_upload_admission(&ctx, &token, version.file_path.as_deref().unwrap()).await?,
        StatusCode::OK
    );
    ctx.cleanup().await?;
    Ok(())
}

async fn stopped_file(
    ctx: &TestContext,
) -> Result<(
    ProcessSpec,
    attune_common::models::artifact_version::ArtifactVersion,
    String,
)> {
    let (spec, selected, resource) = admitted(ctx).await?;
    let owner = ctx.user.as_ref().unwrap().id;
    let master = ctx.state.config.security.encryption_key.as_deref().unwrap();
    let LaunchTarget::Execution {
        execution, worker, ..
    } = spec.target
    else {
        unreachable!()
    };
    Effects::reserve(&ctx.pool, &spec, owner, &selected, master).await?;
    Effects::begin_create(&ctx.pool, spec.intent, master).await?;
    Effects::record_resource(&ctx.pool, spec.intent, &resource).await?;
    Effects::consume_stdin(&ctx.pool, spec.intent, &resource, master).await?;
    let version = ordinary_file_version(ctx, execution).await?;
    Effects::record_exit(
        &ctx.pool,
        spec.intent,
        &resource,
        LaunchExitOutcome {
            exit_code: 0,
            oom_killed: false,
        },
    )
    .await?;
    let token = attune_common::auth::WorkerTokenProvider::new(
        owner,
        worker.to_string(),
        ctx.state.jwt_config.as_ref().clone(),
    )
    .token()?;
    Ok((spec, version, token))
}

#[tokio::test]
async fn native_file_publication_rejects_a_replaced_worker_incarnation() -> Result<()> {
    use axum::http::StatusCode;
    attune_common::auth::install_crypto_provider();
    let ctx = TestContext::new().await?.with_admin_auth().await?;
    let (spec, version, token) = stopped_file(&ctx).await?;
    let path = version.file_path.as_deref().unwrap();
    assert_eq!(
        file_upload_admission(&ctx, &token, path).await?,
        StatusCode::OK
    );
    let LaunchTarget::Execution { worker, .. } = spec.target else {
        unreachable!()
    };
    sqlx::query("UPDATE worker SET meta=meta||jsonb_build_object('registration_incarnation',$2::text) WHERE id=$1")
        .bind(worker).bind(Uuid::new_v4().to_string()).execute(&ctx.pool).await?;
    assert_eq!(
        file_upload_admission(&ctx, &token, path).await?,
        StatusCode::FORBIDDEN
    );
    ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn native_file_publication_fences_worker_expiry_after_target_wait() -> Result<()> {
    use attune_common::{
        models::enums::ArtifactBodyState,
        repositories::artifact::{ArtifactRepository, ArtifactVersionRepository},
    };
    use axum::{
        body::Body,
        http::{Request, StatusCode},
    };
    use std::time::Duration;
    use tower::ServiceExt;
    attune_common::auth::install_crypto_provider();
    let ctx = TestContext::new().await?.with_admin_auth().await?;
    let (spec, version, token) = stopped_file(&ctx).await?;
    let path = version.file_path.as_deref().unwrap();
    // Warm the exact source path before the real clock begins.
    assert_eq!(
        file_upload_admission(&ctx, &token, path).await?,
        StatusCode::OK
    );
    let LaunchTarget::Execution { worker, .. } = spec.target else {
        unreachable!()
    };
    let mut barrier = ctx.pool.begin().await?;
    let blocker: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *barrier)
        .await?;
    ArtifactRepository::find_by_id_for_update(&mut barrier, version.artifact)
        .await?
        .ok_or("artifact")?;
    let deadline:chrono::DateTime<chrono::Utc> = sqlx::query_scalar("UPDATE worker SET last_heartbeat=clock_timestamp()-interval '80 seconds' WHERE id=$1 RETURNING last_heartbeat+interval '90 seconds'")
        .bind(worker).fetch_one(&ctx.pool).await?;
    let request = Request::post(format!("/api/v1/internal/artifacts/direct-upload/{path}"))
        .header("authorization",format!("Bearer {token}")).header("content-type","application/json")
        .body(Body::from(serde_json::to_vec(&json!({"size_bytes":0,"sha256":Sha256Digest::of_bytes(b"").as_str(),"content_type":"application/octet-stream"}))?))?;
    let app = ctx.app.clone();
    let mut call = tokio::spawn(async move { app.oneshot(request).await });
    let mut barrier = Some(barrier);
    let result = async {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let waiting:bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE $1=ANY(pg_blocking_pids(pid)))")
                    .bind(blocker).fetch_one(&ctx.pool).await?;
                if waiting {return Ok::<_,Box<dyn std::error::Error>>(())}
                if call.is_finished() {return Err("request did not reach the target barrier".into())}
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }).await??;
        tokio::time::timeout(Duration::from_secs(20), async {
            loop {
                let expired:bool = sqlx::query_scalar("SELECT clock_timestamp()>=$1").bind(deadline).fetch_one(&ctx.pool).await?;
                if expired {return Ok::<_,sqlx::Error>(())}
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }).await??;
        barrier.take().unwrap().rollback().await?;
        let response = tokio::time::timeout(Duration::from_secs(5), &mut call).await???;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        assert_eq!(ArtifactVersionRepository::find_by_id(&ctx.pool, version.id).await?.unwrap().body_state, Some(ArtifactBodyState::Pending));
        Ok::<_,Box<dyn std::error::Error>>(())
    }.await;
    if let Some(barrier) = barrier {
        barrier.rollback().await?;
    }
    if !call.is_finished() {
        call.abort();
        let _ = call.await;
    }
    ctx.cleanup().await?;
    result
}
