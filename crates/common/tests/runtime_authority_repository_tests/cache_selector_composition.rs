//! Same-advance VALUE -> actual selector Control -> prospective cache activation.
use super::*;
use attune_common::config::CacheAdmissionConfig;
use attune_common::repositories::{
    cache::{
        CacheEntryInput, CacheEntryRepository, CacheGenerationRepository, CacheIngestRepository,
        CacheNamespacePolicy, CacheNamespaceRepository, CacheOwnerScope, CacheTransactionMode,
        CreateCacheGenerationInput, CreateCacheGenerationResult, CreateCacheNamespaceInput,
    },
    declared_input::DeclaredInputRepository,
    execution_runtime_admission::ExecutionAdmissionRepository as Admissions,
    execution_secret_value::ExecutionSecretValueRepository as Secrets,
    inherited_input::{
        InheritedInputRepository as Inputs, InputEntityKind, InputSite, InputSitePurpose,
    },
    native_input_snapshot::{CacheSelectorControl, NativeInputSnapshotRepository as Snapshots},
    recorded_input_authority::RecordedInputAuthorityRepository as Contributors,
    runtime_target::{ComponentTarget, RuntimeTarget},
    schema_contract::{BindingOwner, SchemaContractRepository},
    workflow::{
        CreateWorkflowDefinitionInput, CreateWorkflowExecutionInput, WorkflowDefinitionRepository,
        WorkflowExecutionRepository,
    },
    workflow_cache_iteration::WorkflowCacheIterationRepository as Iterations,
};

#[tokio::test]
async fn actual_selector_control_waits_for_prior_value_and_bound_cache_frontier() {
    selector_composition_case(false, false).await;
}

#[tokio::test]
async fn canonical_false_selector_is_control_not_a_failed_transition_condition() {
    selector_composition_case(true, false).await;
}

#[tokio::test]
async fn original_cache_deadline_survives_linked_eof_and_an_actual_target_wait() {
    selector_composition_case(false, true).await;
}

