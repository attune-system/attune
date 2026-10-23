mod helpers;
mod managed_readmission_fixture;

use attune_common::{
    repositories::{
        action::{ActionRepository, UpdateActionInput},
        execution::{CreateExecutionInput, ExecutionRepository},
        identity::{
            CreatePermissionAssignmentInput, CreatePermissionSetInput,
            PermissionAssignmentRepository, PermissionSetRepository,
        },
        managed_executable::ManagedExecutableRepository,
        runtime_authority::{RuntimeAuthorityRepository, RuntimeOwner, RuntimePolicyRevision},
        schema_type::SchemaTypeRepository,
        Create, FindByRef,
    },
    runtime_authority::{HumanAttribution, InvocationBinding, RequestAttribution},
};
use axum::http::StatusCode;
use helpers::{Result, TestContext};
use serde_json::json;

async fn expect_status(
    response: helpers::TestResponse,
    status: StatusCode,
    stage: &str,
) -> Result<serde_json::Value> {
    let actual = response.status();
    let body = response.text().await?;
    assert_eq!(actual, status, "{stage}: {body}");
    Ok(serde_json::from_str(&body)?)
}

// Use the same pg_blocking_pids observation and bounded yield loop as the
// cache API lock-order regressions. Timing alone is not a barrier.
async fn wait_for_blocking_edge(
    observer: &mut sqlx::PgConnection,
    blocker: i32,
    waiter: Option<i32>,
) -> Result<i32> {
    Ok(tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            let waiting: Option<i32> = sqlx::query_scalar(
                "SELECT pid FROM pg_stat_activity WHERE datid = (SELECT oid FROM pg_database WHERE datname = current_database()) \
                 AND ($2::INT IS NULL OR pid = $2) AND $1 = ANY(pg_blocking_pids(pid))",
            ).bind(blocker).bind(waiter).fetch_optional(&mut *observer).await?;
            if let Some(pid) = waiting { return Ok::<_, sqlx::Error>(pid); }
            tokio::task::yield_now().await;
        }
    }).await??)
}

async fn preview_releases_graph_before_queued_writer_proceeds(
    ctx: &TestContext,
    token: &str,
    action: i64,
    path: &str,
    revision: i64,
) -> Result<()> {
    // This intentionally graph-free test blocker pauses only the endpoint's
    // target validation. It does not implement a production mutation path.
    let mut blocker = ctx.pool.begin().await?;
    sqlx::query("SELECT id FROM action WHERE id = $1 FOR UPDATE")
        .bind(action)
        .fetch_one(&mut *blocker)
        .await?;
    let blocker_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *blocker)
        .await?;
    let mut writer = ctx.pool.begin().await?;
    let writer_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *writer)
        .await?;
    let mut observer = ctx.pool.acquire().await?;
    let (start_writer, writer_ready) = tokio::sync::oneshot::channel();
    let mutation = async {
        writer_ready.await?;
        SchemaTypeRepository::lock_reclamation(&mut writer).await?;
        ActionRepository::update(
            &mut *writer,
            action,
            UpdateActionInput {
                label: Some("Graph writer completed".into()),
                ..Default::default()
            },
        )
        .await?;
        writer.commit().await?;
        Ok::<_, Box<dyn std::error::Error>>(())
    };
    let observation = async {
        let result: Result<()> = async {
            let reader_pid = wait_for_blocking_edge(&mut observer, blocker_pid, None).await?;
            let graph_held: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM pg_locks WHERE pid=$1 AND locktype='advisory' \
                 AND mode='ShareLock' AND granted AND objsubid=1 \
                 AND ((classid::BIGINT << 32) | objid::BIGINT) = hashtextextended(current_schema() || ':named-type-graph', 0))",
            ).bind(reader_pid).fetch_one(&mut *observer).await?;
            if !graph_held { return Err("execution-options locked its target before shared graph admission".into()); }
            start_writer.send(()).map_err(|_| "graph writer barrier closed")?;
            wait_for_blocking_edge(&mut observer, reader_pid, Some(writer_pid)).await?;
            let queued_on_graph: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM pg_locks held JOIN pg_locks queued \
                 USING (locktype,database,classid,objid,objsubid) \
                 WHERE held.pid=$1 AND queued.pid=$2 AND held.locktype='advisory' \
                 AND held.mode='ShareLock' AND held.granted \
                 AND queued.mode='ExclusiveLock' AND NOT queued.granted)",
            ).bind(reader_pid).bind(writer_pid).fetch_one(&mut *observer).await?;
            if !queued_on_graph { return Err("graph writer was not queued behind the preview's shared graph lock".into()); }
            Ok(())
        }.await;
        // Always release our target blocker, including on failed observation.
        blocker.rollback().await?;
        result
    };
    let deadline = std::time::Duration::from_secs(30);
    let (response, mutation, observation) = tokio::join!(
        tokio::time::timeout(deadline, ctx.get(path, Some(token))),
        tokio::time::timeout(deadline, mutation),
        observation,
    );
    observation?;
    let response =
        expect_status(response??, StatusCode::OK, "GET with queued graph writer").await?;
    mutation??;
    assert_eq!(response["data"]["policy_revision_id"], revision);
    assert_eq!(
        response["data"]["default_permission_set_refs"],
        json!(["standard"])
    );
    Ok(())
}

