//! Actual B plan and scheduler consumers of A's private ResultView capabilities.
//! No generated-success recipe or reader/GC acceptance is asserted here.
use super::*;
use fixture::NativeResultCase;

mod task_input_tests;

const LITERAL: &str = "{{ keystore['foreign.never_parse'] }}";

#[tokio::test]
async fn ordinary_result_view_conditions_and_overwrites_feed_the_actual_child_input() {
    use attune_common::{
        repositories::{
            accepted_input::AcceptedInputRepository,
            declared_input_materialization::DeclaredInputMaterializationRepository as Materialize,
            inherited_input::{InputEntityKind, InputSitePurpose},
        },
        secret_provenance::SecretProvenance,
    };
    let (db, files, parent, workflow, child) = fixture::original_executor_native_result(
        NativeResultCase::ProspectiveResultChild,
        canonical_task_graph,
    )
    .await;
    let mut tx = db.begin().await.unwrap();
    CacheEntryRepository::protect_transaction(&mut tx, CacheTransactionMode::PinMutation)
        .await
        .unwrap();
    let plan = Transitions::prepare_for_completion(&mut tx, &child, fixture::KEY)
        .await
        .unwrap()
        .unwrap();
    assert!(!plan.fires(0).unwrap());
    for index in 1..5 {
        assert!(plan.fires(index).unwrap());
    }
    let decisions = plan.completion_decisions();
    assert_eq!(decisions.len(), 5);
    assert_eq!(decisions[0].value(), &serde_json::json!(false));
    let sources = Native::capture_workflow_with_key(&mut tx, parent.id, Some(fixture::KEY))
        .await
        .unwrap();
    let context = ExecutionScheduler::prepare_native_transition_context(
        &mut tx,
        &plan,
        &sources,
        Some(fixture::KEY),
    )
    .await
    .unwrap();
    let outcome = ExecutionScheduler::advance_workflow_prepared(
        &mut tx,
        &AtomicUsize::new(0),
        Some(fixture::KEY),
        &child,
        &SchedulerMetadataCaches::new(),
        Some(&plan),
        Some(&sources),
        Some(&context),
        None,
        None,
        None,
    )
    .await
    .unwrap();
    assert_eq!(outcome.execution_requests.len(), 1);
    let children = ExecutionRepository::find_by_parent(&mut *tx, parent.id)
        .await
        .unwrap();
    let consumer = children
        .iter()
        .find(|row| {
            row.workflow_task
                .as_ref()
                .is_some_and(|task| task.task_name == "consumer")
        })
        .unwrap();
    let fields = ExecutionSecretValueRepository::find_stored_by_entity(
        &mut *tx,
        ENTITY_EXECUTION_CONFIG,
        consumer.id,
    )
    .await
    .unwrap();
    assert!(
        !fields.is_empty(),
        "the child receives real purpose-qualified lineage, not plain JSON"
    );
    // The scheduler must have sealed this nonempty projection in its original
    // creation transaction. Revalidation cannot create or repair that seal.
    let accepted_inputs =
        AcceptedInputRepository::revalidate_execution(&mut tx, consumer.id, fixture::KEY)
            .await
            .expect("nonempty consumer input has its genuine original acceptance seal");
    let input_receipts =
        Materialize::prepare_execution_input_receipts(&mut tx, consumer.id, fixture::KEY)
            .await
            .expect("actual consumer ciphertext is bound to its first typed deliveries");
    assert_eq!(input_receipts.len(), fields.len());
    for field in &fields {
        assert_eq!(field.source_kind, "provenance");
        let provenance: SecretProvenance =
            serde_json::from_str(field.source_ref.as_deref().unwrap()).unwrap();
        let receipt = input_receipts
            .iter()
            .find(|receipt| {
                receipt.site().execution == consumer.id
                    && receipt.site().kind == InputEntityKind::ExecutionConfig
                    && receipt.site().purpose == InputSitePurpose::Value
                    && receipt.site().path == field.json_path
            })
            .expect("every stored field has its own config/value first delivery");
        assert_eq!(provenance.origins, vec![receipt.origin()]);
        assert!(provenance.templates.is_empty());
    }
    let config = attune_common::secret_values::restore_secret_values(
        consumer.config.clone().unwrap(),
        &fields,
        fixture::KEY,
    )
    .unwrap();
    assert_eq!(config["value"], serde_json::json!(LITERAL));
    assert_eq!(
        config["whole"],
        serde_json::json!({"literal":"replacement"})
    );
    let actual = WorkflowExecutionRepository::find_by_id(&mut *tx, workflow.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(actual.status, ExecutionStatus::Running);
    assert_eq!(actual.completed_tasks, vec!["next"]);
    assert_eq!(actual.current_tasks, vec!["consumer"]);
    assert_eq!(
        ExecutionRepository::find_by_id(&mut *tx, parent.id)
            .await
            .unwrap()
            .unwrap()
            .status,
        ExecutionStatus::Running
    );
    assert_published_program(&mut tx, parent.id, child.id).await;
    let attune_common::secret_provenance::SecretOrigin::Entity {
        entity_id: false_receipt,
        ..
    } = decisions[0].origin()
    else {
        panic!("actual Condition has a purpose-qualified receipt");
    };
    let proofs: Vec<JsonValue> = sqlx::query_scalar("SELECT proof FROM runtime_input_receipt WHERE execution=$1 AND entity_kind='workflow_variables' AND destination_purpose='value'").bind(parent.id).fetch_all(&mut *tx).await.unwrap();
    assert_eq!(proofs.len(), 4);
    assert!(
        proofs
            .iter()
            .all(|proof| proof["selections"]
                .as_array()
                .unwrap()
                .iter()
                .any(|selection| {
                    selection["source"]["purpose"] == "control"
                        && selection["source"]["receipt"] == serde_json::json!(false_receipt)
                })),
        "each actual publication retains the original independent false control"
    );
    plan.ensure_fresh_on(&mut tx).await.unwrap();
    sources.ensure_fresh_on(&mut tx).await.unwrap();
    // A commit owner can reopen the EXACT original child inputs after the
    // complete advancement, including nonempty first deliveries. Duplicated
    // plain pending messages reuse the same opaque per-child proof owner.
    let mut duplicate_pending = outcome.execution_requests.clone();
    duplicate_pending.push(outcome.execution_requests[0].clone());
    let input_publications =
        PreparedWorkflowInputPublications::prepare(&mut tx, &duplicate_pending, Some(fixture::KEY))
            .await
            .expect("reopen original nonempty inputs without sealing or new authority");
    let mut forged_pending = outcome.execution_requests[0].clone();
    forged_pending.action_ref.push_str(".wrong-target");
    let mut forged_publications = duplicate_pending.clone();
    forged_publications.push(forged_pending.clone());
    assert!(
        PreparedWorkflowInputPublications::prepare(
            &mut tx,
            &forged_publications,
            Some(fixture::KEY),
        )
        .await
        .is_err(),
        "a valid first message must not hide a same-ID invalid duplicate"
    );
    let mut forged_first = WorkflowPublications::default();
    forged_first.push(forged_pending);
    forged_first.extend(duplicate_pending.clone());
    assert!(
        PreparedWorkflowInputPublications::prepare(&mut tx, &forged_first, Some(fixture::KEY),)
            .await
            .is_err(),
        "an invalid first duplicate must not be hidden by a later valid message"
    );
    // A deferred child is transport bookkeeping, not a pending message. Two
    // recursive outcome merges must preserve its original ID even with no MQ
    // payload at either level. No fixture acceptance or receipt is synthesized.
    let mut deferred = WorkflowPublications::default();
    deferred.register_created(consumer.id);
    deferred.register_created(consumer.id);
    let mut recursive_outcome = WorkflowPublications::default();
    recursive_outcome.extend(deferred);
    let mut outer_outcome = WorkflowPublications::default();
    outer_outcome.extend(recursive_outcome);
    assert!(
        outer_outcome.is_empty(),
        "no message was queued for this deferred owner"
    );
    assert_eq!(
        outer_outcome.owner_ids().collect::<Vec<_>>(),
        vec![consumer.id]
    );
    let deferred_inputs =
        PreparedWorkflowInputPublications::prepare(&mut tx, &outer_outcome, Some(fixture::KEY))
            .await
            .expect("a deferred owner survives merging and reopens original persisted input");
    let mut missing_deferred = WorkflowPublications::default();
    missing_deferred.register_created(i64::MAX);
    assert!(
        PreparedWorkflowInputPublications::prepare(&mut tx, &missing_deferred, Some(fixture::KEY),)
            .await
            .is_err(),
        "an absent deferred owner must fail exact indexed lookup"
    );
    let empty = WorkflowPublications::default();
    PreparedWorkflowInputPublications::prepare(&mut tx, &empty, None)
        .await
        .expect("zero tracked owners require neither lookup nor encryption key")
        .ensure_fresh_on(&mut tx)
        .await
        .unwrap();
    accepted_inputs.ensure_fresh_on(&mut tx).await.unwrap();
    for receipt in &input_receipts {
        receipt.ensure_fresh_on(&mut tx).await.unwrap();
    }
    input_publications.ensure_fresh_on(&mut tx).await.unwrap();
    deferred_inputs.ensure_fresh_on(&mut tx).await.unwrap();
    tx.commit().await.unwrap();
    input_publications.ensure_fresh().unwrap();
    deferred_inputs.ensure_fresh().unwrap();
    deferred_inputs.ensure_child_fresh(consumer.id).unwrap();
    for pending in &duplicate_pending {
        input_publications
            .ensure_child_fresh(pending.execution_id)
            .unwrap();
    }
    accepted_inputs.ensure_fresh().unwrap();
    for receipt in &input_receipts {
        receipt.ensure_fresh().unwrap();
    }
    plan.ensure_fresh().unwrap();
    sources.ensure_fresh().unwrap();
    db.cleanup().await.unwrap();
    files.close().unwrap();
}

async fn assert_published_program(conn: &mut PgConnection, parent: i64, child: i64) {
    let releases: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM runtime_native_result_view_release WHERE source_execution=$1",
    )
    .bind(child)
    .fetch_one(&mut *conn)
    .await
    .unwrap();
    assert_eq!(releases, 1);
    // Four real stages and the four view-dependent Conditions publish exactly
    // their issued receipts. The first independent false decision is not staged.
    let staged: Vec<(String, String, bool)> = sqlx::query_as("SELECT r.destination_purpose,r.destination_path,s.publication_transaction IS NOT NULL FROM runtime_input_receipt r JOIN runtime_input_receipt_staged s ON s.receipt=r.id WHERE r.execution=$1 ORDER BY r.id").bind(parent).fetch_all(&mut *conn).await.unwrap();
    assert_eq!(staged.len(), 8);
    assert!(staged.iter().all(|row| row.2));
    assert_eq!(
        staged
            .iter()
            .filter(|row| row.0 == "value" && matches!(row.1.as_str(), "/whole" | "/whole/literal"))
            .count(),
        2
    );
    assert!(staged.iter().all(|row| row.1 != "/not_fired"));
    let shorthand_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM runtime_input_receipt WHERE execution=$1 AND destination_path='/whole' AND proof->'native'->>'recipe'='result_shorthand'").bind(parent).fetch_one(&mut *conn).await.unwrap();
    assert_eq!(
        shorthand_count, 1,
        "no second record_native_result_shorthand receipt replaces the issued stage"
    );
}

