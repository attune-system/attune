//! Actual admitted result projection, authored output and retained source replay.
use super::*;
use attune_common::repositories::{
    action::{ActionRepository, UpdateActionInput},
    declared_input::DeclaredInputRepository,
    execution_secret_value::ExecutionSecretValueRepository as Secrets,
    inherited_input::{
        InheritedInputRepository as Inputs, InputEntityKind, InputSite, InputSitePurpose,
        NativeResultStageBinding, NativeResultViewBinding,
    },
    native_input_snapshot::{NativeInputSnapshotRepository as Snapshots, NativeStorageParent},
    schema_contract::{BindingOwner, SchemaContractRepository as Schemas},
    workflow::{
        CreateWorkflowDefinitionInput, CreateWorkflowExecutionInput, WorkflowDefinitionRepository,
        WorkflowExecutionRepository,
    },
};
use attune_common::secret_provenance::SecretOrigin;
use serde_json::Value;

#[tokio::test]
async fn genuine_stdout_view_records_authored_output_and_replays_after_raw_child_gc() {
    result_view_case(false, false, false, false).await;
}

#[tokio::test]
async fn genuine_failure_metadata_has_native_recipe_and_retained_original_children() {
    result_view_case(true, false, false, false).await;
}

#[tokio::test]
async fn native_failure_and_result_view_expire_at_the_actual_last_workflow_row_wait() {
    result_view_case(true, true, false, false).await;
}

#[tokio::test]
async fn pending_value_to_with_items_control_must_publish_before_item_receipt_and_first_seal() {
    result_view_case(false, false, true, false).await;
}

#[tokio::test]
async fn pending_iteration_control_keeps_original_approval_deadline_across_actual_target_wait() {
    result_view_case(false, true, true, false).await;
}

#[tokio::test]
async fn prospective_result_conditions_and_whole_publication_require_each_real_release() {
    result_view_case(false, false, false, true).await;
}

