//! Real admitted sources, workflow state, router calls and PostgreSQL barriers.
use super::*;
use crate::helpers::TestResponse;
use attune_common::{
    models::{
        inquiry::{InquiryResponseOption, InquiryResponseOptionStyle},
        workflow::WorkflowTaskWaitTarget,
    },
    repositories::{
        action::ActionRepository,
        inquiry::{CreateWorkflowInquiryInput, InquiryRepository},
        pack::PackRepository,
        schema_type::SchemaTypeRepository,
        workflow::{
            CreateWorkflowDefinitionInput, CreateWorkflowExecutionInput,
            WorkflowDefinitionRepository, WorkflowExecutionRepository,
        },
        workflow_task_wait::{CreateWorkflowTaskWaitInput, WorkflowTaskWaitRepository},
        FindByRef,
    },
};
use tokio::task::JoinHandle;
use tracing::instrument::WithSubscriber;

const ROOT_ACTION: &str = "recorded_emitter.cancel_root";

struct Tree {
    ids: Vec<i64>,
    workflow: i64,
    inquiry: Option<i64>,
}

async fn fixture_and_tree(count: usize, chain: bool) -> Result<(Fixture, Tree)> {
    let mut fixture = fixture_with_additional_grants(false, false, &[], &[], &[]).await?;
    let pack = PackRepository::find_by_id(&fixture.ctx.pool, fixture.pack_id)
        .await?
        .unwrap();
    let emitter = ActionRepository::find_by_ref(&fixture.ctx.pool, "recorded_emitter.emit")
        .await?
        .unwrap();
    let action = create_test_action(
        &fixture.ctx.pool,
        fixture.pack_id,
        "recorded_emitter",
        ROOT_ACTION,
    )
    .await?;
    let definition = json!({"version":"1.0","tasks":[{"name":"emit","action":"recorded_emitter.emit","permission_set_refs":[]}]});
    // Use a complete canonical definition, not the older shared fixture's
    // workflow-only context marker with an incomplete authoring body.
    let _: attune_common::workflow::WorkflowDefinition =
        serde_json::from_value(definition.clone())?;
    let definition = WorkflowDefinitionRepository::create(
        &fixture.ctx.pool,
        CreateWorkflowDefinitionInput {
            r#ref: ROOT_ACTION.into(),
            pack: fixture.pack_id,
            pack_ref: "recorded_emitter".into(),
            label: "Cancellation root".into(),
            description: None,
            version: "1.0.0".into(),
            param_schema: None,
            out_schema: None,
            definition,
            tags: vec![],
        },
    )
    .await?;
    ActionRepository::link_workflow_def(&fixture.ctx.pool, action.id, definition.id).await?;
    activate_test_pack_release_with_projections(
        &fixture.ctx.pool,
        &pack,
        &PackProjectionIds {
            actions: vec![emitter.id, action.id],
            ..Default::default()
        },
    )
    .await?;
    let mut tx = fixture.ctx.pool.begin().await?;
    let snapshot =
        ExecutableSnapshotRepository::resolve_for_action_ref(&mut *tx, ROOT_ACTION).await?;
    let mut policy = RuntimeAuthorityRepository::find_policy(&mut tx, fixture.policy_id)
        .await?
        .unwrap()
        .policy
        .0;
    policy.release_id = snapshot.release.id;
    policy.release_digest = snapshot.release.digest.clone();
    for invocation in &mut policy.invocations {
        invocation.target_release_id = snapshot.release.id;
        invocation.target_release_digest = snapshot.release.digest.clone();
        invocation.source_id = None;
        invocation.metadata_revision = None;
        invocation.dispatch_template_digest = None;
        invocation.foreign_workflow_policy = None;
    }
    let mut root_invocation = policy.invocations[0].clone();
    let root_binding = InvocationBinding::Action {
        action_ref: ROOT_ACTION.into(),
    };
    root_invocation.binding = root_binding.clone();
    root_invocation.source_id = None;
    root_invocation.metadata_revision = None;
    root_invocation.dispatch_template_digest = None;
    root_invocation.foreign_workflow_policy = None;
    root_invocation.permission_sets.clear();
    let mut task_invocation = root_invocation.clone();
    task_invocation.binding = InvocationBinding::WorkflowTask {
        workflow_ref: ROOT_ACTION.into(),
        task_path: "/tasks/0".into(),
        action_ref: "recorded_emitter.emit".into(),
    };
    policy.invocations.push(root_invocation);
    policy.invocations.push(task_invocation);
    let policy = RuntimeAuthorityRepository::capture_policy(&mut tx, &policy).await?;
    RuntimeAuthorityRepository::activate_policy(&mut tx, policy.id).await?;
    // Refresh the source against the same actual new release. A policy may not
    // authorize a same-ref execution pinned to the previous code snapshot.
    let emitter_snapshot =
        ExecutableSnapshotRepository::resolve_for_action_ref(&mut *tx, "recorded_emitter.emit")
            .await?;
    let refs = vec!["recorded_emitter.events".to_owned()];
    let source_selected = policy.policy.select(
        policy.id,
        &policy.policy.invocations[0].binding,
        &refs,
        RequestAttribution::Human {
            actor: HumanAttribution {
                identity_id: fixture.requester,
                login: fixture.ctx.user.as_ref().unwrap().login.clone(),
            },
        },
    )?;
    let source = ExecutionRepository::create_pinned(
        &mut *tx,
        CreateExecutionInput {
            action: Some(emitter.id),
            action_ref: emitter.r#ref.clone(),
            status: ExecutionStatus::Running,
            executor: Some(fixture.requester),
            permission_set_refs: refs.clone(),
            config: Some(json!({})),
            ..Default::default()
        },
        &emitter_snapshot,
    )
    .await?;
    let schema = SchemaContractRepository::materialize_action(&mut tx, &emitter_snapshot).await?;
    SchemaContractRepository::attach(&mut tx, BindingOwner::Execution, source.id, &schema).await?;
    let admission = RuntimeAuthorityRepository::record_admission(
        &mut tx,
        RuntimeOwner::Execution(source.id),
        &source_selected,
    )
    .await?;
    fixture.credential = RuntimeCredentialBinding {
        execution_id: source.id,
        admission_id: admission.id,
        sponsor_identity_id: source_selected.owner.sponsor_identity_id,
        action_ref: emitter.r#ref.clone(),
        attempt: source_selected.attempt,
    };
    fixture.token = generate_admitted_execution_token(
        &fixture.credential,
        &refs,
        &fixture.ctx.state.jwt_config,
        Some(360),
    )?;
    let selected = policy.policy.select(
        policy.id,
        &root_binding,
        &[],
        RequestAttribution::Human {
            actor: HumanAttribution {
                identity_id: fixture.requester,
                login: fixture.ctx.user.as_ref().unwrap().login.clone(),
            },
        },
    )?;
    let root = ExecutionRepository::create_pinned(
        &mut *tx,
        CreateExecutionInput {
            action: Some(snapshot.executable.action.id),
            action_ref: snapshot.executable.action.r#ref.clone(),
            status: ExecutionStatus::Running,
            executor: Some(fixture.requester),
            permission_set_refs: vec![],
            config: Some(json!({"private":"root_private_value"})),
            ..Default::default()
        },
        &snapshot,
    )
    .await?;
    let schema = SchemaContractRepository::materialize_action(&mut tx, &snapshot).await?;
    SchemaContractRepository::attach(&mut tx, BindingOwner::Execution, root.id, &schema).await?;
    RuntimeAuthorityRepository::record_admission(
        &mut tx,
        RuntimeOwner::Execution(root.id),
        &selected,
    )
    .await?;
    let workflow = WorkflowExecutionRepository::create(
        &mut *tx,
        CreateWorkflowExecutionInput {
            execution: root.id,
            workflow_def: snapshot.executable.workflow_definition.as_ref().unwrap().id,
            task_graph: json!({
                "nodes":{"emit":{"name":"emit","task_type":"action","action":"recorded_emitter.emit","input":{},"permission_set_refs":[],"transitions":[],"inbound_tasks":[]}},
                "entry_points":["emit"],"inbound_edges":{"emit":[]},"outbound_edges":{"emit":[]},
            }),
            variables: json!({"private":"root_private_value"}),
            status: ExecutionStatus::Running,
        },
    )
    .await?;
    sqlx::query("UPDATE workflow_execution SET current_tasks = ARRAY['emit'] WHERE id = $1")
        .bind(workflow.id)
        .execute(&mut *tx)
        .await?;
    let mut ids = vec![root.id];
    for index in 1..count {
        let parent = if chain { *ids.last().unwrap() } else { root.id };
        let id = child(
            &fixture,
            &mut tx,
            &policy,
            parent,
            workflow.id,
            index as i32,
        )
        .await?;
        ids.push(id);
    }
    // Action-owned inquiries derive their complete scope from a real task
    // execution, not from the containing workflow action. A one-node tree has
    // no task creator; keep that budget case at exactly one execution.
    let inquiry = if let Some(&creator) = ids.get(1) {
        let inquiry = InquiryRepository::create_workflow_inquiry_idempotent(
            &mut tx,
            CreateWorkflowInquiryInput {
                created_by_execution: creator,
                purpose: "continue".into(),
                prompt: "root_private_value".into(),
                response_schema: None,
                response_options: vec![InquiryResponseOption {
                    r#ref: "continue".into(),
                    label: "Continue".into(),
                    style: InquiryResponseOptionStyle::Default,
                    response: json!({}),
                }],
                assigned_to: None,
                timeout_seconds: None,
            },
        )
        .await?;
        assert_eq!(inquiry.created_by_execution, creator);
        assert_eq!(inquiry.workflow_execution, Some(workflow.id));
        assert_eq!(inquiry.workflow_task_name.as_deref(), Some("emit"));
        assert_eq!(inquiry.action_attempt_family, Some(creator));
        assert_eq!(inquiry.purpose.as_deref(), Some("continue"));
        assert_eq!(
            inquiry.status,
            attune_common::models::enums::InquiryStatus::Pending
        );
        WorkflowTaskWaitRepository::create_or_get(
            &mut tx,
            CreateWorkflowTaskWaitInput {
                workflow_execution: workflow.id,
                task_name: "emit".into(),
                target: WorkflowTaskWaitTarget::Inquiry(inquiry.id),
            },
        )
        .await?;
        Some(inquiry.id)
    } else {
        None
    };
    tx.commit().await?;
    fixture.policy_id = policy.id;
    Ok((
        fixture,
        Tree {
            ids,
            workflow: workflow.id,
            inquiry,
        },
    ))
}

