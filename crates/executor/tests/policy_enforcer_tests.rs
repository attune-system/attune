//! Integration tests for PolicyEnforcer
//!
//! These tests verify policy enforcement logic including:
//! - Rate limiting
//! - Concurrency control
//! - Quota management
//! - Policy scope handling

use attune_common::{
    config::Config,
    models::enums::{ExecutionStatus, PolicyMethod},
    repositories::{
        action::{ActionRepository, CreateActionInput},
        execution::{CreateExecutionInput, ExecutionRepository},
        pack::{CreatePackInput, PackRepository},
        runtime::{CreateRuntimeInput, RuntimeRepository},
        Create,
    },
    test_database::TestDatabase,
};
use attune_executor::policy_enforcer::{ExecutionPolicy, PolicyEnforcer, RateLimit};
use chrono::Utc;
use sqlx::PgPool;

/// Test helper to set up database connection
async fn setup_db() -> TestDatabase {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_else(|_| ".".to_string());
    let config_path = format!("{}/../../config.test.yaml", manifest_dir);
    let config = Config::load_from_file(&config_path).expect("Failed to load test config");
    TestDatabase::create(&config.database)
        .await
        .expect("Failed to create isolated test database")
        .with_cleanup_on_drop()
}

/// Test helper to create a test pack
async fn create_test_pack(pool: &PgPool, suffix: &str) -> i64 {
    use serde_json::json;

    let pack_input = CreatePackInput {
        r#ref: format!("test_pack_{}", suffix),
        label: format!("Test Pack {}", suffix),
        description: Some(format!("Test pack for policy tests {}", suffix)),
        version: "1.0.0".to_string(),
        conf_schema: json!({}),
        config: json!({}),
        meta: json!({}),
        tags: vec![],
        runtime_deps: vec![],
        dependencies: vec![],
        is_standard: false,
        installers: json!({}),
    };

    let pack = PackRepository::create(pool, pack_input)
        .await
        .expect("Failed to create test pack");
    pack.id
}

/// Test helper to create a test runtime
#[allow(dead_code)]
async fn create_test_runtime(pool: &PgPool, suffix: &str) -> i64 {
    use serde_json::json;

    let runtime_input = CreateRuntimeInput {
        r#ref: format!("test_runtime_{}", suffix),
        pack: None,
        pack_ref: None,
        description: Some(format!("Test runtime {}", suffix)),
        name: format!("Python {}", suffix),
        aliases: vec![],
        distributions: json!({"ubuntu": "python3"}),
        installation: Some(json!({"method": "apt"})),
        execution_config: json!({
            "interpreter": {
                "binary": "python3",
                "args": ["-u"],
                "file_extension": ".py"
            }
        }),
        auto_detected: false,
        detection_config: json!({}),
    };

    let runtime = RuntimeRepository::create(pool, runtime_input)
        .await
        .expect("Failed to create test runtime");
    runtime.id
}

/// Test helper to create a test action
async fn create_test_action(pool: &PgPool, pack_id: i64, suffix: &str) -> i64 {
    let action_input = CreateActionInput {
        r#ref: format!("test_pack_{}.action", suffix),
        pack: pack_id,
        pack_ref: format!("test_pack_{}", suffix),
        label: format!("Test Action {}", suffix),
        description: Some(format!("Test action {}", suffix)),
        entrypoint: "echo test".to_string(),
        runtime: None,
        enabled: true,
        runtime_version_constraint: None,
        required_worker_runtimes: serde_json::json!({}),
        worker_selector: serde_json::json!({}),
        worker_tolerations: serde_json::json!([]),
        worker_affinity: serde_json::json!({}),
        param_schema: None,
        out_schema: None,
        is_adhoc: false,
        accesses_mcp: false,
        default_execution_permission_set_refs: Vec::new(),
        reference_visibility: Default::default(),
        reference_allowed_pack_refs: Vec::new(),
        artifact_retention_policy: None,
        artifact_retention_limit: None,
        log_retention_policy: None,
        log_retention_limit: None,
        timeout_seconds: None,
    };

    let action = ActionRepository::create(pool, action_input)
        .await
        .expect("Failed to create test action");
    action.id
}

