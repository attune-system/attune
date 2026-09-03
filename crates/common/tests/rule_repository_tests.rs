//! Integration tests for Rule repository
//!
//! These tests verify CRUD operations, queries, and constraints
//! for the Rule repository.

mod helpers;

use attune_common::{
    repositories::{
        rule::{CreateRuleInput, RuleRepository, RuleSensorPlacementInput, UpdateRuleInput},
        sensor_admission::{
            SensorAdmissionFailureKind, SensorAdmissionRepository, SensorAdmissionRequirement,
        },
        Create, Delete, FindById, FindByRef, List, Patch, Update,
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
async fn test_create_rule() {
    let pool = create_test_pool().await.unwrap();

    // Setup: Create pack, action, and trigger
    let pack = PackFixture::new_unique("test_pack")
        .create(&pool)
        .await
        .unwrap();

    let action = ActionFixture::new_unique(pack.id, &pack.r#ref, "test_action")
        .create(&pool)
        .await
        .unwrap();

    let trigger =
        TriggerFixture::new_unique(Some(pack.id), Some(pack.r#ref.clone()), "test_trigger")
            .create(&pool)
            .await
            .unwrap();

    // Create rule
    let rule_ref = format!("{}.test_rule", pack.r#ref);
    let input = CreateRuleInput {
        r#ref: rule_ref.clone(),
        pack: pack.id,
        pack_ref: pack.r#ref.clone(),
        label: "Test Rule".to_string(),
        description: Some("A test rule".to_string()),
        action: action.id,
        action_ref: action.r#ref.clone(),
        trigger: trigger.id,
        trigger_ref: trigger.r#ref.clone(),
        conditions: json!({"equals": {"event.status": "success"}}),
        action_params: json!({}),
        trigger_params: json!({}),
        trace_tag_template: None,
        permission_set_refs: None,
        enabled: true,
        is_adhoc: false,
        owner_identity: None,
    };

    let rule = RuleRepository::create_with_sensor_placement(
        &pool,
        input,
        RuleSensorPlacementInput {
            selector: json!({"site": "edge"}),
            tolerations: json!([]),
            affinity: json!({}),
        },
    )
    .await
    .unwrap();

    assert_eq!(rule.r#ref, rule_ref);
    assert_eq!(rule.pack, pack.id);
    assert_eq!(rule.pack_ref, pack.r#ref);
    assert_eq!(rule.label, "Test Rule");
    assert_eq!(rule.description, Some("A test rule".to_string()));
    assert_eq!(rule.action, Some(action.id));
    assert_eq!(rule.action_ref, action.r#ref);
    assert_eq!(rule.trigger, Some(trigger.id));
    assert_eq!(rule.trigger_ref, trigger.r#ref);
    assert_eq!(rule.sensor_worker_selector, json!({"site": "edge"}));
    let mut connection = pool.acquire().await.unwrap();
    let failures = SensorAdmissionRepository::assess_rule(
        &mut connection,
        rule.id,
        SensorAdmissionRequirement::Structural,
    )
    .await
    .unwrap();
    assert_eq!(failures.len(), 1);
    assert_eq!(
        failures[0].kind,
        SensorAdmissionFailureKind::PlacementOnUnmanagedTrigger
    );
    assert_eq!(
        rule.conditions,
        json!({"equals": {"event.status": "success"}})
    );
    assert!(rule.enabled);
    assert!(rule.created.timestamp() > 0);
    assert!(rule.updated.timestamp() > 0);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_create_rule_disabled() {
    let pool = create_test_pool().await.unwrap();

    let pack = PackFixture::new_unique("disabled_pack")
        .create(&pool)
        .await
        .unwrap();

    let action = ActionFixture::new_unique(pack.id, &pack.r#ref, "action")
        .create(&pool)
        .await
        .unwrap();

    let trigger = TriggerFixture::new_unique(Some(pack.id), Some(pack.r#ref.clone()), "trigger")
        .create(&pool)
        .await
        .unwrap();

    let input = CreateRuleInput {
        r#ref: format!("{}.disabled_rule", pack.r#ref),
        pack: pack.id,
        pack_ref: pack.r#ref.clone(),
        label: "Disabled Rule".to_string(),
        description: Some("A disabled rule".to_string()),
        action: action.id,
        action_ref: action.r#ref.clone(),
        trigger: trigger.id,
        trigger_ref: trigger.r#ref.clone(),
        conditions: json!([]),
        action_params: json!({}),
        trigger_params: json!({}),
        trace_tag_template: None,
        permission_set_refs: None,
        enabled: false,
        is_adhoc: false,
        owner_identity: None,
    };

    let rule = RuleRepository::create(&pool, input).await.unwrap();

    assert!(!rule.enabled);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_create_rule_with_complex_conditions() {
    let pool = create_test_pool().await.unwrap();

    let pack = PackFixture::new_unique("complex_pack")
        .create(&pool)
        .await
        .unwrap();

    let action = ActionFixture::new_unique(pack.id, &pack.r#ref, "action")
        .create(&pool)
        .await
        .unwrap();

    let trigger = TriggerFixture::new_unique(Some(pack.id), Some(pack.r#ref.clone()), "trigger")
        .create(&pool)
        .await
        .unwrap();

    let conditions = json!({
        "and": [
            {"equals": {"event.type": "webhook"}},
            {"greater_than": {"event.priority": 5}},
            {"contains": {"event.tags": "important"}}
        ]
    });

    let input = CreateRuleInput {
        r#ref: format!("{}.complex_rule", pack.r#ref),
        pack: pack.id,
        pack_ref: pack.r#ref.clone(),
        label: "Complex Rule".to_string(),
        description: Some("Rule with complex conditions".to_string()),
        action: action.id,
        action_ref: action.r#ref.clone(),
        trigger: trigger.id,
        trigger_ref: trigger.r#ref.clone(),
        conditions: conditions.clone(),
        action_params: json!({}),
        trigger_params: json!({}),
        trace_tag_template: None,
        permission_set_refs: None,
        enabled: true,
        is_adhoc: false,
        owner_identity: None,
    };

    let rule = RuleRepository::create(&pool, input).await.unwrap();

    assert_eq!(rule.conditions, conditions);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_create_rule_duplicate_ref() {
    let pool = create_test_pool().await.unwrap();

    let pack = PackFixture::new_unique("dup_pack")
        .create(&pool)
        .await
        .unwrap();

    let action = ActionFixture::new_unique(pack.id, &pack.r#ref, "action")
        .create(&pool)
        .await
        .unwrap();

    let trigger = TriggerFixture::new_unique(Some(pack.id), Some(pack.r#ref.clone()), "trigger")
        .create(&pool)
        .await
        .unwrap();

    let rule_ref = format!("{}.duplicate_rule", pack.r#ref);

    // Create first rule
    let input1 = CreateRuleInput {
        r#ref: rule_ref.clone(),
        pack: pack.id,
        pack_ref: pack.r#ref.clone(),
        label: "First Rule".to_string(),
        description: Some("First".to_string()),
        action: action.id,
        action_ref: action.r#ref.clone(),
        trigger: trigger.id,
        trigger_ref: trigger.r#ref.clone(),
        conditions: json!([]),
        action_params: json!({}),
        trigger_params: json!({}),
        trace_tag_template: None,
        permission_set_refs: None,
        enabled: true,
        is_adhoc: false,
        owner_identity: None,
    };

    RuleRepository::create(&pool, input1).await.unwrap();

    // Try to create second rule with same ref
    let input2 = CreateRuleInput {
        r#ref: rule_ref.clone(),
        pack: pack.id,
        pack_ref: pack.r#ref.clone(),
        label: "Second Rule".to_string(),
        description: Some("Second".to_string()),
        action: action.id,
        action_ref: action.r#ref.clone(),
        trigger: trigger.id,
        trigger_ref: trigger.r#ref.clone(),
        conditions: json!([]),
        action_params: json!({}),
        trigger_params: json!({}),
        trace_tag_template: None,
        permission_set_refs: None,
        enabled: true,
        is_adhoc: false,
        owner_identity: None,
    };

    let result = RuleRepository::create(&pool, input2).await;

    assert!(result.is_err());
    match result.unwrap_err() {
        Error::AlreadyExists {
            entity,
            field,
            value,
        } => {
            assert_eq!(entity, "Rule");
            assert_eq!(field, "ref");
            assert_eq!(value, rule_ref);
        }
        _ => panic!("Expected AlreadyExists error"),
    }
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_create_rule_invalid_ref_format_uppercase() {
    let pool = create_test_pool().await.unwrap();

    let pack = PackFixture::new_unique("upper_pack")
        .create(&pool)
        .await
        .unwrap();

    let action = ActionFixture::new_unique(pack.id, &pack.r#ref, "action")
        .create(&pool)
        .await
        .unwrap();

    let trigger = TriggerFixture::new_unique(Some(pack.id), Some(pack.r#ref.clone()), "trigger")
        .create(&pool)
        .await
        .unwrap();

    let input = CreateRuleInput {
        r#ref: format!("{}.UPPERCASE_RULE", pack.r#ref),
        pack: pack.id,
        pack_ref: pack.r#ref.clone(),
        label: "Upper Rule".to_string(),
        description: Some("Invalid uppercase ref".to_string()),
        action: action.id,
        action_ref: action.r#ref.clone(),
        trigger: trigger.id,
        trigger_ref: trigger.r#ref.clone(),
        conditions: json!([]),
        action_params: json!({}),
        trigger_params: json!({}),
        trace_tag_template: None,
        permission_set_refs: None,
        enabled: true,
        is_adhoc: false,
        owner_identity: None,
    };

    let result = RuleRepository::create(&pool, input).await;

    assert!(result.is_err());
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_create_rule_invalid_ref_format_no_dot() {
    let pool = create_test_pool().await.unwrap();

    let pack = PackFixture::new_unique("nodot_pack")
        .create(&pool)
        .await
        .unwrap();

    let action = ActionFixture::new_unique(pack.id, &pack.r#ref, "action")
        .create(&pool)
        .await
        .unwrap();

    let trigger = TriggerFixture::new_unique(Some(pack.id), Some(pack.r#ref.clone()), "trigger")
        .create(&pool)
        .await
        .unwrap();

    let input = CreateRuleInput {
        r#ref: "nodotinref".to_string(),
        pack: pack.id,
        pack_ref: pack.r#ref.clone(),
        label: "No Dot Rule".to_string(),
        description: Some("Invalid ref without dot".to_string()),
        action: action.id,
        action_ref: action.r#ref.clone(),
        trigger: trigger.id,
        trigger_ref: trigger.r#ref.clone(),
        conditions: json!([]),
        action_params: json!({}),
        trigger_params: json!({}),
        trace_tag_template: None,
        permission_set_refs: None,
        enabled: true,
        is_adhoc: false,
        owner_identity: None,
    };

    let result = RuleRepository::create(&pool, input).await;

    assert!(result.is_err());
}

// ============================================================================
// READ Tests
// ============================================================================

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_find_rule_by_id() {
    let pool = create_test_pool().await.unwrap();

    let pack = PackFixture::new_unique("find_pack")
        .create(&pool)
        .await
        .unwrap();

    let action = ActionFixture::new_unique(pack.id, &pack.r#ref, "action")
        .create(&pool)
        .await
        .unwrap();

    let trigger = TriggerFixture::new_unique(Some(pack.id), Some(pack.r#ref.clone()), "trigger")
        .create(&pool)
        .await
        .unwrap();

    let input = CreateRuleInput {
        r#ref: format!("{}.find_rule", pack.r#ref),
        pack: pack.id,
        pack_ref: pack.r#ref.clone(),
        label: "Find Rule".to_string(),
        description: Some("Rule to find".to_string()),
        action: action.id,
        action_ref: action.r#ref.clone(),
        trigger: trigger.id,
        trigger_ref: trigger.r#ref.clone(),
        conditions: json!([]),
        action_params: json!({}),
        trigger_params: json!({}),
        trace_tag_template: None,
        permission_set_refs: None,
        enabled: true,
        is_adhoc: false,
        owner_identity: None,
    };

    let created = RuleRepository::create(&pool, input).await.unwrap();

    let found = RuleRepository::find_by_id(&pool, created.id)
        .await
        .unwrap()
        .expect("Rule should exist");

    assert_eq!(found.id, created.id);
    assert_eq!(found.r#ref, created.r#ref);
    assert_eq!(found.label, created.label);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_find_rule_by_id_not_found() {
    let pool = create_test_pool().await.unwrap();

    let result = RuleRepository::find_by_id(&pool, 999999).await.unwrap();

    assert!(result.is_none());
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_find_rule_by_ref() {
    let pool = create_test_pool().await.unwrap();

    let pack = PackFixture::new_unique("ref_pack")
        .create(&pool)
        .await
        .unwrap();

    let action = ActionFixture::new_unique(pack.id, &pack.r#ref, "action")
        .create(&pool)
        .await
        .unwrap();

    let trigger = TriggerFixture::new_unique(Some(pack.id), Some(pack.r#ref.clone()), "trigger")
        .create(&pool)
        .await
        .unwrap();

    let rule_ref = format!("{}.find_by_ref", pack.r#ref);
    let input = CreateRuleInput {
        r#ref: rule_ref.clone(),
        pack: pack.id,
        pack_ref: pack.r#ref.clone(),
        label: "Find By Ref Rule".to_string(),
        description: Some("Find by ref".to_string()),
        action: action.id,
        action_ref: action.r#ref.clone(),
        trigger: trigger.id,
        trigger_ref: trigger.r#ref.clone(),
        conditions: json!([]),
        action_params: json!({}),
        trigger_params: json!({}),
        trace_tag_template: None,
        permission_set_refs: None,
        enabled: true,
        is_adhoc: false,
        owner_identity: None,
    };

    let created = RuleRepository::create(&pool, input).await.unwrap();

    let found = RuleRepository::find_by_ref(&pool, &rule_ref)
        .await
        .unwrap()
        .expect("Rule should exist");

    assert_eq!(found.id, created.id);
    assert_eq!(found.r#ref, rule_ref);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_find_rule_by_ref_not_found() {
    let pool = create_test_pool().await.unwrap();

    let result = RuleRepository::find_by_ref(&pool, "nonexistent.rule")
        .await
        .unwrap();

    assert!(result.is_none());
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_list_rules() {
    let pool = create_test_pool().await.unwrap();

    let pack = PackFixture::new_unique("list_pack")
        .create(&pool)
        .await
        .unwrap();

    let action = ActionFixture::new_unique(pack.id, &pack.r#ref, "action")
        .create(&pool)
        .await
        .unwrap();

    let trigger = TriggerFixture::new_unique(Some(pack.id), Some(pack.r#ref.clone()), "trigger")
        .create(&pool)
        .await
        .unwrap();

    // Create multiple rules
    for i in 1..=3 {
        let input = CreateRuleInput {
            r#ref: format!("{}.list_rule_{}", pack.r#ref, i),
            pack: pack.id,
            pack_ref: pack.r#ref.clone(),
            label: format!("List Rule {}", i),
            description: Some(format!("Rule {}", i)),
            action: action.id,
            action_ref: action.r#ref.clone(),
            trigger: trigger.id,
            trigger_ref: trigger.r#ref.clone(),
            conditions: json!([]),
            action_params: json!({}),
            trigger_params: json!({}),
            trace_tag_template: None,
            permission_set_refs: None,
            enabled: true,
            is_adhoc: false,
            owner_identity: None,
        };

        RuleRepository::create(&pool, input).await.unwrap();
    }

    let rules = RuleRepository::list(&pool).await.unwrap();

    // Should have at least our 3 rules (may have more from parallel tests)
    let our_rules: Vec<_> = rules
        .iter()
        .filter(|r| r.r#ref.starts_with(&pack.r#ref))
        .collect();

    assert_eq!(our_rules.len(), 3);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_list_rules_ordered_by_ref() {
    let pool = create_test_pool().await.unwrap();

    let pack = PackFixture::new_unique("order_pack")
        .create(&pool)
        .await
        .unwrap();

    let action = ActionFixture::new_unique(pack.id, &pack.r#ref, "action")
        .create(&pool)
        .await
        .unwrap();

    let trigger = TriggerFixture::new_unique(Some(pack.id), Some(pack.r#ref.clone()), "trigger")
        .create(&pool)
        .await
        .unwrap();

    // Create rules in non-alphabetical order
    let names = vec!["charlie", "alice", "bob"];
    for name in names {
        let input = CreateRuleInput {
            r#ref: format!("{}.{}", pack.r#ref, name),
            pack: pack.id,
            pack_ref: pack.r#ref.clone(),
            label: name.to_string(),
            description: Some(name.to_string()),
            action: action.id,
            action_ref: action.r#ref.clone(),
            trigger: trigger.id,
            trigger_ref: trigger.r#ref.clone(),
            conditions: json!([]),
            action_params: json!({}),
            trigger_params: json!({}),
            trace_tag_template: None,
            permission_set_refs: None,
            enabled: true,
            is_adhoc: false,
            owner_identity: None,
        };

        RuleRepository::create(&pool, input).await.unwrap();
    }

    let rules = RuleRepository::list(&pool).await.unwrap();
    let our_rules: Vec<_> = rules
        .iter()
        .filter(|r| r.r#ref.starts_with(&pack.r#ref))
        .collect();

    // Check they are ordered alphabetically
    assert!(our_rules[0].r#ref.contains("alice"));
    assert!(our_rules[1].r#ref.contains("bob"));
    assert!(our_rules[2].r#ref.contains("charlie"));
}

// ============================================================================
// UPDATE Tests
// ============================================================================

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_update_rule_label() {
    let pool = create_test_pool().await.unwrap();

    let pack = PackFixture::new_unique("update_pack")
        .create(&pool)
        .await
        .unwrap();

    let action = ActionFixture::new_unique(pack.id, &pack.r#ref, "action")
        .create(&pool)
        .await
        .unwrap();

    let trigger = TriggerFixture::new_unique(Some(pack.id), Some(pack.r#ref.clone()), "trigger")
        .create(&pool)
        .await
        .unwrap();

    let input = CreateRuleInput {
        r#ref: format!("{}.update_rule", pack.r#ref),
        pack: pack.id,
        pack_ref: pack.r#ref.clone(),
        label: "Original Label".to_string(),
        description: Some("Original".to_string()),
        action: action.id,
        action_ref: action.r#ref.clone(),
        trigger: trigger.id,
        trigger_ref: trigger.r#ref.clone(),
        conditions: json!([]),
        action_params: json!({}),
        trigger_params: json!({}),
        trace_tag_template: None,
        permission_set_refs: None,
        enabled: true,
        is_adhoc: false,
        owner_identity: None,
    };

    let created = RuleRepository::create(&pool, input).await.unwrap();

    let update = UpdateRuleInput {
        label: Some("Updated Label".to_string()),
        ..Default::default()
    };

    let updated = RuleRepository::update(&pool, created.id, update)
        .await
        .unwrap();

    assert_eq!(updated.label, "Updated Label");
    assert_eq!(updated.description, Some("Original".to_string())); // unchanged
    assert!(updated.updated > created.updated);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_update_rule_description() {
    let pool = create_test_pool().await.unwrap();

    let pack = PackFixture::new_unique("desc_pack")
        .create(&pool)
        .await
        .unwrap();

    let action = ActionFixture::new_unique(pack.id, &pack.r#ref, "action")
        .create(&pool)
        .await
        .unwrap();

    let trigger = TriggerFixture::new_unique(Some(pack.id), Some(pack.r#ref.clone()), "trigger")
        .create(&pool)
        .await
        .unwrap();

    let input = CreateRuleInput {
        r#ref: format!("{}.desc_rule", pack.r#ref),
        pack: pack.id,
        pack_ref: pack.r#ref.clone(),
        label: "Test".to_string(),
        description: Some("Old description".to_string()),
        action: action.id,
        action_ref: action.r#ref.clone(),
        trigger: trigger.id,
        trigger_ref: trigger.r#ref.clone(),
        conditions: json!([]),
        action_params: json!({}),
        trigger_params: json!({}),
        trace_tag_template: None,
        permission_set_refs: None,
        enabled: true,
        is_adhoc: false,
        owner_identity: None,
    };

    let created = RuleRepository::create(&pool, input).await.unwrap();

    let update = UpdateRuleInput {
        description: Some(Patch::Set("New description".to_string())),
        ..Default::default()
    };

    let updated = RuleRepository::update(&pool, created.id, update)
        .await
        .unwrap();

    assert_eq!(updated.description, Some("New description".to_string()));
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_update_rule_conditions() {
    let pool = create_test_pool().await.unwrap();

    let pack = PackFixture::new_unique("cond_pack")
        .create(&pool)
        .await
        .unwrap();

    let action = ActionFixture::new_unique(pack.id, &pack.r#ref, "action")
        .create(&pool)
        .await
        .unwrap();

    let trigger = TriggerFixture::new_unique(Some(pack.id), Some(pack.r#ref.clone()), "trigger")
        .create(&pool)
        .await
        .unwrap();

    let input = CreateRuleInput {
        r#ref: format!("{}.cond_rule", pack.r#ref),
        pack: pack.id,
        pack_ref: pack.r#ref.clone(),
        label: "Test".to_string(),
        description: Some("Test".to_string()),
        action: action.id,
        action_ref: action.r#ref.clone(),
        trigger: trigger.id,
        trigger_ref: trigger.r#ref.clone(),
        conditions: json!({"old": "condition"}),
        action_params: json!({}),
        trigger_params: json!({}),
        trace_tag_template: None,
        permission_set_refs: None,
        enabled: true,
        is_adhoc: false,
        owner_identity: None,
    };

    let created = RuleRepository::create(&pool, input).await.unwrap();

    let new_conditions = json!({"new": "condition", "count": 42});
    let update = UpdateRuleInput {
        conditions: Some(new_conditions.clone()),
        ..Default::default()
    };

    let updated = RuleRepository::update(&pool, created.id, update)
        .await
        .unwrap();

    assert_eq!(updated.conditions, new_conditions);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_update_rule_enabled() {
    let pool = create_test_pool().await.unwrap();

    let pack = PackFixture::new_unique("enabled_pack")
        .create(&pool)
        .await
        .unwrap();

    let action = ActionFixture::new_unique(pack.id, &pack.r#ref, "action")
        .create(&pool)
        .await
        .unwrap();

    let trigger = TriggerFixture::new_unique(Some(pack.id), Some(pack.r#ref.clone()), "trigger")
        .create(&pool)
        .await
        .unwrap();

    let input = CreateRuleInput {
        r#ref: format!("{}.enabled_rule", pack.r#ref),
        pack: pack.id,
        pack_ref: pack.r#ref.clone(),
        label: "Test".to_string(),
        description: Some("Test".to_string()),
        action: action.id,
        action_ref: action.r#ref.clone(),
        trigger: trigger.id,
        trigger_ref: trigger.r#ref.clone(),
        conditions: json!([]),
        action_params: json!({}),
        trigger_params: json!({}),
        trace_tag_template: None,
        permission_set_refs: None,
        enabled: true,
        is_adhoc: false,
        owner_identity: None,
    };

    let created = RuleRepository::create(&pool, input).await.unwrap();

    let update = UpdateRuleInput {
        enabled: Some(false),
        action_params: None,
        trigger_params: None,
        ..Default::default()
    };

    let updated = RuleRepository::update(&pool, created.id, update)
        .await
        .unwrap();

    assert!(!updated.enabled);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_update_rule_multiple_fields() {
    let pool = create_test_pool().await.unwrap();

    let pack = PackFixture::new_unique("multi_pack")
        .create(&pool)
        .await
        .unwrap();

    let action = ActionFixture::new_unique(pack.id, &pack.r#ref, "action")
        .create(&pool)
        .await
        .unwrap();

    let trigger = TriggerFixture::new_unique(Some(pack.id), Some(pack.r#ref.clone()), "trigger")
        .create(&pool)
        .await
        .unwrap();

    let input = CreateRuleInput {
        r#ref: format!("{}.multi_rule", pack.r#ref),
        pack: pack.id,
        pack_ref: pack.r#ref.clone(),
        label: "Old".to_string(),
        description: Some("Old".to_string()),
        action: action.id,
        action_ref: action.r#ref.clone(),
        trigger: trigger.id,
        trigger_ref: trigger.r#ref.clone(),
        conditions: json!([]),
        action_params: json!({}),
        trigger_params: json!({}),
        trace_tag_template: None,
        permission_set_refs: None,
        enabled: true,
        is_adhoc: false,
        owner_identity: None,
    };

    let created = RuleRepository::create(&pool, input).await.unwrap();

    let update = UpdateRuleInput {
        label: Some("New Label".to_string()),
        description: Some(Patch::Set("New Description".to_string())),
        action: None,
        action_ref: None,
        trigger: None,
        trigger_ref: None,
        conditions: Some(json!({"updated": true})),
        action_params: None,
        trigger_params: None,
        permission_set_refs: None,
        enabled: Some(false),
        ..Default::default()
    };

    let updated = RuleRepository::update(&pool, created.id, update)
        .await
        .unwrap();

    assert_eq!(updated.label, "New Label");
    assert_eq!(updated.description, Some("New Description".to_string()));
    assert_eq!(updated.conditions, json!({"updated": true}));
    assert!(!updated.enabled);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_update_rule_action_and_trigger_refs() {
    let pool = create_test_pool().await.unwrap();

    let pack = PackFixture::new_unique("retarget_pack")
        .create(&pool)
        .await
        .unwrap();

    let original_action = ActionFixture::new_unique(pack.id, &pack.r#ref, "action_one")
        .create(&pool)
        .await
        .unwrap();
    let replacement_action = ActionFixture::new_unique(pack.id, &pack.r#ref, "action_two")
        .create(&pool)
        .await
        .unwrap();

    let original_trigger =
        TriggerFixture::new_unique(Some(pack.id), Some(pack.r#ref.clone()), "trigger_one")
            .create(&pool)
            .await
            .unwrap();
    let replacement_trigger =
        TriggerFixture::new_unique(Some(pack.id), Some(pack.r#ref.clone()), "trigger_two")
            .create(&pool)
            .await
            .unwrap();

    let input = CreateRuleInput {
        r#ref: format!("{}.retarget_rule", pack.r#ref),
        pack: pack.id,
        pack_ref: pack.r#ref.clone(),
        label: "Retarget Me".to_string(),
        description: None,
        action: original_action.id,
        action_ref: original_action.r#ref.clone(),
        trigger: original_trigger.id,
        trigger_ref: original_trigger.r#ref.clone(),
        conditions: json!([]),
        action_params: json!({}),
        trigger_params: json!({}),
        trace_tag_template: None,
        permission_set_refs: None,
        enabled: true,
        is_adhoc: false,
        owner_identity: None,
    };

    let created = RuleRepository::create(&pool, input).await.unwrap();

    let update = UpdateRuleInput {
        action: Some(replacement_action.id),
        action_ref: Some(replacement_action.r#ref.clone()),
        trigger: Some(replacement_trigger.id),
        trigger_ref: Some(replacement_trigger.r#ref.clone()),
        ..Default::default()
    };

    let updated = RuleRepository::update(&pool, created.id, update)
        .await
        .unwrap();

    assert_eq!(updated.action, Some(replacement_action.id));
    assert_eq!(updated.action_ref, replacement_action.r#ref);
    assert_eq!(updated.trigger, Some(replacement_trigger.id));
    assert_eq!(updated.trigger_ref, replacement_trigger.r#ref);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_update_rule_no_changes() {
    let pool = create_test_pool().await.unwrap();

    let pack = PackFixture::new_unique("nochange_pack")
        .create(&pool)
        .await
        .unwrap();

    let action = ActionFixture::new_unique(pack.id, &pack.r#ref, "action")
        .create(&pool)
        .await
        .unwrap();

    let trigger = TriggerFixture::new_unique(Some(pack.id), Some(pack.r#ref.clone()), "trigger")
        .create(&pool)
        .await
        .unwrap();

    let input = CreateRuleInput {
        r#ref: format!("{}.nochange_rule", pack.r#ref),
        pack: pack.id,
        pack_ref: pack.r#ref.clone(),
        label: "Test".to_string(),
        description: Some("Test".to_string()),
        action: action.id,
        action_ref: action.r#ref.clone(),
        trigger: trigger.id,
        trigger_ref: trigger.r#ref.clone(),
        conditions: json!([]),
        action_params: json!({}),
        trigger_params: json!({}),
        trace_tag_template: None,
        permission_set_refs: None,
        enabled: true,
        is_adhoc: false,
        owner_identity: None,
    };

    let created = RuleRepository::create(&pool, input).await.unwrap();

    let update = UpdateRuleInput::default();

    let updated = RuleRepository::update(&pool, created.id, update)
        .await
        .unwrap();

    assert_eq!(updated.label, created.label);
    assert_eq!(updated.description, created.description);
}

// ============================================================================
// DELETE Tests
// ============================================================================

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_delete_rule() {
    let pool = create_test_pool().await.unwrap();

    let pack = PackFixture::new_unique("delete_pack")
        .create(&pool)
        .await
        .unwrap();

    let action = ActionFixture::new_unique(pack.id, &pack.r#ref, "action")
        .create(&pool)
        .await
        .unwrap();

    let trigger = TriggerFixture::new_unique(Some(pack.id), Some(pack.r#ref.clone()), "trigger")
        .create(&pool)
        .await
        .unwrap();

    let input = CreateRuleInput {
        r#ref: format!("{}.delete_rule", pack.r#ref),
        pack: pack.id,
        pack_ref: pack.r#ref.clone(),
        label: "To Delete".to_string(),
        description: Some("Will be deleted".to_string()),
        action: action.id,
        action_ref: action.r#ref.clone(),
        trigger: trigger.id,
        trigger_ref: trigger.r#ref.clone(),
        conditions: json!([]),
        action_params: json!({}),
        trigger_params: json!({}),
        trace_tag_template: None,
        permission_set_refs: None,
        enabled: true,
        is_adhoc: false,
        owner_identity: None,
    };

    let created = RuleRepository::create(&pool, input).await.unwrap();

    let deleted = RuleRepository::delete(&pool, created.id).await.unwrap();

    assert!(deleted);

    let found = RuleRepository::find_by_id(&pool, created.id).await.unwrap();

    assert!(found.is_none());
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_delete_rule_not_found() {
    let pool = create_test_pool().await.unwrap();

    let deleted = RuleRepository::delete(&pool, 999999).await.unwrap();

    assert!(!deleted);
}

// ============================================================================
// SPECIALIZED QUERY Tests
// ============================================================================

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_find_rules_by_pack() {
    let pool = create_test_pool().await.unwrap();

    let pack1 = PackFixture::new_unique("pack1")
        .create(&pool)
        .await
        .unwrap();

    let pack2 = PackFixture::new_unique("pack2")
        .create(&pool)
        .await
        .unwrap();

    let action1 = ActionFixture::new_unique(pack1.id, &pack1.r#ref, "action1")
        .create(&pool)
        .await
        .unwrap();

    let action2 = ActionFixture::new_unique(pack2.id, &pack2.r#ref, "action2")
        .create(&pool)
        .await
        .unwrap();

    let trigger1 =
        TriggerFixture::new_unique(Some(pack1.id), Some(pack1.r#ref.clone()), "trigger1")
            .create(&pool)
            .await
            .unwrap();

    let trigger2 =
        TriggerFixture::new_unique(Some(pack2.id), Some(pack2.r#ref.clone()), "trigger2")
            .create(&pool)
            .await
            .unwrap();

    // Create 2 rules for pack1
    for i in 1..=2 {
        let input = CreateRuleInput {
            r#ref: format!("{}.rule{}", pack1.r#ref, i),
            pack: pack1.id,
            pack_ref: pack1.r#ref.clone(),
            label: format!("Rule {}", i),
            description: Some(format!("Rule {}", i)),
            action: action1.id,
            action_ref: action1.r#ref.clone(),
            trigger: trigger1.id,
            trigger_ref: trigger1.r#ref.clone(),
            conditions: json!([]),
            action_params: json!({}),
            trigger_params: json!({}),
            trace_tag_template: None,
            permission_set_refs: None,
            enabled: true,
            is_adhoc: false,
            owner_identity: None,
        };

        RuleRepository::create(&pool, input).await.unwrap();
    }

    // Create 1 rule for pack2
    let input = CreateRuleInput {
        r#ref: format!("{}.rule1", pack2.r#ref),
        pack: pack2.id,
        pack_ref: pack2.r#ref.clone(),
        label: "Pack2 Rule".to_string(),
        description: Some("Pack2".to_string()),
        action: action2.id,
        action_ref: action2.r#ref.clone(),
        trigger: trigger2.id,
        trigger_ref: trigger2.r#ref.clone(),
        conditions: json!([]),
        action_params: json!({}),
        trigger_params: json!({}),
        trace_tag_template: None,
        permission_set_refs: None,
        enabled: true,
        is_adhoc: false,
        owner_identity: None,
    };

    RuleRepository::create(&pool, input).await.unwrap();

    let pack1_rules = RuleRepository::find_by_pack(&pool, pack1.id).await.unwrap();

    assert_eq!(pack1_rules.len(), 2);
    assert!(pack1_rules.iter().all(|r| r.pack == pack1.id));

    let pack2_rules = RuleRepository::find_by_pack(&pool, pack2.id).await.unwrap();

    assert_eq!(pack2_rules.len(), 1);
    assert_eq!(pack2_rules[0].pack, pack2.id);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_find_rules_by_action() {
    let pool = create_test_pool().await.unwrap();

    let pack = PackFixture::new_unique("action_pack")
        .create(&pool)
        .await
        .unwrap();

    let action1 = ActionFixture::new_unique(pack.id, &pack.r#ref, "action1")
        .create(&pool)
        .await
        .unwrap();

    let action2 = ActionFixture::new_unique(pack.id, &pack.r#ref, "action2")
        .create(&pool)
        .await
        .unwrap();

    let trigger = TriggerFixture::new_unique(Some(pack.id), Some(pack.r#ref.clone()), "trigger")
        .create(&pool)
        .await
        .unwrap();

    // Create 2 rules for action1
    for i in 1..=2 {
        let input = CreateRuleInput {
            r#ref: format!("{}.rule_a1_{}", pack.r#ref, i),
            pack: pack.id,
            pack_ref: pack.r#ref.clone(),
            label: format!("Action1 Rule {}", i),
            description: Some("Test".to_string()),
            action: action1.id,
            action_ref: action1.r#ref.clone(),
            trigger: trigger.id,
            trigger_ref: trigger.r#ref.clone(),
            conditions: json!([]),
            action_params: json!({}),
            trigger_params: json!({}),
            trace_tag_template: None,
            permission_set_refs: None,
            enabled: true,
            is_adhoc: false,
            owner_identity: None,
        };

        RuleRepository::create(&pool, input).await.unwrap();
    }

    // Create 1 rule for action2
    let input = CreateRuleInput {
        r#ref: format!("{}.rule_a2_1", pack.r#ref),
        pack: pack.id,
        pack_ref: pack.r#ref.clone(),
        label: "Action2 Rule".to_string(),
        description: Some("Test".to_string()),
        action: action2.id,
        action_ref: action2.r#ref.clone(),
        trigger: trigger.id,
        trigger_ref: trigger.r#ref.clone(),
        conditions: json!([]),
        action_params: json!({}),
        trigger_params: json!({}),
        trace_tag_template: None,
        permission_set_refs: None,
        enabled: true,
        is_adhoc: false,
        owner_identity: None,
    };

    RuleRepository::create(&pool, input).await.unwrap();

    let action1_rules = RuleRepository::find_by_action(&pool, action1.id)
        .await
        .unwrap();

    assert_eq!(action1_rules.len(), 2);
    assert!(action1_rules.iter().all(|r| r.action == Some(action1.id)));

    let action2_rules = RuleRepository::find_by_action(&pool, action2.id)
        .await
        .unwrap();

    assert_eq!(action2_rules.len(), 1);
    assert_eq!(action2_rules[0].action, Some(action2.id));
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_find_rules_by_trigger() {
    let pool = create_test_pool().await.unwrap();

    let pack = PackFixture::new_unique("trigger_pack")
        .create(&pool)
        .await
        .unwrap();

    let action = ActionFixture::new_unique(pack.id, &pack.r#ref, "action")
        .create(&pool)
        .await
        .unwrap();

    let trigger1 = TriggerFixture::new_unique(Some(pack.id), Some(pack.r#ref.clone()), "trigger1")
        .create(&pool)
        .await
        .unwrap();

    let trigger2 = TriggerFixture::new_unique(Some(pack.id), Some(pack.r#ref.clone()), "trigger2")
        .create(&pool)
        .await
        .unwrap();

    // Create 2 rules for trigger1
    for i in 1..=2 {
        let input = CreateRuleInput {
            r#ref: format!("{}.rule_t1_{}", pack.r#ref, i),
            pack: pack.id,
            pack_ref: pack.r#ref.clone(),
            label: format!("Trigger1 Rule {}", i),
            description: Some("Test".to_string()),
            action: action.id,
            action_ref: action.r#ref.clone(),
            trigger: trigger1.id,
            trigger_ref: trigger1.r#ref.clone(),
            conditions: json!([]),
            action_params: json!({}),
            trigger_params: json!({}),
            trace_tag_template: None,
            permission_set_refs: None,
            enabled: true,
            is_adhoc: false,
            owner_identity: None,
        };

        RuleRepository::create(&pool, input).await.unwrap();
    }

    // Create 1 rule for trigger2
    let input = CreateRuleInput {
        r#ref: format!("{}.rule_t2_1", pack.r#ref),
        pack: pack.id,
        pack_ref: pack.r#ref.clone(),
        label: "Trigger2 Rule".to_string(),
        description: Some("Test".to_string()),
        action: action.id,
        action_ref: action.r#ref.clone(),
        trigger: trigger2.id,
        trigger_ref: trigger2.r#ref.clone(),
        conditions: json!([]),
        action_params: json!({}),
        trigger_params: json!({}),
        trace_tag_template: None,
        permission_set_refs: None,
        enabled: true,
        is_adhoc: false,
        owner_identity: None,
    };

    RuleRepository::create(&pool, input).await.unwrap();

    let trigger1_rules = RuleRepository::find_by_trigger(&pool, trigger1.id)
        .await
        .unwrap();

    assert_eq!(trigger1_rules.len(), 2);
    assert!(trigger1_rules
        .iter()
        .all(|r| r.trigger == Some(trigger1.id)));

    let trigger2_rules = RuleRepository::find_by_trigger(&pool, trigger2.id)
        .await
        .unwrap();

    assert_eq!(trigger2_rules.len(), 1);
    assert_eq!(trigger2_rules[0].trigger, Some(trigger2.id));
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_find_enabled_rules() {
    let pool = create_test_pool().await.unwrap();

    let pack = PackFixture::new_unique("enabled_filter_pack")
        .create(&pool)
        .await
        .unwrap();

    let action = ActionFixture::new_unique(pack.id, &pack.r#ref, "action")
        .create(&pool)
        .await
        .unwrap();

    let trigger = TriggerFixture::new_unique(Some(pack.id), Some(pack.r#ref.clone()), "trigger")
        .create(&pool)
        .await
        .unwrap();

    // Create enabled rules
    for i in 1..=2 {
        let input = CreateRuleInput {
            r#ref: format!("{}.enabled_{}", pack.r#ref, i),
            pack: pack.id,
            pack_ref: pack.r#ref.clone(),
            label: format!("Enabled {}", i),
            description: Some("Test".to_string()),
            action: action.id,
            action_ref: action.r#ref.clone(),
            trigger: trigger.id,
            trigger_ref: trigger.r#ref.clone(),
            conditions: json!([]),
            action_params: json!({}),
            trigger_params: json!({}),
            trace_tag_template: None,
            permission_set_refs: None,
            enabled: true,
            is_adhoc: false,
            owner_identity: None,
        };

        RuleRepository::create(&pool, input).await.unwrap();
    }

    // Create disabled rules
    for i in 1..=2 {
        let input = CreateRuleInput {
            r#ref: format!("{}.disabled_{}", pack.r#ref, i),
            pack: pack.id,
            pack_ref: pack.r#ref.clone(),
            label: format!("Disabled {}", i),
            description: Some("Test".to_string()),
            action: action.id,
            action_ref: action.r#ref.clone(),
            trigger: trigger.id,
            trigger_ref: trigger.r#ref.clone(),
            conditions: json!([]),
            action_params: json!({}),
            trigger_params: json!({}),
            trace_tag_template: None,
            permission_set_refs: None,
            enabled: false,
            is_adhoc: false,
            owner_identity: None,
        };

        RuleRepository::create(&pool, input).await.unwrap();
    }

    let enabled_rules = RuleRepository::find_enabled(&pool).await.unwrap();

    // Filter to only our pack's rules
    let our_enabled: Vec<_> = enabled_rules
        .iter()
        .filter(|r| r.r#ref.starts_with(&pack.r#ref))
        .collect();

    assert_eq!(our_enabled.len(), 2);
    assert!(our_enabled.iter().all(|r| r.enabled));
}

// ============================================================================
// FOREIGN KEY CONSTRAINT Tests
// ============================================================================

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_cascade_delete_pack_deletes_rules() {
    let pool = create_test_pool().await.unwrap();

    let pack = PackFixture::new_unique("cascade_pack")
        .create(&pool)
        .await
        .unwrap();

    let action = ActionFixture::new_unique(pack.id, &pack.r#ref, "action")
        .create(&pool)
        .await
        .unwrap();

    let trigger = TriggerFixture::new_unique(Some(pack.id), Some(pack.r#ref.clone()), "trigger")
        .create(&pool)
        .await
        .unwrap();

    let input = CreateRuleInput {
        r#ref: format!("{}.cascade_rule", pack.r#ref),
        pack: pack.id,
        pack_ref: pack.r#ref.clone(),
        label: "Cascade Rule".to_string(),
        description: Some("Will be cascade deleted".to_string()),
        action: action.id,
        action_ref: action.r#ref.clone(),
        trigger: trigger.id,
        trigger_ref: trigger.r#ref.clone(),
        conditions: json!([]),
        action_params: json!({}),
        trigger_params: json!({}),
        trace_tag_template: None,
        permission_set_refs: None,
        enabled: true,
        is_adhoc: false,
        owner_identity: None,
    };

    let rule = RuleRepository::create(&pool, input).await.unwrap();

    // Delete the pack
    sqlx::query("DELETE FROM pack WHERE id = $1")
        .bind(pack.id)
        .execute(&pool)
        .await
        .unwrap();

    // Rule should be cascade deleted
    let found = RuleRepository::find_by_id(&pool, rule.id).await.unwrap();

    assert!(found.is_none());
}

// ============================================================================
// TIMESTAMP Tests
// ============================================================================

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_rule_timestamps() {
    let pool = create_test_pool().await.unwrap();

    let pack = PackFixture::new_unique("timestamp_pack")
        .create(&pool)
        .await
        .unwrap();

    let action = ActionFixture::new_unique(pack.id, &pack.r#ref, "action")
        .create(&pool)
        .await
        .unwrap();

    let trigger = TriggerFixture::new_unique(Some(pack.id), Some(pack.r#ref.clone()), "trigger")
        .create(&pool)
        .await
        .unwrap();

    let input = CreateRuleInput {
        r#ref: format!("{}.ts_rule", pack.r#ref),
        pack: pack.id,
        pack_ref: pack.r#ref.clone(),
        label: "Timestamp Rule".to_string(),
        description: Some("Test timestamps".to_string()),
        action: action.id,
        action_ref: action.r#ref.clone(),
        trigger: trigger.id,
        trigger_ref: trigger.r#ref.clone(),
        conditions: json!([]),
        action_params: json!({}),
        trigger_params: json!({}),
        trace_tag_template: None,
        permission_set_refs: None,
        enabled: true,
        is_adhoc: false,
        owner_identity: None,
    };

    let created = RuleRepository::create(&pool, input).await.unwrap();

    assert!(created.created.timestamp() > 0);
    assert!(created.updated.timestamp() > 0);
    assert_eq!(created.created, created.updated);

    let original_updated = created.updated - chrono::Duration::seconds(1);
    set_updated_for_test(&pool, "rule", created.id, original_updated).await;

    let update = UpdateRuleInput {
        label: Some("Updated".to_string()),
        ..Default::default()
    };

    let updated = RuleRepository::update(&pool, created.id, update)
        .await
        .unwrap();

    assert_eq!(updated.created, created.created); // created unchanged
    assert!(updated.updated > original_updated); // updated changed
}

// ============================================================================
// owner_identity Tests
// ============================================================================

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_create_rule_with_owner_identity() {
    let pool = create_test_pool().await.unwrap();

    let identity = IdentityFixture::new_unique("rule_owner")
        .create(&pool)
        .await
        .unwrap();

    let pack = PackFixture::new_unique("test_pack")
        .create(&pool)
        .await
        .unwrap();
    let action = ActionFixture::new_unique(pack.id, &pack.r#ref, "test_action")
        .create(&pool)
        .await
        .unwrap();
    let trigger =
        TriggerFixture::new_unique(Some(pack.id), Some(pack.r#ref.clone()), "test_trigger")
            .create(&pool)
            .await
            .unwrap();

    let rule_ref = format!("{}.test_rule_owner", pack.r#ref);
    let input = CreateRuleInput {
        r#ref: rule_ref.clone(),
        pack: pack.id,
        pack_ref: pack.r#ref.clone(),
        label: "Owned Rule".to_string(),
        description: None,
        action: action.id,
        action_ref: action.r#ref.clone(),
        trigger: trigger.id,
        trigger_ref: trigger.r#ref.clone(),
        conditions: json!({}),
        action_params: json!({}),
        trigger_params: json!({}),
        trace_tag_template: None,
        permission_set_refs: None,
        enabled: true,
        is_adhoc: true,
        owner_identity: Some(identity.id),
    };

    let created = RuleRepository::create(&pool, input).await.unwrap();
    assert_eq!(created.owner_identity, Some(identity.id));

    let fetched = RuleRepository::find_by_id(&pool, created.id)
        .await
        .unwrap()
        .expect("rule should exist");
    assert_eq!(fetched.owner_identity, Some(identity.id));
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_create_rule_without_owner_identity_defaults_null() {
    let pool = create_test_pool().await.unwrap();

    let pack = PackFixture::new_unique("test_pack")
        .create(&pool)
        .await
        .unwrap();
    let action = ActionFixture::new_unique(pack.id, &pack.r#ref, "test_action")
        .create(&pool)
        .await
        .unwrap();
    let trigger =
        TriggerFixture::new_unique(Some(pack.id), Some(pack.r#ref.clone()), "test_trigger")
            .create(&pool)
            .await
            .unwrap();

    let rule_ref = format!("{}.test_rule_no_owner", pack.r#ref);
    let input = CreateRuleInput {
        r#ref: rule_ref.clone(),
        pack: pack.id,
        pack_ref: pack.r#ref.clone(),
        label: "Unowned Rule".to_string(),
        description: None,
        action: action.id,
        action_ref: action.r#ref.clone(),
        trigger: trigger.id,
        trigger_ref: trigger.r#ref.clone(),
        conditions: json!({}),
        action_params: json!({}),
        trigger_params: json!({}),
        trace_tag_template: None,
        permission_set_refs: None,
        enabled: true,
        is_adhoc: false,
        owner_identity: None,
    };

    let created = RuleRepository::create(&pool, input).await.unwrap();
    assert_eq!(created.owner_identity, None);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_update_rule_owner_identity_set_and_clear() {
    let pool = create_test_pool().await.unwrap();

    let identity = IdentityFixture::new_unique("rule_owner_upd")
        .create(&pool)
        .await
        .unwrap();

    let pack = PackFixture::new_unique("test_pack")
        .create(&pool)
        .await
        .unwrap();
    let action = ActionFixture::new_unique(pack.id, &pack.r#ref, "test_action")
        .create(&pool)
        .await
        .unwrap();
    let trigger =
        TriggerFixture::new_unique(Some(pack.id), Some(pack.r#ref.clone()), "test_trigger")
            .create(&pool)
            .await
            .unwrap();

    let rule_ref = format!("{}.test_rule_upd_owner", pack.r#ref);
    let input = CreateRuleInput {
        r#ref: rule_ref.clone(),
        pack: pack.id,
        pack_ref: pack.r#ref.clone(),
        label: "Rule".to_string(),
        description: None,
        action: action.id,
        action_ref: action.r#ref.clone(),
        trigger: trigger.id,
        trigger_ref: trigger.r#ref.clone(),
        conditions: json!({}),
        action_params: json!({}),
        trigger_params: json!({}),
        trace_tag_template: None,
        permission_set_refs: None,
        enabled: true,
        is_adhoc: true,
        owner_identity: None,
    };
    let created = RuleRepository::create(&pool, input).await.unwrap();
    assert_eq!(created.owner_identity, None);

    // Set owner_identity via update
    let update = UpdateRuleInput {
        owner_identity: Some(Patch::Set(identity.id)),
        ..Default::default()
    };
    let updated = RuleRepository::update(&pool, created.id, update)
        .await
        .unwrap();
    assert_eq!(updated.owner_identity, Some(identity.id));

    // Clear owner_identity via update
    let update = UpdateRuleInput {
        owner_identity: Some(Patch::Clear),
        ..Default::default()
    };
    let cleared = RuleRepository::update(&pool, created.id, update)
        .await
        .unwrap();
    assert_eq!(cleared.owner_identity, None);
}
