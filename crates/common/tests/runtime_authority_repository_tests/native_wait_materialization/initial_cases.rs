use super::*;
use attune_common::repositories::native_wait_materialization::{
    InitialWaitApply, NativeInitialWaitMaterializationRepository as Initial,
};
use attune_common::repositories::Update;

#[derive(Clone, Copy)]
pub(crate) enum InitialCase {
    Terminal,
    InProgress,
    WrongTransaction,
    MissingRelease,
    ConcurrentIdentical,
    Conflicting,
    Expiry,
    CancelledParent,
    MissingReservation,
    Recovery(&'static str),
    RecoveryExpiry,
    RecoveryArray,
}

pub(crate) async fn initial_lifecycle(
    pool: &sqlx::PgPool,
    workflow: Id,
    parent: Id,
    task: &str,
    target: Target,
    key: &str,
    mode: InitialCase,
    expiry: chrono::DateTime<Utc>,
) {
    if let InitialCase::Recovery(state) = mode {
        initial_recovery(
            pool, workflow, parent, task, target, key, state, None, false,
        )
        .await;
        return;
    }
    if matches!(mode, InitialCase::RecoveryExpiry) {
        initial_recovery(
            pool,
            workflow,
            parent,
            task,
            target,
            key,
            "failed",
            Some(expiry),
            false,
        )
        .await;
        return;
    }
    if matches!(mode, InitialCase::RecoveryArray) {
        initial_recovery(
            pool, workflow, parent, task, target, key, "failed", None, true,
        )
        .await;
        return;
    }
    if matches!(mode, InitialCase::InProgress) {
        let mut tx = pool.begin().await.unwrap();
        status(
            &mut tx,
            target,
            if matches!(target, Target::Inquiry(_)) {
                "pending"
            } else {
                "running"
            },
        )
        .await;
        tx.commit().await.unwrap();
    }
    let mut tx = pool.begin().await.unwrap();
    protect(&mut tx).await;
    if matches!(mode, InitialCase::MissingRelease) {
        missing_release(&mut tx, workflow, parent, task, target, key).await;
        tx.rollback().await.unwrap();
        assert!(Waits::find_by_workflow_task(pool, workflow, task)
            .await
            .unwrap()
            .is_none());
        return;
    }
    let mut plan = Initial::prepare(&mut tx, workflow, task, Some(key), None)
        .await
        .unwrap()
        .unwrap();
    assert!(
        Waits::find_by_workflow_task(&mut *tx, workflow, task)
            .await
            .unwrap()
            .is_none(),
        "source preparation must not acquire the wait's workflow FK by inserting a bootstrap row"
    );
    if matches!(mode, InitialCase::WrongTransaction) {
        let mut other = pool.begin().await.unwrap();
        // Reject the foreign transaction before taking any target/admission
        // locks. Reacquiring the owning transaction's exclusive cache gate here
        // would deadlock the fixture before it reaches the identity check.
        let rejection = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            plan.apply(&mut other, key),
        )
        .await
        .expect("foreign transaction rejection must not wait for the source owner");
        assert!(matches!(
            rejection,
            Err(attune_common::Error::PermissionDenied(_))
        ));
        other.rollback().await.unwrap();
        tx.rollback().await.unwrap();
        assert!(Waits::find_by_workflow_task(pool, workflow, task)
            .await
            .unwrap()
            .is_none());
        return;
    }
    if matches!(
        mode,
        InitialCase::CancelledParent | InitialCase::MissingReservation
    ) {
        if matches!(mode, InitialCase::CancelledParent) {
            ExecutionRepository::update(
                &mut *tx,
                parent,
                attune_common::repositories::execution::UpdateExecutionInput {
                    status: Some(ExecutionStatus::Cancelled),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        } else {
            let removed=sqlx::query("DELETE FROM runtime_native_wait_reservation WHERE prepared_transaction=pg_current_xact_id()::TEXT")
                .execute(&mut *tx).await.unwrap().rows_affected();
            assert_eq!(removed, 1);
        }
        assert!(plan.apply(&mut tx, key).await.is_err());
        tx.rollback().await.unwrap();
        assert!(Waits::find_by_workflow_task(pool, workflow, task)
            .await
            .unwrap()
            .is_none());
        assert!(
            ExecutionRepository::find_by_workflow_task(pool, workflow, task, None)
                .await
                .unwrap()
                .is_none()
        );
        return;
    }
    if matches!(mode, InitialCase::Conflicting) {
        let competing_pool = pool.clone();
        let competing_task = task.to_owned();
        let mut competing = tokio::spawn(async move {
            let mut tx = competing_pool.begin().await.unwrap();
            let wait = Waits::create_or_get(
                &mut tx,
                attune_common::repositories::workflow_task_wait::CreateWorkflowTaskWaitInput {
                    workflow_execution: workflow,
                    task_name: competing_task,
                    target: Target::Execution(parent),
                },
            )
            .await
            .unwrap();
            tx.commit().await.unwrap();
            wait
        });
        let joined = tokio::time::timeout(std::time::Duration::from_secs(30), &mut competing).await;
        if joined.is_err() {
            competing.abort();
            let _ = competing.await;
            tx.rollback().await.unwrap();
            panic!("competing registration did not finish before any owning workflow target lock");
        }
        let other = joined.unwrap().unwrap();
        assert!(plan.apply(&mut tx, key).await.is_err());
        tx.rollback().await.unwrap();
        assert_eq!(
            Waits::find_by_workflow_task(pool, workflow, task)
                .await
                .unwrap()
                .unwrap()
                .id,
            other.id
        );
        assert!(
            ExecutionRepository::find_by_workflow_task(pool, workflow, task, None)
                .await
                .unwrap()
                .is_none()
        );
        let registrations: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM runtime_native_wait_registration WHERE wait=$1",
        )
        .bind(other.id)
        .fetch_one(pool)
        .await
        .unwrap();
        assert_eq!(registrations, 0);
        return;
    }
    if matches!(mode, InitialCase::Expiry) {
        observed_expiry(pool, &mut tx, workflow, &mut plan, key, expiry).await;
        tx.rollback().await.unwrap();
        assert!(Waits::find_by_workflow_task(pool, workflow, task)
            .await
            .unwrap()
            .is_none());
        assert!(
            ExecutionRepository::find_by_workflow_task(pool, workflow, task, None)
                .await
                .unwrap()
                .is_none()
        );
        return;
    }
    let mut concurrent = None;
    if matches!(mode, InitialCase::ConcurrentIdentical) {
        let own_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *tx)
            .await
            .unwrap();
        let (send, receive) = tokio::sync::oneshot::channel();
        let worker_pool = pool.clone();
        let worker_task = task.to_owned();
        let worker_key = key.to_owned();
        let handle = tokio::spawn(async move {
            let mut tx = worker_pool.begin().await.unwrap();
            let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
                .fetch_one(&mut *tx)
                .await
                .unwrap();
            send.send(pid).unwrap();
            protect(&mut tx).await;
            let rejected =
                Initial::prepare(&mut tx, workflow, &worker_task, Some(&worker_key), None).await;
            assert!(
                matches!(rejected, Err(attune_common::Error::InvalidState(ref message))
                if message == "Initial native wait already registered; use the registered checkpoint")
            );
            let registered = Waits::find_by_workflow_task(&mut *tx, workflow, &worker_task)
                .await
                .unwrap()
                .unwrap();
            let existing =
                ExecutionRepository::find_by_workflow_task(&mut *tx, workflow, &worker_task, None)
                    .await
                    .unwrap()
                    .unwrap();
            let mut checkpoint = Delivery::prepare(&mut tx, &registered, Some(&worker_key))
                .await
                .unwrap()
                .unwrap();
            let replayed = checkpoint
                .apply(&mut tx, &worker_key)
                .await
                .unwrap()
                .unwrap();
            assert_eq!(
                replayed.id, existing.id,
                "the registration loser must reuse the original admitted child"
            );
            assert_eq!(replayed.config, existing.config);
            checkpoint.ensure_fresh_on(&mut tx).await.unwrap();
            tx.rollback().await.unwrap();
        });
        let pid = match tokio::time::timeout(std::time::Duration::from_secs(5), receive).await {
            Ok(Ok(pid)) => pid,
            other => {
                handle.abort();
                let _ = handle.await;
                tx.rollback().await.unwrap();
                panic!("concurrent registration must announce its PID before admission: {other:?}");
            }
        };
        let observed = tokio::time::timeout(std::time::Duration::from_secs(30), async {
            loop {
                let blocked: bool = sqlx::query_scalar("SELECT $2 = ANY(pg_blocking_pids($1))")
                    .bind(pid)
                    .bind(own_pid)
                    .fetch_one(pool)
                    .await
                    .unwrap();
                if blocked {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await;
        if observed.is_err() {
            handle.abort();
            let _ = handle.await;
            tx.rollback().await.unwrap();
            panic!("no actual concurrent registration source wait was observed");
        }
        concurrent = Some(handle);
    }
    let InitialWaitApply::Registered(child) = plan.apply(&mut tx, key).await.unwrap() else {
        panic!("first actual registration wins");
    };
    let wait = Waits::find_by_workflow_task(&mut *tx, workflow, task)
        .await
        .unwrap()
        .unwrap();
    assert!(wait.id > 0);
    let original_tx:bool=sqlx::query_scalar("SELECT registration_transaction=pg_current_xact_id()::TEXT FROM runtime_native_wait_registration WHERE wait=$1")
        .bind(wait.id).fetch_one(&mut *tx).await.unwrap();
    assert!(original_tx);
    if matches!(mode, InitialCase::InProgress) {
        assert!(child.is_none());
        assert_eq!(
            wait.state,
            attune_common::models::WorkflowTaskWaitState::Waiting
        );
    } else {
        let child = child.as_ref().unwrap();
        original_child_acceptance(&mut tx, parent, child).await;
        let same_tx:bool=sqlx::query_scalar("SELECT release_transaction=pg_current_xact_id()::TEXT FROM runtime_native_wait_release WHERE wait=$1")
            .bind(wait.id).fetch_one(&mut *tx).await.unwrap();
        assert!(same_tx);
        let fields = Secrets::find_stored_by_entity(&mut *tx, "execution_config", child.id)
            .await
            .unwrap();
        let value = attune_common::secret_values::restore_secret_values(
            child.config.clone().unwrap(),
            &fields,
            key,
        )
        .unwrap();
        assert_eq!(
            value["token"],
            match target {
                Target::Execution(_) => "execution-original",
                Target::Inquiry(_) => "inquiry-original",
                Target::WorkQueueItem(_) => "queue-original",
            }
        );
    }
    plan.ensure_fresh_on(&mut tx).await.unwrap();
    tx.commit().await.unwrap();
    plan.ensure_fresh().unwrap();
    if matches!(mode, InitialCase::InProgress) && matches!(target, Target::Inquiry(_)) {
        let mut gc = pool.begin().await.unwrap();
        attune_common::repositories::file_evidence_retention::FileEvidenceRetentionRepository::lock_reclamation(&mut gc)
            .await.unwrap();
        attune_common::repositories::cache::CacheEntryRepository::protect_transaction(
            &mut gc,
            attune_common::repositories::cache::CacheTransactionMode::PinMutation,
        )
        .await
        .unwrap();
        let Target::Inquiry(inquiry) = target else {
            unreachable!()
        };
        let error =
            attune_common::repositories::inquiry::InquiryRepository::delete(&mut *gc, inquiry)
                .await
                .expect_err("a waiting inquiry must retain its physical target");
        assert!(
            error
                .to_string()
                .contains("workflow_task_wait_active_inquiry_fkey"),
            "{error}"
        );
        gc.rollback().await.unwrap();
    }
    if let Some(mut handle) = concurrent {
        let joined = tokio::time::timeout(std::time::Duration::from_secs(45), &mut handle).await;
        if joined.is_err() {
            handle.abort();
            let _ = handle.await;
            panic!("losing registration did not roll back");
        }
        joined.unwrap().unwrap();
    }
    if !matches!(mode, InitialCase::InProgress) {
        reclaim(pool, target).await;
    }
    let mut next = pool.begin().await.unwrap();
    protect(&mut next).await;
    if matches!(mode, InitialCase::InProgress) {
        let state = Workflows::find_by_id(&mut *next, workflow)
            .await
            .unwrap()
            .unwrap();
        let mut variables = state.variables;
        variables.as_object_mut().unwrap().insert(
            match target {
                Target::Execution(_) => "target_execution",
                Target::Inquiry(_) => "target_inquiry",
                Target::WorkQueueItem(_) => "target_queue",
            }
            .into(),
            json!("deliberately-not-a-target"),
        );
        Workflows::update(
            &mut *next,
            workflow,
            attune_common::repositories::workflow::UpdateWorkflowExecutionInput {
                variables: Some(variables),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        status(
            &mut next,
            target,
            if matches!(target, Target::Inquiry(_)) {
                "responded"
            } else {
                "completed"
            },
        )
        .await;
        next.commit().await.unwrap();
        next = pool.begin().await.unwrap();
        protect(&mut next).await;
    }
    let registered = Waits::find_by_workflow_task(&mut *next, workflow, task)
        .await
        .unwrap()
        .unwrap();
    let mut replay = Delivery::prepare(&mut next, &registered, Some(key))
        .await
        .unwrap()
        .unwrap();
    let delivered = replay.apply(&mut next, key).await.unwrap().unwrap();
    if let Some(child) = child {
        assert_eq!(delivered.id, child.id);
        assert_eq!(delivered.config, child.config);
    }
    replay.ensure_fresh_on(&mut next).await.unwrap();
    next.commit().await.unwrap();
    replay.ensure_fresh().unwrap();
    plan.ensure_fresh().unwrap();
    let count: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM workflow_task_wait WHERE workflow_execution=$1 AND task_name=$2",
    )
    .bind(workflow)
    .bind(task)
    .fetch_one(pool)
    .await
    .unwrap();
    assert_eq!(count, 1);
}

async fn initial_recovery(
    pool: &sqlx::PgPool,
    workflow: Id,
    parent: Id,
    task: &str,
    target: Target,
    key: &str,
    state: &str,
    expiry: Option<chrono::DateTime<Utc>>,
    arrays: bool,
) {
    let mut tx = pool.begin().await.unwrap();
    status(&mut tx, target, state).await;
    tx.commit().await.unwrap();
    let mut tx = pool.begin().await.unwrap();
    protect(&mut tx).await;
    let mut plan = Initial::prepare(&mut tx, workflow, task, Some(key), None)
        .await
        .unwrap()
        .expect("real prospective terminal recovery source");
    assert!(Waits::find_by_workflow_task(&mut *tx, workflow, task)
        .await
        .unwrap()
        .is_none());
    let reserved:Id=sqlx::query_scalar("SELECT wait FROM runtime_native_wait_reservation WHERE prepared_transaction=pg_current_xact_id()::TEXT")
        .fetch_one(&mut *tx).await.unwrap();
    assert!(reserved > 0);
    let published = Workflows::find_by_id(&mut *tx, workflow)
        .await
        .unwrap()
        .unwrap();
    assert!(published.variables.get("a").is_none() && published.variables.get("b").is_none());
    let released: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM runtime_native_wait_release WHERE wait=$1)",
    )
    .bind(reserved)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert!(!released);
    let stages:i64=sqlx::query_scalar("SELECT count(*) FROM runtime_input_receipt_staged s JOIN runtime_input_receipt r ON r.id=s.receipt WHERE r.execution=$1 AND s.prepared_transaction=pg_current_xact_id()::TEXT AND s.publication_transaction IS NULL")
        .bind(parent).fetch_one(&mut *tx).await.unwrap();
    assert_eq!(
        stages,
        if arrays {
            5
        } else if state == "failed" {
            2
        } else {
            1
        }
    );
    if let Some(expiry) = expiry {
        observed_expiry(pool, &mut tx, workflow, &mut plan, key, expiry).await;
        tx.rollback().await.unwrap();
        assert!(Waits::find_by_workflow_task(pool, workflow, task)
            .await
            .unwrap()
            .is_none());
        assert!(ExecutionRepository::find_by_workflow_task(
            pool,
            workflow,
            "native_recovery",
            None
        )
        .await
        .unwrap()
        .is_none());
        let values = Workflows::find_by_id(pool, workflow)
            .await
            .unwrap()
            .unwrap()
            .variables;
        assert!(values.get("a").is_none() && values.get("b").is_none());
        return;
    }
    let InitialWaitApply::Recovered(children) = plan.apply(&mut tx, key).await.unwrap() else {
        panic!("initial source must recover through the actual target binder");
    };
    assert_eq!(children.len(), 1);
    let child = &children[0];
    original_child_acceptance(&mut tx, parent, child).await;
    let wait = Waits::find_by_workflow_task(&mut *tx, workflow, task)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(wait.id, reserved);
    assert_eq!(plan.terminal_wait(), Some(reserved));
    let same_tx:bool=sqlx::query_scalar("SELECT registration.registration_transaction=pg_current_xact_id()::TEXT AND r.release_transaction=pg_current_xact_id()::TEXT FROM runtime_native_wait_registration registration JOIN runtime_native_wait_release r ON r.wait=registration.wait WHERE registration.wait=$1")
        .bind(reserved).fetch_one(&mut *tx).await.unwrap();
    assert!(same_tx);
    assert!(
        ExecutionRepository::find_by_workflow_task(&mut *tx, workflow, task, None)
            .await
            .unwrap()
            .is_none(),
        "no failure witness execution is invented for the owning wait"
    );
    let fields = Secrets::find_stored_by_entity(&mut *tx, "execution_config", child.id)
        .await
        .unwrap();
    let actual = attune_common::secret_values::restore_secret_values(
        child.config.clone().unwrap(),
        &fields,
        key,
    )
    .unwrap();
    assert_eq!(
        actual["token"],
        if state == "failed" {
            "execution-original"
        } else {
            "recovered"
        }
    );
    terminal_controls(
        &mut tx,
        reserved,
        parent,
        child,
        task,
        key,
        false,
        state == "failed",
    )
    .await;
    if arrays {
        let receipts=attune_common::repositories::declared_input_materialization::DeclaredInputMaterializationRepository::prepare_execution_input_receipts(&mut tx,child.id,key).await.unwrap();
        let old_a: std::collections::BTreeSet<_> = receipts
            .iter()
            .flat_map(|receipt| receipt.source_receipts())
            .filter(|source| source.site().execution == parent && source.site().path == "/a")
            .map(|source| source.receipt())
            .collect();
        let old_b: std::collections::BTreeSet<_> = receipts
            .iter()
            .flat_map(|receipt| receipt.source_receipts())
            .filter(|source| source.site().execution == parent && source.site().path == "/b")
            .map(|source| source.receipt())
            .collect();
        assert!(
            old_a.len() >= 2 && old_b.len() >= 2,
            "captured pre-overwrite VALUE history must not collapse into the final-name map"
        );
    }
    plan.ensure_fresh_on(&mut tx).await.unwrap();
    tx.commit().await.unwrap();
    plan.ensure_fresh().unwrap();
    reclaim(pool, target).await;
    let mut replay_tx = pool.begin().await.unwrap();
    protect(&mut replay_tx).await;
    let mut replay = Continue::prepare(&mut replay_tx, &wait, Some(key))
        .await
        .unwrap()
        .unwrap();
    let pending = replay.apply(&mut replay_tx, key).await.unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].id, child.id);
    assert_eq!(pending[0].config, child.config);
    replay.ensure_fresh_on(&mut replay_tx).await.unwrap();
    replay_tx.commit().await.unwrap();
    replay.ensure_fresh().unwrap();
}

async fn observed_expiry(
    pool: &sqlx::PgPool,
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    workflow: Id,
    plan: &mut attune_common::repositories::native_wait_materialization::PreparedNativeInitialWait,
    key: &str,
    expiry: chrono::DateTime<Utc>,
) {
    let mut blocker = pool.begin().await.unwrap();
    sqlx::query("SELECT id FROM workflow_execution WHERE id=$1 FOR UPDATE")
        .bind(workflow)
        .fetch_one(&mut *blocker)
        .await
        .unwrap();
    let blocker_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *blocker)
        .await
        .unwrap();
    let waiter: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut **tx)
        .await
        .unwrap();
    let pool = pool.clone();
    let release = tokio::spawn(async move {
        tokio::time::timeout(std::time::Duration::from_secs(60), async {
            let mut observed = false;
            loop {
                let blocked: bool = sqlx::query_scalar("SELECT $2=ANY(pg_blocking_pids($1))")
                    .bind(waiter)
                    .bind(blocker_pid)
                    .fetch_one(&pool)
                    .await
                    .unwrap();
                let expired: bool = sqlx::query_scalar("SELECT clock_timestamp()>=$1")
                    .bind(expiry)
                    .fetch_one(&pool)
                    .await
                    .unwrap();
                if blocked && !observed {
                    assert!(
                        !expired,
                        "initial approval expired before the observed workflow target wait"
                    );
                    observed = true;
                }
                if observed && blocked && expired {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        blocker.rollback().await.unwrap();
    });
    let result = tokio::time::timeout(std::time::Duration::from_secs(70), plan.apply(tx, key))
        .await
        .unwrap();
    release.await.unwrap();
    assert!(result.is_err());
}

async fn missing_release(
    conn: &mut sqlx::PgConnection,
    workflow: Id,
    parent: Id,
    task: &str,
    target: Target,
    key: &str,
) {
    use attune_common::repositories::{
        declared_input::DeclaredInputRepository as Declarations,
        declared_input_materialization::DeclaredInputMaterializationRepository as Materialize,
        execution_runtime_admission::ExecutionAdmissionRepository as Admissions,
        inherited_input::{
            InheritedInputRepository as Inputs, InputEntityKind, InputSite, InputSitePurpose,
        },
        native_input_snapshot::NativeInputSnapshotRepository as Snapshots,
        runtime_authority::RuntimeOwner,
        schema_contract::{BindingOwner, SchemaContractRepository as Schemas},
    };
    let root = ExecutionRepository::find_by_id(&mut *conn, parent)
        .await
        .unwrap()
        .unwrap();
    let graph = &root
        .executable_snapshot
        .as_ref()
        .unwrap()
        .executable
        .workflow_definition
        .as_ref()
        .unwrap()
        .definition;
    let node = graph["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["name"] == task)
        .unwrap();
    let action = node["action"].as_str().unwrap();
    let path = Materialize::workflow_task_site_path(conn, parent, task, action)
        .await
        .unwrap();
    let declarations = Declarations::bindings(conn, parent).await.unwrap();
    let binding = declarations
        .iter()
        .find(|binding| binding.source_path == format!("{path}/wait_for/execution"))
        .unwrap();
    let mut session = Inputs::begin_render(
        conn,
        parent,
        &binding.declaration_revision,
        &binding.source_path,
        &binding.expression,
        InputSitePurpose::Control,
    )
    .await
    .unwrap();
    let parameter = binding.expression.contains("parameters.");
    Inputs::bind_public_field(
        conn,
        &mut session,
        if parameter {
            "/parameters/target_execution"
        } else {
            "/target_execution"
        },
        &InputSite {
            execution: parent,
            kind: if parameter {
                InputEntityKind::ExecutionConfig
            } else {
                InputEntityKind::WorkflowVariables
            },
            purpose: InputSitePurpose::Value,
            path: "/target_execution".into(),
        },
    )
    .await
    .unwrap();
    let rendered = Inputs::evaluate_with_keys(conn, session, key)
        .await
        .unwrap();
    let receiving = Inputs::prepare_destination(
        conn,
        &rendered,
        &InputSite {
            execution: parent,
            kind: InputEntityKind::WorkflowVariables,
            purpose: InputSitePurpose::Control,
            path: binding.source_path.clone(),
        },
        key,
    )
    .await
    .unwrap();
    let control = Inputs::record_prepared(
        conn,
        &rendered,
        &receiving,
        rendered.value(),
        rendered.selections(),
        key,
    )
    .await
    .unwrap();
    let executable = Admissions::resolve_workflow_snapshot(conn, parent, task, action)
        .await
        .unwrap();
    let refs: Vec<String> = serde_json::from_value(node["permission_set_refs"].clone()).unwrap();
    let admission = Admissions::workflow_child(conn, parent, task, &executable, &refs)
        .await
        .unwrap();
    let prepared = Snapshots::prepare_initial_wait(
        conn,
        parent,
        workflow,
        &path,
        &admission,
        target,
        Some(&control),
        key,
    )
    .await
    .unwrap();
    let child=ExecutionRepository::create_pinned(&mut *conn,CreateExecutionInput {action:Some(executable.executable.action.id),
        action_ref:action.into(),parent:Some(parent),executor:root.executor,enforcement:root.enforcement,status:ExecutionStatus::Requested,config:None,permission_set_refs:refs,
        workflow_task:Some(serde_json::from_value(json!({"workflow_execution":workflow,"task_name":task,"retry_count":0,"max_retries":0,"timed_out":false})).unwrap()),
        ..Default::default()},&executable).await.unwrap();
    admission
        .attach(conn, RuntimeOwner::Execution(child.id))
        .await
        .unwrap();
    attune_common::repositories::rule_authority::RuleAuthorityRepository::copy_execution(
        conn, parent, child.id,
    )
    .await
    .unwrap();
    attune_common::repositories::recorded_input_authority::RecordedInputAuthorityRepository::copy(
        conn,
        "execution",
        parent,
        "execution",
        child.id,
    )
    .await
    .unwrap();
    let schema = Admissions::workflow_schema(conn, parent, &executable)
        .await
        .unwrap();
    Schemas::attach(conn, BindingOwner::Execution, child.id, &schema)
        .await
        .unwrap();
    let input = declarations
        .iter()
        .find(|binding| binding.source_path == format!("{path}/params/token"))
        .unwrap();
    let mut session = Inputs::begin_render(
        conn,
        parent,
        &input.declaration_revision,
        &input.source_path,
        &input.expression,
        InputSitePurpose::Value,
    )
    .await
    .unwrap();
    prepared.bind_input(&mut session).unwrap();
    let rendered = Inputs::evaluate_with_keys(conn, session, key)
        .await
        .unwrap();
    let destination = Inputs::prepare_destination(
        conn,
        &rendered,
        &InputSite {
            execution: child.id,
            kind: InputEntityKind::ExecutionConfig,
            purpose: InputSitePurpose::Value,
            path: "/token".into(),
        },
        key,
    )
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
        "a genuine prospective source cannot publish before its actual normalized wait release"
    );
    assert!(Waits::find_by_workflow_task(&mut *conn, workflow, task)
        .await
        .unwrap()
        .is_none());
}
