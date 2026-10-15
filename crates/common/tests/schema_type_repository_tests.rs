//! Disposable repository-owned fixtures for the named type storage foundation.
mod helpers;

use attune_common::{
    models::AbsentMetadataPolicy,
    repositories::{
        component_lifecycle::{ComponentLifecycleRepository, PackProjectionIds},
        pack_release::{CreatePackReleaseInput, PackReleaseRepository},
        schema_type::SchemaTypeRepository,
    },
    schema_type::SchemaTypeDefinition,
    value_schema::{compile_inline, SchemaContext},
};
use helpers::{create_test_pool, PackFixture};
use serde_json::json;

#[tokio::test]
async fn consumer_edges_block_retirement_and_policy_restriction_until_replaced() {
    let db = create_test_pool().await.unwrap();
    let owner = PackFixture::new_unique("edge_owner")
        .create(&db)
        .await
        .unwrap();
    let first = PackFixture::new_unique("edge_first")
        .create(&db)
        .await
        .unwrap();
    let second = PackFixture::new_unique("edge_second")
        .create(&db)
        .await
        .unwrap();
    let mut definition = SchemaTypeDefinition::parse(
        &format!(
            "ref: {}.value\nlabel: Value\nreference_visibility: public\nschema:\n  type: string\n",
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
    let schema = json!({"value":{"type":definition.r#ref}});
    let mut tx = db.begin().await.unwrap();
    SchemaTypeRepository::upsert(&mut tx, owner.id, &definition, &compiled)
        .await
        .unwrap();
    for pack in [&first, &second] {
        SchemaTypeRepository::replace_consumer_references(
            &mut tx,
            pack.id,
            "pack",
            &pack.r#ref,
            &[("conf_schema", Some(&schema))],
        )
        .await
        .unwrap();
    }
    assert!(
        SchemaTypeRepository::protect_omissions(&mut tx, owner.id, &[])
            .await
            .is_err()
    );
    definition.reference_visibility = attune_common::models::ActionReferenceVisibility::Private;
    assert!(
        SchemaTypeRepository::upsert(&mut tx, owner.id, &definition, &compiled)
            .await
            .is_err()
    );
    SchemaTypeRepository::replace_consumer_references(
        &mut tx,
        first.id,
        "pack",
        &first.r#ref,
        &[("conf_schema", None)],
    )
    .await
    .unwrap();
    assert!(SchemaTypeRepository::replace_consumer_references(
        &mut tx,
        second.id,
        "pack",
        &second.r#ref,
        &[("conf_schema", None), ("unsupported", None)]
    )
    .await
    .is_err());
    assert!(
        SchemaTypeRepository::protect_omissions(&mut tx, owner.id, &[])
            .await
            .is_err()
    );
    assert!(
        SchemaTypeRepository::upsert(&mut tx, owner.id, &definition, &compiled)
            .await
            .is_err()
    );
    SchemaTypeRepository::replace_consumer_references(
        &mut tx,
        second.id,
        "pack",
        &second.r#ref,
        &[("conf_schema", None)],
    )
    .await
    .unwrap();
    SchemaTypeRepository::protect_omissions(&mut tx, owner.id, &[])
        .await
        .unwrap();
    SchemaTypeRepository::upsert(&mut tx, owner.id, &definition, &compiled)
        .await
        .unwrap();
    tx.rollback().await.unwrap();
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn installed_private_types_are_discoverable_and_assignable_without_pack_dependencies() {
    use attune_common::{
        repositories::schema_contract::SchemaContractRepository,
        value_schema::{CompiledContract, SourceIdentity},
    };
    let db = create_test_pool().await.unwrap();
    let pack = PackFixture::new_unique("private-type")
        .create(&db)
        .await
        .unwrap();
    let definition = SchemaTypeDefinition::parse(&format!("ref: {}.value\nlabel: Private value\nreference_visibility: private\nschema:\n  type: string\n", pack.r#ref), &pack.r#ref).unwrap();
    let compiled = compile_inline(
        &definition.r#ref,
        SchemaContext::SingleValue,
        &definition.schema,
    )
    .unwrap();
    let mut transaction = db.begin().await.unwrap();
    let row = SchemaTypeRepository::upsert(&mut transaction, pack.id, &definition, &compiled)
        .await
        .unwrap();
    let release = PackReleaseRepository::create_or_get(
        &mut transaction,
        CreatePackReleaseInput {
            pack: pack.id,
            pack_ref: pack.r#ref.clone(),
            version: "1.0.0".into(),
            digest: "c".repeat(64),
            object_key: "owned/private-type".into(),
            provider_version: "1".into(),
            content_path: "owned/private-type".into(),
            archive_size: 1,
            manifest: json!({"dependencies": {}}),
        },
    )
    .await
    .unwrap();
    PackReleaseRepository::activate_projected(
        &mut transaction,
        pack.id,
        release.id,
        &PackProjectionIds {
            schema_types: vec![row.id],
            ..Default::default()
        },
    )
    .await
    .unwrap();
    transaction.commit().await.unwrap();
    assert!(SchemaTypeRepository::find_installed(&db, &definition.r#ref)
        .await
        .unwrap()
        .is_some());
    let (rows, total) = SchemaTypeRepository::list_installed(
        &mut *db.acquire().await.unwrap(),
        Some(&pack.r#ref),
        Some("private value"),
        10,
        0,
    )
    .await
    .unwrap();
    assert_eq!(total, 1);
    assert_eq!(rows[0].r#ref, definition.r#ref);
    let mut transaction = db.begin().await.unwrap();
    let assigned = SchemaContractRepository::materialize_assignment(
        &mut transaction,
        SourceIdentity::Assigned {
            component_ref: "system.typed".into(),
            assignment_id: uuid::Uuid::new_v4(),
        },
        SchemaContext::SingleValue,
        &json!({"type": definition.r#ref}),
    )
    .await
    .unwrap();
    assert_eq!(assigned.dependencies.len(), 1);
    assert_eq!(
        assigned.dependencies[0].source.component_ref(),
        definition.r#ref
    );
    CompiledContract::new(&assigned)
        .unwrap()
        .validate(&json!("public schema, protected value"))
        .unwrap();
    assert!(CompiledContract::new(&assigned)
        .unwrap()
        .validate(&json!(42))
        .is_err());
    let unrelated = PackFixture::new_unique("owned-assignment")
        .create(&db)
        .await
        .unwrap();
    let error = SchemaContractRepository::compile_owned_assignment(
        &mut transaction,
        SourceIdentity::Assigned {
            component_ref: unrelated.r#ref.clone(),
            assignment_id: uuid::Uuid::new_v4(),
        },
        SchemaContext::FlatDocument,
        &json!({"value":{"type":definition.r#ref}}),
        &unrelated,
    )
    .await
    .unwrap_err();
    assert!(error.to_string().contains("reference policy"));
    transaction.rollback().await.unwrap();
    db.cleanup().await.unwrap();
}

#[tokio::test]
async fn external_preflight_requires_release_policy_dependency_and_preserves_reverse_edges() {
    use attune_common::pack_registry::schema_types::preflight;
    use attune_common::repositories::pack::PackRepository;
    let db = create_test_pool().await.unwrap();
    let provider = PackFixture::new("schema-provider")
        .create(&db)
        .await
        .unwrap();
    let mut fixture = PackFixture::new("schema-consumer");
    fixture.dependencies = vec!["schema-provider@^1".into()];
    let consumer = fixture.create(&db).await.unwrap();
    let definition = SchemaTypeDefinition::parse(
        "ref: schema-provider.value\nlabel: Value\nreference_visibility: public\nschema:\n  type: string\n", "schema-provider").unwrap();
    let contract = compile_inline(
        &definition.r#ref,
        SchemaContext::SingleValue,
        &definition.schema,
    )
    .unwrap();
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join("types")).unwrap();
    std::fs::write(
        root.path().join("types/value.yaml"),
        "ref: schema-consumer.value\nlabel: Value\nschema:\n  type: schema-provider.value\n",
    )
    .unwrap();
    let mut tx = db.begin().await.unwrap();
    assert!(preflight(
        &mut tx,
        consumer.id,
        &consumer.r#ref,
        root.path(),
        AbsentMetadataPolicy::Remove
    )
    .await
    .is_err());
    tx.rollback().await.unwrap();

    let mut tx = db.begin().await.unwrap();
    let provider_row = SchemaTypeRepository::upsert(&mut tx, provider.id, &definition, &contract)
        .await
        .unwrap();
    let release = PackReleaseRepository::create_or_get(
        &mut tx,
        CreatePackReleaseInput {
            pack: provider.id,
            pack_ref: provider.r#ref.clone(),
            version: "1.0.0".into(),
            digest: "d".repeat(64),
            object_key: "owned/schema-provider".into(),
            provider_version: "1".into(),
            content_path: "owned/schema-provider".into(),
            archive_size: 1,
            manifest: json!({"dependencies": {}}),
        },
    )
    .await
    .unwrap();
    PackReleaseRepository::activate_projected(
        &mut tx,
        provider.id,
        release.id,
        &PackProjectionIds {
            schema_types: vec![provider_row.id],
            ..Default::default()
        },
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();

    let mut tx = db.begin().await.unwrap();
    let plan = preflight(
        &mut tx,
        consumer.id,
        &consumer.r#ref,
        root.path(),
        AbsentMetadataPolicy::Remove,
    )
    .await
    .unwrap();
    let consumer_definition = &plan.declared["schema-consumer.value"];
    let consumer_contract = &plan.contracts["schema-consumer.value"];
    SchemaTypeRepository::upsert(&mut tx, consumer.id, consumer_definition, consumer_contract)
        .await
        .unwrap();
    SchemaTypeRepository::refresh_type_references(&mut tx, consumer.id)
        .await
        .unwrap();
    let consumer_release = PackReleaseRepository::create_or_get(
        &mut tx,
        CreatePackReleaseInput {
            pack: consumer.id,
            pack_ref: consumer.r#ref.clone(),
            version: "1.0.0".into(),
            digest: "e".repeat(64),
            object_key: "owned/schema-consumer".into(),
            provider_version: "1".into(),
            content_path: "owned/schema-consumer".into(),
            archive_size: 1,
            manifest: json!({"dependencies": {"schema-provider": "^1"}}),
        },
    )
    .await
    .unwrap();
    SchemaTypeRepository::capture_release(&mut tx, consumer.id, consumer_release.id)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let mut tx = db.begin().await.unwrap();
    let runtime =
        attune_common::repositories::schema_contract::SchemaContractRepository::materialize(
            &mut tx,
            attune_common::value_schema::SourceIdentity::Released {
            // Actions and types have separate namespaces and may share a ref.
            component_ref: "schema-consumer.value".into(),
                release_id: consumer_release.id,
                release_digest: consumer_release.digest.clone(),
            },
            SchemaContext::FlatDocument,
        &json!({"value": {"type": "schema-consumer.value", "required": true}, "again": {"type": "schema-consumer.value"}}),
        )
        .await
        .unwrap();
    let prepared = attune_common::value_schema::CompiledContract::new(&runtime).unwrap();
    assert!(prepared.validate(&json!({"value": "valid"})).is_ok());
    assert!(prepared.validate(&json!({"value": 12})).is_err());
    tx.rollback().await.unwrap();
    let mut tx = db.begin().await.unwrap();
    assert!(
        SchemaTypeRepository::protect_omissions(&mut tx, provider.id, &[])
            .await
            .is_err()
    );
    tx.rollback().await.unwrap();
    assert!(PackRepository::delete(&*db, provider.id).await.is_err());
    let mut private = definition.clone();
    private.reference_visibility = attune_common::models::ActionReferenceVisibility::Private;
    let mut tx = db.begin().await.unwrap();
    assert!(
        SchemaTypeRepository::upsert(&mut tx, provider.id, &private, &contract)
            .await
            .is_err()
    );
    tx.rollback().await.unwrap();

    let mut tx = db.begin().await.unwrap();
    ComponentLifecycleRepository::reconcile_omissions_with_policy(
        &mut tx,
        consumer.id,
        &PackProjectionIds::default(),
        AbsentMetadataPolicy::Remove,
    )
    .await
    .unwrap();
    SchemaTypeRepository::protect_omissions(&mut tx, provider.id, &[])
        .await
        .unwrap();
    tx.rollback().await.unwrap();
}

#[tokio::test]
async fn revisions_are_immutable_and_retirement_does_not_delete_them() {
    let db = create_test_pool().await.unwrap();
    let pack = PackFixture::new("schema-test").create(&db).await.unwrap();
    let definition = SchemaTypeDefinition::parse(
        "ref: schema-test.value\nlabel: Value\nschema:\n  type: string\n",
        "schema-test",
    )
    .unwrap();
    let contract = compile_inline(
        &definition.r#ref,
        SchemaContext::SingleValue,
        &definition.schema,
    )
    .unwrap();
    let mut tx = db.begin().await.unwrap();
    let row = SchemaTypeRepository::upsert(&mut tx, pack.id, &definition, &contract)
        .await
        .unwrap();
    let release = PackReleaseRepository::create_or_get(
        &mut tx,
        CreatePackReleaseInput {
            pack: pack.id,
            pack_ref: pack.r#ref.clone(),
            version: "1.0.0".into(),
            digest: "a".repeat(64),
            object_key: "owned/schema-test".into(),
            provider_version: "1".into(),
            content_path: "owned/schema-test".into(),
            archive_size: 1,
            manifest: json!({}),
        },
    )
    .await
    .unwrap();
    SchemaTypeRepository::capture_release(&mut tx, pack.id, release.id)
        .await
        .unwrap();
    SchemaTypeRepository::capture_release(&mut tx, pack.id, release.id)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    let revisions = SchemaTypeRepository::list_revisions(&db, release.id)
        .await
        .unwrap();
    assert_eq!(revisions.len(), 1);
    let mut tx = db.begin().await.unwrap();
    let before = SchemaTypeRepository::release_view(&mut tx, release.id, "schema-test")
        .await
        .unwrap();
    assert!(matches!(
        before[&definition.r#ref],
        attune_common::value_schema::RegistryEntry::Available(_)
    ));
    ComponentLifecycleRepository::reconcile_omissions_with_policy(
        &mut tx,
        pack.id,
        &PackProjectionIds::default(),
        AbsentMetadataPolicy::Remove,
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    assert!(SchemaTypeRepository::find_by_ref(&db, &definition.r#ref)
        .await
        .unwrap()
        .unwrap()
        .retired_at
        .is_some());
    assert_eq!(
        SchemaTypeRepository::list_revisions(&db, release.id)
            .await
            .unwrap()
            .len(),
        1
    );
    let mut tx = db.begin().await.unwrap();
    let retired = SchemaTypeRepository::release_view(&mut tx, release.id, "schema-test")
        .await
        .unwrap();
    assert!(matches!(
        retired[&definition.r#ref],
        attune_common::value_schema::RegistryEntry::Retired
    ));
    tx.rollback().await.unwrap();

    let mut tx = db.begin().await.unwrap();
    let error = sqlx::query("UPDATE schema_type_revision SET content_digest = $2 WHERE id = $1")
        .bind(revisions[0].id)
        .bind("b".repeat(64))
        .execute(&mut *tx)
        .await
        .unwrap_err();
    assert!(error.to_string().contains("immutable"));
    tx.rollback().await.unwrap();
    assert_eq!(row.pack, pack.id);
}

#[tokio::test]
async fn ownership_contract_mismatch_and_changed_release_views_fail_closed() {
    let db = create_test_pool().await.unwrap();
    let pack = PackFixture::new("schema-owner").create(&db).await.unwrap();
    let other = PackFixture::new("schema-other").create(&db).await.unwrap();
    let mut definition = SchemaTypeDefinition::parse(
        "ref: schema-owner.value\nlabel: Value\nschema:\n  type: integer\n",
        "schema-owner",
    )
    .unwrap();
    let contract = compile_inline(
        &definition.r#ref,
        SchemaContext::SingleValue,
        &definition.schema,
    )
    .unwrap();
    let mut tx = db.begin().await.unwrap();
    assert!(
        SchemaTypeRepository::upsert(&mut tx, other.id, &definition, &contract)
            .await
            .is_err()
    );
    tx.rollback().await.unwrap();
    let mut tx = db.begin().await.unwrap();
    let row = SchemaTypeRepository::upsert(&mut tx, pack.id, &definition, &contract)
        .await
        .unwrap();
    let release = PackReleaseRepository::create_or_get(
        &mut tx,
        CreatePackReleaseInput {
            pack: pack.id,
            pack_ref: pack.r#ref.clone(),
            version: "1.0.0".into(),
            digest: "c".repeat(64),
            object_key: "owned/schema-owner".into(),
            provider_version: "1".into(),
            content_path: "owned/schema-owner".into(),
            archive_size: 1,
            manifest: json!({}),
        },
    )
    .await
    .unwrap();
    SchemaTypeRepository::capture_release(&mut tx, pack.id, release.id)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    definition.schema = json!({"type": "string"});
    let mut tx = db.begin().await.unwrap();
    assert!(
        SchemaTypeRepository::upsert(&mut tx, pack.id, &definition, &contract)
            .await
            .is_err()
    );
    tx.rollback().await.unwrap();
    let changed = compile_inline(
        &definition.r#ref,
        SchemaContext::SingleValue,
        &definition.schema,
    )
    .unwrap();
    let mut tx = db.begin().await.unwrap();
    SchemaTypeRepository::upsert(&mut tx, pack.id, &definition, &changed)
        .await
        .unwrap();
    assert!(
        SchemaTypeRepository::capture_release(&mut tx, pack.id, release.id)
            .await
            .is_err()
    );
    tx.rollback().await.unwrap();
    let stored = SchemaTypeRepository::find_by_ref(&db, &definition.r#ref)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.id, row.id);
    assert_eq!(stored.schema, json!({"type": "integer"}));
}
