use super::*;
use attune_common::{
    models::Execution,
    mq::publisher::classify_confirmation,
    repositories::{
        action::{ActionRepository, UpdateActionInput},
        declared_input_materialization::DeclaredInputMaterializationRepository as Inputs,
        execution_secret_value::ExecutionSecretValueRepository as Secrets,
        maintenance::MaintenanceRepository as Maintenance,
        recorded_input_authority::RecordedInputAuthorityRepository as Contributors,
        requested_execution_publication::RequestedExecutionPublicationRepository as Requested,
        schema_contract::{BindingOwner, SchemaContractRepository as Schemas},
        FindById,
    },
    secret_values::{
        prepare_secret_values, redaction_marker, SecretSource, SecretValueInput,
        ENTITY_EXECUTION_CONFIG,
    },
};

const KEY: &str = "requested-publication-test-owned-key";

#[tokio::test]
async fn first_workflow_attempt_recovery_rejects_a_leaf_identity_as_the_admitted_workflow_source() {
    let (db, child) =
        super::native_retry_materialization::requested_first_attempt_with_stale_source_fixture()
            .await;
    let mut tx = db.begin().await.unwrap();
    let denied = matches!(Requested::prepare(&mut tx, &child, "native-retry-test-owned-encryption-key").await,
        Err(attune_common::Error::PermissionDenied(message)) if message == "Retained workflow source differs from its admitted graph");
    tx.rollback().await.unwrap();
    db.cleanup().await.unwrap();
    assert!(denied);
}

