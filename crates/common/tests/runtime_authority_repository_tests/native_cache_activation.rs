use super::native_cache_checkpoint::Case;
use super::*;
use attune_common::{
    models::{Execution, ExecutionExecutableSnapshot, WorkflowExecution, WorkflowTaskMetadata},
    repositories::{
        cache::{CacheEntryRepository, CacheTransactionMode},
        declared_input::DeclaredInputRepository,
        declared_input_materialization::DeclaredInputMaterializationRepository as Materialize,
        execution_runtime_admission::ExecutionAdmissionRepository as Admissions,
        inherited_input::InheritedInputRepository as Inputs,
        native_cache_materialization::{
            NativeCacheMaterializationRepository as Cache, NativeCachePreparationProfile,
        },
        native_input_origin::NativeInputOriginRepository as Native,
        schema_contract::{BindingOwner, SchemaContractRepository as Schemas},
        workflow::{UpdateWorkflowExecutionInput, WorkflowExecutionRepository as Workflows},
        workflow_cache_iteration::WorkflowCacheIterationRepository as Iterations,
        FindById, Update,
    },
    secret_values::ENTITY_EXECUTION_CONFIG,
    test_database::TestDatabase,
};

// These are correctness/lock-order tests, not wall-clock performance gates.
// Allow concurrent fixture work to finish before the original finite consent
// expires; expiry itself is still observed at the actual workflow target wait.
const PREPARATION_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);
const CONSENT_LIFETIME_SECONDS: i64 = 60;

#[tokio::test]
async fn same_advance_pending_namespace_activates_two_real_cache_successors_and_refills() {
    super::run_native_cache_fixture(Some(Case::Activate)).await;
}
#[tokio::test]
async fn same_advance_empty_cache_successors_finish_without_synthetic_children() {
    super::run_native_cache_fixture(Some(Case::ActivateEmpty)).await;
}

/// Measure the actual production factory type without invoking the function
/// item, constructing its arguments, issuing proofs, or polling a future.
#[test]
fn cache_prepare_work_public_future_has_bounded_inline_storage() {
    fn future_bytes<'a, F: std::future::Future + Send>(
        _: impl FnOnce(
            &'a mut sqlx::PgConnection,
            &'a Execution,
            &'a str,
            Option<&'a Execution>,
            &'a str,
        ) -> F,
    ) -> usize {
        std::mem::size_of::<F>()
    }
    let bytes = future_bytes(Cache::prepare_work);
    assert!(
        bytes <= 2 * std::mem::size_of::<usize>(),
        "actual Cache::prepare_work future must fit its boxed owning boundary; bytes={bytes}"
    );
}
#[tokio::test]
async fn same_advance_false_decision_never_resolves_unselected_namespace_keys() {
    super::run_native_cache_fixture(Some(Case::ActivateFalse)).await;
}
#[tokio::test]
async fn same_advance_selector_rejects_changed_actual_workflow_variables_before_publication() {
    super::run_native_cache_fixture(Some(Case::ActivateStale)).await;
}
#[tokio::test]
async fn same_advance_value_consent_expires_at_real_workflow_target_wait() {
    super::run_native_cache_fixture(Some(Case::ActivateExpired)).await;
}

#[tokio::test]
async fn ordinary_successor_frame_uses_real_predecessor_selected_sites_and_sealed_publications() {
    super::run_native_cache_fixture(Some(Case::ActivateOrdinary)).await;
}