async fn child(
    fixture: &Fixture,
    conn: &mut sqlx::PgConnection,
    policy: &attune_common::repositories::runtime_authority::RuntimePolicyRevision,
    parent: i64,
    workflow: i64,
    index: i32,
) -> Result<i64> {
    let source = ExecutionRepository::find_by_id(&mut *conn, fixture.credential.execution_id)
        .await?
        .unwrap();
    let selected = policy.policy.select(
        policy.id,
        &InvocationBinding::WorkflowTask {
            workflow_ref: ROOT_ACTION.into(),
            task_path: "/tasks/0".into(),
            action_ref: "recorded_emitter.emit".into(),
        },
        &[],
        RequestAttribution::ParentExecution {
            execution_id: parent,
        },
    )?;
    let snapshot = source.executable_snapshot.unwrap();
    let execution = ExecutionRepository::create_pinned(
        &mut *conn,
        CreateExecutionInput {
            action: Some(snapshot.executable.action.id),
            action_ref: snapshot.executable.action.r#ref.clone(),
            status: ExecutionStatus::Scheduled,
            parent: Some(parent),
            workflow_task: Some(serde_json::from_value(json!({
                "workflow_execution":workflow,"task_name":"emit","task_index":index,
                "retry_count":0,"max_retries":0,"timed_out":false,
            }))?),
            permission_set_refs: vec![],
            config: Some(json!({"private":"child_private_value"})),
            ..Default::default()
        },
        &snapshot,
    )
    .await?;
    let schema = SchemaContractRepository::materialize_action(&mut *conn, &snapshot).await?;
    SchemaContractRepository::attach(&mut *conn, BindingOwner::Execution, execution.id, &schema)
        .await?;
    RuntimeAuthorityRepository::record_admission(
        conn,
        RuntimeOwner::Execution(execution.id),
        &selected,
    )
    .await?;
    Ok(execution.id)
}

