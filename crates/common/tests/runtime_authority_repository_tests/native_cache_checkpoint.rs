use super::*;
#[path = "native_cache_checkpoint/h_projected_completion.rs"]
mod h_projected_completion;
use attune_common::{
    models::{Execution, WorkflowExecution},
    repositories::{
        cache::{CacheEntryRepository, CacheTransactionMode},
        declared_input::DeclaredInputRepository,
        inherited_input::InheritedInputRepository as Inputs,
        native_cache_materialization::NativeCacheMaterializationRepository as Cache,
        native_input_origin::NativeInputOriginRepository as Native,
        workflow::WorkflowExecutionRepository as Workflows,
        workflow_cache_iteration::{
            UpdateWorkflowCacheIterationProgressInput,
            WorkflowCacheIterationRepository as Iterations,
        },
        FindById, Update,
    },
    secret_values::ENTITY_EXECUTION_CONFIG,
    test_database::TestDatabase,
};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Case {
    Static,
    Dynamic,
    Empty,
    Stale,
    Expired,
    Activate,
    ActivateEmpty,
    ActivateFalse,
    ActivateStale,
    ActivateExpired,
    ActivateOrdinary,
    CompletionSource,
    CompletionFailed,
    CompletionChanged,
    CompletionPersistedEmpty,
    GroupNext,
    GroupFailedNext,
    GroupChanged,
}

impl Case {
    pub(super) fn activation(self) -> bool {
        matches!(
            self,
            Self::Activate
                | Self::ActivateEmpty
                | Self::ActivateFalse
                | Self::ActivateStale
                | Self::ActivateExpired
                | Self::ActivateOrdinary
        )
    }
    pub(super) fn empty(self) -> bool {
        matches!(
            self,
            Self::Empty | Self::ActivateEmpty | Self::CompletionPersistedEmpty
        )
    }
    pub(super) fn group(self) -> bool {
        matches!(
            self,
            Self::GroupNext | Self::GroupFailedNext | Self::GroupChanged
        )
    }
}

#[tokio::test]
async fn native_cache_checkpoint_static_entry_two_successors_eof_and_first_child_seals() {
    super::run_native_cache_fixture(Some(Case::Static)).await;
}
#[tokio::test]
async fn native_cache_checkpoint_committed_dynamic_selector_refills_and_completes() {
    super::run_native_cache_fixture(Some(Case::Dynamic)).await;
}
#[tokio::test]
async fn native_cache_checkpoint_empty_eof_creates_no_synthetic_child() {
    super::run_native_cache_fixture(Some(Case::Empty)).await;
}
#[tokio::test]
async fn native_cache_checkpoint_stale_actual_cursor_cannot_publish_reserved_child() {
    super::run_native_cache_fixture(Some(Case::Stale)).await;
}
#[tokio::test]
async fn native_cache_checkpoint_selector_approval_expires_during_actual_target_wait() {
    super::run_native_cache_fixture(Some(Case::Expired)).await;
}

#[tokio::test]
async fn native_cache_completion_source_covers_real_latest_attempts_and_checkpoint_eof() {
    super::run_native_cache_fixture(Some(Case::CompletionSource)).await;
}
#[tokio::test]
async fn native_cache_completion_source_authenticates_failed_batch_without_fabricating_aggregate_value(
) {
    super::run_native_cache_fixture(Some(Case::CompletionFailed)).await;
}
#[tokio::test]
async fn native_cache_completion_changed_attempt_denies_progress_even_when_summary_counts_match() {
    super::run_native_cache_fixture(Some(Case::CompletionChanged)).await;
}
#[tokio::test]
async fn native_cache_completion_persisted_empty_eof_has_real_frontier_and_zero_children() {
    super::run_native_cache_fixture(Some(Case::CompletionPersistedEmpty)).await;
}

