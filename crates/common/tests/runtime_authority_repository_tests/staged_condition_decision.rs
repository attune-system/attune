use super::*;
use attune_common::repositories::{
    declared_input::DeclaredInputRepository,
    execution_secret_value::ExecutionSecretValueRepository as Secrets,
    inherited_input::{
        InheritedInputRepository as Inputs, InputEntityKind, InputSite, InputSitePurpose,
    },
    key::KeyRepository,
    workflow::{
        CreateWorkflowDefinitionInput, CreateWorkflowExecutionInput, WorkflowDefinitionRepository,
        WorkflowExecutionRepository,
    },
};

#[tokio::test]
async fn false_staged_decision_skips_unselected_key_and_controls_unchanged_output() {
    decision_case(false, None, true).await;
}

#[tokio::test]
async fn general_staged_decision_also_retains_true_without_weakening_branch_constructor() {
    decision_case(true, None, true).await;
}

#[tokio::test]
async fn standalone_false_decision_stays_fresh_after_parent_publication_without_output() {
    decision_case(false, None, false).await;
}

#[tokio::test]
async fn standalone_false_decision_retains_both_original_deadlines_across_target_wait() {
    for original in 0..2 {
        decision_case(false, Some(original), false).await;
    }
}

async fn decision_case(expected: bool, expiring_original: Option<usize>, with_output: bool) {
    let (db, fixture) = fixture().await;
    let owner = &fixture.policy.policy.owner;
    let key = "staged-decision-test-owned-encryption-key";
    let original = if expected { "different" } else { "original" };
    let source_key = helpers::KeyFixture::new_system_unique("decision_source", original)
        .create(&db)
        .await
        .unwrap();
    let gate_key = helpers::KeyFixture::new_system_unique("decision_gate", "yes")
        .create(&db)
        .await
        .unwrap();
    let never_selected = "system.staged_decision_never_selected";
    assert!(KeyRepository::find_by_ref(&*db, never_selected)
        .await
        .unwrap()
        .is_none());
    let graph_action = ActionFixture::new(owner.pack_id, &owner.pack_ref, "decision_graph")
        .create(&db)
        .await
        .unwrap();
    let leaf = ActionFixture::new(owner.pack_id, &owner.pack_ref, "decision_leaf")
        .create(&db)
        .await
        .unwrap();
    attune_common::repositories::action::ActionRepository::update(
        &db,
        graph_action.id,
        attune_common::repositories::action::UpdateActionInput {
            param_schema: Some(json!({"source":{"type":"string"},"gate":{"type":"string"}})),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let source_expression = "{{ keystore[parameters.source] }}";
    let condition_expression = "{{ keystore[parameters.gate] and first == 'different' }}";
    let output_schema = json!({"token":{"type":"string","default":"{{ first }}"}});
    let definition = json!({"tasks":[{"name":"publish","action":leaf.r#ref,"publish":{"first":source_expression},"next":[{"when":condition_expression,"publish":{"second":"{{ keystore['system.staged_decision_never_selected'] }}"}},{"when":"{{ parameters.gate }}"}]}],"output":output_schema});
    let graph = WorkflowDefinitionRepository::create(
        &db,
        CreateWorkflowDefinitionInput {
            r#ref: graph_action.r#ref.clone(),
            pack: owner.pack_id,
            pack_ref: owner.pack_ref.clone(),
            label: "Retained decision".into(),
            description: None,
            version: "1.0.0".into(),
            param_schema: None,
            out_schema: Some(output_schema),
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
    let release = activate_release(
        &mut tx,
        owner.pack_id,
        &owner.pack_ref,
        vec![graph_action.id, leaf.id],
        'd',
    )
    .await;
    let snapshots = ExecutableSnapshotRepository::resolve_release_actions(&mut tx, release.id)
        .await
        .unwrap();
    let snapshot = &snapshots[&graph_action.r#ref];
    let binding = InvocationBinding::Action {
        action_ref: graph_action.r#ref.clone(),
    };
    let policy = RuntimePolicy {
        owner: owner.clone(),
        release_id: release.id,
        release_digest: release.digest.clone(),
        concrete_targets: BTreeMap::new(),
        invocations: vec![InvocationPolicy::from_executable(
            binding.clone(),
            snapshot,
            Some(&[]),
            &BTreeMap::new(),
        )
        .unwrap()],
    };
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
            executor: Some(fixture.human_id),
            config: Some(json!({"source":source_key.r#ref,"gate":gate_key.r#ref})),
            status: ExecutionStatus::Running,
            permission_set_refs: vec![],
            ..Default::default()
        },
        snapshot,
    )
    .await
    .unwrap();
    let authority = policy
        .policy
        .select(
            policy.id,
            &binding,
            &[],
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
    let schema = attune_common::repositories::schema_contract::SchemaContractRepository::materialize_action_pair(&mut tx, snapshot).await.unwrap();
    attune_common::repositories::schema_contract::SchemaContractRepository::attach(
        &mut tx,
        attune_common::repositories::schema_contract::BindingOwner::Execution,
        root.id,
        &schema,
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
    tx.commit().await.unwrap();

    let mut tx = db.begin().await.unwrap();
    let declarations = DeclaredInputRepository::bindings(&mut tx, root.id)
        .await
        .unwrap();
    let declaration = |expression: &str| {
        declarations
            .iter()
            .find(|decl| decl.expression == expression)
            .unwrap()
    };
    let source_binding = declaration(source_expression);
    let condition_binding = declaration(condition_expression);
    assert_eq!(
        declaration("{{ first }}").source_path,
        "/workflow/output/token/default"
    );
    let expiry = Utc::now() + Duration::seconds(10);
    let mut approvals = Vec::new();
    for (index, (decl, source)) in [
        (source_binding, &source_key),
        (condition_binding, &gate_key),
    ]
    .into_iter()
    .enumerate()
    {
        let grants = serde_json::from_value(json!([{"resource":"keys","actions":["read","decrypt"],"constraints":{"refs":[source.r#ref]}}])).unwrap();
        approvals.push(
            Inputs::approve_external_input(
                &mut tx,
                root.id,
                &decl.declaration_revision,
                &decl.source_path,
                &decl.expression,
                fixture.human_id,
                grants,
                Some(if expiring_original == Some(index) {
                    expiry
                } else {
                    Utc::now() + Duration::hours(1)
                }),
            )
            .await
            .unwrap(),
        );
    }
    assert_ne!(approvals[0], approvals[1]);
    let mut session = Inputs::begin_render(
        &mut tx,
        root.id,
        &source_binding.declaration_revision,
        &source_binding.source_path,
        source_expression,
        InputSitePurpose::Value,
    )
    .await
    .unwrap();
    Inputs::bind_public_parameter(&mut tx, &mut session, "/source")
        .await
        .unwrap();
    let rendered = Inputs::evaluate_with_keys(&mut tx, session, key)
        .await
        .unwrap();
    let variable_site = InputSite {
        execution: root.id,
        kind: InputEntityKind::WorkflowVariables,
        purpose: InputSitePurpose::Value,
        path: "/first".into(),
    };
    let first = Inputs::prepare_staged_value(&mut tx, &rendered, &variable_site, key)
        .await
        .unwrap();
    let mut session = Inputs::begin_render(
        &mut tx,
        root.id,
        &condition_binding.declaration_revision,
        &condition_binding.source_path,
        condition_expression,
        InputSitePurpose::Condition,
    )
    .await
    .unwrap();
    Inputs::bind_staged_value(&mut session, "/first", &first).unwrap();
    Inputs::bind_public_parameter(&mut tx, &mut session, "/gate")
        .await
        .unwrap();
    let condition = Inputs::evaluate_condition_with_keys(&mut tx, session, key)
        .await
        .unwrap();
    assert_eq!(condition.value(), &json!(expected));
    let condition_site = InputSite {
        purpose: InputSitePurpose::Condition,
        path: condition_binding.source_path.clone(),
        ..variable_site.clone()
    };
    if !expected {
        assert!(
            Inputs::prepare_condition_control(&mut tx, &condition, &condition_site, key)
                .await
                .is_err(),
            "the successful-branch constructor stays true-only"
        );
    }
    assert!(
        Inputs::prepare_staged_value(&mut tx, &condition, &variable_site, key)
            .await
            .is_err()
    );
    let wrong_site = InputSite {
        path: "/workflow/tasks/0/next/1/when".into(),
        ..condition_site.clone()
    };
    let raw_binding = declaration("{{ parameters.gate }}");
    let mut raw_session = Inputs::begin_render(
        &mut tx,
        root.id,
        &raw_binding.declaration_revision,
        &raw_binding.source_path,
        &raw_binding.expression,
        InputSitePurpose::Condition,
    )
    .await
    .unwrap();
    Inputs::bind_public_parameter(&mut tx, &mut raw_session, "/gate")
        .await
        .unwrap();
    let raw_condition = Inputs::evaluate(&mut tx, raw_session).await.unwrap();
    assert!(raw_condition.value().is_string());
    assert!(
        Inputs::prepare_condition_decision(&mut tx, &raw_condition, &wrong_site, key)
            .await
            .is_err(),
        "a truthy non-Boolean Condition render is not a canonical decision"
    );
    assert!(
        Inputs::prepare_condition_decision(&mut tx, &condition, &wrong_site, key)
            .await
            .is_err()
    );
    assert!(
        Inputs::prepare_condition_decision(&mut tx, &rendered, &condition_site, key)
            .await
            .is_err(),
        "a Value render cannot be relabeled as a decision"
    );
    let decision = Inputs::prepare_condition_decision(&mut tx, &condition, &condition_site, key)
        .await
        .unwrap();
    assert_eq!(decision.value(), &json!(expected));
    decision.ensure_fresh().unwrap();
    decision.ensure_fresh_on(&mut tx).await.unwrap();
    let prepared_condition = Inputs::prepare_destination(&mut tx, &condition, &condition_site, key)
        .await
        .unwrap();
    assert!(
        Inputs::record_prepared(
            &mut tx,
            &condition,
            &prepared_condition,
            condition.value(),
            condition.selections(),
            key,
        )
        .await
        .is_err(),
        "even a retained false decision is not public while its VALUE parent is pending"
    );
    assert!(
        Inputs::receipt_context(&mut tx, decision.site(), &decision.origin(), key)
            .await
            .is_err()
    );
    assert!(
        Inputs::delivery_allowed(&mut tx, decision.site(), &decision.origin(), key)
            .await
            .is_err()
    );

    // Never begin/evaluate the authored second publication. Its missing Key is
    // not a selected source. Output instead consumes the unchanged first stage.
    let output = if with_output {
        let decl = declaration("{{ first }}");
        let mut session = Inputs::begin_render(
            &mut tx,
            root.id,
            &decl.declaration_revision,
            &decl.source_path,
            &decl.expression,
            InputSitePurpose::Value,
        )
        .await
        .unwrap();
        Inputs::bind_staged_value(&mut session, "/first", &first).unwrap();
        assert!(Inputs::bind_prepared(
            &mut session,
            "/decision_as_value",
            &decision,
            decision.value()
        )
        .is_err());
        Inputs::attach_prepared_control(&mut session, &decision).unwrap();
        let output = Inputs::evaluate_with_keys(&mut tx, session, key)
            .await
            .unwrap();
        assert_eq!(output.value(), &json!(original));
        assert_eq!(output.selections().len(), 2);
        let site = InputSite {
            kind: InputEntityKind::ExecutionResult,
            path: "/token".into(),
            ..variable_site.clone()
        };
        let prepared = Inputs::prepare_destination(&mut tx, &output, &site, key)
            .await
            .unwrap();
        assert!(Inputs::record_prepared(
            &mut tx,
            &output,
            &prepared,
            output.value(),
            output.selections(),
            key
        )
        .await
        .is_err());
        Some((output, prepared, site))
    } else {
        None
    };

    // No output/render carries the decision in expiry cases. The caller holds
    // only the official opaque token through this real target-row lock wait.
    let release_blocker = if expiring_original.is_some() {
        assert!(output.is_none());
        let mut blocker = db.begin().await.unwrap();
        sqlx::query("SELECT id FROM workflow_execution WHERE id=$1 FOR UPDATE")
            .bind(workflow.id)
            .fetch_one(&mut *blocker)
            .await
            .unwrap();
        let blocker_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *blocker)
            .await
            .unwrap();
        let waiter_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *tx)
            .await
            .unwrap();
        let pool = (*db).clone();
        Some(tokio::spawn(async move {
            tokio::time::timeout(std::time::Duration::from_secs(60), async {
                let mut observed_wait = false;
                loop {
                    let blocked: bool = sqlx::query_scalar("SELECT $2 = ANY(pg_blocking_pids($1))")
                        .bind(waiter_pid)
                        .bind(blocker_pid)
                        .fetch_one(&pool)
                        .await
                        .unwrap();
                    let expired: bool = sqlx::query_scalar("SELECT clock_timestamp() >= $1")
                        .bind(expiry)
                        .fetch_one(&pool)
                        .await
                        .unwrap();
                    if blocked && !observed_wait {
                        assert!(
                            !expired,
                            "approval expired before the observed target-wait barrier"
                        );
                        observed_wait = true;
                    }
                    if observed_wait && blocked && expired {
                        break;
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                }
            })
            .await
            .unwrap();
            blocker.rollback().await.unwrap();
        }))
    } else {
        None
    };
    tokio::time::timeout(
        std::time::Duration::from_secs(70),
        sqlx::query("UPDATE workflow_execution SET variables=$2 WHERE id=$1")
            .bind(workflow.id)
            .bind(json!({"first":attune_common::secret_values::redaction_marker()}))
            .execute(&mut *tx),
    )
    .await
    .unwrap()
    .unwrap();
    if let Some(blocker) = release_blocker {
        blocker.await.unwrap();
    }
    let fields = attune_common::secret_values::prepare_secret_values(
        vec![attune_common::secret_values::SecretValueInput {
            json_path: "/first".into(),
            value: first.value().clone(),
            source_kind: "provenance".into(),
            source_ref: Some(serde_json::to_string(&first.provenance()).unwrap()),
        }],
        key,
    )
    .unwrap();
    Secrets::upsert_many_with_conn(&mut tx, "workflow_variables", root.id, &fields)
        .await
        .unwrap();
    if let Some(original) = expiring_original {
        assert_eq!(first.ensure_fresh_on(&mut tx).await.is_err(), original == 0);
        assert!(
            decision.ensure_fresh().is_err(),
            "standalone synchronous check retains either original expiry"
        );
        assert!(
            decision.ensure_fresh_on(&mut tx).await.is_err(),
            "standalone same-TX check does not reopen receipts or need an output carrier"
        );
        tx.rollback().await.unwrap();
        db.cleanup().await.unwrap();
        return;
    }
    Inputs::finalize_staged_value(&mut tx, &first, key)
        .await
        .unwrap();
    let replay = Inputs::receipt_context(&mut tx, decision.site(), &decision.origin(), key)
        .await
        .unwrap();
    assert_eq!(
        replay.origin(),
        decision.origin(),
        "the same recorded decision becomes replayable, not a second evaluation"
    );
    let recorded_output = if let Some((output, prepared, site)) = output {
        let receipt = Inputs::record_prepared(
            &mut tx,
            &output,
            &prepared,
            output.value(),
            output.selections(),
            key,
        )
        .await
        .unwrap();
        assert_eq!(receipt.source_receipts().len(), 2);
        assert!(receipt.source_origins().iter().any(|origin| matches!(origin, attune_common::secret_provenance::SecretOrigin::Key(key) if key.key_id == source_key.id)));
        assert!(receipt.source_origins().iter().any(|origin| matches!(origin, attune_common::secret_provenance::SecretOrigin::Key(key) if key.key_id == gate_key.id)));
        Some((receipt, site))
    } else {
        None
    };
    decision.ensure_fresh_on(&mut tx).await.unwrap();
    tx.commit().await.unwrap();
    decision.ensure_fresh().unwrap();
    let mut tx = db.begin().await.unwrap();
    assert!(
        decision.ensure_fresh_on(&mut tx).await.is_err(),
        "held decisions cannot cross transactions"
    );
    let replayed =
        Inputs::prepare_source(&mut tx, root.id, decision.site(), &decision.origin(), key)
            .await
            .unwrap();
    assert_eq!(
        replayed.value(),
        &json!(expected),
        "immutable replay preserves false as false"
    );
    assert_eq!(replayed.origin(), decision.origin());
    replayed.ensure_fresh_on(&mut tx).await.unwrap();
    if let Some((receipt, site)) = recorded_output {
        assert!(
            Inputs::receipt_context(&mut tx, &site, &receipt.origin(), key)
                .await
                .is_ok()
        );
    }
    assert!(KeyRepository::find_by_ref(&mut *tx, never_selected)
        .await
        .unwrap()
        .is_none());
    assert!(!authority.has_api_access());
    tx.rollback().await.unwrap();
    db.cleanup().await.unwrap();
}