async fn cancel_source(fixture: &mut Fixture, ids: Option<&[i64]>) -> Result<()> {
    let mut grant = json!({"resource":"executions","actions":["cancel"]});
    if let Some(ids) = ids {
        grant["constraints"] = json!({"ids":ids});
    }
    append_local_reader(fixture, grant).await?;
    let (credential, token) = independent_cancel_source(fixture).await?;
    fixture.credential = credential;
    fixture.token = token;
    Ok(())
}

async fn independent_cancel_source(
    fixture: &Fixture,
) -> Result<(RuntimeCredentialBinding, String)> {
    let (credential, token) = independent_source(fixture).await?;
    let mut tx = fixture.ctx.pool.begin().await?;
    let execution = ExecutionRepository::find_by_id(&mut *tx, credential.execution_id)
        .await?
        .unwrap();
    let snapshot = execution.executable_snapshot.unwrap();
    let schema = SchemaContractRepository::materialize_action(&mut tx, &snapshot).await?;
    SchemaContractRepository::attach(&mut tx, BindingOwner::Execution, execution.id, &schema)
        .await?;
    tx.commit().await?;
    Ok((credential, token))
}

fn path(tree: &Tree) -> String {
    format!("/api/v1/executions/{}/cancel", tree.ids[0])
}