async fn admitted_options_action(
    ctx: &TestContext,
) -> Result<(attune_common::models::Action, RuntimePolicyRevision)> {
    let actor = ctx.user.as_ref().unwrap().id;
    let pack = helpers::create_test_pack(&ctx.pool, "options_metadata").await?;
    let source = ctx.test_packs_dir.join("options_metadata-release");
    std::fs::create_dir_all(source.join("actions"))?;
    std::fs::create_dir(source.join("runtimes"))?;
    std::fs::write(
        source.join("pack.yaml"),
        "ref: options_metadata\nversion: '1.0.0'\nconf_schema: {}\nconfig: {}\n",
    )?;
    std::fs::write(
        source.join("runtimes/native.yaml"),
        "ref: options_metadata.native\nname: Options fixture native\nexecution_config: {}\n",
    )?;
    std::fs::write(
        source.join("actions/run.yaml"),
        "ref: options_metadata.run\nrunner_type: options_metadata.native\nentry_point: run.sh\ndefault_execution_permission_set_refs: []\n",
    )?;
    std::fs::write(source.join("actions/run.sh"), "#!/bin/sh\nexit 0\n")?;
    let original = managed_readmission_fixture::deploy(ctx, &pack, &source, actor).await?;
    let action = ActionRepository::find_by_ref(&ctx.pool, "options_metadata.run")
        .await?
        .ok_or("deployed action is missing")?;
    // Local human execute permission must use the actual structural pack proof.
    // Neither the reserved standard choice nor any runtime set is assigned here.
    let rights = PermissionSetRepository::create(&ctx.pool, CreatePermissionSetInput {
        r#ref: "options_metadata.editor".into(), pack: Some(pack.id), pack_ref: Some(pack.r#ref.clone()), label: None, description: None,
        grants: json!([
            {"resource":"actions","actions":["execute"],"constraints":{"managing_pack_id":pack.id,"refs":["options_metadata.run"]}},
            {"resource":"actions","actions":["update"],"constraints":{"refs":["options_metadata.run"]}}
        ]),
    }).await?;
    PermissionAssignmentRepository::create(
        &ctx.pool,
        CreatePermissionAssignmentInput {
            identity: actor,
            permset: rights.id,
        },
    )
    .await?;
    attune_api::authz::AuthorizationService::invalidate_identity_authz_cache(actor).await;
    attune_api::authz::AuthorizationService::invalidate_permission_set_caches().await;
    // One deployment must prepare the original metadata pin. No manual API
    // readmission is allowed to repair a missing deployment producer here.
    let binding = InvocationBinding::Action {
        action_ref: action.r#ref.clone(),
    };
    assert!(original
        .policy
        .invocations
        .iter()
        .find(|invocation| invocation.binding == binding)
        .ok_or("initial Action binding is missing")?
        .metadata_revision
        .is_some());
    Ok((action, original))
}