/// Test helper to create a test execution
async fn create_test_execution(
    pool: &PgPool,
    action_id: i64,
    action_ref: &str,
    status: ExecutionStatus,
) -> i64 {
    let execution_input = CreateExecutionInput {
        action: Some(action_id),
        action_ref: action_ref.to_string(),
        config: None,
        env_vars: None,
        parent: None,
        enforcement: None,
        executor: None,
        permission_set_refs: Vec::new(),
        artifact_retention_policy: None,
        artifact_retention_limit: None,
        worker_selector: None,
        worker_tolerations: None,
        worker_affinity: None,
        worker: None,
        status,
        trace_tag: None,
        result: None,
        workflow_task: None,
        timeout_seconds: None,
    };

    let execution = ExecutionRepository::create(pool, execution_input)
        .await
        .expect("Failed to create test execution");
    execution.id
}

/// Test helper to cleanup test data
async fn cleanup_test_data(pool: &PgPool, pack_id: i64) {
    // Delete executions first (they reference actions)
    sqlx::query("DELETE FROM execution WHERE action IN (SELECT id FROM action WHERE pack = $1)")
        .bind(pack_id)
        .execute(pool)
        .await
        .expect("Failed to cleanup executions");

    // Delete actions
    sqlx::query("DELETE FROM action WHERE pack = $1")
        .bind(pack_id)
        .execute(pool)
        .await
        .expect("Failed to cleanup actions");

    // Delete pack
    sqlx::query("DELETE FROM pack WHERE id = $1")
        .bind(pack_id)
        .execute(pool)
        .await
        .expect("Failed to cleanup pack");
}

#[tokio::test]
#[ignore] // Requires database
async fn test_policy_enforcer_creation() {
    let pool = setup_db().await;
    let enforcer = PolicyEnforcer::new(pool.pool().clone());

    // Should be created with default policy (no limits)
    assert!(enforcer
        .check_policies(1, None)
        .await
        .expect("Policy check failed")
        .is_none());
}

#[tokio::test]
#[ignore] // Requires database
async fn test_global_rate_limit() {
    let pool = setup_db().await;
    let timestamp = Utc::now().timestamp();
    let pack_id = create_test_pack(&pool, &format!("rate_limit_{}", timestamp)).await;
    let action_id = create_test_action(&pool, pack_id, &format!("rate_limit_{}", timestamp)).await;
    let action_ref = format!("test_pack_rate_limit_{}.action", timestamp);

    // Create a policy with a very low rate limit
    let policy = ExecutionPolicy {
        rate_limit: Some(RateLimit {
            max_executions: 2,
            window_seconds: 60,
        }),
        concurrency_limit: None,
        concurrency_method: PolicyMethod::Enqueue,
        concurrency_parameters: Vec::new(),
        quotas: None,
    };

    let enforcer = PolicyEnforcer::with_global_policy(pool.clone(), policy);

    // First execution should be allowed
    let violation = enforcer
        .check_policies(action_id, Some(pack_id))
        .await
        .expect("Policy check failed");
    assert!(violation.is_none(), "First execution should be allowed");

    // Create an execution to increase count
    create_test_execution(&pool, action_id, &action_ref, ExecutionStatus::Requested).await;

    // Second execution should be allowed
    let violation = enforcer
        .check_policies(action_id, Some(pack_id))
        .await
        .expect("Policy check failed");
    assert!(violation.is_none(), "Second execution should be allowed");

    // Create another execution
    create_test_execution(&pool, action_id, &action_ref, ExecutionStatus::Requested).await;

    // Third execution should be blocked by rate limit
    let violation = enforcer
        .check_policies(action_id, Some(pack_id))
        .await
        .expect("Policy check failed");
    assert!(
        violation.is_some(),
        "Third execution should be blocked by rate limit"
    );

    // Cleanup
    cleanup_test_data(&pool, pack_id).await;
}

