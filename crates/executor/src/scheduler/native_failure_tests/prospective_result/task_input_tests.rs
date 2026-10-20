//! Genuine original native sources, whole-field destination proof and tamper denial.
use super::*;
use attune_common::repositories::{
    accepted_input::AcceptedInputRepository as Accepted,
    declared_input_materialization::DeclaredInputMaterializationRepository as Materialize,
    schema_contract::{BindingOwner, SchemaContractRepository as Schemas},
};
use attune_common::secret_provenance::{SecretOrigin, SecretProvenance};
use futures::FutureExt;
use std::panic::AssertUnwindSafe;

async fn composite_case(tamper: bool) {
    let (db, files, parent, workflow, completed) = fixture::original_executor_native_result(
        NativeResultCase::ProspectiveCompositeChild,
        canonical_task_graph,
    )
    .await;
    let result = AssertUnwindSafe(async {
    let mut tx = db.begin().await.unwrap();
    CacheEntryRepository::protect_transaction(&mut tx, CacheTransactionMode::PinMutation).await.unwrap();
    let plan = Transitions::prepare_for_completion(&mut tx, &completed, fixture::KEY).await.unwrap().unwrap();
    let sources = Native::capture_workflow_with_key(&mut tx, parent.id, Some(fixture::KEY)).await.unwrap();
    let context = ExecutionScheduler::prepare_native_transition_context(&mut tx, &plan, &sources, Some(fixture::KEY)).await.unwrap();
    let outcome = ExecutionScheduler::advance_workflow_prepared(
        &mut tx, &AtomicUsize::new(0), Some(fixture::KEY), &completed,
        &SchedulerMetadataCaches::new(), Some(&plan), Some(&sources), Some(&context),
        None, None, None,
    ).await.unwrap();
    assert_eq!(outcome.execution_requests.len(), 1);
    let children = ExecutionRepository::find_by_parent(&mut *tx, parent.id).await.unwrap();
    let consumer = children.iter().find(|row| row.workflow_task.as_ref()
        .is_some_and(|task| task.task_name == "consumer")).unwrap();
    let before = ExecutionSecretValueRepository::find_stored_by_entity(&mut *tx, ENTITY_EXECUTION_CONFIG, consumer.id).await.unwrap();
    assert_eq!(before.iter().map(|field| field.json_path.as_str()).collect::<Vec<_>>(), vec!["/value", "/whole"]);
    let restored = attune_common::secret_values::restore_secret_values(
        consumer.config.clone().unwrap(), &before, fixture::KEY,
    ).unwrap();
    assert_eq!(restored["whole"], serde_json::json!({"literal":"replacement","other":LITERAL}));
    let fence = Accepted::revalidate_execution(&mut tx, consumer.id, fixture::KEY).await.unwrap();
    let receipts = Materialize::prepare_execution_input_receipts(&mut tx, consumer.id, fixture::KEY).await.unwrap();
    assert_eq!(receipts.len(), 2);
    // Diagnostic SQL inspects the actual archive produced by the owning writer.
    // No receipt, admission, signature, config or acceptance row is fabricated.
    let body: JsonValue = sqlx::query_scalar("SELECT body FROM runtime_accepted_input WHERE execution=$1")
        .bind(consumer.id).fetch_one(&mut *tx).await.unwrap();
    let field = body["fields"].as_array().unwrap().iter().find(|field| field["json_path"] == "/whole").unwrap();
    let provenance: SecretProvenance = serde_json::from_str(field["source_ref"].as_str().unwrap()).unwrap();
    let [SecretOrigin::Entity { entity_type, entity_id, path }] = provenance.origins.as_slice() else {
        panic!("whole field has exactly one canonical destination VALUE receipt");
    };
    assert_eq!(entity_type, "declared_input:execution_config:value");
    assert_eq!(path, "/whole");
    let proof: JsonValue = sqlx::query_scalar("SELECT proof FROM runtime_input_receipt WHERE id=$1")
        .bind(*entity_id).fetch_one(&mut *tx).await.unwrap();
    let selections = proof["selections"].as_array().unwrap();
    for leaf in ["/whole/literal", "/whole/other"] {
        assert!(selections.iter().any(|selected| selected["source"]["purpose"] == "inherited"
            && selected["source"]["site"]["path"] == leaf));
    }
    assert!(!selections.iter().any(|selected| selected["source"]["purpose"] == "inherited"
        && selected["source"]["site"]["path"] == "/whole"), "overwritten whole is not the replacement VALUE");
    assert!(selections.iter().any(|selected| selected["source"]["purpose"] == "control"
        && selected["source"]["site"]["path"] == "/whole"), "old whole retains its original independent dependencies as CONTROL");
    let parents: Vec<i64> = sqlx::query_scalar("SELECT source_receipt FROM runtime_input_receipt_source WHERE receipt=$1 ORDER BY source_receipt")
        .bind(*entity_id).fetch_all(&mut *tx).await.unwrap();
    assert!(!parents.is_empty());
    assert_eq!(parents.len(), parents.iter().copied().collect::<std::collections::BTreeSet<_>>().len());
    if tamper {
        let binding = Schemas::find(&mut tx, BindingOwner::Execution, consumer.id).await.unwrap().unwrap();
        let contract = attune_common::value_schema::CompiledContract::new(&binding.input).unwrap();
        let authored = &parent.executable_snapshot.as_ref().unwrap().executable.workflow_definition.as_ref().unwrap().definition["tasks"][1]["input"];
        let prepared = plan.prepare_task_input(&mut tx, "consumer", &consumer.action_ref, authored, &contract, fixture::KEY).await.unwrap();
        let mut changed = prepared.supplied().clone();
        changed["whole"]["literal"] = serde_json::json!("invented replacement");
        let count_before: i64 = sqlx::query_scalar("SELECT count(*) FROM runtime_input_receipt")
            .fetch_one(&mut *tx).await.unwrap();
        let error = prepared.record_child(&mut tx, consumer.id, &changed, fixture::KEY).await.unwrap_err();
        assert!(matches!(error, attune_common::Error::PermissionDenied(ref message)
            if message == "Prepared workflow child input value differs from its original render"));
        let after = ExecutionSecretValueRepository::find_stored_by_entity(&mut *tx, ENTITY_EXECUTION_CONFIG, consumer.id).await.unwrap();
        assert_eq!(before, after);
        let count_after: i64 = sqlx::query_scalar("SELECT count(*) FROM runtime_input_receipt")
            .fetch_one(&mut *tx).await.unwrap();
        assert_eq!(count_before, count_after, "tampered actual data mints no replacement receipt");
        Accepted::revalidate_execution(&mut tx, consumer.id, fixture::KEY).await.unwrap();
    }
    fence.ensure_fresh_on(&mut tx).await.unwrap();
    for receipt in &receipts { receipt.ensure_fresh_on(&mut tx).await.unwrap(); }
    let state = WorkflowExecutionRepository::find_by_id(&mut *tx, workflow.id).await.unwrap().unwrap();
    assert_eq!(state.status, ExecutionStatus::Running);
    tx.rollback().await.unwrap();
    }).catch_unwind().await;
    let database_cleanup = db.cleanup().await;
    let files_cleanup = files.close();
    database_cleanup.unwrap();
    files_cleanup.unwrap();
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
}