#[tokio::test]
async fn first_workflow_attempt_recovery_preserves_current_task_and_pause_guards() {
    use attune_common::repositories::workflow::{
        UpdateWorkflowExecutionInput, WorkflowExecutionRepository,
    };
    for case in 0..3 {
        let (db, child, workflow) =
            super::native_retry_materialization::requested_first_attempt_fixture().await;
        if case != 0 {
            WorkflowExecutionRepository::update(
                &db,
                workflow,
                UpdateWorkflowExecutionInput {
                    paused: (case == 1).then_some(true),
                    current_tasks: (case == 2).then_some(vec![]),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        }
        let mut tx = db.begin().await.unwrap();
        let prepared =
            Requested::prepare(&mut tx, &child, "native-retry-test-owned-encryption-key")
                .await
                .unwrap()
                .unwrap();
        let locked = prepared.lock_target(&mut tx).await;
        if case == 0 {
            locked.unwrap();
            prepared
                .publish_bounded(&mut tx, async { Ok(()) })
                .await
                .unwrap();
            tx.commit().await.unwrap();
        } else {
            assert!(locked.is_err());
            tx.rollback().await.unwrap();
        }
        db.cleanup().await.unwrap();
    }
}

#[tokio::test]
async fn workflow_retry_republication_delegates_existing_continuation_and_receipt_guards() {
    let (db, retry) = super::native_retry_materialization::requested_retry_fixture().await;
    let mut tx = db.begin().await.unwrap();
    let prepared = Requested::prepare(&mut tx, &retry, "native-retry-test-owned-encryption-key")
        .await
        .unwrap()
        .unwrap();
    prepared.lock_target(&mut tx).await.unwrap();
    prepared
        .publish_bounded(&mut tx, async { Ok(()) })
        .await
        .unwrap();
    tx.commit().await.unwrap();
    db.cleanup().await.unwrap();
}
struct Root {
    db: TestDatabase,
    base: Fixture,
    execution: Execution,
}

async fn root(seal: bool, lifetime: Option<Duration>) -> Root {
    root_with_secret(seal, lifetime, true).await
}

async fn root_with_secret(seal: bool, lifetime: Option<Duration>, secret: bool) -> Root {
    let (db, base) = fixture_with_approval_lifetime(
        Some(json!({"token":{"type":"string","secret":true},"public":{"type":"string"}})),
        None,
        lifetime,
    )
    .await;
    let mut tx = db.begin().await.unwrap();
    let binding = &base.policy.policy.invocations[0].binding;
    let snapshot = attune_common::repositories::managed_executable::ManagedExecutableRepository::resolve_invocation(&mut tx, &base.policy.policy, binding).await.unwrap();
    let refs = base.admission.authority.selected_refs.clone();
    let selected = base
        .policy
        .policy
        .select(
            base.policy.id,
            binding,
            &refs,
            base.admission.authority.attribution.clone(),
        )
        .unwrap();
    let execution = ExecutionRepository::create_pinned(
        &mut *tx,
        CreateExecutionInput {
            action: Some(snapshot.executable.action.id),
            action_ref: snapshot.executable.action.r#ref.clone(),
            status: ExecutionStatus::Requested,
            executor: Some(base.human_id),
            permission_set_refs: refs,
            config: Some(if secret {
                json!({"token":redaction_marker(),"public":"original"})
            } else {
                json!({"public":"original"})
            }),
            ..Default::default()
        },
        &snapshot,
    )
    .await
    .unwrap();
    RuntimeAuthorityRepository::record_admission(
        &mut tx,
        RuntimeOwner::Execution(execution.id),
        &selected,
    )
    .await
    .unwrap();
    let schema = Schemas::materialize_action_pair(&mut tx, &snapshot)
        .await
        .unwrap();
    Schemas::attach(&mut tx, BindingOwner::Execution, execution.id, &schema)
        .await
        .unwrap();
    Contributors::capture_human(&mut tx, "execution", execution.id, base.human_id)
        .await
        .unwrap();
    let fields = prepare_secret_values(
        vec![SecretValueInput {
            json_path: "/token".into(),
            value: json!("{{ literal_not_a_new_declaration }}"),
            source_kind: "provenance".into(),
            source_ref: Some(
                serde_json::to_string(
                    &SecretSource::ParameterSchema {
                        path: "/token".into(),
                    }
                    .provenance(),
                )
                .unwrap(),
            ),
        }],
        KEY,
    )
    .unwrap();
    if secret {
        Secrets::upsert_many_with_conn(&mut tx, ENTITY_EXECUTION_CONFIG, execution.id, &fields)
            .await
            .unwrap();
    }
    if seal {
        Inputs::seal_execution_inputs(&mut tx, execution.id, KEY)
            .await
            .unwrap();
    }
    tx.commit().await.unwrap();
    Root {
        db,
        base,
        execution,
    }
}

#[tokio::test]
async fn admitted_nonworkflow_requested_recovery_keeps_original_attempt_after_freeze_and_disablement(
) {
    let root = root(true, None).await;
    assert!(root.execution.workflow_task.is_none());
    IdentityRepository::update(
        &root.db,
        root.base.human_id,
        UpdateIdentityInput {
            frozen: Some(true),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    ActionRepository::update(
        &root.db,
        root.execution.action.unwrap(),
        UpdateActionInput {
            enabled: Some(false),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let before =
        Secrets::find_stored_by_entity(&root.db, ENTITY_EXECUTION_CONFIG, root.execution.id)
            .await
            .unwrap();
    let mut tx = root.db.begin().await.unwrap();
    let admission = RuntimeAuthorityRepository::execution_admission(&mut tx, root.execution.id)
        .await
        .unwrap()
        .unwrap();
    let prepared = Requested::prepare(&mut tx, &root.execution, KEY)
        .await
        .unwrap()
        .unwrap();
    prepared.lock_target(&mut tx).await.unwrap();
    Maintenance::mark_execution_reschedule_attempt(
        &mut *tx,
        root.execution.id,
        "requested-test",
        "recover same root",
        3,
        0,
        false,
    )
    .await
    .unwrap()
    .unwrap();
    prepared
        .publish_bounded(&mut tx, async {
            classify_confirmation(true, lapin::Confirmation::Ack(None)).map(|_| ())
        })
        .await
        .unwrap();
    let after = RuntimeAuthorityRepository::execution_admission(&mut tx, root.execution.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(admission.id, after.id);
    assert_eq!(admission.authority.attempt, after.authority.attempt);
    assert_eq!(
        Secrets::find_stored_by_entity(&mut *tx, ENTITY_EXECUTION_CONFIG, root.execution.id)
            .await
            .unwrap(),
        before
    );
    tx.commit().await.unwrap();
    root.db.cleanup().await.unwrap();
}

#[tokio::test]
async fn existing_row_recovery_neither_repairs_missing_acceptance_nor_accepts_config_or_ciphertext_drift(
) {
    for case in 0..3 {
        let root = root(case != 0, None).await;
        if case == 1 {
            sqlx::query("UPDATE execution SET config=jsonb_set(config,'{public}','\"changed\"') WHERE id=$1").bind(root.execution.id).execute(&*root.db).await.unwrap();
        } else if case == 2 {
            sqlx::query("UPDATE execution_secret_value SET encrypted_value='{}' WHERE entity_type='execution_config' AND entity_id=$1").bind(root.execution.id).execute(&*root.db).await.unwrap();
        }
        let actual = ExecutionRepository::find_by_id(&root.db, root.execution.id)
            .await
            .unwrap()
            .unwrap();
        let mut tx = root.db.begin().await.unwrap();
        assert!(Requested::prepare(&mut tx, &actual, KEY).await.is_err());
        tx.rollback().await.unwrap();
        root.db.cleanup().await.unwrap();
    }
}

#[tokio::test]
async fn cancelled_target_and_revoked_original_admission_cannot_publish() {
    for revoked in [false, true] {
        let root = root(true, None).await;
        let mut tx = root.db.begin().await.unwrap();
        if revoked {
            RuntimeAuthorityRepository::revoke_approval(&mut tx, root.base.approval.id)
                .await
                .unwrap();
        } else {
            ExecutionRepository::update(
                &mut *tx,
                root.execution.id,
                UpdateExecutionInput {
                    status: Some(ExecutionStatus::Cancelled),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        }
        tx.commit().await.unwrap();
        let actual = ExecutionRepository::find_by_id(&root.db, root.execution.id)
            .await
            .unwrap()
            .unwrap();
        let mut tx = root.db.begin().await.unwrap();
        assert!(matches!(
            Requested::prepare(&mut tx, &actual, KEY).await,
            Ok(None) | Err(_)
        ));
        tx.rollback().await.unwrap();
        root.db.cleanup().await.unwrap();
    }
}

#[tokio::test]
async fn two_prepared_source_holders_never_deadlock_on_the_requested_target_upgrade() {
    let root = root(true, None).await;
    let mut a = root.db.begin().await.unwrap();
    let pa = Requested::prepare(&mut a, &root.execution, KEY)
        .await
        .unwrap()
        .unwrap();
    let mut b = root.db.begin().await.unwrap();
    let pb = Requested::prepare(&mut b, &root.execution, KEY)
        .await
        .unwrap()
        .unwrap();
    let busy =
        tokio::time::timeout(std::time::Duration::from_secs(1), pa.lock_target(&mut a)).await;
    a.rollback().await.unwrap();
    assert!(matches!(
        busy,
        Ok(Err(attune_common::Error::InvalidState(_)))
    ));
    pb.lock_target(&mut b).await.unwrap();
    pb.publish_bounded(&mut b, async { Ok(()) }).await.unwrap();
    b.commit().await.unwrap();
    root.db.cleanup().await.unwrap();
}

#[tokio::test]
async fn prepared_requested_source_cannot_move_transactions_or_publish_without_target_lock() {
    let root = root(true, None).await;
    let mut a = root.db.begin().await.unwrap();
    let prepared = Requested::prepare(&mut a, &root.execution, KEY)
        .await
        .unwrap()
        .unwrap();
    assert!(prepared
        .publish_bounded(&mut a, async { Ok(()) })
        .await
        .is_err());
    let mut b = root.db.begin().await.unwrap();
    assert!(prepared.lock_target(&mut b).await.is_err());
    b.rollback().await.unwrap();
    a.rollback().await.unwrap();
    root.db.cleanup().await.unwrap();
}

#[tokio::test]
async fn zero_receipt_public_root_still_binds_original_source_transaction() {
    let root = root_with_secret(true, None, false).await;
    assert!(
        Secrets::find_stored_by_entity(&root.db, ENTITY_EXECUTION_CONFIG, root.execution.id)
            .await
            .unwrap()
            .is_empty()
    );
    let mut a = root.db.begin().await.unwrap();
    let prepared = Requested::prepare(&mut a, &root.execution, KEY)
        .await
        .unwrap()
        .unwrap();
    a.rollback().await.unwrap();
    let mut b = root.db.begin().await.unwrap();
    assert!(prepared.lock_target(&mut b).await.is_err());
    b.rollback().await.unwrap();
    let mut c = root.db.begin().await.unwrap();
    let fresh = Requested::prepare(&mut c, &root.execution, KEY)
        .await
        .unwrap()
        .unwrap();
    fresh.lock_target(&mut c).await.unwrap();
    fresh
        .publish_bounded(&mut c, async { Ok(()) })
        .await
        .unwrap();
    c.commit().await.unwrap();
    root.db.cleanup().await.unwrap();
}

#[tokio::test]
async fn requested_root_nack_or_missing_confirmation_rolls_back_the_real_mark() {
    for confirmation in [
        lapin::Confirmation::Nack(None),
        lapin::Confirmation::NotRequested,
    ] {
        let root = root(true, None).await;
        let mut tx = root.db.begin().await.unwrap();
        let prepared = Requested::prepare(&mut tx, &root.execution, KEY)
            .await
            .unwrap()
            .unwrap();
        prepared.lock_target(&mut tx).await.unwrap();
        Maintenance::mark_execution_reschedule_attempt(
            &mut *tx,
            root.execution.id,
            "requested-test",
            "negative confirmation",
            3,
            0,
            false,
        )
        .await
        .unwrap()
        .unwrap();
        assert!(prepared
            .publish_bounded(&mut tx, async move {
                classify_confirmation(true, confirmation).map(|_| ())
            })
            .await
            .is_err());
        tx.rollback().await.unwrap();
        let marks: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM execution_reschedule_state WHERE execution_id=$1",
        )
        .bind(root.execution.id)
        .fetch_one(&*root.db)
        .await
        .unwrap();
        assert_eq!(marks, 0);
        root.db.cleanup().await.unwrap();
    }
}

#[tokio::test]
async fn policy_queue_promotion_keeps_the_original_target_proof_after_unknown_publication() {
    use attune_common::repositories::execution_admission::{
        AdmissionEnqueueOutcome, ExecutionAdmissionRepository as Slots,
    };
    let root = root(true, None).await;
    let action = root.execution.action.unwrap();
    let active = root.base.credential.execution_id;
    assert!(
        Slots::try_acquire(root.db.pool(), action, active, 1, None)
            .await
            .unwrap()
            .acquired
    );
    assert!(matches!(
        Slots::enqueue(root.db.pool(), 10, action, root.execution.id, 1, None)
            .await
            .unwrap(),
        AdmissionEnqueueOutcome::Enqueued
    ));
    assert_eq!(
        Slots::wait_status(root.db.pool(), root.execution.id)
            .await
            .unwrap(),
        Some(false)
    );
    let before =
        Secrets::find_stored_by_entity(&root.db, ENTITY_EXECUTION_CONFIG, root.execution.id)
            .await
            .unwrap();
    let mut tx = root.db.begin().await.unwrap();
    let original = RuntimeAuthorityRepository::execution_admission(&mut tx, root.execution.id)
        .await
        .unwrap()
        .unwrap();
    tx.rollback().await.unwrap();
    // The actual slot-release repository commits promotion before the
    // completion-listener's separate Requested publication transaction.
    let release = Slots::release_active_slot(root.db.pool(), active)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(release.action_id, action);
    assert_eq!(release.next_execution_id, Some(root.execution.id));
    let mut publication = root.db.begin().await.unwrap();
    let prepared = Requested::prepare(&mut publication, &root.execution, KEY)
        .await
        .unwrap()
        .unwrap();
    prepared.lock_target(&mut publication).await.unwrap();
    assert!(prepared
        .publish_bounded(&mut publication, async {
            classify_confirmation(true, lapin::Confirmation::NotRequested).map(|_| ())
        })
        .await
        .is_err());
    publication.rollback().await.unwrap();
    assert_eq!(
        Slots::wait_status(root.db.pool(), root.execution.id)
            .await
            .unwrap(),
        Some(true)
    );
    let mut recovery = root.db.begin().await.unwrap();
    let prepared = Requested::prepare(&mut recovery, &root.execution, KEY)
        .await
        .unwrap()
        .unwrap();
    prepared.lock_target(&mut recovery).await.unwrap();
    prepared
        .publish_bounded(&mut recovery, async {
            classify_confirmation(true, lapin::Confirmation::Ack(None)).map(|_| ())
        })
        .await
        .unwrap();
    let after = RuntimeAuthorityRepository::execution_admission(&mut recovery, root.execution.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(after.id, original.id);
    assert_eq!(after.authority.attempt, original.authority.attempt);
    assert_eq!(
        Secrets::find_stored_by_entity(&mut *recovery, ENTITY_EXECUTION_CONFIG, root.execution.id)
            .await
            .unwrap(),
        before
    );
    recovery.commit().await.unwrap();
    root.db.cleanup().await.unwrap();
}

#[tokio::test]
async fn policy_queue_release_does_not_authorize_a_subsequently_cancelled_target() {
    use attune_common::repositories::execution_admission::ExecutionAdmissionRepository as Slots;
    let root = root(true, None).await;
    let action = root.execution.action.unwrap();
    let active = root.base.credential.execution_id;
    assert!(
        Slots::try_acquire(root.db.pool(), action, active, 1, None)
            .await
            .unwrap()
            .acquired
    );
    Slots::enqueue(root.db.pool(), 10, action, root.execution.id, 1, None)
        .await
        .unwrap();
    let release = Slots::release_active_slot(root.db.pool(), active)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(release.next_execution_id, Some(root.execution.id));
    let cancelled = ExecutionRepository::update(
        &root.db,
        root.execution.id,
        UpdateExecutionInput {
            status: Some(ExecutionStatus::Cancelled),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let mut publication = root.db.begin().await.unwrap();
    assert!(Requested::prepare(&mut publication, &cancelled, KEY)
        .await
        .unwrap()
        .is_none());
    publication.rollback().await.unwrap();
    root.db.cleanup().await.unwrap();
}

#[tokio::test]
async fn target_approval_expiry_after_real_reschedule_state_wait_prevents_publication_and_rolls_back_mark(
) {
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    };
    let root = root(true, Some(Duration::seconds(15))).await;
    Maintenance::mark_execution_reschedule_attempt(
        root.db.pool(),
        root.execution.id,
        "requested-test",
        "existing mark",
        3,
        0,
        true,
    )
    .await
    .unwrap()
    .unwrap();
    let mut blocker = root.db.begin().await.unwrap();
    let blocker_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *blocker)
        .await
        .unwrap();
    sqlx::query(
        "SELECT execution_id FROM execution_reschedule_state WHERE execution_id=$1 FOR UPDATE",
    )
    .bind(root.execution.id)
    .fetch_one(&mut *blocker)
    .await
    .unwrap();
    let pool = root.db.pool().clone();
    let execution = root.execution.clone();
    let published = Arc::new(AtomicBool::new(false));
    let sent = published.clone();
    let (ready, source_ready) = tokio::sync::oneshot::channel();
    let mut writer = tokio::spawn(async move {
        let mut tx = pool.begin().await.unwrap();
        let prepared = Requested::prepare(&mut tx, &execution, KEY)
            .await
            .unwrap()
            .unwrap();
        prepared.lock_target(&mut tx).await.unwrap();
        let _ = ready.send(());
        Maintenance::mark_execution_reschedule_attempt(
            &mut *tx,
            execution.id,
            "requested-test",
            "waited target",
            3,
            0,
            true,
        )
        .await
        .unwrap()
        .unwrap();
        let denied = prepared
            .publish_bounded(&mut tx, async move {
                sent.store(true, Ordering::SeqCst);
                Ok(())
            })
            .await
            .is_err();
        tx.rollback().await.unwrap();
        denied
    });
    let ready = tokio::time::timeout(std::time::Duration::from_secs(5), source_ready)
        .await
        .is_ok();
    let waited = tokio::time::timeout(std::time::Duration::from_secs(3), async {
        loop {
            let waiting: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE $1=ANY(pg_blocking_pids(pid)) AND query LIKE '%execution_reschedule_state%')")
                .bind(blocker_pid).fetch_one(root.db.pool()).await.unwrap();
            if waiting { break; }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    }).await.is_ok();
    let expired = tokio::time::timeout(std::time::Duration::from_secs(20), async {
        loop {
            let expired: bool = sqlx::query_scalar(
                "SELECT clock_timestamp() >= expires_at FROM runtime_external_approval WHERE id=$1",
            )
            .bind(root.base.approval.id)
            .fetch_one(root.db.pool())
            .await
            .unwrap();
            if expired {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .is_ok();
    blocker.commit().await.unwrap();
    let denied = match tokio::time::timeout(std::time::Duration::from_secs(5), &mut writer).await {
        Ok(Ok(denied)) => denied,
        Ok(Err(_)) => false,
        Err(_) => {
            writer.abort();
            let _ = writer.await;
            false
        }
    };
    let count: i32 = sqlx::query_scalar(
        "SELECT attempt_count FROM execution_reschedule_state WHERE execution_id=$1",
    )
    .bind(root.execution.id)
    .fetch_one(root.db.pool())
    .await
    .unwrap();
    root.db.cleanup().await.unwrap();
    assert!(ready && waited && expired);
    assert!(denied);
    assert!(!published.load(Ordering::SeqCst));
    assert_eq!(count, 1);
}