#[tokio::test]
#[ignore] // Requires database
async fn test_concurrency_limit() {
    let pool = setup_db().await;
    let timestamp = Utc::now().timestamp();
    let pack_id = create_test_pack(&pool, &format!("concurrency_{}", timestamp)).await;
    let action_id = create_test_action(&pool, pack_id, &format!("concurrency_{}", timestamp)).await;
    let action_ref = format!("test_pack_concurrency_{}.action", timestamp);

    // Create a policy with a concurrency limit
    let policy = ExecutionPolicy {
        rate_limit: None,
        concurrency_limit: Some(2),
        concurrency_method: PolicyMethod::Enqueue,
        concurrency_parameters: Vec::new(),
        quotas: None,
    };

    let enforcer = PolicyEnforcer::with_global_policy(pool.clone(), policy);

    // First running execution should be allowed
    let violation = enforcer
        .check_policies(action_id, Some(pack_id))
        .await
        .expect("Policy check failed");
    assert!(violation.is_none(), "First execution should be allowed");

    // Create a running execution
    create_test_execution(&pool, action_id, &action_ref, ExecutionStatus::Running).await;

    // Second running execution should be allowed
    let violation = enforcer
        .check_policies(action_id, Some(pack_id))
        .await
        .expect("Policy check failed");
    assert!(violation.is_none(), "Second execution should be allowed");

    // Create another running execution
    create_test_execution(&pool, action_id, &action_ref, ExecutionStatus::Running).await;

    // Third execution should be blocked by concurrency limit
    let violation = enforcer
        .check_policies(action_id, Some(pack_id))
        .await
        .expect("Policy check failed");
    assert!(
        violation.is_some(),
        "Third execution should be blocked by concurrency limit"
    );

    // Cleanup
    cleanup_test_data(&pool, pack_id).await;
}

#[tokio::test]
#[ignore] // Requires database
async fn test_action_specific_policy() {
    let pool = setup_db().await;
    let timestamp = Utc::now().timestamp();
    let pack_id = create_test_pack(&pool, &format!("action_policy_{}", timestamp)).await;
    let action_id =
        create_test_action(&pool, pack_id, &format!("action_policy_{}", timestamp)).await;

    // Create enforcer with no global policy
    let mut enforcer = PolicyEnforcer::new(pool.clone());

    // Set action-specific policy with strict limit
    let action_policy = ExecutionPolicy {
        rate_limit: Some(RateLimit {
            max_executions: 1,
            window_seconds: 60,
        }),
        concurrency_limit: None,
        concurrency_method: PolicyMethod::Enqueue,
        concurrency_parameters: Vec::new(),
        quotas: None,
    };
    enforcer.set_action_policy(action_id, action_policy);

    // First execution should be allowed
    let violation = enforcer
        .check_policies(action_id, Some(pack_id))
        .await
        .expect("Policy check failed");
    assert!(violation.is_none(), "First execution should be allowed");

    // Create an execution
    let action_ref = format!("test_pack_action_policy_{}.action", timestamp);
    create_test_execution(&pool, action_id, &action_ref, ExecutionStatus::Requested).await;

    // Second execution should be blocked by action-specific policy
    let violation = enforcer
        .check_policies(action_id, Some(pack_id))
        .await
        .expect("Policy check failed");
    assert!(
        violation.is_some(),
        "Second execution should be blocked by action policy"
    );

    // Cleanup
    cleanup_test_data(&pool, pack_id).await;
}