#[tokio::test]
async fn genuine_leaf_receipts_compose_a_whole_secret_child_field_with_overwrite_controls() {
    composite_case(false).await;
}

#[tokio::test]
async fn tampered_whole_destination_is_denied_without_rewriting_original_input() {
    composite_case(true).await;
}

async fn independent_recovery_case(status: ExecutionStatus) {
    let (db, files, parent, workflow, completed) =
        fixture::original_executor_independent_recovery(status, canonical_task_graph).await;
    let result = AssertUnwindSafe(async {
        let mut tx = db.begin().await.unwrap();
        CacheEntryRepository::protect_transaction(&mut tx, CacheTransactionMode::PinMutation).await.unwrap();
        assert_eq!(completed.status, status);
        let plan = Transitions::prepare_for_completion(&mut tx, &completed, fixture::KEY).await.unwrap().unwrap();
        assert!(plan.fires(0).unwrap(), "actual failed/timeout recovery edge fires");
        let recorded = plan.record(&mut tx, &workflow, fixture::KEY).await.unwrap();
        ExecutionSecretValueRepository::replace_entity_with_conn(
            &mut tx, attune_common::secret_values::ENTITY_WORKFLOW_VARIABLES,
            parent.id, &recorded.secrets,
        ).await.unwrap();
        WorkflowExecutionRepository::update(&mut *tx, workflow.id,
            attune_common::repositories::workflow::UpdateWorkflowExecutionInput {
                variables: Some(recorded.variables.clone()), ..Default::default()
            }).await.unwrap();
        // collect_leaves keeps a template-free object as one whole literal.
        // Its actual Independent Value receipt and encrypted projection are at
        // /recovered, not at the object's scalar member /recovered/literal.
        let field = recorded.secrets.iter().find(|field| field.json_path == "/recovered")
            .expect("recovery record phase must produce the protected whole-root projection");
        assert!(attune_common::secret_values::is_redaction_marker(&recorded.variables["recovered"]),
            "recovery record phase must protect the whole variable, not leave a public constant");
        assert!(field.source_kind == "provenance", "recovery root must retain typed provenance");
        let provenance: SecretProvenance = serde_json::from_str(field.source_ref.as_deref()
            .expect("recovery root projection must retain its original typed receipt reference"))
            .expect("recovery root typed provenance must decode");
        let [SecretOrigin::Entity { entity_type, entity_id, path }] = provenance.origins.as_slice() else {
            panic!("actual recovery publication has one original typed Value receipt");
        };
        assert_eq!(entity_type, "declared_input:workflow_variables:value");
        assert_eq!(path, "/recovered");
        let site = attune_common::repositories::inherited_input::InputSite {
            execution: parent.id,
            kind: attune_common::repositories::inherited_input::InputEntityKind::WorkflowVariables,
            purpose: attune_common::repositories::inherited_input::InputSitePurpose::Value,
            path: "/recovered".into(),
        };
        let source = attune_common::repositories::inherited_input::InheritedInputRepository::prepare_source(
            &mut tx, parent.id, &site, &provenance.origins[0], fixture::KEY,
        ).await.expect("published recovery root must authenticate its actual typed Value and stored protected object");
        assert!(source.value() == &serde_json::json!({"literal":"recovery"}),
            "authenticated recovery root must match the original whole object");
        let staged: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM runtime_input_receipt_staged WHERE receipt=$1)")
            .bind(*entity_id).fetch_one(&mut *tx).await.unwrap();
        assert!(!staged, "failed/timeout factory records Independent, not staged publication evidence");
        let restored = attune_common::secret_values::restore_secret_values(
            recorded.variables.clone(),
            &ExecutionSecretValueRepository::find_stored_by_entity(&mut *tx,
                attune_common::secret_values::ENTITY_WORKFLOW_VARIABLES, parent.id).await.unwrap(),
            fixture::KEY,
        ).unwrap();
        assert!(restored["recovered"] == serde_json::json!({"literal":"recovery"}),
            "stored protected recovery root must restore the original object");
        let authored = &parent.executable_snapshot.as_ref().unwrap().executable.workflow_definition.as_ref().unwrap().definition["tasks"][1]["input"];
        // The actual consumer uses this same original leaf action/contract.
        let binding = Schemas::find(&mut tx, BindingOwner::Execution, completed.id).await.unwrap().unwrap();
        let contract = attune_common::value_schema::CompiledContract::new(&binding.input).unwrap();
        assert_eq!(contract.secret_paths(authored).unwrap(), vec!["/whole"]);
        assert_eq!(authored["whole"], serde_json::json!("{{ variables.recovered }}"));
        assert!(!plan.supports_task_input(authored, &contract).unwrap(),
            "the real opaque Independent frame must be structurally unselected before input evaluation");
        let receipts_before: i64 = sqlx::query_scalar("SELECT count(*) FROM runtime_input_receipt")
            .fetch_one(&mut *tx).await.unwrap();
        let children_before = ExecutionRepository::find_by_parent(&mut *tx, parent.id).await.unwrap().len();
        match plan.prepare_task_input(&mut tx, "consumer", &completed.action_ref, authored, &contract, fixture::KEY).await {
            Ok(_) => panic!("direct preparation cannot bypass the opaque frame mode guard"),
            Err(error) => assert!(matches!(error, attune_common::Error::PermissionDenied(_))),
        }
        let receipts_after: i64 = sqlx::query_scalar("SELECT count(*) FROM runtime_input_receipt")
            .fetch_one(&mut *tx).await.unwrap();
        assert_eq!(receipts_before, receipts_after);
        assert_eq!(children_before, ExecutionRepository::find_by_parent(&mut *tx, parent.id).await.unwrap().len());
        source.ensure_fresh_on(&mut tx).await.expect("original recovery root proof must remain fresh after direct preparation denial");
        plan.ensure_fresh_on(&mut tx).await.unwrap();
        tx.rollback().await.unwrap();
    }).catch_unwind().await;
    let database_cleanup = db.cleanup().await;
    let files_cleanup = files.close();
    database_cleanup.unwrap();
    files_cleanup.unwrap();
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
}

#[tokio::test]
async fn recorded_failed_independent_recovery_frame_is_unselected_and_direct_prepare_denies() {
    independent_recovery_case(ExecutionStatus::Failed).await;
}

#[tokio::test]
async fn recorded_timeout_independent_recovery_frame_is_unselected_and_direct_prepare_denies() {
    independent_recovery_case(ExecutionStatus::Timeout).await;
}
