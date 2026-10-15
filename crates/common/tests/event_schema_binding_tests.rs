mod helpers;

use attune_common::repositories::{
    event::{CreateEventInput, EventRepository},
    event_schema::EventSchemaRepository,
    execution_secret_value::ExecutionSecretValueRepository,
    trigger::{CreateTriggerInput, TriggerRepository, UpdateTriggerInput},
    Create, Delete, FindById, Patch,
};
use helpers::create_test_pool;
use serde_json::json;

#[tokio::test]
async fn internal_producers_bind_and_encrypt_events_atomically() {
    let db = create_test_pool().await.unwrap();
    let trigger = TriggerRepository::create(
        &db,
        CreateTriggerInput {
            r#ref: "test.internal_event".into(),
            pack: None,
            pack_ref: None,
            label: "Internal event".into(),
            description: None,
            enabled: true,
            param_schema: None,
            out_schema: Some(json!({"token":{"type":"string","secret":true,"required":true}})),
            event_config_schema: None,
            sensor: None,
            sensor_ref: None,
            is_adhoc: true,
            reference_visibility: Default::default(),
            reference_allowed_pack_refs: vec![],
        },
    )
    .await
    .unwrap();
    let input = CreateEventInput {
        trigger: Some(trigger.id),
        trigger_ref: trigger.r#ref.clone(),
        config: None,
        payload: Some(json!({"token":"private-internal-event"})),
        trace_tag: None,
        source: None,
        source_ref: Some("attune.system".into()),
        rule: None,
        rule_ref: None,
    };
    let error = EventSchemaRepository::create_internal(&db, input.clone(), None)
        .await
        .unwrap_err();
    assert!(!error.to_string().contains("private-internal-event"));
    let key = "0123456789abcdef0123456789abcdef";
    let event = EventSchemaRepository::create_internal(&db, input, Some(key))
        .await
        .unwrap()
        .unwrap();
    assert!(!event
        .payload
        .as_ref()
        .unwrap()
        .to_string()
        .contains("private-internal-event"));
    let binding = EventSchemaRepository::find(&db, event.id)
        .await
        .unwrap()
        .unwrap();
    assert!(binding.matches_owner(&event));
    let secrets = ExecutionSecretValueRepository::find_stored_by_entity(
        &db,
        attune_common::secret_values::ENTITY_EVENT_PAYLOAD,
        event.id,
    )
    .await
    .unwrap();
    assert_eq!(secrets.len(), 1);
    let value =
        attune_common::secret_values::restore_secret_values(event.payload.unwrap(), &secrets, key)
            .unwrap();
    assert_eq!(value["token"], "private-internal-event");
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM event WHERE trigger = $1")
        .bind(trigger.id)
        .fetch_one(&db)
        .await
        .unwrap();
    assert_eq!(
        count, 1,
        "failed producer admission must not leave an event behind"
    );
}