#[tokio::test]
async fn execution_options_use_metadata_readmission_not_code_or_live_defaults() -> Result<()> {
    let ctx = TestContext::new().await?.with_auth().await?;
    let actor = ctx.user.as_ref().unwrap().id;
    let token = ctx.token.as_deref().unwrap();
    let (action, original) = admitted_options_action(&ctx).await?;
    let binding = InvocationBinding::Action {
        action_ref: action.r#ref.clone(),
    };
    let mut tx = ctx.pool.begin().await?;
    let retained =
        ManagedExecutableRepository::resolve_invocation(&mut tx, &original.policy, &binding)
            .await?;
    assert!(retained
        .executable
        .action
        .default_execution_permission_set_refs
        .is_empty());
    let execution = ExecutionRepository::create_pinned(
        &mut *tx,
        CreateExecutionInput {
            action: Some(action.id),
            action_ref: action.r#ref.clone(),
            executor: Some(actor),
            permission_set_refs: vec![],
            config: Some(json!({})),
            ..Default::default()
        },
        &retained,
    )
    .await?;
    let selected = original.policy.select(
        original.id,
        &binding,
        &[],
        RequestAttribution::Human {
            actor: HumanAttribution {
                identity_id: actor,
                login: ctx.user.as_ref().unwrap().login.clone(),
            },
        },
    )?;
    RuntimeAuthorityRepository::record_admission(
        &mut tx,
        RuntimeOwner::Execution(execution.id),
        &selected,
    )
    .await?;
    tx.commit().await?;

    let path = "/api/v1/actions/options_metadata.run/execution-options";
    let initial = expect_status(
        ctx.get(path, Some(token)).await?,
        StatusCode::OK,
        "initial admitted empty-default GET",
    )
    .await?;
    assert_eq!(initial["data"]["default_permission_set_refs"], json!([]));
    // Real API edit captures a new metadata revision on the SAME code release.
    expect_status(
        ctx.put(
            "/api/v1/actions/options_metadata.run",
            json!({"default_execution_permission_set_refs":["standard"]}),
            Some(token),
        )
        .await?,
        StatusCode::OK,
        "standard-default metadata PUT",
    )
    .await?;
    let mut tx = ctx.pool.begin().await?;
    let current = RuntimeAuthorityRepository::current_policy(&mut tx, action.pack)
        .await?
        .unwrap();
    assert_ne!(current.id, original.id);
    assert_eq!(current.policy.release_id, original.policy.release_id);
    assert_ne!(
        current.policy.invocations[0].metadata_revision,
        original.policy.invocations[0].metadata_revision
    );
    let old = ManagedExecutableRepository::resolve_invocation(&mut tx, &original.policy, &binding)
        .await?;
    assert!(old
        .executable
        .action
        .default_execution_permission_set_refs
        .is_empty());
    // The old execution's stored snapshot and explicit empty selection survive.
    let stored: serde_json::Value =
        sqlx::query_scalar("SELECT executable_snapshot FROM execution WHERE id = $1")
            .bind(execution.id)
            .fetch_one(&mut *tx)
            .await?;
    assert_eq!(
        stored["executable"]["action"]["default_execution_permission_set_refs"],
        json!([])
    );
    let stored_refs: Vec<String> =
        sqlx::query_scalar("SELECT permission_set_refs FROM execution WHERE id = $1")
            .bind(execution.id)
            .fetch_one(&mut *tx)
            .await?;
    assert!(stored_refs.is_empty());
    tx.commit().await?;
    let updated = expect_status(
        ctx.get(path, Some(token)).await?,
        StatusCode::OK,
        "GET after standard-default readmission",
    )
    .await?;
    assert_eq!(updated["data"]["policy_revision_id"], current.id);
    assert_eq!(
        updated["data"]["default_permission_set_refs"],
        json!(["standard"])
    );
    assert_eq!(updated["data"]["default_selection_available"], true);
    // Bypass readmission only to simulate an arbitrary mutable projection edit.
    ActionRepository::update(
        &ctx.pool,
        action.id,
        UpdateActionInput {
            default_execution_permission_set_refs: Some(vec![]),
            ..Default::default()
        },
    )
    .await?;
    let unchanged = expect_status(
        ctx.get(path, Some(token)).await?,
        StatusCode::OK,
        "GET after unadmitted projection edit",
    )
    .await?;
    assert_eq!(
        unchanged["data"]["default_permission_set_refs"],
        json!(["standard"])
    );
    assert_eq!(unchanged["data"]["policy_revision_id"], current.id);

    // A managed source without a policy pointer must not become a legacy
    // human-delegation preview, even though its code release still exists.
    sqlx::query("DELETE FROM pack_runtime_policy_current WHERE pack = $1")
        .bind(action.pack)
        .execute(&ctx.pool)
        .await?;
    ctx.get(path, Some(token))
        .await?
        .assert_status(StatusCode::FORBIDDEN);
    let mut tx = ctx.pool.begin().await?;
    RuntimeAuthorityRepository::activate_policy(&mut tx, current.id).await?;
    let mut without_metadata = current.policy.0.clone();
    for invocation in &mut without_metadata.invocations {
        invocation.metadata_revision = None;
    }
    let missing = RuntimeAuthorityRepository::capture_policy(&mut tx, &without_metadata).await?;
    RuntimeAuthorityRepository::activate_policy(&mut tx, missing.id).await?;
    tx.commit().await?;
    ctx.get(path, Some(token))
        .await?
        .assert_status(StatusCode::FORBIDDEN);
    let mut tx = ctx.pool.begin().await?;
    RuntimeAuthorityRepository::activate_policy(&mut tx, current.id).await?;
    RuntimeAuthorityRepository::revoke_policy(&mut tx, current.id).await?;
    tx.commit().await?;
    ctx.get(path, Some(token))
        .await?
        .assert_status(StatusCode::FORBIDDEN);
    ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn execution_options_reader_finishes_before_queued_graph_writer() -> Result<()> {
    let ctx = TestContext::new().await?.with_auth().await?;
    let token = ctx.token.as_deref().unwrap();
    let (action, original) = admitted_options_action(&ctx).await?;
    expect_status(
        ctx.put(
            "/api/v1/actions/options_metadata.run",
            json!({"default_execution_permission_set_refs":["standard"]}),
            Some(token),
        )
        .await?,
        StatusCode::OK,
        "graph barrier standard-default metadata PUT",
    )
    .await?;
    let mut tx = ctx.pool.begin().await?;
    let current = RuntimeAuthorityRepository::current_policy(&mut tx, action.pack)
        .await?
        .unwrap();
    assert_ne!(current.id, original.id);
    tx.rollback().await?;
    preview_releases_graph_before_queued_writer_proceeds(
        &ctx,
        token,
        action.id,
        "/api/v1/actions/options_metadata.run/execution-options",
        current.id,
    )
    .await?;
    ctx.cleanup().await?;
    Ok(())
}
