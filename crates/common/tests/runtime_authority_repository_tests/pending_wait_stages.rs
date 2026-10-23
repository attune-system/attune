//! A genuinely issued persisted wait can privately parent source-phase stages.
//! Publication still needs its real release and original VALUE program order.
use super::*;
use attune_common::repositories::{
    action::{ActionRepository, UpdateActionInput},
    declared_input::DeclaredInputRepository,
    execution_runtime_admission::ExecutionAdmissionRepository as Admissions,
    execution_secret_value::ExecutionSecretValueRepository as Secrets,
    inherited_input::{
        InheritedInputRepository as Inputs, InputEntityKind, InputSite, InputSitePurpose,
    },
    native_input_snapshot::NativeInputSnapshotRepository as Snapshots,
    recorded_input_authority::RecordedInputAuthorityRepository as Contributors,
    runtime_target::{ComponentTarget, RuntimeTarget},
    schema_contract::{BindingOwner, SchemaContractRepository as Schemas},
    workflow::{
        CreateWorkflowDefinitionInput, CreateWorkflowExecutionInput, WorkflowDefinitionRepository,
        WorkflowExecutionRepository,
    },
    workflow_task_wait::{CreateWorkflowTaskWaitInput, WorkflowTaskWaitRepository as Waits},
};

#[tokio::test]
async fn issued_wait_value_condition_and_dependent_value_stage_before_release_then_seal() {
    pending_wait_case(false).await;
}
#[tokio::test]
async fn private_wait_stages_retain_owning_read_approval_across_actual_target_wait() {
    pending_wait_case(true).await;
}