#[tokio::test]
async fn ordinary_result_view_output_leaves_wait_for_the_actual_ordered_publications() {
    let (db, files, parent, workflow, child) = fixture::original_executor_native_result(
        NativeResultCase::ProspectiveResultOutput,
        canonical_task_graph,
    )
    .await;
    let mut tx = db.begin().await.unwrap();
    CacheEntryRepository::protect_transaction(&mut tx, CacheTransactionMode::PinMutation)
        .await
        .unwrap();
    let plan = Transitions::prepare_for_completion(&mut tx, &child, fixture::KEY)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        plan.output().unwrap(),
        &serde_json::json!({"first":LITERAL,"latest":"replacement"})
    );
    assert_eq!(plan.prepared_output_leaves().count(), 2);
    assert!(parent
        .executable_snapshot
        .as_ref()
        .unwrap()
        .executable
        .workflow_definition
        .as_ref()
        .unwrap()
        .definition["output_map"]
        .get("succeeded")
        .is_none());
    // The existing authored-leaf recorder compares this final composition. This
    // does not issue status authority, persist a root output or complete a root.
    let mut composed = plan.output().unwrap().clone();
    composed["succeeded"] = serde_json::json!(true);
    assert!(plan
        .record_output(&mut tx, None, &composed, fixture::KEY)
        .await
        .is_err());
    plan.record(&mut tx, &workflow, fixture::KEY).await.unwrap();
    assert_published_program(&mut tx, parent.id, child.id).await;
    let origins = plan
        .record_output(&mut tx, None, &composed, fixture::KEY)
        .await
        .unwrap();
    assert_eq!(origins.len(), 2);
    assert_eq!(
        ExecutionRepository::find_by_id(&mut *tx, parent.id)
            .await
            .unwrap()
            .unwrap()
            .result,
        parent.result
    );
    plan.ensure_fresh_on(&mut tx).await.unwrap();
    tx.commit().await.unwrap();
    plan.ensure_fresh().unwrap();
    db.cleanup().await.unwrap();
    files.close().unwrap();
}

