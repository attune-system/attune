//! Actual completion materializer regressions. Source-only; central runs PG18.
use super::*;
use attune_common::{
    models::{Execution, WorkflowExecution},
    repositories::{
        declared_input::DeclaredInputRepository,
        execution::UpdateExecutionInput,
        execution_secret_value::ExecutionSecretValueRepository as Secrets,
        identity::{
            CreatePermissionAssignmentInput, CreatePermissionSetInput,
            PermissionAssignmentRepository, PermissionSetRepository,
        },
        inherited_input::{
            InheritedInputRepository as Inputs, InputEntityKind, InputSite, InputSitePurpose,
        },
        native_transition_materialization::NativeTransitionMaterializationRepository as Transitions,
        schema_contract::{BindingOwner, SchemaContractRepository},
        workflow::{UpdateWorkflowExecutionInput, WorkflowExecutionRepository as Workflows},
        FindById,
    },
    secret_values::{
        prepare_secret_values, redact_secret_path_sources, restore_secret_values, SecretSource,
        ENTITY_EXECUTION_RESULT, ENTITY_WORKFLOW_VARIABLES,
    },
};
use serde_json::Value;

#[derive(Clone, Copy)]
pub(super) enum Case {
    Unconditional,
    Decisions,
    StaleVariables,
    ExpiredDecision,
}

#[tokio::test]
async fn native_unconditional_stages_publish_a_b_overwrite_a_and_terminal_output() {
    run_native_initial_variable_case(Some(Case::Unconditional)).await;
}

#[tokio::test]
async fn native_true_false_decisions_control_staged_publications_and_output_without_key_replay() {
    run_native_initial_variable_case(Some(Case::Decisions)).await;
}

#[tokio::test]
async fn native_staged_completion_rejects_changed_target_variables_before_publication() {
    run_native_initial_variable_case(Some(Case::StaleVariables)).await;
}

#[tokio::test]
async fn native_staged_decision_expiry_at_target_wait_preserves_variables_without_output() {
    run_native_initial_variable_case(Some(Case::ExpiredDecision)).await;
}

pub(super) fn configure_graph(graph: &mut Value, case: Case) {
    graph["vars"]["a"] = json!("old a");
    graph["vars"]["b"] = json!("old b");
    graph["vars"]["untouched"] = json!("untouched");
    let a = json!({"a":"{{ keystore['system.staged_completion'] }}"});
    let b = json!({"b":"{{ workflow.a }}"});
    let overwritten = json!({"a":"final {{ variables.b }}"});
    graph["tasks"][0]["next"] = if matches!(case, Case::Decisions | Case::ExpiredDecision) {
        json!([
            {"publish":[a],"do":[]},
            {"when":"{{ workflow.a == parameters.value }}","publish":[b],"do":[]},
            {"publish":[overwritten],"do":[]},
            {"when":"{{ vars.b == parameters.value and keystore['system.staged_decision'] == 'allow' and keystore['foreign.must_not_read'] }}",
                "publish":[{"must_not_publish":"{{ keystore['foreign.must_not_publish'] }}"}],"do":[]}
        ])
    } else {
        json!([{"publish":[a,b,overwritten],"do":[]}])
    };
    graph["output_map"] = if matches!(case, Case::ExpiredDecision) {
        json!({})
    } else {
        json!({"b_output":"{{ vars.b }}","a_output":"{{ workflow.a }}",
            "untouched_output":"{{ variables.untouched }}","literal_output":"actual literal"})
    };
}

pub(super) fn saved_graph(snapshot: &ExecutionExecutableSnapshot) -> Value {
    let authored = &snapshot
        .executable
        .workflow_definition
        .as_ref()
        .unwrap()
        .definition;
    let transitions: Vec<_> = authored["tasks"][0]["next"]
        .as_array()
        .unwrap()
        .iter()
        .map(|next| {
            let publish: Vec<_> = next["publish"]
                .as_array()
                .unwrap()
                .iter()
                .flat_map(|directive| directive.as_object().unwrap().iter())
                .map(|(name, value)| json!({"name":name,"value":value}))
                .collect();
            json!({"when":next.get("when"),"publish":publish,"do_tasks":[]})
        })
        .collect();
    json!({"nodes":{"next":{"transitions":transitions}}})
}

