use super::object_seal_boundary::SealGetBarrier;
use super::*;
use attune_common::{
    models::LogStreamBackend,
    repositories::{
        artifact::ArtifactVersionRepository,
        file_evidence_retention::FileEvidenceRetentionRepository,
        log_stream::LogStreamRepository,
        sensor_schema::SensorSchemaRepository,
        sensor_workload::{
            AcquireSensorWorkloadInput, AcquireSensorWorkloadOutcome, SensorWorkloadLeaseInput,
            SensorWorkloadRepository,
        },
    },
};
use axum::{body::Body, http::Request};
use futures::FutureExt;
use std::{
    sync::{atomic::Ordering, Arc},
    time::Duration,
};
use tower::ServiceExt;

async fn snapshot(pool: &sqlx::PgPool, stream: i64) -> helpers::Result<Value> {
    Ok(sqlx::query_scalar("SELECT jsonb_build_object('phase',s.close_phase,'revision',s.close_revision,'input',s.close_input,'hash',s.close_input_sha256,'deadline',s.close_deadline,'sealed',s.sealed,'state',v.body_state,'size',v.size_bytes,'digest',v.sha256,'membership',(SELECT jsonb_agg(to_jsonb(x) ORDER BY x.sequence) FROM log_segment x WHERE x.stream=s.id),'evidence',(SELECT jsonb_agg(to_jsonb(e)) FROM runtime_log_final_manifest e WHERE e.artifact_version=v.id)) FROM log_stream s JOIN artifact_version v ON v.id=s.artifact_version WHERE s.id=$1").bind(stream).fetch_one(pool).await?)
}
async fn response(
    app: axum::Router,
    method: &str,
    uri: String,
    token: &str,
    bytes: Vec<u8>,
) -> helpers::Result<StatusCode> {
    let response = app
        .oneshot(
            Request::builder()
                .method(method)
                .uri(uri)
                .header("authorization", format!("Bearer {token}"))
                .body(Body::from(bytes))?,
        )
        .await?;
    let status = response.status();
    let body = axum::body::to_bytes(response.into_body(), 65536).await?;
    eprintln!(
        "object producer HTTP {status}: {}",
        String::from_utf8_lossy(&body)
    );
    Ok(status)
}
// SQL clock advances while the final Source holds SHARE locks; only the target
// version is blocked. No authority mutation is required to pass those locks.
async fn wait_blocked(pool: &sqlx::PgPool, blocker: i32) -> helpers::Result<()> {
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let waiting: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE $1=ANY(pg_blocking_pids(pid)) AND datid=(SELECT oid FROM pg_database WHERE datname=current_database()))").bind(blocker).fetch_one(pool).await?;
            if waiting { return Ok::<_, sqlx::Error>(()); }
            tokio::task::yield_now().await;
        }
    }).await??;
    Ok(())
}
async fn sensor_case(mode: u8) -> helpers::Result<()> {
    static WARM: tokio::sync::OnceCell<()> = tokio::sync::OnceCell::const_new();
    WARM.get_or_try_init(|| async {
        TestContext::new().await?.cleanup().await?;
        Ok::<_, Box<dyn std::error::Error>>(())
    })
    .await?;
    let SensorLogFixture {
        ctx,
        pack,
        sensor,
        workload,
        binding,
        artifact,
    } = sensor_log_fixture().await?;
    let version = ArtifactVersionRepository::create_log_pending(
        &ctx.pool,
        artifact.id,
        &artifact.r#ref,
        LogStreamBackend::ObjectSegments,
        "text/plain".into(),
        None,
        None,
        Some("sensor".into()),
    )
    .await?;
    let mut tx = ctx.pool.begin().await?;
    FileEvidenceRetentionRepository::lock_capture(&mut tx).await?;
    SensorSchemaRepository::attach_log_version(&mut tx, version.id, binding).await?;
    tx.commit().await?;
    let stream = LogStreamRepository::create_with_backend(
        &ctx.pool,
        version.id,
        LogStreamBackend::ObjectSegments,
        32768,
        500,
    )
    .await?;
    let token =
        attune_common::auth::jwt::generate_sensor_token_with_cache_authority_and_workload_fence(
            ctx.user.as_ref().unwrap().id,
            &sensor.r#ref,
            vec![],
            Some(&pack.r#ref),
            &[],
            &[],
            workload.fence(),
            &ctx.state.jwt_config,
            Some(120),
        )?;
    let store = Arc::new(SealGetBarrier {
        inner: ctx.state.blob_store.clone(),
        uploaded: tokio::sync::Notify::new(),
        finish: tokio::sync::Semaphore::new(0),
        gets: std::sync::atomic::AtomicUsize::new(0),
    });
    let mut state = (*ctx.state).clone();
    state.blob_store = store.clone();
    let state = Arc::new(state);
    let app = attune_api::server::Server::new(state.clone()).router();
    let bytes = b"genuine sensor object log\n".repeat(128);
    assert_eq!(
        response(
            app.clone(),
            "PUT",
            format!("/api/v1/internal/logs/{}/segments/0", version.id),
            &token,
            bytes.clone()
        )
        .await?,
        StatusCode::CREATED
    );
    let uri = format!("/api/v1/internal/logs/{}/seal", version.id);
    let request = Request::builder()
        .method("POST")
        .uri(&uri)
        .header("authorization", format!("Bearer {token}"))
        .body(Body::empty())?;
    let http_app = app.clone();
    let http_request = request;
    let mut http = tokio::spawn(async move {
        let result = http_app.oneshot(http_request).await;
        assert!(mode < 4, "injected HTTP waiter panic");
        result
    });
    let mut joined = false;
    let result = std::panic::AssertUnwindSafe(async {
        tokio::time::timeout(Duration::from_secs(10),store.uploaded.notified()).await?;
        if mode==3 {return Err("injected producer GET failure".into());}
        if mode==5 {
            assert!(SensorWorkloadRepository::release(&ctx.pool,workload.fence()).await?);
            return Err("injected pre-join producer failure".into());
        }
        let pending=snapshot(&ctx.pool,stream.id).await?;
        assert_eq!(pending["phase"],"closing"); assert_eq!(pending["state"],"pending"); assert_eq!(pending["sealed"],false); assert!(pending["evidence"].is_null());
        let mut blocker=None;
        if mode==1 {
            assert!(SensorWorkloadRepository::release(&ctx.pool,workload.fence()).await?);
            let fence=workload.fence();
            let next=match SensorWorkloadRepository::acquire_or_renew(&ctx.pool,AcquireSensorWorkloadInput {sensor_id:sensor.id,worker_id:fence.worker_id,worker_instance:fence.worker_instance,lease_seconds:300}).await? {AcquireSensorWorkloadOutcome::Acquired(x)=>x,_=>panic!("reacquire")};
            let next=SensorWorkloadRepository::begin_process(&ctx.pool,next).await?.unwrap();
            assert_ne!(next.generation,workload.generation);
        }
        if mode==2 {
            assert!(SensorWorkloadRepository::renew(&ctx.pool,SensorWorkloadLeaseInput {fence:workload.fence(),lease_seconds:3}).await?.is_some());
            let mut tx=ctx.pool.begin().await?;
            let pid:i32=sqlx::query_scalar("SELECT pg_backend_pid()").fetch_one(&mut *tx).await?;
            sqlx::query("SELECT id FROM artifact_version WHERE id=$1 FOR UPDATE").bind(version.id).execute(&mut *tx).await?;
            blocker=Some((tx,pid));
        }
        store.finish.add_permits(1);
        if let Some((tx,pid))=blocker {
            wait_blocked(&ctx.pool,pid).await?;
            sqlx::query("SELECT pg_sleep(GREATEST(0,extract(epoch FROM (lease_expires_at-clock_timestamp())))::double precision) FROM sensor_workload_assignment WHERE workload=$1").bind(workload.workload_id).execute(&ctx.pool).await?;
            let expired:bool=sqlx::query_scalar("SELECT lease_expires_at<=clock_timestamp() FROM sensor_workload_assignment WHERE workload=$1").bind(workload.workload_id).fetch_one(&ctx.pool).await?; assert!(expired);
            tx.rollback().await?;
        }
        let response=tokio::time::timeout(Duration::from_secs(10),&mut http).await.inspect(|_| { joined=true; })???;
        let status=response.status();
        let body=axum::body::to_bytes(response.into_body(),65536).await?;eprintln!("sensor seal {status}: {}",String::from_utf8_lossy(&body));
        assert_eq!(status,if mode==0 {StatusCode::OK} else {StatusCode::FORBIDDEN});
        let final_state=snapshot(&ctx.pool,stream.id).await?;
        if mode==0 {
            use sha2::{Digest,Sha256};
            assert_eq!(final_state["state"],"ready");assert_eq!(final_state["phase"],"published");assert_eq!(final_state["sealed"],true);
            assert_eq!(final_state["size"],bytes.len() as i64); assert_eq!(final_state["digest"],Sha256::digest(&bytes).iter().map(|b|format!("{b:02x}")).collect::<String>());
            assert!(final_state["evidence"].is_null(),"supported plain sensor has no encrypted worker manifest");
        } else {assert_eq!(final_state,pending);}
        assert_eq!(store.gets.load(Ordering::SeqCst),1);
        Ok::<_,Box<dyn std::error::Error>>(())
    }).catch_unwind().await;
    store.finish.add_permits(10);
    // Releasing GET lets the ordinary production-owned inner seal complete.
    // Aborting only this waiter would detach it; file_transfers does not own it.
    let finished = if !joined {
        http.await.map(|_| ())
    } else {
        Ok(())
    };
    // Capture before teardown, assert only after cleanup so a failed observation
    // cannot recreate the early-exit leak being tested.
    let completion = if mode >= 3 {
        Some(snapshot(&ctx.pool, stream.id).await)
    } else {
        None
    };
    let drained = ctx.state.file_transfers.drain(Duration::from_secs(5)).await;
    drop(app);
    drop(state);
    drop(store);
    ctx.cleanup().await?;
    assert!(drained, "owned seal drain watchdog");
    if let Some(completion) = completion {
        let completion = completion?;
        assert_eq!(
            completion["phase"], "published",
            "inner seal did not publish before cleanup"
        );
        assert_eq!(completion["state"], "ready");
        assert_eq!(completion["sealed"], true);
    }
    finished?;
    match result {
        Ok(result) => result,
        Err(panic) => std::panic::resume_unwind(panic),
    }
}
#[tokio::test]
async fn object_seal_sensor_current_generation_publishes_verified_bytes() -> helpers::Result<()> {
    sensor_case(0).await
}
#[tokio::test]
async fn object_seal_sensor_released_reacquired_generation_denies_old_source() -> helpers::Result<()>
{
    sensor_case(1).await
}
#[tokio::test]
async fn object_seal_sensor_lease_expires_during_actual_target_wait() -> helpers::Result<()> {
    sensor_case(2).await
}

#[tokio::test]
async fn object_seal_native_workflow_after_terminal_completion() -> helpers::Result<()> {
    use attune_common::{
        models::ExecutionStatus,
        repositories::{
            action::ActionRepository,
            component_lifecycle::PackProjectionIds,
            delegated_execution::DelegatedExecutionRepository as Delegated,
            executable_snapshot::ExecutableSnapshotRepository,
            execution::{CreateExecutionInput, ExecutionRepository, UpdateExecutionInput},
            schema_contract::{BindingOwner, SchemaContractRepository},
            workflow::{CreateWorkflowExecutionInput, WorkflowExecutionRepository},
            Update,
        },
    };
    let ctx = TestContext::new().await?.with_auth().await?;
    let result = std::panic::AssertUnwindSafe(async {
        let pack = helpers::create_test_pack(&ctx.pool, "native_seal").await?;
        let action =
            helpers::create_test_action(&ctx.pool, pack.id, &pack.r#ref, "native_seal.workflow")
                .await?;
        let leaf = helpers::create_test_action(&ctx.pool, pack.id, &pack.r#ref, "native_seal.leaf")
            .await?;
        let def =
            helpers::create_test_workflow(&ctx.pool, pack.id, &pack.r#ref, &action.r#ref).await?;
        let graph = attune_common::workflow::parse_workflow_yaml(&format!(
            "version: '1.0.0'\ntasks:\n  - name: finish\n    action: {}\n",
            leaf.r#ref
        ))?;
        attune_common::repositories::workflow::WorkflowDefinitionRepository::update(
            &ctx.pool,
            def.id,
            attune_common::repositories::workflow::UpdateWorkflowDefinitionInput {
                definition: Some(serde_json::to_value(graph)?),
                ..Default::default()
            },
        )
        .await?;
        ActionRepository::link_workflow_def(&ctx.pool, action.id, def.id).await?;
        helpers::activate_test_pack_release_with_projections(
            &ctx.pool,
            &pack,
            &PackProjectionIds {
                actions: vec![action.id, leaf.id],
                workflows: vec![def.id],
                ..Default::default()
            },
        )
        .await?;
        let mut tx = ctx.pool.begin().await?;
        let executable =
            ExecutableSnapshotRepository::resolve_for_action(&mut *tx, action.id).await?;
        let binding = SchemaContractRepository::materialize_action(&mut tx, &executable).await?;
        let execution = ExecutionRepository::create_pinned(
            &mut *tx,
            CreateExecutionInput {
                action: Some(action.id),
                action_ref: action.r#ref.clone(),
                executor: Some(ctx.user.as_ref().unwrap().id),
                status: ExecutionStatus::Requested,
                ..Default::default()
            },
            &executable,
        )
        .await?;
        SchemaContractRepository::attach(&mut tx, BindingOwner::Execution, execution.id, &binding)
            .await?;
        Delegated::select(&mut tx, execution.id, None).await?;
        ExecutionRepository::initialize_config_with_conn(
            &mut tx,
            execution.id,
            None,
            &executable,
            Some(json!({})),
            &[],
        )
        .await?;
        let workflow = WorkflowExecutionRepository::create(
            &mut *tx,
            CreateWorkflowExecutionInput {
                execution: execution.id,
                workflow_def: def.id,
                task_graph: json!({}),
                variables: json!({}),
                status: ExecutionStatus::Running,
            },
        )
        .await?;
        tx.commit().await?;
        let artifact = ArtifactRepository::create(
            &ctx.pool,
            CreateArtifactInput {
                r#ref: format!("{}.workflow.log", action.r#ref),
                scope: OwnerType::Action,
                owner: action.r#ref,
                r#type: ArtifactType::FileText,
                visibility: ArtifactVisibility::Private,
                classification: ArtifactClassification::RuntimeLog,
                retention_policy: RetentionPolicyType::Versions,
                retention_limit: 4,
                name: None,
                description: None,
                content_type: Some("text/plain".into()),
                data: None,
            },
        )
        .await?;
        let version = ArtifactVersionRepository::create_log_pending(
            &ctx.pool,
            artifact.id,
            &artifact.r#ref,
            LogStreamBackend::ObjectSegments,
            "text/plain".into(),
            Some(execution.id),
            Some(json!({"kind":"workflow_log","workflow_execution":workflow.id})),
            Some("executor".into()),
        )
        .await?;
        let stream = LogStreamRepository::create_with_backend(
            &ctx.pool,
            version.id,
            LogStreamBackend::ObjectSegments,
            32768,
            500,
        )
        .await?;
        let token = attune_common::auth::jwt::generate_worker_token(
            0,
            "executor",
            &ctx.state.jwt_config,
            Some(120),
        )?;
        let app = attune_api::server::Server::new(ctx.state.clone()).router();
        let bytes = b"native workflow terminal log\n".repeat(128);
        assert_eq!(
            response(
                app.clone(),
                "PUT",
                format!("/api/v1/internal/logs/{}/segments/0", version.id),
                &token,
                bytes.clone()
            )
            .await?,
            StatusCode::CREATED
        );
        let mut tx = ctx.pool.begin().await?;
        Delegated::close(&mut tx, execution.id).await?;
        WorkflowExecutionRepository::update(
            &mut *tx,
            workflow.id,
            attune_common::repositories::workflow::UpdateWorkflowExecutionInput {
                status: Some(ExecutionStatus::Completed),
                ..Default::default()
            },
        )
        .await?;
        ExecutionRepository::update(
            &mut *tx,
            execution.id,
            UpdateExecutionInput {
                status: Some(ExecutionStatus::Completed),
                ..Default::default()
            },
        )
        .await?;
        tx.commit().await?;
        assert_eq!(
            response(
                app.clone(),
                "POST",
                format!("/api/v1/internal/logs/{}/seal", version.id),
                &token,
                vec![]
            )
            .await?,
            StatusCode::OK
        );
        let state = snapshot(&ctx.pool, stream.id).await?;
        use sha2::{Digest, Sha256};
        assert_eq!(state["phase"], "published");
        assert_eq!(state["state"], "ready");
        assert_eq!(state["sealed"], true);
        assert_eq!(state["size"], bytes.len() as i64);
        assert_eq!(
            state["digest"],
            Sha256::digest(&bytes)
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>()
        );
        assert!(state["evidence"].is_null());
        drop(app);
        Ok::<_, Box<dyn std::error::Error>>(())
    })
    .catch_unwind()
    .await;
    let drained = ctx.state.file_transfers.drain(Duration::from_secs(5)).await;
    ctx.cleanup().await?;
    assert!(drained, "native seal drain watchdog");
    match result {
        Ok(result) => result,
        Err(panic) => std::panic::resume_unwind(panic),
    }
}

#[tokio::test]
async fn object_seal_sensor_gated_get_error_awaits_inner_seal_cleanup() -> helpers::Result<()> {
    let error = sensor_case(3).await.unwrap_err();
    assert_eq!(error.to_string(), "injected producer GET failure");
    Ok(())
}
#[tokio::test]
async fn object_seal_sensor_http_panic_is_joined_once_before_cleanup() -> helpers::Result<()> {
    let error = sensor_case(4).await.unwrap_err();
    let error = error
        .downcast_ref::<tokio::task::JoinError>()
        .expect("HTTP panic must propagate as JoinError after cleanup");
    assert!(error.is_panic());
    Ok(())
}

#[tokio::test]
async fn object_seal_sensor_late_waiter_panic_cannot_mask_failed_publication() -> helpers::Result<()>
{
    let panic = std::panic::AssertUnwindSafe(sensor_case(5))
        .catch_unwind()
        .await
        .expect_err(
            "pending publication must fail its assertion, not return an accepted waiter JoinError",
        );
    let message = panic
        .downcast_ref::<String>()
        .map(String::as_str)
        .or_else(|| panic.downcast_ref::<&str>().copied())
        .expect("publication assertion panic");
    assert!(
        message.contains("inner seal did not publish before cleanup"),
        "unexpected panic: {message}"
    );
    Ok(())
}
