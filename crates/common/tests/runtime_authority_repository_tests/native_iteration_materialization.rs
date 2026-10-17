//! Actual ordinary prepare/apply callers. No cache frontier or invented items.
use super::*;
use attune_common::{
    models::{Execution, WorkflowExecution},
    repositories::{
        cache::{CacheEntryRepository, CacheTransactionMode},
        declared_input::DeclaredInputRepository,
        declared_input_materialization::DeclaredInputMaterializationRepository as Materialize,
        execution_secret_value::ExecutionSecretValueRepository as Secrets,
        inherited_input::InheritedInputRepository as Inputs,
        managed_executable::ManagedExecutableRepository,
        native_input_origin::NativeInputOriginRepository as Native,
        native_iteration_materialization::NativeIterationMaterializationRepository as Iteration,
        native_workflow_materialization::NativeWorkflowMaterializationRepository,
        schema_contract::{BindingOwner, SchemaContractRepository as Schemas},
        workflow::{
            CreateWorkflowDefinitionInput, CreateWorkflowExecutionInput,
            UpdateWorkflowExecutionInput, WorkflowDefinitionRepository,
            WorkflowExecutionRepository as Workflows,
        },
        FindById,
    },
    secret_values::{ENTITY_EXECUTION_CONFIG, ENTITY_WORKFLOW_VARIABLES},
};
use serde_json::Value;

const KEY: &str = "0123456789abcdef0123456789abcdef";
const LITERAL: &str = "{{ keystore['system.must_not_be_reparsed'] }}";

// Use the owning production parser/builder for I's persisted graph contract,
// rather than fabricating a nodes/retry table in a repository fixture.
#[path = "../../../executor/src/workflow/graph.rs"]
#[allow(dead_code)] // Graph inspection helpers are not all needed by this fixture.
mod executor_task_graph;

async fn setup(
    items: Value,
    batch: Option<usize>,
    expiring: bool,
) -> (TestDatabase, Execution, WorkflowExecution, Id) {
    let (db, root, workflow, human, _) = setup_graph(items, batch, expiring, 0, false).await;
    (db, root, workflow, human)
}

async fn setup_graph(
    items: Value,
    batch: Option<usize>,
    expiring: bool,
    successors: usize,
    pending: bool,
) -> (
    TestDatabase,
    Execution,
    WorkflowExecution,
    Id,
    Option<Execution>,
) {
    setup_graph_kind(items, batch, expiring, successors, pending, None).await
}

