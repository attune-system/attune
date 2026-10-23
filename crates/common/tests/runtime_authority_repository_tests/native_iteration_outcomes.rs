//! Real D dispatch/input seals -> finite A group CONTROL -> canonical replay.
use super::*;
use attune_common::{
    models::{Execution, WorkflowExecution},
    repositories::{
        action::{ActionRepository, UpdateActionInput},
        declared_input::DeclaredInputRepository,
        declared_input_materialization::DeclaredInputMaterializationRepository as Materialize,
        inherited_input::{
            InheritedInputRepository as Inputs, InputEntityKind, InputSite, InputSitePurpose,
        },
        native_input_snapshot::NativeInputSnapshotRepository as Snapshots,
        native_iteration_materialization::NativeIterationMaterializationRepository as Iteration,
        recorded_input_authority::RecordedInputAuthorityRepository as Contributors,
        schema_contract::{BindingOwner, SchemaContractRepository as Schemas},
        workflow::{
            CreateWorkflowDefinitionInput, CreateWorkflowExecutionInput,
            WorkflowDefinitionRepository, WorkflowExecutionRepository as Workflows,
        },
        FindById,
    },
};
use std::sync::Arc;
const KEY: &str = "ordinary-group-outcome-original-test-key";

#[tokio::test]
async fn group_control_uses_all_latest_members_not_the_delivered_last_success() {
    group_case(true).await;
}
#[tokio::test]
async fn group_control_records_literal_dependency_and_replays_after_raw_member_gc() {
    group_case(false).await;
}
#[tokio::test]
async fn zero_dispatch_is_not_a_nonempty_group_source() {
    let (db, root, _) = setup().await;
    let mut tx = db.begin().await.unwrap();
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
async fn truncated_original_dispatch_cannot_shrink_the_original_sealed_list() {
    let (db, root, workflow) = setup().await;
    let mut tx = db.begin().await.unwrap();
    let mut plan = Iteration::prepare(&mut tx, &root, "ordinary", None, KEY)
        .await
        .unwrap();
    let created = plan.apply(&mut tx, KEY).await.unwrap();
    assert_eq!(created.requested.len(), 2);
    for child in &created.requested {
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
    }
    tx.commit().await.unwrap();
    let mut tx = db.begin().await.unwrap();
    let missing = created.requested[1].id;
    sqlx::query(
        "DELETE FROM workflow_task_dispatch WHERE workflow_execution=$1 AND execution_id=$2",
    )
    .bind(workflow.id)
    .bind(missing)
    .execute(&mut *tx)
    .await
    .unwrap();
    ExecutionRepository::delete(&mut *tx, missing)
        .await
        .unwrap();
    if let Ok(Some(d)) = Iteration::prepare_completion(&mut tx, root.id, "ordinary", KEY).await {
        assert!(
            Inputs::prepare_native_iteration_outcome(&mut tx, &Arc::new(d), KEY)
                .await
                .is_err(),
            "the original with_items Control still contains two items"
        );
    }
    tx.rollback().await.unwrap();
    db.cleanup().await.unwrap();
}

async fn group_case(failed_first: bool) {
    let (db, root, workflow) = setup().await;
    let mut tx = db.begin().await.unwrap();
    let mut plan = Iteration::prepare(&mut tx, &root, "ordinary", None, KEY)
        .await
        .unwrap();
    let created = plan.apply(&mut tx, KEY).await.unwrap();
    assert_eq!(created.total, 2);
    assert_eq!(created.requested.len(), 2);
    plan.ensure_fresh_on(&mut tx).await.unwrap();
    tx.commit().await.unwrap();
    plan.ensure_fresh().unwrap();
    let mut tx = db.begin().await.unwrap();
    let mut ids = Vec::new();
    for (index, child) in created.requested.iter().enumerate() {
        let status = if failed_first && index == 0 {
            ExecutionStatus::Failed
        } else {
            ExecutionStatus::Completed
        };
        ExecutionRepository::update(
            &mut *tx,
            child.id,
            UpdateExecutionInput {
                status: Some(status),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        ids.push(child.id);
    }
    tx.commit().await.unwrap();

    let mut tx = db.begin().await.unwrap();
    let d = Arc::new(
        Iteration::prepare_completion(&mut tx, root.id, "ordinary", KEY)
            .await
            .unwrap()
            .unwrap(),
    );
    let source = Inputs::prepare_native_iteration_outcome(&mut tx, &d, KEY)
        .await
        .unwrap();
    let declarations = DeclaredInputRepository::bindings(&mut tx, root.id)
        .await
        .unwrap();
    let condition = declarations
        .iter()
        .find(|d| d.source_path == "/workflow/tasks/0/next/0/when")
        .unwrap();
    let mut session = Inputs::begin_render(
        &mut tx,
        root.id,
        &condition.declaration_revision,
        &condition.source_path,
        &condition.expression,
        InputSitePurpose::Condition,
    )
    .await
    .unwrap();
    Inputs::bind_native_iteration_outcome(&mut session, &source).unwrap();
    let rendered = Inputs::evaluate_condition(&mut tx, session).await.unwrap();
    assert_eq!(rendered.value(), &json!(!failed_first));
    let site = InputSite {
        execution: root.id,
        kind: InputEntityKind::WorkflowVariables,
        purpose: InputSitePurpose::Condition,
        path: condition.source_path.clone(),
    };
    let receipt = Inputs::record_render(
        &mut tx,
        &rendered,
        &site,
        rendered.value(),
        rendered.selections(),
        KEY,
    )
    .await
    .unwrap();
    let metadata = receipt
        .native_source_metadata()
        .iter()
        .find(|m| m.snapshot() == source.snapshot())
        .unwrap();
    assert_eq!(metadata.source_kind(), "ordinary_iteration_outcome");
    assert!(metadata.source_site().is_none());
    assert!(!receipt.source_origins().iter().any(|origin|matches!(origin,attune_common::secret_provenance::SecretOrigin::Entity {entity_type,entity_id,..} if entity_type=="execution_result"&&ids.contains(entity_id))));
    let frozen = Snapshots::source_evidence(&mut tx, metadata, KEY)
        .await
        .unwrap();
    let group = frozen.iteration_outcome().unwrap();
    assert_eq!(group.members().len(), 2);
    assert_eq!(
        group.status(),
        if failed_first { "failed" } else { "completed" }
    );
    assert_eq!(group.members()[0].original().execution(), ids[0]);
    assert_eq!(group.members()[1].latest().execution(), ids[1]);
    assert!(!group.list_controls().is_empty());
    let storage = Snapshots::storage_evidence(&mut tx, source.snapshot())
        .await
        .unwrap();
    assert!(storage.wait_release().is_none());
    assert!(storage.result_view_release().is_none());
    assert!(storage.completion_release().is_none());
    assert_eq!(storage.iteration_outcome().unwrap().members().len(), 2);
    assert!(frozen.fields().is_empty());
    assert!(!group.members()[0]
        .original()
        .original_input_fields()
        .is_empty());
    sqlx::query("SAVEPOINT group_release_guard")
        .execute(&mut *tx)
        .await
        .unwrap();
    assert!(
        sqlx::query("DELETE FROM runtime_native_iteration_outcome_release WHERE snapshot=$1")
            .bind(source.snapshot())
            .execute(&mut *tx)
            .await
            .is_err()
    );
    sqlx::query("ROLLBACK TO SAVEPOINT group_release_guard")
        .execute(&mut *tx)
        .await
        .unwrap();
    let retained = Inputs::retention_parents(&mut tx, receipt_id(&receipt))
        .await
        .unwrap();
    assert!(retained.native_snapshots().contains(&source.snapshot()));

    let mut other = db.begin().await.unwrap();
    let mut wrong = Inputs::begin_render(
        &mut other,
        root.id,
        &condition.declaration_revision,
        &condition.source_path,
        &condition.expression,
        InputSitePurpose::Condition,
    )
    .await
    .unwrap();
    assert!(Inputs::bind_native_iteration_outcome(&mut wrong, &source).is_err());
    other.rollback().await.unwrap();
    let wrong = Inputs::begin_render(
        &mut tx,
        root.id,
        &condition.declaration_revision,
        &condition.source_path,
        &condition.expression,
        InputSitePurpose::Value,
    )
    .await;
    assert!(
        matches!(wrong, Err(attune_common::Error::PermissionDenied(_))),
        "a Condition declaration must be rejected before a VALUE capability can be issued"
    );

    // Actual target pre-write guard, then a legitimate variable publication. D's
    // member-only check must not compare a replacement workflow baseline.
    let actual = Workflows::find_by_id_for_update(&mut *tx, workflow.id)
        .await
        .unwrap()
        .unwrap();
    let actual_parent = ExecutionRepository::find_by_id(&mut *tx, root.id)
        .await
        .unwrap()
        .unwrap();
    source
        .require_original_rows(&actual_parent, &actual)
        .unwrap();
    source.require_source_current(&mut tx).await.unwrap();
    let mut literal = Inputs::literal(
        &mut tx,
        root.id,
        &condition.declaration_revision,
        "/workflow/tasks/0/next/1/publish/0/summary",
    )
    .await
    .unwrap();
    Inputs::attach_native_iteration_control(&mut literal, &source).unwrap();
    let literal_site = InputSite {
        execution: root.id,
        kind: InputEntityKind::WorkflowVariables,
        purpose: InputSitePurpose::Value,
        path: "/summary".into(),
    };
    let literal_receipt = Inputs::record_render(
        &mut tx,
        &literal,
        &literal_site,
        literal.value(),
        literal.selections(),
        KEY,
    )
    .await
    .unwrap();
    assert_eq!(literal.value(), &json!("literal lifecycle dependency"));
    assert!(literal_receipt.native_source_metadata().contains(metadata));
    let fields = attune_common::secret_values::prepare_secret_values(
        vec![attune_common::secret_values::SecretValueInput {
            json_path: "/summary".into(),
            value: literal.value().clone(),
            source_kind: "provenance".into(),
            source_ref: Some(serde_json::to_string(&literal_receipt.provenance()).unwrap()),
        }],
        KEY,
    )
    .unwrap();
    attune_common::repositories::execution_secret_value::ExecutionSecretValueRepository::upsert_many_with_conn(&mut tx,"workflow_variables",root.id,&fields).await.unwrap();
    sqlx::query("UPDATE workflow_execution SET variables=$2 WHERE id=$1")
        .bind(workflow.id)
        .bind(json!({"summary":attune_common::secret_values::redaction_marker()}))
        .execute(&mut *tx)
        .await
        .unwrap();
    source.require_members_current(&mut tx).await.unwrap();
    assert!(source.require_source_current(&mut tx).await.is_err());
    sqlx::query("SAVEPOINT changed_group_input")
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("UPDATE execution SET config=config || $2::JSONB WHERE id=$1")
        .bind(ids[1])
        .bind(json!({"unaccepted":"changed input buffer"}))
        .execute(&mut *tx)
        .await
        .unwrap();
    assert!(source.ensure_fresh_on(&mut tx).await.is_err());
    sqlx::query("ROLLBACK TO SAVEPOINT changed_group_input")
        .execute(&mut *tx)
        .await
        .unwrap();
    source.ensure_fresh_on(&mut tx).await.unwrap();
    sqlx::query("SAVEPOINT changed_group_member")
        .execute(&mut *tx)
        .await
        .unwrap();
    ExecutionRepository::update(
        &mut *tx,
        ids[1],
        UpdateExecutionInput {
            status: Some(ExecutionStatus::Cancelled),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert!(source.require_members_current(&mut tx).await.is_err());
    sqlx::query("ROLLBACK TO SAVEPOINT changed_group_member")
        .execute(&mut *tx)
        .await
        .unwrap();
    source.require_members_current(&mut tx).await.unwrap();
    source.ensure_fresh_on(&mut tx).await.unwrap();
    let origin = receipt.origin();
    let snapshot = source.snapshot();
    tx.commit().await.unwrap();
    source.ensure_fresh().unwrap();

    // Historical replay needs the complete normalized archive and original
    // receipt/schema/admission graph, not the mutable raw children or dispatch.
    let mut tx = db.begin().await.unwrap();
    for child in ids {
        sqlx::query("DELETE FROM execution_secret_value WHERE entity_type='execution_config' AND entity_id=$1").bind(child).execute(&mut *tx).await.unwrap();
        ExecutionRepository::delete(&mut *tx, child).await.unwrap();
    }
    sqlx::query("DELETE FROM workflow_task_dispatch WHERE workflow_execution=$1")
        .bind(workflow.id)
        .execute(&mut *tx)
        .await
        .unwrap();
    let replay = Inputs::receipt_context(&mut tx, &site, &origin, KEY)
        .await
        .unwrap();
    assert_eq!(replay.origin(), origin);
    assert!(replay
        .native_source_metadata()
        .iter()
        .any(|m| m.snapshot() == snapshot));
    assert!(Snapshots::storage_evidence(&mut tx, snapshot)
        .await
        .unwrap()
        .iteration_outcome()
        .is_some());
    tx.rollback().await.unwrap();
    db.cleanup().await.unwrap();
}
fn receipt_id(receipt: &attune_common::repositories::inherited_input::ReceiptContext) -> Id {
    match receipt.origin() {
        attune_common::secret_provenance::SecretOrigin::Entity { entity_id, .. } => entity_id,
        _ => panic!("receipt entity origin"),
    }
}

async fn setup() -> (TestDatabase, Execution, WorkflowExecution) {
    let (db, fixture) = fixture().await;
    let owner = &fixture.policy.policy.owner;
    let action = ActionFixture::new(owner.pack_id, &owner.pack_ref, "group_graph")
        .create(&db)
        .await
        .unwrap();
    let leaf = ActionFixture::new(owner.pack_id, &owner.pack_ref, "group_leaf")
        .create(&db)
        .await
        .unwrap();
    ActionRepository::update(
        &db,
        action.id,
        UpdateActionInput {
            param_schema: Some(json!({"items":{"type":"array","items":{"type":"string"}}})),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    ActionRepository::update(
        &db,
        leaf.id,
        UpdateActionInput {
            param_schema: Some(json!({"item":{"type":"string"}})),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let definition = json!({"tasks":[{"name":"ordinary","action":leaf.r#ref,"with_items":"{{ parameters.items }}","concurrency":2,"params":{"item":"{{ item }}"},"permission_set_refs":[],"next":[{"when":"{{ succeeded() }}"},{"publish":[{"summary":"literal lifecycle dependency"}]}]}]});
    let graph = WorkflowDefinitionRepository::create(
        &db,
        CreateWorkflowDefinitionInput {
            r#ref: action.r#ref.clone(),
            pack: owner.pack_id,
            pack_ref: owner.pack_ref.clone(),
            label: "Group lifecycle".into(),
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
        'd',
    )
    .await;
    let snapshots = ExecutableSnapshotRepository::resolve_release_actions(&mut tx, release.id)
        .await
        .unwrap();
    let root_binding = InvocationBinding::Action {
        action_ref: action.r#ref.clone(),
    };
    let child_binding = InvocationBinding::WorkflowTask {
        workflow_ref: action.r#ref.clone(),
        task_path: "/tasks/ordinary".into(),
        action_ref: leaf.r#ref.clone(),
    };
    let policy = RuntimePolicy {
        owner: owner.clone(),
        release_id: release.id,
        release_digest: release.digest.clone(),
        concrete_targets: BTreeMap::new(),
        invocations: [root_binding.clone(), child_binding]
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
    let snapshot = &snapshots[&action.r#ref];
    let authority = policy
        .policy
        .select(
            policy.id,
            &root_binding,
            &[],
            fixture.admission.authority.attribution.clone(),
        )
        .unwrap();
    let root = ExecutionRepository::create_pinned(
        &mut *tx,
        CreateExecutionInput {
            action: Some(action.id),
            action_ref: action.r#ref.clone(),
            status: ExecutionStatus::Requested,
            executor: Some(fixture.human_id),
            ..Default::default()
        },
        snapshot,
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
    let schema = Schemas::materialize_action_pair(&mut tx, snapshot)
        .await
        .unwrap();
    Schemas::attach(&mut tx, BindingOwner::Execution, root.id, &schema)
        .await
        .unwrap();
    ExecutionRepository::initialize_config_with_conn(
        &mut tx,
        root.id,
        Some(admission.id),
        snapshot,
        Some(json!({"items":["first","last"]})),
        &[],
    )
    .await
    .unwrap();
    Contributors::capture_human(&mut tx, "execution", root.id, fixture.human_id)
        .await
        .unwrap();
    Materialize::seal_execution_inputs(&mut tx, root.id, KEY)
        .await
        .unwrap();
    let workflow = Workflows::create(
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
    tx.commit().await.unwrap();
    (db, root, workflow)
}