async fn result_view_case(failure: bool, expires: bool, pending: bool, private_results: bool) {
    let (db, fixture) = if expires {
        fixture_with_approval_lifetime(None, None, Some(Duration::seconds(30))).await
    } else {
        fixture().await
    };
    let owner = &fixture.policy.policy.owner;
    let key = NATIVE_WAIT_TEST_KEY;
    let graph_action = ActionFixture::new(owner.pack_id, &owner.pack_ref, "result_view_graph")
        .create(&db)
        .await
        .unwrap();
    let leaf = ActionFixture::new(owner.pack_id, &owner.pack_ref, "result_view_leaf")
        .create(&db)
        .await
        .unwrap();
    ActionRepository::update(
        &db,
        leaf.id,
        UpdateActionInput {
            out_schema: Some(json!({"stdout":{"type":"string","secret":true}})),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let mut definition = json!({"tasks":[{"name":"source","action":leaf.r#ref,"publish":["whole"]}],
        "output_map":{"answer":"{{ result().token }}"}});
    if failure {
        definition["tasks"][0]["next"] = json!([{"when":"{{ result().token == 'never' }}"}]);
    }
    if private_results {
        definition["tasks"][0]
            .as_object_mut()
            .unwrap()
            .remove("publish");
        definition["tasks"][0]["next"] = json!([
            {"when":"{{ result().token != 'never' }}","publish":["whole"]},
            {"when":"{{ workflow.whole.token != 'never' }}"},
            {"when":"{{ workflow.whole.token == 'never' }}"},
            {"publish":[{"scalar":"{{ result().token }}"}],"do":[]}
        ]);
        definition["output_map"]["answer"] = json!("{{ workflow.whole.token }}");
    }
    if pending {
        ActionRepository::update(
            &db,
            graph_action.id,
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
        definition["tasks"][0]["publish"] = json!(["whole",{"chosen":"{{ parameters.items }}"}]);
        definition["tasks"].as_array_mut().unwrap().push(json!({"name":"iterate","action":leaf.r#ref,"with_items":"{{ workflow.chosen }}","batch_size":1,"params":{"item":"{{ item }}"}}));
    }
    let graph = WorkflowDefinitionRepository::create(
        &db,
        CreateWorkflowDefinitionInput {
            r#ref: graph_action.r#ref.clone(),
            pack: owner.pack_id,
            pack_ref: owner.pack_ref.clone(),
            label: "Result views".into(),
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
    ActionRepository::link_workflow_def(&db, graph_action.id, graph.id)
        .await
        .unwrap();
    let mut tx = db.begin().await.unwrap();
    let release = activate_release(
        &mut tx,
        owner.pack_id,
        &owner.pack_ref,
        vec![graph_action.id, leaf.id],
        'a',
    )
    .await;
    let snapshots = ExecutableSnapshotRepository::resolve_release_actions(&mut tx, release.id)
        .await
        .unwrap();
    let root_binding = InvocationBinding::Action {
        action_ref: graph_action.r#ref.clone(),
    };
    let permission_sets = fixture.policy.policy.invocations[0].permission_sets.clone();
    let root_refs = permission_sets.keys().cloned().collect::<Vec<_>>();
    let child_binding = InvocationBinding::WorkflowTask {
        workflow_ref: graph_action.r#ref.clone(),
        task_path: "/tasks/source".into(),
        action_ref: leaf.r#ref.clone(),
    };
    let mut policy = RuntimePolicy {
        owner: owner.clone(),
        release_id: release.id,
        release_digest: release.digest.clone(),
        concrete_targets: fixture.policy.policy.concrete_targets.clone(),
        invocations: vec![
            InvocationPolicy::from_executable(
                root_binding.clone(),
                &snapshots[&graph_action.r#ref],
                Some(&root_refs),
                &permission_sets,
            )
            .unwrap(),
            InvocationPolicy::from_executable(
                child_binding.clone(),
                &snapshots[&leaf.r#ref],
                Some(&[]),
                &BTreeMap::new(),
            )
            .unwrap(),
        ],
    };
    let iteration_binding = InvocationBinding::WorkflowTask {
        workflow_ref: graph_action.r#ref.clone(),
        task_path: "/tasks/iterate".into(),
        action_ref: leaf.r#ref.clone(),
    };
    if pending {
        policy.invocations.push(
            InvocationPolicy::from_executable(
                iteration_binding.clone(),
                &snapshots[&leaf.r#ref],
                Some(&[]),
                &BTreeMap::new(),
            )
            .unwrap(),
        );
    }
    let policy = RuntimeAuthorityRepository::capture_policy(&mut tx, &policy)
        .await
        .unwrap();
    RuntimeAuthorityRepository::activate_policy(&mut tx, policy.id)
        .await
        .unwrap();
    let root = ExecutionRepository::create_pinned(
        &mut *tx,
        CreateExecutionInput {
            action: Some(graph_action.id),
            action_ref: graph_action.r#ref.clone(),
            status: ExecutionStatus::Running,
            config: Some(if pending {
                json!({"items":["alpha","beta"]})
            } else {
                json!({})
            }),
            permission_set_refs: root_refs.clone(),
            ..Default::default()
        },
        &snapshots[&graph_action.r#ref],
    )
    .await
    .unwrap();
    let authority = policy
        .policy
        .select(
            policy.id,
            &root_binding,
            &root_refs,
            fixture.admission.authority.attribution.clone(),
        )
        .unwrap();
    RuntimeAuthorityRepository::record_admission(
        &mut tx,
        RuntimeOwner::Execution(root.id),
        &authority,
    )
    .await
    .unwrap();
    if pending {
        attune_common::repositories::recorded_input_authority::RecordedInputAuthorityRepository::capture_human(&mut tx,"execution",root.id,fixture.human_id).await.unwrap();
    }
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
    let child=ExecutionRepository::create_pinned(&mut *tx,CreateExecutionInput {
        action:Some(leaf.id),action_ref:leaf.r#ref.clone(),parent:Some(root.id),status:if failure {ExecutionStatus::Failed}else {ExecutionStatus::Completed},
        result:Some(json!({"stdout":attune_common::secret_values::redaction_marker()})),config:Some(json!({})),
        workflow_task:Some(serde_json::from_value(json!({"workflow_execution":workflow.id,"task_name":"source","retry_count":0,"max_retries":0,"timed_out":false})).unwrap()),
        permission_set_refs:vec![],..Default::default()
    },&snapshots[&leaf.r#ref]).await.unwrap();
    let authority = policy
        .policy
        .select(
            policy.id,
            &child_binding,
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
    for (id, snapshot) in [
        (root.id, &snapshots[&graph_action.r#ref]),
        (child.id, &snapshots[&leaf.r#ref]),
    ] {
        let binding = Schemas::materialize_action_pair(&mut tx, snapshot)
            .await
            .unwrap();
        Schemas::attach(&mut tx, BindingOwner::Execution, id, &binding)
            .await
            .unwrap();
    }
    let literal = "{{ keystore['never.evaluate.this'] }}";
    let fields = attune_common::secret_values::prepare_secret_values(
        vec![attune_common::secret_values::SecretValueInput {
            json_path: "/stdout".into(),
            value: json!(serde_json::to_string(&json!({"token":literal})).unwrap()),
            source_kind: "parameter_schema".into(),
            source_ref: Some("/stdout".into()),
        }],
        key,
    )
    .unwrap();
    Secrets::upsert_many_with_conn(&mut tx, "execution_result", child.id, &fields)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    let mut tx = db.begin().await.unwrap();
    if private_results {
        let (site, origin, snapshot) =
            pending_result_case(&db, &mut tx, &root, &workflow, child.id, key).await;
        tx.commit().await.unwrap();
        let mut replay = db.begin().await.unwrap();
        sqlx::query("DELETE FROM execution WHERE id=$1")
            .bind(child.id)
            .execute(&mut *replay)
            .await
            .unwrap();
        let retained = Inputs::receipt_context(&mut replay, &site, &origin, key)
            .await
            .unwrap();
        assert!(retained
            .native_source_metadata()
            .iter()
            .any(|source| source.snapshot() == snapshot));
        let view = Inputs::replay_native_result_view(&mut replay, root.id, snapshot, key)
            .await
            .unwrap();
        assert_eq!(view.value()["token"], json!(literal));
        Snapshots::storage_evidence(&mut replay, snapshot)
            .await
            .unwrap();
        replay.rollback().await.unwrap();
        db.cleanup().await.unwrap();
        return;
    }
    if pending {
        let control = pending_iteration_case(
            &db,
            &mut tx,
            &fixture,
            &policy,
            &snapshots[&leaf.r#ref],
            &root,
            &workflow,
            &iteration_binding,
            key,
        )
        .await;
        if let Some(control) = control {
            tx.commit().await.unwrap();
            control.ensure_fresh().unwrap();
        } else {
            tx.rollback().await.unwrap();
        }
        db.cleanup().await.unwrap();
        return;
    }
    if failure {
        use attune_common::repositories::native_failure_materialization::NativeFailureMaterializationRepository as Failures;
        let state = Failures::prepare_for_completion(&mut tx, child.id)
            .await
            .unwrap()
            .unwrap();
        let decision_view = Inputs::prepare_native_result_view(&mut tx, root.id, child.id, key)
            .await
            .unwrap();
        decision_view.finalize(&mut tx).await.unwrap();
        assert!(Inputs::prepare_native_result_shorthand(&mut tx,&decision_view,"/workflow/tasks/0/publish/0",&[],key).await.is_err(),"a failed child and ignored legacy directive cannot publish through absence of an authored when");
        let declarations = DeclaredInputRepository::bindings(&mut tx, root.id)
            .await
            .unwrap();
        let binding = declarations
            .iter()
            .find(|binding| binding.source_path == "/workflow/tasks/0/next/0/when")
            .unwrap();
        let mut session = Inputs::begin_render(
            &mut tx,
            root.id,
            &binding.declaration_revision,
            &binding.source_path,
            &binding.expression,
            InputSitePurpose::Condition,
        )
        .await
        .unwrap();
        Inputs::bind_native_result_view(
            &mut session,
            &decision_view,
            NativeResultViewBinding::CurrentAndTask,
        )
        .unwrap();
        let rendered = Inputs::evaluate(&mut tx, session).await.unwrap();
        assert_eq!(rendered.value(), &json!(false));
        let condition_site = InputSite {
            execution: root.id,
            kind: InputEntityKind::WorkflowVariables,
            purpose: InputSitePurpose::Condition,
            path: binding.source_path.clone(),
        };
        let decision = Inputs::prepare_condition_decision(&mut tx, &rendered, &condition_site, key)
            .await
            .unwrap();
        let prepared =
            Inputs::prepare_native_completion_metadata(&mut tx, &state, &[], &[&decision], key)
                .await
                .unwrap();
        let destination = Inputs::prepare_native_completion_destination(&mut tx, &prepared, key)
            .await
            .unwrap();
        if expires {
            let observed = Inputs::prepare_native_result_view(&mut tx, root.id, child.id, key)
                .await
                .unwrap();
            prepared.ensure_fresh_on(&mut tx).await.unwrap();
            observed.ensure_fresh().unwrap();
            let deadline = fixture
                .approval
                .expires_at
                .expect("fixture has real recorded approval expiry");
            let mut blocker = db.begin().await.unwrap();
            sqlx::query("SELECT id FROM workflow_execution WHERE id=$1 FOR UPDATE")
                .bind(workflow.id)
                .execute(&mut *blocker)
                .await
                .unwrap();
            let release = tokio::spawn(async move {
                sqlx::query("SELECT pg_sleep(GREATEST(0,EXTRACT(EPOCH FROM ($1::TIMESTAMPTZ-clock_timestamp()))))").bind(deadline).execute(&mut *blocker).await.unwrap();
                blocker.commit().await.unwrap();
            });
            sqlx::query("SELECT id FROM workflow_execution WHERE id=$1 FOR UPDATE")
                .bind(workflow.id)
                .execute(&mut *tx)
                .await
                .unwrap();
            release.await.unwrap();
            prepared
                .require_original_rows(state.parent(), state.workflow())
                .unwrap();
            assert!(prepared.ensure_fresh_on(&mut tx).await.is_err());
            assert!(prepared.ensure_fresh().is_err());
            assert!(destination.ensure_fresh().is_err());
            assert!(observed.ensure_fresh_on(&mut tx).await.is_err());
            assert!(observed.ensure_fresh().is_err());
            assert!(prepared
                .finalize(&mut tx, state.failed_tasks())
                .await
                .is_err());
            assert!(observed.finalize(&mut tx).await.is_err());
            let released: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM runtime_native_completion_release WHERE snapshot=$1)",
            )
            .bind(prepared.snapshot())
            .fetch_one(&mut *tx)
            .await
            .unwrap();
            assert!(
                !released,
                "last-wait expiry must not publish a native recipe"
            );
            tx.rollback().await.unwrap();
            db.cleanup().await.unwrap();
            return;
        }
        assert_eq!(prepared.value(), state.metadata());
        assert_eq!(prepared.value()["succeeded"], json!(false));
        assert!(
            prepared
                .finalize(&mut tx, state.failed_tasks())
                .await
                .is_err(),
            "owning original-row comparison is mandatory"
        );
        prepared
            .require_original_rows(state.parent(), state.workflow())
            .unwrap();
        let actual_failed: Vec<String> = sqlx::query_scalar(
            "UPDATE workflow_execution SET failed_tasks=$2 WHERE id=$1 RETURNING failed_tasks",
        )
        .bind(workflow.id)
        .bind(state.failed_tasks())
        .fetch_one(&mut *tx)
        .await
        .unwrap();
        assert!(prepared.finalize(&mut tx, &[]).await.is_err());
        prepared.finalize(&mut tx, &actual_failed).await.unwrap();
        let mut forged = prepared.value().clone();
        forged["error"] = json!("caller chosen summary");
        assert!(Inputs::record_native_completion_metadata(
            &mut tx,
            &prepared,
            &destination,
            &actual_failed,
            &forged,
            key
        )
        .await
        .is_err());
        let receipt = Inputs::record_native_completion_metadata(
            &mut tx,
            &prepared,
            &destination,
            &actual_failed,
            prepared.value(),
            key,
        )
        .await
        .unwrap();
        assert_eq!(prepared.secret_path_sources(&receipt).unwrap()[0].path, "");
        let metadata = receipt
            .native_source_metadata()
            .iter()
            .find(|metadata| metadata.snapshot() == prepared.snapshot())
            .unwrap()
            .clone();
        assert_eq!(metadata.source_kind(), "workflow_failure_metadata");
        let origin = receipt.origin();
        let site = receipt.site().clone();
        let snapshot = prepared.snapshot();
        let storage = Snapshots::storage_evidence(&mut tx, snapshot)
            .await
            .unwrap();
        assert!(storage.wait_release().is_none());
        assert!(storage.result_view_release().is_none());
        assert_eq!(
            storage.completion_release().unwrap().completed_execution(),
            child.id
        );
        assert_eq!(storage.completion_sources()[0].execution(), child.id);
        assert!(
            storage
                .receipt_edges()
                .iter()
                .any(|(_, site)| site == &condition_site),
            "the actual false Condition remains a CONTROL dependency"
        );
        assert_eq!(
            storage.completion_sources()[0].original_fields()[0].json_path,
            "/stdout"
        );
        tx.commit().await.unwrap();
        prepared.ensure_fresh().unwrap();
        destination.ensure_fresh().unwrap();
        let mut tx = db.begin().await.unwrap();
        sqlx::query("DELETE FROM execution WHERE id=$1")
            .bind(child.id)
            .execute(&mut *tx)
            .await
            .unwrap();
        let replay = Inputs::receipt_context(&mut tx, &site, &origin, key)
            .await
            .unwrap();
        assert!(replay.native_source_metadata().contains(&metadata));
        let evidence = Snapshots::source_evidence(&mut tx, &metadata, key)
            .await
            .unwrap();
        assert_eq!(evidence.completion_sources()[0].execution(), child.id);
        Snapshots::storage_evidence(&mut tx, snapshot)
            .await
            .unwrap();
        tx.rollback().await.unwrap();
        db.cleanup().await.unwrap();
        return;
    }
    let view = Inputs::prepare_native_result_view(&mut tx, root.id, child.id, key)
        .await
        .unwrap();
    let original_body: Value =
        sqlx::query_scalar("SELECT body FROM runtime_native_input_snapshot WHERE id=$1")
            .bind(view.snapshot())
            .fetch_one(&mut *tx)
            .await
            .unwrap();
    for wrong_contract in [true, false] {
        let mut corrupted = original_body.clone();
        if wrong_contract {
            corrupted["source"]["contract"] = Value::Null;
        } else {
            corrupted["source"]["fields"][0]["json_path"] = json!("/token");
        }
        let corrupt:Id=sqlx::query_scalar("INSERT INTO runtime_native_input_snapshot(renderer_admission,policy_revision,body,body_digest,encrypted_value,encryption_key_hash) SELECT renderer_admission,policy_revision,$2,$3,encrypted_value,encryption_key_hash FROM runtime_native_input_snapshot WHERE id=$1 RETURNING id").bind(view.snapshot()).bind(&corrupted).bind(fixture_digest(&corrupted)).fetch_one(&mut *tx).await.unwrap();
        assert!(Inputs::replay_native_result_view(&mut tx,root.id,corrupt,key).await.is_err(),"a recomputed outer digest must not bless a changed original source contract/field pointer");
    }
    assert_eq!(view.value()["token"], json!(literal));
    let mapping = view
        .mappings()
        .iter()
        .find(|mapping| mapping.view_path() == "/token")
        .unwrap();
    assert_eq!(mapping.source_path(), "/stdout");
    assert_eq!(mapping.decoded_path(), Some("/token"));
    let mut other = db.begin().await.unwrap();
    assert!(
        view.ensure_fresh_on(&mut other).await.is_err(),
        "opaque view must not cross transactions"
    );
    other.rollback().await.unwrap();
    let declarations = DeclaredInputRepository::bindings(&mut tx, root.id)
        .await
        .unwrap();
    let declaration = declarations
        .iter()
        .find(|declaration| declaration.source_path == "/workflow/output_map/answer")
        .unwrap();
    let mut session = Inputs::begin_render(
        &mut tx,
        root.id,
        &declaration.declaration_revision,
        &declaration.source_path,
        &declaration.expression,
        InputSitePurpose::Value,
    )
    .await
    .unwrap();
    Inputs::bind_native_result_view(&mut session, &view, NativeResultViewBinding::CurrentAndTask)
        .unwrap();
    let rendered = Inputs::evaluate(&mut tx, session).await.unwrap();
    assert_eq!(rendered.value(), &json!(literal));
    let site = InputSite {
        execution: root.id,
        kind: InputEntityKind::ExecutionResult,
        purpose: InputSitePurpose::Value,
        path: "/answer".into(),
    };
    let mut wrong = site.clone();
    wrong.path = "/not_the_authored_output".into();
    assert!(
        Inputs::prepare_native_value_destination(&mut tx, &view, &rendered, &wrong, key)
            .await
            .is_err()
    );
    let destination =
        Inputs::prepare_native_value_destination(&mut tx, &view, &rendered, &site, key)
            .await
            .unwrap();
    let shorthand = Inputs::prepare_native_result_shorthand(
        &mut tx,
        &view,
        "/workflow/tasks/0/publish/0",
        &[],
        key,
    )
    .await
    .unwrap();
    assert_eq!(shorthand.site().path, "/whole");
    assert_eq!(
        shorthand.value(),
        view.value(),
        "string publication publishes the entire canonical view, not result[name]"
    );
    assert!(
        Inputs::record_native_value(
            &mut tx,
            &view,
            &rendered,
            &destination,
            rendered.value(),
            key
        )
        .await
        .is_err(),
        "an unreleased observation cannot publish a receipt"
    );
    view.finalize(&mut tx).await.unwrap();
    assert!(
        Inputs::record_native_result_shorthand(&mut tx, &view, &shorthand, view.value(), key)
            .await
            .is_err(),
        "owning original workflow-row comparison is mandatory"
    );
    shorthand.require_original_workflow(&workflow).unwrap();
    assert!(Inputs::record_native_result_shorthand(
        &mut tx,
        &view,
        &shorthand,
        &view.value()["token"],
        key
    )
    .await
    .is_err());
    let whole =
        Inputs::record_native_result_shorthand(&mut tx, &view, &shorthand, view.value(), key)
            .await
            .unwrap();
    let whole_origin = whole.origin();
    let whole_site = whole.site().clone();
    sqlx::query("SAVEPOINT stale_output_target")
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("UPDATE execution SET result=$2 WHERE id=$1")
        .bind(root.id)
        .bind(json!({"changed":true}))
        .execute(&mut *tx)
        .await
        .unwrap();
    assert!(Inputs::record_native_value(
        &mut tx,
        &view,
        &rendered,
        &destination,
        rendered.value(),
        key
    )
    .await
    .is_err());
    sqlx::query("ROLLBACK TO SAVEPOINT stale_output_target")
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("RELEASE SAVEPOINT stale_output_target")
        .execute(&mut *tx)
        .await
        .unwrap();
    let receipt = Inputs::record_native_value(
        &mut tx,
        &view,
        &rendered,
        &destination,
        rendered.value(),
        key,
    )
    .await
    .unwrap();
    let origin = receipt.origin();
    let snapshot = view.snapshot();
    let metadata = receipt
        .native_source_metadata()
        .iter()
        .find(|metadata| metadata.snapshot() == snapshot)
        .unwrap()
        .clone();
    assert_eq!(metadata.source_kind(), "execution_result_view");
    let storage = Snapshots::storage_evidence(&mut tx, snapshot)
        .await
        .unwrap();
    assert_eq!(storage.result_view_release().unwrap().snapshot(), snapshot);
    assert!(storage.wait_release().is_none());
    assert!(!storage
        .physical_parents()
        .iter()
        .any(|parent| matches!(parent, NativeStorageParent::WorkflowTaskWait { .. })));
    tx.commit().await.unwrap();
    view.ensure_fresh().unwrap();
    destination.ensure_fresh().unwrap();

    let mut tx = db.begin().await.unwrap();
    sqlx::query("DELETE FROM execution WHERE id=$1")
        .bind(child.id)
        .execute(&mut *tx)
        .await
        .unwrap();
    let retained = Inputs::replay_native_result_view(&mut tx, root.id, snapshot, key)
        .await
        .unwrap();
    assert_eq!(retained.value()["token"], json!(literal));
    retained.require_current(&mut tx).await.unwrap();
    let replay = Inputs::receipt_context(&mut tx, &site, &origin, key)
        .await
        .unwrap();
    assert!(replay.native_source_metadata().contains(&metadata));
    let replay_whole = Inputs::receipt_context(&mut tx, &whole_site, &whole_origin, key)
        .await
        .unwrap();
    assert!(replay_whole.native_source_metadata().contains(&metadata));
    Snapshots::source_evidence(&mut tx, &metadata, key)
        .await
        .unwrap();
    Snapshots::storage_evidence(&mut tx, snapshot)
        .await
        .unwrap();
    tx.rollback().await.unwrap();
    db.cleanup().await.unwrap();
}

async fn pending_result_case(
    db: &TestDatabase,
    conn: &mut sqlx::PgConnection,
    root: &attune_common::models::Execution,
    workflow: &attune_common::models::WorkflowExecution,
    child: Id,
    key: &str,
) -> (InputSite, SecretOrigin, Id) {
    let view = Inputs::prepare_native_result_view(conn, root.id, child, key)
        .await
        .unwrap();
    let declarations = DeclaredInputRepository::bindings(conn, root.id)
        .await
        .unwrap();
    // B's ordinary completion path stages a transformed VALUE directly from
    // result(), without a shorthand/Condition that could incidentally carry the
    // view's private capability. Use the real unconditional authored leaf.
    sqlx::query("SAVEPOINT direct_result_value_case")
        .execute(&mut *conn)
        .await
        .unwrap();
    let scalar = declarations
        .iter()
        .find(|binding| binding.source_path == "/workflow/tasks/0/next/3/publish/0/scalar")
        .unwrap();
    let mut direct = Inputs::begin_render(
        conn,
        root.id,
        &scalar.declaration_revision,
        &scalar.source_path,
        &scalar.expression,
        InputSitePurpose::Value,
    )
    .await
    .unwrap();
    Inputs::bind_native_result_view(&mut direct, &view, NativeResultViewBinding::CurrentAndTask)
        .unwrap();
    let direct = Inputs::evaluate(conn, direct).await.unwrap();
    assert_eq!(
        direct.value(),
        &json!("{{ keystore['never.evaluate.this'] }}")
    );
    let scalar_site = InputSite {
        execution: root.id,
        kind: InputEntityKind::WorkflowVariables,
        purpose: InputSitePurpose::Value,
        path: "/scalar".into(),
    };
    let direct_destination = Inputs::prepare_destination(conn, &direct, &scalar_site, key)
        .await
        .unwrap();
    // Merely possessing a prospective read must never relax ordinary recording.
    assert!(Inputs::record_prepared(
        conn,
        &direct,
        &direct_destination,
        direct.value(),
        direct.selections(),
        key
    )
    .await
    .is_err());
    let staged_scalar = Inputs::prepare_staged_value(conn, &direct, &scalar_site, key)
        .await
        .expect(
            "an actual bound ResultView VALUE retains its own private read before source release",
        );
    assert_eq!(staged_scalar.value(), direct.value());
    assert!(Inputs::receipt_context(
        conn,
        &scalar_site,
        &staged_scalar.provenance().origins[0],
        key
    )
    .await
    .is_err());
    assert!(Inputs::finalize_staged_value(conn, &staged_scalar, key)
        .await
        .is_err());
    let mut other = db.begin().await.unwrap();
    assert!(staged_scalar.ensure_fresh_on(&mut other).await.is_err());
    other.rollback().await.unwrap();
    // This subcase deliberately tests pre-publication behavior only. Roll back
    // its own private receipt, leaving the owning view and remaining test intact.
    sqlx::query("ROLLBACK TO SAVEPOINT direct_result_value_case")
        .execute(&mut *conn)
        .await
        .unwrap();
    sqlx::query("RELEASE SAVEPOINT direct_result_value_case")
        .execute(&mut *conn)
        .await
        .unwrap();
    let mut conditions: Vec<
        attune_common::repositories::inherited_input::PreparedNativeResultCondition,
    > = Vec::new();
    let mut stage = None;
    for index in 0..3 {
        let path = format!("/workflow/tasks/0/next/{index}/when");
        let binding = declarations
            .iter()
            .find(|binding| binding.source_path == path)
            .unwrap();
        let mut session = Inputs::begin_render(
            conn,
            root.id,
            &binding.declaration_revision,
            &path,
            &binding.expression,
            InputSitePurpose::Condition,
        )
        .await
        .unwrap();
        if index == 0 {
            Inputs::bind_native_result_view(
                &mut session,
                &view,
                NativeResultViewBinding::CurrentAndTask,
            )
            .unwrap();
        } else {
            Inputs::bind_native_result_stage(
                &mut session,
                stage.as_ref().unwrap(),
                NativeResultStageBinding::Workflow,
            )
            .unwrap();
            Inputs::attach_prepared_control(&mut session, conditions[0].source()).unwrap();
        }
        let rendered = Inputs::evaluate(conn, session).await.unwrap();
        assert_eq!(rendered.value(), &json!(index != 2));
        let site = InputSite {
            execution: root.id,
            kind: InputEntityKind::WorkflowVariables,
            purpose: InputSitePurpose::Condition,
            path,
        };
        assert!(Inputs::record_render(
            conn,
            &rendered,
            &site,
            rendered.value(),
            rendered.selections(),
            key
        )
        .await
        .is_err());
        let condition = Inputs::prepare_native_result_condition(conn, &view, &rendered, &site, key)
            .await
            .unwrap();
        let mut wrong_site = site.clone();
        wrong_site.path = format!("/workflow/tasks/0/next/{}/when", (index + 1) % 3);
        assert!(
            Inputs::prepare_native_result_condition(conn, &view, &rendered, &wrong_site, key)
                .await
                .is_err()
        );
        assert!(
            Inputs::receipt_context(conn, &site, &condition.receipt().origin(), key)
                .await
                .is_err()
        );
        assert!(Inputs::finalize_native_result_condition(conn, &condition)
            .await
            .is_err());
        if index == 0 {
            let destination = Inputs::prepare_native_result_shorthand(
                conn,
                &view,
                "/workflow/tasks/0/next/0/publish/0",
                &[condition.source()],
                key,
            )
            .await
            .unwrap();
            let prospective = Inputs::prepare_native_result_stage(conn, &view, &destination, key)
                .await
                .unwrap();
            prospective.require_original_workflow(workflow).unwrap();
            let mut other = db.begin().await.unwrap();
            assert!(prospective.ensure_fresh_on(&mut other).await.is_err());
            other.rollback().await.unwrap();
            assert!(Inputs::receipt_context(
                conn,
                prospective.receipt().site(),
                &prospective.receipt().origin(),
                key
            )
            .await
            .is_err());
            stage = Some(prospective);
        }
        conditions.push(condition);
    }
    let stage = stage.unwrap();
    let output_binding = declarations
        .iter()
        .find(|binding| binding.source_path == "/workflow/output_map/answer")
        .unwrap();
    let mut session = Inputs::begin_render(
        conn,
        root.id,
        &output_binding.declaration_revision,
        &output_binding.source_path,
        &output_binding.expression,
        InputSitePurpose::Value,
    )
    .await
    .unwrap();
    Inputs::bind_native_result_stage(&mut session, &stage, NativeResultStageBinding::Workflow)
        .unwrap();
    assert!(Inputs::bind_prepared(
        &mut session,
        "/workflow/not_a_value",
        conditions[2].source(),
        &json!(false)
    )
    .is_err());
    for condition in &conditions {
        Inputs::attach_prepared_control(&mut session, condition.source()).unwrap();
    }
    let output = Inputs::evaluate(conn, session).await.unwrap();
    assert_eq!(
        output.value(),
        &json!("{{ keystore['never.evaluate.this'] }}")
    );
    let site = InputSite {
        execution: root.id,
        kind: InputEntityKind::ExecutionResult,
        purpose: InputSitePurpose::Value,
        path: "/answer".into(),
    };
    let destination = Inputs::prepare_destination(conn, &output, &site, key)
        .await
        .unwrap();
    view.finalize(conn).await.unwrap();
    assert!(Inputs::finalize_native_result_stage(conn, &stage, key)
        .await
        .is_err());
    Inputs::finalize_native_result_condition(conn, &conditions[0])
        .await
        .unwrap();
    // The actual release is insufficient without the actual published VALUE.
    assert!(
        Inputs::receipt_context(conn, stage.receipt().site(), &stage.receipt().origin(), key)
            .await
            .is_err()
    );
    assert!(
        Inputs::finalize_native_result_condition(conn, &conditions[1])
            .await
            .is_err()
    );
    let receipt = stage.receipt();
    let fields = attune_common::secret_values::prepare_secret_values(
        vec![attune_common::secret_values::SecretValueInput {
            json_path: "/whole".into(),
            value: json!({"token":"changed"}),
            source_kind: "provenance".into(),
            source_ref: Some(serde_json::to_string(&receipt.provenance()).unwrap()),
        }],
        key,
    )
    .unwrap();
    sqlx::query("UPDATE workflow_execution SET variables=$2 WHERE id=$1")
        .bind(workflow.id)
        .bind(json!({"whole":attune_common::secret_values::redaction_marker()}))
        .execute(&mut *conn)
        .await
        .unwrap();
    Secrets::upsert_many_with_conn(conn, "workflow_variables", root.id, &fields)
        .await
        .unwrap();
    assert!(Inputs::finalize_native_result_stage(conn, &stage, key)
        .await
        .is_err());
    let mut wrong_provenance = receipt.provenance();
    wrong_provenance.origins.push(receipt.origin());
    let wrong_fields = attune_common::secret_values::prepare_secret_values(
        vec![attune_common::secret_values::SecretValueInput {
            json_path: "/whole".into(),
            value: view.value().clone(),
            source_kind: "provenance".into(),
            source_ref: Some(serde_json::to_string(&wrong_provenance).unwrap()),
        }],
        key,
    )
    .unwrap();
    Secrets::upsert_many_with_conn(conn, "workflow_variables", root.id, &wrong_fields)
        .await
        .unwrap();
    assert!(Inputs::finalize_native_result_stage(conn, &stage, key)
        .await
        .is_err());
    let fields = attune_common::secret_values::prepare_secret_values(
        vec![attune_common::secret_values::SecretValueInput {
            json_path: "/whole".into(),
            value: view.value().clone(),
            source_kind: "provenance".into(),
            source_ref: Some(serde_json::to_string(&receipt.provenance()).unwrap()),
        }],
        key,
    )
    .unwrap();
    Secrets::upsert_many_with_conn(conn, "workflow_variables", root.id, &fields)
        .await
        .unwrap();
    Inputs::finalize_native_result_stage(conn, &stage, key)
        .await
        .unwrap();
    assert!(Inputs::record_prepared(
        conn,
        &output,
        &destination,
        output.value(),
        output.selections(),
        key
    )
    .await
    .is_err());
    Inputs::finalize_native_result_condition(conn, &conditions[1])
        .await
        .unwrap();
    // False still contributes a separate, mandatory released CONTROL.
    assert!(Inputs::record_prepared(
        conn,
        &output,
        &destination,
        output.value(),
        output.selections(),
        key
    )
    .await
    .is_err());
    Inputs::finalize_native_result_condition(conn, &conditions[2])
        .await
        .unwrap();
    let recorded = Inputs::record_prepared(
        conn,
        &output,
        &destination,
        output.value(),
        output.selections(),
        key,
    )
    .await
    .unwrap();
    let replayed = Inputs::receipt_context(conn, &site, &recorded.origin(), key)
        .await
        .unwrap();
    assert_eq!(
        replayed.native_source_metadata(),
        recorded.native_source_metadata()
    );
    assert!(!conditions[2].value());
    let sources: i64 =
        sqlx::query_scalar("SELECT count(*) FROM runtime_input_receipt_source WHERE receipt=$1")
            .bind(match recorded.origin() {
                SecretOrigin::Entity { entity_id, .. } => entity_id,
                _ => unreachable!(),
            })
            .fetch_one(&mut *conn)
            .await
            .unwrap();
    assert!(
        sources >= 4,
        "whole VALUE and all three Conditions must survive recording"
    );
    (site, recorded.origin(), view.snapshot())
}

async fn pending_iteration_case(
    db: &TestDatabase,
    conn: &mut sqlx::PgConnection,
    fixture: &Fixture,
    policy: &RuntimePolicyRevision,
    snapshot: &attune_common::models::ExecutionExecutableSnapshot,
    root: &attune_common::models::Execution,
    workflow: &attune_common::models::WorkflowExecution,
    invocation: &InvocationBinding,
    key: &str,
) -> Option<attune_common::repositories::inherited_input::PreparedIterationControl> {
    use attune_common::repositories::{accepted_input::AcceptedInputRepository, FindById};
    let declarations = DeclaredInputRepository::bindings(conn, root.id)
        .await
        .unwrap();
    let publish = declarations
        .iter()
        .find(|binding| binding.source_path == "/workflow/tasks/0/publish/1/chosen")
        .unwrap();
    let list = declarations
        .iter()
        .find(|binding| binding.source_path == "/workflow/tasks/1/with_items")
        .unwrap();
    let parameter = declarations
        .iter()
        .find(|binding| binding.source_path == "/workflow/tasks/1/params/item")
        .unwrap();
    let mut session = Inputs::begin_render(
        conn,
        root.id,
        &publish.declaration_revision,
        &publish.source_path,
        &publish.expression,
        InputSitePurpose::Value,
    )
    .await
    .unwrap();
    Inputs::bind_public_parameter(conn, &mut session, "/items")
        .await
        .unwrap();
    let rendered = Inputs::evaluate(conn, session).await.unwrap();
    let value_site = InputSite {
        execution: root.id,
        kind: InputEntityKind::WorkflowVariables,
        purpose: InputSitePurpose::Value,
        path: "/chosen".into(),
    };
    let staged = Inputs::prepare_staged_value(conn, &rendered, &value_site, key)
        .await
        .unwrap();
    let mut session = Inputs::begin_render(
        conn,
        root.id,
        &list.declaration_revision,
        &list.source_path,
        &list.expression,
        InputSitePurpose::Control,
    )
    .await
    .unwrap();
    Inputs::bind_staged_value(&mut session, "/workflow/chosen", &staged).unwrap();
    let rendered_list = Inputs::evaluate(conn, session).await.unwrap();
    let control = Inputs::prepare_iteration_control(conn, &rendered_list, key)
        .await
        .unwrap();
    assert_eq!(control.value(), rendered_list.value());
    assert_eq!(control.batch_size(), Some(1));
    assert_eq!(control.item_count(), 2);
    let item = Inputs::prepare_iteration_item_from_control(&control, 0).unwrap();
    assert_eq!(item.value(), &json!("alpha"));
    assert!(Inputs::prepare_iteration_item_from_control(&control, 2).is_err());
    let mut other = db.begin().await.unwrap();
    assert!(control.ensure_fresh_on(&mut other).await.is_err());
    other.rollback().await.unwrap();
    let mut session = Inputs::begin_render(
        conn,
        root.id,
        &parameter.declaration_revision,
        &parameter.source_path,
        &parameter.expression,
        InputSitePurpose::Value,
    )
    .await
    .unwrap();
    Inputs::bind_iteration_item(&mut session, &item).unwrap();
    let rendered = Inputs::evaluate(conn, session).await.unwrap();
    let child=ExecutionRepository::create_pinned(&mut *conn,CreateExecutionInput {
        action:Some(snapshot.executable.action.id),action_ref:snapshot.executable.action.r#ref.clone(),parent:Some(root.id),status:ExecutionStatus::Requested,
        config:Some(json!({"item":attune_common::secret_values::redaction_marker()})),permission_set_refs:vec![],
        workflow_task:Some(serde_json::from_value(json!({"workflow_execution":workflow.id,"task_name":"iterate","task_index":0,"retry_count":0,"max_retries":0,"timed_out":false})).unwrap()),..Default::default()
    },snapshot).await.unwrap();
    let authority = policy
        .policy
        .select(
            policy.id,
            invocation,
            &[],
            RequestAttribution::ParentExecution {
                execution_id: root.id,
            },
        )
        .unwrap();
    RuntimeAuthorityRepository::record_admission(
        conn,
        RuntimeOwner::Execution(child.id),
        &authority,
    )
    .await
    .unwrap();
    attune_common::repositories::recorded_input_authority::RecordedInputAuthorityRepository::copy(
        conn,
        "execution",
        root.id,
        "execution",
        child.id,
    )
    .await
    .unwrap();
    let schema = Schemas::materialize_action_pair(conn, snapshot)
        .await
        .unwrap();
    Schemas::attach(conn, BindingOwner::Execution, child.id, &schema)
        .await
        .unwrap();
    let site = InputSite {
        execution: child.id,
        kind: InputEntityKind::ExecutionConfig,
        purpose: InputSitePurpose::Value,
        path: "/item".into(),
    };
    let destination = Inputs::prepare_destination(conn, &rendered, &site, key)
        .await
        .unwrap();
    if let Some(deadline) = fixture.approval.expires_at {
        let mut blocker = db.begin().await.unwrap();
        sqlx::query("SELECT id FROM workflow_execution WHERE id=$1 FOR UPDATE")
            .bind(workflow.id)
            .execute(&mut *blocker)
            .await
            .unwrap();
        let release = tokio::spawn(async move {
            sqlx::query("SELECT pg_sleep(GREATEST(0,EXTRACT(EPOCH FROM ($1::TIMESTAMPTZ-clock_timestamp()))))").bind(deadline).execute(&mut *blocker).await.unwrap();
            blocker.commit().await.unwrap();
        });
        sqlx::query("SELECT id FROM workflow_execution WHERE id=$1 FOR UPDATE")
            .bind(workflow.id)
            .execute(&mut *conn)
            .await
            .unwrap();
        release.await.unwrap();
        control.require_original_workflow(workflow).unwrap();
        assert!(control.ensure_fresh_on(conn).await.is_err());
        assert!(control.ensure_fresh().is_err());
        assert!(control.finalize(conn).await.is_err());
        let published: Option<String> = sqlx::query_scalar(
            "SELECT publication_transaction FROM runtime_native_iteration_control WHERE receipt=$1",
        )
        .bind(match control.origin() {
            attune_common::secret_provenance::SecretOrigin::Entity { entity_id, .. } => entity_id,
            _ => unreachable!(),
        })
        .fetch_one(&mut *conn)
        .await
        .unwrap();
        assert!(published.is_none());
        return None;
    }
    assert!(Inputs::record_prepared(
        conn,
        &rendered,
        &destination,
        rendered.value(),
        rendered.selections(),
        key
    )
    .await
    .is_err());
    assert!(
        Inputs::receipt_context(conn, control.site(), &control.origin(), key)
            .await
            .is_err(),
        "public replay cannot import pending VALUE ancestry"
    );
    assert!(
        control.finalize(conn).await.is_err(),
        "original target-row guard is mandatory"
    );
    let actual = WorkflowExecutionRepository::find_by_id(&mut *conn, workflow.id)
        .await
        .unwrap()
        .unwrap();
    control.require_original_workflow(&actual).unwrap();
    assert!(
        control.finalize(conn).await.is_err(),
        "upstream VALUE is not yet published"
    );
    sqlx::query("UPDATE workflow_execution SET variables=$2 WHERE id=$1")
        .bind(workflow.id)
        .bind(json!({"chosen":attune_common::secret_values::redaction_marker()}))
        .execute(&mut *conn)
        .await
        .unwrap();
    let fields = attune_common::secret_values::prepare_secret_values(
        vec![attune_common::secret_values::SecretValueInput {
            json_path: "/chosen".into(),
            value: staged.value().clone(),
            source_kind: "provenance".into(),
            source_ref: Some(serde_json::to_string(&staged.provenance()).unwrap()),
        }],
        key,
    )
    .unwrap();
    Secrets::upsert_many_with_conn(conn, "workflow_variables", root.id, &fields)
        .await
        .unwrap();
    Inputs::finalize_staged_value(conn, &staged, key)
        .await
        .unwrap();
    assert!(
        Inputs::record_prepared(
            conn,
            &rendered,
            &destination,
            rendered.value(),
            rendered.selections(),
            key
        )
        .await
        .is_err(),
        "VALUE publication alone does not release ordinary Control"
    );
    control.finalize(conn).await.unwrap();
    let receipt = Inputs::record_prepared(
        conn,
        &rendered,
        &destination,
        rendered.value(),
        rendered.selections(),
        key,
    )
    .await
    .unwrap();
    let fields = attune_common::secret_values::prepare_secret_values(
        vec![attune_common::secret_values::SecretValueInput {
            json_path: "/item".into(),
            value: rendered.value().clone(),
            source_kind: "provenance".into(),
            source_ref: Some(serde_json::to_string(&receipt.provenance()).unwrap()),
        }],
        key,
    )
    .unwrap();
    Secrets::upsert_many_with_conn(conn, "execution_config", child.id, &fields)
        .await
        .unwrap();
    AcceptedInputRepository::seal_execution(conn, child.id, key)
        .await
        .unwrap();
    Inputs::receipt_context(conn, control.site(), &control.origin(), key)
        .await
        .unwrap();
    let replay = Inputs::receipt_context(conn, &site, &receipt.origin(), key)
        .await
        .unwrap();
    replay.ensure_fresh().unwrap();
    control.ensure_fresh().unwrap();
    Some(control)
}
