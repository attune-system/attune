mod helpers;

use attune_common::{
    auth::jwt::generate_access_token,
    repositories::{
        identity::{
            CreateIdentityInput, CreatePermissionAssignmentInput, CreatePermissionSetInput,
            IdentityRepository, PermissionAssignmentRepository, PermissionSetRepository,
            UpdateIdentityInput,
        },
        pack::PackRepository,
        trigger::TriggerRepository,
        Create, Delete, FindByRef, Update,
    },
};
use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use helpers::{Result, TestContext};
use serde_json::{json, Value};
use std::time::Duration;
use tower::ServiceExt;

async fn actor(ctx: &TestContext, name: &str, grants: Value) -> Result<(i64, i64, String)> {
    let identity = IdentityRepository::create(
        &ctx.pool,
        CreateIdentityInput {
            login: name.into(),
            display_name: None,
            attributes: json!({}),
            password_hash: None,
        },
    )
    .await?;
    let set = PermissionSetRepository::create(
        &ctx.pool,
        CreatePermissionSetInput {
            r#ref: format!("fixture.{name}"),
            pack: None,
            pack_ref: None,
            label: None,
            description: None,
            grants,
        },
    )
    .await?;
    let assignment = PermissionAssignmentRepository::create(
        &ctx.pool,
        CreatePermissionAssignmentInput {
            identity: identity.id,
            permset: set.id,
        },
    )
    .await?;
    Ok((
        identity.id,
        assignment.id,
        generate_access_token(identity.id, name, &ctx.state.jwt_config)?,
    ))
}

fn post(
    ctx: &TestContext,
    path: &str,
    token: &str,
    body: Value,
) -> tokio::task::JoinHandle<axum::response::Response> {
    let app = ctx.app.clone();
    let request = Request::builder()
        .method("POST")
        .uri(path)
        .header("content-type", "application/json")
        .header("authorization", format!("Bearer {token}"))
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap();
    tokio::spawn(async move { app.oneshot(request).await.unwrap() })
}

async fn wait_for_blocked(ctx: &TestContext, blocker: i32) -> Result<()> {
    tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            let waiting: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity a WHERE $1 = ANY(pg_blocking_pids(a.pid)))").bind(blocker).fetch_one(&ctx.pool).await?;
            if waiting { return Ok::<_, sqlx::Error>(()); }
            tokio::task::yield_now().await;
        }
    }).await.map_err(|_| format!("No request blocked on owned backend {blocker}"))??;
    Ok(())
}