async fn selector_composition_case(boolean_selector: bool, expires_after_wait: bool) {
    let (db, fixture) = fixture().await;
    let owner = &fixture.policy.policy.owner;
    let key = NATIVE_CACHE_TEST_KEY;
    let supplied_namespace = if boolean_selector {
        json!(false)
    } else {
        json!("selector-cache")
    };
    let namespace_name = if boolean_selector {
        "false"
    } else {
        "selector-cache"
    };
    let graph_action = ActionFixture::new(owner.pack_id, &owner.pack_ref, "selector_graph")
        .create(&db)
        .await
        .unwrap();
    let leaf = ActionFixture::new(owner.pack_id, &owner.pack_ref, "selector_leaf")
        .create(&db)
        .await
        .unwrap();
    attune_common::repositories::action::ActionRepository::update(
        &db,
        graph_action.id,
        attune_common::repositories::action::UpdateActionInput {
            param_schema: Some(
                json!({"namespace":{"type":if boolean_selector {"boolean"}else {"string"}}}),
            ),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    attune_common::repositories::action::ActionRepository::update(
        &db,
        leaf.id,
        attune_common::repositories::action::UpdateActionInput {
            param_schema: Some(json!({"token":{"type":"string"}})),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let mut definition = json!({"tasks":[
        {"name":"publish","action":leaf.r#ref,"publish":{"selected_namespace":"{{ parameters.namespace }}"},"next":[{"do":["scan"]}]},
        {"name":"scan","action":leaf.r#ref,"iterate_cache":{"namespace":"{{ workflow.selected_namespace }}","page_size":1},"permission_set_refs":["standard"],"params":{"token":"{{ item.value.token }}"}}
    ]});
    if expires_after_wait {
        definition["tasks"][1]["iterate_cache"]["require_fresh"] = json!(true);
    }
    let graph = WorkflowDefinitionRepository::create(
        &db,
        CreateWorkflowDefinitionInput {
            r#ref: graph_action.r#ref.clone(),
            pack: owner.pack_id,
            pack_ref: owner.pack_ref.clone(),
            label: "Selector composition".into(),
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
    let mut setup = db.begin().await.unwrap();
    let release = activate_release(
        &mut setup,
        owner.pack_id,
        &owner.pack_ref,
        vec![graph_action.id, leaf.id],
        'e',
    )
    .await;
    let snapshots = ExecutableSnapshotRepository::resolve_release_actions(&mut setup, release.id)
        .await
        .unwrap();
    let root_binding = InvocationBinding::Action {
        action_ref: graph_action.r#ref.clone(),
    };
    let scan_binding = InvocationBinding::WorkflowTask {
        workflow_ref: graph_action.r#ref.clone(),
        task_path: "/tasks/scan".into(),
        action_ref: leaf.r#ref.clone(),
    };
    let publish_binding = InvocationBinding::WorkflowTask {
        workflow_ref: graph_action.r#ref.clone(),
        task_path: "/tasks/publish".into(),
        action_ref: leaf.r#ref.clone(),
    };
    let policy = RuntimePolicy {
        owner: owner.clone(),
        release_id: release.id,
        release_digest: release.digest.clone(),
        concrete_targets: BTreeMap::new(),
        invocations: vec![
            InvocationPolicy::from_executable(
                root_binding.clone(),
                &snapshots[&graph_action.r#ref],
                Some(&[]),
                &BTreeMap::new(),
            )
            .unwrap(),
            InvocationPolicy::from_executable(
                publish_binding,
                &snapshots[&leaf.r#ref],
                Some(&[]),
                &BTreeMap::new(),
            )
            .unwrap(),
            InvocationPolicy::from_executable(
                scan_binding.clone(),
                &snapshots[&leaf.r#ref],
                Some(&["standard".into()]),
                &BTreeMap::new(),
            )
            .unwrap(),
        ],
    };
    let policy = RuntimeAuthorityRepository::capture_policy(&mut setup, &policy)
        .await
        .unwrap();
    RuntimeAuthorityRepository::activate_policy(&mut setup, policy.id)
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
    let root = ExecutionRepository::create_pinned(
        &mut *setup,
        CreateExecutionInput {
            action: Some(graph_action.id),
            action_ref: graph_action.r#ref.clone(),
            status: ExecutionStatus::Running,
            executor: Some(fixture.human_id),
            config: Some(json!({"namespace":supplied_namespace})),
            permission_set_refs: vec![],
            ..Default::default()
        },
        &snapshots[&graph_action.r#ref],
    )
    .await
    .unwrap();
    let root_admission = RuntimeAuthorityRepository::record_admission(
        &mut setup,
        RuntimeOwner::Execution(root.id),
        &authority,
    )
    .await
    .unwrap();
    let schema = SchemaContractRepository::materialize_action_pair(
        &mut setup,
        &snapshots[&graph_action.r#ref],
    )
    .await
    .unwrap();
    SchemaContractRepository::attach(&mut setup, BindingOwner::Execution, root.id, &schema)
        .await
        .unwrap();
    Contributors::capture_human(&mut setup, "execution", root.id, fixture.human_id)
        .await
        .unwrap();
    let workflow = WorkflowExecutionRepository::create(
        &mut *setup,
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
    let child_authority = policy
        .policy
        .select(
            policy.id,
            &scan_binding,
            &["standard".into()],
            RequestAttribution::ParentExecution {
                execution_id: root.id,
            },
        )
        .unwrap();
    setup.commit().await.unwrap();
    let namespace = CacheNamespaceRepository::create_api_with_policy(
        &db,
        CreateCacheNamespaceInput {
            mode: Default::default(),
            owner: CacheOwnerScope::pack(owner.pack_id, Some(owner.pack_ref.clone())),
            namespace: namespace_name.into(),
            policy: if expires_after_wait {
                CacheNamespacePolicy {
                    freshness_target_seconds: 30,
                    ..Default::default()
                }
            } else {
                CacheNamespacePolicy::default()
            },
        },
        &CacheAdmissionConfig::default(),
    )
    .await
    .unwrap();
    let generation = CacheGenerationRepository::create_or_get(
        &db,
        &CreateCacheGenerationInput {
            namespace: namespace.id,
            client_refresh_id: "selector-composition".into(),
            expected_active_generation: None,
            expected_chunk_count: 1,
            expected_count: Some(1),
            expected_bytes: None,
            checksum_algorithm: None,
            checksum: None,
            source_revision: None,
            created_by: None,
            created_by_execution: None,
        },
    )
    .await
    .unwrap();
    let CreateCacheGenerationResult::Created(generation) = generation else {
        panic!("owned cache generation must be new")
    };
    CacheIngestRepository::insert_chunk(
        &db,
        generation.id,
        0,
        "selector-composition-chunk",
        &[CacheEntryInput {
            external_id: "one".into(),
            value: json!({"token":"selected-data"}),
            source_updated_at: None,
            source_checksum: None,
        }],
    )
    .await
    .unwrap();
    CacheGenerationRepository::seal(&db, generation.id)
        .await
        .unwrap();
    CacheGenerationRepository::promote(
        &db,
        namespace.id,
        generation.id,
        None,
        Utc::now() + Duration::hours(1),
    )
    .await
    .unwrap();

    let mut tx = db.begin().await.unwrap();
    CacheEntryRepository::protect_transaction(&mut tx, CacheTransactionMode::PinMutation)
        .await
        .unwrap();
    let task = Admissions::select(
        &mut tx,
        owner.pack_id,
        scan_binding,
        &snapshots[&leaf.r#ref],
        &["standard".into()],
        RequestAttribution::ParentExecution {
            execution_id: root.id,
        },
        RuntimeTarget::Component(ComponentTarget::Action, graph_action.id),
    )
    .await
    .unwrap();
    // A real admitted destination, not a guessed ID. Its immutable config, first
    // seal, source preparation and eventual publication share this transaction.
    let child=ExecutionRepository::create_pinned(&mut *tx,CreateExecutionInput {action:Some(leaf.id),action_ref:leaf.r#ref.clone(),parent:Some(root.id),status:ExecutionStatus::Requested,config:Some(json!({"token":attune_common::secret_values::redaction_marker()})),permission_set_refs:vec!["standard".into()],workflow_task:Some(serde_json::from_value(json!({"workflow_execution":workflow.id,"task_name":"scan","task_index":0,"retry_count":0,"max_retries":0,"timed_out":false})).unwrap()),..Default::default()},&snapshots[&leaf.r#ref]).await.unwrap();
    RuntimeAuthorityRepository::record_admission(
        &mut tx,
        RuntimeOwner::Execution(child.id),
        &child_authority,
    )
    .await
    .unwrap();
    Contributors::copy(&mut tx, "execution", root.id, "execution", child.id)
        .await
        .unwrap();
    let child_schema =
        SchemaContractRepository::materialize_action_pair(&mut tx, &snapshots[&leaf.r#ref])
            .await
            .unwrap();
    SchemaContractRepository::attach(&mut tx, BindingOwner::Execution, child.id, &child_schema)
        .await
        .unwrap();
    let declarations = DeclaredInputRepository::bindings(&mut tx, root.id)
        .await
        .unwrap();
    let publish = declarations
        .iter()
        .find(|decl| decl.expression == "{{ parameters.namespace }}")
        .unwrap();
    let selector = declarations
        .iter()
        .find(|decl| decl.source_path == "/workflow/tasks/1/iterate_cache/namespace")
        .unwrap();
    let params = declarations
        .iter()
        .find(|decl| decl.source_path == "/workflow/tasks/1/params/token")
        .unwrap();
    let mut value_session = Inputs::begin_render(
        &mut tx,
        root.id,
        &publish.declaration_revision,
        &publish.source_path,
        &publish.expression,
        InputSitePurpose::Value,
    )
    .await
    .unwrap();
    Inputs::bind_public_parameter(&mut tx, &mut value_session, "/namespace")
        .await
        .unwrap();
    let value = Inputs::evaluate(&mut tx, value_session).await.unwrap();
    let value_site = InputSite {
        execution: root.id,
        kind: InputEntityKind::WorkflowVariables,
        purpose: InputSitePurpose::Value,
        path: "/selected_namespace".into(),
    };
    let staged = Inputs::prepare_staged_value(&mut tx, &value, &value_site, key)
        .await
        .unwrap();
    let mut selector_session = Inputs::begin_render(
        &mut tx,
        root.id,
        &selector.declaration_revision,
        &selector.source_path,
        &selector.expression,
        InputSitePurpose::Control,
    )
    .await
    .unwrap();
    Inputs::bind_staged_value(
        &mut selector_session,
        "/workflow/selected_namespace",
        &staged,
    )
    .unwrap();
    let rendered_selector = Inputs::evaluate(&mut tx, selector_session).await.unwrap();
    assert_eq!(rendered_selector.value(), &supplied_namespace);
    let selector_site = InputSite {
        execution: root.id,
        kind: InputEntityKind::WorkflowVariables,
        purpose: InputSitePurpose::Control,
        path: selector.source_path.clone(),
    };
    assert!(
        Inputs::prepare_control_source(&mut tx, &value, &selector_site, key)
            .await
            .is_err(),
        "a VALUE render cannot be relabeled as a selector Control"
    );
    let ordinary = Inputs::prepare_destination(&mut tx, &rendered_selector, &selector_site, key)
        .await
        .unwrap();
    assert!(Inputs::record_prepared(
        &mut tx,
        &rendered_selector,
        &ordinary,
        rendered_selector.value(),
        rendered_selector.selections(),
        key
    )
    .await
    .is_err());
    for wrong in [
        InputSite {
            purpose: InputSitePurpose::Value,
            ..selector_site.clone()
        },
        InputSite {
            kind: InputEntityKind::ExecutionConfig,
            ..selector_site.clone()
        },
        InputSite {
            path: "/workflow/tasks/0/iterate_cache/namespace".into(),
            ..selector_site.clone()
        },
        InputSite {
            path: "/workflow/tasks/1/next/0/when".into(),
            ..selector_site.clone()
        },
    ] {
        assert!(
            Inputs::prepare_control_source(&mut tx, &rendered_selector, &wrong, key)
                .await
                .is_err()
        );
    }
    sqlx::query("SAVEPOINT selector_missing_authority")
        .execute(&mut *tx)
        .await
        .unwrap();
    RuntimeAuthorityRepository::revoke_admission(&mut tx, root_admission.id)
        .await
        .unwrap();
    assert!(
        Inputs::prepare_control_source(&mut tx, &rendered_selector, &selector_site, key)
            .await
            .is_err()
    );
    sqlx::query("ROLLBACK TO SAVEPOINT selector_missing_authority")
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("RELEASE SAVEPOINT selector_missing_authority")
        .execute(&mut *tx)
        .await
        .unwrap();
    let control = Inputs::prepare_control_source(&mut tx, &rendered_selector, &selector_site, key)
        .await
        .unwrap();
    assert!(
        Inputs::receipt_context(&mut tx, control.site(), &control.origin(), key)
            .await
            .is_err()
    );
    let prepared = Snapshots::prepare_cache_initial_from_controls(
        &mut tx,
        root.id,
        "/workflow/tasks/1",
        &task,
        workflow.id,
        65536,
        &[CacheSelectorControl::prepared(&control)],
        key,
    )
    .await
    .unwrap();
    assert_eq!(prepared.count(), 1);
    let pending_snapshot:Id=sqlx::query_scalar("SELECT id FROM runtime_native_input_snapshot WHERE renderer_admission=$1 ORDER BY id DESC LIMIT 1").bind(root_admission.id).fetch_one(&mut *tx).await.unwrap();
    assert!(Snapshots::storage_evidence(&mut tx, pending_snapshot)
        .await
        .is_err());
    assert!(
        Iterations::find_by_workflow_task(&mut *tx, workflow.id, "scan")
            .await
            .unwrap()
            .is_none()
    );
    let mut params_session = Inputs::begin_render(
        &mut tx,
        root.id,
        &params.declaration_revision,
        &params.source_path,
        &params.expression,
        InputSitePurpose::Value,
    )
    .await
    .unwrap();
    assert!(
        Inputs::bind_prepared(&mut params_session, "/spoof", &control, control.value()).is_err()
    );
    Inputs::bind_cache_input(&mut params_session, &prepared).unwrap();
    let rendered = Inputs::evaluate(&mut tx, params_session).await.unwrap();
    assert!(
        Inputs::prepare_staged_value(
            &mut tx,
            &rendered,
            &InputSite {
                execution: root.id,
                kind: InputEntityKind::WorkflowVariables,
                purpose: InputSitePurpose::Value,
                path: "/unbound_cache_is_not_a_wait_stage".into()
            },
            key
        )
        .await
        .is_err(),
        "private issued-wait staging must not release an unbound cache snapshot"
    );
    assert_eq!(rendered.value(), &json!("selected-data"));
    let destination = InputSite {
        execution: child.id,
        kind: InputEntityKind::ExecutionConfig,
        purpose: InputSitePurpose::Value,
        path: "/token".into(),
    };
    let receipt_destination = Inputs::prepare_destination(&mut tx, &rendered, &destination, key)
        .await
        .unwrap();
    assert!(Inputs::record_prepared(
        &mut tx,
        &rendered,
        &receipt_destination,
        rendered.value(),
        rendered.selections(),
        key
    )
    .await
    .is_err());
    let mut other = db.begin().await.unwrap();
    assert!(Snapshots::prepare_cache_initial_from_controls(
        &mut other,
        root.id,
        "/workflow/tasks/1",
        &task,
        workflow.id,
        65536,
        &[CacheSelectorControl::prepared(&control)],
        key
    )
    .await
    .is_err());
    other.rollback().await.unwrap();
    let pending_control_field = attune_common::secret_values::prepare_secret_values(
        vec![attune_common::secret_values::SecretValueInput {
            json_path: destination.path.clone(),
            value: control.value().clone(),
            source_kind: "provenance".into(),
            source_ref: Some(
                serde_json::to_string(&attune_common::secret_provenance::SecretProvenance {
                    origins: vec![control.origin()],
                    templates: vec![],
                })
                .unwrap(),
            ),
        }],
        key,
    )
    .unwrap();
    Secrets::upsert_many_with_conn(
        &mut tx,
        "execution_config",
        child.id,
        &pending_control_field,
    )
    .await
    .unwrap();
    assert!(
        attune_common::repositories::accepted_input::AcceptedInputRepository::seal_execution(
            &mut tx, child.id, key
        )
        .await
        .is_err(),
        "an issued pending selector Control cannot become accepted VALUE input"
    );

    if expires_after_wait {
        let deadline = prepared
            .fresh_until()
            .expect("required-fresh source has an original deadline");
        let eof = Snapshots::prepare_cache_next(&mut tx, &prepared, 65536, key)
            .await
            .unwrap();
        assert_eq!(eof.count(), 0);
        assert_eq!(eof.fresh_until(),Some(deadline),"EOF retains the original deadline despite its immutable frontier having no new acquisition deadline");
        prepared.ensure_fresh_on(&mut tx).await.unwrap();
        eof.ensure_fresh().unwrap();
        let create = prepared.initial_create_input().unwrap();
        let mut blocker = db.begin().await.unwrap();
        sqlx::query("SELECT id FROM workflow_execution WHERE id=$1 FOR UPDATE")
            .bind(workflow.id)
            .execute(&mut *blocker)
            .await
            .unwrap();
        // Wait for the actual authenticated expiry, not an arbitrary fixed sleep.
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
        assert!(prepared.ensure_fresh_on(&mut tx).await.is_err());
        assert!(prepared.ensure_fresh().is_err(),"B's pure final handoff check must include generation freshness, not only approval time");
        assert!(eof.ensure_fresh_on(&mut tx).await.is_err());
        assert!(eof.ensure_fresh().is_err());
        let iteration = Iterations::create(&mut *tx, create).await.unwrap();
        assert!(
            Snapshots::bind_cache_frontier(&mut tx, &prepared, iteration.id)
                .await
                .is_err()
        );
        let bound: Option<Id> = sqlx::query_scalar(
            "SELECT bound_iteration FROM runtime_native_cache_frontier WHERE snapshot=$1",
        )
        .bind(pending_snapshot)
        .fetch_one(&mut *tx)
        .await
        .unwrap();
        assert!(
            bound.is_none(),
            "expiry cannot publish a frontier or release its selector Control"
        );
        tx.rollback().await.unwrap();
        db.cleanup().await.unwrap();
        return;
    }
    // Outer target phase. Neither an iteration row nor its bound flag can stand
    // in for actual prior VALUE publication and finalization.
    let iteration = Iterations::create(&mut *tx, prepared.initial_create_input().unwrap())
        .await
        .unwrap();
    assert!(
        Snapshots::bind_cache_frontier(&mut tx, &prepared, iteration.id)
            .await
            .is_err()
    );
    sqlx::query("UPDATE workflow_execution SET variables=$2 WHERE id=$1")
        .bind(workflow.id)
        .bind(json!({"selected_namespace":attune_common::secret_values::redaction_marker()}))
        .execute(&mut *tx)
        .await
        .unwrap();
    let fields = attune_common::secret_values::prepare_secret_values(
        vec![attune_common::secret_values::SecretValueInput {
            json_path: value_site.path.clone(),
            value: staged.value().clone(),
            source_kind: "provenance".into(),
            source_ref: Some(serde_json::to_string(&staged.provenance()).unwrap()),
        }],
        key,
    )
    .unwrap();
    Secrets::upsert_many_with_conn(&mut tx, "workflow_variables", root.id, &fields)
        .await
        .unwrap();
    assert!(
        Snapshots::bind_cache_frontier(&mut tx, &prepared, iteration.id)
            .await
            .is_err(),
        "a write without actual VALUE finalization is still pending"
    );
    Inputs::finalize_staged_value(&mut tx, &staged, key)
        .await
        .unwrap();
    assert!(
        attune_common::repositories::accepted_input::AcceptedInputRepository::seal_execution(
            &mut tx, child.id, key
        )
        .await
        .is_err(),
        "VALUE finalization alone cannot release the selector Control or seal it as VALUE"
    );
    assert!(
        Inputs::receipt_context(&mut tx, control.site(), &control.origin(), key)
            .await
            .is_err(),
        "selector replay still awaits the actual bound cache frontier"
    );
    assert!(Inputs::record_prepared(
        &mut tx,
        &rendered,
        &receipt_destination,
        rendered.value(),
        rendered.selections(),
        key
    )
    .await
    .is_err());
    Snapshots::bind_cache_frontier(&mut tx, &prepared, iteration.id)
        .await
        .unwrap();
    let selector_replay = Inputs::receipt_context(&mut tx, control.site(), &control.origin(), key)
        .await
        .unwrap();
    assert_eq!(selector_replay.origin(), control.origin());
    let receipt = Inputs::record_prepared(
        &mut tx,
        &rendered,
        &receipt_destination,
        rendered.value(),
        rendered.selections(),
        key,
    )
    .await
    .unwrap();
    assert_eq!(receipt.site().execution, child.id);
    assert_eq!(receipt.native_source_metadata().len(), 1);
    assert!(
        Inputs::retention_parents(&mut tx, receipt.source_receipts()[0].receipt())
            .await
            .unwrap()
            .retained_bytes()
            > 0
    );
    let delivered = attune_common::secret_values::prepare_secret_values(
        vec![attune_common::secret_values::SecretValueInput {
            json_path: destination.path.clone(),
            value: rendered.value().clone(),
            source_kind: "provenance".into(),
            source_ref: Some(serde_json::to_string(&receipt.provenance()).unwrap()),
        }],
        key,
    )
    .unwrap();
    Secrets::upsert_many_with_conn(&mut tx, "execution_config", child.id, &delivered)
        .await
        .unwrap();
    attune_common::repositories::accepted_input::AcceptedInputRepository::seal_execution(
        &mut tx, child.id, key,
    )
    .await
    .unwrap();
    Iterations::update_scan_progress(&mut *tx,iteration.id,attune_common::repositories::workflow_cache_iteration::UpdateWorkflowCacheIterationProgressInput {
        last_external_id:prepared.after_external_id().unwrap().into(),next_batch_index:prepared.index()+1,scanned_count:i64::try_from(prepared.count()).unwrap(),dispatched_count:1,
    }).await.unwrap().unwrap();
    tx.commit().await.unwrap();
    db.cleanup().await.unwrap();
}