async fn pending_wait_case(expires: bool) {
    let (db, fixture) = fixture().await;
    let owner = &fixture.policy.policy.owner;
    let key = NATIVE_WAIT_TEST_KEY;
    let literal = "{{ keystore['never.evaluate.this'] }}";
    let action = ActionFixture::new(owner.pack_id, &owner.pack_ref, "pending_wait_graph")
        .create(&db)
        .await
        .unwrap();
    let leaf = ActionFixture::new(owner.pack_id, &owner.pack_ref, "pending_wait_leaf")
        .create(&db)
        .await
        .unwrap();
    ActionRepository::update(
        &db,
        leaf.id,
        UpdateActionInput {
            param_schema: Some(json!({"token":{"type":"string"}})),
            out_schema: Some(json!({"token":{"type":"string","secret":true}})),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let set_ref = format!("{}.pending_wait_reads", owner.pack_ref);
    let definition = json!({"tasks":[
        {"name":"observe","action":leaf.r#ref,"wait_for":{"execution":"{{ workflow.target_execution }}"},"permission_set_refs":[set_ref],"next":[
            {"publish":[{"a":"{{ execution.observe.result.token }}"}]},
            {"when":"{{ workflow.a != '' }}","publish":[{"b":"{{ workflow.a }}"}],"do":["consumer"]}
        ]},
        {"name":"consumer","action":leaf.r#ref,"params":{"token":"{{ workflow.b }}"},"next":[{"when":"{{ succeeded() }}"}]},
        {"name":"source","action":leaf.r#ref}
    ]});
    let graph = WorkflowDefinitionRepository::create(
        &db,
        CreateWorkflowDefinitionInput {
            r#ref: action.r#ref.clone(),
            pack: owner.pack_id,
            pack_ref: owner.pack_ref.clone(),
            label: "Pending wait stages".into(),
            description: None,
            version: "1.0.0".into(),
            param_schema: None,
            out_schema: None,
            tags: vec![],
            definition: definition.clone(),
        },
    )
    .await
    .unwrap();
    ActionRepository::link_workflow_def(&db, action.id, graph.id)
        .await
        .unwrap();
    let mut tx = db.begin().await.unwrap();
    let release = activate_release(
        &mut tx,
        owner.pack_id,
        &owner.pack_ref,
        vec![action.id, leaf.id],
        'e',
    )
    .await;
    let snapshots = ExecutableSnapshotRepository::resolve_release_actions(&mut tx, release.id)
        .await
        .unwrap();
    let root = ExecutionRepository::create_pinned(
        &mut *tx,
        CreateExecutionInput {
            action: Some(action.id),
            action_ref: action.r#ref.clone(),
            status: ExecutionStatus::Running,
            config: Some(json!({})),
            permission_set_refs: vec![],
            ..Default::default()
        },
        &snapshots[&action.r#ref],
    )
    .await
    .unwrap();
    let workflow = WorkflowExecutionRepository::create(
        &mut *tx,
        CreateWorkflowExecutionInput {
            execution: root.id,
            workflow_def: graph.id,
            task_graph: definition,
            variables: json!({}),
            status: ExecutionStatus::Running,
        },
    )
    .await
    .unwrap();
    let producer=ExecutionRepository::create_pinned(&mut *tx,CreateExecutionInput {action:Some(leaf.id),action_ref:leaf.r#ref.clone(),parent:Some(root.id),status:ExecutionStatus::Completed,config:Some(json!({})),result:Some(json!({"token":attune_common::secret_values::redaction_marker()})),workflow_task:Some(serde_json::from_value(json!({"workflow_execution":workflow.id,"task_name":"source","retry_count":0,"max_retries":0,"timed_out":false})).unwrap()),permission_set_refs:vec![],..Default::default()},&snapshots[&leaf.r#ref]).await.unwrap();
    sqlx::query("UPDATE workflow_execution SET variables=$2 WHERE id=$1")
        .bind(workflow.id)
        .bind(json!({"target_execution":producer.id}))
        .execute(&mut *tx)
        .await
        .unwrap();
    let grants = json!([{"resource":"executions","actions":["read","decrypt"],"constraints":{"ids":[producer.id]}}]);
    let role = PermissionSetRepository::create(
        &mut *tx,
        CreatePermissionSetInput {
            r#ref: "fixture.pending_wait_input".into(),
            pack: None,
            pack_ref: None,
            label: None,
            description: None,
            grants: grants.clone(),
        },
    )
    .await
    .unwrap();
    PermissionAssignmentRepository::create(
        &mut *tx,
        CreatePermissionAssignmentInput {
            identity: fixture.human_id,
            permset: role.id,
        },
    )
    .await
    .unwrap();
    let deadline = Utc::now()
        + if expires {
            Duration::seconds(30)
        } else {
            Duration::hours(1)
        };
    let approval = RuntimeAuthorityRepository::issue_approval(
        &mut tx,
        owner.pack_id,
        fixture.human_id,
        serde_json::from_value(grants).unwrap(),
        Some(deadline),
    )
    .await
    .unwrap();
    let set = attune_common::runtime_authority::AdmittedPermissionSet::external(
        &set_ref,
        owner,
        &approval.grants,
        &[approval.approval().unwrap()],
    )
    .unwrap();
    let sets = BTreeMap::from([(set_ref.clone(), set)]);
    let root_binding = InvocationBinding::Action {
        action_ref: action.r#ref.clone(),
    };
    let task_bindings =
        ["observe", "consumer", "source"].map(|name| InvocationBinding::WorkflowTask {
            workflow_ref: action.r#ref.clone(),
            task_path: format!("/tasks/{name}"),
            action_ref: leaf.r#ref.clone(),
        });
    let mut invocations = vec![InvocationPolicy::from_executable(
        root_binding.clone(),
        &snapshots[&action.r#ref],
        Some(&[]),
        &sets,
    )
    .unwrap()];
    for (index, binding) in task_bindings.iter().enumerate() {
        let refs = if index == 0 {
            vec![set_ref.clone()]
        } else {
            vec![]
        };
        invocations.push(
            InvocationPolicy::from_executable(
                binding.clone(),
                &snapshots[&leaf.r#ref],
                Some(&refs),
                &sets,
            )
            .unwrap(),
        );
    }
    let policy = RuntimeAuthorityRepository::capture_policy(
        &mut tx,
        &RuntimePolicy {
            owner: owner.clone(),
            release_id: release.id,
            release_digest: release.digest.clone(),
            concrete_targets: BTreeMap::new(),
            invocations,
        },
    )
    .await
    .unwrap();
    RuntimeAuthorityRepository::activate_policy(&mut tx, policy.id)
        .await
        .unwrap();
    let root_authority = policy
        .policy
        .select(
            policy.id,
            &root_binding,
            &[],
            fixture.admission.authority.attribution.clone(),
        )
        .unwrap();
    RuntimeAuthorityRepository::record_admission(
        &mut tx,
        RuntimeOwner::Execution(root.id),
        &root_authority,
    )
    .await
    .unwrap();
    Contributors::capture_human(&mut tx, "execution", root.id, fixture.human_id)
        .await
        .unwrap();
    let source_authority = policy
        .policy
        .select(
            policy.id,
            &task_bindings[2],
            &[],
            RequestAttribution::ParentExecution {
                execution_id: root.id,
            },
        )
        .unwrap();
    RuntimeAuthorityRepository::record_admission(
        &mut tx,
        RuntimeOwner::Execution(producer.id),
        &source_authority,
    )
    .await
    .unwrap();
    for (id, snapshot) in [
        (root.id, &snapshots[&action.r#ref]),
        (producer.id, &snapshots[&leaf.r#ref]),
    ] {
        let binding = Schemas::materialize_action_pair(&mut tx, snapshot)
            .await
            .unwrap();
        Schemas::attach(&mut tx, BindingOwner::Execution, id, &binding)
            .await
            .unwrap();
    }
    let fields = attune_common::secret_values::prepare_secret_values(
        vec![attune_common::secret_values::SecretValueInput {
            json_path: "/token".into(),
            value: json!(literal),
            source_kind: "parameter_schema".into(),
            source_ref: Some("/token".into()),
        }],
        key,
    )
    .unwrap();
    Secrets::upsert_many_with_conn(&mut tx, "execution_result", producer.id, &fields)
        .await
        .unwrap();
    let wait = Waits::create_or_get(
        &mut tx,
        CreateWorkflowTaskWaitInput {
            workflow_execution: workflow.id,
            task_name: "observe".into(),
            target: attune_common::models::WorkflowTaskWaitTarget::Execution(producer.id),
        },
    )
    .await
    .unwrap();
    // This is an already registered wait from an earlier activation. Prospective
    // initial registration is J's separate factory/binder, not a bootstrap here.
    tx.commit().await.unwrap();

    let mut tx = db.begin().await.unwrap();
    let task = Admissions::select(
        &mut tx,
        owner.pack_id,
        task_bindings[0].clone(),
        &snapshots[&leaf.r#ref],
        &[set_ref],
        RequestAttribution::ParentExecution {
            execution_id: root.id,
        },
        RuntimeTarget::Component(ComponentTarget::Action, action.id),
    )
    .await
    .unwrap();
    let declarations = DeclaredInputRepository::bindings(&mut tx, root.id)
        .await
        .unwrap();
    let selector = declarations
        .iter()
        .find(|binding| binding.source_path == "/workflow/tasks/0/wait_for/execution")
        .unwrap();
    let a_binding = declarations
        .iter()
        .find(|binding| binding.source_path == "/workflow/tasks/0/next/0/publish/0/a")
        .unwrap();
    let when = declarations
        .iter()
        .find(|binding| binding.source_path == "/workflow/tasks/0/next/1/when")
        .unwrap();
    let b_binding = declarations
        .iter()
        .find(|binding| binding.source_path == "/workflow/tasks/0/next/1/publish/0/b")
        .unwrap();
    let parameter = declarations
        .iter()
        .find(|binding| binding.source_path == "/workflow/tasks/1/params/token")
        .unwrap();
    let foreign = declarations
        .iter()
        .find(|binding| binding.source_path == "/workflow/tasks/1/next/0/when")
        .unwrap();
    let mut session = Inputs::begin_render(
        &mut tx,
        root.id,
        &selector.declaration_revision,
        &selector.source_path,
        &selector.expression,
        InputSitePurpose::Control,
    )
    .await
    .unwrap();
    Inputs::bind_public_field(
        &mut tx,
        &mut session,
        "/workflow/target_execution",
        &InputSite {
            execution: root.id,
            kind: InputEntityKind::WorkflowVariables,
            purpose: InputSitePurpose::Value,
            path: "/target_execution".into(),
        },
    )
    .await
    .unwrap();
    let rendered = Inputs::evaluate(&mut tx, session).await.unwrap();
    let control_site = InputSite {
        execution: root.id,
        kind: InputEntityKind::WorkflowVariables,
        purpose: InputSitePurpose::Control,
        path: selector.source_path.clone(),
    };
    let selector = Inputs::record_render(
        &mut tx,
        &rendered,
        &control_site,
        rendered.value(),
        rendered.selections(),
        key,
    )
    .await
    .unwrap();
    let prepared = Snapshots::prepare_wait(
        &mut tx,
        root.id,
        "/workflow/tasks/0",
        &task,
        wait.id,
        Some(&selector),
        key,
    )
    .await
    .unwrap();
    let mut session = Inputs::begin_render(
        &mut tx,
        root.id,
        &foreign.declaration_revision,
        &foreign.source_path,
        &foreign.expression,
        InputSitePurpose::Condition,
    )
    .await
    .unwrap();
    assert!(
        Inputs::bind_prepared_wait_outcome(&mut session, &prepared).is_err(),
        "foreign owning task must not borrow this native outcome"
    );
    let mut other = db.begin().await.unwrap();
    let mut session = Inputs::begin_render(
        &mut other,
        root.id,
        &a_binding.declaration_revision,
        &a_binding.source_path,
        &a_binding.expression,
        InputSitePurpose::Value,
    )
    .await
    .unwrap();
    assert!(Inputs::bind_prepared_wait_input(&mut session, &prepared).is_err());
    other.rollback().await.unwrap();
    let mut session = Inputs::begin_render(
        &mut tx,
        root.id,
        &a_binding.declaration_revision,
        &a_binding.source_path,
        &a_binding.expression,
        InputSitePurpose::Value,
    )
    .await
    .unwrap();
    Inputs::bind_prepared_wait_input(&mut session, &prepared).unwrap();
    let a_render = Inputs::evaluate(&mut tx, session).await.unwrap();
    let a_site = InputSite {
        execution: root.id,
        kind: InputEntityKind::WorkflowVariables,
        purpose: InputSitePurpose::Value,
        path: "/a".into(),
    };
    let a = Inputs::prepare_staged_value(&mut tx, &a_render, &a_site, key)
        .await
        .unwrap();
    assert_eq!(a.value(), &json!(literal));
    assert!(
        Inputs::record_render(
            &mut tx,
            &a_render,
            &a_site,
            a_render.value(),
            a_render.selections(),
            key
        )
        .await
        .is_err(),
        "ordinary public recording still needs the actual release"
    );
    let mut session = Inputs::begin_render(
        &mut tx,
        root.id,
        &when.declaration_revision,
        &when.source_path,
        &when.expression,
        InputSitePurpose::Condition,
    )
    .await
    .unwrap();
    Inputs::bind_staged_value(&mut session, "/workflow/a", &a).unwrap();
    let condition = Inputs::evaluate(&mut tx, session).await.unwrap();
    assert_eq!(condition.value(), &json!(true));
    let condition_site = InputSite {
        execution: root.id,
        kind: InputEntityKind::WorkflowVariables,
        purpose: InputSitePurpose::Condition,
        path: when.source_path.clone(),
    };
    let condition = Inputs::prepare_condition_decision(&mut tx, &condition, &condition_site, key)
        .await
        .unwrap();
    let mut session = Inputs::begin_render(
        &mut tx,
        root.id,
        &b_binding.declaration_revision,
        &b_binding.source_path,
        &b_binding.expression,
        InputSitePurpose::Value,
    )
    .await
    .unwrap();
    assert!(
        Inputs::bind_prepared(&mut session, "/workflow/a", &condition, &json!(true)).is_err(),
        "a Condition remains CONTROL, never a staged VALUE"
    );
    Inputs::bind_staged_value(&mut session, "/workflow/a", &a).unwrap();
    Inputs::attach_prepared_control(&mut session, &condition).unwrap();
    let b_render = Inputs::evaluate(&mut tx, session).await.unwrap();
    let b_site = InputSite {
        execution: root.id,
        kind: InputEntityKind::WorkflowVariables,
        purpose: InputSitePurpose::Value,
        path: "/b".into(),
    };
    let b = Inputs::prepare_staged_value(&mut tx, &b_render, &b_site, key)
        .await
        .unwrap();
    assert_eq!(b.value(), &json!(literal));
    let mut session = Inputs::begin_render(
        &mut tx,
        root.id,
        &parameter.declaration_revision,
        &parameter.source_path,
        &parameter.expression,
        InputSitePurpose::Value,
    )
    .await
    .unwrap();
    Inputs::bind_staged_value(&mut session, "/workflow/b", &b).unwrap();
    let input = Inputs::evaluate(&mut tx, session).await.unwrap();
    let child=ExecutionRepository::create_pinned(&mut *tx,CreateExecutionInput {action:Some(leaf.id),action_ref:leaf.r#ref.clone(),parent:Some(root.id),status:ExecutionStatus::Requested,config:Some(json!({"token":attune_common::secret_values::redaction_marker()})),workflow_task:Some(serde_json::from_value(json!({"workflow_execution":workflow.id,"task_name":"consumer","retry_count":0,"max_retries":0,"timed_out":false})).unwrap()),permission_set_refs:vec![],..Default::default()},&snapshots[&leaf.r#ref]).await.unwrap();
    let authority = policy
        .policy
        .select(
            policy.id,
            &task_bindings[1],
            &[],
            RequestAttribution::ParentExecution {
                execution_id: root.id,
            },
        )
        .unwrap();
    RuntimeAuthorityRepository::record_admission(
        &mut tx,
        RuntimeOwner::Execution(child.id),
        &authority,
    )
    .await
    .unwrap();
    Contributors::copy(&mut tx, "execution", root.id, "execution", child.id)
        .await
        .unwrap();
    let binding = Schemas::materialize_action_pair(&mut tx, &snapshots[&leaf.r#ref])
        .await
        .unwrap();
    Schemas::attach(&mut tx, BindingOwner::Execution, child.id, &binding)
        .await
        .unwrap();
    let site = InputSite {
        execution: child.id,
        kind: InputEntityKind::ExecutionConfig,
        purpose: InputSitePurpose::Value,
        path: "/token".into(),
    };
    let destination = Inputs::prepare_destination(&mut tx, &input, &site, key)
        .await
        .unwrap();
    assert!(
        Inputs::receipt_context(&mut tx, &a_site, &a.provenance().origins[0], key)
            .await
            .is_err()
    );
    assert!(
        Inputs::receipt_context(&mut tx, condition.site(), &condition.origin(), key)
            .await
            .is_err()
    );
    if expires {
        a.ensure_fresh_on(&mut tx).await.unwrap();
        condition.ensure_fresh_on(&mut tx).await.unwrap();
        let waiter: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *tx)
            .await
            .unwrap();
        let mut blocker = db.begin().await.unwrap();
        sqlx::query("SELECT id FROM workflow_execution WHERE id=$1 FOR UPDATE")
            .bind(workflow.id)
            .execute(&mut *blocker)
            .await
            .unwrap();
        let release = tokio::spawn(async move {
            loop {
                let waiting: bool =
                    sqlx::query_scalar("SELECT cardinality(pg_blocking_pids($1))>0")
                        .bind(waiter)
                        .fetch_one(&mut *blocker)
                        .await
                        .unwrap();
                if waiting {
                    break;
                }
                assert!(
                    Utc::now() < deadline + Duration::seconds(10),
                    "actual workflow waiter was not observed"
                );
                tokio::task::yield_now().await;
            }
            sqlx::query("SELECT pg_sleep(GREATEST(0,EXTRACT(EPOCH FROM ($1::TIMESTAMPTZ-clock_timestamp()))))").bind(deadline).execute(&mut *blocker).await.unwrap();
            blocker.commit().await.unwrap();
        });
        sqlx::query("SELECT id FROM workflow_execution WHERE id=$1 FOR UPDATE")
            .bind(workflow.id)
            .execute(&mut *tx)
            .await
            .unwrap();
        release.await.unwrap();
        assert!(a.ensure_fresh().is_err());
        assert!(condition.ensure_fresh().is_err());
        assert!(b.ensure_fresh().is_err());
        assert!(Inputs::finalize_staged_value(&mut tx, &a, key)
            .await
            .is_err());
        assert!(Inputs::record_prepared(
            &mut tx,
            &input,
            &destination,
            input.value(),
            input.selections(),
            key
        )
        .await
        .is_err());
        tx.rollback().await.unwrap();
        db.cleanup().await.unwrap();
        return;
    }
    sqlx::query("SELECT id FROM workflow_execution WHERE id=$1 FOR UPDATE")
        .bind(workflow.id)
        .execute(&mut *tx)
        .await
        .unwrap();
    let mut variables = json!({"target_execution":producer.id,"a":attune_common::secret_values::redaction_marker(),"b":attune_common::secret_values::redaction_marker()});
    sqlx::query("UPDATE workflow_execution SET variables=$2 WHERE id=$1")
        .bind(workflow.id)
        .bind(&variables)
        .execute(&mut *tx)
        .await
        .unwrap();
    let fields = attune_common::secret_values::prepare_secret_values(
        vec![
            attune_common::secret_values::SecretValueInput {
                json_path: "/a".into(),
                value: a.value().clone(),
                source_kind: "provenance".into(),
                source_ref: Some(serde_json::to_string(&a.provenance()).unwrap()),
            },
            attune_common::secret_values::SecretValueInput {
                json_path: "/b".into(),
                value: b.value().clone(),
                source_kind: "provenance".into(),
                source_ref: Some(serde_json::to_string(&b.provenance()).unwrap()),
            },
        ],
        key,
    )
    .unwrap();
    Secrets::upsert_many_with_conn(&mut tx, "workflow_variables", root.id, &fields)
        .await
        .unwrap();
    assert!(
        Inputs::finalize_staged_value(&mut tx, &a, key)
            .await
            .is_err(),
        "correct buffers without real normalized release cannot publish"
    );
    assert!(Inputs::finalize_staged_value(&mut tx, &b, key)
        .await
        .is_err());
    let released = Snapshots::release_wait(&mut tx, prepared).await.unwrap();
    assert!(
        Inputs::finalize_staged_value(&mut tx, &b, key)
            .await
            .is_err(),
        "release does not waive a -> when -> b order"
    );
    Inputs::finalize_staged_value(&mut tx, &a, key)
        .await
        .unwrap();
    variables["b"] = json!("changed target buffer");
    sqlx::query("UPDATE workflow_execution SET variables=$2 WHERE id=$1")
        .bind(workflow.id)
        .bind(&variables)
        .execute(&mut *tx)
        .await
        .unwrap();
    assert!(Inputs::finalize_staged_value(&mut tx, &b, key)
        .await
        .is_err());
    variables["b"] = attune_common::secret_values::redaction_marker();
    sqlx::query("UPDATE workflow_execution SET variables=$2 WHERE id=$1")
        .bind(workflow.id)
        .bind(&variables)
        .execute(&mut *tx)
        .await
        .unwrap();
    let changed = attune_common::secret_values::prepare_secret_values(
        vec![attune_common::secret_values::SecretValueInput {
            json_path: "/b".into(),
            value: json!("changed ciphertext value"),
            source_kind: "provenance".into(),
            source_ref: Some(serde_json::to_string(&b.provenance()).unwrap()),
        }],
        key,
    )
    .unwrap();
    Secrets::upsert_many_with_conn(&mut tx, "workflow_variables", root.id, &changed)
        .await
        .unwrap();
    assert!(Inputs::finalize_staged_value(&mut tx, &b, key)
        .await
        .is_err());
    Secrets::upsert_many_with_conn(&mut tx, "workflow_variables", root.id, &fields)
        .await
        .unwrap();
    Inputs::finalize_staged_value(&mut tx, &b, key)
        .await
        .unwrap();
    let receipt = Inputs::record_prepared(
        &mut tx,
        &input,
        &destination,
        input.value(),
        input.selections(),
        key,
    )
    .await
    .unwrap();
    let fields = attune_common::secret_values::prepare_secret_values(
        vec![attune_common::secret_values::SecretValueInput {
            json_path: "/token".into(),
            value: input.value().clone(),
            source_kind: "provenance".into(),
            source_ref: Some(serde_json::to_string(&receipt.provenance()).unwrap()),
        }],
        key,
    )
    .unwrap();
    Secrets::upsert_many_with_conn(&mut tx, "execution_config", child.id, &fields)
        .await
        .unwrap();
    attune_common::repositories::accepted_input::AcceptedInputRepository::seal_execution(
        &mut tx, child.id, key,
    )
    .await
    .unwrap();
    Inputs::receipt_context(&mut tx, &b_site, &b.provenance().origins[0], key)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    a.ensure_fresh().unwrap();
    b.ensure_fresh().unwrap();
    condition.ensure_fresh().unwrap();
    released.ensure_fresh().unwrap();
    let mut other = db.begin().await.unwrap();
    assert!(a.ensure_fresh_on(&mut other).await.is_err());
    Inputs::receipt_context(&mut other, &b_site, &b.provenance().origins[0], key)
        .await
        .unwrap();
    other.rollback().await.unwrap();
    db.cleanup().await.unwrap();
}