#[tokio::test]
async fn event_sections_keep_the_admission_contract_after_trigger_updates_and_clean_up_with_owner()
{
    let db = create_test_pool().await.unwrap();
    let trigger = TriggerRepository::create(
        &db,
        CreateTriggerInput {
            r#ref: "test.bound-event".into(),
            pack: None,
            pack_ref: None,
            label: "Bound event".into(),
            description: None,
            enabled: true,
            param_schema: Some(json!({"interval": {"type": "integer", "required": true}})),
            out_schema: Some(json!({"message": {"type": "string", "required": true}})),
            event_config_schema: Some(json!({"token": {"type": "string", "secret": true}})),
            sensor: None,
            sensor_ref: None,
            is_adhoc: true,
            reference_visibility: Default::default(),
            reference_allowed_pack_refs: vec![],
        },
    )
    .await
    .unwrap();
    let mut tx = db.begin().await.unwrap();
    let (_, binding) = EventSchemaRepository::materialize_current(&mut tx, trigger.id, false)
        .await
        .unwrap();
    // Trigger-instance requiredness does not constrain emitted data.
    let prepared = binding
        .prepare(
            Some(json!({"message": "original"})),
            Some(json!({"token": "private"})),
        )
        .unwrap();
    assert!(binding.prepare(Some(json!({})), None).is_err());
    let event = EventRepository::create(
        &mut *tx,
        CreateEventInput {
            trigger: Some(trigger.id),
            trigger_ref: trigger.r#ref.clone(),
            payload: prepared.payload,
            config: prepared.config,
            trace_tag: None,
            source: None,
            source_ref: None,
            rule: None,
            rule_ref: None,
        },
    )
    .await
    .unwrap();
    EventSchemaRepository::attach(&mut tx, event.id, &binding)
        .await
        .unwrap();
    let secrets = attune_common::secret_values::prepare_secret_values(
        prepared.config_secrets,
        "owned-event-encryption-key-32-characters",
    )
    .unwrap();
    ExecutionSecretValueRepository::upsert_many_with_conn(
        &mut tx,
        attune_common::secret_values::ENTITY_EVENT_CONFIG,
        event.id,
        &secrets,
    )
    .await
    .unwrap();
    EventSchemaRepository::attach(&mut tx, event.id, &binding)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    TriggerRepository::update(
        &db,
        trigger.id,
        UpdateTriggerInput {
            out_schema: Some(Patch::Set(
                json!({"count": {"type": "integer", "required": true}}),
            )),
            event_config_schema: Some(Patch::Clear),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let stored = EventSchemaRepository::find(&db, event.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored, binding);
    stored
        .validate_restored(
            Some(&json!({"message": "original"})),
            Some(&json!({"token": "private"})),
        )
        .unwrap();
    let mut tx = db.begin().await.unwrap();
    let (_, latest) = EventSchemaRepository::materialize_current(&mut tx, trigger.id, false)
        .await
        .unwrap();
    assert!(latest
        .prepare(Some(json!({"message": "original"})), None)
        .is_err());
    assert!(EventSchemaRepository::attach(&mut tx, event.id, &latest)
        .await
        .is_err());
    tx.rollback().await.unwrap();
    TriggerRepository::delete(&db, trigger.id).await.unwrap();
    let archived = EventRepository::find_by_id(&db, event.id)
        .await
        .unwrap()
        .unwrap();
    assert!(binding.matches_owner(&archived));
    assert_eq!(
        EventSchemaRepository::find(&db, event.id).await.unwrap(),
        Some(binding)
    );
    assert!(
        sqlx::query("UPDATE event_schema_binding SET contract = '{}' WHERE event = $1")
            .bind(event.id)
            .execute(&db)
            .await
            .is_err()
    );
    assert!(sqlx::query("UPDATE execution_secret_value SET encrypted_value = 'changed' WHERE entity_type = 'event_config' AND entity_id = $1")
        .bind(event.id).execute(&db).await.is_err());
    assert!(sqlx::query(
        "DELETE FROM execution_secret_value WHERE entity_type = 'event_config' AND entity_id = $1"
    )
    .bind(event.id)
    .execute(&db)
    .await
    .is_err());
    EventRepository::delete(&db, event.id).await.unwrap();
    assert!(EventSchemaRepository::find(&db, event.id)
        .await
        .unwrap()
        .is_none());
    assert!(ExecutionSecretValueRepository::find_stored_by_entity(
        &db,
        attune_common::secret_values::ENTITY_EVENT_CONFIG,
        event.id
    )
    .await
    .unwrap()
    .is_empty());
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn default_partition_repair_preserves_bindings_and_partition_expiry_prunes_them() {
    use attune_common::{
        config::NativeMaintenanceConfig,
        event_schema::EventSchemaBinding,
        repositories::{
            native_maintenance::{
                partitions::{utc_day, PartitionRepository},
                ManagedTable,
            },
            FindById,
        },
    };
    let db = create_test_pool().await.unwrap();
    let config = NativeMaintenanceConfig::default();
    // Fresh installations precreate nearby daily leaves. This owned database
    // first expires those empty leaves so the fixture exercises DEFAULT repair.
    PartitionRepository::expire_before(
        &db,
        ManagedTable::Event,
        utc_day(chrono::Utc::now()) + chrono::Duration::days(2),
        &config,
        false,
        || false,
    )
    .await
    .unwrap();
    let mut tx = db.begin().await.unwrap();
    let event = EventRepository::create(
        &mut *tx,
        CreateEventInput {
            trigger: None,
            trigger_ref: "test.physical-move".into(),
            config: None,
            payload: Some(json!({"message": "original"})),
            trace_tag: None,
            source: None,
            source_ref: None,
            rule: None,
            rule_ref: None,
        },
    )
    .await
    .unwrap();
    let binding = EventSchemaBinding {
        trigger_id: None,
        trigger_ref: event.trigger_ref.clone(),
        payload: None,
        config: None,
        webhook: false,
    };
    EventSchemaRepository::attach(&mut tx, event.id, &binding)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let day = utc_day(event.created);
    let repaired = PartitionRepository::ensure_day(&db, ManagedTable::Event, day, &config)
        .await
        .unwrap();
    assert!(matches!(repaired, attune_common::repositories::native_maintenance::partitions::PartitionRepairOutcome::Applied { rows_moved: 1 }));
    assert_eq!(
        EventSchemaRepository::find(&db, event.id).await.unwrap(),
        Some(binding)
    );
    let expired = PartitionRepository::expire_before(
        &db,
        ManagedTable::Event,
        day + chrono::Duration::days(1),
        &config,
        false,
        || false,
    )
    .await
    .unwrap();
    assert_eq!(expired.partitions_dropped, 1);
    assert!(EventRepository::find_by_id(&db, event.id)
        .await
        .unwrap()
        .is_none());
    assert!(EventSchemaRepository::find(&db, event.id)
        .await
        .unwrap()
        .is_none());
    db.cleanup().await.unwrap();
}