#[tokio::test]
async fn ordinary_result_view_plan_cannot_cross_the_source_transaction() {
    let (db, files, parent, workflow, child) = fixture::original_executor_native_result(
        NativeResultCase::ProspectiveResultOutput,
        canonical_task_graph,
    )
    .await;
    let mut tx = db.begin().await.unwrap();
    CacheEntryRepository::protect_transaction(&mut tx, CacheTransactionMode::PinMutation)
        .await
        .unwrap();
    let plan = Transitions::prepare_for_completion(&mut tx, &child, fixture::KEY)
        .await
        .unwrap()
        .unwrap();
    tx.commit().await.unwrap();
    let mut tx = db.begin().await.unwrap();
    CacheEntryRepository::protect_transaction(&mut tx, CacheTransactionMode::PinMutation)
        .await
        .unwrap();
    assert!(plan.record(&mut tx, &workflow, fixture::KEY).await.is_err());
    assert_eq!(
        WorkflowExecutionRepository::find_by_id(&mut *tx, workflow.id)
            .await
            .unwrap()
            .unwrap()
            .variables,
        workflow.variables
    );
    let releases: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM runtime_native_result_view_release WHERE source_execution=$1",
    )
    .bind(child.id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert_eq!(releases, 0);
    assert_eq!(
        ExecutionRepository::find_by_id(&mut *tx, parent.id)
            .await
            .unwrap()
            .unwrap()
            .result,
        parent.result
    );
    tx.rollback().await.unwrap();
    db.cleanup().await.unwrap();
    files.close().unwrap();
}

