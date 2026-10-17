//! CENTRAL ONLY, UNRUN. Actual D dispatch/seals -> A group INPUT archive -> F files.
//! Requires ALL A d2b2a8fb/628303e9/b0d1cf3e and the matching D/J APIs.
use super::*;
use attune_common::{
    models::{Execution, WorkflowExecution},
    repositories::{
        action::{ActionRepository, UpdateActionInput},
        declared_input_materialization::DeclaredInputMaterializationRepository as Materialize,
        execution_secret_value::ExecutionSecretValueRepository as Secrets,
        native_iteration_materialization::NativeIterationMaterializationRepository as Iteration,
        workflow::WorkflowExecutionRepository as Workflows,
    },
    secret_provenance::SecretOrigin,
    secret_values::{prepare_secret_values, redact_secret_parameters},
};
use std::sync::Arc;

#[tokio::test]
async fn ordinary_group_input_archive_keeps_file_after_raw_member_config_and_dispatch_gc() {
    custody_case(false).await;
}

#[tokio::test]
async fn one_original_member_revocation_cannot_be_hidden_by_the_other_members_ceiling() {
    custody_case(true).await;
}

async fn setup() -> (TestDatabase, Fixture, Execution, WorkflowExecution) {
    let (db, fixture) = native_file_producer_fixture().await;
    let owner = &fixture.policy.policy.owner;
    let action = ActionFixture::new(owner.pack_id, &owner.pack_ref, "f_group_graph")
        .create(&db)
        .await
        .unwrap();
    let leaf = ActionFixture::new(owner.pack_id, &owner.pack_ref, "f_group_leaf")
        .create(&db)
        .await
        .unwrap();
    ActionRepository::update(
        &db,
        action.id,
        UpdateActionInput {
            param_schema: Some(
                json!({"items":{"type":"array","items":{"type":"string"},"secret":true}}),
            ),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    ActionRepository::update(
        &db,
        leaf.id,
        UpdateActionInput {
            param_schema: Some(json!({"item":{"type":"string","secret":true}})),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let definition = json!({"tasks":[{"name":"ordinary","action":leaf.r#ref,"with_items":"{{ parameters.items }}",
        "concurrency":2,"params":{"item":"{{ item }}"},"permission_set_refs":[],
        "next":[{"when":"{{ succeeded() }}"},{"publish":[{"summary":"literal lifecycle dependency"}]}]}]});
    let graph = WorkflowDefinitionRepository::create(
        &db,
        CreateWorkflowDefinitionInput {
            r#ref: action.r#ref.clone(),
            pack: owner.pack_id,
            pack_ref: owner.pack_ref.clone(),
            label: "F group custody".into(),
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
    let root_binding = InvocationBinding::Action {
        action_ref: action.r#ref.clone(),
    };
    let child_binding = InvocationBinding::WorkflowTask {
        workflow_ref: action.r#ref.clone(),
        task_path: "/tasks/ordinary".into(),
        action_ref: leaf.r#ref.clone(),
    };
    let policy = RuntimeAuthorityRepository::capture_policy(
        &mut tx,
        &RuntimePolicy {
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
        },
    )
    .await
    .unwrap();
    RuntimeAuthorityRepository::activate_policy(&mut tx, policy.id)
        .await
        .unwrap();
    let snapshot = &snapshots[&action.r#ref];
    let selected = policy
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
            executor: Some(fixture.human_id),
            status: ExecutionStatus::Requested,
            config: None,
            ..Default::default()
        },
        snapshot,
    )
    .await
    .unwrap();
    let admission = RuntimeAuthorityRepository::record_admission(
        &mut tx,
        RuntimeOwner::Execution(root.id),
        &selected,
    )
    .await
    .unwrap();
    let schema = SchemaContractRepository::materialize_action_pair(&mut tx, snapshot)
        .await
        .unwrap();
    schema.validate().unwrap();
    SchemaContractRepository::attach(&mut tx, BindingOwner::Execution, root.id, &schema)
        .await
        .unwrap();
    let (public, fields) = redact_secret_parameters(
        json!({"items":["original first input","original last input"]}),
        snapshot.executable.action.param_schema.as_ref(),
    );
    let fields = prepare_secret_values(fields, KEY).unwrap();
    ExecutionRepository::initialize_config_with_conn(
        &mut tx,
        root.id,
        Some(admission.id),
        snapshot,
        Some(public),
        &fields,
    )
    .await
    .unwrap();
    attune_common::repositories::recorded_input_authority::RecordedInputAuthorityRepository::capture_human(&mut tx,"execution",root.id,fixture.human_id).await.unwrap();
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
    (db, fixture, root, workflow)
}

async fn custody_case(revoke_member: bool) {
    let (db, fixture, root, workflow) = setup().await;
    let mut tx = db.begin().await.unwrap();
    let mut plan = Iteration::prepare(&mut tx, &root, "ordinary", None, KEY)
        .await
        .unwrap();
    let created = plan.apply(&mut tx, KEY).await.unwrap();
    assert_eq!(created.requested.len(), 2, "not a fake zero-member group");
    plan.ensure_fresh_on(&mut tx).await.unwrap();
    tx.commit().await.unwrap();
    let mut tx = db.begin().await.unwrap();
    let mut ids = Vec::new();
    for (index, child) in created.requested.iter().enumerate() {
        ExecutionRepository::update(
            &mut *tx,
            child.id,
            UpdateExecutionInput {
                status: Some(if index == 0 {
                    ExecutionStatus::Failed
                } else {
                    ExecutionStatus::Completed
                }),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert!(
            !Secrets::find_by_entity(&mut *tx, "execution_config", child.id)
                .await
                .unwrap()
                .is_empty()
        );
        assert!(
            child.result.is_none(),
            "group provenance is input observation, not a result field"
        );
        ids.push(child.id);
    }
    tx.commit().await.unwrap();

    let mut tx = db.begin().await.unwrap();
    let actual = Arc::new(
        Iteration::prepare_completion(&mut tx, root.id, "ordinary", KEY)
            .await
            .unwrap()
            .unwrap(),
    );
    let outcome = Inputs::prepare_native_iteration_outcome(&mut tx, &actual, KEY)
        .await
        .unwrap();
    let snapshot = outcome.snapshot();
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
    Inputs::bind_native_iteration_outcome(&mut session, &outcome).unwrap();
    let rendered = Inputs::evaluate_condition(&mut tx, session).await.unwrap();
    assert_eq!(
        rendered.value(),
        &json!(false),
        "the successful last child cannot hide the failed original member"
    );
    let condition_site = InputSite {
        execution: root.id,
        kind: InputEntityKind::WorkflowVariables,
        purpose: InputSitePurpose::Condition,
        path: condition.source_path.clone(),
    };
    let condition_receipt = Inputs::record_render(
        &mut tx,
        &rendered,
        &condition_site,
        rendered.value(),
        rendered.selections(),
        KEY,
    )
    .await
    .unwrap();
    let mut literal = Inputs::literal(
        &mut tx,
        root.id,
        &condition.declaration_revision,
        "/workflow/tasks/0/next/1/publish/0/summary",
    )
    .await
    .unwrap();
    Inputs::attach_native_iteration_control(&mut literal, &outcome).unwrap();
    let site = InputSite {
        execution: root.id,
        kind: InputEntityKind::WorkflowVariables,
        purpose: InputSitePurpose::Value,
        path: "/summary".into(),
    };
    let receipt = Inputs::record_render(
        &mut tx,
        &literal,
        &site,
        literal.value(),
        literal.selections(),
        KEY,
    )
    .await
    .unwrap();
    let storage = Snapshots::storage_evidence(&mut tx, snapshot)
        .await
        .unwrap();
    assert_eq!(storage.archive_owner(), ("native_input_snapshot", snapshot));
    assert!(
        storage.wait_release().is_none()
            && storage.result_view_release().is_none()
            && storage.completion_release().is_none()
    );
    assert!(storage.archive_fields().is_empty() && storage.contributor_manifests().is_empty());
    assert!(storage.completion_sources().is_empty());
    let group = storage.iteration_outcome().unwrap();
    assert_eq!(group.members().len(), 2);
    assert_eq!(group.status(), "failed");
    assert!(!group.list_controls().is_empty());
    let mut admissions = Vec::new();
    for member in group.members() {
        for observed in [member.original(), member.latest()] {
            assert!(ids.contains(&observed.execution()));
            assert!(!observed.original_input_fields().is_empty());
            assert!(!observed.input_receipt_edges().is_empty());
            assert!(!observed.contributor_manifests().is_empty());
            admissions.push(observed.admission());
        }
    }
    admissions.sort_unstable();
    admissions.dedup();
    assert_eq!(admissions.len(), 2);
    assert!(storage.origins().iter().all(
        |origin| matches!(origin,SecretOrigin::Entity{entity_type,entity_id,path}
        if entity_type=="ordinary_iteration_outcome"&&*entity_id==snapshot&&path.is_empty())
    ));
    let normalized: Vec<serde_json::Value> = sqlx::query_scalar(
        "SELECT member FROM runtime_native_iteration_outcome_member WHERE snapshot=$1 ORDER BY ordinal",
    )
    .bind(snapshot)
    .fetch_all(&mut *tx)
    .await
    .unwrap();
    assert_eq!(
        serde_json::to_value(&normalized).unwrap(),
        serde_json::to_value(group.members()).unwrap(),
        "both original/latest copies must match the whole normalized set, not one caller body"
    );
    let release:(Id,Id,String)=sqlx::query_as("SELECT execution,workflow,release_transaction FROM runtime_native_iteration_outcome_release WHERE snapshot=$1")
        .bind(snapshot).fetch_one(&mut *tx).await.unwrap();
    assert_eq!(
        release,
        (
            group.execution(),
            group.workflow(),
            group.transaction().to_owned()
        )
    );
    tx.commit().await.unwrap();

    let mut tx = db.begin().await.unwrap();
    Evidence::lock_reclamation(&mut tx).await.unwrap();
    for id in &ids {
        assert!(ExecutionRepository::delete(&mut *tx, *id).await.unwrap());
        assert!(Secrets::find_by_entity(&mut *tx, "execution_config", *id)
            .await
            .unwrap()
            .is_empty());
    }
    sqlx::query("DELETE FROM workflow_task_dispatch WHERE workflow_execution=$1")
        .bind(workflow.id)
        .execute(&mut *tx)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let directory = tempfile::tempdir().unwrap();
    let store = FilesystemBlobStore::new(directory.path().join("objects")).unwrap();
    // Actual initial file reservation occurs after BOTH raw configs and member
    // executions/dispatch have gone. No raw-current-child fallback can pass.
    let held = hold(&db, &fixture, &receipt, directory.path(), &store).await;
    let pinned:Vec<Id>=sqlx::query_scalar("SELECT source FROM file_evidence_pin WHERE owner=$1 AND kind='runtime_authority_admission' AND source=ANY($2) ORDER BY source")
        .bind(held.version).bind(&admissions).fetch_all(db.pool()).await.unwrap();
    assert_eq!(pinned, admissions);
    let raw:i64=sqlx::query_scalar("SELECT count(*) FROM file_evidence_pin WHERE owner=$1 AND kind='execution' AND source=ANY($2)")
        .bind(held.version).bind(&ids).fetch_one(db.pool()).await.unwrap();
    assert_eq!(raw, 0);
    let mut tx = db.begin().await.unwrap();
    Evidence::lock_reclamation(&mut tx).await.unwrap();
    for deletion in [
        "DELETE FROM runtime_native_iteration_outcome_member WHERE snapshot=$1",
        "DELETE FROM runtime_native_iteration_outcome_release WHERE snapshot=$1",
    ] {
        sqlx::query("SAVEPOINT group_archive_guard")
            .execute(&mut *tx)
            .await
            .unwrap();
        assert!(
            sqlx::query(deletion)
                .bind(snapshot)
                .execute(&mut *tx)
                .await
                .is_err(),
            "normalized observations and release cannot be removed while their archive exists"
        );
        sqlx::query("ROLLBACK TO SAVEPOINT group_archive_guard")
            .execute(&mut *tx)
            .await
            .unwrap();
        sqlx::query("RELEASE SAVEPOINT group_archive_guard")
            .execute(&mut *tx)
            .await
            .unwrap();
    }
    Snapshots::storage_evidence(&mut tx, snapshot)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    if revoke_member {
        let mut tx = db.begin().await.unwrap();
        Evidence::lock_reclamation(&mut tx).await.unwrap();
        RuntimeAuthorityRepository::revoke_admission(&mut tx, admissions[0])
            .await
            .unwrap();
        tx.commit().await.unwrap();
        let mut tx = db.begin().await.unwrap();
        assert!(
            Inputs::receipt_context(&mut tx, receipt.site(), &receipt.origin(), KEY)
                .await
                .is_err(),
            "each original observation fence remains independent; a sibling is not a grant union"
        );
        tx.rollback().await.unwrap();
    }
    sqlx::query("UPDATE file_evidence_owner SET evidence_state='pending_audit' WHERE version=$1")
        .bind(held.version)
        .execute(db.pool())
        .await
        .unwrap();
    assert!(
        Evidence::readmit(db.pool(), held.version).await.unwrap(),
        "keyless physical custody does not reacquire current reader rights"
    );
    let mut tx = db.begin().await.unwrap();
    Evidence::lock_reclamation(&mut tx).await.unwrap();
    assert!(
        !Evidence::eligible(&mut tx, "runtime_native_input_snapshot", snapshot)
            .await
            .unwrap()
    );
    assert_eq!(
        sqlx::query("DELETE FROM runtime_native_input_snapshot WHERE id=$1")
            .bind(snapshot)
            .execute(&mut *tx)
            .await
            .unwrap()
            .rows_affected(),
        0
    );
    assert_eq!(
        Evidence::request_cleanup(&mut tx, &[held.version])
            .await
            .unwrap()
            .len(),
        1
    );
    assert!(ArtifactVersionRepository::delete(&mut *tx, held.version)
        .await
        .unwrap());
    tx.commit().await.unwrap();
    store
        .delete(&held.object.key, &held.object.provider_version)
        .await
        .unwrap();
    let mut tx = db.begin().await.unwrap();
    Evidence::lock_reclamation(&mut tx).await.unwrap();
    Evidence::object_removed(
        &mut tx,
        held.object.key.as_str(),
        Some(held.object.provider_version.as_stored()),
    )
    .await
    .unwrap();
    assert!(
        !Evidence::eligible(&mut tx, "runtime_native_input_snapshot", snapshot)
            .await
            .unwrap()
    );
    tx.commit().await.unwrap();
    tokio::fs::remove_file(directory.path().join(&held.staging))
        .await
        .unwrap();
    let mut tx = db.begin().await.unwrap();
    Evidence::lock_reclamation(&mut tx).await.unwrap();
    Evidence::staging_removed(&mut tx, &held.staging, held.version)
        .await
        .unwrap();
    let pins: i64 = sqlx::query_scalar("SELECT count(*) FROM file_evidence_pin WHERE owner=$1")
        .bind(held.version)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    assert_eq!(pins, 0);
    assert!(ExecutionRepository::delete(&mut *tx, root.id)
        .await
        .unwrap());
    tx.commit().await.unwrap();
    drop(condition_receipt);
    for _ in 0..16 {
        PackRetentionRepository::collect(
            db.pool(),
            directory.path(),
            Utc::now() - Duration::days(30),
            1,
            1,
        )
        .await
        .unwrap();
    }
    let remaining:i64=sqlx::query_scalar("SELECT (SELECT count(*) FROM runtime_native_input_snapshot WHERE id=$1)+(SELECT count(*) FROM runtime_native_iteration_outcome_member WHERE snapshot=$1)+(SELECT count(*) FROM runtime_native_iteration_outcome_release WHERE snapshot=$1)")
        .bind(snapshot).fetch_one(db.pool()).await.unwrap();
    assert_eq!(
        remaining, 0,
        "one real archive owns normalized members and release through exact physical settlement"
    );
    db.cleanup().await.unwrap();
}
