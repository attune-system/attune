//! Integration tests for Sensor repository
//!
//! These tests verify CRUD operations, queries, and constraints
//! for the Sensor repository.

mod helpers;

use attune_common::{
    repositories::{
        trigger::{CreateSensorInput, SensorRepository, UpdateSensorInput},
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
async fn test_create_sensor_minimal() {
    let pool = create_test_pool().await.unwrap();

    // Create dependencies
    let pack = PackFixture::new_unique("sensor_pack")
        .create(&pool)
        .await
        .unwrap();

    let _trigger = TriggerFixture::new_unique(Some(pack.id), Some(pack.r#ref.clone()), "webhook")
        .create(&pool)
        .await
        .unwrap();

    let runtime = RuntimeFixture::new_unique(Some(pack.id), Some(pack.r#ref.clone()), "python3")
        .create(&pool)
        .await
        .unwrap();

    // Create sensor
    let sensor = SensorFixture::new_unique(
        Some(pack.id),
        Some(pack.r#ref.clone()),
        runtime.id,
        runtime.r#ref.clone(),
        "webhook_sensor",
    )
    .create(&pool)
    .await
    .unwrap();

    assert!(sensor.id > 0);
    assert!(sensor.r#ref.contains(&pack.r#ref));
    assert_eq!(sensor.pack, Some(pack.id));
    assert_eq!(sensor.pack_ref, Some(pack.r#ref));
    assert_eq!(sensor.runtime, runtime.id);
    assert_eq!(sensor.runtime_ref, runtime.r#ref);
    assert!(sensor.enabled);
    assert_eq!(sensor.param_schema, None);
    assert!(sensor.created.timestamp() > 0);
    assert!(sensor.updated.timestamp() > 0);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_create_sensor_with_param_schema() {
    let pool = create_test_pool().await.unwrap();

    let pack = PackFixture::new_unique("schema_pack")
        .create(&pool)
        .await
        .unwrap();

    let _trigger = TriggerFixture::new_unique(Some(pack.id), Some(pack.r#ref.clone()), "event")
        .create(&pool)
        .await
        .unwrap();

    let runtime = RuntimeFixture::new_unique(Some(pack.id), Some(pack.r#ref.clone()), "python3")
        .create(&pool)
        .await
        .unwrap();

    let param_schema = json!({
        "type": "object",
        "properties": {
            "interval": {
                "type": "integer",
                "minimum": 1
            },
            "endpoint": {
                "type": "string",
                "format": "uri"
            }
        },
        "required": ["interval"]
    });

    let sensor = SensorFixture::new_unique(
        Some(pack.id),
        Some(pack.r#ref.clone()),
        runtime.id,
        runtime.r#ref.clone(),
        "polling_sensor",
    )
    .with_param_schema(param_schema.clone())
    .create(&pool)
    .await
    .unwrap();

    assert_eq!(sensor.param_schema, Some(param_schema));
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_create_sensor_without_pack() {
    let pool = create_test_pool().await.unwrap();

    let _trigger = TriggerFixture::new_unique(None, None, "webhook")
        .create(&pool)
        .await
        .unwrap();

    let runtime = RuntimeFixture::new_unique(None, None, "python3")
        .create(&pool)
        .await
        .unwrap();

    let sensor = SensorFixture::new_unique(
        None,
        None,
        runtime.id,
        runtime.r#ref.clone(),
        "system_sensor",
    )
    .create(&pool)
    .await
    .unwrap();

    assert_eq!(sensor.pack, None);
    assert_eq!(sensor.pack_ref, None);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_create_sensor_duplicate_ref_fails() {
    let pool = create_test_pool().await.unwrap();

    let pack = PackFixture::new_unique("dup_pack")
        .create(&pool)
        .await
        .unwrap();

    let _trigger = TriggerFixture::new_unique(Some(pack.id), Some(pack.r#ref.clone()), "event")
        .create(&pool)
        .await
        .unwrap();

    let runtime = RuntimeFixture::new_unique(Some(pack.id), Some(pack.r#ref.clone()), "python3")
        .create(&pool)
        .await
        .unwrap();

    // Create first sensor
    let sensor_ref = format!("{}.duplicate_sensor", pack.r#ref);
    let input = CreateSensorInput {
        r#ref: sensor_ref.clone(),
        pack: Some(pack.id),
        pack_ref: Some(pack.r#ref.clone()),
        label: "Duplicate Sensor".to_string(),
        description: Some("Test sensor".to_string()),
        entrypoint: "sensors/dup.py".to_string(),
        runtime: runtime.id,
        runtime_ref: runtime.r#ref.clone(),
        runtime_version_constraint: None,
        enabled: true,
        param_schema: None,
        config: None,
        worker_selector: json!({}),
        worker_tolerations: json!([]),
        worker_affinity: json!({}),
        artifact_retention_policy: None,
        artifact_retention_limit: None,
        log_retention_policy: None,
        log_retention_limit: None,
    };

    SensorRepository::create(&pool, input.clone())
        .await
        .unwrap();

    // Try to create second sensor with same ref
    let result = SensorRepository::create(&pool, input).await;
    assert!(result.is_err());
    // Should fail with database error due to unique constraint violation
    assert!(result.is_err());
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_create_sensor_invalid_ref_format_fails() {
    let pool = create_test_pool().await.unwrap();

    let pack = PackFixture::new_unique("invalid_pack")
        .create(&pool)
        .await
        .unwrap();

    let _trigger = TriggerFixture::new_unique(Some(pack.id), Some(pack.r#ref.clone()), "event")
        .create(&pool)
        .await
        .unwrap();

    let runtime = RuntimeFixture::new_unique(Some(pack.id), Some(pack.r#ref.clone()), "python3")
        .create(&pool)
        .await
        .unwrap();

    // Try invalid ref formats
    let invalid_refs = vec![
        "no_dot",             // Missing dot
        "too.many.dots.here", // Too many dots
        "UPPERCASE.sensor",   // Uppercase not allowed
    ];

    for invalid_ref in invalid_refs {
        let input = CreateSensorInput {
            r#ref: invalid_ref.to_string(),
            pack: Some(pack.id),
            pack_ref: Some(pack.r#ref.clone()),
            label: "Invalid Sensor".to_string(),
            description: Some("Test sensor".to_string()),
            entrypoint: "sensors/invalid.py".to_string(),
            runtime: runtime.id,
            runtime_ref: runtime.r#ref.clone(),
            runtime_version_constraint: None,
            enabled: true,
            param_schema: None,
            config: None,
            worker_selector: json!({}),
            worker_tolerations: json!([]),
            worker_affinity: json!({}),
            artifact_retention_policy: None,
            artifact_retention_limit: None,
            log_retention_policy: None,
            log_retention_limit: None,
        };

        let result = SensorRepository::create(&pool, input).await;
        assert!(
            result.is_err(),
            "Expected error for invalid ref: {}",
            invalid_ref
        );
    }
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_create_sensor_invalid_pack_fails() {
    let pool = create_test_pool().await.unwrap();

    let _trigger = TriggerFixture::new_unique(None, None, "event")
        .create(&pool)
        .await
        .unwrap();

    let runtime = RuntimeFixture::new_unique(None, None, "python3")
        .create(&pool)
        .await
        .unwrap();

    let input = CreateSensorInput {
        r#ref: "invalid.sensor".to_string(),
        pack: Some(99999), // Non-existent pack
        pack_ref: Some("invalid".to_string()),
        label: "Invalid Pack Sensor".to_string(),
        description: Some("Test sensor".to_string()),
        entrypoint: "sensors/invalid.py".to_string(),
        runtime: runtime.id,
        runtime_ref: runtime.r#ref.clone(),
        runtime_version_constraint: None,
        enabled: true,
        param_schema: None,
        config: None,
        worker_selector: json!({}),
        worker_tolerations: json!([]),
        worker_affinity: json!({}),
        artifact_retention_policy: None,
        artifact_retention_limit: None,
        log_retention_policy: None,
        log_retention_limit: None,
    };

    let result = SensorRepository::create(&pool, input).await;
    assert!(result.is_err());
    assert!(matches!(result.unwrap_err(), Error::Database(_)));
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_create_sensor_invalid_runtime_fails() {
    let pool = create_test_pool().await.unwrap();

    let input = CreateSensorInput {
        r#ref: "invalid.sensor".to_string(),
        pack: None,
        pack_ref: None,
        label: "Invalid Runtime Sensor".to_string(),
        description: Some("Test sensor".to_string()),
        entrypoint: "sensors/invalid.py".to_string(),
        runtime: 99999, // Non-existent runtime
        runtime_ref: "invalid.runtime".to_string(),
        runtime_version_constraint: None,
        enabled: true,
        param_schema: None,
        config: None,
        worker_selector: json!({}),
        worker_tolerations: json!([]),
        worker_affinity: json!({}),
        artifact_retention_policy: None,
        artifact_retention_limit: None,
        log_retention_policy: None,
        log_retention_limit: None,
    };

    let result = SensorRepository::create(&pool, input).await;
    assert!(result.is_err());
    assert!(matches!(result.unwrap_err(), Error::Database(_)));
}

// ============================================================================
// READ Tests
// ============================================================================

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_find_by_id_exists() {
    let pool = create_test_pool().await.unwrap();

    let pack = PackFixture::new_unique("find_pack")
        .create(&pool)
        .await
        .unwrap();

    let _trigger = TriggerFixture::new_unique(Some(pack.id), Some(pack.r#ref.clone()), "event")
        .create(&pool)
        .await
        .unwrap();

    let runtime = RuntimeFixture::new_unique(Some(pack.id), Some(pack.r#ref.clone()), "python3")
        .create(&pool)
        .await
        .unwrap();

    let sensor = SensorFixture::new_unique(
        Some(pack.id),
        Some(pack.r#ref.clone()),
        runtime.id,
        runtime.r#ref.clone(),
        "find_sensor",
    )
    .create(&pool)
    .await
    .unwrap();

    let found = SensorRepository::find_by_id(&pool, sensor.id)
        .await
        .unwrap();

    assert!(found.is_some());
    let found = found.unwrap();
    assert_eq!(found.id, sensor.id);
    assert_eq!(found.r#ref, sensor.r#ref);
    assert_eq!(found.label, sensor.label);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_find_by_id_not_exists() {
    let pool = create_test_pool().await.unwrap();

    let result = SensorRepository::find_by_id(&pool, 99999).await.unwrap();
    assert!(result.is_none());
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_get_by_id_exists() {
    let pool = create_test_pool().await.unwrap();

    let pack = PackFixture::new_unique("get_pack")
        .create(&pool)
        .await
        .unwrap();

    let _trigger = TriggerFixture::new_unique(Some(pack.id), Some(pack.r#ref.clone()), "event")
        .create(&pool)
        .await
        .unwrap();

    let runtime = RuntimeFixture::new_unique(Some(pack.id), Some(pack.r#ref.clone()), "python3")
        .create(&pool)
        .await
        .unwrap();

    let sensor = SensorFixture::new_unique(
        Some(pack.id),
        Some(pack.r#ref.clone()),
        runtime.id,
        runtime.r#ref.clone(),
        "get_sensor",
    )
    .create(&pool)
    .await
    .unwrap();

    let found = SensorRepository::get_by_id(&pool, sensor.id).await.unwrap();

    assert_eq!(found.id, sensor.id);
    assert_eq!(found.r#ref, sensor.r#ref);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_get_by_id_not_exists_fails() {
    let pool = create_test_pool().await.unwrap();

    let result = SensorRepository::get_by_id(&pool, 99999).await;
    assert!(result.is_err());
    assert!(matches!(result.unwrap_err(), Error::NotFound { .. }));
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_find_by_ref_exists() {
    let pool = create_test_pool().await.unwrap();

    let pack = PackFixture::new_unique("ref_pack")
        .create(&pool)
        .await
        .unwrap();

    let _trigger = TriggerFixture::new_unique(Some(pack.id), Some(pack.r#ref.clone()), "event")
        .create(&pool)
        .await
        .unwrap();

    let runtime = RuntimeFixture::new_unique(Some(pack.id), Some(pack.r#ref.clone()), "python3")
        .create(&pool)
        .await
        .unwrap();

    let sensor = SensorFixture::new_unique(
        Some(pack.id),
        Some(pack.r#ref.clone()),
        runtime.id,
        runtime.r#ref.clone(),
        "ref_sensor",
    )
    .create(&pool)
    .await
    .unwrap();

    let found = SensorRepository::find_by_ref(&pool, &sensor.r#ref)
        .await
        .unwrap();

    assert!(found.is_some());
    let found = found.unwrap();
    assert_eq!(found.id, sensor.id);
    assert_eq!(found.r#ref, sensor.r#ref);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_find_by_ref_not_exists() {
    let pool = create_test_pool().await.unwrap();

    let result = SensorRepository::find_by_ref(&pool, "nonexistent.sensor")
        .await
        .unwrap();
    assert!(result.is_none());
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_get_by_ref_exists() {
    let pool = create_test_pool().await.unwrap();

    let pack = PackFixture::new_unique("getref_pack")
        .create(&pool)
        .await
        .unwrap();

    let _trigger = TriggerFixture::new_unique(Some(pack.id), Some(pack.r#ref.clone()), "event")
        .create(&pool)
        .await
        .unwrap();

    let runtime = RuntimeFixture::new_unique(Some(pack.id), Some(pack.r#ref.clone()), "python3")
        .create(&pool)
        .await
        .unwrap();

    let sensor = SensorFixture::new_unique(
        Some(pack.id),
        Some(pack.r#ref.clone()),
        runtime.id,
        runtime.r#ref.clone(),
        "getref_sensor",
    )
    .create(&pool)
    .await
    .unwrap();

    let found = SensorRepository::get_by_ref(&pool, &sensor.r#ref)
        .await
        .unwrap();

    assert_eq!(found.id, sensor.id);
    assert_eq!(found.r#ref, sensor.r#ref);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_get_by_ref_not_exists_fails() {
    let pool = create_test_pool().await.unwrap();

    let result = SensorRepository::get_by_ref(&pool, "nonexistent.sensor").await;
    assert!(result.is_err());
    assert!(matches!(result.unwrap_err(), Error::NotFound { .. }));
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_list_all_sensors() {
    let pool = create_test_pool().await.unwrap();

    let pack = PackFixture::new_unique("list_pack")
        .create(&pool)
        .await
        .unwrap();

    let _trigger = TriggerFixture::new_unique(Some(pack.id), Some(pack.r#ref.clone()), "event")
        .create(&pool)
        .await
        .unwrap();

    let runtime = RuntimeFixture::new_unique(Some(pack.id), Some(pack.r#ref.clone()), "python3")
        .create(&pool)
        .await
        .unwrap();

    // Create multiple sensors
    let _sensor1 = SensorFixture::new_unique(
        Some(pack.id),
        Some(pack.r#ref.clone()),
        runtime.id,
        runtime.r#ref.clone(),
        "sensor_a",
    )
    .create(&pool)
    .await
    .unwrap();

    let _sensor2 = SensorFixture::new_unique(
        Some(pack.id),
        Some(pack.r#ref.clone()),
        runtime.id,
        runtime.r#ref.clone(),
        "sensor_b",
    )
    .create(&pool)
    .await
    .unwrap();

    let sensors = SensorRepository::list(&pool).await.unwrap();

    // Should have at least our 2 sensors (may have more from other parallel tests)
    assert!(sensors.len() >= 2);

    // Verify sensors are sorted by ref
    for i in 1..sensors.len() {
        assert!(sensors[i - 1].r#ref <= sensors[i].r#ref);
    }
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_list_empty() {
    let pool = create_test_pool().await.unwrap();

    // Count should be at least 0 (may have sensors from parallel tests)
    let sensors = SensorRepository::list(&pool).await.unwrap();
    // Just verify we can list sensors without error
    drop(sensors);
}

// ============================================================================
// UPDATE Tests
// ============================================================================

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_update_label() {
    let pool = create_test_pool().await.unwrap();

    let pack = PackFixture::new_unique("update_pack")
        .create(&pool)
        .await
        .unwrap();

    let _trigger = TriggerFixture::new_unique(Some(pack.id), Some(pack.r#ref.clone()), "event")
        .create(&pool)
        .await
        .unwrap();

    let runtime = RuntimeFixture::new_unique(Some(pack.id), Some(pack.r#ref.clone()), "python3")
        .create(&pool)
        .await
        .unwrap();

    let sensor = SensorFixture::new_unique(
        Some(pack.id),
        Some(pack.r#ref.clone()),
        runtime.id,
        runtime.r#ref.clone(),
        "update_sensor",
    )
    .create(&pool)
    .await
    .unwrap();

    let original_updated = sensor.updated - chrono::Duration::seconds(1);
    set_updated_for_test(&pool, "sensor", sensor.id, original_updated).await;

    let input = UpdateSensorInput {
        label: Some("Updated Sensor Label".to_string()),
        ..Default::default()
    };

    let updated = SensorRepository::update(&pool, sensor.id, input)
        .await
        .unwrap();

    assert_eq!(updated.id, sensor.id);
    assert_eq!(updated.label, "Updated Sensor Label");
    assert_eq!(updated.description, sensor.description); // Unchanged
    assert!(updated.updated > original_updated);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_update_description() {
    let pool = create_test_pool().await.unwrap();

    let pack = PackFixture::new_unique("desc_pack")
        .create(&pool)
        .await
        .unwrap();

    let _trigger = TriggerFixture::new_unique(Some(pack.id), Some(pack.r#ref.clone()), "event")
        .create(&pool)
        .await
        .unwrap();

    let runtime = RuntimeFixture::new_unique(Some(pack.id), Some(pack.r#ref.clone()), "python3")
        .create(&pool)
        .await
        .unwrap();

    let sensor = SensorFixture::new_unique(
        Some(pack.id),
        Some(pack.r#ref.clone()),
        runtime.id,
        runtime.r#ref.clone(),
        "desc_sensor",
    )
    .create(&pool)
    .await
    .unwrap();

    let input = UpdateSensorInput {
        description: Some(Patch::Set("New description for the sensor".to_string())),
        ..Default::default()
    };

    let updated = SensorRepository::update(&pool, sensor.id, input)
        .await
        .unwrap();

    assert_eq!(
        updated.description,
        Some("New description for the sensor".to_string())
    );
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_update_entrypoint() {
    let pool = create_test_pool().await.unwrap();

    let pack = PackFixture::new_unique("entry_pack")
        .create(&pool)
        .await
        .unwrap();

    let _trigger = TriggerFixture::new_unique(Some(pack.id), Some(pack.r#ref.clone()), "event")
        .create(&pool)
        .await
        .unwrap();

    let runtime = RuntimeFixture::new_unique(Some(pack.id), Some(pack.r#ref.clone()), "python3")
        .create(&pool)
        .await
        .unwrap();

    let sensor = SensorFixture::new_unique(
        Some(pack.id),
        Some(pack.r#ref.clone()),
        runtime.id,
        runtime.r#ref.clone(),
        "entry_sensor",
    )
    .create(&pool)
    .await
    .unwrap();

    let input = UpdateSensorInput {
        entrypoint: Some("sensors/new_entrypoint.py".to_string()),
        ..Default::default()
    };

    let updated = SensorRepository::update(&pool, sensor.id, input)
        .await
        .unwrap();

    assert_eq!(updated.entrypoint, "sensors/new_entrypoint.py");
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_update_enabled_status() {
    let pool = create_test_pool().await.unwrap();

    let pack = PackFixture::new_unique("enabled_pack")
        .create(&pool)
        .await
        .unwrap();

    let _trigger = TriggerFixture::new_unique(Some(pack.id), Some(pack.r#ref.clone()), "event")
        .create(&pool)
        .await
        .unwrap();

    let runtime = RuntimeFixture::new_unique(Some(pack.id), Some(pack.r#ref.clone()), "python3")
        .create(&pool)
        .await
        .unwrap();

    let sensor = SensorFixture::new_unique(
        Some(pack.id),
        Some(pack.r#ref.clone()),
        runtime.id,
        runtime.r#ref.clone(),
        "enabled_sensor",
    )
    .with_enabled(true)
    .create(&pool)
    .await
    .unwrap();

    assert!(sensor.enabled);

    let input = UpdateSensorInput {
        enabled: Some(false),
        ..Default::default()
    };

    let updated = SensorRepository::update(&pool, sensor.id, input)
        .await
        .unwrap();

    assert!(!updated.enabled);

    // Enable it again
    let input = UpdateSensorInput {
        enabled: Some(true),
        ..Default::default()
    };

    let updated = SensorRepository::update(&pool, sensor.id, input)
        .await
        .unwrap();

    assert!(updated.enabled);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_update_param_schema() {
    let pool = create_test_pool().await.unwrap();

    let pack = PackFixture::new_unique("schema_update_pack")
        .create(&pool)
        .await
        .unwrap();

    let _trigger = TriggerFixture::new_unique(Some(pack.id), Some(pack.r#ref.clone()), "event")
        .create(&pool)
        .await
        .unwrap();

    let runtime = RuntimeFixture::new_unique(Some(pack.id), Some(pack.r#ref.clone()), "python3")
        .create(&pool)
        .await
        .unwrap();

    let sensor = SensorFixture::new_unique(
        Some(pack.id),
        Some(pack.r#ref.clone()),
        runtime.id,
        runtime.r#ref.clone(),
        "schema_sensor",
    )
    .create(&pool)
    .await
    .unwrap();

    let new_schema = json!({
        "type": "object",
        "properties": {
            "timeout": {
                "type": "integer",
                "minimum": 0
            }
        }
    });

    let input = UpdateSensorInput {
        param_schema: Some(Patch::Set(new_schema.clone())),
        ..Default::default()
    };

    let updated = SensorRepository::update(&pool, sensor.id, input)
        .await
        .unwrap();

    assert_eq!(updated.param_schema, Some(new_schema));
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_update_multiple_fields() {
    let pool = create_test_pool().await.unwrap();

    let pack = PackFixture::new_unique("multi_pack")
        .create(&pool)
        .await
        .unwrap();

    let _trigger = TriggerFixture::new_unique(Some(pack.id), Some(pack.r#ref.clone()), "event")
        .create(&pool)
        .await
        .unwrap();

    let runtime = RuntimeFixture::new_unique(Some(pack.id), Some(pack.r#ref.clone()), "python3")
        .create(&pool)
        .await
        .unwrap();

    let sensor = SensorFixture::new_unique(
        Some(pack.id),
        Some(pack.r#ref.clone()),
        runtime.id,
        runtime.r#ref.clone(),
        "multi_sensor",
    )
    .create(&pool)
    .await
    .unwrap();

    let input = UpdateSensorInput {
        label: Some("Multi Update".to_string()),
        description: Some(Patch::Set("Updated multiple fields".to_string())),
        entrypoint: Some("sensors/multi.py".to_string()),
        enabled: Some(false),
        param_schema: Some(Patch::Set(json!({"type": "object"}))),
        ..Default::default()
    };

    let updated = SensorRepository::update(&pool, sensor.id, input)
        .await
        .unwrap();

    assert_eq!(updated.label, "Multi Update");
    assert_eq!(
        updated.description,
        Some("Updated multiple fields".to_string())
    );
    assert_eq!(updated.entrypoint, "sensors/multi.py");
    assert!(!updated.enabled);
    assert_eq!(updated.param_schema, Some(json!({"type": "object"})));
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_update_no_changes() {
    let pool = create_test_pool().await.unwrap();

    let pack = PackFixture::new_unique("nochange_pack")
        .create(&pool)
        .await
        .unwrap();

    let _trigger = TriggerFixture::new_unique(Some(pack.id), Some(pack.r#ref.clone()), "event")
        .create(&pool)
        .await
        .unwrap();

    let runtime = RuntimeFixture::new_unique(Some(pack.id), Some(pack.r#ref.clone()), "python3")
        .create(&pool)
        .await
        .unwrap();

    let sensor = SensorFixture::new_unique(
        Some(pack.id),
        Some(pack.r#ref.clone()),
        runtime.id,
        runtime.r#ref.clone(),
        "nochange_sensor",
    )
    .create(&pool)
    .await
    .unwrap();

    let original_updated = sensor.updated;

    let input = UpdateSensorInput::default();

    let updated = SensorRepository::update(&pool, sensor.id, input)
        .await
        .unwrap();

    assert_eq!(updated.id, sensor.id);
    assert_eq!(updated.label, sensor.label);
    assert_eq!(updated.description, sensor.description);
    assert_eq!(updated.entrypoint, sensor.entrypoint);
    assert_eq!(updated.enabled, sensor.enabled);
    // Updated timestamp should not change when no fields are updated
    assert_eq!(updated.updated, original_updated);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_update_nonexistent_sensor_fails() {
    let pool = create_test_pool().await.unwrap();

    let input = UpdateSensorInput {
        label: Some("Updated".to_string()),
        ..Default::default()
    };

    let result = SensorRepository::update(&pool, 99999, input).await;
    assert!(result.is_err());
}

// ============================================================================
// DELETE Tests
// ============================================================================

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_delete_existing_sensor() {
    let pool = create_test_pool().await.unwrap();

    let pack = PackFixture::new_unique("delete_pack")
        .create(&pool)
        .await
        .unwrap();

    let _trigger = TriggerFixture::new_unique(Some(pack.id), Some(pack.r#ref.clone()), "event")
        .create(&pool)
        .await
        .unwrap();

    let runtime = RuntimeFixture::new_unique(Some(pack.id), Some(pack.r#ref.clone()), "python3")
        .create(&pool)
        .await
        .unwrap();

    let sensor = SensorFixture::new_unique(
        Some(pack.id),
        Some(pack.r#ref.clone()),
        runtime.id,
        runtime.r#ref.clone(),
        "delete_sensor",
    )
    .create(&pool)
    .await
    .unwrap();

    let deleted = SensorRepository::delete(&pool, sensor.id).await.unwrap();
    assert!(deleted);

    // Verify sensor is gone
    let result = SensorRepository::find_by_id(&pool, sensor.id)
        .await
        .unwrap();
    assert!(result.is_none());
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_delete_nonexistent_sensor() {
    let pool = create_test_pool().await.unwrap();

    let deleted = SensorRepository::delete(&pool, 99999).await.unwrap();
    assert!(!deleted);
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_delete_sensor_when_pack_deleted() {
    let pool = create_test_pool().await.unwrap();

    let pack = PackFixture::new_unique("cascade_pack")
        .create(&pool)
        .await
        .unwrap();

    let _trigger = TriggerFixture::new_unique(Some(pack.id), Some(pack.r#ref.clone()), "event")
        .create(&pool)
        .await
        .unwrap();

    let runtime = RuntimeFixture::new_unique(Some(pack.id), Some(pack.r#ref.clone()), "python3")
        .create(&pool)
        .await
        .unwrap();

    let sensor = SensorFixture::new_unique(
        Some(pack.id),
        Some(pack.r#ref.clone()),
        runtime.id,
        runtime.r#ref.clone(),
        "cascade_sensor",
    )
    .create(&pool)
    .await
    .unwrap();

    // Delete the pack
    use attune_common::repositories::{pack::PackRepository, Delete as _};
    PackRepository::delete(&pool, pack.id).await.unwrap();

    // Sensor should also be deleted due to CASCADE
    let result = SensorRepository::find_by_id(&pool, sensor.id)
        .await
        .unwrap();
    assert!(result.is_none());
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_delete_sensor_nullifies_trigger_sensor_ref() {
    let pool = create_test_pool().await.unwrap();

    let pack = PackFixture::new_unique("trigger_cascade_pack")
        .create(&pool)
        .await
        .unwrap();

    let trigger = TriggerFixture::new_unique(Some(pack.id), Some(pack.r#ref.clone()), "event")
        .create(&pool)
        .await
        .unwrap();

    let runtime = RuntimeFixture::new_unique(Some(pack.id), Some(pack.r#ref.clone()), "python3")
        .create(&pool)
        .await
        .unwrap();

    let sensor = SensorFixture::new_unique(
        Some(pack.id),
        Some(pack.r#ref.clone()),
        runtime.id,
        runtime.r#ref.clone(),
        "trigger_cascade_sensor",
    )
    .create(&pool)
    .await
    .unwrap();

    // Link trigger to sensor
    use attune_common::repositories::trigger::TriggerRepository;
    let update_input = attune_common::repositories::trigger::UpdateTriggerInput {
        label: None,
        description: None,
        enabled: None,
        param_schema: None,
        out_schema: None,
        sensor: Some(attune_common::repositories::Patch::Set(sensor.id)),
        sensor_ref: Some(attune_common::repositories::Patch::Set(
            sensor.r#ref.clone(),
        )),
        ..Default::default()
    };
    let updated_trigger = TriggerRepository::update(&pool, trigger.id, update_input)
        .await
        .unwrap();
    assert_eq!(updated_trigger.sensor, Some(sensor.id));

    // Delete the sensor
    use attune_common::repositories::Delete as _;
    SensorRepository::delete(&pool, sensor.id).await.unwrap();

    // Trigger should still exist but with sensor set to NULL (ON DELETE SET NULL)
    let result = TriggerRepository::find_by_id(&pool, trigger.id)
        .await
        .unwrap()
        .unwrap();
    assert!(result.sensor.is_none());
    assert_eq!(result.sensor_ref.as_deref(), Some(sensor.r#ref.as_str()));
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_delete_sensor_when_runtime_deleted() {
    let pool = create_test_pool().await.unwrap();

    let pack = PackFixture::new_unique("runtime_cascade_pack")
        .create(&pool)
        .await
        .unwrap();

    let _trigger = TriggerFixture::new_unique(Some(pack.id), Some(pack.r#ref.clone()), "event")
        .create(&pool)
        .await
        .unwrap();

    let runtime = RuntimeFixture::new_unique(Some(pack.id), Some(pack.r#ref.clone()), "python3")
        .create(&pool)
        .await
        .unwrap();

    let sensor = SensorFixture::new_unique(
        Some(pack.id),
        Some(pack.r#ref.clone()),
        runtime.id,
        runtime.r#ref.clone(),
        "runtime_cascade_sensor",
    )
    .create(&pool)
    .await
    .unwrap();

    // Delete the runtime
    use attune_common::repositories::{runtime::RuntimeRepository, Delete as _};
    RuntimeRepository::delete(&pool, runtime.id).await.unwrap();

    // Sensor should also be deleted due to CASCADE
    let result = SensorRepository::find_by_id(&pool, sensor.id)
        .await
        .unwrap();
    assert!(result.is_none());
}

// ============================================================================
// Specialized Query Tests
// ============================================================================

// Note: test_find_by_trigger removed — the relationship is now trigger→sensor
// (trigger.sensor FK), so the equivalent query is TriggerRepository::find_by_sensor.
// See trigger_repository_tests.rs for those tests.

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_find_enabled() {
    let pool = create_test_pool().await.unwrap();

    let pack = PackFixture::new_unique("enabled_find_pack")
        .create(&pool)
        .await
        .unwrap();

    let _trigger = TriggerFixture::new_unique(Some(pack.id), Some(pack.r#ref.clone()), "event")
        .create(&pool)
        .await
        .unwrap();

    let runtime = RuntimeFixture::new_unique(Some(pack.id), Some(pack.r#ref.clone()), "python3")
        .create(&pool)
        .await
        .unwrap();

    // Create enabled sensor
    let enabled_sensor = SensorFixture::new_unique(
        Some(pack.id),
        Some(pack.r#ref.clone()),
        runtime.id,
        runtime.r#ref.clone(),
        "enabled_sensor",
    )
    .with_enabled(true)
    .create(&pool)
    .await
    .unwrap();

    // Create disabled sensor
    let _disabled_sensor = SensorFixture::new_unique(
        Some(pack.id),
        Some(pack.r#ref.clone()),
        runtime.id,
        runtime.r#ref.clone(),
        "disabled_sensor",
    )
    .with_enabled(false)
    .create(&pool)
    .await
    .unwrap();

    let enabled_sensors = SensorRepository::find_enabled(&pool).await.unwrap();

    // Should only contain enabled sensors
    assert!(enabled_sensors.iter().all(|s| s.enabled));
    assert!(enabled_sensors.iter().any(|s| s.id == enabled_sensor.id));
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_find_enabled_empty() {
    let pool = create_test_pool().await.unwrap();

    let pack = PackFixture::new_unique("disabled_only_pack")
        .create(&pool)
        .await
        .unwrap();

    let _trigger = TriggerFixture::new_unique(Some(pack.id), Some(pack.r#ref.clone()), "event")
        .create(&pool)
        .await
        .unwrap();

    let runtime = RuntimeFixture::new_unique(Some(pack.id), Some(pack.r#ref.clone()), "python3")
        .create(&pool)
        .await
        .unwrap();

    // Create only disabled sensor
    let disabled = SensorFixture::new_unique(
        Some(pack.id),
        Some(pack.r#ref.clone()),
        runtime.id,
        runtime.r#ref.clone(),
        "disabled",
    )
    .with_enabled(false)
    .create(&pool)
    .await
    .unwrap();

    let enabled_sensors = SensorRepository::find_enabled(&pool).await.unwrap();
    // May have enabled sensors from other parallel tests, just verify our disabled sensor is not in the list
    assert!(enabled_sensors.iter().all(|s| s.id != disabled.id));
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_find_by_pack() {
    let pool = create_test_pool().await.unwrap();

    let pack1 = PackFixture::new_unique("pack_find1")
        .create(&pool)
        .await
        .unwrap();

    let pack2 = PackFixture::new_unique("pack_find2")
        .create(&pool)
        .await
        .unwrap();

    let _trigger1 = TriggerFixture::new_unique(Some(pack1.id), Some(pack1.r#ref.clone()), "event1")
        .create(&pool)
        .await
        .unwrap();

    let _trigger2 = TriggerFixture::new_unique(Some(pack2.id), Some(pack2.r#ref.clone()), "event2")
        .create(&pool)
        .await
        .unwrap();

    let runtime1 = RuntimeFixture::new_unique(Some(pack1.id), Some(pack1.r#ref.clone()), "python3")
        .create(&pool)
        .await
        .unwrap();

    let runtime2 = RuntimeFixture::new_unique(Some(pack2.id), Some(pack2.r#ref.clone()), "nodejs")
        .create(&pool)
        .await
        .unwrap();

    // Create sensors for pack1
    let sensor1 = SensorFixture::new_unique(
        Some(pack1.id),
        Some(pack1.r#ref.clone()),
        runtime1.id,
        runtime1.r#ref.clone(),
        "pack1_sensor1",
    )
    .create(&pool)
    .await
    .unwrap();

    let sensor2 = SensorFixture::new_unique(
        Some(pack1.id),
        Some(pack1.r#ref.clone()),
        runtime1.id,
        runtime1.r#ref.clone(),
        "pack1_sensor2",
    )
    .create(&pool)
    .await
    .unwrap();

    // Create sensor for pack2
    let _sensor3 = SensorFixture::new_unique(
        Some(pack2.id),
        Some(pack2.r#ref.clone()),
        runtime2.id,
        runtime2.r#ref.clone(),
        "pack2_sensor",
    )
    .create(&pool)
    .await
    .unwrap();

    let pack1_sensors = SensorRepository::find_by_pack(&pool, pack1.id)
        .await
        .unwrap();

    assert_eq!(pack1_sensors.len(), 2);
    assert!(pack1_sensors.iter().all(|s| s.pack == Some(pack1.id)));
    assert!(pack1_sensors.iter().any(|s| s.id == sensor1.id));
    assert!(pack1_sensors.iter().any(|s| s.id == sensor2.id));
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_find_by_pack_no_sensors() {
    let pool = create_test_pool().await.unwrap();

    let pack = PackFixture::new_unique("empty_pack")
        .create(&pool)
        .await
        .unwrap();

    let sensors = SensorRepository::find_by_pack(&pool, pack.id)
        .await
        .unwrap();

    assert_eq!(sensors.len(), 0);
}

// ============================================================================
// Timestamp Tests
// ============================================================================

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_created_timestamp_set_automatically() {
    let pool = create_test_pool().await.unwrap();

    let pack = PackFixture::new_unique("timestamp_pack")
        .create(&pool)
        .await
        .unwrap();

    let _trigger = TriggerFixture::new_unique(Some(pack.id), Some(pack.r#ref.clone()), "event")
        .create(&pool)
        .await
        .unwrap();

    let runtime = RuntimeFixture::new_unique(Some(pack.id), Some(pack.r#ref.clone()), "python3")
        .create(&pool)
        .await
        .unwrap();

    let before = database_clock(&pool).await;

    let sensor = SensorFixture::new_unique(
        Some(pack.id),
        Some(pack.r#ref.clone()),
        runtime.id,
        runtime.r#ref.clone(),
        "timestamp_sensor",
    )
    .create(&pool)
    .await
    .unwrap();

    let after = database_clock(&pool).await;

    assert!(sensor.created >= before);
    assert!(sensor.created <= after);
    assert_eq!(sensor.created, sensor.updated); // Should be equal on creation
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_updated_timestamp_changes_on_update() {
    let pool = create_test_pool().await.unwrap();

    let pack = PackFixture::new_unique("update_time_pack")
        .create(&pool)
        .await
        .unwrap();

    let _trigger = TriggerFixture::new_unique(Some(pack.id), Some(pack.r#ref.clone()), "event")
        .create(&pool)
        .await
        .unwrap();

    let runtime = RuntimeFixture::new_unique(Some(pack.id), Some(pack.r#ref.clone()), "python3")
        .create(&pool)
        .await
        .unwrap();

    let sensor = SensorFixture::new_unique(
        Some(pack.id),
        Some(pack.r#ref.clone()),
        runtime.id,
        runtime.r#ref.clone(),
        "update_time_sensor",
    )
    .create(&pool)
    .await
    .unwrap();

    let original_updated = sensor.updated - chrono::Duration::seconds(1);
    set_updated_for_test(&pool, "sensor", sensor.id, original_updated).await;

    let input = UpdateSensorInput {
        label: Some("Updated".to_string()),
        ..Default::default()
    };

    let updated = SensorRepository::update(&pool, sensor.id, input)
        .await
        .unwrap();

    assert!(updated.updated > original_updated);
    assert_eq!(updated.created, sensor.created); // Created should not change
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_updated_timestamp_unchanged_on_read() {
    let pool = create_test_pool().await.unwrap();

    let pack = PackFixture::new_unique("read_time_pack")
        .create(&pool)
        .await
        .unwrap();

    let _trigger = TriggerFixture::new_unique(Some(pack.id), Some(pack.r#ref.clone()), "event")
        .create(&pool)
        .await
        .unwrap();

    let runtime = RuntimeFixture::new_unique(Some(pack.id), Some(pack.r#ref.clone()), "python3")
        .create(&pool)
        .await
        .unwrap();

    let sensor = SensorFixture::new_unique(
        Some(pack.id),
        Some(pack.r#ref.clone()),
        runtime.id,
        runtime.r#ref.clone(),
        "read_time_sensor",
    )
    .create(&pool)
    .await
    .unwrap();

    let original_updated = sensor.updated;

    // Read the sensor
    let found = SensorRepository::find_by_id(&pool, sensor.id)
        .await
        .unwrap()
        .unwrap();

    assert_eq!(found.updated, original_updated); // Should not change
}

// ============================================================================
// JSON Field Tests
// ============================================================================

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_param_schema_complex_structure() {
    let pool = create_test_pool().await.unwrap();

    let pack = PackFixture::new_unique("complex_schema_pack")
        .create(&pool)
        .await
        .unwrap();

    let _trigger = TriggerFixture::new_unique(Some(pack.id), Some(pack.r#ref.clone()), "event")
        .create(&pool)
        .await
        .unwrap();

    let runtime = RuntimeFixture::new_unique(Some(pack.id), Some(pack.r#ref.clone()), "python3")
        .create(&pool)
        .await
        .unwrap();

    let complex_schema = json!({
        "type": "object",
        "properties": {
            "connection": {
                "type": "object",
                "properties": {
                    "host": { "type": "string" },
                    "port": { "type": "integer" },
                    "ssl": { "type": "boolean" }
                },
                "required": ["host", "port"]
            },
            "filters": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "field": { "type": "string" },
                        "operator": { "enum": ["eq", "ne", "gt", "lt"] },
                        "value": {}
                    }
                }
            },
            "poll_interval": {
                "type": "integer",
                "minimum": 1,
                "maximum": 3600
            }
        },
        "required": ["connection", "poll_interval"]
    });

    let sensor = SensorFixture::new_unique(
        Some(pack.id),
        Some(pack.r#ref.clone()),
        runtime.id,
        runtime.r#ref.clone(),
        "complex_sensor",
    )
    .with_param_schema(complex_schema.clone())
    .create(&pool)
    .await
    .unwrap();

    // Retrieve and verify
    let found = SensorRepository::find_by_id(&pool, sensor.id)
        .await
        .unwrap()
        .unwrap();

    assert_eq!(found.param_schema, Some(complex_schema));
}

#[tokio::test]
#[ignore = "integration test — requires database"]
async fn test_param_schema_can_be_null() {
    let pool = create_test_pool().await.unwrap();

    let pack = PackFixture::new_unique("null_schema_pack")
        .create(&pool)
        .await
        .unwrap();

    let _trigger = TriggerFixture::new_unique(Some(pack.id), Some(pack.r#ref.clone()), "event")
        .create(&pool)
        .await
        .unwrap();

    let runtime = RuntimeFixture::new_unique(Some(pack.id), Some(pack.r#ref.clone()), "python3")
        .create(&pool)
        .await
        .unwrap();

    let sensor = SensorFixture::new_unique(
        Some(pack.id),
        Some(pack.r#ref.clone()),
        runtime.id,
        runtime.r#ref.clone(),
        "null_schema_sensor",
    )
    .create(&pool)
    .await
    .unwrap();

    assert_eq!(sensor.param_schema, None);

    // Update to add schema
    let schema = json!({"type": "object"});
    let input = UpdateSensorInput {
        param_schema: Some(Patch::Set(schema.clone())),
        ..Default::default()
    };

    let updated = SensorRepository::update(&pool, sensor.id, input)
        .await
        .unwrap();

    assert_eq!(updated.param_schema, Some(schema));
}
