//! Minimal owning empty-root fixture shared with protected human route tests.
#[path = "../../../executor/src/workflow/graph.rs"]
#[allow(dead_code)] // The fixture uses the production builder, not its inspectors.
mod executor_task_graph;
use crate::helpers;
use attune_common::{
    models::{Execution, ExecutionStatus, Id, WorkflowExecution},
    repositories::{
        action::{ActionRepository, UpdateActionInput},
        cache::{CacheEntryRepository, CacheTransactionMode},
        component_lifecycle::PackProjectionIds,
        declared_input_materialization::DeclaredInputMaterializationRepository as Materialize,
        executable_snapshot::ExecutableSnapshotRepository,
        execution::{CreateExecutionInput, ExecutionRepository, UpdateExecutionInput},
        execution_secret_value::ExecutionSecretValueRepository as Secrets,
        identity::{
            CreatePermissionAssignmentInput, CreatePermissionSetInput,
            PermissionAssignmentRepository, PermissionSetRepository,
        },
        managed_executable::ManagedExecutableRepository,
        native_failure_materialization::NativeFailureMaterializationRepository as Success,
        native_iteration_materialization::NativeIterationMaterializationRepository as Iteration,
        native_transition_materialization::NativeTransitionMaterializationRepository as Transitions,
        native_workflow_materialization::NativeWorkflowMaterializationRepository,
        pack_release::{CreatePackReleaseInput, PackReleaseRepository},
        rule_authority::RuleAuthorityRepository,
        runtime_authority::{RuntimeAuthorityRepository as Authority, RuntimeOwner},
        schema_contract::{BindingOwner, SchemaContractRepository as Schemas},
        workflow::{
            CreateWorkflowDefinitionInput, CreateWorkflowExecutionInput,
            UpdateWorkflowExecutionInput, WorkflowDefinitionRepository,
            WorkflowExecutionRepository as Workflows,
        },
        Create, FindById, Update,
    },
    runtime_authority::{
        HumanAttribution, InvocationBinding, InvocationPolicy, RequestAttribution, RuntimePolicy,
    },
    test_database::TestDatabase,
};
use helpers::*;
use serde_json::json;
use std::collections::BTreeMap;
const KEY: &str = "0123456789abcdef0123456789abcdef";

pub(crate) async fn human_reader(
    db: &sqlx::PgPool,
    refs: &[String],
    actions: &[&str],
) -> (Id, String, Id) {
    let identity = IdentityFixture::new_unique("empty_root_reader")
        .create(db)
        .await
        .unwrap();
    let assignment = add_reader_grant(db, identity.id, refs, actions).await;
    (identity.id, identity.login, assignment)
}

pub(crate) async fn add_reader_grant(
    db: &sqlx::PgPool,
    identity: Id,
    refs: &[String],
    actions: &[&str],
) -> Id {
    let set = PermissionSetRepository::create(db, CreatePermissionSetInput {
        r#ref: format!("fixture.empty_root_reader_{}", uuid::Uuid::new_v4().simple()),
        pack: None, pack_ref: None, label: None, description: None,
        grants: json!([{"resource":"executions","actions":actions,"constraints":{"refs":refs}}]),
    }).await.unwrap();
    let assignment = PermissionAssignmentRepository::create(
        db,
        CreatePermissionAssignmentInput {
            identity,
            permset: set.id,
        },
    )
    .await
    .unwrap();
    assignment.id
}

