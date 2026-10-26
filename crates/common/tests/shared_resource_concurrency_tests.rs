//! Real PostgreSQL admissions, including independent pools and duplicate delivery.
mod helpers;

use attune_common::{
    config::Config,
    models::{Action, ExecutionStatus, PolicyMethod},
    repositories::{
        action::{CreatePolicyInput, PolicyRepository, UpdatePolicyInput},
        execution::{CreateExecutionInput, ExecutionRepository, UpdateExecutionInput},
        execution_admission::{
            AdmissionEnqueueOutcome as Outcome, ExecutionAdmissionRepository as Admission,
            StandardConcurrencyAdmission,
        },
        maintenance::MaintenanceRepository,
        Create, Update,
    },
    test_database::TestDatabase,
};
use helpers::{ActionFixture, PackFixture};
use serde_json::{json, Value};
use sqlx::PgPool;
use std::sync::Arc;
use tokio::sync::Barrier;

async fn database() -> TestDatabase {
    let config = Config::load_from_file(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../config.test.yaml"
    ))
    .unwrap();
    TestDatabase::create(&config.database).await.unwrap()
}

async fn actions(pool: &PgPool) -> [Action; 3] {
    let pack = PackFixture::new_unique("shared")
        .create(pool)
        .await
        .unwrap();
    let a = ActionFixture::new(pack.id, &pack.r#ref, "deploy")
        .create(pool)
        .await
        .unwrap();
    let other_pack = PackFixture::new_unique("shared_other")
        .create(pool)
        .await
        .unwrap();
    let b = ActionFixture::new(other_pack.id, &other_pack.r#ref, "configure")
        .create(pool)
        .await
        .unwrap();
    let c = ActionFixture::new(pack.id, &pack.r#ref, "unrelated")
        .create(pool)
        .await
        .unwrap();
    [a, b, c]
}

fn policy(actions: &[Action; 3], method: PolicyMethod) -> CreatePolicyInput {
    CreatePolicyInput {
        r#ref: "test.shared_org".into(),
        name: "Shared organization".into(),
        description: None,
        pack: None,
        pack_ref: None,
        action: None,
        action_ref: None,
        enabled: true,
        priority: 0,
        parameters: vec![],
        method: None,
        threshold: None,
        rate_limit_max_executions: None,
        rate_limit_window_seconds: None,
        quotas: json!([]),
        tags: vec![],
        resource_concurrency: Some(
            json!({"namespace":"salesforce_org", "limit":1, "method":method,
            "participants":[{"action":actions[0].r#ref,"resource_parameter":"org_id"}, {"action":actions[1].r#ref,"resource_parameter":"target.organization_id"}]}),
        ),
    }
}

async fn execution(pool: &PgPool, action: &Action, config: Value) -> i64 {
    execution_with_parent(pool, action, config, None).await
}

async fn execution_with_parent(
    pool: &PgPool,
    action: &Action,
    config: Value,
    parent: Option<i64>,
) -> i64 {
    ExecutionRepository::create(
        pool,
        CreateExecutionInput {
            action: Some(action.id),
            action_ref: action.r#ref.clone(),
            config: Some(config),
            env_vars: None,
            parent,
            enforcement: None,
            executor: None,
            permission_set_refs: vec![],
            artifact_retention_policy: None,
            artifact_retention_limit: None,
            worker_selector: None,
            worker_tolerations: None,
            worker_affinity: None,
            worker: None,
            status: ExecutionStatus::Requested,
            trace_tag: None,
            result: None,
            workflow_task: None,
            timeout_seconds: None,
        },
    )
    .await
    .unwrap()
    .id
}

async fn admit(pool: &PgPool, action: &Action, id: i64, config: &Value) -> Option<Outcome> {
    Admission::admit_shared_resource(pool, 100, action.id, id, Some(config), None)
        .await
        .unwrap()
}

async fn finish(pool: &PgPool, id: i64, status: ExecutionStatus) {
    ExecutionRepository::update(
        pool,
        id,
        UpdateExecutionInput {
            status: Some(status),
            ..Default::default()
        },
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn workflow_lineage_does_not_partition_resource_groups() {
    let db = database().await;
    let pool = db.pool();
    let actions = actions(pool).await;
    PolicyRepository::create(pool, policy(&actions, PolicyMethod::Enqueue))
        .await
        .unwrap();
    let parent_a = execution(pool, &actions[2], json!({})).await;
    let parent_b = execution(pool, &actions[2], json!({})).await;
    let config_a = json!({"org_id":"same-org"});
    let config_b = json!({"target":{"organization_id":"same-org"}});
    let child_a = execution_with_parent(pool, &actions[0], config_a.clone(), Some(parent_a)).await;
    let child_b = execution_with_parent(pool, &actions[1], config_b.clone(), Some(parent_b)).await;
    assert_eq!(
        admit(pool, &actions[0], child_a, &config_a).await,
        Some(Outcome::Acquired)
    );
    assert_eq!(
        admit(pool, &actions[1], child_b, &config_b).await,
        Some(Outcome::Enqueued)
    );
    assert_eq!(admit(pool, &actions[2], parent_a, &json!({})).await, None);
    finish(pool, child_a, ExecutionStatus::Failed).await;
    assert_eq!(
        admit(pool, &actions[1], child_b, &config_b).await,
        Some(Outcome::Acquired)
    );
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn shared_identity_fifo_release_and_disabled_behavior() {
    let db = database().await;
    let pool = db.pool();
    let actions = actions(pool).await;
    let p = PolicyRepository::create(pool, policy(&actions, PolicyMethod::Enqueue))
        .await
        .unwrap();
    // A shared policy never becomes the effective global standard policy.
    assert!(PolicyRepository::find_latest_global(pool)
        .await
        .unwrap()
        .is_none());
    let a = json!({"org_id":"org-a"});
    let b = json!({"target":{"organization_id":"org-a"}});
    let other = json!({"org_id":"org-b"});
    let first = execution(pool, &actions[0], a.clone()).await;
    let second = execution(pool, &actions[1], b.clone()).await;
    let third = execution(pool, &actions[0], a.clone()).await;
    let different = execution(pool, &actions[0], other.clone()).await;
    let unrelated = execution(pool, &actions[2], json!({})).await;
    assert_eq!(
        admit(pool, &actions[0], first, &a).await,
        Some(Outcome::Acquired)
    );
    assert_eq!(
        admit(pool, &actions[1], second, &b).await,
        Some(Outcome::Enqueued)
    );
    assert_eq!(
        admit(pool, &actions[0], third, &a).await,
        Some(Outcome::Enqueued)
    );
    assert_eq!(
        admit(pool, &actions[0], different, &other).await,
        Some(Outcome::Acquired)
    );
    assert_eq!(admit(pool, &actions[2], unrelated, &json!({})).await, None);
    let mut changed_resource = policy(&actions, PolicyMethod::Enqueue)
        .resource_concurrency
        .unwrap();
    changed_resource["limit"] = json!(2);
    assert!(PolicyRepository::update(
        pool,
        p.id,
        UpdatePolicyInput {
            resource_concurrency: Some(Some(changed_resource)),
            ..Default::default()
        }
    )
    .await
    .unwrap_err()
    .to_string()
    .contains("active or waiting"));
    // Terminal release is idempotent and independent of executor memory/messages.
    finish(pool, first, ExecutionStatus::Completed).await;
    finish(pool, first, ExecutionStatus::Completed).await;
    assert_eq!(
        admit(pool, &actions[0], third, &a).await,
        Some(Outcome::Enqueued)
    );
    assert_eq!(
        admit(pool, &actions[1], second, &b).await,
        Some(Outcome::Acquired)
    );
    finish(pool, second, ExecutionStatus::Cancelled).await;
    assert_eq!(
        admit(pool, &actions[0], third, &a).await,
        Some(Outcome::Acquired)
    );
    PolicyRepository::update(
        pool,
        p.id,
        UpdatePolicyInput {
            enabled: Some(false),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let disabled = execution(pool, &actions[0], a.clone()).await;
    assert_eq!(admit(pool, &actions[0], disabled, &a).await, None);
    // Existing admitted reservations remain associated with their old policy.
    assert_eq!(
        admit(pool, &actions[0], third, &a).await,
        Some(Outcome::Acquired)
    );
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn independent_executor_pools_and_duplicate_messages_do_not_overadmit() {
    let db = database().await;
    let pool = db.pool();
    let actions = actions(pool).await;
    PolicyRepository::create(pool, policy(&actions, PolicyMethod::Enqueue))
        .await
        .unwrap();
    let replica = PgPool::connect_with(
        pool.connect_options()
            .as_ref()
            .clone()
            .options([("search_path", db.schema())]),
    )
    .await
    .unwrap();
    let barrier = Arc::new(Barrier::new(17));
    let mut tasks = Vec::new();
    for index in 0..16 {
        let action = actions[index % 2].clone();
        let config = if index % 2 == 0 {
            json!({"org_id":"same"})
        } else {
            json!({"target":{"organization_id":"same"}})
        };
        let id = execution(pool, &action, config.clone()).await;
        let pool = if index % 2 == 0 {
            pool.clone()
        } else {
            replica.clone()
        };
        let barrier = barrier.clone();
        tasks.push(tokio::spawn(async move {
            barrier.wait().await;
            let result = admit(&pool, &action, id, &config).await;
            let duplicate = admit(&pool, &action, id, &config).await;
            assert_eq!(result, duplicate);
            result
        }));
    }
    barrier.wait().await;
    let mut acquired = 0;
    for task in tasks {
        acquired += usize::from(task.await.unwrap() == Some(Outcome::Acquired));
    }
    assert_eq!(acquired, 1);
    let reservations: i64 = sqlx::query_scalar("SELECT count(*) FROM execution_resource_admission")
        .fetch_one(pool)
        .await
        .unwrap();
    assert_eq!(reservations, 16);
    replica.close().await;
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn cancel_invalid_membership_and_atomic_standard_constraint() {
    let db = database().await;
    let pool = db.pool();
    let actions = actions(pool).await;
    let p = PolicyRepository::create(pool, policy(&actions, PolicyMethod::Cancel))
        .await
        .unwrap();
    let mut conflict = policy(&actions, PolicyMethod::Enqueue);
    conflict.r#ref = "test.conflicting".into();
    assert!(PolicyRepository::create(pool, conflict.clone())
        .await
        .unwrap_err()
        .to_string()
        .contains("another enabled shared resource policy"));
    conflict.enabled = false;
    let disabled_conflict = PolicyRepository::create(pool, conflict).await.unwrap();
    assert!(PolicyRepository::update(
        pool,
        disabled_conflict.id,
        UpdatePolicyInput {
            enabled: Some(true),
            ..Default::default()
        }
    )
    .await
    .is_err());
    for value in [json!({}), json!({"org_id":null}), json!({"org_id":[]})] {
        let id = execution(pool, &actions[0], value.clone()).await;
        assert!(
            Admission::admit_shared_resource(pool, 100, actions[0].id, id, Some(&value), None)
                .await
                .unwrap_err()
                .to_string()
                .contains("Policy violation:")
        );
    }
    let config = json!({"org_id":"org"});
    let first = execution(pool, &actions[0], config.clone()).await;
    assert_eq!(
        admit(pool, &actions[0], first, &config).await,
        Some(Outcome::Acquired)
    );
    let second = execution(pool, &actions[0], config.clone()).await;
    let standard = || {
        Some(StandardConcurrencyAdmission {
            limit: 1,
            method: PolicyMethod::Enqueue,
            group_key: None,
        })
    };
    assert!(Admission::admit_shared_resource(
        pool,
        100,
        actions[0].id,
        second,
        Some(&config),
        standard()
    )
    .await
    .is_err());
    assert_eq!(Admission::wait_status(pool, second).await.unwrap(), None);
    finish(pool, first, ExecutionStatus::Timeout).await;
    assert_eq!(
        Admission::admit_shared_resource(
            pool,
            100,
            actions[0].id,
            second,
            Some(&config),
            standard()
        )
        .await
        .unwrap(),
        Some(Outcome::Acquired)
    );
    let third = execution(pool, &actions[0], json!({"org_id":"different"})).await;
    // An unsuccessful next-message publication must not restore only half of
    // a shared admission. A fresh scheduling attempt reacquires both slots.
    let release = Admission::release_active_slot(pool, second)
        .await
        .unwrap()
        .unwrap();
    Admission::restore_active_slot(pool, second, &release)
        .await
        .unwrap();
    assert_eq!(Admission::wait_status(pool, second).await.unwrap(), None);
    assert_eq!(
        Admission::admit_shared_resource(
            pool,
            100,
            actions[0].id,
            second,
            Some(&config),
            standard()
        )
        .await
        .unwrap(),
        Some(Outcome::Acquired)
    );
    assert_eq!(
        Admission::admit_shared_resource(
            pool,
            100,
            actions[0].id,
            third,
            Some(&json!({"org_id":"different"})),
            standard()
        )
        .await
        .unwrap(),
        Some(Outcome::Enqueued)
    );
    let active: bool = sqlx::query_scalar(
        "SELECT active FROM execution_resource_admission WHERE execution_id = $1",
    )
    .bind(third)
    .fetch_one(pool)
    .await
    .unwrap();
    assert!(
        !active,
        "failed standard admission must not occupy a shared slot"
    );
    finish(pool, second, ExecutionStatus::Abandoned).await;
    Admission::release_active_slot(pool, second).await.unwrap();
    // Existing supervisor recovery recognizes a shared waiter even if its
    // standard reservation remains queued and its retry budget is exhausted.
    sqlx::query("UPDATE execution SET updated = NOW() - interval '1 hour' WHERE id = $1")
        .bind(third)
        .execute(pool)
        .await
        .unwrap();
    let candidates =
        MaintenanceRepository::find_requested_executions_for_reschedule(pool, 0, 0, 100)
            .await
            .unwrap();
    assert!(candidates
        .iter()
        .any(|candidate| candidate.execution_id == third));
    assert_eq!(
        Admission::admit_shared_resource(
            pool,
            100,
            actions[0].id,
            third,
            Some(&json!({"org_id":"different"})),
            standard()
        )
        .await
        .unwrap(),
        Some(Outcome::Acquired)
    );
    assert!(PolicyRepository::update(
        pool,
        p.id,
        UpdatePolicyInput {
            enabled: Some(false),
            ..Default::default()
        }
    )
    .await
    .is_ok());
    db.cleanup().await.unwrap();
}
