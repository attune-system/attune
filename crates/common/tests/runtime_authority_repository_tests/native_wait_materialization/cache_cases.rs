use super::*;
use attune_common::repositories::{
    cache::{
        CacheEntryInput, CacheGenerationRepository, CacheIngestRepository, CacheNamespacePolicy,
        CacheNamespaceRepository, CacheOwnerScope, CreateCacheGenerationInput,
        CreateCacheGenerationResult, CreateCacheNamespaceInput,
    },
    inherited_input::InputSitePurpose,
    runtime_authority::RuntimeAuthorityRepository as Authority,
    workflow_cache_iteration::WorkflowCacheIterationRepository as Iterations,
};

pub(crate) async fn seed(
    db: &attune_common::test_database::TestDatabase,
    pack: Id,
    pack_ref: &str,
    empty: bool,
) {
    let namespace = CacheNamespaceRepository::create_api_with_policy(
        db.pool(),
        CreateCacheNamespaceInput {
            mode: Default::default(),
            owner: CacheOwnerScope::pack(pack, Some(pack_ref.into())),
            namespace: "execution-original".into(),
            policy: CacheNamespacePolicy::default(),
        },
        &attune_common::config::CacheAdmissionConfig::default(),
    )
    .await
    .unwrap();
    let generation = CacheGenerationRepository::create_or_get(
        db.pool(),
        &CreateCacheGenerationInput {
            namespace: namespace.id,
            client_refresh_id: "wait-frame-owned".into(),
            expected_active_generation: None,
            expected_chunk_count: 1,
            expected_count: Some(if empty { 0 } else { 1 }),
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
        panic!("owned generation must be new")
    };
    let entries = if empty {
        vec![]
    } else {
        vec![CacheEntryInput {
            external_id: "first".into(),
            value: json!({"token":"{{ keystore['system.wait_frame_never_evaluate'] }}"}),
            source_updated_at: None,
            source_checksum: None,
        }]
    };
    CacheIngestRepository::insert_chunk(
        db.pool(),
        generation.id,
        0,
        "wait-frame-owned-chunk",
        &entries,
    )
    .await
    .unwrap();
    CacheGenerationRepository::seal(db.pool(), generation.id)
        .await
        .unwrap();
    CacheGenerationRepository::promote(
        db.pool(),
        namespace.id,
        generation.id,
        None,
        Utc::now() + Duration::hours(1),
    )
    .await
    .unwrap();
}

pub(crate) async fn check(
    db: &attune_common::test_database::TestDatabase,
    workflow: Id,
    parent: Id,
    task: &str,
    key: &str,
    empty: bool,
) {
    let mut tx = db.begin().await.unwrap();
    let wait = Waits::find_by_workflow_task(&mut *tx, workflow, task)
        .await
        .unwrap()
        .unwrap();
    status(&mut tx, wait.target().unwrap(), "failed").await;
    tx.commit().await.unwrap();
    let mut tx = db.begin().await.unwrap();
    protect(&mut tx).await;
    let mut plan = Continue::prepare(&mut tx, &wait, Some(key))
        .await
        .unwrap()
        .expect("real wait/cache caller");
    let before = Workflows::find_by_id(&mut *tx, workflow)
        .await
        .unwrap()
        .unwrap();
    assert!(before.variables.get("a").is_none() && before.variables.get("b").is_none());
    assert!(
        Iterations::find_by_workflow_task(&mut *tx, workflow, "native_recovery")
            .await
            .unwrap()
            .is_none()
    );
    let release: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM runtime_native_wait_release WHERE wait=$1)",
    )
    .bind(wait.id)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    assert!(!release);
    // A changed owning buffer must deny before release/publication/cache bind.
    sqlx::query("SAVEPOINT wait_cache_changed")
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("UPDATE workflow_execution SET variables=variables || '{\"b\":\"caller-replacement\"}'::jsonb WHERE id=$1")
        .bind(workflow).execute(&mut *tx).await.unwrap();
    assert!(plan.apply(&mut tx, key).await.is_err());
    sqlx::query("ROLLBACK TO SAVEPOINT wait_cache_changed")
        .execute(&mut *tx)
        .await
        .unwrap();
    sqlx::query("RELEASE SAVEPOINT wait_cache_changed")
        .execute(&mut *tx)
        .await
        .unwrap();
    let children = plan.apply(&mut tx, key).await.unwrap();
    assert_eq!(children.len(), usize::from(!empty));
    let iteration = Iterations::find_by_workflow_task(&mut *tx, workflow, "native_recovery")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(iteration.dispatched_count, i64::from(!empty));
    let state = Workflows::find_by_id(&mut *tx, workflow)
        .await
        .unwrap()
        .unwrap();
    assert!(state.completed_tasks.iter().any(|name| name == task));
    if empty {
        assert!(state
            .completed_tasks
            .iter()
            .any(|name| name == "native_recovery"));
    }
    for child in &children {
        original_child_acceptance(&mut tx, parent, child).await;
        assert!(child.id > 0);
        assert_eq!(
            child.workflow_task.as_ref().unwrap().task_name,
            "native_recovery"
        );
        assert!(child.workflow_task.as_ref().unwrap().task_index.is_some());
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
            "{{ keystore['system.wait_frame_never_evaluate'] }}"
        );
        let receipts=attune_common::repositories::declared_input_materialization::DeclaredInputMaterializationRepository::prepare_execution_input_receipts(&mut tx,child.id,key).await.unwrap();
        let conditions: std::collections::BTreeSet<_> = receipts
            .iter()
            .flat_map(|receipt| receipt.source_receipts())
            .filter(|source| {
                source.site().execution == parent
                    && source.site().purpose == InputSitePurpose::Condition
            })
            .map(|source| source.site().path.clone())
            .collect();
        assert_eq!(
            conditions.len(),
            5,
            "false decisions survive even on cache child inputs"
        );
    }
    let fields = Secrets::find_stored_by_entity(&mut *tx, "workflow_variables", parent)
        .await
        .unwrap();
    let values =
        attune_common::secret_values::restore_secret_values(state.variables, &fields, key).unwrap();
    assert_eq!(values["a"], "execution-original");
    assert_eq!(values["b"], "execution-original");
    plan.ensure_fresh_on(&mut tx).await.unwrap();
    for mutation in 0..3 {
        sqlx::query("SAVEPOINT wait_cache_publication_change")
            .execute(&mut *tx)
            .await
            .unwrap();
        match mutation {
            0 => {
                sqlx::query("UPDATE workflow_execution SET variables=variables || '{\"b\":\"changed-after-seal\"}'::jsonb WHERE id=$1")
                .bind(workflow).execute(&mut *tx).await.unwrap();
            }
            1 => {
                let deleted=sqlx::query("DELETE FROM execution_secret_value WHERE entity_type='workflow_variables' AND entity_id=$1 AND json_path='/b'")
                .bind(parent).execute(&mut *tx).await.unwrap();
                assert_eq!(deleted.rows_affected(), 1);
            }
            _ => {
                let error = sqlx::query("DELETE FROM runtime_native_wait_release WHERE wait=$1")
                    .bind(wait.id)
                    .execute(&mut *tx)
                    .await
                    .unwrap_err();
                let database = error.as_database_error().expect("immutable release guard");
                assert_eq!(database.code().as_deref(), Some("P0001"));
                assert_eq!(
                    database.message(),
                    "Native input snapshot evidence is immutable"
                );
            }
        }
        if mutation != 2 {
            assert!(
                plan.ensure_fresh_on(&mut tx).await.is_err(),
                "sealed frame denies changed variable/ciphertext, case {mutation}"
            );
        }
        sqlx::query("ROLLBACK TO SAVEPOINT wait_cache_publication_change")
            .execute(&mut *tx)
            .await
            .unwrap();
        sqlx::query("RELEASE SAVEPOINT wait_cache_publication_change")
            .execute(&mut *tx)
            .await
            .unwrap();
        plan.ensure_fresh_on(&mut tx).await.unwrap();
    }
    let source = wait.target().unwrap();
    let Target::Execution(source) = source else {
        panic!("owned execution wait")
    };
    let admission = Authority::execution_admission(&mut tx, source)
        .await
        .unwrap()
        .unwrap();
    plan.ensure_fresh_on(&mut tx).await.unwrap();
    tx.commit().await.unwrap();
    plan.ensure_fresh().unwrap();

    // Held source locks fence concurrent revocation within the preparation
    // transaction; ensure_fresh_on checks the carried absolute deadlines, not
    // newly loaded authority. A new preparation must reject revoked originals.
    let mut revoked_tx = db.begin().await.unwrap();
    protect(&mut revoked_tx).await;
    assert!(Authority::revoke_admission(&mut revoked_tx, admission.id)
        .await
        .unwrap());
    let released_wait = Waits::find_by_workflow_task(&mut *revoked_tx, workflow, task)
        .await
        .unwrap()
        .unwrap();
    assert!(Continue::prepare(&mut revoked_tx, &released_wait, Some(key)).await.is_err(),
        "fresh cache continuation preparation rejects revoked original Wait authority, including no-child activation");
    revoked_tx.rollback().await.unwrap();

    reclaim(db.pool(), wait.target().unwrap()).await;
    let mut replay_tx = db.begin().await.unwrap();
    protect(&mut replay_tx).await;
    let actual_wait = Waits::find_by_workflow_task(&mut *replay_tx, workflow, task)
        .await
        .unwrap()
        .unwrap();
    let snapshots: i64 = sqlx::query_scalar("SELECT count(*) FROM runtime_native_input_snapshot")
        .fetch_one(&mut *replay_tx)
        .await
        .unwrap();
    let receipts: i64 = sqlx::query_scalar("SELECT count(*) FROM runtime_input_receipt")
        .fetch_one(&mut *replay_tx)
        .await
        .unwrap();
    let mut replay = Continue::prepare(&mut replay_tx, &actual_wait, Some(key))
        .await
        .unwrap()
        .expect("real original cache activation replay after source GC");
    sqlx::query("SAVEPOINT wait_cache_replay_stream_change")
        .execute(&mut *replay_tx)
        .await
        .unwrap();
    sqlx::query(
        "UPDATE workflow_cache_iteration SET next_batch_index=next_batch_index+1, dispatched_count=dispatched_count+1, scanned_count=scanned_count+1 WHERE id=$1",
    )
    .bind(iteration.id)
    .execute(&mut *replay_tx)
    .await
    .unwrap();
    assert!(replay.apply(&mut replay_tx, key).await.is_err());
    sqlx::query("ROLLBACK TO SAVEPOINT wait_cache_replay_stream_change")
        .execute(&mut *replay_tx)
        .await
        .unwrap();
    sqlx::query("RELEASE SAVEPOINT wait_cache_replay_stream_change")
        .execute(&mut *replay_tx)
        .await
        .unwrap();
    // The held stream target check denies before consuming the wait observation.
    // No source preparation or graph traversal is repeated behind target locks.
    let pending = replay.apply(&mut replay_tx, key).await.unwrap();
    assert_eq!(
        pending.iter().map(|child| child.id).collect::<Vec<_>>(),
        children.iter().map(|child| child.id).collect::<Vec<_>>()
    );
    for (actual, original) in pending.iter().zip(&children) {
        assert_eq!(actual.config, original.config);
    }
    assert_eq!(
        serde_json::to_value(
            Iterations::find_by_workflow_task(&mut *replay_tx, workflow, "native_recovery")
                .await
                .unwrap()
                .unwrap()
        )
        .unwrap(),
        serde_json::to_value(iteration).unwrap()
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM runtime_native_input_snapshot")
            .fetch_one(&mut *replay_tx)
            .await
            .unwrap(),
        snapshots
    );
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM runtime_input_receipt")
            .fetch_one(&mut *replay_tx)
            .await
            .unwrap(),
        receipts,
        "replay does not rerender or issue replacement decisions/values"
    );
    replay.ensure_fresh_on(&mut replay_tx).await.unwrap();
    replay_tx.commit().await.unwrap();
    replay.ensure_fresh().unwrap();
}