pub(super) fn configure(case: Case, definition: &mut serde_json::Value) {
    if case.group() {
        let mut task = definition["tasks"][0].clone();
        task.as_object_mut().unwrap().remove("iterate_cache");
        task.as_object_mut().unwrap().remove("batch_size");
        task["with_items"] = json!("{{ [parameters.cache_name, parameters.cache_name] }}");
        task["concurrency"] = json!(2);
        task["params"] = json!({"token":{"label":"{{ item }}"},"ordinal":"{{ index }}"});
        let mut after = task.clone();
        after["name"] = json!("after");
        after["with_items"] = json!("{{ workflow.carried }}");
        after["params"]["token"] = json!({"token":"child-literal"});
        let mut failed = after.clone();
        failed["name"] = json!("after_failed");
        let program = json!([{"chosen":[{"token":"original"}]},{"carried":"{{ chosen }}"},{"chosen":[{"token":"overwrite"}]}]);
        task["next"] = json!([
            {"when":"{{ succeeded() }}","publish":program,"do":["after"]},
            {"when":"{{ failed() }}","publish":[{"chosen":[{"token":"failed"}]},{"carried":"{{ chosen }}"}],"do":["after_failed"]}
        ]);
        let unselected = if case == Case::GroupFailedNext { 0 } else { 1 };
        task["next"][unselected]["publish"]
            .as_array_mut()
            .unwrap()
            .push(json!({"never":"{{ keystore['system.unselected_group_next'] }}"}));
        definition["tasks"] = json!([task, after, failed]);
        return;
    }
    let task = &mut definition["tasks"][0];
    task["concurrency"] = json!(if case == Case::Dynamic || case == Case::Stale {
        1
    } else {
        4
    });
    if matches!(
        case,
        Case::Static
            | Case::Empty
            | Case::CompletionSource
            | Case::CompletionFailed
            | Case::CompletionChanged
            | Case::CompletionPersistedEmpty
    ) {
        task["iterate_cache"]["namespace"] = json!("native-input");
    }
    if case == Case::Expired {
        task["iterate_cache"]["namespace"] =
            json!("{{ keystore['system.cache_checkpoint_namespace'] }}");
    }
}