#[tokio::test]
async fn pack_scoped_ad_hoc_lifecycle_preserves_association_without_management() -> Result<()> {
    let ctx = TestContext::new().await?;
    let pack = helpers::create_test_pack(&ctx.pool, "associated").await?;
    let other = helpers::create_test_pack(&ctx.pool, "adjacent").await?;
    let (_, _, bearer) = actor(&ctx, "associated_actor", json!([{ "resource":"triggers", "actions":["create","update","delete"], "constraints":{"pack_refs":[pack.r#ref]} }])).await?;
    for (pack_ref, expected) in [
        (&pack.r#ref, StatusCode::CREATED),
        (&other.r#ref, StatusCode::FORBIDDEN),
    ] {
        let response = ctx
            .post(
                "/api/v1/triggers",
                json!({"ref":format!("{pack_ref}.created"),"pack_ref":pack_ref,"label":"Created"}),
                Some(&bearer),
            )
            .await?;
        assert_eq!(response.status(), expected, "{}", response.text().await?);
    }
    helpers::create_test_trigger(&ctx.pool, other.id, "adjacent.existing").await?;
    for toggle in ["disable", "enable"] {
        assert_eq!(
            ctx.post(
                &format!("/api/v1/triggers/associated.created/{toggle}"),
                json!({}),
                Some(&bearer)
            )
            .await?
            .status(),
            StatusCode::OK
        );
        assert_eq!(
            ctx.post(
                &format!("/api/v1/triggers/adjacent.existing/{toggle}"),
                json!({}),
                Some(&bearer)
            )
            .await?
            .status(),
            StatusCode::FORBIDDEN
        );
    }
    assert_eq!(
        ctx.delete("/api/v1/triggers/adjacent.existing", Some(&bearer))
            .await?
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        ctx.delete("/api/v1/triggers/associated.created", Some(&bearer))
            .await?
            .status(),
        StatusCode::OK
    );
    ctx.cleanup().await?;
    Ok(())
}

async fn create_after_revocation(kind: &str) -> Result<()> {
    let ctx = TestContext::new().await?;
    let pack = helpers::create_test_pack(&ctx.pool, "race_create").await?;
    let (identity, assignment, bearer) = actor(&ctx,"racer",json!([{ "resource":"triggers", "actions":["create"], "constraints":{"pack_refs":[pack.r#ref]} }])).await?;
    let mut blocker = ctx.pool.begin().await?;
    PackRepository::acquire_mutation_lock(&mut blocker, &pack.r#ref).await?;
    let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *blocker)
        .await?;
    let request = post(
        &ctx,
        "/api/v1/triggers",
        &bearer,
        json!({"ref":"race_create.denied","pack_ref":pack.r#ref,"label":"Denied"}),
    );
    wait_for_blocked(&ctx, pid).await?;
    if kind == "freeze" {
        IdentityRepository::update(
            &ctx.pool,
            identity,
            UpdateIdentityInput {
                frozen: Some(true),
                ..Default::default()
            },
        )
        .await?;
    } else {
        PermissionAssignmentRepository::delete(&ctx.pool, assignment).await?;
    }
    blocker.commit().await?;
    let response = tokio::time::timeout(Duration::from_secs(20), request).await??;
    assert!(matches!(
        response.status(),
        StatusCode::FORBIDDEN | StatusCode::UNAUTHORIZED
    ));
    assert!(
        TriggerRepository::find_by_ref(&ctx.pool, "race_create.denied")
            .await?
            .is_none()
    );
    ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn create_rechecks_freeze_after_pack_lock_wait() -> Result<()> {
    create_after_revocation("freeze").await
}
#[tokio::test]
async fn create_rechecks_revoked_assignment_after_pack_lock_wait() -> Result<()> {
    create_after_revocation("revoke").await
}

// A SHARE table lock admits preflight SELECTs but fences the INSERT after IAM.
// Revocation must then wait for the authorized publication transaction.
#[tokio::test]
async fn create_holds_fresh_grant_through_insert_and_commit() -> Result<()> {
    let ctx = TestContext::new().await?;
    let pack = helpers::create_test_pack(&ctx.pool, "held_create").await?;
    let (_, assignment, bearer) = actor(&ctx,"held_actor",json!([{ "resource":"triggers", "actions":["create"], "constraints":{"pack_refs":[pack.r#ref]} }])).await?;
    let mut blocker = ctx.pool.begin().await?;
    sqlx::query("LOCK TABLE trigger IN SHARE MODE")
        .execute(&mut *blocker)
        .await?;
    let blocker_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *blocker)
        .await?;
    let request = post(
        &ctx,
        "/api/v1/triggers",
        &bearer,
        json!({"ref":"held_create.accepted","pack_ref":pack.r#ref,"label":"Accepted"}),
    );
    wait_for_blocked(&ctx, blocker_pid).await?;
    let mut revoker = ctx.pool.begin().await?;
    let revoker_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *revoker)
        .await?;
    let revocation = tokio::spawn(async move {
        PermissionAssignmentRepository::delete(&mut *revoker, assignment).await?;
        revoker.commit().await?;
        Ok::<_, attune_common::Error>(())
    });
    tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            let blocked: bool = sqlx::query_scalar("SELECT cardinality(pg_blocking_pids($1)) > 0")
                .bind(revoker_pid)
                .fetch_one(&ctx.pool)
                .await?;
            if blocked {
                return Ok::<_, sqlx::Error>(());
            }
            assert!(
                !revocation.is_finished(),
                "IAM lock was released before INSERT publication"
            );
            tokio::task::yield_now().await;
        }
    })
    .await??;
    blocker.commit().await?;
    let response = tokio::time::timeout(Duration::from_secs(20), request).await??;
    assert_eq!(response.status(), StatusCode::CREATED);
    tokio::time::timeout(Duration::from_secs(20), revocation).await???;
    assert!(
        TriggerRepository::find_by_ref(&ctx.pool, "held_create.accepted")
            .await?
            .is_some()
    );
    assert_eq!(
        ctx.post(
            "/api/v1/triggers",
            json!({"ref":"held_create.denied","pack_ref":pack.r#ref,"label":"Denied"}),
            Some(&bearer)
        )
        .await?
        .status(),
        StatusCode::FORBIDDEN
    );
    ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn sync_rechecks_owner_role_removal_under_pack_namespace_lock() -> Result<()> {
    use attune_common::repositories::identity::{
        CreateIdentityRoleAssignmentInput, IdentityRoleAssignmentRepository,
    };
    let ctx = TestContext::new().await?;
    let pack = helpers::create_test_pack(&ctx.pool, "role_sync").await?;
    let (identity, _, bearer) = actor(
        &ctx,
        "role_installer",
        json!([{ "resource":"packs", "actions":["install"], "constraints":{"owner":"self"} }]),
    )
    .await?;
    let mut connection = ctx.pool.acquire().await?;
    PackRepository::set_owner_roles(&mut connection, pack.id, vec!["workflow-publishers".into()])
        .await?;
    drop(connection);
    let membership = IdentityRoleAssignmentRepository::create(
        &ctx.pool,
        CreateIdentityRoleAssignmentInput {
            identity,
            role: "workflow-publishers".into(),
            source: "test".into(),
            managed: false,
        },
    )
    .await?;
    let mut blocker = ctx.pool.begin().await?;
    PackRepository::acquire_mutation_lock(&mut blocker, &pack.r#ref).await?;
    let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *blocker)
        .await?;
    let request = post(
        &ctx,
        "/api/v1/packs/role_sync/workflows/sync",
        &bearer,
        json!({}),
    );
    wait_for_blocked(&ctx, pid).await?;
    IdentityRoleAssignmentRepository::delete(&ctx.pool, membership.id).await?;
    blocker.commit().await?;
    let response = tokio::time::timeout(Duration::from_secs(20), request).await??;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert!(!ctx.test_packs_dir.join("role_sync").exists());
    ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn failed_file_publication_rolls_back_rows_and_removes_only_owned_files() -> Result<()> {
    use attune_common::repositories::workflow::WorkflowDefinitionRepository;
    let ctx = TestContext::new().await?;
    let pack = helpers::create_test_pack(&ctx.pool, "file_failure").await?;
    let (_,_,bearer)=actor(&ctx,"file_actor",json!([{ "resource":"actions", "actions":["create"], "constraints":{"pack_refs":[pack.r#ref]} }])).await?;
    let directory = ctx.test_packs_dir.join("file_failure/actions");
    std::fs::create_dir_all(&directory)?;
    std::fs::write(directory.join("conflict.yaml"), "unrelated existing file")?;
    let response=ctx.post("/api/v1/packs/file_failure/workflow-files",json!({"name":"conflict","pack_ref":pack.r#ref,"label":"Conflict","version":"1.0.0","definition":{"version":"1.0.0","tasks":[]}}),Some(&bearer)).await?;
    assert_eq!(
        response.status(),
        StatusCode::CONFLICT,
        "{}",
        response.text().await?
    );
    assert_eq!(
        std::fs::read_to_string(directory.join("conflict.yaml"))?,
        "unrelated existing file"
    );
    assert!(!directory.join("workflows/conflict.workflow.yaml").exists());
    assert!(
        WorkflowDefinitionRepository::find_by_ref(&ctx.pool, "file_failure.conflict")
            .await?
            .is_none()
    );
    assert!(!std::fs::read_dir(&directory)?.any(|entry| entry
        .unwrap()
        .file_name()
        .to_string_lossy()
        .starts_with(".workflow-publication-")));
    ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn server_rejected_file_commit_cleans_publication_and_database_rows() -> Result<()> {
    use attune_common::repositories::{
        action::ActionRepository, workflow::WorkflowDefinitionRepository,
    };
    let ctx = TestContext::new().await?;
    let pack = helpers::create_test_pack(&ctx.pool, "commit_failure").await?;
    let (_,_,bearer)=actor(&ctx,"commit_actor",json!([{ "resource":"actions", "actions":["create"], "constraints":{"pack_refs":[pack.r#ref]} }])).await?;
    sqlx::raw_sql("CREATE FUNCTION reject_workflow_commit() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN RAISE EXCEPTION 'owned injected commit rejection'; END $$; CREATE CONSTRAINT TRIGGER reject_workflow_commit AFTER INSERT ON workflow_definition DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION reject_workflow_commit();").execute(&ctx.pool).await?;
    let response=ctx.post("/api/v1/packs/commit_failure/workflow-files",json!({"name":"rejected","pack_ref":pack.r#ref,"label":"Rejected","version":"1.0.0","definition":{"version":"1.0.0","tasks":[]}}),Some(&bearer)).await?;
    assert_eq!(
        response.status(),
        StatusCode::BAD_REQUEST,
        "{}",
        response.text().await?
    );
    assert!(
        WorkflowDefinitionRepository::find_by_ref(&ctx.pool, "commit_failure.rejected")
            .await?
            .is_none()
    );
    assert!(
        ActionRepository::find_by_ref(&ctx.pool, "commit_failure.rejected")
            .await?
            .is_none()
    );
    assert!(!ctx.test_packs_dir.join("commit_failure").exists());
    ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn canceled_file_request_before_commit_cleans_stage_and_rolls_back_rows() -> Result<()> {
    use attune_common::repositories::workflow::WorkflowDefinitionRepository;
    let ctx = TestContext::new().await?;
    let pack = helpers::create_test_pack(&ctx.pool, "cancel_file").await?;
    let (_,_,bearer)=actor(&ctx,"cancel_actor",json!([{ "resource":"actions", "actions":["create"], "constraints":{"pack_refs":[pack.r#ref]} }])).await?;
    let mut blocker = ctx.pool.begin().await?;
    sqlx::query("LOCK TABLE workflow_definition IN SHARE MODE")
        .execute(&mut *blocker)
        .await?;
    let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *blocker)
        .await?;
    let request = post(
        &ctx,
        "/api/v1/packs/cancel_file/workflow-files",
        &bearer,
        json!({"name":"canceled","pack_ref":pack.r#ref,"label":"Canceled","version":"1.0.0","definition":{"version":"1.0.0","tasks":[]}}),
    );
    wait_for_blocked(&ctx, pid).await?;
    request.abort();
    assert!(request.await.unwrap_err().is_cancelled());
    blocker.rollback().await?;
    assert!(
        ctx.state
            .file_transfers
            .drain(Duration::from_secs(20))
            .await
    );
    assert!(!ctx.test_packs_dir.join("cancel_file").exists());
    assert!(
        WorkflowDefinitionRepository::find_by_ref(&ctx.pool, "cancel_file.canceled")
            .await?
            .is_none()
    );
    ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn disconnected_file_waiter_does_not_cancel_commit_owner() -> Result<()> {
    use attune_common::repositories::{
        action::ActionRepository, workflow::WorkflowDefinitionRepository,
    };
    let ctx = TestContext::new().await?;
    let pack = helpers::create_test_pack(&ctx.pool, "commit_disconnect").await?;
    let (_,_,bearer)=actor(&ctx,"disconnect_actor",json!([{ "resource":"actions", "actions":["create"], "constraints":{"pack_refs":[pack.r#ref]} }])).await?;
    sqlx::raw_sql("CREATE FUNCTION hold_workflow_commit() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN PERFORM pg_advisory_xact_lock(126126); RETURN NEW; END $$; CREATE CONSTRAINT TRIGGER hold_workflow_commit AFTER INSERT ON workflow_definition DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION hold_workflow_commit();").execute(&ctx.pool).await?;
    let mut blocker = ctx.pool.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(126126)")
        .execute(&mut *blocker)
        .await?;
    let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *blocker)
        .await?;
    let request = post(
        &ctx,
        "/api/v1/packs/commit_disconnect/workflow-files",
        &bearer,
        json!({"name":"accepted","pack_ref":pack.r#ref,"label":"Accepted","version":"1.0.0","definition":{"version":"1.0.0","tasks":[]}}),
    );
    wait_for_blocked(&ctx, pid).await?;
    assert!(ctx
        .test_packs_dir
        .join("commit_disconnect/actions/accepted.yaml")
        .exists());
    request.abort();
    assert!(request.await.unwrap_err().is_cancelled());
    blocker.rollback().await?;
    assert!(
        ctx.state
            .file_transfers
            .drain(Duration::from_secs(20))
            .await
    );
    assert!(
        WorkflowDefinitionRepository::find_by_ref(&ctx.pool, "commit_disconnect.accepted")
            .await?
            .is_some()
    );
    assert!(
        ActionRepository::find_by_ref(&ctx.pool, "commit_disconnect.accepted")
            .await?
            .is_some()
    );
    assert!(ctx
        .test_packs_dir
        .join("commit_disconnect/actions/workflows/accepted.workflow.yaml")
        .exists());
    ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn sync_cannot_mutate_a_workflow_structurally_owned_by_an_adjacent_pack() -> Result<()> {
    use attune_common::repositories::workflow::WorkflowDefinitionRepository;
    let ctx = TestContext::new().await?;
    let allowed = helpers::create_test_pack(&ctx.pool, "sync_allowed").await?;
    let adjacent = helpers::create_test_pack(&ctx.pool, "sync_adjacent").await?;
    helpers::create_test_action(&ctx.pool, allowed.id, &allowed.r#ref, "sync_allowed.action")
        .await?;
    let foreign = helpers::create_test_workflow(
        &ctx.pool,
        adjacent.id,
        &adjacent.r#ref,
        "sync_adjacent.foreign",
    )
    .await?;
    // Deliberate historical/corrupt association: production repository creation
    // rejects cross-pack refs. Test the importer against an existing bad row.
    sqlx::query("UPDATE workflow_definition SET ref = 'sync_allowed.foreign' WHERE id = $1")
        .bind(foreign.id)
        .execute(&ctx.pool)
        .await?;
    let foreign = WorkflowDefinitionRepository::find_by_ref(&ctx.pool, "sync_allowed.foreign")
        .await?
        .unwrap();
    let (_,_,bearer)=actor(&ctx,"sync_installer",json!([{ "resource":"packs", "actions":["install"], "constraints":{"refs":[allowed.r#ref]} }])).await?;
    let directory = ctx.test_packs_dir.join("sync_allowed/workflows");
    std::fs::create_dir_all(&directory)?;
    std::fs::write(directory.join("foreign.yaml"),"ref: sync_allowed.foreign\nlabel: Must not replace foreign owner\nversion: '1.0.0'\ntasks:\n  - name: run\n    action: sync_allowed.action\n")?;
    let response = ctx
        .post(
            "/api/v1/packs/sync_allowed/workflows/sync",
            json!({}),
            Some(&bearer),
        )
        .await?;
    assert!(
        response.status().is_client_error(),
        "{}",
        response.text().await?
    );
    let unchanged = WorkflowDefinitionRepository::find_by_ref(&ctx.pool, "sync_allowed.foreign")
        .await?
        .unwrap();
    assert_eq!(unchanged.pack, adjacent.id);
    assert_eq!(unchanged.updated, foreign.updated);
    assert_eq!(unchanged.definition, foreign.definition);
    ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn concurrent_workflow_deletes_wait_for_pin_admission_before_companion_locks() -> Result<()> {
    use attune_common::repositories::{
        action::ActionRepository,
        cache::{CacheEntryRepository, CacheTransactionMode},
        workflow::WorkflowDefinitionRepository,
    };
    let ctx = TestContext::new().await?;
    let pack = helpers::create_test_pack(&ctx.pool, "delete_order").await?;
    let (_,_,bearer)=actor(&ctx,"delete_actor",json!([{ "resource":"actions", "actions":["create","delete"], "constraints":{"pack_refs":[pack.r#ref]} }])).await?;
    let created=ctx.post("/api/v1/workflows",json!({"ref":"delete_order.target","pack_ref":pack.r#ref,"label":"Target","version":"1.0.0","definition":{"version":"1.0.0","tasks":[]}}),Some(&bearer)).await?;
    assert_eq!(
        created.status(),
        StatusCode::CREATED,
        "{}",
        created.text().await?
    );
    let workflow = WorkflowDefinitionRepository::find_by_ref(&ctx.pool, "delete_order.target")
        .await?
        .unwrap();
    let companion = ActionRepository::find_by_workflow_def(&ctx.pool, workflow.id)
        .await?
        .unwrap();
    let mut blocker = ctx.pool.begin().await?;
    CacheEntryRepository::protect_transaction(&mut blocker, CacheTransactionMode::PinMutation)
        .await?;
    let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *blocker)
        .await?;
    let mut requests = Vec::new();
    for _ in 0..2 {
        let app = ctx.app.clone();
        let request = Request::builder()
            .method("DELETE")
            .uri("/api/v1/workflows/delete_order.target")
            .header("authorization", format!("Bearer {bearer}"))
            .body(Body::empty())
            .unwrap();
        requests.push(tokio::spawn(
            async move { app.oneshot(request).await.unwrap() },
        ));
    }
    tokio::time::timeout(Duration::from_secs(20), async {
        loop {
            // The first delete waits on pin admission while holding exclusive graph
            // admission; the second waits on that graph holder. Follow the actual
            // wait chain so both requests must be blocked before probing the action.
            let waiting: i64 = sqlx::query_scalar(
                "WITH RECURSIVE waiters(pid) AS (\
                 SELECT pid FROM pg_stat_activity WHERE $1 = ANY(pg_blocking_pids(pid)) \
                 UNION SELECT a.pid FROM pg_stat_activity a JOIN waiters w \
                 ON w.pid = ANY(pg_blocking_pids(a.pid))) \
                 SELECT count(*) FROM waiters",
            )
            .bind(pid)
            .fetch_one(&ctx.pool)
            .await?;
            if waiting == 2 {
                return Ok::<_, sqlx::Error>(());
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .map_err(|_| "Both delete requests did not reach the owned admission wait chain")??;
    // With the old order, both waiters already held FOR SHARE on this action.
    // NOWAIT proves the new admission waiters protect no companion rows yet.
    let mut probe = ctx.pool.begin().await?;
    sqlx::query("SELECT id FROM action WHERE id = $1 FOR UPDATE NOWAIT")
        .bind(companion.id)
        .fetch_one(&mut *probe)
        .await?;
    probe.rollback().await?;
    blocker.rollback().await?;
    let mut statuses = Vec::new();
    for request in requests {
        statuses.push(
            tokio::time::timeout(Duration::from_secs(20), request)
                .await??
                .status(),
        );
    }
    assert_eq!(
        statuses
            .iter()
            .filter(|status| **status == StatusCode::OK)
            .count(),
        1,
        "{statuses:?}"
    );
    assert!(
        statuses.iter().all(|status| matches!(
            *status,
            StatusCode::OK | StatusCode::NOT_FOUND | StatusCode::CONFLICT
        )),
        "{statuses:?}"
    );
    assert!(
        WorkflowDefinitionRepository::find_by_ref(&ctx.pool, "delete_order.target")
            .await?
            .is_none()
    );
    ctx.cleanup().await?;
    Ok(())
}