async fn states(
    fixture: &Fixture,
    tree: &Tree,
) -> Result<(Vec<(i64, String)>, String, String, String)> {
    let executions =
        sqlx::query_as("SELECT id,status::text FROM execution WHERE id = ANY($1) ORDER BY id")
            .bind(&tree.ids)
            .fetch_all(&fixture.ctx.pool)
            .await?;
    let workflow = sqlx::query_scalar("SELECT status::text FROM workflow_execution WHERE id = $1")
        .bind(tree.workflow)
        .fetch_one(&fixture.ctx.pool)
        .await?;
    let inquiry = sqlx::query_scalar("SELECT status::text FROM inquiry WHERE id = $1")
        .bind(tree.inquiry)
        .fetch_one(&fixture.ctx.pool)
        .await?;
    let wait = sqlx::query_scalar(
        "SELECT state::text FROM workflow_task_wait WHERE workflow_execution = $1",
    )
    .bind(tree.workflow)
    .fetch_one(&fixture.ctx.pool)
    .await?;
    Ok((executions, workflow, inquiry, wait))
}

fn request(fixture: &Fixture, tree: &Tree) -> JoinHandle<TestResponse> {
    use axum::{
        body::Body,
        http::{header, Request},
    };
    use tower::Service;
    let mut app = fixture.ctx.app.clone();
    let request = Request::builder()
        .method("POST")
        .uri(path(tree))
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::AUTHORIZATION, format!("Bearer {}", fixture.token))
        .body(Body::from("{}"))
        .unwrap();
    tokio::spawn(async move {
        TestResponse::new(app.call(request).await.expect("root cancellation request"))
    })
}

async fn finish<T>(mut task: JoinHandle<T>) -> Result<T> {
    match tokio::time::timeout(std::time::Duration::from_secs(15), &mut task).await {
        Ok(result) => Ok(result?),
        Err(_) => {
            task.abort();
            let _ = task.await;
            Err("root cancellation exceeded deadlock bound".into())
        }
    }
}

async fn blocked(pool: &sqlx::PgPool, blocker: i32, query: &str) -> Result<i32> {
    Ok(tokio::time::timeout(std::time::Duration::from_secs(10),async {
        loop {
            let pid: Option<i32> = sqlx::query_scalar("SELECT pid FROM pg_stat_activity WHERE $1 = ANY(pg_blocking_pids(pid)) AND query LIKE $2 LIMIT 1")
                .bind(blocker).bind(format!("%{query}%")).fetch_optional(pool).await?;
            if let Some(pid) = pid { break Ok::<_,sqlx::Error>(pid); }
            tokio::task::yield_now().await;
        }
    }).await??)
}

async fn cleanup(
    blocker: Option<sqlx::Transaction<'static, sqlx::Postgres>>,
    effect: Option<JoinHandle<TestResponse>>,
    writer: Option<JoinHandle<()>>,
) -> Result<()> {
    if let Some(task) = &effect {
        task.abort();
    }
    if let Some(task) = &writer {
        task.abort();
    }
    let rollback = match blocker {
        Some(tx) => tx.rollback().await,
        None => Ok(()),
    };
    let effect = match effect {
        Some(task) => task.await.map(|_| ()).or_else(|error| {
            if error.is_cancelled() {
                Ok(())
            } else {
                Err(error)
            }
        }),
        None => Ok(()),
    };
    let writer = match writer {
        Some(task) => task.await.map(|_| ()).or_else(|error| {
            if error.is_cancelled() {
                Ok(())
            } else {
                Err(error)
            }
        }),
        None => Ok(()),
    };
    rollback?;
    effect?;
    writer?;
    Ok(())
}