pub(super) fn configure(case: Case, graph: &mut serde_json::Value, producer: &str) {
    let mut cached = graph["tasks"][0].clone();
    cached["concurrency"] = json!(4);
    cached["iterate_cache"]["namespace"] = json!(if case == Case::ActivateFalse {
        "{{ keystore['system.unselected_cache_activation_namespace'] }}"
    } else {
        "{{ workflow.first_namespace }}"
    });
    let mut cached2 = cached.clone();
    cached2["name"] = json!("cached2");
    let namespace = if case == Case::ActivateExpired {
        "{{ keystore['system.cache_activation_namespace'] }}"
    } else {
        "{{ parameters.cache_name }}"
    };
    let mut transition = json!({"publish":[{"selected_namespace":namespace},{"first_namespace":"{{ selected_namespace }}"},{"selected_namespace":"{{ first_namespace }}"}],"do":["cached","cached2"]});
    if case == Case::ActivateFalse {
        transition["when"] = json!("{{ false }}");
    }
    graph["tasks"] =
        json!([{"name":"choose","action":producer,"next":[transition]}, cached, cached2]);
    if case == Case::ActivateOrdinary {
        graph["tasks"][0]["next"][0]["when"] = json!("{{ succeeded() }}");
        graph["tasks"][0]["next"][0]["publish"][0]["selected_namespace"] =
            json!([{"token":"one"},{"token":"two"}]);
        for task in graph["tasks"].as_array_mut().unwrap().iter_mut().skip(1) {
            task.as_object_mut().unwrap().remove("iterate_cache");
            task["with_items"] = json!("{{ workflow.first_namespace }}");
            task["params"]["token"] = json!({"token":"child-literal"});
        }
    }
}

pub(super) async fn create_producer(
    conn: &mut sqlx::PgConnection,
    root: &Execution,
    workflow: &WorkflowExecution,
    snapshot: &ExecutionExecutableSnapshot,
    key: &str,
) -> Execution {
    let schema = Admissions::workflow_schema(conn, root.id, snapshot)
        .await
        .unwrap();
    let child = Admissions::create_workflow_task_if_absent_pinned_with_conn(
        conn,
        CreateExecutionInput {
            action: Some(snapshot.executable.action.id),
            action_ref: snapshot.executable.action.r#ref.clone(),
            parent: Some(root.id),
            executor: root.executor,
            config: None,
            status: ExecutionStatus::Requested,
            workflow_task: Some(WorkflowTaskMetadata {
                workflow_execution: workflow.id,
                task_name: "choose".into(),
                task_index: None,
                task_batch: None,
                triggered_by: None,
                retry_count: 0,
                max_retries: 0,
                timeout_seconds: None,
                next_retry_at: None,
                timed_out: false,
                duration_ms: None,
                started_at: None,
                completed_at: None,
            }),
            ..Default::default()
        },
        snapshot,
        workflow.id,
        "choose",
        None,
    )
    .await
    .unwrap();
    assert!(child.created);
    Schemas::attach(conn, BindingOwner::Execution, child.execution.id, &schema)
        .await
        .unwrap();
    let admission = Admissions::require(conn, RuntimeOwner::Execution(child.execution.id))
        .await
        .unwrap();
    ExecutionRepository::initialize_config_with_conn(
        conn,
        child.execution.id,
        Some(admission.id),
        snapshot,
        Some(json!({})),
        &[],
    )
    .await
    .unwrap();
    Materialize::seal_execution_inputs(conn, child.execution.id, key)
        .await
        .unwrap();
    ExecutionRepository::update(
        &mut *conn,
        child.execution.id,
        UpdateExecutionInput {
            status: Some(ExecutionStatus::Completed),
            result: Some(json!({"succeeded":true})),
            ..Default::default()
        },
    )
    .await
    .unwrap()
}

