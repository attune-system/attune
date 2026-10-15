mod helpers;

use attune_common::{
    repositories::{
        action::{ActionRepository, UpdateActionInput},
        component_lifecycle::PackProjectionIds,
        pack::{PackRepository, UpdatePackInput},
        pack_release::{CreatePackReleaseInput, PackReleaseRepository},
        schema_type::SchemaTypeRepository,
        trigger::TriggerRepository,
        workflow::{CreateWorkflowDefinitionInput, WorkflowDefinitionRepository},
        Delete, FindById,
    },
    schema_type::SchemaTypeDefinition,
    value_schema::{compile_inline, SchemaContext},
};
use helpers::{create_test_pool, ActionFixture, PackFixture, TriggerFixture};
use serde_json::json;

#[tokio::test]
async fn named_consumer_writes_validate_atomically_and_retain_dependency_edges() {
    let db = create_test_pool().await.unwrap();
    let owner = PackFixture::new_unique("consumer_type_owner")
        .create(&db)
        .await
        .unwrap();
    let consumer = PackFixture::new_unique("consumer_type_user")
        .create(&db)
        .await
        .unwrap();
    let definition = SchemaTypeDefinition::parse(
        &format!(
            "ref: {}.value\nlabel: Value\nschema: {{type: string, minLength: 3}}\n",
            owner.r#ref
        ),
        &owner.r#ref,
    )
    .unwrap();
    let compiled = compile_inline(
        &definition.r#ref,
        SchemaContext::SingleValue,
        &definition.schema,
    )
    .unwrap();
    let mut transaction = db.begin().await.unwrap();
    let row = SchemaTypeRepository::upsert(&mut transaction, owner.id, &definition, &compiled)
        .await
        .unwrap();
    let release = PackReleaseRepository::create_or_get(
        &mut transaction,
        CreatePackReleaseInput {
            pack: owner.id,
            pack_ref: owner.r#ref.clone(),
            version: "1.0.0".into(),
            digest: "b".repeat(64),
            object_key: "owned/consumer-types".into(),
            provider_version: "1".into(),
            content_path: "owned/consumer-types".into(),
            archive_size: 1,
            manifest: json!({"dependencies": {}}),
        },
    )
    .await
    .unwrap();
    PackReleaseRepository::activate_projected(
        &mut transaction,
        owner.id,
        release.id,
        &PackProjectionIds {
            schema_types: vec![row.id],
            ..Default::default()
        },
    )
    .await
    .unwrap();
    transaction.commit().await.unwrap();

    let schema = json!({"value": {"type": definition.r#ref, "default": "valid"}});
    assert!(ActionFixture::new(consumer.id, &consumer.r#ref, "denied")
        .with_param_schema(schema.clone())
        .create(&db)
        .await
        .is_err());
    PackRepository::update(
        &db,
        consumer.id,
        UpdatePackInput {
            dependencies: Some(vec![format!("{}@^1", owner.r#ref)]),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let action = ActionFixture::new(consumer.id, &consumer.r#ref, "named")
        .with_param_schema(schema.clone())
        .create(&db)
        .await
        .unwrap();
    for invalid in [
        json!({"value": {"type": definition.r#ref, "default": "ab"}}),
        json!({"value": {"type": definition.r#ref, "secret": true, "default": "private-consumer-fixture"}}),
        json!({"value": {"type": "unavailable.value"}}),
    ] {
        let error = ActionRepository::update(
            &db,
            action.id,
            UpdateActionInput {
                param_schema: Some(invalid),
                ..Default::default()
            },
        )
        .await
        .unwrap_err();
        assert!(!error.to_string().contains("private-consumer-fixture"));
        assert_eq!(
            ActionRepository::get_by_id(&db, action.id)
                .await
                .unwrap()
                .param_schema
                .as_ref(),
            Some(&schema)
        );
    }
    let mut transaction = db.begin().await.unwrap();
    assert!(
        SchemaTypeRepository::protect_omissions(&mut transaction, owner.id, &[])
            .await
            .is_err()
    );
    transaction.rollback().await.unwrap();
    ActionRepository::update(
        &db,
        action.id,
        UpdateActionInput {
            param_schema: Some(json!({"value": {"type": "string"}})),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let mut transaction = db.begin().await.unwrap();
    SchemaTypeRepository::protect_omissions(&mut transaction, owner.id, &[])
        .await
        .unwrap();
    transaction.rollback().await.unwrap();
    let unowned = TriggerFixture::new_unique(None, None, "public_named_consumer")
        .with_out_schema(json!({"value": {"type": definition.r#ref}}))
        .create(&db)
        .await
        .unwrap();
    let mut transaction = db.begin().await.unwrap();
    let (_, event_binding) =
        attune_common::repositories::event_schema::EventSchemaRepository::materialize_current(
            &mut transaction,
            unowned.id,
            false,
        )
        .await
        .unwrap();
    assert_eq!(event_binding.payload.unwrap().dependencies.len(), 1);
    assert!(
        SchemaTypeRepository::protect_omissions(&mut transaction, owner.id, &[])
            .await
            .is_err()
    );
    let mut private = definition.clone();
    private.reference_visibility = attune_common::models::ActionReferenceVisibility::Private;
    assert!(
        SchemaTypeRepository::upsert(&mut transaction, owner.id, &private, &compiled)
            .await
            .is_err()
    );
    transaction.rollback().await.unwrap();
    assert!(TriggerRepository::delete(&db, unowned.id).await.unwrap());
    let mut transaction = db.begin().await.unwrap();
    SchemaTypeRepository::protect_omissions(&mut transaction, owner.id, &[])
        .await
        .unwrap();
    SchemaTypeRepository::upsert(&mut transaction, owner.id, &private, &compiled)
        .await
        .unwrap();
    transaction.commit().await.unwrap();
    assert!(
        TriggerFixture::new_unique(None, None, "private_named_consumer")
            .with_out_schema(json!({"value": {"type": definition.r#ref}}))
            .create(&db)
            .await
            .is_err()
    );
    let mut transaction = db.begin().await.unwrap();
    SchemaTypeRepository::upsert(&mut transaction, owner.id, &definition, &compiled)
        .await
        .unwrap();
    transaction.commit().await.unwrap();
    let deleted = ActionFixture::new(consumer.id, &consumer.r#ref, "deleted_named")
        .with_param_schema(schema.clone())
        .create(&db)
        .await
        .unwrap();
    let plain = json!({"value": {"type": "string"}});
    let (input, output) = tokio::join!(
        ActionRepository::update(
            &db,
            deleted.id,
            UpdateActionInput {
                param_schema: Some(plain.clone()),
                ..Default::default()
            }
        ),
        ActionRepository::update(
            &db,
            deleted.id,
            UpdateActionInput {
                out_schema: Some(schema.clone()),
                ..Default::default()
            }
        ),
    );
    input.unwrap();
    output.unwrap();
    let stored = ActionRepository::get_by_id(&db, deleted.id).await.unwrap();
    assert_eq!(stored.param_schema, Some(plain));
    assert_eq!(stored.out_schema, Some(schema));
    let mut transaction = db.begin().await.unwrap();
    assert!(
        SchemaTypeRepository::protect_omissions(&mut transaction, owner.id, &[])
            .await
            .is_err()
    );
    transaction.rollback().await.unwrap();
    assert!(ActionRepository::delete(&db, deleted.id).await.unwrap());
    let mut transaction = db.begin().await.unwrap();
    SchemaTypeRepository::protect_omissions(&mut transaction, owner.id, &[])
        .await
        .unwrap();
    transaction.rollback().await.unwrap();
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn graph_only_workflow_schema_validation_precedes_mutation() {
    let db = create_test_pool().await.unwrap();
    let pack = PackFixture::new_unique("graph_consumer")
        .create(&db)
        .await
        .unwrap();
    let input = CreateWorkflowDefinitionInput {
        r#ref: format!("{}.run", pack.r#ref),
        pack: pack.id,
        pack_ref: pack.r#ref,
        label: "Graph schema".into(),
        description: None,
        version: "1.0.0".into(),
        param_schema: None,
        out_schema: None,
        tags: vec![],
        definition: json!({"parameters": {"value": {"type": "integer", "default": "invalid"}}, "tasks": []}),
    };
    assert!(WorkflowDefinitionRepository::create(&db, input.clone())
        .await
        .is_err());
    use attune_common::repositories::FindByRef;
    assert!(WorkflowDefinitionRepository::find_by_ref(&db, &input.r#ref)
        .await
        .unwrap()
        .is_none());
    let mut valid = input;
    valid.definition =
        json!({"parameters": {"value": {"type": "integer", "default": 0}}, "tasks": []});
    WorkflowDefinitionRepository::create(&db, valid)
        .await
        .unwrap();
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn schema_consumer_writes_preserve_the_outer_rollback_scope() {
    let db = create_test_pool().await.unwrap();
    let mut connection = db.acquire().await.unwrap();
    <<sqlx::Postgres as sqlx::Database>::TransactionManager as sqlx::TransactionManager>::begin(
        &mut *connection,
        None,
    )
    .await
    .unwrap();
    let pack = PackFixture::new_unique("consumer_rollback")
        .create(&mut *connection)
        .await
        .unwrap();
    ActionFixture::new(pack.id, &pack.r#ref, "run")
        .create(&mut *connection)
        .await
        .unwrap();
    <<sqlx::Postgres as sqlx::Database>::TransactionManager as sqlx::TransactionManager>::rollback(
        &mut *connection,
    )
    .await
    .unwrap();
    assert!(PackRepository::find_by_id(&mut *connection, pack.id)
        .await
        .unwrap()
        .is_none());
    drop(connection);
    db.cleanup().await.unwrap();
}