#[tokio::test]
async fn running_workflow_root_cancels_each_authorized_descendant_and_prerequisite_without_read_rights(
) -> Result<()> {
    let (mut fixture, tree) = fixture_and_tree(4, true).await?;
    cancel_source(&mut fixture, Some(&tree.ids)).await?;
    sqlx::query("UPDATE execution SET status = CASE WHEN id = $1 THEN 'running'::execution_status_enum ELSE 'completed'::execution_status_enum END WHERE id = ANY($2)")
        .bind(tree.ids[1]).bind(vec![tree.ids[1],tree.ids[3]]).execute(&fixture.ctx.pool).await?;
    let response = fixture
        .ctx
        .post(&path(&tree), json!({}), Some(&fixture.token))
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    let response: Value = response.json().await?;
    assert!(!response.to_string().contains("private_value"));
    assert!(response["data"]["config"].is_null());
    assert!(response["data"]["parent"].is_null());
    let state = states(&fixture, &tree).await?;
    assert_eq!(
        state
            .0
            .iter()
            .map(|(_, status)| status.as_str())
            .collect::<Vec<_>>(),
        vec!["canceling", "canceling", "cancelled", "completed"]
    );
    assert_eq!(
        (state.1, state.2, state.3),
        ("cancelled".into(), "cancelled".into(), "cancelled".into())
    );
    let current_tasks: Vec<String> =
        sqlx::query_scalar("SELECT current_tasks FROM workflow_execution WHERE id = $1")
            .bind(tree.workflow)
            .fetch_one(&fixture.ctx.pool)
            .await?;
    assert!(current_tasks.is_empty());
    fixture.ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn every_descendant_including_terminal_requires_cancel_and_denial_rolls_back_the_whole_tree(
) -> Result<()> {
    for terminal in [false, true] {
        let (mut fixture, tree) = fixture_and_tree(3, false).await?;
        cancel_source(&mut fixture, Some(&tree.ids[..2])).await?;
        if terminal {
            sqlx::query("UPDATE execution SET status = 'completed' WHERE id = $1")
                .bind(tree.ids[2])
                .execute(&fixture.ctx.pool)
                .await?;
        }
        let before = states(&fixture, &tree).await?;
        let response = fixture
            .ctx
            .post(&path(&tree), json!({}), Some(&fixture.token))
            .await?;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        let response: Value = response.json().await?;
        assert!(!response.to_string().contains(&tree.ids[2].to_string()));
        assert!(!response.to_string().contains("private_value"));
        assert_eq!(before, states(&fixture, &tree).await?);
        fixture.ctx.cleanup().await?;
    }
    Ok(())
}

#[tokio::test]
async fn tree_cancellation_queries_are_constant_for_one_64_100_nodes_and_never_scan_global_candidates(
) -> Result<()> {
    for count in [1, 64, 100] {
        let (mut fixture, tree) = fixture_and_tree(count, false).await?;
        cancel_source(&mut fixture, None).await?;
        sqlx::query("INSERT INTO execution(action_ref,status) SELECT 'unrelated.private','running'::execution_status_enum FROM generate_series(1,10001)")
            .execute(&fixture.ctx.pool).await?;
        let mut tx = fixture.ctx.pool.begin().await?;
        let authority =
            RuntimeAuthorityRepository::validate_execution_credential(&mut tx, &fixture.credential)
                .await?;
        let observer = Statements::default();
        let subscriber = tracing_subscriber::registry().with(observer.clone());
        let changes = ExecutionRepository::cancel_recorded_tree(&mut tx, tree.ids[0], &authority)
            .with_subscriber(subscriber)
            .await?;
        assert_eq!(changes.len(), count);
        assert_eq!(observer.0.load(Ordering::Relaxed), 18, "tree nodes={count}");
        assert!(changes
            .iter()
            .all(|(execution, _)| execution.config.is_none()
                && execution.result.is_none()
                && execution.executable_snapshot.is_none()));
        tx.commit().await?;
        let unrelated: i64 = sqlx::query_scalar("SELECT count(*) FROM execution WHERE action_ref = 'unrelated.private' AND status = 'running'")
            .fetch_one(&fixture.ctx.pool).await?;
        assert_eq!(unrelated, 10001);
        fixture.ctx.cleanup().await?;
    }
    Ok(())
}

#[tokio::test]
async fn legal_256_node_tree_is_supported_but_overflow_and_cycles_have_no_effect() -> Result<()> {
    for count in [256, 257] {
        let (mut fixture, tree) = fixture_and_tree(count, false).await?;
        cancel_source(&mut fixture, None).await?;
        let before = states(&fixture, &tree).await?;
        let response = fixture
            .ctx
            .post(&path(&tree), json!({}), Some(&fixture.token))
            .await?;
        assert_eq!(
            response.status(),
            if count == 256 {
                StatusCode::OK
            } else {
                StatusCode::FORBIDDEN
            }
        );
        if count == 257 {
            assert_eq!(before, states(&fixture, &tree).await?);
        }
        fixture.ctx.cleanup().await?;
    }
    let (mut fixture, tree) = fixture_and_tree(3, true).await?;
    cancel_source(&mut fixture, None).await?;
    sqlx::query("UPDATE execution SET parent = $2 WHERE id = $1")
        .bind(tree.ids[0])
        .bind(tree.ids[2])
        .execute(&fixture.ctx.pool)
        .await?;
    let before = states(&fixture, &tree).await?;
    let response = fixture
        .ctx
        .post(&path(&tree), json!({}), Some(&fixture.token))
        .await?;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert_eq!(before, states(&fixture, &tree).await?);
    fixture.ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn queued_graph_writer_does_not_deadlock_cancellation_after_the_execution_target_wait(
) -> Result<()> {
    let (mut fixture, tree) = fixture_and_tree(3, false).await?;
    cancel_source(&mut fixture, None).await?;
    let mut blocker = Some(fixture.ctx.pool.begin().await?);
    let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut **blocker.as_mut().unwrap())
        .await?;
    sqlx::query("SELECT id FROM execution WHERE id = $1 FOR UPDATE")
        .bind(tree.ids[1])
        .fetch_one(&mut **blocker.as_mut().unwrap())
        .await?;
    let mut effect = Some(request(&fixture, &tree));
    let mut writer = None;
    let outcome: Result<()> = async {
        let effect_pid = blocked(&fixture.ctx.pool, pid, "FOR UPDATE").await?;
        let pool = fixture.ctx.pool.clone();
        let (send, receive) = tokio::sync::oneshot::channel();
        writer = Some(tokio::spawn(async move {
            let mut tx = pool.begin().await.unwrap();
            let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
                .fetch_one(&mut *tx)
                .await
                .unwrap();
            let _ = send.send(pid);
            SchemaTypeRepository::lock_reclamation(&mut tx)
                .await
                .unwrap();
            tx.rollback().await.unwrap();
        }));
        let writer_pid =
            tokio::time::timeout(std::time::Duration::from_secs(10), receive).await??;
        let waiting = blocked(&fixture.ctx.pool, effect_pid, "pg_advisory").await?;
        if waiting != writer_pid {
            return Err("graph writer did not wait behind the actual cancellation source".into());
        }
        blocker.take().unwrap().commit().await?;
        let response = finish(effect.take().unwrap()).await?;
        finish(writer.take().unwrap()).await?;
        if response.status() != StatusCode::OK {
            return Err("cancellation failed with a queued graph writer".into());
        }
        let state = states(&fixture, &tree).await?;
        if state
            .0
            .iter()
            .any(|(_, status)| status != "canceling" && status != "cancelled")
        {
            return Err("queued writer prevented exact tree cancellation".into());
        }
        Ok(())
    }
    .await;
    let cleanup = cleanup(blocker, effect, writer).await;
    let database_cleanup = fixture.ctx.cleanup().await;
    cleanup?;
    database_cleanup?;
    outcome
}

#[tokio::test]
async fn scheduler_spawn_committed_during_workflow_lock_wait_rejects_stale_tree_without_partial_effect(
) -> Result<()> {
    let (mut fixture, tree) = fixture_and_tree(3, false).await?;
    cancel_source(&mut fixture, None).await?;
    let mut blocker = Some(fixture.ctx.pool.begin().await?);
    SchemaTypeRepository::lock_selection(blocker.as_mut().unwrap()).await?;
    let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut **blocker.as_mut().unwrap())
        .await?;
    WorkflowExecutionRepository::acquire_advisory_lock(blocker.as_mut().unwrap(), tree.workflow)
        .await?;
    let mut effect = Some(request(&fixture, &tree));
    let outcome: Result<()> = async {
        blocked(&fixture.ctx.pool, pid, "pg_advisory_xact_lock").await?;
        let policy =
            RuntimeAuthorityRepository::find_policy(blocker.as_mut().unwrap(), fixture.policy_id)
                .await?
                .unwrap();
        let spawned = child(
            &fixture,
            blocker.as_mut().unwrap(),
            &policy,
            tree.ids[0],
            tree.workflow,
            99,
        )
        .await?;
        blocker.take().unwrap().commit().await?;
        let response = finish(effect.take().unwrap()).await?;
        if response.status() != StatusCode::CONFLICT {
            return Err("new scheduler child did not reject stale cancellation membership".into());
        }
        let state = states(&fixture, &tree).await?;
        if state.0[0].1 != "running"
            || state.0[1..].iter().any(|(_, status)| status != "scheduled")
            || state.1 != "running"
            || state.2 != "pending"
            || state.3 != "waiting"
        {
            return Err("expansion conflict partially cancelled the tree".into());
        }
        let status: String = sqlx::query_scalar("SELECT status::text FROM execution WHERE id = $1")
            .bind(spawned)
            .fetch_one(&fixture.ctx.pool)
            .await?;
        if status != "scheduled" {
            return Err("racing child was cancelled without membership authorization".into());
        }
        Ok(())
    }
    .await;
    let cleanup = cleanup(blocker, effect, None).await;
    let database_cleanup = fixture.ctx.cleanup().await;
    cleanup?;
    database_cleanup?;
    outcome
}