pub(super) async fn check(
    case: Case,
    db: &TestDatabase,
    root: &Execution,
    workflow: &WorkflowExecution,
    human: Id,
) {
    let key = NATIVE_CACHE_TEST_KEY;
    if case.group() {
        super::native_group_transitions::check(case, db, root, workflow, key).await;
        return;
    }
    assert!(Cache::supports(root, "cached"));
    let mut successor = root.clone();
    successor
        .executable_snapshot
        .as_mut()
        .unwrap()
        .executable
        .workflow_definition
        .as_mut()
        .unwrap()
        .definition["tasks"][0]["next"] = json!([{"do":["other"]}]);
    assert!(
        !Cache::supports(&successor, "cached"),
        "later successor adapter keeps ownership of other graph shapes"
    );
    let approval = if case == Case::Expired {
        helpers::KeyFixture::new_system("cache_checkpoint_namespace", "native-input")
            .create(db)
            .await
            .unwrap();
        let set = PermissionSetRepository::create(db.pool(), CreatePermissionSetInput {
            r#ref: "fixture.cache_checkpoint_selector_issuer".into(), pack: None, pack_ref: None, label: None, description: None,
            grants: json!([{"resource":"keys","actions":["read"],"constraints":{"refs":["system.cache_checkpoint_namespace"]}}]),
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
        let declaration = DeclaredInputRepository::bindings(&mut tx, root.id)
            .await
            .unwrap()
            .into_iter()
            .find(|declaration| {
                declaration.source_path == "/workflow/tasks/0/iterate_cache/namespace"
            })
            .unwrap();
        let approval = Inputs::approve_external_input(&mut tx, root.id, &declaration.declaration_revision,
            &declaration.source_path, &declaration.expression, human,
            serde_json::from_value(json!([{"resource":"keys","actions":["read"],"constraints":{"refs":["system.cache_checkpoint_namespace"]}}])).unwrap(),
            Some(Utc::now() + Duration::seconds(10))).await.unwrap();
        tx.commit().await.unwrap();
        Some(approval)
    } else {
        None
    };
    let mut blocker = if approval.is_some() {
        let mut tx = db.pool().begin().await.unwrap();
        Workflows::find_by_id_for_update(&mut *tx, workflow.id)
            .await
            .unwrap()
            .unwrap();
        Some(tx)
    } else {
        None
    };
    let mut saw_children = Vec::new();
    let mut original_cursor = None;
    let mut reserved_empty = false;
    loop {
        let mut tx = db.begin().await.unwrap();
        CacheEntryRepository::protect_transaction(&mut tx, CacheTransactionMode::PinMutation)
            .await
            .unwrap();
        let actual = ExecutionRepository::find_by_id(&mut *tx, root.id)
            .await
            .unwrap()
            .unwrap();
        if case == Case::CompletionFailed && !saw_children.is_empty() {
            let iteration = Iterations::find_by_workflow_task(&mut *tx, workflow.id, "cached")
                .await
                .unwrap()
                .unwrap();
            let source =
                Cache::prepare_completion_source(&mut tx, root.id, workflow.id, iteration.id, key)
                    .await
                    .unwrap()
                    .unwrap();
            assert!(source.has_failed_latest_attempt());
            assert_eq!(
                source
                    .attempts()
                    .iter()
                    .filter(|attempt| attempt.is_latest())
                    .count(),
                3
            );
            assert!(source.attempts().iter().any(|attempt| attempt.is_latest()
                && attempt.execution().status == ExecutionStatus::Failed));
            source.require_current(&mut tx).await.unwrap();
            h_projected_completion::check_source(&mut tx, &source).await;
            assert!(Cache::prepare_completion_source(
                &mut tx,
                saw_children[0],
                workflow.id,
                iteration.id,
                key
            )
            .await
            .is_err());
            assert!(Iterations::find_by_id(&mut *tx, iteration.id)
                .await
                .unwrap()
                .unwrap()
                .completed_at
                .is_none());
            assert!(!Workflows::find_by_id(&mut *tx, workflow.id)
                .await
                .unwrap()
                .unwrap()
                .completed_tasks
                .iter()
                .any(|name| name == "cached"));
            source.ensure_fresh_on(&mut tx).await.unwrap();
            tx.rollback().await.unwrap();
            source.ensure_fresh().unwrap();
            return;
        }
        let profile = attune_common::repositories::native_cache_materialization::NativeCachePreparationProfile::default();
        let backend: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *tx)
            .await
            .unwrap();
        let prepared = {
            let preparation = profile.record(Cache::prepare(&mut tx, &actual, "cached", key));
            tokio::pin!(preparation);
            tokio::select! {
                result = &mut preparation => result,
                _ = tokio::time::sleep(std::time::Duration::from_secs(5)) => {
                    // Observe while the timed-out query is still alive. No SQL,
                    // source values, credentials or payloads enter diagnostics.
                    let state = tokio::time::timeout(std::time::Duration::from_secs(1),
                        sqlx::query_as::<_, (String, Option<String>, Option<String>, Vec<i32>)>(
                            "SELECT state, wait_event_type, wait_event, pg_blocking_pids(pid) FROM pg_stat_activity WHERE pid=$1")
                            .bind(backend).fetch_optional(db.pool())).await;
                    panic!("source preparation exceeded unchanged five-second bound; session: {state:?}; preparation counters: {:?}", profile.snapshot());
                }
            }
        };
        let mut plan = prepared.unwrap();
        // Production prepares the same plan before its outer advisory lock.
        plan.ensure_fresh_on(&mut tx).await.unwrap();
        if case == Case::CompletionPersistedEmpty && !reserved_empty {
            let source = plan
                .completion_source()
                .expect("initial empty checkpoint owns its genuine zero-child source");
            assert!(source.original_iteration().is_none());
            assert!(source.attempts().is_empty());
            h_projected_completion::check_source(&mut tx, source).await;
            let row = Iterations::create_or_find_for_update(
                &mut tx,
                source.eof().initial_create_input().unwrap(),
            )
            .await
            .unwrap();
            attune_common::repositories::native_input_snapshot::NativeInputSnapshotRepository::bind_cache_frontier(&mut tx, source.eof(), row.id).await.unwrap();
            source.ensure_fresh_on(&mut tx).await.unwrap();
            tx.commit().await.unwrap();
            source.ensure_fresh().unwrap();
            reserved_empty = true;
            continue;
        }
        let projected = if let Some(source) = plan.completion_source() {
            Some(h_projected_completion::check_source(&mut tx, source).await)
        } else {
            None
        };
        if let Some(source) = plan.completion_source() {
            assert!(source.eof().exhausted());
            assert_eq!(source.eof().count(), 0);
            assert!(source.bound_iteration().is_none());
            source.require_current(&mut tx).await.unwrap();
            if matches!(
                case,
                Case::CompletionSource | Case::CompletionChanged | Case::CompletionPersistedEmpty
            ) {
                let iteration = source
                    .original_iteration()
                    .expect("persisted completion has actual iteration identity")
                    .clone();
                let standalone = Cache::prepare_completion_source(
                    &mut tx,
                    root.id,
                    workflow.id,
                    iteration.id,
                    key,
                )
                .await
                .unwrap()
                .unwrap();
                assert_eq!(standalone.attempts().len(), saw_children.len());
                assert!(!standalone.has_failed_latest_attempt());
                if case == Case::CompletionSource {
                    assert!(attune_common::repositories::declared_input_materialization::DeclaredInputMaterializationRepository::prepare_execution_input_receipts(
                        &mut tx, saw_children[0], key,
                    ).await.is_err(), "observing a legitimate terminal cache source must not reopen its process delivery gate");
                    for attempt in standalone.attempts() {
                        assert_eq!(
                            attempt.execution().result.as_ref().unwrap()["data"]["literal"],
                            json!("{{ keystore['system.never_resolve_completion'] }}")
                        );
                    }
                }
                standalone.require_current(&mut tx).await.unwrap();
                if case == Case::CompletionChanged {
                    let h_plan = attune_common::repositories::native_failure_materialization::NativeFailureMaterializationRepository::prepare_projected_cache_completion(
                        &mut tx, root.id, workflow.id, &standalone,
                    ).await.unwrap().unwrap();
                    ExecutionRepository::update(&mut *tx, saw_children[0], UpdateExecutionInput {
                        result: Some(json!({"succeeded":true,"literal":"{{ keystore['system.do_not_resolve'] }}"})),
                        ..Default::default()
                    }).await.unwrap();
                    assert!(standalone.require_current(&mut tx).await.is_err());
                    assert!(h_plan.require_current(&mut tx).await.is_err());
                    assert!(plan.apply(&mut tx, key).await.is_err());
                    assert!(Iterations::find_by_id(&mut *tx, iteration.id)
                        .await
                        .unwrap()
                        .unwrap()
                        .completed_at
                        .is_none());
                    tx.rollback().await.unwrap();
                    return;
                }
            }
        }
        if let (Some(approval), Some(mut blocker)) = (approval, blocker.take()) {
            let waiter: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
                .fetch_one(&mut *tx)
                .await
                .unwrap();
            let release = tokio::spawn(async move {
                let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(20);
                let mut observed = false;
                loop {
                    observed |=
                        sqlx::query_scalar::<_, bool>("SELECT cardinality(pg_blocking_pids($1))>0")
                            .bind(waiter)
                            .fetch_one(&mut *blocker)
                            .await
                            .unwrap();
                    let expired: bool = sqlx::query_scalar("SELECT expires_at <= clock_timestamp() FROM runtime_external_approval WHERE id=$1")
                        .bind(approval).fetch_one(&mut *blocker).await.unwrap();
                    if observed && expired {
                        blocker.commit().await.unwrap();
                        return;
                    }
                    assert!(
                        tokio::time::Instant::now() < deadline,
                        "must observe actual workflow target wait before source approval expires"
                    );
                    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                }
            });
            let result =
                tokio::time::timeout(std::time::Duration::from_secs(25), plan.apply(&mut tx, key))
                    .await;
            release.await.unwrap();
            assert!(result.unwrap().is_err());
            assert!(plan.ensure_fresh().is_err());
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
            tx.rollback().await.unwrap();
            assert!(ExecutionRepository::find_by_parent(db.pool(), root.id)
                .await
                .unwrap()
                .is_empty());
            return;
        }
        if case == Case::Stale && original_cursor.is_some() {
            let iteration = Iterations::find_by_workflow_task(&mut *tx, workflow.id, "cached")
                .await
                .unwrap()
                .unwrap();
            Iterations::update_scan_progress(
                &mut *tx,
                iteration.id,
                UpdateWorkflowCacheIterationProgressInput {
                    last_external_id: "c".into(),
                    next_batch_index: iteration.next_batch_index + 1,
                    scanned_count: iteration.scanned_count + 1,
                    dispatched_count: iteration.dispatched_count + 1,
                },
            )
            .await
            .unwrap()
            .unwrap();
            assert!(plan.apply(&mut tx, key).await.is_err());
            tx.rollback().await.unwrap();
            assert_eq!(
                ExecutionRepository::find_by_parent(db.pool(), root.id)
                    .await
                    .unwrap()
                    .len(),
                saw_children.len()
            );
            assert_eq!(
                Iterations::find_by_workflow_task(db.pool(), workflow.id, "cached")
                    .await
                    .unwrap()
                    .unwrap()
                    .last_external_id,
                original_cursor
            );
            return;
        }
        let outcome = plan.apply(&mut tx, key).await.unwrap();
        if let Some(projected) = projected {
            let actual = Workflows::find_by_id(&mut *tx, workflow.id)
                .await
                .unwrap()
                .unwrap();
            projected.require_projected_task_state(&actual).unwrap();
        }
        if let Some(source) = plan.completion_source() {
            assert!(source.bound_iteration().is_some());
            assert_eq!(
                source.bound_frontier().unwrap().state,
                attune_common::models::WorkflowCacheIterationState::Scanning
            );
            assert_eq!(
                source.terminal_frontier().unwrap().state,
                attune_common::models::WorkflowCacheIterationState::Completed
            );
            source.require_terminal_frontier(&mut tx).await.unwrap();
        }
        if saw_children.is_empty() && case == Case::Static {
            assert_eq!(
                outcome.children.len(),
                3,
                "initial plus two linked pages bind in the same activation"
            );
        }
        if case == Case::Dynamic {
            assert!(
                outcome.children.len() <= 1,
                "refill respects the single persisted slot"
            );
        }
        for child in &outcome.children {
            assert_eq!(child.parent, Some(root.id));
            let metadata = child.workflow_task.as_ref().unwrap();
            assert_eq!(metadata.task_index, Some(saw_children.len() as i32));
            assert_eq!(
                metadata.task_batch,
                Some(if saw_children.len() == 2 { 1 } else { 2 })
            );
            let restored = Native::capture_with_key(&mut tx, child.id, &[], Some(key))
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
            assert_eq!(restored["ordinal"], json!(saw_children.len()));
            let schema =
                attune_common::repositories::schema_contract::SchemaContractRepository::find(
                    &mut tx,
                    attune_common::repositories::schema_contract::BindingOwner::Execution,
                    child.id,
                )
                .await
                .unwrap()
                .unwrap();
            attune_common::value_schema::CompiledContract::new(&schema.input)
                .unwrap()
                .validate(&restored)
                .unwrap();
            assert_eq!(
                restored["token"]["token"],
                json!(match saw_children.len() {
                    0 => "{{ keystore['system.never_resolve'] }}",
                    1 => "c",
                    _ => "e",
                })
            );
            let seal: bool = sqlx::query_scalar(
                "SELECT EXISTS(SELECT 1 FROM runtime_accepted_input a JOIN runtime_accepted_input_epoch e ON e.admission=a.admission WHERE a.execution=$1 AND e.transaction_id=pg_current_xact_id()::TEXT)",
            )
            .bind(child.id)
            .fetch_one(&mut *tx)
            .await
            .unwrap();
            assert!(
                seal,
                "real child has its first accepted-input seal in creation transaction"
            );
            saw_children.push(child.id);
        }
        let completed = outcome.completed.is_some();
        if completed {
            assert_eq!(
                outcome.completed.unwrap().status,
                ExecutionStatus::Completed
            );
        }
        plan.ensure_fresh_on(&mut tx).await.unwrap();
        tx.commit().await.unwrap();
        plan.ensure_fresh().unwrap();
        let iteration = Iterations::find_by_workflow_task(db.pool(), workflow.id, "cached")
            .await
            .unwrap()
            .unwrap();
        original_cursor = iteration.last_external_id.clone();
        if completed {
            assert_eq!(saw_children.len(), if case.empty() { 0 } else { 3 });
            assert_eq!(iteration.scanned_count, if case.empty() { 0 } else { 5 });
            assert_eq!(iteration.next_batch_index, if case.empty() { 0 } else { 3 });
            return;
        }
        if case == Case::Static && saw_children.len() == 3 {
            let mut recovery = db.begin().await.unwrap();
            CacheEntryRepository::protect_transaction(
                &mut recovery,
                CacheTransactionMode::PinMutation,
            )
            .await
            .unwrap();
            let actual = ExecutionRepository::find_by_id(&mut *recovery, root.id)
                .await
                .unwrap()
                .unwrap();
            let mut replay = Cache::prepare(&mut recovery, &actual, "cached", key)
                .await
                .unwrap();
            let recovered = replay.apply(&mut recovery, key).await.unwrap();
            assert_eq!(recovered.children.iter().map(|child| child.id).collect::<Vec<_>>(), saw_children,
                "broker retry returns the same source-revalidated requests, not additional children");
            assert!(recovered.completed.is_none());
            replay.ensure_fresh_on(&mut recovery).await.unwrap();
            recovery.commit().await.unwrap();
            replay.ensure_fresh().unwrap();
        }
        // Independent worker completions free the real persisted slots. No
        // synthetic execution/result is used to prove an empty stream outcome.
        let mut tx = db.begin().await.unwrap();
        for id in outcome.children.into_iter().map(|child| child.id) {
            ExecutionRepository::update(
                &mut *tx,
                id,
                UpdateExecutionInput {
                    status: Some(if case == Case::CompletionFailed && id == saw_children[0] {
                        ExecutionStatus::Failed
                    } else {
                        ExecutionStatus::Completed
                    }),
                    result: Some(if case == Case::CompletionFailed && id == saw_children[0] {
                        json!({"succeeded":false,"error":"actual batch failed"})
                    } else if case == Case::CompletionSource {
                        json!({"succeeded":true,"data":{"literal":"{{ keystore['system.never_resolve_completion'] }}"}})
                    } else {
                        json!({"succeeded":true})
                    }),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        }
        tx.commit().await.unwrap();
        assert!(
            saw_children.len() <= 3,
            "checkpoint must stop at pinned EOF"
        );
    }
}