pub(super) async fn check(
    case: Case,
    db: &TestDatabase,
    root: &Execution,
    workflow: &WorkflowExecution,
    producer: &Execution,
) {
    let key = NATIVE_CACHE_TEST_KEY;
    if case == Case::ActivateOrdinary {
        check_ordinary_frame(db, root, workflow, producer, key).await;
        return;
    }
    assert!(Cache::supports_work(root, "choose", Some(producer)));
    // Reserve the diagnostic connection before issuing expiring consent.
    let mut observer = tokio::time::timeout(std::time::Duration::from_secs(2), db.pool().acquire())
        .await
        .expect("cache preparation observer must acquire its own bounded connection")
        .unwrap();
    let approval = if case == Case::ActivateExpired {
        helpers::KeyFixture::new_system("cache_activation_namespace", "native-input")
            .create(db)
            .await
            .unwrap();
        let human = producer.executor.unwrap();
        let set = PermissionSetRepository::create(db.pool(), CreatePermissionSetInput {
            r#ref: "fixture.cache_activation_issuer".into(), pack: None, pack_ref: None, label: None, description: None,
            grants: json!([{"resource":"keys","actions":["read"],"constraints":{"refs":["system.cache_activation_namespace"]}}]),
        }).await.unwrap();
        PermissionAssignmentRepository::create(
            db.pool(),
            CreatePermissionAssignmentInput {
                identity: human,
                permset: set.id,
            },
        )
        .await
        .unwrap();
        let mut tx = db.begin().await.unwrap();
        let binding = DeclaredInputRepository::bindings(&mut tx, root.id)
            .await
            .unwrap()
            .into_iter()
            .find(|binding| {
                binding.source_path == "/workflow/tasks/0/next/0/publish/0/selected_namespace"
            })
            .unwrap();
        let approval = Inputs::approve_external_input(&mut tx, root.id, &binding.declaration_revision,
            &binding.source_path, &binding.expression, human,
            serde_json::from_value(json!([{"resource":"keys","actions":["read"],"constraints":{"refs":["system.cache_activation_namespace"]}}])).unwrap(),
            Some(Utc::now()+Duration::seconds(CONSENT_LIFETIME_SECONDS))).await.unwrap();
        tx.commit().await.unwrap();
        Some(approval)
    } else {
        None
    };
    let blocker = if approval.is_some() {
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
    CacheEntryRepository::protect_transaction(&mut tx, CacheTransactionMode::PinMutation)
        .await
        .unwrap();
    let source_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *tx)
        .await
        .unwrap();
    let (finished, stop) = tokio::sync::oneshot::channel();
    let profile = NativeCachePreparationProfile::default();
    let preparation = async {
        let result = tokio::time::timeout(
            PREPARATION_TIMEOUT,
            profile.record(Cache::prepare_work(
                &mut tx,
                root,
                "choose",
                Some(producer),
                key,
            )),
        )
        .await;
        let _ = finished.send(());
        result
    };
    let (prepared, observations) = tokio::join!(
        preparation,
        observe_source_preparation(&mut observer, source_pid, stop)
    );
    drop(observer);
    let mut plan = prepared.unwrap_or_else(|_| panic!(
        "all activation sources and destinations must prepare without workflow target locks; source_pid={source_pid}; metadata_only={observations:?}; source_profile={:?}", profile.snapshot()
    )).unwrap_or_else(|error| panic!("activation source preparation failed: {error}; source_profile={:?}", profile.snapshot()));
    let metrics = profile.snapshot();
    eprintln!("bounded cache preparation profile: {metrics:?}");
    assert_eq!(
        metrics
            .iter()
            .find(|metric| metric.operation == "sources.prepare")
            .unwrap()
            .finished,
        1,
        "the actual predecessor prepares its original opaque source set once"
    );
    if case != Case::ActivateFalse {
        assert_eq!(metrics.iter().find(|metric| metric.operation == "sources.reuse_check").unwrap().finished, 2,
            "both real cache successors reuse that exact scope without preparing replacement source graphs");
        assert_eq!(metrics.iter().find(|metric| metric.operation == "control.destination").map_or(0, |metric| metric.calls), 0,
            "issued selectors prepare their actual destination once inside A's constructor, never an unused duplicate first");
        assert_eq!(
            metrics
                .iter()
                .find(|metric| metric.operation == "selector.proof")
                .unwrap()
                .finished,
            2
        );
    }
    plan.ensure_fresh_on(&mut tx).await.unwrap();
    assert!(
        Iterations::find_by_workflow_task(&mut *tx, workflow.id, "cached")
            .await
            .unwrap()
            .is_none()
    );
    assert_eq!(
        Workflows::find_by_id(&mut *tx, workflow.id)
            .await
            .unwrap()
            .unwrap()
            .variables,
        workflow.variables
    );
    if case == Case::ActivateStale {
        Workflows::update(
            &mut *tx,
            workflow.id,
            UpdateWorkflowExecutionInput {
                variables: Some(json!({"tampered":true})),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert!(plan.apply(&mut tx, key).await.is_err());
        let published: i64 = sqlx::query_scalar("SELECT count(*) FROM runtime_input_receipt_staged WHERE publication_transaction IS NOT NULL")
            .fetch_one(&mut *tx).await.unwrap();
        assert_eq!(
            published, 0,
            "stale target variables cannot finalize any staged VALUE or selector Control"
        );
        tx.rollback().await.unwrap();
        assert_eq!(
            ExecutionRepository::find_by_parent(db.pool(), root.id)
                .await
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            Workflows::find_by_id(db.pool(), workflow.id)
                .await
                .unwrap()
                .unwrap()
                .variables,
            workflow.variables
        );
        assert!(
            Iterations::find_by_workflow_task(db.pool(), workflow.id, "cached")
                .await
                .unwrap()
                .is_none()
        );
        return;
    }
    if let (Some(approval), Some(mut blocker)) = (approval, blocker) {
        let blocker_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *blocker)
            .await
            .unwrap();
        let waiter: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *tx)
            .await
            .unwrap();
        let release = tokio::spawn(async move {
            let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(70);
            let mut observed = false;
            loop {
                let blocked =
                    sqlx::query_scalar::<_, bool>("SELECT $2 = ANY(pg_blocking_pids($1))")
                        .bind(waiter)
                        .bind(blocker_pid)
                        .fetch_one(&mut *blocker)
                        .await
                        .unwrap();
                let expired: bool = sqlx::query_scalar("SELECT expires_at <= clock_timestamp() FROM runtime_external_approval WHERE id=$1").bind(approval).fetch_one(&mut *blocker).await.unwrap();
                if blocked && !observed {
                    assert!(
                        !expired,
                        "original VALUE consent must be valid when the target wait starts"
                    );
                    observed = true;
                }
                if observed && blocked && expired {
                    blocker.commit().await.unwrap();
                    return;
                }
                assert!(
                    tokio::time::Instant::now() < deadline,
                    "must observe actual target wait and original VALUE expiry"
                );
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
            }
        });
        let result =
            tokio::time::timeout(std::time::Duration::from_secs(80), plan.apply(&mut tx, key))
                .await;
        release.await.unwrap();
        assert!(result.unwrap().is_err());
        assert!(plan.ensure_fresh().is_err());
        let published: i64 = sqlx::query_scalar("SELECT count(*) FROM runtime_input_receipt_staged WHERE publication_transaction IS NOT NULL")
            .fetch_one(&mut *tx).await.unwrap();
        assert_eq!(
            published, 0,
            "expired original consent fails before any stage publication"
        );
        assert_eq!(
            Workflows::find_by_id(&mut *tx, workflow.id)
                .await
                .unwrap()
                .unwrap()
                .variables,
            workflow.variables
        );
        assert!(
            Iterations::find_by_workflow_task(&mut *tx, workflow.id, "cached")
                .await
                .unwrap()
                .is_none()
        );
        tx.rollback().await.unwrap();
        assert_eq!(
            ExecutionRepository::find_by_parent(db.pool(), root.id)
                .await
                .unwrap()
                .len(),
            1
        );
        return;
    }
    let result = plan.apply(&mut tx, key).await.unwrap();
    let state = Workflows::find_by_id(&mut *tx, workflow.id)
        .await
        .unwrap()
        .unwrap();
    assert!(state.completed_tasks.contains(&"choose".into()));
    if case == Case::ActivateFalse {
        assert!(result.children.is_empty());
        assert!(result.completed.is_some());
        assert_eq!(state.skipped_tasks.len(), 2);
        assert!(
            Iterations::find_by_workflow_task(&mut *tx, workflow.id, "cached")
                .await
                .unwrap()
                .is_none()
        );
    } else {
        if result.completed.is_none() {
            let view = Native::capture_with_key(&mut tx, root.id, &[], Some(key))
                .await
                .unwrap()
                .restore_prepared(
                    &mut tx,
                    attune_common::secret_values::ENTITY_WORKFLOW_VARIABLES,
                    root.id,
                    state.variables.clone(),
                    Some(key),
                )
                .await
                .unwrap();
            assert_eq!(
                view,
                json!({"selected_namespace":"native-input","first_namespace":"native-input"})
            );
        }
        assert_eq!(
            result.children.len(),
            if case == Case::ActivateEmpty { 0 } else { 6 }
        );
        assert_eq!(
            state.current_tasks.len(),
            if case == Case::ActivateEmpty { 0 } else { 2 }
        );
        for child in &result.children {
            let metadata = child.workflow_task.as_ref().unwrap();
            assert!(matches!(metadata.task_name.as_str(), "cached" | "cached2"));
            assert!(metadata
                .task_index
                .is_some_and(|index| (0..3).contains(&index)));
            let actual = Native::capture_with_key(&mut tx, child.id, &[], Some(key))
                .await
                .unwrap()
                .restore_prepared(
                    &mut tx,
                    ENTITY_EXECUTION_CONFIG,
                    child.id,
                    child.config.clone().unwrap(),
                    Some(key),
                )
                .await
                .unwrap();
            assert_eq!(actual["ordinal"], json!(metadata.task_index.unwrap()));
            let sealed: bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM runtime_accepted_input a JOIN runtime_accepted_input_epoch e ON e.admission=a.admission WHERE a.execution=$1 AND e.transaction_id=pg_current_xact_id()::TEXT)")
                .bind(child.id).fetch_one(&mut *tx).await.unwrap();
            assert!(sealed);
            let selector_path = format!(
                "/workflow/tasks/{}/iterate_cache/namespace",
                if metadata.task_name == "cached" { 1 } else { 2 }
            );
            let retained: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM runtime_input_receipt_snapshot d JOIN runtime_input_receipt input ON input.id=d.receipt JOIN runtime_native_input_snapshot_parent p ON p.snapshot=d.snapshot JOIN runtime_input_receipt control ON control.id=p.receipt WHERE input.proof#>>'{site,execution}'=$1 AND control.proof#>>'{site,path}'=$2 AND control.proof#>>'{site,purpose}'='control')"
            ).bind(child.id.to_string()).bind(selector_path).fetch_one(&mut *tx).await.unwrap();
            assert!(
                retained,
                "the real child input retains its native page and exact selector Control parent"
            );
        }
        let controls: i64=sqlx::query_scalar("SELECT count(*) FROM runtime_input_receipt_staged s JOIN runtime_input_receipt r ON r.id=s.receipt WHERE r.proof#>>'{site,purpose}'='control' AND s.publication_transaction=pg_current_xact_id()::TEXT")
            .fetch_one(&mut *tx).await.unwrap();
        assert_eq!(
            controls, 2,
            "both real non-Boolean selector Controls finalize at their actual cache frontier"
        );
    }
    plan.ensure_fresh_on(&mut tx).await.unwrap();
    tx.commit().await.unwrap();
    plan.ensure_fresh().unwrap();
    if case == Case::Activate {
        let mut replay_tx = db.begin().await.unwrap();
        CacheEntryRepository::protect_transaction(
            &mut replay_tx,
            CacheTransactionMode::PinMutation,
        )
        .await
        .unwrap();
        let parent = ExecutionRepository::find_by_id(&mut *replay_tx, root.id)
            .await
            .unwrap()
            .unwrap();
        let mut replay =
            Cache::prepare_work(&mut replay_tx, &parent, "choose", Some(producer), key)
                .await
                .unwrap();
        let recovered = replay.apply(&mut replay_tx, key).await.unwrap();
        assert_eq!(recovered.children.iter().map(|child| child.id).collect::<Vec<_>>(),
            result.children.iter().map(|child| child.id).collect::<Vec<_>>(),
            "duplicate predecessor delivery recovers original requests without repeating publications");
        assert!(recovered.completed.is_none());
        replay.ensure_fresh_on(&mut replay_tx).await.unwrap();
        replay_tx.commit().await.unwrap();
        replay.ensure_fresh().unwrap();
        let mut tx = db.begin().await.unwrap();
        for child in &result.children {
            ExecutionRepository::update(
                &mut *tx,
                child.id,
                UpdateExecutionInput {
                    status: Some(ExecutionStatus::Completed),
                    result: Some(json!({"succeeded":true})),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        }
        tx.commit().await.unwrap();
        for task in ["cached", "cached2"] {
            let mut tx = db.begin().await.unwrap();
            CacheEntryRepository::protect_transaction(&mut tx, CacheTransactionMode::PinMutation)
                .await
                .unwrap();
            let parent = ExecutionRepository::find_by_id(&mut *tx, root.id)
                .await
                .unwrap()
                .unwrap();
            let mut refill = Cache::prepare_work(&mut tx, &parent, task, None, key)
                .await
                .unwrap();
            let out = refill.apply(&mut tx, key).await.unwrap();
            assert!(out.children.is_empty());
            assert_eq!(out.completed.is_some(), task == "cached2");
            refill.ensure_fresh_on(&mut tx).await.unwrap();
            tx.commit().await.unwrap();
            refill.ensure_fresh().unwrap();
        }
        assert_eq!(
            Workflows::find_by_id(db.pool(), workflow.id)
                .await
                .unwrap()
                .unwrap()
                .completed_tasks
                .len(),
            3
        );
    }
}

/// Runs concurrently with the actual production prepare future. No source rows,
/// query text, parameters, ciphertext or values are collected. Keep only recent
/// changed metadata, and join the observer before using or rolling back tx.
async fn observe_source_preparation(
    conn: &mut sqlx::PgConnection,
    source_pid: i32,
    mut stop: tokio::sync::oneshot::Receiver<()>,
) -> Vec<serde_json::Value> {
    let mut samples = std::collections::VecDeque::new();
    let mut tick = tokio::time::interval(std::time::Duration::from_millis(200));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    for _ in 0..64 {
        tokio::select! {
            biased;
            _ = &mut stop => break,
            _ = tick.tick() => {}
        }
        let snapshot = tokio::time::timeout(std::time::Duration::from_millis(200), sqlx::query_scalar::<_, serde_json::Value>(
            "SELECT jsonb_build_object('state',a.state,'wait_type',a.wait_event_type,'wait',a.wait_event,'query_hash',md5(a.query),'query_kind',CASE WHEN a.query ILIKE '%runtime_native%' THEN 'native_snapshot' WHEN a.query ILIKE '%runtime_input_receipt%' THEN 'input_receipt' WHEN a.query ILIKE '%runtime_schema%' THEN 'schema_binding' WHEN a.query ILIKE '%runtime_authority%' THEN 'runtime_authority' WHEN a.query ILIKE '%workflow_execution%' THEN 'workflow' WHEN a.query ILIKE '%execution%' THEN 'execution' ELSE 'other' END,'blockers',ARRAY(SELECT unnest(pg_blocking_pids(a.pid)) LIMIT 8),'locks',COALESCE((SELECT jsonb_agg(l) FROM (SELECT locktype,mode,granted,relation::regclass::TEXT AS relation FROM pg_locks WHERE pid=a.pid ORDER BY granted,locktype,mode,relation LIMIT 32) l),'[]'::JSONB)) FROM pg_stat_activity a WHERE a.pid=$1"
        ).bind(source_pid).fetch_one(&mut *conn)).await;
        let snapshot = match snapshot {
            Ok(Ok(value)) => value,
            Ok(Err(_)) => json!({"observer":"read_failed"}),
            Err(_) => json!({"observer":"read_deadline"}),
        };
        if samples.back() != Some(&snapshot) {
            if samples.len() == 8 {
                samples.pop_front();
            }
            samples.push_back(snapshot);
        }
    }
    samples.into_iter().collect()
}

async fn check_ordinary_frame(
    db: &TestDatabase,
    root: &Execution,
    workflow: &WorkflowExecution,
    producer: &Execution,
    key: &str,
) {
    use attune_common::repositories::{
        inherited_input::InputSitePurpose,
        native_transition_materialization::{
            NativeTransitionMaterializationRepository as Transitions, NativeTransitionRenderFrame,
        },
    };
    let mut tx = db.begin().await.unwrap();
    CacheEntryRepository::protect_transaction(&mut tx, CacheTransactionMode::PinMutation)
        .await
        .unwrap();
    assert!(
        !Cache::supports_work(root, "choose", Some(producer)),
        "ordinary eligibility is independent of cache guards"
    );
    let frame = Transitions::prepare_ordinary_successors(&mut tx, producer.id, key)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(frame.execution(), root.id);
    assert_eq!(
        frame.selected_targets(),
        &["cached".to_owned(), "cached2".to_owned()]
    );
    let binding = DeclaredInputRepository::bindings(&mut tx, root.id)
        .await
        .unwrap()
        .into_iter()
        .find(|binding| {
            binding.source_path == format!("{}/with_items", frame.target_path("cached").unwrap())
        })
        .unwrap();
    let mut wrong = binding.clone();
    wrong.expression = "{{ [] }}".into();
    let mut wrong_session = Inputs::begin_render(
        &mut tx,
        root.id,
        &binding.declaration_revision,
        &binding.source_path,
        &binding.expression,
        InputSitePurpose::Control,
    )
    .await
    .unwrap();
    assert!(frame
        .bind(&mut tx, &mut wrong_session, &wrong, key)
        .await
        .is_err());
    let mut session = Inputs::begin_render(
        &mut tx,
        root.id,
        &binding.declaration_revision,
        &binding.source_path,
        &binding.expression,
        InputSitePurpose::Control,
    )
    .await
    .unwrap();
    frame
        .bind(&mut tx, &mut session, &binding, key)
        .await
        .unwrap();
    let rendered = Inputs::evaluate_with_keys(&mut tx, session, key)
        .await
        .unwrap();
    assert_eq!(rendered.value(), &json!([{"token":"one"},{"token":"two"}]));
    let mut literal = Inputs::literal(
        &mut tx,
        root.id,
        &binding.declaration_revision,
        &format!(
            "{}/params/token/token",
            frame.target_path("cached").unwrap()
        ),
    )
    .await
    .unwrap();
    frame.attach_literal(&mut literal).unwrap();
    assert!(
        !literal.selections().is_empty(),
        "literal destinations retain the actual selected decision Control"
    );
    // This is B's genuine source/render bridge, not a D list Control or child
    // caller acceptance claim. A/D own the remaining list/item construction.
    assert!(frame.require_published(&mut tx).await.is_err());
    let actual = Workflows::find_by_id_for_update(&mut *tx, workflow.id)
        .await
        .unwrap()
        .unwrap();
    let recorded = frame.record(&mut tx, &actual, key).await.unwrap();
    attune_common::repositories::execution_secret_value::ExecutionSecretValueRepository::replace_entity_with_conn(
        &mut tx, attune_common::secret_values::ENTITY_WORKFLOW_VARIABLES, root.id, &recorded.secrets).await.unwrap();
    Workflows::update(
        &mut *tx,
        workflow.id,
        UpdateWorkflowExecutionInput {
            variables: Some(recorded.variables),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    frame.require_published(&mut tx).await.unwrap();
    assert!(frame.record(&mut tx, &actual, key).await.is_err());
    Workflows::update(
        &mut *tx,
        workflow.id,
        UpdateWorkflowExecutionInput {
            variables: Some(json!({"changed":true})),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert!(frame.require_published(&mut tx).await.is_err());
    frame.ensure_fresh_on(&mut tx).await.unwrap();
    tx.rollback().await.unwrap();
    frame.ensure_fresh().unwrap();
}