#[tokio::test]
async fn source_revocation_does_not_revoke_an_independent_same_sponsor_canceller_or_union_its_scope(
) -> Result<()> {
    let (mut fixture, tree) = fixture_and_tree(3, false).await?;
    cancel_source(&mut fixture, Some(&tree.ids[..2])).await?;
    let narrow = fixture.token.clone();
    append_local_reader(
        &mut fixture,
        json!({"resource":"executions","actions":["cancel"]}),
    )
    .await?;
    let (broad, broad_token) = independent_cancel_source(&fixture).await?;
    let (other, other_token) = independent_cancel_source(&fixture).await?;
    assert_eq!(broad.sponsor_identity_id, other.sponsor_identity_id);
    assert_ne!(broad.execution_id, other.execution_id);
    assert_ne!(broad.admission_id, other.admission_id);
    let before = states(&fixture, &tree).await?;
    assert_eq!(
        fixture
            .ctx
            .post(&path(&tree), json!({}), Some(&narrow))
            .await?
            .status(),
        StatusCode::FORBIDDEN
    );
    assert_eq!(before, states(&fixture, &tree).await?);
    let mut tx = fixture.ctx.pool.begin().await?;
    RuntimeAuthorityRepository::revoke_admission(&mut tx, broad.admission_id).await?;
    tx.commit().await?;
    assert_eq!(
        fixture
            .ctx
            .post(&path(&tree), json!({}), Some(&broad_token))
            .await?
            .status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(before, states(&fixture, &tree).await?);
    assert_eq!(
        fixture
            .ctx
            .post(&path(&tree), json!({}), Some(&other_token))
            .await?
            .status(),
        StatusCode::OK
    );
    fixture.ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn late_workflow_state_creation_waits_for_cancellation_and_cannot_resurrect_a_cancelled_parent(
) -> Result<()> {
    // Requires the documented B creation-side parent SHARE fence. The plain
    // retained execution ID is not an FK and cannot provide that fence itself.
    let (mut fixture, original) = fixture_and_tree(2, false).await?;
    cancel_source(&mut fixture, None).await?;
    let mut tx = fixture.ctx.pool.begin().await?;
    let snapshot =
        ExecutableSnapshotRepository::resolve_for_action_ref(&mut *tx, ROOT_ACTION).await?;
    let policy = RuntimeAuthorityRepository::find_policy(&mut tx, fixture.policy_id)
        .await?
        .unwrap();
    let selected = policy.policy.select(
        policy.id,
        &InvocationBinding::Action {
            action_ref: ROOT_ACTION.into(),
        },
        &[],
        RequestAttribution::Human {
            actor: HumanAttribution {
                identity_id: fixture.requester,
                login: fixture.ctx.user.as_ref().unwrap().login.clone(),
            },
        },
    )?;
    let target = ExecutionRepository::create_pinned(
        &mut *tx,
        CreateExecutionInput {
            action: Some(snapshot.executable.action.id),
            action_ref: snapshot.executable.action.r#ref.clone(),
            status: ExecutionStatus::Scheduling,
            permission_set_refs: vec![],
            config: Some(json!({})),
            ..Default::default()
        },
        &snapshot,
    )
    .await?;
    let schema = SchemaContractRepository::materialize_action(&mut tx, &snapshot).await?;
    SchemaContractRepository::attach(&mut tx, BindingOwner::Execution, target.id, &schema).await?;
    RuntimeAuthorityRepository::record_admission(
        &mut tx,
        RuntimeOwner::Execution(target.id),
        &selected,
    )
    .await?;
    tx.commit().await?;
    let target_tree = Tree {
        ids: vec![target.id],
        workflow: 0,
        inquiry: None,
    };
    let mut blocker = Some(fixture.ctx.pool.begin().await?);
    let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut **blocker.as_mut().unwrap())
        .await?;
    sqlx::query("LOCK TABLE workflow_task_wait IN SHARE MODE")
        .execute(&mut **blocker.as_mut().unwrap())
        .await?;
    let mut effect = Some(request(&fixture, &target_tree));
    let mut writer = None;
    let outcome: Result<()> = async {
        let effect_pid = blocked(&fixture.ctx.pool, pid, "UPDATE workflow_task_wait").await?;
        let pool = fixture.ctx.pool.clone();
        let definition = snapshot.executable.workflow_definition.as_ref().unwrap().id;
        writer = Some(tokio::spawn(async move {
            let result = WorkflowExecutionRepository::create_or_get_by_execution(
                &pool,
                CreateWorkflowExecutionInput {
                    execution: target.id,
                    workflow_def: definition,
                    task_graph: json!({}),
                    variables: json!({}),
                    status: ExecutionStatus::Running,
                },
            )
            .await;
            assert!(
                result.is_err(),
                "workflow-state creator resurrected a cancelled execution"
            );
        }));
        blocked(&fixture.ctx.pool, effect_pid, "workflow_execution").await?;
        blocker.take().unwrap().commit().await?;
        let response = finish(effect.take().unwrap()).await?;
        finish(writer.take().unwrap()).await?;
        if response.status() != StatusCode::OK {
            return Err(
                "cancellation failed before the late creator could observe terminal state".into(),
            );
        }
        let state: String = sqlx::query_scalar("SELECT status::text FROM execution WHERE id = $1")
            .bind(target_tree.ids[0])
            .fetch_one(&fixture.ctx.pool)
            .await?;
        let exists: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM workflow_execution WHERE execution = $1)",
        )
        .bind(target_tree.ids[0])
        .fetch_one(&fixture.ctx.pool)
        .await?;
        if state != "cancelled" || exists {
            return Err("late creator left live workflow state after cancellation".into());
        }
        if states(&fixture, &original).await?.1 != "running" {
            return Err("cancellation changed an unrelated workflow".into());
        }
        Ok(())
    }
    .await;
    let cleanup = cleanup(blocker, effect, writer).await;
    let database_cleanup = fixture.ctx.cleanup().await;
    cleanup?;
    database_cleanup?;
    outcome
}