#[tokio::test]
async fn ordinary_result_view_changed_source_denies_before_any_variable_publication() {
    let (db, files, parent, workflow, child) = fixture::original_executor_native_result(
        NativeResultCase::ProspectiveResultOutput,
        canonical_task_graph,
    )
    .await;
    let mut tx = db.begin().await.unwrap();
    CacheEntryRepository::protect_transaction(&mut tx, CacheTransactionMode::PinMutation)
        .await
        .unwrap();
    let plan = Transitions::prepare_for_completion(&mut tx, &child, fixture::KEY)
        .await
        .unwrap()
        .unwrap();
    ExecutionRepository::update(
        &mut *tx,
        child.id,
        UpdateExecutionInput {
            result: Some(serde_json::json!({"data":{"literal":"changed"}})),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert!(plan.record(&mut tx, &workflow, fixture::KEY).await.is_err());
    assert_eq!(
        WorkflowExecutionRepository::find_by_id(&mut *tx, workflow.id)
            .await
            .unwrap()
            .unwrap()
            .variables,
        workflow.variables
    );
    let releases: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM runtime_native_result_view_release WHERE source_execution=$1",
    )
    .bind(child.id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert_eq!(releases, 0);
    let pending: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM runtime_input_receipt_staged s JOIN runtime_input_receipt r ON r.id=s.receipt WHERE r.execution=$1 AND s.publication_transaction IS NOT NULL").bind(parent.id).fetch_one(&mut *tx).await.unwrap();
    assert_eq!(pending, 0);
    tx.rollback().await.unwrap();
    db.cleanup().await.unwrap();
    files.close().unwrap();
}

#[tokio::test]
async fn ordinary_result_view_missing_real_native_release_denies_output_recording() {
    missing_publication("/first").await;
}

#[tokio::test]
async fn ordinary_result_view_condition_cannot_be_rebound_as_an_output_value() {
    use attune_common::repositories::{
        declared_input::DeclaredInputRepository,
        inherited_input::{InheritedInputRepository as Inputs, InputSitePurpose},
    };
    let (db, files, parent, _, child) = fixture::original_executor_native_result(
        NativeResultCase::ProspectiveResultOutput,
        canonical_task_graph,
    )
    .await;
    let mut tx = db.begin().await.unwrap();
    CacheEntryRepository::protect_transaction(&mut tx, CacheTransactionMode::PinMutation)
        .await
        .unwrap();
    let plan = Transitions::prepare_for_completion(&mut tx, &child, fixture::KEY)
        .await
        .unwrap()
        .unwrap();
    let binding = DeclaredInputRepository::bindings(&mut tx, parent.id)
        .await
        .unwrap()
        .into_iter()
        .find(|binding| binding.source_path == "/workflow/output_map/first")
        .unwrap();
    let mut session = Inputs::begin_render(
        &mut tx,
        parent.id,
        &binding.declaration_revision,
        &binding.source_path,
        &binding.expression,
        InputSitePurpose::Value,
    )
    .await
    .unwrap();
    let decisions = plan.completion_decisions();
    let condition = decisions[1];
    assert_eq!(condition.site().purpose, InputSitePurpose::Condition);
    assert!(Inputs::bind_prepared(&mut session, "/copied", condition, condition.value()).is_err());
    Inputs::attach_prepared_control(&mut session, condition).unwrap();
    tx.rollback().await.unwrap();
    db.cleanup().await.unwrap();
    files.close().unwrap();
}

#[tokio::test]
async fn ordinary_result_view_overwrite_ancestry_stays_private_before_publication() {
    missing_publication("/latest").await;
}

async fn missing_publication(path: &str) {
    use attune_common::repositories::inherited_input::{
        InheritedInputRepository as Inputs, InputEntityKind, InputSite, InputSitePurpose,
    };
    let (db, files, parent, workflow, child) = fixture::original_executor_native_result(
        NativeResultCase::ProspectiveResultOutput,
        canonical_task_graph,
    )
    .await;
    let mut tx = db.begin().await.unwrap();
    CacheEntryRepository::protect_transaction(&mut tx, CacheTransactionMode::PinMutation)
        .await
        .unwrap();
    let plan = Transitions::prepare_for_completion(&mut tx, &child, fixture::KEY)
        .await
        .unwrap()
        .unwrap();
    let (_, rendered) = plan
        .prepared_output_leaves()
        .find(|(site, _)| *site == path)
        .unwrap();
    let site = InputSite {
        execution: parent.id,
        kind: InputEntityKind::ExecutionResult,
        purpose: InputSitePurpose::Value,
        path: path.into(),
    };
    let destination = Inputs::prepare_destination(&mut tx, rendered, &site, fixture::KEY)
        .await
        .unwrap();
    // Exercise A's actual record gate, not only B's recorded-phase shortcut.
    // Both the original native release and the real staged ancestry are absent.
    // No immutable release/publication row is edited or fabricated to test this.
    assert!(Inputs::record_prepared(
        &mut tx,
        rendered,
        &destination,
        rendered.value(),
        rendered.selections(),
        fixture::KEY
    )
    .await
    .is_err());
    let releases: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM runtime_native_result_view_release WHERE source_execution=$1",
    )
    .bind(child.id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert_eq!(releases, 0);
    let published: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM runtime_input_receipt_staged s JOIN runtime_input_receipt r ON r.id=s.receipt WHERE r.execution=$1 AND s.publication_transaction IS NOT NULL").bind(parent.id).fetch_one(&mut *tx).await.unwrap();
    assert_eq!(published, 0);
    tx.rollback().await.unwrap();
    assert_eq!(
        WorkflowExecutionRepository::find_by_id(db.pool(), workflow.id)
            .await
            .unwrap()
            .unwrap()
            .variables,
        workflow.variables
    );
    assert_eq!(
        ExecutionRepository::find_by_id(db.pool(), parent.id)
            .await
            .unwrap()
            .unwrap()
            .result,
        parent.result
    );
    db.cleanup().await.unwrap();
    files.close().unwrap();
}