pub(super) async fn prepare_issuer(db: &TestDatabase, human: i64, value: &str) {
    helpers::KeyFixture::new_system("staged_completion", value)
        .create(db)
        .await
        .unwrap();
    helpers::KeyFixture::new_system("staged_decision", "deny")
        .create(db)
        .await
        .unwrap();
    let set = PermissionSetRepository::create(
        db.pool(),
        CreatePermissionSetInput {
            r#ref: "fixture.staged_completion_issuer".into(),
            pack: None,
            pack_ref: None,
            label: None,
            description: None,
            grants: json!([{"resource":"keys","actions":["read"],
            "constraints":{"refs":["system.staged_completion","system.staged_decision"]}}]),
        },
    )
    .await
    .unwrap();
    PermissionAssignmentRepository::create(
        db.pool(),
        CreatePermissionAssignmentInput {
            identity: human,
            permset: set.id,
        },
    )
    .await
    .unwrap();
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn check(
    case: Case,
    db: &TestDatabase,
    snapshot: &ExecutionExecutableSnapshot,
    parent: &Execution,
    child: &Execution,
    workflow: &WorkflowExecution,
    human: i64,
    key: &str,
    supplied: &str,
) {
    let mut consent = db.begin().await.unwrap();
    let bindings = DeclaredInputRepository::bindings(&mut consent, parent.id)
        .await
        .unwrap();
    let mut expiring = None;
    for (path, reference, expiry) in [
        (
            "/workflow/tasks/0/next/0/publish/0/a",
            "system.staged_completion",
            false,
        ),
        (
            "/workflow/tasks/0/next/3/when",
            "system.staged_decision",
            matches!(case, Case::ExpiredDecision),
        ),
    ] {
        let Some(binding) = bindings.iter().find(|binding| binding.source_path == path) else {
            continue;
        };
        let approval = Inputs::approve_external_input(
            &mut consent,
            parent.id,
            &binding.declaration_revision,
            path,
            &binding.expression,
            human,
            serde_json::from_value(
                json!([{"resource":"keys","actions":["read"],"constraints":{"refs":[reference]}}]),
            )
            .unwrap(),
            expiry.then(|| chrono::Utc::now() + chrono::Duration::seconds(10)),
        )
        .await
        .unwrap();
        if expiry {
            expiring = Some(approval);
        }
    }
    consent.commit().await.unwrap();

    let initial_secrets =
        Secrets::find_stored_by_entity(db.pool(), ENTITY_WORKFLOW_VARIABLES, parent.id)
            .await
            .unwrap();
    let mut blocker = if expiring.is_some() {
        let mut tx = db.pool().begin().await.unwrap();
        Workflows::find_by_id_for_update(&mut *tx, workflow.id)
            .await
            .unwrap()
            .unwrap();
        Some(tx)
    } else {
        None
    };
    let mut tx = db.begin().await.unwrap();
    // This is the production completion entry, before its outer advisory/row lock.
    let plan = Transitions::prepare_for_completion(&mut tx, child, key)
        .await
        .unwrap()
        .unwrap();
    plan.require_transitions(&saved_graph(snapshot)["nodes"]["next"]["transitions"])
        .unwrap();
    let mut changed_graph = saved_graph(snapshot);
    changed_graph["nodes"]["next"]["transitions"][0]["publish"][0]["value"] =
        json!("changed graph");
    assert!(plan
        .require_transitions(&changed_graph["nodes"]["next"]["transitions"])
        .is_err());
    plan.ensure_fresh().unwrap();
    if matches!(case, Case::Decisions | Case::ExpiredDecision) {
        assert!(plan.fires(1).unwrap());
        assert!(
            !plan.fires(3).unwrap(),
            "neither missing foreign Key may be selected"
        );
    }
    let contract = SchemaContractRepository::find(&mut tx, BindingOwner::Execution, parent.id)
        .await
        .unwrap()
        .unwrap()
        .output;
    if let Some(output) = plan.output() {
        let mut premature = output.clone();
        premature["succeeded"] = json!(true);
        assert!(
            plan.record_output(&mut tx, contract.as_ref(), &premature, key)
                .await
                .is_err(),
            "prepared output cannot publish through transitive pending stages"
        );
    }

    if matches!(case, Case::StaleVariables) {
        let mut changed = workflow.variables.clone();
        changed["concurrent_writer"] = json!(true);
        tokio::time::timeout(
            std::time::Duration::from_secs(5),
            Workflows::update(
                db.pool(),
                workflow.id,
                UpdateWorkflowExecutionInput {
                    variables: Some(changed.clone()),
                    ..Default::default()
                },
            ),
        )
        .await
        .unwrap()
        .unwrap();
        // Even passing the old source object cannot bypass the actual locked row.
        assert!(plan.record(&mut tx, workflow, key).await.is_err());
        assert_eq!(
            Workflows::find_by_id(&mut *tx, workflow.id)
                .await
                .unwrap()
                .unwrap()
                .variables,
            changed
        );
        assert_eq!(
            Secrets::find_stored_by_entity(&mut *tx, ENTITY_WORKFLOW_VARIABLES, parent.id)
                .await
                .unwrap(),
            initial_secrets
        );
        tx.rollback().await.unwrap();
        return;
    }

    if let Some(approval) = expiring {
        let waiter: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *tx)
            .await
            .unwrap();
        let mut blocker = blocker.take().unwrap();
        let release = tokio::spawn(async move {
            let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(20);
            let mut observed_wait = false;
            loop {
                let blocked: bool =
                    sqlx::query_scalar("SELECT cardinality(pg_blocking_pids($1)) > 0")
                        .bind(waiter)
                        .fetch_one(&mut *blocker)
                        .await
                        .unwrap();
                observed_wait |= blocked;
                let expired: bool = sqlx::query_scalar("SELECT expires_at <= clock_timestamp() FROM runtime_external_approval WHERE id=$1")
                    .bind(approval).fetch_one(&mut *blocker).await.unwrap();
                if observed_wait && expired {
                    blocker.commit().await.unwrap();
                    return;
                }
                assert!(
                    tokio::time::Instant::now() < deadline,
                    "must observe the real target wait and source expiry"
                );
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        });
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(25),
            plan.record(&mut tx, workflow, key),
        )
        .await;
        release.await.unwrap();
        assert!(result.unwrap().is_err());
        assert!(
            plan.output().is_none(),
            "the false decision has no output fence carrier"
        );
        assert!(
            plan.ensure_fresh().is_err(),
            "the held standalone decision must retain its own expiry"
        );
        assert_eq!(
            Workflows::find_by_id(&mut *tx, workflow.id)
                .await
                .unwrap()
                .unwrap()
                .variables,
            workflow.variables
        );
        assert_eq!(
            Secrets::find_stored_by_entity(&mut *tx, ENTITY_WORKFLOW_VARIABLES, parent.id)
                .await
                .unwrap(),
            initial_secrets
        );
        assert!(ExecutionRepository::find_by_id(&mut *tx, parent.id)
            .await
            .unwrap()
            .unwrap()
            .result
            .is_none());
        tx.rollback().await.unwrap();
        return;
    }

    let recorded = plan.record(&mut tx, workflow, key).await.unwrap();
    assert_eq!(
        recorded
            .publications
            .iter()
            .map(|(_, name, _, _)| name.as_str())
            .collect::<Vec<_>>(),
        vec!["a", "b", "a"]
    );
    let actual = Workflows::find_by_id(&mut *tx, workflow.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(actual.variables, recorded.variables);
    let actual_secrets =
        Secrets::find_stored_by_entity(&mut *tx, ENTITY_WORKFLOW_VARIABLES, parent.id)
            .await
            .unwrap();
    let variables = restore_secret_values(actual.variables, &actual_secrets, key).unwrap();
    assert_eq!(variables["a"], format!("final {supplied}"));
    assert_eq!(variables["b"], supplied);
    assert_eq!(variables["untouched"], "untouched");
    assert!(!variables
        .as_object()
        .unwrap()
        .contains_key("must_not_publish"));
    let provenance = |ordinal: usize| {
        let SecretSource::Bound(proof) = &recorded.publications[ordinal].3[0].source else {
            panic!("must retain exact typed provenance")
        };
        proof.origins[0].clone()
    };
    let first_a = provenance(0);
    let a_site = InputSite {
        execution: parent.id,
        kind: InputEntityKind::WorkflowVariables,
        purpose: InputSitePurpose::Value,
        path: "/a".into(),
    };
    let b_site = InputSite {
        path: "/b".into(),
        ..a_site.clone()
    };
    let b = Inputs::receipt_context(&mut tx, &b_site, &provenance(1), key)
        .await
        .unwrap();
    assert_eq!(
        b.source_receipt(&first_a).unwrap().site(),
        &a_site,
        "b keeps the authentic intermediate a, not the later overwritten field"
    );
    assert!(Inputs::receipt_context(&mut tx, &a_site, &first_a, key)
        .await
        .is_ok());
    assert!(Inputs::delivery_allowed(&mut tx, &a_site, &first_a, key)
        .await
        .is_err());

    let mut output = plan.output().unwrap().clone();
    output["succeeded"] = json!(true);
    let mut mismatch = output.clone();
    mismatch["b_output"] = json!("not the opaque render");
    assert!(plan
        .record_output(&mut tx, contract.as_ref(), &mismatch, key)
        .await
        .is_err());
    let sources = plan
        .record_output(&mut tx, contract.as_ref(), &output, key)
        .await
        .unwrap();
    let (redacted, secrets) = redact_secret_path_sources(output.clone(), &sources);
    Secrets::replace_entity_with_conn(
        &mut tx,
        ENTITY_EXECUTION_RESULT,
        parent.id,
        &prepare_secret_values(secrets, key).unwrap(),
    )
    .await
    .unwrap();
    ExecutionRepository::update(
        &mut *tx,
        parent.id,
        UpdateExecutionInput {
            result: Some(redacted),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    plan.ensure_fresh_on(&mut tx).await.unwrap();
    tx.commit().await.unwrap();
    plan.ensure_fresh().unwrap();
    assert_eq!(output["b_output"], supplied);
    assert_eq!(output["a_output"], format!("final {supplied}"));
    assert_eq!(output["untouched_output"], "untouched");
    let stored = ExecutionRepository::find_by_id(db.pool(), parent.id)
        .await
        .unwrap()
        .unwrap();
    let stored_secrets =
        Secrets::find_stored_by_entity(db.pool(), ENTITY_EXECUTION_RESULT, parent.id)
            .await
            .unwrap();
    assert_eq!(
        restore_secret_values(stored.result.unwrap(), &stored_secrets, key).unwrap(),
        output
    );
}
