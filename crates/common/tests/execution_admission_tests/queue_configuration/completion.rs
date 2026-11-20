use super::*;
use attune_common::{
    models::{
        enums::{WorkerStatus, WorkerType},
        WorkQueueItemStatus,
    },
    repositories::{
        accepted_batch_dependency::AcceptedBatchDependencyRepository as Batch,
        delegated_execution::ProtectedQueueAck,
        execution::UpdateExecutionInput,
        recorded_input_authority::RecordedInputAuthorityRepository as Contributors,
        runtime::{CreateWorkerInput, WorkerRepository},
        schema_contract::{BindingOwner, SchemaContractRepository},
        work_queue::{LeaseWorkQueueItemsInput, UpdateWorkQueueItemInput, WorkQueueItemRepository},
        work_queue_output::WorkQueueOutputRepository as Output,
        worker_delivery::WorkerDeliveryRepository as Delivery,
        Patch,
    },
};
use chrono::{Duration, Utc};
use uuid::Uuid;

#[tokio::test]
async fn recorded_queue_completion_uses_the_closed_worker_and_accepted_settings() {
    let db = create_test_pool().await.unwrap();
    let (db, snapshot, policy, human) = fixture_with_queue_shape(
        db,
        false,
        false,
        None,
        false,
        false,
        Some(json!({"queue_ack":{"type":"object","secret":true}})),
    )
    .await;
    let mut tx = db.begin().await.unwrap();
    attune_common::repositories::schema_type::SchemaTypeRepository::lock_reclamation(&mut tx)
        .await
        .unwrap();
    let queue =
        WorkQueueRepository::find_by_ref(&mut *tx, &format!("{}.inbox", policy.owner.pack_ref))
            .await
            .unwrap()
            .unwrap();
    let queue = WorkQueueRepository::update(
        &mut *tx,
        queue.id,
        UpdateWorkQueueInput {
            config: Some(json!({"ack_contract":{"version":2},"dispatch":{"retry_limit":3}})),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let revision = Authority::capture_policy(&mut tx, &policy).await.unwrap();
    Authority::activate_policy(&mut tx, revision.id)
        .await
        .unwrap();
    let item = accepted_record(&mut tx, &queue, &snapshot, human, json!({})).await;
    let admission = Admission::queue_batch(&mut tx, &queue, std::slice::from_ref(&item), &snapshot)
        .await
        .unwrap();
    let execution = ExecutionRepository::create_pinned(
        &mut *tx,
        CreateExecutionInput {
            action: Some(snapshot.executable.action.id),
            action_ref: snapshot.executable.action.r#ref.clone(),
            permission_set_refs: admission.authority().selected_refs.clone(),
            status: ExecutionStatus::Requested,
            ..Default::default()
        },
        &snapshot,
    )
    .await
    .unwrap();
    let record = admission
        .attach(&mut tx, RuntimeOwner::Execution(execution.id))
        .await
        .unwrap();
    let binding = SchemaContractRepository::materialize_action_pair(&mut tx, &snapshot)
        .await
        .unwrap();
    SchemaContractRepository::attach(&mut tx, BindingOwner::Execution, execution.id, &binding)
        .await
        .unwrap();
    let items = WorkQueueItemRepository::lease_next_batch(
        &mut *tx,
        LeaseWorkQueueItemsInput {
            queue: queue.id,
            ready_statuses: vec![WorkQueueItemStatus::Queued],
            limit: 1,
            separate_requesters: false,
            batch_coalescing: None,
            leased_execution: Some(execution.id),
            lease_token: Uuid::new_v4(),
            lease_expires_at: Utc::now() + Duration::minutes(5),
        },
    )
    .await
    .unwrap();
    Contributors::capture_queue_execution(&mut tx, execution.id, &[item.id])
        .await
        .unwrap();
    Batch::capture_managed(&mut tx, record.id, &queue, &items, None, QUEUE_KEY)
        .await
        .unwrap();
    let worker = WorkerRepository::create(
        &mut *tx,
        CreateWorkerInput {
            name: "recorded-queue-completion".into(),
            worker_type: WorkerType::Local,
            runtime: None,
            host: None,
            port: None,
            status: Some(WorkerStatus::Active),
            capabilities: None,
            meta: None,
        },
    )
    .await
    .unwrap();
    let running = ExecutionRepository::update(
        &mut *tx,
        execution.id,
        UpdateExecutionInput {
            status: Some(ExecutionStatus::Running),
            worker: Some(worker.id),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let delivery = Delivery::register(
        &mut tx,
        execution.id,
        worker.id,
        Uuid::new_v4(),
        Utc::now() + Duration::minutes(5),
    )
    .await
    .unwrap();

    let mut changed = queue.clone();
    changed.config = json!({"ack_contract":{"version":9},"dispatch":{"retry_limit":1}});
    let accepted = Output::accepted_completion_config(&mut tx, &changed, &running, &items)
        .await
        .unwrap();
    assert_eq!((accepted.ack_version(), accepted.retry_limit()), (2, 3));
    assert!(
        Output::accepted_completion_config(&mut tx, &queue, &running, &[])
            .await
            .is_err()
    );
    let mut other_lease = items.clone();
    other_lease[0].lease_token = Some(Uuid::new_v4());
    assert!(
        Output::accepted_completion_config(&mut tx, &queue, &running, &other_lease)
            .await
            .is_err()
    );
    assert!(
        Output::recorded_ack_controls(&mut tx, &accepted, Some(QUEUE_KEY))
            .await
            .is_err()
    );
    let ack = json!({"version":2,"items":[{"id":items[0].id,"status":"completed","summary":{"token":"dummy-ack-secret"}}]});
    let result = json!({"data":{"queue_ack":ack},"queue_ack":ack});
    let mut mismatched = delivery.clone();
    mismatched.worker_instance = Uuid::new_v4();
    assert!(Delivery::complete_result(
        &mut tx,
        &mismatched,
        ExecutionStatus::Completed,
        result.clone(),
        QUEUE_KEY
    )
    .await
    .is_err());
    assert_eq!(
        ExecutionRepository::find_by_id(&mut *tx, execution.id)
            .await
            .unwrap()
            .unwrap()
            .status,
        ExecutionStatus::Running
    );
    let completed = Delivery::complete_result(
        &mut tx,
        &delivery,
        ExecutionStatus::Completed,
        result.clone(),
        QUEUE_KEY,
    )
    .await
    .unwrap();
    assert_eq!(completed.execution.status, ExecutionStatus::Completed);
    assert!(!completed
        .execution
        .result
        .unwrap()
        .to_string()
        .contains("dummy-ack-secret"));
    let (control, origins) = Output::recorded_ack_controls(&mut tx, &accepted, Some(QUEUE_KEY))
        .await
        .unwrap();
    let ProtectedQueueAck::Controls(control) = control else {
        panic!("typed recorded acknowledgement expected")
    };
    assert_eq!(
        control.items[0].summary.as_ref().unwrap()["token"],
        "dummy-ack-secret"
    );
    origins.ensure_fresh_on(&mut tx).await.unwrap();
    assert!(Delivery::complete_result(
        &mut tx,
        &delivery,
        ExecutionStatus::Completed,
        result,
        QUEUE_KEY
    )
    .await
    .is_err());
    tx.commit().await.unwrap();

    let mut next = db.begin().await.unwrap();
    assert!(
        Output::recorded_ack_controls(&mut next, &accepted, Some(QUEUE_KEY))
            .await
            .is_err()
    );
    let protected = Output::accepted_completion_config(&mut next, &queue, &running, &items)
        .await
        .unwrap();
    let (control, origins) = Output::recorded_ack_controls(&mut next, &protected, Some(QUEUE_KEY))
        .await
        .unwrap();
    let ProtectedQueueAck::Controls(control) = control else {
        panic!("persisted recorded acknowledgement expected")
    };
    let metadata = Output::prepare_item_metadata(
        &mut next,
        execution.id,
        0,
        WorkQueueItemStatus::Completed,
        json!({"status":"completed","summary":control.items[0].summary}),
        None,
        Some(QUEUE_KEY),
    )
    .await
    .unwrap();
    assert!(attune_common::secret_values::is_redaction_marker(
        &metadata.summary["summary"]
    ));
    let item = WorkQueueItemRepository::update_if_statuses(
        &mut *next,
        items[0].id,
        &[WorkQueueItemStatus::Leased],
        UpdateWorkQueueItemInput {
            status: Some(WorkQueueItemStatus::Completed),
            leased_execution: Some(Patch::Clear),
            lease_token: Some(Patch::Clear),
            lease_expires_at: Some(Patch::Clear),
            ack_summary: Some(Patch::Set(metadata.summary.clone())),
            ..Default::default()
        },
    )
    .await
    .unwrap()
    .unwrap();
    Output::persist_item_metadata(&mut next, item.id, &metadata)
        .await
        .unwrap();
    protected.ensure_fresh_on(&mut next).await.unwrap();
    origins.ensure_fresh_on(&mut next).await.unwrap();
    next.commit().await.unwrap();
    let mut terminal = db.begin().await.unwrap();
    assert!(
        Output::accepted_completion_config(&mut terminal, &queue, &running, &items)
            .await
            .is_err()
    );
    assert_eq!(item.status, WorkQueueItemStatus::Completed);
    assert!(!item
        .ack_summary
        .unwrap()
        .to_string()
        .contains("dummy-ack-secret"));
    terminal.rollback().await.unwrap();
    db.cleanup().await.unwrap();
}