#[tokio::test]
#[ignore] // Requires database
async fn test_pack_specific_policy() {
    let pool = setup_db().await;
    let timestamp = Utc::now().timestamp();
    let pack_id = create_test_pack(&pool, &format!("pack_policy_{}", timestamp)).await;
    let action_id = create_test_action(&pool, pack_id, &format!("pack_policy_{}", timestamp)).await;
    let action_ref = format!("test_pack_pack_policy_{}.action", timestamp);

    // Create enforcer with no global policy
    let mut enforcer = PolicyEnforcer::new(pool.clone());

    // Set pack-specific policy
    let pack_policy = ExecutionPolicy {
        rate_limit: None,
        concurrency_limit: Some(1),
        concurrency_method: PolicyMethod::Enqueue,
        concurrency_parameters: Vec::new(),
        quotas: None,
    };
    enforcer.set_pack_policy(pack_id, pack_policy);

    // First running execution should be allowed
    let violation = enforcer
        .check_policies(action_id, Some(pack_id))
        .await
        .expect("Policy check failed");
    assert!(violation.is_none(), "First execution should be allowed");

    // Create a running execution
    create_test_execution(&pool, action_id, &action_ref, ExecutionStatus::Running).await;

    // Second execution should be blocked by pack policy
    let violation = enforcer
        .check_policies(action_id, Some(pack_id))
        .await
        .expect("Policy check failed");
    assert!(
        violation.is_some(),
        "Second execution should be blocked by pack policy"
    );

    // Cleanup
    cleanup_test_data(&pool, pack_id).await;
}

#[tokio::test]
#[ignore] // Requires database
async fn test_policy_priority() {
    let pool = setup_db().await;
    let timestamp = Utc::now().timestamp();
    let pack_id = create_test_pack(&pool, &format!("priority_{}", timestamp)).await;
    let action_id = create_test_action(&pool, pack_id, &format!("priority_{}", timestamp)).await;

    // Create enforcer with lenient global policy
    let global_policy = ExecutionPolicy {
        rate_limit: Some(RateLimit {
            max_executions: 100,
            window_seconds: 60,
        }),
        concurrency_limit: None,
        concurrency_method: PolicyMethod::Enqueue,
        concurrency_parameters: Vec::new(),
        quotas: None,
    };
    let mut enforcer = PolicyEnforcer::with_global_policy(pool.clone(), global_policy);

    // Set strict action-specific policy (should override global)
    let action_policy = ExecutionPolicy {
        rate_limit: Some(RateLimit {
            max_executions: 1,
            window_seconds: 60,
        }),
        concurrency_limit: None,
        concurrency_method: PolicyMethod::Enqueue,
        concurrency_parameters: Vec::new(),
        quotas: None,
    };
    enforcer.set_action_policy(action_id, action_policy);

    // First execution should be allowed
    let violation = enforcer
        .check_policies(action_id, Some(pack_id))
        .await
        .expect("Policy check failed");
    assert!(violation.is_none(), "First execution should be allowed");

    // Create an execution
    let action_ref = format!("test_pack_priority_{}.action", timestamp);
    create_test_execution(&pool, action_id, &action_ref, ExecutionStatus::Requested).await;

    // Second execution should be blocked by action policy (not global policy)
    let violation = enforcer
        .check_policies(action_id, Some(pack_id))
        .await
        .expect("Policy check failed");
    assert!(
        violation.is_some(),
        "Action policy should override global policy"
    );

    // Cleanup
    cleanup_test_data(&pool, pack_id).await;
}

#[test]
fn test_policy_violation_display() {
    use attune_executor::policy_enforcer::PolicyViolation;

    let violation = PolicyViolation::RateLimitExceeded {
        limit: 10,
        window_seconds: 60,
        current_count: 15,
    };
    let display = violation.to_string();
    assert!(display.contains("Rate limit exceeded"));
    assert!(display.contains("15"));
    assert!(display.contains("60"));
    assert!(display.contains("10"));

    let violation = PolicyViolation::ConcurrencyLimitExceeded {
        limit: 5,
        current_count: 8,
    };
    let display = violation.to_string();
    assert!(display.contains("Concurrency limit exceeded"));
    assert!(display.contains("8"));
    assert!(display.contains("5"));
}