async fn setup_graph_kind(
    items: Value,
    batch: Option<usize>,
    expiring: bool,
    successors: usize,
    pending: bool,
    group: Option<(bool, bool)>,
) -> (
    TestDatabase,
    Execution,
    WorkflowExecution,
    Id,
    Option<Execution>,
) {
    let (db, fixture) = fixture().await;
    let item_type = if items.is_array() {
        "array"
    } else if items.is_object() {
        "object"
    } else {
        "string"
    };
    let owner = &fixture.policy.policy.owner;
    let graph_action = ActionFixture::new(owner.pack_id, &owner.pack_ref, "ordinary_graph")
        .create(&db)
        .await
        .unwrap();
    let leaf = ActionFixture::new(owner.pack_id, &owner.pack_ref, "ordinary_leaf")
        .create(&db)
        .await
        .unwrap();
    let producer = if successors > 0 {
        Some(
            ActionFixture::new(owner.pack_id, &owner.pack_ref, "ordinary_producer")
                .create(&db)
                .await
                .unwrap(),
        )
    } else {
        None
    };
    attune_common::repositories::action::ActionRepository::update(
        &db,
        graph_action.id,
        attune_common::repositories::action::UpdateActionInput {
            param_schema: Some(json!({"items":{"type":item_type},"literal":{"type":"string"}})),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    attune_common::repositories::action::ActionRepository::update(&db, leaf.id,
        attune_common::repositories::action::UpdateActionInput {param_schema: Some(json!({"payload":{"type":"object","required":true,"secret":expiring},"ordinal":{"type":"integer","required":true}})), ..Default::default()}).await.unwrap();
    let mut definition = json!({"vars":{"saved":"{{ parameters.literal }}"},"tasks":[{
        "name":"ordinary", "action":leaf.r#ref, "with_items":"{{ parameters.items }}", "concurrency":2,
        "permission_set_refs":[], "params":{"payload":{"selected":"{{ item }}", "saved":"{{ workflow.saved }}"},"ordinal":"{{ index }}"}
    }]});
    if let Some(batch) = batch {
        definition["tasks"][0]["batch_size"] = json!(batch);
    }
    if expiring {
        definition["tasks"][0]["with_items"] = json!("{{ [keystore['system.iteration_gate']] }}");
    }
    if let Some((expect_failed, retry)) = group {
        definition["version"] = json!("1.0.0");
        let input = definition["tasks"][0]
            .as_object_mut()
            .unwrap()
            .remove("params")
            .unwrap();
        definition["tasks"][0]["input"] = input;
        let source = definition["tasks"][0].clone();
        for name in ["ordinary_success", "ordinary_failure"] {
            let mut target = source.clone();
            target["name"] = json!(name);
            target["with_items"] = json!("{{ workflow.carried }}");
            definition["tasks"].as_array_mut().unwrap().push(target);
        }
        let selected = json!([{"chosen":"{{ parameters.items }}"},{"carried":"{{ workflow.chosen }}"},{"chosen":"overwritten"}]);
        let unselected = json!([{"forbidden":"{{ keystore['system.unselected_group_branch'] }}"}]);
        helpers::KeyFixture::new_system("unselected_group_branch", "never-authorized")
            .create(&db)
            .await
            .unwrap();
        definition["tasks"][0]["next"] = json!([
            {"when":"{{ succeeded() }}","do":["ordinary_success"],"publish":if expect_failed {unselected.clone()} else {selected.clone()}},
            {"when":"{{ failed() }}","do":["ordinary_failure"],"publish":if expect_failed {selected} else {unselected}}
        ]);
        if retry {
            definition["tasks"][0]["retry"] = json!({"count":1,"delay":0});
        }
    }
    if let Some(producer) = &producer {
        if pending {
            definition["tasks"][0]["with_items"] = json!("{{ workflow.selected }}");
        }
        let mut names = vec!["ordinary".to_owned()];
        for ordinal in 1..successors {
            let mut node = definition["tasks"][0].clone();
            let name = format!("ordinary_{ordinal}");
            node["name"] = json!(name);
            definition["tasks"].as_array_mut().unwrap().push(node);
            names.push(name);
        }
        let mut transition = json!({"when":"{{ succeeded() }}","do":names});
        if pending {
            transition["publish"] = json!([{"selected":"{{ parameters.items }}"}]);
        }
        definition["tasks"].as_array_mut().unwrap().insert(
            0,
            json!({"name":"choose","action":producer.r#ref,"params":{},"next":[transition]}),
        );
    }
    let graph = WorkflowDefinitionRepository::create(
        &db,
        CreateWorkflowDefinitionInput {
            r#ref: graph_action.r#ref.clone(),
            pack: owner.pack_id,
            pack_ref: owner.pack_ref.clone(),
            label: "Ordinary iteration".into(),
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
    attune_common::repositories::action::ActionRepository::link_workflow_def(
        &db,
        graph_action.id,
        graph.id,
    )
    .await
    .unwrap();
    let mut tx = db.begin().await.unwrap();
    let mut actions = vec![graph_action.id, leaf.id];
    if let Some(producer) = &producer {
        actions.push(producer.id);
    }
    let release = activate_release(&mut tx, owner.pack_id, &owner.pack_ref, actions, 'd').await;
    let snapshots = ExecutableSnapshotRepository::resolve_release_actions(&mut tx, release.id)
        .await
        .unwrap();
    let root_binding = InvocationBinding::Action {
        action_ref: graph_action.r#ref.clone(),
    };
    let task_binding = InvocationBinding::WorkflowTask {
        workflow_ref: graph_action.r#ref.clone(),
        task_path: "/tasks/ordinary".into(),
        action_ref: leaf.r#ref.clone(),
    };
    let mut bindings = vec![root_binding.clone(), task_binding];
    if let Some(producer) = &producer {
        bindings.push(InvocationBinding::WorkflowTask {
            workflow_ref: graph_action.r#ref.clone(),
            task_path: "/tasks/choose".into(),
            action_ref: producer.r#ref.clone(),
        });
        for ordinal in 1..successors {
            bindings.push(InvocationBinding::WorkflowTask {
                workflow_ref: graph_action.r#ref.clone(),
                task_path: format!("/tasks/ordinary_{ordinal}"),
                action_ref: leaf.r#ref.clone(),
            });
        }
    }
    if group.is_some() {
        for name in ["ordinary_success", "ordinary_failure"] {
            bindings.push(InvocationBinding::WorkflowTask {
                workflow_ref: graph_action.r#ref.clone(),
                task_path: format!("/tasks/{name}"),
                action_ref: leaf.r#ref.clone(),
            });
        }
    }
    let policy = RuntimePolicy {
        owner: owner.clone(),
        release_id: release.id,
        release_digest: release.digest.clone(),
        concrete_targets: BTreeMap::new(),
        invocations: bindings
            .into_iter()
            .map(|binding| {
                InvocationPolicy::from_executable(
                    binding.clone(),
                    &snapshots[binding.action_ref()],
                    Some(&[]),
                    &BTreeMap::new(),
                )
                .unwrap()
            })
            .collect(),
    };
    let policy = RuntimeAuthorityRepository::capture_policy(&mut tx, &policy)
        .await
        .unwrap();
    RuntimeAuthorityRepository::activate_policy(&mut tx, policy.id)
        .await
        .unwrap();
    let snapshot =
        ManagedExecutableRepository::resolve_invocation(&mut tx, &policy.policy, &root_binding)
            .await
            .unwrap();
    let authority = policy
        .policy
        .select(
            policy.id,
            &root_binding,
            &[],
            fixture.admission.authority.attribution.clone(),
        )
        .unwrap();
    let mut root = ExecutionRepository::create_pinned(
        &mut *tx,
        CreateExecutionInput {
            action: Some(graph_action.id),
            action_ref: graph_action.r#ref.clone(),
            status: ExecutionStatus::Requested,
            executor: Some(fixture.human_id),
            ..Default::default()
        },
        &snapshot,
    )
    .await
    .unwrap();
    let admission = RuntimeAuthorityRepository::record_admission(
        &mut tx,
        RuntimeOwner::Execution(root.id),
        &authority,
    )
    .await
    .unwrap();
    let schema = Schemas::materialize_action_pair(&mut tx, &snapshot)
        .await
        .unwrap();
    Schemas::attach(&mut tx, BindingOwner::Execution, root.id, &schema)
        .await
        .unwrap();
    root = ExecutionRepository::initialize_config_with_conn(
        &mut tx,
        root.id,
        Some(admission.id),
        &snapshot,
        Some(json!({"items":items,"literal":LITERAL})),
        &[],
    )
    .await
    .unwrap();
    attune_common::repositories::recorded_input_authority::RecordedInputAuthorityRepository::capture_human(
        &mut tx, "execution", root.id, fixture.human_id).await.unwrap();
    Materialize::seal_execution_inputs(&mut tx, root.id, KEY)
        .await
        .unwrap();
    let variables =
        NativeWorkflowMaterializationRepository::prepare_initial_variables(&mut tx, root.id, KEY)
            .await
            .unwrap();
    Secrets::replace_entity_with_conn(
        &mut tx,
        ENTITY_WORKFLOW_VARIABLES,
        root.id,
        variables.secrets(),
    )
    .await
    .unwrap();
    let mut workflow = Workflows::create(
        &mut *tx,
        CreateWorkflowExecutionInput {
            execution: root.id,
            workflow_def: graph.id,
            task_graph: if group.is_some() {
                let parsed: attune_common::workflow::WorkflowDefinition =
                    serde_json::from_value(definition.clone()).unwrap();
                serde_json::to_value(
                    executor_task_graph::TaskGraph::from_workflow(&parsed).unwrap(),
                )
                .unwrap()
            } else {
                definition
            },
            variables: variables.variables().clone(),
            status: ExecutionStatus::Running,
        },
    )
    .await
    .unwrap();
    if group.is_some() {
        workflow = Workflows::update(
            &mut *tx,
            workflow.id,
            UpdateWorkflowExecutionInput {
                current_tasks: Some(vec!["ordinary".into()]),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    }
    variables.ensure_fresh_on(&mut tx).await.unwrap();
    root = ExecutionRepository::update(
        &mut *tx,
        root.id,
        UpdateExecutionInput {
            status: Some(ExecutionStatus::Running),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let completed = if let Some(producer) = &producer {
        use attune_common::{
            models::WorkflowTaskMetadata,
            repositories::execution_runtime_admission::ExecutionAdmissionRepository as Admissions,
        };
        let snapshot = &snapshots[&producer.r#ref];
        let schema = Admissions::workflow_schema(&mut tx, root.id, snapshot)
            .await
            .unwrap();
        let created = Admissions::create_workflow_task_if_absent_pinned_with_conn(
            &mut tx,
            CreateExecutionInput {
                action: Some(producer.id),
                action_ref: producer.r#ref.clone(),
                parent: Some(root.id),
                executor: root.executor,
                status: ExecutionStatus::Requested,
                workflow_task: Some(WorkflowTaskMetadata {
                    workflow_execution: workflow.id,
                    task_name: "choose".into(),
                    task_index: None,
                    task_batch: None,
                    triggered_by: None,
                    retry_count: 0,
                    max_retries: 0,
                    timeout_seconds: None,
                    next_retry_at: None,
                    timed_out: false,
                    duration_ms: None,
                    started_at: None,
                    completed_at: None,
                }),
                ..Default::default()
            },
            snapshot,
            workflow.id,
            "choose",
            None,
        )
        .await
        .unwrap();
        assert!(created.created);
        let child = created.execution;
        Schemas::attach(&mut tx, BindingOwner::Execution, child.id, &schema)
            .await
            .unwrap();
        let admission = Admissions::require(&mut tx, RuntimeOwner::Execution(child.id))
            .await
            .unwrap();
        ExecutionRepository::initialize_config_with_conn(
            &mut tx,
            child.id,
            Some(admission.id),
            snapshot,
            Some(json!({})),
            &[],
        )
        .await
        .unwrap();
        Materialize::seal_execution_inputs(&mut tx, child.id, KEY)
            .await
            .unwrap();
        Some(
            ExecutionRepository::update(
                &mut *tx,
                child.id,
                UpdateExecutionInput {
                    status: Some(ExecutionStatus::Completed),
                    ..Default::default()
                },
            )
            .await
            .unwrap(),
        )
    } else {
        None
    };
    tx.commit().await.unwrap();
    (db, root, workflow, fixture.human_id, completed)
}

#[tokio::test]
async fn native_ordinary_iteration_initial_empty_has_no_child_or_invented_index() {
    run(json!([]), None).await;
}
#[tokio::test]
async fn native_ordinary_iteration_single_item_restores_original_native_variables() {
    run(json!([{"nested":[LITERAL,{"also":LITERAL}]}]), None).await;
}
#[tokio::test]
async fn native_ordinary_iteration_multiple_items_create_all_seals_before_concurrency_window() {
    run(json!([{"v":LITERAL},{"v":2},{"v":3}]), None).await;
}

#[tokio::test]
async fn native_ordinary_iteration_non_array_wraps_once_without_reinterpreting_data() {
    run(json!({"nested":LITERAL}), None).await;
}
#[tokio::test]
async fn native_ordinary_iteration_nested_batch_uses_batch_ordinals_and_never_reparses_data() {
    run(json!([{"v":[LITERAL]},{"v":2},{"v":3}]), Some(2)).await;
}
#[tokio::test]
async fn native_ordinary_iteration_batch_one_preserves_existing_scalar_item_semantics() {
    run(json!([{"v":LITERAL},{"v":2}]), Some(1)).await;
}

async fn run(items: Value, batch: Option<usize>) {
    let (db, root, workflow, _) = setup(items.clone(), batch, false).await;
    let mut tx = db.begin().await.unwrap();
    CacheEntryRepository::protect_transaction(&mut tx, CacheTransactionMode::PinMutation)
        .await
        .unwrap();
    let mut plan = Iteration::prepare(&mut tx, &root, "ordinary", None, KEY)
        .await
        .unwrap();
    let count = items
        .as_array()
        .map_or(1, Vec::len)
        .div_ceil(batch.unwrap_or(1));
    assert_eq!(plan.total(), count);
    let outcome = plan.apply(&mut tx, KEY).await.unwrap();
    assert_eq!(outcome.total, count);
    assert_eq!(outcome.requested.len(), count.min(2));
    assert_eq!(outcome.remaining, count as i64);
    if count == 0 {
        let completed = outcome
            .completed_parent
            .as_ref()
            .expect("real autonomous empty completion");
        assert_eq!(completed.id, root.id);
        assert_eq!(completed.status, ExecutionStatus::Completed);
        assert!(
            completed.result.is_none(),
            "status-only empty completion must not invent a native VALUE"
        );
        assert_eq!(
            Workflows::find_by_id(&mut *tx, workflow.id)
                .await
                .unwrap()
                .unwrap()
                .status,
            ExecutionStatus::Completed
        );
    } else {
        assert!(outcome.completed_parent.is_none());
    }
    let children = ExecutionRepository::find_by_parent(&mut *tx, root.id)
        .await
        .unwrap();
    assert_eq!(children.len(), count);
    for (ordinal, child) in children.iter().enumerate() {
        assert_eq!(
            child.workflow_task.as_ref().unwrap().task_index,
            Some(ordinal as i32)
        );
        assert_eq!(
            child.workflow_task.as_ref().unwrap().task_batch,
            batch.filter(|size| *size > 1).map(|_| ordinal as i32)
        );
        assert_eq!(child.parent, Some(root.id));
        assert!(child.permission_set_refs.is_empty());
        let restored = Native::capture_with_key(&mut tx, child.id, &[], Some(KEY))
            .await
            .unwrap()
            .restore_prepared(
                &mut tx,
                ENTITY_EXECUTION_CONFIG,
                child.id,
                child.config.clone().unwrap(),
                Some(KEY),
            )
            .await
            .unwrap();
        let selected = match batch.filter(|size| *size > 1) {
            None => items
                .as_array()
                .map_or_else(|| items.clone(), |items| items[ordinal].clone()),
            Some(size) => json!(
                items.as_array().unwrap()
                    [ordinal * size..((ordinal + 1) * size).min(items.as_array().unwrap().len())]
            ),
        };
        assert_eq!(
            restored,
            json!({"payload":{"selected":selected,"saved":LITERAL},"ordinal":ordinal})
        );
        let schema = Schemas::find(&mut tx, BindingOwner::Execution, child.id)
            .await
            .unwrap()
            .unwrap();
        attune_common::value_schema::CompiledContract::new(&schema.input)
            .unwrap()
            .validate(&restored)
            .unwrap();
        let accepted: (bool, i64) = sqlx::query_as("SELECT e.transaction_id=pg_current_xact_id()::TEXT,jsonb_array_length(a.body->'contributors')::BIGINT FROM runtime_accepted_input a JOIN runtime_accepted_input_epoch e ON e.admission=a.admission WHERE a.execution=$1")
            .bind(child.id).fetch_one(&mut *tx).await.unwrap();
        assert_eq!(accepted, (true, 1), "each original child seals its actual inherited contributor in the creation transaction");
    }
    assert_eq!(
        Workflows::find_by_id(&mut *tx, workflow.id)
            .await
            .unwrap()
            .unwrap()
            .variables,
        workflow.variables
    );
    plan.ensure_fresh_on(&mut tx).await.unwrap();
    assert!(plan.apply(&mut tx, KEY).await.is_err());
    tx.commit().await.unwrap();
    plan.ensure_fresh().unwrap();
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn native_ordinary_iteration_stale_variables_reject_whole_reserved_sibling_set() {
    let (db, root, workflow, _) = setup(json!([1, 2]), None, false).await;
    let mut tx = db.begin().await.unwrap();
    CacheEntryRepository::protect_transaction(&mut tx, CacheTransactionMode::PinMutation)
        .await
        .unwrap();
    let mut plan = Iteration::prepare(&mut tx, &root, "ordinary", None, KEY)
        .await
        .unwrap();
    Workflows::update(
        &mut *tx,
        workflow.id,
        UpdateWorkflowExecutionInput {
            variables: Some(json!({"saved":"changed"})),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert!(plan.apply(&mut tx, KEY).await.is_err());
    tx.rollback().await.unwrap();
    assert!(ExecutionRepository::find_by_parent(db.pool(), root.id)
        .await
        .unwrap()
        .is_empty());
    assert_eq!(
        Workflows::find_by_id(db.pool(), workflow.id)
            .await
            .unwrap()
            .unwrap()
            .variables,
        workflow.variables
    );
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn native_ordinary_iteration_changed_reserved_ordinal_rolls_back_all_children() {
    let (db, root, _, _) = setup(json!([1, 2]), None, false).await;
    let mut tx = db.begin().await.unwrap();
    CacheEntryRepository::protect_transaction(&mut tx, CacheTransactionMode::PinMutation)
        .await
        .unwrap();
    let mut plan = Iteration::prepare(&mut tx, &root, "ordinary", None, KEY)
        .await
        .unwrap();
    let row = ExecutionRepository::find_by_parent(&mut *tx, root.id)
        .await
        .unwrap()
        .remove(0);
    let mut metadata = row.workflow_task.unwrap();
    metadata.task_index = Some(200);
    ExecutionRepository::update(
        &mut *tx,
        row.id,
        UpdateExecutionInput {
            workflow_task: Some(metadata),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert!(plan.apply(&mut tx, KEY).await.is_err());
    tx.rollback().await.unwrap();
    assert!(ExecutionRepository::find_by_parent(db.pool(), root.id)
        .await
        .unwrap()
        .is_empty());
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn native_ordinary_iteration_recovery_keeps_slots_and_failed_siblings_without_recreating_history(
) {
    let (db, root, _, _) = setup(json!([1, 2, 3]), None, false).await;
    let mut tx = db.begin().await.unwrap();
    CacheEntryRepository::protect_transaction(&mut tx, CacheTransactionMode::PinMutation)
        .await
        .unwrap();
    let mut plan = Iteration::prepare(&mut tx, &root, "ordinary", None, KEY)
        .await
        .unwrap();
    let initial = plan.apply(&mut tx, KEY).await.unwrap();
    assert_eq!(initial.requested.len(), 2);
    let first = initial.requested[0].id;
    let second = initial.requested[1].id;
    plan.ensure_fresh_on(&mut tx).await.unwrap();
    tx.commit().await.unwrap();
    ExecutionRepository::update(
        db.pool(),
        first,
        UpdateExecutionInput {
            status: Some(ExecutionStatus::Failed),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    ExecutionRepository::update(
        db.pool(),
        second,
        UpdateExecutionInput {
            status: Some(ExecutionStatus::Running),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let mut recovery = db.begin().await.unwrap();
    CacheEntryRepository::protect_transaction(&mut recovery, CacheTransactionMode::PinMutation)
        .await
        .unwrap();
    let mut plan = Iteration::prepare(&mut recovery, &root, "ordinary", None, KEY)
        .await
        .unwrap();
    let outcome = plan.apply(&mut recovery, KEY).await.unwrap();
    assert_eq!(outcome.requested.len(), 1);
    assert_eq!(
        outcome.requested[0]
            .workflow_task
            .as_ref()
            .unwrap()
            .task_index,
        Some(2)
    );
    assert_eq!(
        ExecutionRepository::find_by_parent(&mut *recovery, root.id)
            .await
            .unwrap()
            .len(),
        3
    );
    plan.ensure_fresh_on(&mut recovery).await.unwrap();
    recovery.commit().await.unwrap();
    plan.ensure_fresh().unwrap();
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn native_ordinary_iteration_control_expiry_during_actual_workflow_target_wait_rolls_back() {
    let (db, root, workflow, human) = setup(json!([]), None, true).await;
    helpers::KeyFixture::new_system("iteration_gate", LITERAL)
        .create(&db)
        .await
        .unwrap();
    let set = PermissionSetRepository::create(db.pool(), CreatePermissionSetInput {r#ref:"fixture.iteration_issuer".into(), pack:None,
        pack_ref:None,label:None,description:None,grants:json!([{"resource":"keys","actions":["read"],"constraints":{"refs":["system.iteration_gate"]}}])}).await.unwrap();
    PermissionAssignmentRepository::create(
        db.pool(),
        CreatePermissionAssignmentInput {
            identity: human,
            permset: set.id,
        },
    )
    .await
    .unwrap();
    let mut consent = db.begin().await.unwrap();
    let declaration = DeclaredInputRepository::bindings(&mut consent, root.id)
        .await
        .unwrap()
        .into_iter()
        .find(|d| d.source_path == "/workflow/tasks/0/with_items")
        .unwrap();
    let approval = Inputs::approve_external_input(&mut consent, root.id, &declaration.declaration_revision, &declaration.source_path,
        &declaration.expression, human, serde_json::from_value(json!([{"resource":"keys","actions":["read"],"constraints":{"refs":["system.iteration_gate"]}}])).unwrap(),
        Some(Utc::now()+Duration::seconds(10))).await.unwrap();
    consent.commit().await.unwrap();
    let mut blocker = db.begin().await.unwrap();
    Workflows::find_by_id_for_update(&mut *blocker, workflow.id)
        .await
        .unwrap()
        .unwrap();
    let mut tx = db.begin().await.unwrap();
    CacheEntryRepository::protect_transaction(&mut tx, CacheTransactionMode::PinMutation)
        .await
        .unwrap();
    let mut plan = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        Iteration::prepare(&mut tx, &root, "ordinary", None, KEY),
    )
    .await
    .expect("ordinary sources must prepare before the blocked workflow target")
    .unwrap();
    let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    let release = tokio::spawn(async move {
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(25);
        let mut waited = false;
        loop {
            let blocked: bool = sqlx::query_scalar("SELECT cardinality(pg_blocking_pids($1))>0")
                .bind(pid)
                .fetch_one(&mut *blocker)
                .await
                .unwrap();
            let expired: bool = sqlx::query_scalar(
                "SELECT expires_at <= clock_timestamp() FROM runtime_external_approval WHERE id=$1",
            )
            .bind(approval)
            .fetch_one(&mut *blocker)
            .await
            .unwrap();
            if blocked && !waited {
                assert!(!expired, "the target wait must be observed before expiry");
                waited = true;
            }
            if blocked && waited && expired {
                blocker.rollback().await.unwrap();
                return;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "target-wait/expiry barrier was not observed"
            );
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    });
    let result =
        tokio::time::timeout(std::time::Duration::from_secs(30), plan.apply(&mut tx, KEY)).await;
    release.await.unwrap();
    assert!(result.unwrap().is_err());
    tx.rollback().await.unwrap();
    assert!(ExecutionRepository::find_by_parent(db.pool(), root.id)
        .await
        .unwrap()
        .is_empty());
    db.cleanup().await.unwrap();
}

async fn settle_status_only(statuses: &[ExecutionStatus], expected: ExecutionStatus) {
    let items = Value::Array((0..statuses.len()).map(|index| json!(index)).collect());
    let (db, root, workflow, _) = setup(items, None, false).await;
    let mut tx = db.begin().await.unwrap();
    CacheEntryRepository::protect_transaction(&mut tx, CacheTransactionMode::PinMutation)
        .await
        .unwrap();
    let mut initial = Iteration::prepare(&mut tx, &root, "ordinary", None, KEY)
        .await
        .unwrap();
    initial.apply(&mut tx, KEY).await.unwrap();
    tx.commit().await.unwrap();
    let mut children = ExecutionRepository::find_by_parent(db.pool(), root.id)
        .await
        .unwrap();
    children.sort_by_key(|child| child.workflow_task.as_ref().unwrap().task_index);
    assert_eq!(children.len(), statuses.len());
    for (child, status) in children.iter().zip(statuses) {
        ExecutionRepository::update(
            db.pool(),
            child.id,
            UpdateExecutionInput {
                status: Some(*status),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    }
    let mut tx = db.begin().await.unwrap();
    CacheEntryRepository::protect_transaction(&mut tx, CacheTransactionMode::PinMutation)
        .await
        .unwrap();
    let mut finalizer = Iteration::prepare(&mut tx, &root, "ordinary", None, KEY)
        .await
        .unwrap();
    let whole = Iteration::prepare_completion(&mut tx, root.id, "ordinary", KEY)
        .await
        .unwrap()
        .expect("complete authenticated ordinal coverage");
    assert_eq!(whole.renderer(), root.id);
    assert_eq!(whole.workflow().id, workflow.id);
    assert_eq!(whole.task_name(), "ordinary");
    assert_eq!(whole.ordinal_count(), statuses.len());
    assert_eq!(whole.status(), expected);
    assert_eq!(whole.original_parent().id, root.id);
    assert_eq!(whole.original_siblings().len(), statuses.len());
    assert_eq!(whole.members().len(), statuses.len());
    for (index, member) in whole.members().enumerate() {
        assert_eq!(member.execution(), children[index].id);
        assert_eq!(member.original_execution(), children[index].id);
        assert_eq!(member.ordinal(), index as i32);
        assert_eq!(member.task_batch(), None);
        assert_eq!(member.retry_count(), 0);
        assert_eq!(member.metadata_retry_count(), 0);
        assert_eq!(member.created(), &children[index].created);
        assert_eq!(member.status(), statuses[index]);
        assert_eq!(member.outcome().execution(), member.execution());
        assert_eq!(member.outcome().task_name(), whole.task_name());
    }
    whole.require_source_current(&mut tx).await.unwrap();
    let outcome = finalizer.apply(&mut tx, KEY).await.unwrap();
    assert_eq!(outcome.remaining, 0);
    assert!(outcome.requested.is_empty());
    let parent = outcome.completed_parent.unwrap();
    assert_eq!(parent.id, root.id);
    assert_eq!(parent.status, expected);
    assert!(
        parent.result.is_none(),
        "status facts must not become a fabricated Value"
    );
    let actual = Workflows::find_by_id(&mut *tx, workflow.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(actual.status, expected);
    assert!(actual.current_tasks.is_empty());
    if expected == ExecutionStatus::Completed {
        assert!(actual.completed_tasks.iter().any(|name| name == "ordinary"));
    } else {
        assert!(actual.failed_tasks.iter().any(|name| name == "ordinary"));
    }
    finalizer.ensure_fresh_on(&mut tx).await.unwrap();
    whole.ensure_fresh_on(&mut tx).await.unwrap();
    tx.commit().await.unwrap();
    whole.ensure_fresh().unwrap();
    let mut delivery = db.begin().await.unwrap();
    CacheEntryRepository::protect_transaction(&mut delivery, CacheTransactionMode::PinMutation)
        .await
        .unwrap();
    let proofs = finalizer
        .prepare_control_delivery(&mut delivery, KEY)
        .await
        .unwrap();
    proofs.ensure_fresh_on(&mut delivery).await.unwrap();
    delivery.commit().await.unwrap();
    proofs.ensure_fresh().unwrap();
    assert_eq!(
        ExecutionRepository::find_by_parent(db.pool(), root.id)
            .await
            .unwrap()
            .len(),
        statuses.len()
    );
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn native_ordinary_iteration_last_single_item_settles_real_parent_without_value() {
    settle_status_only(&[ExecutionStatus::Completed], ExecutionStatus::Completed).await;
}

#[tokio::test]
async fn native_ordinary_iteration_last_multiple_items_settle_real_parent_without_value() {
    settle_status_only(
        &[ExecutionStatus::Completed, ExecutionStatus::Completed],
        ExecutionStatus::Completed,
    )
    .await;
}

#[tokio::test]
async fn native_ordinary_iteration_terminal_failed_sibling_settles_failure_without_value() {
    settle_status_only(
        &[ExecutionStatus::Completed, ExecutionStatus::Failed],
        ExecutionStatus::Failed,
    )
    .await;
}

#[tokio::test]
async fn native_ordinary_iteration_changed_latest_status_rejects_prepared_apply() {
    let (db, root, _, _) = setup(json!([1]), None, false).await;
    let mut tx = db.begin().await.unwrap();
    CacheEntryRepository::protect_transaction(&mut tx, CacheTransactionMode::PinMutation)
        .await
        .unwrap();
    let mut initial = Iteration::prepare(&mut tx, &root, "ordinary", None, KEY)
        .await
        .unwrap();
    let child = initial
        .apply(&mut tx, KEY)
        .await
        .unwrap()
        .requested
        .remove(0);
    tx.commit().await.unwrap();
    let mut tx = db.begin().await.unwrap();
    CacheEntryRepository::protect_transaction(&mut tx, CacheTransactionMode::PinMutation)
        .await
        .unwrap();
    let mut prepared = Iteration::prepare(&mut tx, &root, "ordinary", None, KEY)
        .await
        .unwrap();
    ExecutionRepository::update(
        &mut *tx,
        child.id,
        UpdateExecutionInput {
            status: Some(ExecutionStatus::Completed),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert!(prepared.apply(&mut tx, KEY).await.is_err());
    tx.rollback().await.unwrap();
    assert_eq!(
        ExecutionRepository::find_by_id(db.pool(), root.id)
            .await
            .unwrap()
            .unwrap()
            .status,
        ExecutionStatus::Running
    );
    db.cleanup().await.unwrap();
}

async fn run_successor_activation(successors: usize, pending: bool) {
    let (db, root, workflow, _, completed) = setup_graph(
        json!([{"opaque":LITERAL},{"v":2},{"v":3}]),
        None,
        false,
        successors,
        pending,
    )
    .await;
    let completed = completed.unwrap();
    assert!(
        completed.result.is_none(),
        "this predecessor provides an outcome, not a result VALUE"
    );
    let mut tx = db.begin().await.unwrap();
    CacheEntryRepository::protect_transaction(&mut tx, CacheTransactionMode::PinMutation)
        .await
        .unwrap();
    let mut activation = Iteration::prepare_successors(&mut tx, &root, &completed, KEY)
        .await
        .unwrap()
        .expect("genuine predecessor frame");
    assert_eq!(activation.workflow(), workflow.id);
    let before_publication = Workflows::find_by_id(&mut *tx, workflow.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        before_publication.variables, workflow.variables,
        "source preparation must not publish upstream stages to make the list readable"
    );
    assert!(!before_publication
        .completed_tasks
        .iter()
        .any(|name| name == "choose"));
    let requested = activation.apply(&mut tx, KEY).await.unwrap();
    assert!(
        ExecutionRepository::find_by_id(&mut *tx, completed.id)
            .await
            .unwrap()
            .unwrap()
            .result
            .is_none(),
        "successor activation must not invent or persist a null result witness"
    );
    assert_eq!(
        requested.len(),
        successors * 2,
        "each real list preserves its concurrency window"
    );
    assert!(
        activation.apply(&mut tx, KEY).await.is_err(),
        "fan-out writer is one-shot"
    );
    for child in &requested {
        let metadata = child.workflow_task.as_ref().unwrap();
        assert_eq!(metadata.triggered_by.as_deref(), Some("choose"));
        let selected = metadata.task_index.unwrap();
        let inputs = Materialize::prepare_execution_input_receipts(&mut tx, child.id, KEY)
            .await
            .unwrap();
        assert!(!inputs.is_empty());
        let input = Native::capture_with_key(&mut tx, child.id, &[], Some(KEY))
            .await
            .unwrap()
            .restore_prepared(
                &mut tx,
                ENTITY_EXECUTION_CONFIG,
                child.id,
                child.config.clone().unwrap(),
                Some(KEY),
            )
            .await
            .unwrap();
        if selected == 0 {
            assert_eq!(input["payload"]["selected"]["opaque"], json!(LITERAL));
        }
    }
    let published = Workflows::find_by_id(&mut *tx, workflow.id)
        .await
        .unwrap()
        .unwrap();
    assert!(published
        .completed_tasks
        .iter()
        .any(|name| name == "choose"));
    assert_eq!(published.current_tasks.len(), successors);
    if pending {
        assert_eq!(
            published.variables["selected"],
            attune_common::secret_values::redaction_marker()
        );
        let fields = Secrets::find_stored_by_entity(&mut *tx, ENTITY_WORKFLOW_VARIABLES, root.id)
            .await
            .unwrap();
        let restored = attune_common::secret_values::restore_secret_values(
            published.variables.clone(),
            &fields,
            KEY,
        )
        .unwrap();
        assert_eq!(
            restored["selected"],
            json!([{"opaque":LITERAL},{"v":2},{"v":3}])
        );
    }
    activation.ensure_fresh_on(&mut tx).await.unwrap();
    tx.commit().await.unwrap();
    activation.ensure_fresh().unwrap();
    let before = ExecutionRepository::find_by_parent(db.pool(), root.id)
        .await
        .unwrap();
    assert_eq!(
        before.len(),
        1 + successors * 3,
        "all siblings first-sealed at activation"
    );
    let mut tx = db.begin().await.unwrap();
    CacheEntryRepository::protect_transaction(&mut tx, CacheTransactionMode::PinMutation)
        .await
        .unwrap();
    let mut recovery = Iteration::prepare_successors(&mut tx, &root, &completed, KEY)
        .await
        .unwrap()
        .expect("duplicate delivery recovers committed lists, not publications");
    let replay = recovery.apply(&mut tx, KEY).await.unwrap();
    assert_eq!(replay.len(), successors * 2);
    let mut expected: Vec<_> = requested.iter().map(|child| child.id).collect();
    let mut actual: Vec<_> = replay.iter().map(|child| child.id).collect();
    expected.sort();
    actual.sort();
    assert_eq!(actual, expected);
    assert_eq!(
        ExecutionRepository::find_by_parent(&mut *tx, root.id)
            .await
            .unwrap()
            .len(),
        before.len()
    );
    recovery.ensure_fresh_on(&mut tx).await.unwrap();
    tx.commit().await.unwrap();
    recovery.ensure_fresh().unwrap();
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn native_ordinary_iteration_real_predecessor_activates_committed_list_and_recovers() {
    run_successor_activation(1, false).await;
}

#[tokio::test]
async fn native_ordinary_iteration_real_predecessor_fans_out_only_sealed_selected_lists() {
    run_successor_activation(2, false).await;
}

#[tokio::test]
async fn native_ordinary_iteration_pending_publication_uses_official_frame_and_list_control() {
    // Requires A's official pending VALUE-to-iteration Control implementation.
    // This must not be made green by rendering/copying a list outside the token.
    run_successor_activation(2, true).await;
}

#[tokio::test]
async fn native_ordinary_iteration_predecessor_changes_before_activation_apply_are_rejected() {
    let (db, root, _, _, completed) = setup_graph(json!([1]), None, false, 1, false).await;
    let completed = completed.unwrap();
    let mut tx = db.begin().await.unwrap();
    CacheEntryRepository::protect_transaction(&mut tx, CacheTransactionMode::PinMutation)
        .await
        .unwrap();
    let mut activation = Iteration::prepare_successors(&mut tx, &root, &completed, KEY)
        .await
        .unwrap()
        .unwrap();
    ExecutionRepository::update(
        &mut *tx,
        completed.id,
        UpdateExecutionInput {
            result: Some(json!({"changed":true})),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert!(activation.apply(&mut tx, KEY).await.is_err());
    tx.rollback().await.unwrap();
    assert_eq!(
        ExecutionRepository::find_by_parent(db.pool(), root.id)
            .await
            .unwrap()
            .len(),
        1
    );
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn native_ordinary_iteration_indexed_child_cannot_impersonate_successor_predecessor() {
    let (db, root, _, _) = setup(json!([1]), None, false).await;
    let mut tx = db.begin().await.unwrap();
    CacheEntryRepository::protect_transaction(&mut tx, CacheTransactionMode::PinMutation)
        .await
        .unwrap();
    let mut initial = Iteration::prepare(&mut tx, &root, "ordinary", None, KEY)
        .await
        .unwrap();
    let child = initial
        .apply(&mut tx, KEY)
        .await
        .unwrap()
        .requested
        .remove(0);
    assert!(
        Iteration::prepare_completion(&mut tx, root.id, "ordinary", KEY)
            .await
            .unwrap()
            .is_none(),
        "requested ordinal is not whole-iteration completion"
    );
    let completed = ExecutionRepository::update(
        &mut *tx,
        child.id,
        UpdateExecutionInput {
            status: Some(ExecutionStatus::Completed),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert!(
        Iteration::prepare_successors(&mut tx, &root, &completed, KEY)
            .await
            .unwrap()
            .is_none()
    );
    tx.rollback().await.unwrap();
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn native_ordinary_iteration_whole_outcome_rejects_inconsistent_retry_lineage() {
    let (db, root, _, _) = setup(json!([1]), None, false).await;
    let mut tx = db.begin().await.unwrap();
    CacheEntryRepository::protect_transaction(&mut tx, CacheTransactionMode::PinMutation)
        .await
        .unwrap();
    let mut initial = Iteration::prepare(&mut tx, &root, "ordinary", None, KEY)
        .await
        .unwrap();
    let child = initial
        .apply(&mut tx, KEY)
        .await
        .unwrap()
        .requested
        .remove(0);
    let mut metadata = child.workflow_task.unwrap();
    metadata.retry_count = 1;
    ExecutionRepository::update(
        &mut *tx,
        child.id,
        UpdateExecutionInput {
            workflow_task: Some(metadata),
            status: Some(ExecutionStatus::Completed),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert!(
        Iteration::prepare_completion(&mut tx, root.id, "ordinary", KEY)
            .await
            .is_err(),
        "a forged metadata retry is not an authoritative latest attempt"
    );
    tx.rollback().await.unwrap();
    db.cleanup().await.unwrap();
}

async fn terminal_source_fixture() -> (TestDatabase, Execution, WorkflowExecution, Execution) {
    let (db, root, workflow, _) = setup(json!([1]), None, false).await;
    let mut tx = db.begin().await.unwrap();
    CacheEntryRepository::protect_transaction(&mut tx, CacheTransactionMode::PinMutation)
        .await
        .unwrap();
    let mut initial = Iteration::prepare(&mut tx, &root, "ordinary", None, KEY)
        .await
        .unwrap();
    let child = initial
        .apply(&mut tx, KEY)
        .await
        .unwrap()
        .requested
        .remove(0);
    let child = ExecutionRepository::update(
        &mut *tx,
        child.id,
        UpdateExecutionInput {
            status: Some(ExecutionStatus::Completed),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    (db, root, workflow, child)
}

#[tokio::test]
async fn native_ordinary_iteration_terminal_observation_does_not_reopen_input_delivery() {
    let (db, root, _, child) = terminal_source_fixture().await;
    let mut tx = db.begin().await.unwrap();
    CacheEntryRepository::protect_transaction(&mut tx, CacheTransactionMode::PinMutation)
        .await
        .unwrap();
    let whole = Iteration::prepare_completion(&mut tx, root.id, "ordinary", KEY)
        .await
        .unwrap()
        .unwrap();
    whole.require_source_current(&mut tx).await.unwrap();
    let denied = Materialize::prepare_execution_input_receipts(&mut tx, child.id, KEY)
        .await
        .err()
        .expect("terminal input delivery remains forbidden");
    assert!(
        denied
            .to_string()
            .contains("no longer permits input delivery"),
        "terminal delivery gate must remain intact: {denied}"
    );
    whole.require_members_current(&mut tx).await.unwrap();
    let mut finalizer = Iteration::prepare(&mut tx, &root, "ordinary", None, KEY)
        .await
        .unwrap();
    let result = finalizer.apply(&mut tx, KEY).await.unwrap();
    assert_eq!(
        result.completed_parent.unwrap().status,
        ExecutionStatus::Completed
    );
    tx.commit().await.unwrap();
    let mut delivery = db.begin().await.unwrap();
    CacheEntryRepository::protect_transaction(&mut delivery, CacheTransactionMode::PinMutation)
        .await
        .unwrap();
    let observed = finalizer
        .prepare_control_delivery(&mut delivery, KEY)
        .await
        .unwrap();
    observed.ensure_fresh_on(&mut delivery).await.unwrap();
    delivery.commit().await.unwrap();
    observed.ensure_fresh().unwrap();
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn native_ordinary_iteration_membership_check_allows_variable_publication_not_source_rebaseline(
) {
    let (db, root, workflow, child) = terminal_source_fixture().await;
    let mut tx = db.begin().await.unwrap();
    CacheEntryRepository::protect_transaction(&mut tx, CacheTransactionMode::PinMutation)
        .await
        .unwrap();
    let whole = Iteration::prepare_completion(&mut tx, root.id, "ordinary", KEY)
        .await
        .unwrap()
        .unwrap();
    whole.require_source_current(&mut tx).await.unwrap();
    // Model the variable writer's effect only. This operation is not a frame
    // publication seal and cannot authorize effects through the full check.
    Workflows::update(
        &mut *tx,
        workflow.id,
        UpdateWorkflowExecutionInput {
            variables: Some(json!({"saved":"changed"})),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert!(
        whole.require_source_current(&mut tx).await.is_err(),
        "full original workflow check must remain intact"
    );
    whole.require_members_current(&mut tx).await.unwrap();
    let mut metadata = child.workflow_task.unwrap();
    metadata.task_batch = Some(9);
    ExecutionRepository::update(
        &mut *tx,
        child.id,
        UpdateExecutionInput {
            workflow_task: Some(metadata),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert!(
        whole.require_members_current(&mut tx).await.is_err(),
        "publication does not excuse changed membership"
    );
    tx.rollback().await.unwrap();
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn native_ordinary_iteration_terminal_observation_rejects_revoked_source_admission() {
    let (db, root, _, child) = terminal_source_fixture().await;
    let mut tx = db.begin().await.unwrap();
    CacheEntryRepository::protect_transaction(&mut tx, CacheTransactionMode::PinMutation)
        .await
        .unwrap();
    let admission = RuntimeAuthorityRepository::execution_admission(&mut tx, child.id)
        .await
        .unwrap()
        .unwrap();
    assert!(
        RuntimeAuthorityRepository::revoke_admission(&mut tx, admission.id)
            .await
            .unwrap()
    );
    assert!(
        Iteration::prepare_completion(&mut tx, root.id, "ordinary", KEY)
            .await
            .is_err()
    );
    tx.rollback().await.unwrap();
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn native_ordinary_iteration_terminal_observation_requires_original_accepted_archive() {
    let (db, root, _, child) = terminal_source_fixture().await;
    let mut tx = db.begin().await.unwrap();
    CacheEntryRepository::protect_transaction(&mut tx, CacheTransactionMode::PinMutation)
        .await
        .unwrap();
    let whole = Iteration::prepare_completion(&mut tx, root.id, "ordinary", KEY)
        .await
        .unwrap()
        .unwrap();
    // Deliberately corrupt a fixture's current field set. No parser may turn
    // these bytes into new input provenance or repair its original acceptance.
    Secrets::replace_entity_with_conn(
        &mut tx,
        ENTITY_EXECUTION_CONFIG,
        child.id,
        &[attune_common::secret_values::PreparedSecretValue {
            json_path: "/tampered".into(),
            encrypted_value: json!({"invalid":true}),
            encryption_key_hash: attune_common::crypto::hash_encryption_key(KEY),
            source_kind: "provenance".into(),
            source_ref: Some("{}".into()),
        }],
    )
    .await
    .unwrap();
    assert!(
        whole.require_members_current(&mut tx).await.is_err(),
        "held original source fields must still match after a target wait"
    );
    assert!(
        Iteration::prepare_completion(&mut tx, root.id, "ordinary", KEY)
            .await
            .is_err()
    );
    tx.rollback().await.unwrap();
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn native_ordinary_iteration_terminal_source_cannot_complete_cancelled_parent() {
    let (db, root, _, _) = terminal_source_fixture().await;
    let mut tx = db.begin().await.unwrap();
    CacheEntryRepository::protect_transaction(&mut tx, CacheTransactionMode::PinMutation)
        .await
        .unwrap();
    let whole = Iteration::prepare_completion(&mut tx, root.id, "ordinary", KEY)
        .await
        .unwrap()
        .unwrap();
    ExecutionRepository::update(
        &mut *tx,
        root.id,
        UpdateExecutionInput {
            status: Some(ExecutionStatus::Cancelled),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert!(whole.require_source_current(&mut tx).await.is_err());
    assert!(
        Iteration::prepare_completion(&mut tx, root.id, "ordinary", KEY)
            .await
            .unwrap()
            .is_none()
    );
    tx.rollback().await.unwrap();
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn native_ordinary_iteration_terminal_observation_rejects_revoked_runtime_policy() {
    let (db, root, _, child) = terminal_source_fixture().await;
    let mut tx = db.begin().await.unwrap();
    CacheEntryRepository::protect_transaction(&mut tx, CacheTransactionMode::PinMutation)
        .await
        .unwrap();
    let admission = RuntimeAuthorityRepository::execution_admission(&mut tx, child.id)
        .await
        .unwrap()
        .unwrap();
    assert!(
        RuntimeAuthorityRepository::revoke_policy(&mut tx, admission.policy_revision)
            .await
            .unwrap()
    );
    assert!(
        Iteration::prepare_completion(&mut tx, root.id, "ordinary", KEY)
            .await
            .is_err()
    );
    tx.rollback().await.unwrap();
    db.cleanup().await.unwrap();
}

async fn group_activation_fixture(
    failed: bool,
    retry: bool,
) -> (
    TestDatabase,
    Execution,
    WorkflowExecution,
    Vec<Execution>,
    Execution,
) {
    let (db, root, workflow, _, _) = setup_graph_kind(
        json!([{"opaque":LITERAL},{"v":2}]),
        None,
        false,
        0,
        false,
        Some((failed, retry)),
    )
    .await;
    let mut tx = db.begin().await.unwrap();
    CacheEntryRepository::protect_transaction(&mut tx, CacheTransactionMode::PinMutation)
        .await
        .unwrap();
    let mut initial = Iteration::prepare(&mut tx, &root, "ordinary", None, KEY)
        .await
        .unwrap();
    initial.apply(&mut tx, KEY).await.unwrap();
    let mut members = ExecutionRepository::find_by_parent(&mut *tx, root.id)
        .await
        .unwrap();
    members.sort_by_key(|member| member.workflow_task.as_ref().unwrap().task_index);
    assert_eq!(members.len(), 2);
    for (index, member) in members.iter_mut().enumerate() {
        *member = ExecutionRepository::update(
            &mut *tx,
            member.id,
            UpdateExecutionInput {
                status: Some(if index == 0 && (failed || retry) {
                    ExecutionStatus::Failed
                } else {
                    ExecutionStatus::Completed
                }),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    }
    tx.commit().await.unwrap();
    let delivered = if retry {
        use attune_common::repositories::native_retry_materialization::NativeRetryMaterializationRepository as Retry;
        let mut tx = db.begin().await.unwrap();
        CacheEntryRepository::protect_transaction(&mut tx, CacheTransactionMode::PinMutation)
            .await
            .unwrap();
        let workflow = Workflows::find_by_id(&mut *tx, workflow.id)
            .await
            .unwrap()
            .unwrap();
        let prepared = Retry::prepare(&mut tx, &members[0], &workflow, KEY)
            .await
            .unwrap();
        let mut metadata = members[0].workflow_task.clone().unwrap();
        metadata.retry_count += 1;
        metadata.next_retry_at = Some(Utc::now());
        let retry = prepared
            .materialize(&mut tx, metadata)
            .await
            .unwrap()
            .expect("real I-created and first-sealed retry");
        assert_eq!(retry.original_execution, Some(members[0].id));
        assert_eq!(retry.retry_count, 1);
        let completed = ExecutionRepository::update(
            &mut *tx,
            retry.id,
            UpdateExecutionInput {
                status: Some(ExecutionStatus::Completed),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        tx.commit().await.unwrap();
        assert_ne!(completed.id, members[0].id);
        // Deliver the superseded failure, not the successful latest retry. The
        // whole factory must still select the real latest attempt per ordinal.
        members[0].clone()
    } else {
        members[1].clone()
    };
    (db, root, workflow, members, delivered)
}

async fn run_group_activation(failed: bool, retry: bool) {
    let (db, root, workflow, members, delivered) = group_activation_fixture(failed, retry).await;
    let mut tx = db.begin().await.unwrap();
    CacheEntryRepository::protect_transaction(&mut tx, CacheTransactionMode::PinMutation)
        .await
        .unwrap();
    let mut activation = Iteration::prepare_iteration_activation(&mut tx, &root, &delivered, KEY)
        .await
        .unwrap()
        .expect("actual whole-iteration successor frame");
    let source = activation
        .iteration_source()
        .expect("the exact D Arc retained by B's group factory");
    assert_eq!(source.ordinal_count(), 2);
    assert_eq!(
        source.status(),
        if failed {
            ExecutionStatus::Failed
        } else {
            ExecutionStatus::Completed
        }
    );
    if retry {
        let member = source.members().next().unwrap();
        assert_ne!(member.execution(), members[0].id);
        assert_eq!(member.original_execution(), members[0].id);
        assert_eq!(member.retry_count(), 1);
        assert_eq!(member.status(), ExecutionStatus::Completed);
    }
    let requested = activation.apply(&mut tx, KEY).await.unwrap();
    assert_eq!(requested.len(), 2);
    let selected = if failed {
        "ordinary_failure"
    } else {
        "ordinary_success"
    };
    for child in &requested {
        let metadata = child.workflow_task.as_ref().unwrap();
        assert_eq!(metadata.task_name, selected);
        assert_eq!(metadata.triggered_by.as_deref(), Some("ordinary"));
        let input = Native::capture_with_key(&mut tx, child.id, &[], Some(KEY))
            .await
            .unwrap()
            .restore_prepared(
                &mut tx,
                ENTITY_EXECUTION_CONFIG,
                child.id,
                child.config.clone().unwrap(),
                Some(KEY),
            )
            .await
            .unwrap();
        if metadata.task_index == Some(0) {
            assert_eq!(input["payload"]["selected"]["opaque"], json!(LITERAL));
        }
        Materialize::prepare_execution_input_receipts(&mut tx, child.id, KEY)
            .await
            .unwrap();
    }
    let state = Workflows::find_by_id(&mut *tx, workflow.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(state.current_tasks, vec![selected.to_owned()]);
    assert_eq!(state.failed_tasks.contains(&"ordinary".into()), failed);
    assert_eq!(state.completed_tasks.contains(&"ordinary".into()), !failed);
    assert!(
        !state
            .variables
            .as_object()
            .unwrap()
            .contains_key("forbidden"),
        "unselected publication Key must never be evaluated"
    );
    let source = activation.iteration_source().unwrap();
    assert!(
        source.require_source_current(&mut tx).await.is_err(),
        "full original variable equality must not be used after genuine publication"
    );
    source.require_members_current(&mut tx).await.unwrap();
    activation.ensure_fresh_on(&mut tx).await.unwrap();
    tx.commit().await.unwrap();
    activation.ensure_fresh().unwrap();
    let before = ExecutionRepository::find_by_parent(db.pool(), root.id)
        .await
        .unwrap();
    let mut tx = db.begin().await.unwrap();
    CacheEntryRepository::protect_transaction(&mut tx, CacheTransactionMode::PinMutation)
        .await
        .unwrap();
    let mut recovery = Iteration::prepare_iteration_activation(&mut tx, &root, &delivered, KEY)
        .await
        .unwrap()
        .expect("committed successor recovery");
    assert!(
        recovery.iteration_source().is_none(),
        "duplicate delivery must not recapture another group"
    );
    let replay = recovery.apply(&mut tx, KEY).await.unwrap();
    let mut expected: Vec<_> = requested.iter().map(|child| child.id).collect();
    expected.sort();
    let mut actual: Vec<_> = replay.iter().map(|child| child.id).collect();
    actual.sort();
    assert_eq!(actual, expected);
    assert_eq!(
        ExecutionRepository::find_by_parent(&mut *tx, root.id)
            .await
            .unwrap()
            .len(),
        before.len()
    );
    assert_eq!(
        Workflows::find_by_id(&mut *tx, workflow.id)
            .await
            .unwrap()
            .unwrap()
            .variables,
        state.variables,
        "recovery cannot republish overwritten stages or reread publication Keys"
    );
    recovery.ensure_fresh_on(&mut tx).await.unwrap();
    tx.commit().await.unwrap();
    recovery.ensure_fresh().unwrap();
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn native_ordinary_iteration_whole_success_activates_real_successor_children() {
    run_group_activation(false, false).await;
}
#[tokio::test]
async fn native_ordinary_iteration_earlier_failed_member_last_success_selects_failure_successors() {
    run_group_activation(true, false).await;
}
#[tokio::test]
async fn native_ordinary_iteration_real_superseded_retry_selects_latest_group_and_recovers() {
    run_group_activation(false, true).await;
}
#[tokio::test]
async fn native_ordinary_iteration_group_member_changes_before_target_apply_are_rejected() {
    let (db, root, _, members, delivered) = group_activation_fixture(false, false).await;
    let mut tx = db.begin().await.unwrap();
    CacheEntryRepository::protect_transaction(&mut tx, CacheTransactionMode::PinMutation)
        .await
        .unwrap();
    let mut activation = Iteration::prepare_iteration_activation(&mut tx, &root, &delivered, KEY)
        .await
        .unwrap()
        .unwrap();
    let mut metadata = members[0].workflow_task.clone().unwrap();
    metadata.task_batch = Some(99);
    ExecutionRepository::update(
        &mut *tx,
        members[0].id,
        UpdateExecutionInput {
            workflow_task: Some(metadata),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert!(activation.apply(&mut tx, KEY).await.is_err());
    tx.rollback().await.unwrap();
    assert_eq!(
        ExecutionRepository::find_by_parent(db.pool(), root.id)
            .await
            .unwrap()
            .len(),
        2
    );
    db.cleanup().await.unwrap();
}
