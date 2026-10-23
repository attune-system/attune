mod helpers;
use attune_common::{
    event_schema::EventSchemaBinding,
    models::Event,
    repositories::{
        event::{CreateEventInput, EventRepository},
        event_origin::{EventOriginRepository as Sources, EVENT_SOURCE_PAYLOAD},
        native_maintenance::{
            partitions::{utc_day, PartitionRepository},
            ManagedTable,
        },
        Create, FindById,
    },
    secret_values::{prepare_secret_values, SecretSource},
    template_resolver::TemplateContext,
    value_schema::{compile, Limits, RegistryView, SchemaContext, SourceIdentity},
};
use serde_json::json;

#[tokio::test]
async fn partition_twins_keep_distinct_schema_ciphertext_and_template_origins() {
    let db = helpers::create_test_pool().await.unwrap();
    let key = "exact-event-source-test-encryption-key";
    let reference = "test.partition_twins";
    let contract = compile(
        SourceIdentity::Assigned {
            component_ref: reference.into(),
            assignment_id: uuid::Uuid::new_v4(),
        },
        SchemaContext::FlatDocument,
        &json!({"secret":{"type":"string","secret":true}}),
        &RegistryView::new(),
        Limits::default(),
    )
    .unwrap();
    let binding = EventSchemaBinding {
        trigger_id: None,
        trigger_ref: reference.into(),
        payload: Some(contract),
        config: None,
        webhook: false,
    };
    let first_sections = binding
        .prepare(Some(json!({"secret":"first-secret"})), None)
        .unwrap();
    let first_secrets = prepare_secret_values(first_sections.payload_secrets, key).unwrap();
    let mut tx = db.begin().await.unwrap();
    let first = EventRepository::create(
        &mut *tx,
        CreateEventInput {
            trigger: None,
            trigger_ref: reference.into(),
            config: None,
            payload: first_sections.payload,
            trace_tag: None,
            source: None,
            source_ref: None,
            rule: None,
            rule_ref: None,
        },
    )
    .await
    .unwrap();
    let first_source = Sources::capture(&mut tx, &first, &binding, &first_secrets, &[])
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let second_created = first.created + chrono::Duration::days(15);
    PartitionRepository::ensure_day(
        &db,
        ManagedTable::Event,
        utc_day(second_created),
        &Default::default(),
    )
    .await
    .unwrap();
    let second_sections = binding
        .prepare(Some(json!({"secret":"second-secret"})), None)
        .unwrap();
    let second_secrets = prepare_secret_values(second_sections.payload_secrets, key).unwrap();
    let mut tx = db.begin().await.unwrap();
    // Deliberate database fixture: native partitions permit the same numeric ID.
    sqlx::query("INSERT INTO event(id,created,trigger_ref,payload) VALUES ($1,$2,$3,$4)")
        .bind(first.id)
        .bind(second_created)
        .bind(reference)
        .bind(second_sections.payload)
        .execute(&mut *tx)
        .await
        .unwrap();
    let second: Event = EventRepository::find_by_identity(&mut *tx, first.id, second_created)
        .await
        .unwrap()
        .unwrap();
    let second_source = Sources::capture(&mut tx, &second, &binding, &second_secrets, &[])
        .await
        .unwrap();
    assert_ne!(first_source, second_source);
    tx.commit().await.unwrap();
    let tables: Vec<i64> =
        sqlx::query_scalar("SELECT tableoid::BIGINT FROM event WHERE id = $1 ORDER BY created")
            .bind(first.id)
            .fetch_all(&*db)
            .await
            .unwrap();
    assert_eq!(tables.len(), 2);
    assert_ne!(tables[0], tables[1]);
    assert!(EventRepository::find_by_id(&db, first.id).await.is_err());
    for (event, source, expected) in [
        (&first, first_source, "first-secret"),
        (&second, second_source, "second-secret"),
    ] {
        let snapshot = Sources::find(&db, source).await.unwrap();
        assert!(snapshot.matches(event));
        assert_eq!(
            snapshot
                .restore_payload(event.payload.clone().unwrap(), key)
                .unwrap()["secret"],
            expected
        );
        let evidence = Sources::evidence(&db, source).await.unwrap();
        assert_eq!(
            (evidence.event, evidence.created),
            (event.id, event.created)
        );
        assert!(evidence.parent(EVENT_SOURCE_PAYLOAD, "/secret").is_ok());
        assert_eq!(
            Sources::source_id(&db, event.id, event.created)
                .await
                .unwrap(),
            source
        );
        let context = TemplateContext::new(json!({"secret":expected}), json!({}), json!({}))
            .with_event_source_secret_paths(source, vec!["/secret".into()]);
        let rendered = attune_common::template_resolver::resolve_templates_with_sensitivity(
            &json!({"copy":"{{ event.payload.secret }}"}),
            &context,
        );
        assert!(rendered.is_ok());
    }
    assert!(!Sources::find(&db, first_source)
        .await
        .unwrap()
        .matches(&second));
    // The legacy producer still creates an intentionally unqualified origin;
    // it is never silently upgraded by consulting today's event ID lookup.
    assert!(
        matches!(SecretSource::EventPayload { event_id: first.id, path: "/secret".into() }.provenance().origins[0],
        attune_common::secret_provenance::SecretOrigin::Entity { ref entity_type, .. } if entity_type == "event_payload")
    );
    sqlx::query("DELETE FROM event WHERE id=$1 AND created=$2")
        .bind(first.id)
        .bind(first.created)
        .execute(&*db)
        .await
        .unwrap();
    let mut tx = db.begin().await.unwrap();
    assert_eq!(
        Sources::cleanup_orphans(&mut tx, 1, second.created + chrono::Duration::days(1))
            .await
            .unwrap(),
        1
    );
    tx.commit().await.unwrap();
    assert!(Sources::find(&db, first_source).await.is_err());
    assert!(Sources::find(&db, second_source)
        .await
        .unwrap()
        .matches(&second));
    db.cleanup().await.unwrap();
}
