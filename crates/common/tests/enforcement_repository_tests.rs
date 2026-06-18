//! Integration tests for Enforcement repository
//!
//! These tests verify CRUD operations, queries, and constraints
//! for the Enforcement repository.

mod helpers;

use attune_common::{
    models::enums::{EnforcementCondition, EnforcementStatus},
    repositories::{
        event::{CreateEnforcementInput, EnforcementRepository, UpdateEnforcementInput},
        Create, Delete, FindById, List, Update,
    },
    Error,
};
use helpers::*;
use serde_json::json;

// ============================================================================
// CREATE Tests
// ============================================================================

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_create_enforcement_minimal() {
    let pool = create_test_pool().await.unwrap();

    // Create pack, trigger, action, and rule
    let pack = PackFixture::new_unique("enforcement_pack")
        .create(&pool)
        .await
        .unwrap();

    let trigger = TriggerFixture::new_unique(Some(pack.id), Some(pack.r#ref.clone()), "webhook")
        .create(&pool)
        .await
        .unwrap();

    let action = ActionFixture::new_unique(pack.id, &pack.r#ref, "action")
        .create(&pool)
        .await
        .unwrap();

    use attune_common::repositories::rule::{CreateRuleInput, RuleRepository};
    let rule = RuleRepository::create(
        &pool,
        CreateRuleInput {
            r#ref: format!("{}.test_rule", pack.r#ref),
            pack: pack.id,
            pack_ref: pack.r#ref.clone(),
            label: "Test Rule".to_string(),
            description: Some("Test".to_string()),
            action: action.id,
            action_ref: action.r#ref.clone(),
            trigger: trigger.id,
            trigger_ref: trigger.r#ref.clone(),
            conditions: json!({}),
            action_params: json!({}),
            trigger_params: json!({}),
            permission_set_refs: None,
            enabled: true,
            is_adhoc: false,
            owner_identity: None,
        },
    )
    .await
    .unwrap();

    // Create enforcement with minimal fields
    let input = CreateEnforcementInput {
        rule: Some(rule.id),
        rule_ref: rule.r#ref.clone(),
        trigger_ref: trigger.r#ref.clone(),
        config: None,
        event: None,
        status: EnforcementStatus::Created,
        payload: json!({}),
        condition: EnforcementCondition::All,
        conditions: json!([]),
    };

    let enforcement = EnforcementRepository::create(&pool, input).await.unwrap();

    assert!(enforcement.id > 0);
    assert_eq!(enforcement.rule, Some(rule.id));
    assert_eq!(enforcement.rule_ref, rule.r#ref);
    assert_eq!(enforcement.trigger_ref, trigger.r#ref);
    assert_eq!(enforcement.config, None);
    assert_eq!(enforcement.event, None);
    assert_eq!(enforcement.status, EnforcementStatus::Created);
    assert_eq!(enforcement.payload, json!({}));
    assert_eq!(enforcement.condition, EnforcementCondition::All);
    assert_eq!(enforcement.conditions, json!([]));
    assert!(enforcement.created.timestamp() > 0);
    assert_eq!(enforcement.resolved_at, None); // Not yet resolved
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_create_enforcement_with_event() {
    let pool = create_test_pool().await.unwrap();

    let pack = PackFixture::new_unique("event_enforcement_pack")
        .create(&pool)
        .await
        .unwrap();

    let trigger = TriggerFixture::new_unique(Some(pack.id), Some(pack.r#ref.clone()), "webhook")
        .create(&pool)
        .await
        .unwrap();

    let action = ActionFixture::new_unique(pack.id, &pack.r#ref, "action")
        .create(&pool)
        .await
        .unwrap();

    use attune_common::repositories::rule::{CreateRuleInput, RuleRepository};
    let rule = RuleRepository::create(
        &pool,
        CreateRuleInput {
            r#ref: format!("{}.test_rule", pack.r#ref),
            pack: pack.id,
            pack_ref: pack.r#ref.clone(),
            label: "Test Rule".to_string(),
            description: Some("Test".to_string()),
            action: action.id,
            action_ref: action.r#ref.clone(),
            trigger: trigger.id,
            trigger_ref: trigger.r#ref.clone(),
            conditions: json!({}),
            action_params: json!({}),
            trigger_params: json!({}),
            permission_set_refs: None,
            enabled: true,
            is_adhoc: false,
            owner_identity: None,
        },
    )
    .await
    .unwrap();

    // Create an event
    let event = EventFixture::new_unique(Some(trigger.id), &trigger.r#ref)
        .with_payload(json!({"event": "data"}))
        .create(&pool)
        .await
        .unwrap();

    let input = CreateEnforcementInput {
        rule: Some(rule.id),
        rule_ref: rule.r#ref.clone(),
        trigger_ref: trigger.r#ref.clone(),
        config: None,
        event: Some(event.id),
        status: EnforcementStatus::Created,
        payload: json!({"from": "event"}),
        condition: EnforcementCondition::All,
        conditions: json!([]),
    };

    let enforcement = EnforcementRepository::create(&pool, input).await.unwrap();

    assert_eq!(enforcement.event, Some(event.id));
    assert_eq!(enforcement.payload, json!({"from": "event"}));
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_create_enforcement_with_conditions() {
    let pool = create_test_pool().await.unwrap();

    let pack = PackFixture::new_unique("conditions_pack")
        .create(&pool)
        .await
        .unwrap();

    let trigger = TriggerFixture::new_unique(Some(pack.id), Some(pack.r#ref.clone()), "webhook")
        .create(&pool)
        .await
        .unwrap();

    let action = ActionFixture::new_unique(pack.id, &pack.r#ref, "action")
        .create(&pool)
        .await
        .unwrap();

    use attune_common::repositories::rule::{CreateRuleInput, RuleRepository};
    let rule = RuleRepository::create(
        &pool,
        CreateRuleInput {
            r#ref: format!("{}.test_rule", pack.r#ref),
            pack: pack.id,
            pack_ref: pack.r#ref.clone(),
            label: "Test Rule".to_string(),
            description: Some("Test".to_string()),
            action: action.id,
            action_ref: action.r#ref.clone(),
            trigger: trigger.id,
            trigger_ref: trigger.r#ref.clone(),
            conditions: json!({}),
            action_params: json!({}),
            trigger_params: json!({}),
            permission_set_refs: None,
            enabled: true,
            is_adhoc: false,
            owner_identity: None,
        },
    )
    .await
    .unwrap();

    let conditions = json!([
        {"equals": {"event.status": "success"}},
        {"greater_than": {"event.priority": 5}}
    ]);

    let input = CreateEnforcementInput {
        rule: Some(rule.id),
        rule_ref: rule.r#ref.clone(),
        trigger_ref: trigger.r#ref.clone(),
        config: None,
        event: None,
        status: EnforcementStatus::Created,
        payload: json!({}),
        condition: EnforcementCondition::All,
        conditions: conditions.clone(),
    };

    let enforcement = EnforcementRepository::create(&pool, input).await.unwrap();

    assert_eq!(enforcement.condition, EnforcementCondition::All);
    assert_eq!(enforcement.conditions, conditions);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_create_enforcement_with_any_condition() {
    let pool = create_test_pool().await.unwrap();

    let pack = PackFixture::new_unique("any_condition_pack")
        .create(&pool)
        .await
        .unwrap();

    let trigger = TriggerFixture::new_unique(Some(pack.id), Some(pack.r#ref.clone()), "webhook")
        .create(&pool)
        .await
        .unwrap();

    let action = ActionFixture::new_unique(pack.id, &pack.r#ref, "action")
        .create(&pool)
        .await
        .unwrap();

    use attune_common::repositories::rule::{CreateRuleInput, RuleRepository};
    let rule = RuleRepository::create(
        &pool,
        CreateRuleInput {
            r#ref: format!("{}.test_rule", pack.r#ref),
            pack: pack.id,
            pack_ref: pack.r#ref.clone(),
            label: "Test Rule".to_string(),
            description: Some("Test".to_string()),
            action: action.id,
            action_ref: action.r#ref.clone(),
            trigger: trigger.id,
            trigger_ref: trigger.r#ref.clone(),
            conditions: json!({}),
            action_params: json!({}),
            trigger_params: json!({}),
            permission_set_refs: None,
            enabled: true,
            is_adhoc: false,
            owner_identity: None,
        },
    )
    .await
    .unwrap();

    let input = CreateEnforcementInput {
        rule: Some(rule.id),
        rule_ref: rule.r#ref.clone(),
        trigger_ref: trigger.r#ref.clone(),
        config: None,
        event: None,
        status: EnforcementStatus::Created,
        payload: json!({}),
        condition: EnforcementCondition::Any,
        conditions: json!([
            {"equals": {"event.type": "webhook"}},
            {"equals": {"event.type": "timer"}}
        ]),
    };

    let enforcement = EnforcementRepository::create(&pool, input).await.unwrap();

    assert_eq!(enforcement.condition, EnforcementCondition::Any);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_create_enforcement_without_rule_id() {
    let pool = create_test_pool().await.unwrap();

    // Enforcements can be created without a rule ID (rule may have been deleted)
    let input = CreateEnforcementInput {
        rule: None,
        rule_ref: "deleted.rule".to_string(),
        trigger_ref: "some.trigger".to_string(),
        config: None,
        event: None,
        status: EnforcementStatus::Created,
        payload: json!({"reason": "rule was deleted"}),
        condition: EnforcementCondition::All,
        conditions: json!([]),
    };

    let enforcement = EnforcementRepository::create(&pool, input).await.unwrap();

    assert_eq!(enforcement.rule, None);
    assert_eq!(enforcement.rule_ref, "deleted.rule");
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_create_enforcement_with_invalid_rule_fails() {
    let pool = create_test_pool().await.unwrap();

    // Try to create enforcement with non-existent rule ID
    let input = CreateEnforcementInput {
        rule: Some(99999),
        rule_ref: "nonexistent.rule".to_string(),
        trigger_ref: "some.trigger".to_string(),
        config: None,
        event: None,
        status: EnforcementStatus::Created,
        payload: json!({}),
        condition: EnforcementCondition::All,
        conditions: json!([]),
    };

    let result = EnforcementRepository::create(&pool, input).await;

    assert!(result.is_err());
    // Foreign key constraint violation
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_create_enforcement_with_nonexistent_event_succeeds() {
    let pool = create_test_pool().await.unwrap();

    // The enforcement.event column has no FK constraint (event is a hypertable
    // and hypertables cannot be FK targets). A non-existent event ID is accepted
    // as a dangling reference.
    let input = CreateEnforcementInput {
        rule: None,
        rule_ref: "some.rule".to_string(),
        trigger_ref: "some.trigger".to_string(),
        config: None,
        event: Some(99999),
        status: EnforcementStatus::Created,
        payload: json!({}),
        condition: EnforcementCondition::All,
        conditions: json!([]),
    };

    let result = EnforcementRepository::create(&pool, input).await;

    assert!(result.is_ok());
    let enforcement = result.unwrap();
    assert_eq!(enforcement.event, Some(99999));
}

// ============================================================================
// READ Tests
// ============================================================================

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_find_enforcement_by_id() {
    let pool = create_test_pool().await.unwrap();

    let pack = PackFixture::new_unique("find_pack")
        .create(&pool)
        .await
        .unwrap();

    let trigger = TriggerFixture::new_unique(Some(pack.id), Some(pack.r#ref.clone()), "webhook")
        .create(&pool)
        .await
        .unwrap();

    let action = ActionFixture::new_unique(pack.id, &pack.r#ref, "action")
        .create(&pool)
        .await
        .unwrap();

    use attune_common::repositories::rule::{CreateRuleInput, RuleRepository};
    let rule = RuleRepository::create(
        &pool,
        CreateRuleInput {
            r#ref: format!("{}.test_rule", pack.r#ref),
            pack: pack.id,
            pack_ref: pack.r#ref.clone(),
            label: "Test Rule".to_string(),
            description: Some("Test".to_string()),
            action: action.id,
            action_ref: action.r#ref.clone(),
            trigger: trigger.id,
            trigger_ref: trigger.r#ref.clone(),
            conditions: json!({}),
            action_params: json!({}),
            trigger_params: json!({}),
            permission_set_refs: None,
            enabled: true,
            is_adhoc: false,
            owner_identity: None,
        },
    )
    .await
    .unwrap();

    let created_enforcement =
        EnforcementFixture::new_unique(Some(rule.id), &rule.r#ref, &trigger.r#ref)
            .with_payload(json!({"test": "data"}))
            .create(&pool)
            .await
            .unwrap();

    let found = EnforcementRepository::find_by_id(&pool, created_enforcement.id)
        .await
        .unwrap();

    assert!(found.is_some());
    let enforcement = found.unwrap();
    assert_eq!(enforcement.id, created_enforcement.id);
    assert_eq!(enforcement.rule, created_enforcement.rule);
    assert_eq!(enforcement.rule_ref, created_enforcement.rule_ref);
    assert_eq!(enforcement.status, created_enforcement.status);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_find_enforcement_by_id_not_found() {
    let pool = create_test_pool().await.unwrap();

    let result = EnforcementRepository::find_by_id(&pool, 99999)
        .await
        .unwrap();

    assert!(result.is_none());
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_get_enforcement_by_id() {
    let pool = create_test_pool().await.unwrap();

    let pack = PackFixture::new_unique("get_pack")
        .create(&pool)
        .await
        .unwrap();

    let trigger = TriggerFixture::new_unique(Some(pack.id), Some(pack.r#ref.clone()), "webhook")
        .create(&pool)
        .await
        .unwrap();

    let action = ActionFixture::new_unique(pack.id, &pack.r#ref, "action")
        .create(&pool)
        .await
        .unwrap();

    use attune_common::repositories::rule::{CreateRuleInput, RuleRepository};
    let rule = RuleRepository::create(
        &pool,
        CreateRuleInput {
            r#ref: format!("{}.test_rule", pack.r#ref),
            pack: pack.id,
            pack_ref: pack.r#ref.clone(),
            label: "Test Rule".to_string(),
            description: Some("Test".to_string()),
            action: action.id,
            action_ref: action.r#ref.clone(),
            trigger: trigger.id,
            trigger_ref: trigger.r#ref.clone(),
            conditions: json!({}),
            action_params: json!({}),
            trigger_params: json!({}),
            permission_set_refs: None,
            enabled: true,
            is_adhoc: false,
            owner_identity: None,
        },
    )
    .await
    .unwrap();

    let created_enforcement =
        EnforcementFixture::new_unique(Some(rule.id), &rule.r#ref, &trigger.r#ref)
            .create(&pool)
            .await
            .unwrap();

    let enforcement = EnforcementRepository::get_by_id(&pool, created_enforcement.id)
        .await
        .unwrap();

    assert_eq!(enforcement.id, created_enforcement.id);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_get_enforcement_by_id_not_found() {
    let pool = create_test_pool().await.unwrap();

    let result = EnforcementRepository::get_by_id(&pool, 99999).await;

    assert!(result.is_err());
    assert!(matches!(result.unwrap_err(), Error::NotFound { .. }));
}

// ============================================================================
// LIST Tests
// ============================================================================

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_list_enforcements_empty() {
    let pool = create_test_pool().await.unwrap();

    let enforcements = EnforcementRepository::list(&pool).await.unwrap();
    // May have enforcements from other tests, just verify we can list without error
    drop(enforcements);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_list_enforcements() {
    let pool = create_test_pool().await.unwrap();

    let pack = PackFixture::new_unique("list_pack")
        .create(&pool)
        .await
        .unwrap();

    let trigger = TriggerFixture::new_unique(Some(pack.id), Some(pack.r#ref.clone()), "webhook")
        .create(&pool)
        .await
        .unwrap();

    let action = ActionFixture::new_unique(pack.id, &pack.r#ref, "action")
        .create(&pool)
        .await
        .unwrap();

    use attune_common::repositories::rule::{CreateRuleInput, RuleRepository};
    let rule = RuleRepository::create(
        &pool,
        CreateRuleInput {
            r#ref: format!("{}.test_rule", pack.r#ref),
            pack: pack.id,
            pack_ref: pack.r#ref.clone(),
            label: "Test Rule".to_string(),
            description: Some("Test".to_string()),
            action: action.id,
            action_ref: action.r#ref.clone(),
            trigger: trigger.id,
            trigger_ref: trigger.r#ref.clone(),
            conditions: json!({}),
            action_params: json!({}),
            trigger_params: json!({}),
            permission_set_refs: None,
            enabled: true,
            is_adhoc: false,
            owner_identity: None,
        },
    )
    .await
    .unwrap();

    let before_count = EnforcementRepository::list(&pool).await.unwrap().len();

    // Create multiple enforcements
    let mut created_ids = vec![];
    for i in 0..3 {
        let enforcement =
            EnforcementFixture::new_unique(Some(rule.id), &rule.r#ref, &trigger.r#ref)
                .with_payload(json!({"index": i}))
                .create(&pool)
                .await
                .unwrap();
        created_ids.push(enforcement.id);
    }

    let enforcements = EnforcementRepository::list(&pool).await.unwrap();

    assert!(enforcements.len() >= before_count + 3);
    // Verify our enforcements are in the list
    let our_enforcements: Vec<_> = enforcements
        .iter()
        .filter(|e| created_ids.contains(&e.id))
        .collect();
    assert_eq!(our_enforcements.len(), 3);
}

// ============================================================================
// UPDATE Tests
// ============================================================================

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_update_enforcement_status() {
    let pool = create_test_pool().await.unwrap();

    let pack = PackFixture::new_unique("update_pack")
        .create(&pool)
        .await
        .unwrap();

    let trigger = TriggerFixture::new_unique(Some(pack.id), Some(pack.r#ref.clone()), "webhook")
        .create(&pool)
        .await
        .unwrap();

    let action = ActionFixture::new_unique(pack.id, &pack.r#ref, "action")
        .create(&pool)
        .await
        .unwrap();

    use attune_common::repositories::rule::{CreateRuleInput, RuleRepository};
    let rule = RuleRepository::create(
        &pool,
        CreateRuleInput {
            r#ref: format!("{}.test_rule", pack.r#ref),
            pack: pack.id,
            pack_ref: pack.r#ref.clone(),
            label: "Test Rule".to_string(),
            description: Some("Test".to_string()),
            action: action.id,
            action_ref: action.r#ref.clone(),
            trigger: trigger.id,
            trigger_ref: trigger.r#ref.clone(),
            conditions: json!({}),
            action_params: json!({}),
            trigger_params: json!({}),
            permission_set_refs: None,
            enabled: true,
            is_adhoc: false,
            owner_identity: None,
        },
    )
    .await
    .unwrap();

    let enforcement = EnforcementFixture::new_unique(Some(rule.id), &rule.r#ref, &trigger.r#ref)
        .with_status(EnforcementStatus::Created)
        .create(&pool)
        .await
        .unwrap();

    let now = chrono::Utc::now();
    let input = UpdateEnforcementInput {
        status: Some(EnforcementStatus::Processed),
        payload: None,
        resolved_at: Some(now),
    };

    let updated = EnforcementRepository::update(&pool, enforcement.id, input)
        .await
        .unwrap();

    assert_eq!(updated.id, enforcement.id);
    assert_eq!(updated.status, EnforcementStatus::Processed);
    assert!(updated.resolved_at.is_some());
    assert!(updated.resolved_at.unwrap() >= enforcement.created);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_update_enforcement_status_transitions() {
    let pool = create_test_pool().await.unwrap();

    let pack = PackFixture::new_unique("status_pack")
        .create(&pool)
        .await
        .unwrap();

    let trigger = TriggerFixture::new_unique(Some(pack.id), Some(pack.r#ref.clone()), "webhook")
        .create(&pool)
        .await
        .unwrap();

    let action = ActionFixture::new_unique(pack.id, &pack.r#ref, "action")
        .create(&pool)
        .await
        .unwrap();

    use attune_common::repositories::rule::{CreateRuleInput, RuleRepository};
    let rule = RuleRepository::create(
        &pool,
        CreateRuleInput {
            r#ref: format!("{}.test_rule", pack.r#ref),
            pack: pack.id,
            pack_ref: pack.r#ref.clone(),
            label: "Test Rule".to_string(),
            description: Some("Test".to_string()),
            action: action.id,
            action_ref: action.r#ref.clone(),
            trigger: trigger.id,
            trigger_ref: trigger.r#ref.clone(),
            conditions: json!({}),
            action_params: json!({}),
            trigger_params: json!({}),
            permission_set_refs: None,
            enabled: true,
            is_adhoc: false,
            owner_identity: None,
        },
    )
    .await
    .unwrap();

    let enforcement = EnforcementFixture::new_unique(Some(rule.id), &rule.r#ref, &trigger.r#ref)
        .create(&pool)
        .await
        .unwrap();

    // Test status transitions: Created -> Processed
    let now = chrono::Utc::now();
    let updated = EnforcementRepository::update(
        &pool,
        enforcement.id,
        UpdateEnforcementInput {
            status: Some(EnforcementStatus::Processed),
            payload: None,
            resolved_at: Some(now),
        },
    )
    .await
    .unwrap();
    assert_eq!(updated.status, EnforcementStatus::Processed);
    assert!(updated.resolved_at.is_some());

    // Test status transition: Processed -> Disabled (although unusual)
    let updated = EnforcementRepository::update(
        &pool,
        enforcement.id,
        UpdateEnforcementInput {
            status: Some(EnforcementStatus::Disabled),
            payload: None,
            resolved_at: None,
        },
    )
    .await
    .unwrap();
    assert_eq!(updated.status, EnforcementStatus::Disabled);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_update_enforcement_payload() {
    let pool = create_test_pool().await.unwrap();

    let pack = PackFixture::new_unique("payload_pack")
        .create(&pool)
        .await
        .unwrap();

    let trigger = TriggerFixture::new_unique(Some(pack.id), Some(pack.r#ref.clone()), "webhook")
        .create(&pool)
        .await
        .unwrap();

    let action = ActionFixture::new_unique(pack.id, &pack.r#ref, "action")
        .create(&pool)
        .await
        .unwrap();

    use attune_common::repositories::rule::{CreateRuleInput, RuleRepository};
    let rule = RuleRepository::create(
        &pool,
        CreateRuleInput {
            r#ref: format!("{}.test_rule", pack.r#ref),
            pack: pack.id,
            pack_ref: pack.r#ref.clone(),
            label: "Test Rule".to_string(),
            description: Some("Test".to_string()),
            action: action.id,
            action_ref: action.r#ref.clone(),
            trigger: trigger.id,
            trigger_ref: trigger.r#ref.clone(),
            conditions: json!({}),
            action_params: json!({}),
            trigger_params: json!({}),
            permission_set_refs: None,
            enabled: true,
            is_adhoc: false,
            owner_identity: None,
        },
    )
    .await
    .unwrap();

    let enforcement = EnforcementFixture::new_unique(Some(rule.id), &rule.r#ref, &trigger.r#ref)
        .with_payload(json!({"initial": "data"}))
        .create(&pool)
        .await
        .unwrap();

    let new_payload = json!({"updated": "data", "version": 2});
    let input = UpdateEnforcementInput {
        status: None,
        payload: Some(new_payload.clone()),
        resolved_at: None,
    };

    let updated = EnforcementRepository::update(&pool, enforcement.id, input)
        .await
        .unwrap();

    assert_eq!(updated.payload, new_payload);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_update_enforcement_both_fields() {
    let pool = create_test_pool().await.unwrap();

    let pack = PackFixture::new_unique("both_pack")
        .create(&pool)
        .await
        .unwrap();

    let trigger = TriggerFixture::new_unique(Some(pack.id), Some(pack.r#ref.clone()), "webhook")
        .create(&pool)
        .await
        .unwrap();

    let action = ActionFixture::new_unique(pack.id, &pack.r#ref, "action")
        .create(&pool)
        .await
        .unwrap();

    use attune_common::repositories::rule::{CreateRuleInput, RuleRepository};
    let rule = RuleRepository::create(
        &pool,
        CreateRuleInput {
            r#ref: format!("{}.test_rule", pack.r#ref),
            pack: pack.id,
            pack_ref: pack.r#ref.clone(),
            label: "Test Rule".to_string(),
            description: Some("Test".to_string()),
            action: action.id,
            action_ref: action.r#ref.clone(),
            trigger: trigger.id,
            trigger_ref: trigger.r#ref.clone(),
            conditions: json!({}),
            action_params: json!({}),
            trigger_params: json!({}),
            permission_set_refs: None,
            enabled: true,
            is_adhoc: false,
            owner_identity: None,
        },
    )
    .await
    .unwrap();

    let enforcement = EnforcementFixture::new_unique(Some(rule.id), &rule.r#ref, &trigger.r#ref)
        .create(&pool)
        .await
        .unwrap();

    let now = chrono::Utc::now();
    let new_payload = json!({"result": "success"});
    let input = UpdateEnforcementInput {
        status: Some(EnforcementStatus::Processed),
        payload: Some(new_payload.clone()),
        resolved_at: Some(now),
    };

    let updated = EnforcementRepository::update(&pool, enforcement.id, input)
        .await
        .unwrap();

    assert_eq!(updated.status, EnforcementStatus::Processed);
    assert_eq!(updated.payload, new_payload);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_update_enforcement_no_changes() {
    let pool = create_test_pool().await.unwrap();

    let pack = PackFixture::new_unique("nochange_pack")
        .create(&pool)
        .await
        .unwrap();

    let trigger = TriggerFixture::new_unique(Some(pack.id), Some(pack.r#ref.clone()), "webhook")
        .create(&pool)
        .await
        .unwrap();

    let action = ActionFixture::new_unique(pack.id, &pack.r#ref, "action")
        .create(&pool)
        .await
        .unwrap();

    use attune_common::repositories::rule::{CreateRuleInput, RuleRepository};
    let rule = RuleRepository::create(
        &pool,
        CreateRuleInput {
            r#ref: format!("{}.test_rule", pack.r#ref),
            pack: pack.id,
            pack_ref: pack.r#ref.clone(),
            label: "Test Rule".to_string(),
            description: Some("Test".to_string()),
            action: action.id,
            action_ref: action.r#ref.clone(),
            trigger: trigger.id,
            trigger_ref: trigger.r#ref.clone(),
            conditions: json!({}),
            action_params: json!({}),
            trigger_params: json!({}),
            permission_set_refs: None,
            enabled: true,
            is_adhoc: false,
            owner_identity: None,
        },
    )
    .await
    .unwrap();

    let enforcement = EnforcementFixture::new_unique(Some(rule.id), &rule.r#ref, &trigger.r#ref)
        .with_payload(json!({"test": "data"}))
        .create(&pool)
        .await
        .unwrap();

    let input = UpdateEnforcementInput {
        status: None,
        payload: None,
        resolved_at: None,
    };

    let result = EnforcementRepository::update(&pool, enforcement.id, input)
        .await
        .unwrap();

    // Should return existing enforcement without updating
    assert_eq!(result.id, enforcement.id);
    assert_eq!(result.status, enforcement.status);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_update_enforcement_not_found() {
    let pool = create_test_pool().await.unwrap();

    let input = UpdateEnforcementInput {
        status: Some(EnforcementStatus::Processed),
        payload: None,
        resolved_at: Some(chrono::Utc::now()),
    };

    let result = EnforcementRepository::update(&pool, 99999, input).await;

    // When updating non-existent entity with changes, SQLx returns RowNotFound error
    assert!(result.is_err());
}

// ============================================================================
// DELETE Tests
// ============================================================================

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_delete_enforcement() {
    let pool = create_test_pool().await.unwrap();

    let pack = PackFixture::new_unique("delete_pack")
        .create(&pool)
        .await
        .unwrap();

    let trigger = TriggerFixture::new_unique(Some(pack.id), Some(pack.r#ref.clone()), "webhook")
        .create(&pool)
        .await
        .unwrap();

    let action = ActionFixture::new_unique(pack.id, &pack.r#ref, "action")
        .create(&pool)
        .await
        .unwrap();

    use attune_common::repositories::rule::{CreateRuleInput, RuleRepository};
    let rule = RuleRepository::create(
        &pool,
        CreateRuleInput {
            r#ref: format!("{}.test_rule", pack.r#ref),
            pack: pack.id,
            pack_ref: pack.r#ref.clone(),
            label: "Test Rule".to_string(),
            description: Some("Test".to_string()),
            action: action.id,
            action_ref: action.r#ref.clone(),
            trigger: trigger.id,
            trigger_ref: trigger.r#ref.clone(),
            conditions: json!({}),
            action_params: json!({}),
            trigger_params: json!({}),
            permission_set_refs: None,
            enabled: true,
            is_adhoc: false,
            owner_identity: None,
        },
    )
    .await
    .unwrap();

    let enforcement = EnforcementFixture::new_unique(Some(rule.id), &rule.r#ref, &trigger.r#ref)
        .create(&pool)
        .await
        .unwrap();

    let deleted = EnforcementRepository::delete(&pool, enforcement.id)
        .await
        .unwrap();

    assert!(deleted);

    // Verify it's gone
    let found = EnforcementRepository::find_by_id(&pool, enforcement.id)
        .await
        .unwrap();
    assert!(found.is_none());
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_delete_enforcement_not_found() {
    let pool = create_test_pool().await.unwrap();

    let deleted = EnforcementRepository::delete(&pool, 99999).await.unwrap();

    assert!(!deleted);
}

// ============================================================================
// SPECIALIZED QUERY Tests
// ============================================================================

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_find_enforcements_by_rule() {
    let pool = create_test_pool().await.unwrap();

    let pack = PackFixture::new_unique("rule_query_pack")
        .create(&pool)
        .await
        .unwrap();

    let trigger = TriggerFixture::new_unique(Some(pack.id), Some(pack.r#ref.clone()), "webhook")
        .create(&pool)
        .await
        .unwrap();

    let action = ActionFixture::new_unique(pack.id, &pack.r#ref, "action")
        .create(&pool)
        .await
        .unwrap();

    use attune_common::repositories::rule::{CreateRuleInput, RuleRepository};
    let rule1 = RuleRepository::create(
        &pool,
        CreateRuleInput {
            r#ref: format!("{}.rule1", pack.r#ref),
            pack: pack.id,
            pack_ref: pack.r#ref.clone(),
            label: "Rule 1".to_string(),
            description: Some("Test".to_string()),
            action: action.id,
            action_ref: action.r#ref.clone(),
            trigger: trigger.id,
            trigger_ref: trigger.r#ref.clone(),
            conditions: json!({}),
            action_params: json!({}),
            trigger_params: json!({}),
            permission_set_refs: None,
            enabled: true,
            is_adhoc: false,
            owner_identity: None,
        },
    )
    .await
    .unwrap();

    let rule2 = RuleRepository::create(
        &pool,
        CreateRuleInput {
            r#ref: format!("{}.rule2", pack.r#ref),
            pack: pack.id,
            pack_ref: pack.r#ref.clone(),
            label: "Rule 2".to_string(),
            description: Some("Test".to_string()),
            action: action.id,
            action_ref: action.r#ref.clone(),
            trigger: trigger.id,
            trigger_ref: trigger.r#ref.clone(),
            conditions: json!({}),
            action_params: json!({}),
            trigger_params: json!({}),
            permission_set_refs: None,
            enabled: true,
            is_adhoc: false,
            owner_identity: None,
        },
    )
    .await
    .unwrap();

    // Create enforcements for rule1
    for i in 0..3 {
        EnforcementFixture::new_unique(Some(rule1.id), &rule1.r#ref, &trigger.r#ref)
            .with_payload(json!({"rule": 1, "index": i}))
            .create(&pool)
            .await
            .unwrap();
    }

    // Create enforcements for rule2
    for i in 0..2 {
        EnforcementFixture::new_unique(Some(rule2.id), &rule2.r#ref, &trigger.r#ref)
            .with_payload(json!({"rule": 2, "index": i}))
            .create(&pool)
            .await
            .unwrap();
    }

    let enforcements = EnforcementRepository::find_by_rule(&pool, rule1.id)
        .await
        .unwrap();

    assert_eq!(enforcements.len(), 3);
    for enforcement in &enforcements {
        assert_eq!(enforcement.rule, Some(rule1.id));
    }
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_find_enforcements_by_status() {
    let pool = create_test_pool().await.unwrap();

    let pack = PackFixture::new_unique("status_query_pack")
        .create(&pool)
        .await
        .unwrap();

    let trigger = TriggerFixture::new_unique(Some(pack.id), Some(pack.r#ref.clone()), "webhook")
        .create(&pool)
        .await
        .unwrap();

    let action = ActionFixture::new_unique(pack.id, &pack.r#ref, "action")
        .create(&pool)
        .await
        .unwrap();

    use attune_common::repositories::rule::{CreateRuleInput, RuleRepository};
    let rule = RuleRepository::create(
        &pool,
        CreateRuleInput {
            r#ref: format!("{}.test_rule", pack.r#ref),
            pack: pack.id,
            pack_ref: pack.r#ref.clone(),
            label: "Test Rule".to_string(),
            description: Some("Test".to_string()),
            action: action.id,
            action_ref: action.r#ref.clone(),
            trigger: trigger.id,
            trigger_ref: trigger.r#ref.clone(),
            conditions: json!({}),
            action_params: json!({}),
            trigger_params: json!({}),
            permission_set_refs: None,
            enabled: true,
            is_adhoc: false,
            owner_identity: None,
        },
    )
    .await
    .unwrap();

    // Create enforcements with different statuses
    let enf1 = EnforcementFixture::new_unique(Some(rule.id), &rule.r#ref, &trigger.r#ref)
        .with_status(EnforcementStatus::Created)
        .create(&pool)
        .await
        .unwrap();

    let enf2 = EnforcementFixture::new_unique(Some(rule.id), &rule.r#ref, &trigger.r#ref)
        .with_status(EnforcementStatus::Processed)
        .create(&pool)
        .await
        .unwrap();

    let enf3 = EnforcementFixture::new_unique(Some(rule.id), &rule.r#ref, &trigger.r#ref)
        .with_status(EnforcementStatus::Processed)
        .create(&pool)
        .await
        .unwrap();

    let processed_enforcements =
        EnforcementRepository::find_by_status(&pool, EnforcementStatus::Processed)
            .await
            .unwrap();

    // Filter to only our test enforcements
    let our_processed: Vec<_> = processed_enforcements
        .iter()
        .filter(|e| e.id == enf2.id || e.id == enf3.id)
        .collect();
    assert_eq!(our_processed.len(), 2);
    for enforcement in &our_processed {
        assert_eq!(enforcement.status, EnforcementStatus::Processed);
    }

    let created_enforcements =
        EnforcementRepository::find_by_status(&pool, EnforcementStatus::Created)
            .await
            .unwrap();

    // Verify our created enforcement is in the list
    let our_created: Vec<_> = created_enforcements
        .iter()
        .filter(|e| e.id == enf1.id)
        .collect();
    assert_eq!(our_created.len(), 1);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_find_enforcements_by_event() {
    let pool = create_test_pool().await.unwrap();

    let pack = PackFixture::new_unique("event_query_pack")
        .create(&pool)
        .await
        .unwrap();

    let trigger = TriggerFixture::new_unique(Some(pack.id), Some(pack.r#ref.clone()), "webhook")
        .create(&pool)
        .await
        .unwrap();

    let action = ActionFixture::new_unique(pack.id, &pack.r#ref, "action")
        .create(&pool)
        .await
        .unwrap();

    use attune_common::repositories::rule::{CreateRuleInput, RuleRepository};
    let mut rules = Vec::new();
    for i in 0..3 {
        let rule = RuleRepository::create(
            &pool,
            CreateRuleInput {
                r#ref: format!("{}.test_rule_{}", pack.r#ref, i),
                pack: pack.id,
                pack_ref: pack.r#ref.clone(),
                label: format!("Test Rule {}", i),
                description: Some("Test".to_string()),
                action: action.id,
                action_ref: action.r#ref.clone(),
                trigger: trigger.id,
                trigger_ref: trigger.r#ref.clone(),
                conditions: json!({}),
                action_params: json!({}),
                trigger_params: json!({}),
                permission_set_refs: None,
                enabled: true,
                is_adhoc: false,
                owner_identity: None,
            },
        )
        .await
        .unwrap();
        rules.push(rule);
    }

    // Create events
    let event1 = EventFixture::new_unique(Some(trigger.id), &trigger.r#ref)
        .create(&pool)
        .await
        .unwrap();

    let event2 = EventFixture::new_unique(Some(trigger.id), &trigger.r#ref)
        .create(&pool)
        .await
        .unwrap();

    // Create enforcements for event1 across multiple rules.
    for (i, rule) in rules.iter().enumerate() {
        EnforcementFixture::new_unique(Some(rule.id), &rule.r#ref, &trigger.r#ref)
            .with_event(event1.id)
            .with_payload(json!({"event": 1, "index": i}))
            .create(&pool)
            .await
            .unwrap();
    }

    // Create enforcement for event2
    EnforcementFixture::new_unique(Some(rules[0].id), &rules[0].r#ref, &trigger.r#ref)
        .with_event(event2.id)
        .create(&pool)
        .await
        .unwrap();

    let enforcements = EnforcementRepository::find_by_event(&pool, event1.id)
        .await
        .unwrap();

    assert_eq!(enforcements.len(), 3);
    for enforcement in &enforcements {
        assert_eq!(enforcement.event, Some(event1.id));
    }
}

// ============================================================================
// CASCADE & RELATIONSHIP Tests
// ============================================================================

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_delete_rule_sets_enforcement_rule_to_null() {
    let pool = create_test_pool().await.unwrap();

    let pack = PackFixture::new_unique("cascade_rule_pack")
        .create(&pool)
        .await
        .unwrap();

    let trigger = TriggerFixture::new_unique(Some(pack.id), Some(pack.r#ref.clone()), "webhook")
        .create(&pool)
        .await
        .unwrap();

    let action = ActionFixture::new_unique(pack.id, &pack.r#ref, "action")
        .create(&pool)
        .await
        .unwrap();

    use attune_common::repositories::rule::{CreateRuleInput, RuleRepository};
    let rule = RuleRepository::create(
        &pool,
        CreateRuleInput {
            r#ref: format!("{}.test_rule", pack.r#ref),
            pack: pack.id,
            pack_ref: pack.r#ref.clone(),
            label: "Test Rule".to_string(),
            description: Some("Test".to_string()),
            action: action.id,
            action_ref: action.r#ref.clone(),
            trigger: trigger.id,
            trigger_ref: trigger.r#ref.clone(),
            conditions: json!({}),
            action_params: json!({}),
            trigger_params: json!({}),
            permission_set_refs: None,
            enabled: true,
            is_adhoc: false,
            owner_identity: None,
        },
    )
    .await
    .unwrap();

    let enforcement = EnforcementFixture::new(Some(rule.id), &rule.r#ref, &trigger.r#ref)
        .create(&pool)
        .await
        .unwrap();

    // Delete the rule
    use attune_common::repositories::Delete;
    RuleRepository::delete(&pool, rule.id).await.unwrap();

    // Enforcement should still exist but with NULL rule (ON DELETE SET NULL)
    let found_enforcement = EnforcementRepository::find_by_id(&pool, enforcement.id)
        .await
        .unwrap()
        .unwrap();

    assert_eq!(found_enforcement.rule, None);
    assert_eq!(found_enforcement.rule_ref, rule.r#ref); // rule_ref preserved
}

// ============================================================================
// TIMESTAMP Tests
// ============================================================================

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_enforcement_resolved_at_lifecycle() {
    let pool = create_test_pool().await.unwrap();

    let pack = PackFixture::new_unique("timestamp_pack")
        .create(&pool)
        .await
        .unwrap();

    let trigger = TriggerFixture::new_unique(Some(pack.id), Some(pack.r#ref.clone()), "webhook")
        .create(&pool)
        .await
        .unwrap();

    let action = ActionFixture::new_unique(pack.id, &pack.r#ref, "action")
        .create(&pool)
        .await
        .unwrap();

    use attune_common::repositories::rule::{CreateRuleInput, RuleRepository};
    let rule = RuleRepository::create(
        &pool,
        CreateRuleInput {
            r#ref: format!("{}.test_rule", pack.r#ref),
            pack: pack.id,
            pack_ref: pack.r#ref.clone(),
            label: "Test Rule".to_string(),
            description: Some("Test".to_string()),
            action: action.id,
            action_ref: action.r#ref.clone(),
            trigger: trigger.id,
            trigger_ref: trigger.r#ref.clone(),
            conditions: json!({}),
            action_params: json!({}),
            trigger_params: json!({}),
            permission_set_refs: None,
            enabled: true,
            is_adhoc: false,
            owner_identity: None,
        },
    )
    .await
    .unwrap();

    let enforcement = EnforcementFixture::new_unique(Some(rule.id), &rule.r#ref, &trigger.r#ref)
        .create(&pool)
        .await
        .unwrap();

    // Initially, resolved_at is NULL
    assert!(enforcement.created.timestamp() > 0);
    assert_eq!(enforcement.resolved_at, None);

    // Resolve the enforcement and verify resolved_at is set
    let resolved_time = chrono::Utc::now();
    let input = UpdateEnforcementInput {
        status: Some(EnforcementStatus::Processed),
        payload: None,
        resolved_at: Some(resolved_time),
    };

    let updated = EnforcementRepository::update(&pool, enforcement.id, input)
        .await
        .unwrap();

    assert_eq!(updated.created, enforcement.created); // created unchanged
    assert!(updated.resolved_at.is_some());
    assert!(updated.resolved_at.unwrap() >= enforcement.created);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_update_loaded_enforcement_uses_loaded_locator() {
    let pool = create_test_pool().await.unwrap();

    let pack = PackFixture::new_unique("targeted_update_pack")
        .create(&pool)
        .await
        .unwrap();

    let trigger = TriggerFixture::new_unique(Some(pack.id), Some(pack.r#ref.clone()), "webhook")
        .create(&pool)
        .await
        .unwrap();

    let action = ActionFixture::new_unique(pack.id, &pack.r#ref, "action")
        .create(&pool)
        .await
        .unwrap();

    use attune_common::repositories::rule::{CreateRuleInput, RuleRepository};
    let rule = RuleRepository::create(
        &pool,
        CreateRuleInput {
            r#ref: format!("{}.test_rule", pack.r#ref),
            pack: pack.id,
            pack_ref: pack.r#ref.clone(),
            label: "Test Rule".to_string(),
            description: Some("Test".to_string()),
            action: action.id,
            action_ref: action.r#ref.clone(),
            trigger: trigger.id,
            trigger_ref: trigger.r#ref.clone(),
            conditions: json!({}),
            action_params: json!({}),
            trigger_params: json!({}),
            permission_set_refs: None,
            enabled: true,
            is_adhoc: false,
            owner_identity: None,
        },
    )
    .await
    .unwrap();

    let enforcement = EnforcementFixture::new_unique(Some(rule.id), &rule.r#ref, &trigger.r#ref)
        .create(&pool)
        .await
        .unwrap();

    let updated = EnforcementRepository::update_loaded(
        &pool,
        &enforcement,
        UpdateEnforcementInput {
            status: Some(EnforcementStatus::Processed),
            payload: None,
            resolved_at: Some(chrono::Utc::now()),
        },
    )
    .await
    .unwrap();

    assert_eq!(updated.id, enforcement.id);
    assert_eq!(updated.created, enforcement.created);
    assert_eq!(updated.rule_ref, enforcement.rule_ref);
    assert_eq!(updated.status, EnforcementStatus::Processed);
    assert!(updated.resolved_at.is_some());
}