pub(crate) async fn completed_empty_root(
    successor: bool,
) -> (
    TestDatabase,
    Execution,
    WorkflowExecution,
    Option<Execution>,
) {
    let db = create_test_pool().await.unwrap();
    let pack = PackFixture::new_unique("empty_root_human")
        .create(&db)
        .await
        .unwrap();
    let human = IdentityFixture::new_unique("empty_root_submitter")
        .create(&db)
        .await
        .unwrap();
    let action = ActionFixture::new(pack.id, &pack.r#ref, "run")
        .create(&db)
        .await
        .unwrap();
    let leaf = ActionFixture::new(pack.id, &pack.r#ref, "leaf")
        .create(&db)
        .await
        .unwrap();
    for id in [action.id, leaf.id] {
        ActionRepository::update(
            &db,
            id,
            UpdateActionInput {
                param_schema: Some(json!({"items":{"type":"array"}})),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    }
    let mut definition = json!({"version":"1.0.0","tasks":[{"name":"ordinary","action":leaf.r#ref,"with_items":"{{ parameters.items }}","input":{"items":"{{ parameters.items }}"}}],"output_map":{"empty":"{{ parameters.items }}"}});
    if successor {
        definition["tasks"][0]["next"] =
            json!([{"when":"{{ succeeded() }}","do":["ordinary_success"]}]);
        definition["tasks"].as_array_mut().unwrap().push(json!({"name":"ordinary_success","action":leaf.r#ref,"input":{"items":"{{ parameters.items }}"}}));
    }
    let graph = WorkflowDefinitionRepository::create(
        &db,
        CreateWorkflowDefinitionInput {
            r#ref: action.r#ref.clone(),
            pack: pack.id,
            pack_ref: pack.r#ref.clone(),
            label: "Empty root".into(),
            description: None,
            version: "1.0.0".into(),
            param_schema: None,
            out_schema: None,
            definition: definition.clone(),
            tags: vec![],
        },
    )
    .await
    .unwrap();
    ActionRepository::link_workflow_def(&db, action.id, graph.id)
        .await
        .unwrap();
    let mut tx = db.begin().await.unwrap();
    CacheEntryRepository::protect_transaction(&mut tx, CacheTransactionMode::PinMutation)
        .await
        .unwrap();
    let digest = "e".repeat(64);
    let release = PackReleaseRepository::create_or_get(
        &mut tx,
        CreatePackReleaseInput {
            pack: pack.id,
            pack_ref: pack.r#ref.clone(),
            version: "1.0.0".into(),
            digest: digest.clone(),
            object_key: format!("packs/blobs/{digest}.tar.gz"),
            provider_version: format!("e:{digest}"),
            content_path: format!("/packs/.releases/{digest}"),
            archive_size: 1,
            manifest: json!({}),
        },
    )
    .await
    .unwrap();
    PackReleaseRepository::activate_projected(
        &mut tx,
        pack.id,
        release.id,
        &PackProjectionIds {
            actions: vec![action.id, leaf.id],
            ..Default::default()
        },
    )
    .await
    .unwrap();
    RuleAuthorityRepository::capture_deployment(&mut tx, pack.id)
        .await
        .unwrap();
    let owner = Authority::find_owner(&mut tx, pack.id)
        .await
        .unwrap()
        .unwrap();
    let snapshots = ExecutableSnapshotRepository::resolve_release_actions(&mut tx, release.id)
        .await
        .unwrap();
    let root_binding = InvocationBinding::Action {
        action_ref: action.r#ref.clone(),
    };
    let mut bindings = vec![
        root_binding.clone(),
        InvocationBinding::WorkflowTask {
            workflow_ref: action.r#ref.clone(),
            task_path: "/tasks/ordinary".into(),
            action_ref: leaf.r#ref.clone(),
        },
    ];
    if successor {
        bindings.push(InvocationBinding::WorkflowTask {
            workflow_ref: action.r#ref.clone(),
            task_path: "/tasks/ordinary_success".into(),
            action_ref: leaf.r#ref.clone(),
        });
    }
    let policy = Authority::capture_policy(
        &mut tx,
        &RuntimePolicy {
            owner,
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
        },
    )
    .await
    .unwrap();
    Authority::activate_policy(&mut tx, policy.id)
        .await
        .unwrap();
    let snapshot =
        ManagedExecutableRepository::resolve_invocation(&mut tx, &policy.policy, &root_binding)
            .await
            .unwrap();
    let selected = policy
        .policy
        .select(
            policy.id,
            &root_binding,
            &[],
            RequestAttribution::Human {
                actor: HumanAttribution {
                    identity_id: human.id,
                    login: human.login,
                },
            },
        )
        .unwrap();
    let root = ExecutionRepository::create_pinned(
        &mut *tx,
        CreateExecutionInput {
            action: Some(action.id),
            action_ref: action.r#ref,
            executor: Some(human.id),
            status: ExecutionStatus::Requested,
            ..Default::default()
        },
        &snapshot,
    )
    .await
    .unwrap();
    let admission =
        Authority::record_admission(&mut tx, RuntimeOwner::Execution(root.id), &selected)
            .await
            .unwrap();
    let schema = Schemas::materialize_action_pair(&mut tx, &snapshot)
        .await
        .unwrap();
    Schemas::attach(&mut tx, BindingOwner::Execution, root.id, &schema)
        .await
        .unwrap();
    ExecutionRepository::initialize_config_with_conn(
        &mut tx,
        root.id,
        Some(admission.id),
        &snapshot,
        Some(json!({"items":[]})),
        &[],
    )
    .await
    .unwrap();
    attune_common::repositories::recorded_input_authority::RecordedInputAuthorityRepository::capture_human(&mut tx, "execution", root.id, human.id).await.unwrap();
    Materialize::seal_execution_inputs(&mut tx, root.id, KEY)
        .await
        .unwrap();
    let variables =
        NativeWorkflowMaterializationRepository::prepare_initial_variables(&mut tx, root.id, KEY)
            .await
            .unwrap();
    let parsed: attune_common::workflow::WorkflowDefinition =
        serde_json::from_value(definition).unwrap();
    let workflow = Workflows::create(
        &mut *tx,
        CreateWorkflowExecutionInput {
            execution: root.id,
            workflow_def: graph.id,
            task_graph: serde_json::to_value(
                executor_task_graph::TaskGraph::from_workflow(&parsed).unwrap(),
            )
            .unwrap(),
            variables: variables.variables().clone(),
            status: ExecutionStatus::Running,
        },
    )
    .await
    .unwrap();
    Workflows::update(
        &mut *tx,
        workflow.id,
        UpdateWorkflowExecutionInput {
            current_tasks: Some(vec!["ordinary".into()]),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let root = ExecutionRepository::update(
        &mut *tx,
        root.id,
        UpdateExecutionInput {
            status: Some(ExecutionStatus::Running),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    variables.ensure_fresh_on(&mut tx).await.unwrap();
    tx.commit().await.unwrap();
    let mut tx = db.begin().await.unwrap();
    CacheEntryRepository::protect_transaction(&mut tx, CacheTransactionMode::PinMutation)
        .await
        .unwrap();
    let mut source = Iteration::prepare(&mut tx, &root, "ordinary", None, KEY)
        .await
        .unwrap();
    if !successor {
        let result = source.apply(&mut tx, KEY).await.unwrap();
        assert!(result.requested.is_empty());
        let completed = result.completed_parent.unwrap();
        source.ensure_fresh_on(&mut tx).await.unwrap();
        let workflow = Workflows::find_by_id(&mut *tx, workflow.id)
            .await
            .unwrap()
            .unwrap();
        tx.commit().await.unwrap();
        return (db, completed, workflow, None);
    }
    let mut activation = Iteration::prepare_empty_activation(&mut tx, source, KEY)
        .await
        .unwrap();
    let children = activation.apply(&mut tx, KEY).await.unwrap();
    assert_eq!(children.len(), 1);
    let child = ExecutionRepository::update(
        &mut *tx,
        children[0].id,
        UpdateExecutionInput {
            status: Some(ExecutionStatus::Completed),
            result: Some(json!({"data":{"items":[]}})),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    activation.ensure_fresh_on(&mut tx).await.unwrap();
    tx.commit().await.unwrap();
    drop(activation);
    let mut tx = db.begin().await.unwrap();
    CacheEntryRepository::protect_transaction(&mut tx, CacheTransactionMode::PinMutation)
        .await
        .unwrap();
    let current = Workflows::find_by_id(&mut *tx, workflow.id)
        .await
        .unwrap()
        .unwrap();
    let plan = Transitions::prepare_for_completion(&mut tx, &child, KEY)
        .await
        .unwrap()
        .unwrap();
    let output =
        Success::prepare_success_output(&mut tx, root.id, workflow.id, child.id, &plan, KEY)
            .await
            .unwrap()
            .unwrap();
    output.require_original_rows(&root, &current).unwrap();
    let recorded = plan.record(&mut tx, &current, KEY).await.unwrap();
    Secrets::replace_entity_with_conn(&mut tx, "workflow_variables", root.id, &recorded.secrets)
        .await
        .unwrap();
    let projected = Workflows::update(
        &mut *tx,
        workflow.id,
        UpdateWorkflowExecutionInput {
            variables: Some(recorded.variables),
            completed_tasks: Some(vec!["ordinary".into(), "ordinary_success".into()]),
            current_tasks: Some(vec![]),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let (value, sources) = output.record(&mut tx, &projected, KEY).await.unwrap();
    assert_eq!(value, json!({"empty":[],"succeeded":true}));
    let (public, fields) =
        attune_common::secret_values::redact_secret_path_sources(value, &sources);
    let fields = attune_common::secret_values::prepare_secret_values(fields, KEY).unwrap();
    Secrets::replace_entity_with_conn(&mut tx, "execution_result", root.id, &fields)
        .await
        .unwrap();
    let completed = ExecutionRepository::update(
        &mut *tx,
        root.id,
        UpdateExecutionInput {
            status: Some(ExecutionStatus::Completed),
            result: Some(public),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let workflow = Workflows::update(
        &mut *tx,
        workflow.id,
        UpdateWorkflowExecutionInput {
            status: Some(ExecutionStatus::Completed),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    output.ensure_fresh_on(&mut tx).await.unwrap();
    tx.commit().await.unwrap();
    (db, completed, workflow, Some(child))
}
