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
    admitted_token(ctx, &token).await
}

async fn admitted_token(
    ctx: &TestContext,
    token: &str,
) -> Result<(ProcessSpec, SelectedExecutionInput, ResourceIdentity)> {
    let claims = attune_common::auth::jwt::validate_token(token, &ctx.state.jwt_config)?;
    let execution = claims.metadata.unwrap()["execution_id"].as_i64().unwrap();
    sqlx::query("UPDATE execution SET timeout_seconds=300,config=$2 WHERE id=$1")
        .bind(execution)
        .bind(json!({"value":"owned-private-parameters"}))
        .execute(&ctx.pool)
        .await?;
    admitted_execution(ctx, execution).await
}

async fn admitted_execution(
    ctx: &TestContext,
    execution: i64,
) -> Result<(ProcessSpec, SelectedExecutionInput, ResourceIdentity)> {
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

async fn declared_input_case(delivery: &str) -> Result<()> {
    attune_common::auth::install_crypto_provider();
    let ctx = TestContext::new().await?.with_admin_auth().await?;
    let owner = ctx.user.as_ref().unwrap().id;
    for format in ["json", "yaml", "dotenv"] {
        let pack_ref = format!("declared_input_{}", Uuid::new_v4().simple());
        let pack = helpers::create_test_pack(&ctx.pool, &pack_ref).await?;
        let action_ref = format!("{pack_ref}.source");
        let action =
            helpers::create_test_action(&ctx.pool, pack.id, &pack_ref, &action_ref).await?;
        // Author the real action BEFORE its immutable release/admission. No
        // test rewrites a running execution's captured contract.
        sqlx::query("UPDATE action SET parameter_delivery=$2,parameter_format=$3 WHERE id=$1")
            .bind(action.id)
            .bind(delivery)
            .bind(format)
            .execute(&ctx.pool)
            .await?;
        helpers::activate_test_pack_release_with_projections(
            &ctx.pool,
            &pack,
            &attune_common::repositories::component_lifecycle::PackProjectionIds {
                actions: vec![action.id],
                ..Default::default()
            },
        )
        .await?;
        let execution = helpers::delegated_source::create(
            &ctx.pool,
            attune_common::repositories::execution::CreateExecutionInput {
                action: Some(action.id),
                action_ref,
                executor: Some(owner),
                permission_set_refs: vec!["standard".into()],
                ..Default::default()
            },
        )
        .await?;
        let token = helpers::delegated_source::execution_token(&ctx, execution.id).await?;
        let (mut spec, selected, resource) = admitted_token(&ctx, &token).await?;
        spec.invocation
            .env
            .insert("ATTUNE_PARAMETER_DELIVERY".into(), delivery.into());
        spec.invocation
            .env
            .insert("ATTUNE_PARAMETER_FORMAT".into(), format.into());
        let master = ctx.state.config.security.encryption_key.as_deref().unwrap();
        Effects::reserve(&ctx.pool, &spec, owner, &selected, master).await?;
        Effects::begin_create(&ctx.pool, spec.intent, master).await?;
        Effects::record_resource(&ctx.pool, spec.intent, &resource).await?;
        let payload = Effects::consume_stdin(&ctx.pool, spec.intent, &resource, master).await?;
        // Formatting/private file materialization belongs to the guest,
        // never a manager path or a second input serialization contract.
        assert_eq!(
            payload.as_bytes(),
            br#"{"value":"owned-private-parameters"}"#,
        );
        assert!(!spec.invocation.env.contains_key("ATTUNE_PARAMETER_FILE"));
        assert!(
            Effects::consume_stdin(&ctx.pool, spec.intent, &resource, master)
                .await
                .is_err()
        );
    }
    ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn declared_file_input_transports_admitted_json_without_manager_paths() -> Result<()> {
    declared_input_case("file").await
}

#[tokio::test]
async fn declared_stdin_formats_keep_one_shot_canonical_transport() -> Result<()> {
    declared_input_case("stdin").await
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

#[tokio::test]
async fn native_secret_input_file_publication_rejects_replaced_and_restored_ciphertext(
) -> Result<()> {
    use attune_common::{
        auth::WorkerTokenProvider,
        models::{enums::ArtifactBodyState, OwnerType},
        repositories::{
            artifact::ArtifactVersionRepository,
            executable_snapshot::ExecutableSnapshotRepository,
            execution::CreateExecutionInput,
            execution_secret_value::ExecutionSecretValueRepository,
            identity::{
                CreatePermissionAssignmentInput, CreatePermissionSetInput,
                PermissionAssignmentRepository, PermissionSetRepository,
            },
            key::{CreateKeyInput, KeyRepository},
            recorded_input_authority::RecordedInputAuthorityRepository,
            schema_contract::{BindingOwner, SchemaContractRepository},
            Create,
        },
        secret_provenance::SecretProvenance,
        secret_values::{
            prepare_secret_values, redaction_marker, SecretValueInput, ENTITY_EXECUTION_CONFIG,
        },
    };
    use axum::http::StatusCode;
    attune_common::auth::install_crypto_provider();
    let ctx = TestContext::new().await?.with_admin_auth().await?;
    let result = async {
        let owner = ctx.user.as_ref().unwrap().id;
        let master = ctx.state.config.security.encryption_key.as_deref().unwrap();
        let key = KeyRepository::create_encrypted_in_pool(
            &ctx.pool,
            CreateKeyInput {
                local_ref: format!("native_file_origin_{}", Uuid::new_v4().simple()),
                owner_type: OwnerType::Identity,
                owner_identity: Some(owner),
                owner_pack: None,
                owner_pack_ref: None,
                owner_action: None,
                owner_action_ref: None,
                owner_sensor: None,
                owner_sensor_ref: None,
                name: "Owned native input origin".into(),
                encrypted: true,
                encryption_key_hash: None,
                value: json!("owned-secret-input"),
            },
            master,
        )
        .await?;
        let permission = PermissionSetRepository::create(
            &ctx.pool,
            CreatePermissionSetInput {
                r#ref: format!("native_file_input_{}", Uuid::new_v4().simple()),
                pack: None,
                pack_ref: None,
                label: None,
                description: None,
                grants: json!([{
                    "resource":"keys", "actions":["read","decrypt"],
                    "constraints":{"refs":[key.r#ref]}
                }]),
            },
        )
        .await?;
        PermissionAssignmentRepository::create(
            &ctx.pool,
            CreatePermissionAssignmentInput {
                identity: owner,
                permset: permission.id,
            },
        )
        .await?;
        let source_ref = serde_json::to_string(&SecretProvenance::key(
            attune_common::key_access::key_origin(&key),
        ))?;
        let prepare = || {
            prepare_secret_values(
                vec![SecretValueInput {
                    json_path: "/value".into(),
                    value: json!("owned-secret-input"),
                    source_kind: "provenance".into(),
                    source_ref: Some(source_ref.clone()),
                }],
                master,
            )
        };
        let original = prepare()?;
        let pack_ref = format!("secret_file_{}", Uuid::new_v4().simple());
        let pack = helpers::create_test_pack(&ctx.pool, &pack_ref).await?;
        let action_ref = format!("{pack_ref}.source");
        let action =
            helpers::create_test_action(&ctx.pool, pack.id, &pack_ref, &action_ref).await?;
        helpers::activate_test_pack_release_with_projections(
            &ctx.pool,
            &pack,
            &attune_common::repositories::component_lifecycle::PackProjectionIds {
                actions: vec![action.id],
                ..Default::default()
            },
        )
        .await?;
        let mut tx = ctx.pool.begin().await?;
        let snapshot =
            ExecutableSnapshotRepository::resolve_for_action(&mut *tx, action.id).await?;
        let binding = SchemaContractRepository::materialize_action(&mut tx, &snapshot).await?;
        let row = ExecutionRepository::create_pinned(
            &mut *tx,
            CreateExecutionInput {
                action: Some(action.id),
                action_ref,
                executor: Some(owner),
                permission_set_refs: vec!["standard".into()],
                timeout_seconds: Some(300),
                status: attune_common::models::ExecutionStatus::Requested,
                ..Default::default()
            },
            &snapshot,
        )
        .await?;
        let execution = row.id;
        SchemaContractRepository::attach(&mut tx, BindingOwner::Execution, execution, &binding)
            .await?;
        DelegatedExecutionRepository::select(&mut tx, execution, None).await?;
        RecordedInputAuthorityRepository::capture_human(&mut tx, "execution", execution, owner)
            .await?;
        ExecutionRepository::initialize_config_with_conn(
            &mut tx,
            execution,
            None,
            &snapshot,
            Some(json!({"value": redaction_marker()})),
            &original,
        )
        .await?;
        helpers::delegated::start(&mut tx, execution).await?;
        tx.commit().await?;
        let (spec, selected, resource) = admitted_execution(&ctx, execution).await?;
        let LaunchTarget::Execution { worker, .. } = spec.target else {
            unreachable!()
        };
        Effects::reserve(&ctx.pool, &spec, owner, &selected, master).await?;
        Effects::begin_create(&ctx.pool, spec.intent, master).await?;
        Effects::record_resource(&ctx.pool, spec.intent, &resource).await?;
        let input = Effects::consume_stdin(&ctx.pool, spec.intent, &resource, master).await?;
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(input.as_bytes())?,
            json!({"value":"owned-secret-input"})
        );
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

        // Equal plaintext with new ciphertext is a new selected input. Restoring
        // the exact old bytes cannot revive the effect's earlier source epochs.
        ExecutionSecretValueRepository::upsert_many(
            &ctx.pool,
            ENTITY_EXECUTION_CONFIG,
            execution,
            &prepare()?,
        )
        .await?;
        assert_eq!(
            file_upload_admission(&ctx, &token, path).await?,
            StatusCode::FORBIDDEN
        );
        ExecutionSecretValueRepository::upsert_many(
            &ctx.pool,
            ENTITY_EXECUTION_CONFIG,
            execution,
            &original,
        )
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
        Ok(())
    }
    .await;
    ctx.cleanup().await?;
    result
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

async fn wait_for_database_blocker(pool: &sqlx::PgPool, blocker: i32) -> Result<()> {
    tokio::time::timeout(std::time::Duration::from_secs(20), async {
        loop {
            let waiting: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE $1=ANY(pg_blocking_pids(pid)))",
            )
            .bind(blocker)
            .fetch_one(pool)
            .await?;
            if waiting {
                return Ok::<_, sqlx::Error>(());
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await??;
    Ok(())
}

#[tokio::test]
async fn native_file_allocation_commits_before_exit_closes_run() -> Result<()> {
    use axum::{
        body::Body,
        http::{Request, StatusCode},
    };
    use tower::ServiceExt;
    attune_common::auth::install_crypto_provider();
    let ctx = TestContext::new().await?.with_admin_auth().await?;
    let (spec, selected, resource) = admitted(&ctx).await?;
    let master = ctx.state.config.security.encryption_key.as_deref().unwrap();
    let owner = ctx.user.as_ref().unwrap().id;
    let LaunchTarget::Execution { execution, .. } = spec.target else {
        unreachable!()
    };
    Effects::reserve(&ctx.pool, &spec, owner, &selected, master).await?;
    Effects::begin_create(&ctx.pool, spec.intent, master).await?;
    Effects::record_resource(&ctx.pool, spec.intent, &resource).await?;
    Effects::consume_stdin(&ctx.pool, spec.intent, &resource, master).await?;
    let version = ordinary_file_version(&ctx, execution).await?;
    let token = helpers::delegated_source::execution_token(&ctx, execution).await?;
    // Block only target insertion. The request must retain its source Run
    // while waiting here, so a concurrent exit cannot miss this allocation.
    let mut blocker = ctx.pool.begin().await?;
    let blocker_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *blocker)
        .await?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtext('artifact_version'),hashtext($1::text))")
        .bind(version.artifact.to_string())
        .execute(&mut *blocker)
        .await?;
    let app = ctx.app.clone();
    let request = Request::post(format!(
        "/api/v1/artifacts/{}/versions/file",
        version.artifact
    ))
    .header("authorization", format!("Bearer {token}"))
    .header("content-type", "application/json")
    .body(Body::from("{}"))?;
    let mut allocation = Some(tokio::spawn(async move { app.oneshot(request).await }));
    let mut blocker = Some(blocker);
    let mut exit = None;
    let race = async {
        wait_for_database_blocker(&ctx.pool, blocker_pid).await?;
        // This exact operation row is held FOR SHARE by the admitted request.
        let operation_waiter: i32 = sqlx::query_scalar(
            "SELECT pid FROM pg_stat_activity WHERE $1=ANY(pg_blocking_pids(pid))",
        )
        .bind(blocker_pid)
        .fetch_one(&ctx.pool)
        .await?;
        let pool = ctx.pool.clone();
        let intent = spec.intent;
        let resource_copy = resource.clone();
        exit = Some(tokio::spawn(async move {
            Effects::record_exit(
                &pool,
                intent,
                &resource_copy,
                LaunchExitOutcome {
                    exit_code: 0,
                    oom_killed: false,
                },
            )
            .await
        }));
        wait_for_database_blocker(&ctx.pool, operation_waiter).await?;
        blocker.take().unwrap().commit().await?;
        let response = allocation.take().unwrap().await??;
        if response.status() != StatusCode::CREATED {
            return Err("admitted allocation did not commit".into());
        }
        exit.take().unwrap().await??;
        Ok::<_, Box<dyn std::error::Error>>(())
    }
    .await;
    // Failure teardown releases the owned blocker and joins both owners before
    // the fixture pool/database can be disposed.
    if let Some(blocker) = blocker.take() {
        blocker.rollback().await?;
    }
    if race.is_err() {
        if let Some(allocation) = allocation {
            allocation.abort();
            let _ = allocation.await;
        }
        if let Some(exit) = exit {
            exit.abort();
            let _ = exit.await;
        }
        ctx.cleanup().await?;
        return race;
    }
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM artifact_version WHERE artifact=$1 AND execution=$2",
    )
    .bind(version.artifact)
    .bind(execution)
    .fetch_one(&ctx.pool)
    .await?;
    assert_eq!(count, 2);
    for path in [
        format!("/api/v1/artifacts/{}/versions/file", version.artifact),
        format!(
            "/api/v1/artifacts/ref/{}.late-native/versions/file",
            ExecutionRepository::find_by_id(&ctx.pool, execution)
                .await?
                .unwrap()
                .action_ref
        ),
    ] {
        let response = ctx
            .app
            .clone()
            .oneshot(
                Request::post(path)
                    .header("authorization", format!("Bearer {token}"))
                    .header("content-type", "application/json")
                    .body(Body::from("{}"))?,
            )
            .await?;
        assert!(matches!(
            response.status(),
            StatusCode::FORBIDDEN | StatusCode::UNAUTHORIZED
        ));
    }
    let after: i64 = sqlx::query_scalar("SELECT count(*) FROM artifact_version WHERE execution=$1")
        .bind(execution)
        .fetch_one(&ctx.pool)
        .await?;
    assert_eq!(after, 2);
    ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn runtime_file_allocation_cannot_attribute_a_different_execution() -> Result<()> {
    use axum::{
        body::Body,
        http::{Request, StatusCode},
    };
    use tower::ServiceExt;
    attune_common::auth::install_crypto_provider();
    let ctx = TestContext::new().await?.with_admin_auth().await?;
    let (spec, selected, resource) = admitted(&ctx).await?;
    let master = ctx.state.config.security.encryption_key.as_deref().unwrap();
    let owner = ctx.user.as_ref().unwrap().id;
    let LaunchTarget::Execution { execution, .. } = spec.target else {
        unreachable!()
    };
    Effects::reserve(&ctx.pool, &spec, owner, &selected, master).await?;
    Effects::begin_create(&ctx.pool, spec.intent, master).await?;
    Effects::record_resource(&ctx.pool, spec.intent, &resource).await?;
    Effects::consume_stdin(&ctx.pool, spec.intent, &resource, master).await?;
    let version = ordinary_file_version(&ctx, execution).await?;
    let row = ExecutionRepository::find_by_id(&ctx.pool, execution)
        .await?
        .unwrap();
    let token = helpers::delegated_source::execution_token(&ctx, execution).await?;
    for path in [
        format!("/api/v1/artifacts/{}/versions/file", version.artifact),
        format!(
            "/api/v1/artifacts/ref/{}.wrong-native/versions/file",
            row.action_ref
        ),
    ] {
        let response = ctx
            .app
            .clone()
            .oneshot(
                Request::post(path)
                    .header("authorization", format!("Bearer {token}"))
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::to_vec(
                        &json!({"execution": execution + 1000000}),
                    )?))?,
            )
            .await?;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM artifact_version WHERE execution=$1")
        .bind(execution)
        .fetch_one(&ctx.pool)
        .await?;
    assert_eq!(count, 1);
    let response = ctx
        .app
        .clone()
        .oneshot(
            Request::post(format!(
                "/api/v1/artifacts/ref/{}.positive-native/versions/file",
                row.action_ref
            ))
            .header("authorization", format!("Bearer {token}"))
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(&json!({
                "scope": attune_common::models::OwnerType::Action,
                "owner": row.action_ref,
                "visibility": attune_common::models::enums::ArtifactVisibility::Private,
                "type": attune_common::models::enums::ArtifactType::FileBinary,
            }))?))?,
        )
        .await?;
    assert_eq!(response.status(), StatusCode::CREATED);
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM artifact_version WHERE execution=$1")
        .bind(execution)
        .fetch_one(&ctx.pool)
        .await?;
    assert_eq!(count, 2);
    ctx.cleanup().await?;
    Ok(())
}

async fn admitted_cli(
    ctx: &TestContext,
    delivery: &str,
) -> Result<(ProcessSpec, SelectedExecutionInput, ResourceIdentity)> {
    admitted_typed(ctx, delivery, false).await
}

async fn admitted_typed(
    ctx: &TestContext,
    delivery: &str,
    command: bool,
) -> Result<(ProcessSpec, SelectedExecutionInput, ResourceIdentity)> {
    use attune_common::repositories::{
        action::{ActionRepository, UpdateActionInput},
        component_lifecycle::PackProjectionIds,
        runtime::{CreateRuntimeInput, RuntimeRepository},
        Create, Patch,
    };
    let pack_ref = format!("native_cli_{}", Uuid::new_v4().simple());
    let pack = helpers::create_test_pack(&ctx.pool, &pack_ref).await?;
    let runtime = RuntimeRepository::create(
        &ctx.pool,
        CreateRuntimeInput {
            r#ref: format!("{pack_ref}.native"),
            pack: Some(pack.id),
            pack_ref: Some(pack_ref.clone()),
            description: None,
            name: "native".into(),
            aliases: Vec::new(),
            distributions: json!({}),
            installation: None,
            execution_config: json!({}),
            auto_detected: false,
            detection_config: json!({}),
        },
    )
    .await?;
    let action_ref = format!("{pack_ref}.source");
    let action = helpers::create_test_action(&ctx.pool, pack.id, &pack_ref, &action_ref).await?;
    let launch = if command {
        json!({"type":"command","program":"/echo","arguments":[{"literal":"static literal; $(false)","order":0}]})
    } else {
        json!({"type":"native","artifact":"owned","arguments":[{"literal":"static literal; $(false)","order":0}]})
    };
    let action = ActionRepository::update(&ctx.pool, action.id, UpdateActionInput {
        entrypoint: Some(Patch::Clear), runtime: Some(runtime.id),
        launch: Some(Patch::Set(serde_json::from_value(launch)?)),
        parameter_delivery: Some(delivery.into()), parameter_format: Some("json".into()),
        param_schema: Some(json!({"value":{"type":"string","required":true,"cli":{"kind":"positional","order":1}}})),
        ..Default::default()
    }).await?;
    helpers::activate_test_pack_release_with_projections(
        &ctx.pool,
        &pack,
        &PackProjectionIds {
            actions: vec![action.id],
            runtimes: vec![runtime.id],
            ..Default::default()
        },
    )
    .await?;
    let execution = helpers::delegated_source::create(
        &ctx.pool,
        attune_common::repositories::execution::CreateExecutionInput {
            action: Some(action.id),
            action_ref,
            executor: Some(ctx.user.as_ref().unwrap().id),
            permission_set_refs: vec!["standard".into()],
            ..Default::default()
        },
    )
    .await?;
    let token = helpers::delegated_source::execution_token(ctx, execution.id).await?;
    admitted_token(ctx, &token).await
}

async fn cli_stdin_case(delivery: &str) -> Result<()> {
    use attune_common::{
        action_cli::SelectedProgram, repositories::execution_program::ExecutionProgramRepository,
    };
    attune_common::auth::install_crypto_provider();
    let ctx = TestContext::new().await?.with_admin_auth().await?;
    let (mut spec, selected, resource) = admitted_cli(&ctx, delivery).await?;
    let owner = ctx.user.as_ref().unwrap().id;
    let master = ctx.state.config.security.encryption_key.as_deref().unwrap();
    let LaunchTarget::Execution {
        execution, worker, ..
    } = spec.target
    else {
        unreachable!()
    };
    let row = ExecutionRepository::find_by_id(&ctx.pool, execution)
        .await?
        .unwrap();
    let release = row.pack_release.unwrap();
    let program = SelectedProgram {
        path: "/opt/attune/pack/bin/owned".into(),
        sha256: Sha256Digest::of_bytes(b"owned-public-program")
            .as_str()
            .into(),
        invocation_name: "/opt/attune/pack/bin/owned".into(),
        artifact: Some("owned".into()),
        variant: Some("linux-amd64-static".into()),
    };
    ExecutionProgramRepository::record(&ctx.pool, execution, worker, release, &program).await?;
    spec.invocation.argv = vec![
        "/opt/attune/runner/linux-amd64".into(),
        "exec".into(),
        program.sha256.clone(),
        program.path.to_str().unwrap().into(),
        program.invocation_name.clone(),
        "static literal; $(false)".into(),
        "owned-private-parameters".into(),
    ];
    let mut changed = spec.clone();
    changed
        .invocation
        .argv
        .last_mut()
        .unwrap()
        .replace_range(.., "owned-private-parametera");
    assert!(
        Effects::reserve(&ctx.pool, &changed, owner, &selected, master)
            .await
            .is_err()
    );
    let count: i64 =
        sqlx::query_scalar("SELECT count(*) FROM execution_process_effect WHERE intent=$1")
            .bind(spec.intent)
            .fetch_one(&ctx.pool)
            .await?;
    assert_eq!(count, 0);
    Effects::reserve(&ctx.pool, &spec, owner, &selected, master).await?;
    Effects::begin_create(&ctx.pool, spec.intent, master).await?;
    Effects::record_resource(&ctx.pool, spec.intent, &resource).await?;
    let ciphertext: String = sqlx::query_scalar(
        "SELECT encrypted_context FROM execution_process_effect WHERE intent=$1",
    )
    .bind(spec.intent)
    .fetch_one(&ctx.pool)
    .await?;
    assert!(!ciphertext.contains("owned-private-parameters"));
    let payload = Effects::consume_stdin(&ctx.pool, spec.intent, &resource, master).await?;
    if delivery == "none" {
        assert!(payload.as_bytes().is_empty());
    } else {
        assert_eq!(
            payload.as_bytes(),
            br#"{"value":"owned-private-parameters"}"#
        );
    }
    drop(payload); // Model losing the effect ACK; never authorize a second write.
    assert!(
        Effects::consume_stdin(&ctx.pool, spec.intent, &resource, master)
            .await
            .is_err()
    );
    ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn native_cli_none_input_keeps_exact_argv_and_one_shot_empty_stdin() -> Result<()> {
    cli_stdin_case("none").await
}

#[tokio::test]
async fn native_cli_json_input_keeps_exact_argv_and_original_stdin() -> Result<()> {
    cli_stdin_case("stdin").await
}

#[tokio::test]
async fn command_inspection_is_not_action_authority_and_selection_is_exact() -> Result<()> {
    use attune_common::action_cli::SelectedProgram;
    attune_common::auth::install_crypto_provider();
    let ctx = TestContext::new().await?.with_admin_auth().await?;
    let (mut action, input, resource) = admitted_typed(&ctx, "stdin", true).await?;
    let owner = ctx.user.as_ref().unwrap().id;
    let master = ctx.state.config.security.encryption_key.as_deref().unwrap();
    let mut probe = action.clone();
    probe.intent = Uuid::new_v4();
    probe.purpose = LaunchPurpose::RuntimeProbe;
    probe.invocation.argv = vec![
        "/opt/attune/runner/linux-amd64".into(),
        "inspect-command-program".into(),
        "/echo".into(),
    ];
    probe.invocation.env.remove("ATTUNE_API_TOKEN");
    let mut token_probe = probe.clone();
    token_probe
        .invocation
        .env
        .insert("ATTUNE_API_TOKEN".into(), "must-not-be-accepted".into());
    assert!(
        Effects::reserve(&ctx.pool, &token_probe, owner, &input, master)
            .await
            .is_err()
    );
    Effects::reserve(&ctx.pool, &probe, owner, &input, master).await?;
    Effects::begin_create(&ctx.pool, probe.intent, master).await?;
    Effects::record_resource(&ctx.pool, probe.intent, &resource).await?;
    let empty = Effects::consume_stdin(&ctx.pool, probe.intent, &resource, master).await?;
    assert!(empty.as_bytes().is_empty());
    drop(empty); // A lost acknowledgement is not a second inspection.
    assert!(
        Effects::consume_stdin(&ctx.pool, probe.intent, &resource, master)
            .await
            .is_err()
    );
    let program = SelectedProgram {
        path: "/echo".into(),
        sha256: Sha256Digest::of_bytes(b"same-image-echo").as_str().into(),
        invocation_name: "/echo".into(),
        artifact: None,
        variant: None,
    };
    // No selection before positive exit, even with otherwise correct metadata.
    assert!(Effects::complete_command_inspection(
        &ctx.pool,
        probe.intent,
        &resource,
        &program,
        master
    )
    .await
    .is_err());
    Effects::record_exit(
        &ctx.pool,
        probe.intent,
        &resource,
        LaunchExitOutcome {
            exit_code: 0,
            oom_killed: false,
        },
    )
    .await?;
    let mut tx = ctx.pool.begin().await?;
    assert!(Effects::lock_completion(&mut tx, probe.intent, master)
        .await
        .is_err());
    tx.rollback().await?;
    let LaunchTarget::Execution {
        execution, attempt, ..
    } = probe.target
    else {
        unreachable!()
    };
    let token = helpers::delegated_source::execution_token(&ctx, execution).await?;
    let claims = attune_common::auth::jwt::validate_token(&token, &ctx.state.jwt_config)?;
    let mut tx = ctx.pool.begin().await?;
    assert!(
        DelegatedExecutionRepository::validate_claims(&mut tx, &claims)
            .await
            .is_err()
    );
    tx.rollback().await?;
    let version = ordinary_file_version(&ctx, execution).await?;
    let mut tx = ctx.pool.begin().await?;
    assert!(Effects::prepare_file_publication(
        &mut tx,
        version.id,
        execution,
        attempt,
        Some(master)
    )
    .await
    .is_err());
    tx.rollback().await?;
    let mut wrong_program = program.clone();
    wrong_program.invocation_name = "/else".into();
    assert!(Effects::complete_command_inspection(
        &ctx.pool,
        probe.intent,
        &resource,
        &wrong_program,
        master
    )
    .await
    .is_err());
    Effects::complete_command_inspection(&ctx.pool, probe.intent, &resource, &program, master)
        .await?;
    assert_eq!(
        Effects::inspected_program(&ctx.pool, &action, master).await?,
        Some(program.clone())
    );
    action.invocation.argv = vec![
        "/opt/attune/runner/linux-amd64".into(),
        "exec".into(),
        program.sha256.clone(),
        "/echo".into(),
        "/echo".into(),
        "static literal; $(false)".into(),
        "owned-private-parameters".into(),
    ];
    let mut changed_image = action.clone();
    changed_image.image.sha256 = Sha256Digest::of_bytes(b"different image");
    changed_image.image.reference = format!("sha256:{}", changed_image.image.sha256.as_str());
    assert!(
        Effects::reserve(&ctx.pool, &changed_image, owner, &input, master)
            .await
            .is_err()
    );
    let mut changed_platform = action.clone();
    changed_platform.platform = LaunchPlatform::LinuxArm64;
    changed_platform.invocation.argv[0] = "/opt/attune/runner/linux-arm64".into();
    assert!(
        Effects::reserve(&ctx.pool, &changed_platform, owner, &input, master)
            .await
            .is_err()
    );
    // An idempotent metadata publisher owns probe -> program rows. Observe
    // the action waiting on that probe, then prove its metadata read did not
    // take the inverse program -> probe lock order.
    let mut barrier = ctx.pool.begin().await?;
    let blocker: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *barrier)
        .await?;
    let _: Uuid = sqlx::query_scalar(
        "SELECT intent FROM execution_process_effect WHERE intent=$1 FOR UPDATE",
    )
    .bind(probe.intent)
    .fetch_one(&mut *barrier)
    .await?;
    let pool = ctx.pool.clone();
    let action_copy = action.clone();
    let master_copy = master.to_owned();
    let mut reserve = tokio::spawn(async move {
        Effects::reserve(&pool, &action_copy, owner, &input, &master_copy).await
    });
    let mut barrier = Some(barrier);
    let ordered = async {
        wait_for_database_blocker(&ctx.pool, blocker).await?;
        assert!(!reserve.is_finished());
        sqlx::query(
            "UPDATE execution_program_selection SET selection=selection WHERE execution=$1",
        )
        .bind(execution)
        .execute(&mut **barrier.as_mut().unwrap())
        .await?;
        barrier.take().unwrap().commit().await?;
        (&mut reserve).await??;
        Ok::<_, Box<dyn std::error::Error>>(())
    }
    .await;
    if let Some(barrier) = barrier {
        barrier.rollback().await?;
    }
    if !reserve.is_finished() {
        reserve.abort();
        let _ = reserve.await;
    }
    if let Err(error) = ordered {
        ctx.cleanup().await?;
        return Err(error);
    }
    // Same attempt has two receipts; only the Action may consume action input.
    Effects::begin_create(&ctx.pool, action.intent, master).await?;
    Effects::record_resource(&ctx.pool, action.intent, &resource).await?;
    let original = Effects::consume_stdin(&ctx.pool, action.intent, &resource, master).await?;
    assert_eq!(
        original.as_bytes(),
        br#"{"value":"owned-private-parameters"}"#
    );
    let mut tx = ctx.pool.begin().await?;
    assert!(
        DelegatedExecutionRepository::validate_claims(&mut tx, &claims)
            .await?
            .is_some()
    );
    tx.commit().await?;
    ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn command_inspection_rejects_failed_and_revoked_proofs_after_restoration() -> Result<()> {
    use attune_common::action_cli::SelectedProgram;
    attune_common::auth::install_crypto_provider();
    for exit_code in [1, 0] {
        let ctx = TestContext::new().await?.with_admin_auth().await?;
        let (action, input, resource) = admitted_typed(&ctx, "none", true).await?;
        let owner = ctx.user.as_ref().unwrap().id;
        let master = ctx.state.config.security.encryption_key.as_deref().unwrap();
        let mut probe = action.clone();
        probe.intent = Uuid::new_v4();
        probe.purpose = LaunchPurpose::RuntimeProbe;
        probe.invocation.argv = vec![
            "/opt/attune/runner/linux-amd64".into(),
            "inspect-command-program".into(),
            "/echo".into(),
        ];
        probe.invocation.env.remove("ATTUNE_API_TOKEN");
        Effects::reserve(&ctx.pool, &probe, owner, &input, master).await?;
        Effects::begin_create(&ctx.pool, probe.intent, master).await?;
        Effects::record_resource(&ctx.pool, probe.intent, &resource).await?;
        Effects::consume_stdin(&ctx.pool, probe.intent, &resource, master).await?;
        Effects::record_exit(
            &ctx.pool,
            probe.intent,
            &resource,
            LaunchExitOutcome {
                exit_code,
                oom_killed: false,
            },
        )
        .await?;
        let program = SelectedProgram {
            path: "/echo".into(),
            sha256: Sha256Digest::of_bytes(b"same-image-echo").as_str().into(),
            invocation_name: "/echo".into(),
            artifact: None,
            variant: None,
        };
        if exit_code != 0 {
            assert!(Effects::complete_command_inspection(
                &ctx.pool,
                probe.intent,
                &resource,
                &program,
                master
            )
            .await
            .is_err());
            assert!(Effects::inspected_program(&ctx.pool, &action, master)
                .await?
                .is_none());
        } else {
            Effects::complete_command_inspection(
                &ctx.pool,
                probe.intent,
                &resource,
                &program,
                master,
            )
            .await?;
            sqlx::query("UPDATE identity SET frozen=TRUE WHERE id=$1")
                .bind(owner)
                .execute(&ctx.pool)
                .await?;
            sqlx::query("UPDATE identity SET frozen=FALSE WHERE id=$1")
                .bind(owner)
                .execute(&ctx.pool)
                .await?;
            // Restoration cannot restore either captured selection or a late
            // positive output acknowledgement from the old inspection.
            assert!(Effects::inspected_program(&ctx.pool, &action, master)
                .await
                .is_err());
            assert!(Effects::complete_command_inspection(
                &ctx.pool,
                probe.intent,
                &resource,
                &program,
                master
            )
            .await
            .is_err());
        }
        ctx.cleanup().await?;
    }
    Ok(())
}

#[tokio::test]
async fn command_inspection_worker_expiry_during_receipt_wait_cannot_publish() -> Result<()> {
    use attune_common::action_cli::SelectedProgram;
    attune_common::auth::install_crypto_provider();
    let ctx = TestContext::new().await?.with_admin_auth().await?;
    let (mut probe, input, resource) = admitted_typed(&ctx, "none", true).await?;
    let owner = ctx.user.as_ref().unwrap().id;
    let master = ctx
        .state
        .config
        .security
        .encryption_key
        .as_deref()
        .unwrap()
        .to_owned();
    probe.purpose = LaunchPurpose::RuntimeProbe;
    probe.invocation.argv = vec![
        "/opt/attune/runner/linux-amd64".into(),
        "inspect-command-program".into(),
        "/echo".into(),
    ];
    probe.invocation.env.remove("ATTUNE_API_TOKEN");
    Effects::reserve(&ctx.pool, &probe, owner, &input, &master).await?;
    Effects::begin_create(&ctx.pool, probe.intent, &master).await?;
    Effects::record_resource(&ctx.pool, probe.intent, &resource).await?;
    Effects::consume_stdin(&ctx.pool, probe.intent, &resource, &master).await?;
    Effects::record_exit(
        &ctx.pool,
        probe.intent,
        &resource,
        LaunchExitOutcome {
            exit_code: 0,
            oom_killed: false,
        },
    )
    .await?;
    let LaunchTarget::Execution {
        worker, execution, ..
    } = probe.target
    else {
        unreachable!()
    };
    let mut barrier = ctx.pool.begin().await?;
    let blocker: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *barrier)
        .await?;
    let _: Uuid = sqlx::query_scalar(
        "SELECT intent FROM execution_process_effect WHERE intent=$1 FOR UPDATE",
    )
    .bind(probe.intent)
    .fetch_one(&mut *barrier)
    .await?;
    let deadline: chrono::DateTime<chrono::Utc> = sqlx::query_scalar(
        "UPDATE worker SET last_heartbeat=clock_timestamp()-interval '86 seconds' WHERE id=$1 RETURNING last_heartbeat+interval '90 seconds'"
    ).bind(worker).fetch_one(&ctx.pool).await?;
    let pool = ctx.pool.clone();
    let mut call = tokio::spawn(async move {
        let program = SelectedProgram {
            path: "/echo".into(),
            sha256: Sha256Digest::of_bytes(b"same-image-echo").as_str().into(),
            invocation_name: "/echo".into(),
            artifact: None,
            variant: None,
        };
        Effects::complete_command_inspection(&pool, probe.intent, &resource, &program, &master)
            .await
    });
    let mut barrier = Some(barrier);
    let result = async {
        wait_for_database_blocker(&ctx.pool, blocker).await?;
        assert!(!call.is_finished());
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            loop {
                let expired: bool = sqlx::query_scalar("SELECT clock_timestamp()>=$1")
                    .bind(deadline)
                    .fetch_one(&ctx.pool)
                    .await?;
                if expired {
                    return Ok::<_, sqlx::Error>(());
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await??;
        barrier.take().unwrap().commit().await?;
        assert!(
            (&mut call).await?.is_err(),
            "expired worker cannot publish inspected metadata"
        );
        let captured: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM execution_program_selection WHERE execution=$1",
        )
        .bind(execution)
        .fetch_one(&ctx.pool)
        .await?;
        assert_eq!(captured, 0);
        Ok::<_, Box<dyn std::error::Error>>(())
    }
    .await;
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