#[tokio::test]
async fn fixed_approval_expiry_after_last_prerequisite_row_wait_rolls_back_all_cancellation_effects(
) -> Result<()> {
    use attune_common::repositories::identity::{
        CreatePermissionAssignmentInput, CreatePermissionSetInput, PermissionAssignmentRepository,
        PermissionSetRepository,
    };
    let (mut fixture, tree) = fixture_and_tree(3, false).await?;
    let grant: attune_common::rbac::Grant =
        serde_json::from_value(json!({"resource":"executions","actions":["cancel"]}))?;
    let issuer = PermissionSetRepository::create(
        &fixture.ctx.pool,
        CreatePermissionSetInput {
            r#ref: "fixture.root_cancel_issuer".into(),
            pack: None,
            pack_ref: None,
            label: None,
            description: None,
            grants: serde_json::to_value(vec![grant.clone()])?,
        },
    )
    .await?;
    PermissionAssignmentRepository::create(
        &fixture.ctx.pool,
        CreatePermissionAssignmentInput {
            identity: fixture.requester,
            permset: issuer.id,
        },
    )
    .await?;
    let mut tx = fixture.ctx.pool.begin().await?;
    let approval = RuntimeAuthorityRepository::issue_approval(
        &mut tx,
        fixture.pack_id,
        fixture.requester,
        vec![grant.clone()],
        Some(chrono::Utc::now() + chrono::Duration::seconds(20)),
    )
    .await?;
    let deadline = approval.expires_at.unwrap();
    let mut policy = RuntimeAuthorityRepository::find_policy(&mut tx, fixture.policy_id)
        .await?
        .unwrap()
        .policy
        .0;
    policy.invocations[0]
        .permission_sets
        .get_mut("recorded_emitter.events")
        .unwrap()
        .grants
        .push(attune_common::runtime_authority::AdmittedGrant::External {
            approval_id: approval.id,
            grant,
        });
    let policy = RuntimeAuthorityRepository::capture_policy(&mut tx, &policy).await?;
    tx.commit().await?;
    fixture.policy_id = policy.id;
    let (credential, token) = independent_cancel_source(&fixture).await?;
    fixture.credential = credential;
    fixture.token = token;
    let before = states(&fixture, &tree).await?;
    let mut blocker = Some(fixture.ctx.pool.begin().await?);
    let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut **blocker.as_mut().unwrap())
        .await?;
    sqlx::query("SELECT id FROM inquiry WHERE id = $1 FOR UPDATE")
        .bind(tree.inquiry)
        .fetch_one(&mut **blocker.as_mut().unwrap())
        .await?;
    let mut effect = Some(request(&fixture, &tree));
    let outcome: Result<()> = async {
        blocked(&fixture.ctx.pool, pid, "UPDATE inquiry").await?;
        tokio::time::timeout(std::time::Duration::from_secs(30), async {
            loop {
                let expired: bool = sqlx::query_scalar("SELECT clock_timestamp() >= $1")
                    .bind(deadline)
                    .fetch_one(&fixture.ctx.pool)
                    .await?;
                if expired {
                    break Ok::<_, sqlx::Error>(());
                }
                tokio::task::yield_now().await;
            }
        })
        .await??;
        blocker.take().unwrap().commit().await?;
        let response = finish(effect.take().unwrap()).await?;
        if response.status() != StatusCode::FORBIDDEN {
            return Err("expired cancellation escaped its last row wait".into());
        }
        if before != states(&fixture, &tree).await? {
            return Err("expired cancellation partially changed a workflow or descendant".into());
        }
        Ok(())
    }
    .await;
    let cleanup = cleanup(blocker, effect, None).await;
    let database_cleanup = fixture.ctx.cleanup().await;
    cleanup?;
    database_cleanup?;
    outcome
}
