mod helpers;

use attune_common::{
    delegation::DelegationAuthority,
    rbac::Grant,
    repositories::{
        component_configuration::{
            ComponentConfigurationRepository, ENTITY_COMPONENT_CONFIGURATION,
        },
        execution_secret_value::ExecutionSecretValueRepository,
        pack::{PackRepository, UpdatePackInput},
        FindById,
    },
};
use helpers::{create_test_pool, PackFixture};
use serde_json::json;

const KEY: &str = "configuration-test-encryption-key";

#[tokio::test]
async fn equivalent_deployments_preserve_ciphertext_without_decrypt_but_schema_migration_requires_it(
) {
    let pool = create_test_pool().await.unwrap();
    let pack = PackFixture::new_unique("config_auth_migration")
        .create(&pool)
        .await
        .unwrap();
    let mut tx = pool.begin().await.unwrap();
    ComponentConfigurationRepository::lock_selection(&mut tx)
        .await
        .unwrap();
    let pack = PackRepository::update(
        &mut *tx,
        pack.id,
        UpdatePackInput {
            conf_schema: Some(json!({"token":{"type":"string","secret":true}})),
            config: Some(json!({"token":"migration-auth-fixture"})),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let original =
        ComponentConfigurationRepository::bind_pack(&mut tx, &pack, Some(&pack.config), Some(KEY))
            .await
            .unwrap();
    let limited = DelegationAuthority::new(
        7,
        Default::default(),
        serde_json::from_value::<Vec<Grant>>(json!([
            {"resource":"packs","actions":["read","configure"],"constraints":{"refs":[pack.r#ref]}}
        ]))
        .unwrap(),
    );
    ComponentConfigurationRepository::authorize_preserved_migration(
        &mut tx,
        "pack",
        pack.id,
        &pack.r#ref,
        &pack,
        &pack.conf_schema,
        &limited,
    )
    .await
    .unwrap();
    let unchanged = ComponentConfigurationRepository::bind_pack(&mut tx, &pack, None, None)
        .await
        .unwrap();
    assert_eq!(unchanged.id, original.id);
    assert_eq!(
        PackRepository::find_by_id(&mut *tx, pack.id)
            .await
            .unwrap()
            .unwrap()
            .config,
        original.config
    );
    let changed_schema = json!({"token":{"type":"string","secret":true,"minLength":2}});
    assert!(
        ComponentConfigurationRepository::authorize_preserved_migration(
            &mut tx,
            "pack",
            pack.id,
            &pack.r#ref,
            &pack,
            &changed_schema,
            &limited
        )
        .await
        .is_err()
    );
    let allowed = DelegationAuthority::new(
        7,
        Default::default(),
        serde_json::from_value::<Vec<Grant>>(json!([
            {"resource":"packs","actions":["read","decrypt"],"constraints":{"refs":[pack.r#ref]}}
        ]))
        .unwrap(),
    );
    ComponentConfigurationRepository::authorize_preserved_migration(
        &mut tx,
        "pack",
        pack.id,
        &pack.r#ref,
        &pack,
        &changed_schema,
        &allowed,
    )
    .await
    .unwrap();
    tx.rollback().await.unwrap();
    pool.cleanup().await.unwrap();
}

#[tokio::test]
async fn retired_configuration_sources_are_retained_by_normalized_secret_origins() {
    let pool = create_test_pool().await.unwrap();
    let pack = PackFixture::new_unique("config_pins")
        .create(&pool)
        .await
        .unwrap();
    let mut tx = pool.begin().await.unwrap();
    ComponentConfigurationRepository::lock_selection(&mut tx)
        .await
        .unwrap();
    let current =
        ComponentConfigurationRepository::bind_pack(&mut tx, &pack, Some(&pack.config), Some(KEY))
            .await
            .unwrap();
    let old_id: i64 = sqlx::query_scalar(
        "INSERT INTO component_configuration_revision \
        (owner_kind,owner_id,owner_ref,pack_ref,contract,binding_digest,config,created) \
        VALUES ('pack',$1,$2,$2,$3,$4,$5,NOW()-INTERVAL '2 hours') RETURNING id",
    )
    .bind(pack.id)
    .bind(&pack.r#ref)
    .bind(&current.contract)
    .bind(&current.binding_digest)
    .bind(&current.config)
    .fetch_one(&mut *tx)
    .await
    .unwrap();
    let provenance = attune_common::secret_provenance::SecretProvenance {
        origins: vec![attune_common::secret_provenance::SecretOrigin::Entity {
            entity_type: ENTITY_COMPONENT_CONFIGURATION.into(),
            entity_id: old_id,
            path: "/value".into(),
        }],
        templates: vec![],
    };
    let secrets = attune_common::secret_values::prepare_secret_values(
        vec![attune_common::secret_values::SecretValueInput {
            json_path: "/value".into(),
            value: json!("retention-fixture"),
            source_kind: "provenance".into(),
            source_ref: Some(serde_json::to_string(&provenance).unwrap()),
        }],
        KEY,
    )
    .unwrap();
    ExecutionSecretValueRepository::upsert_many_with_conn(
        &mut tx,
        "configuration_retention_fixture",
        pack.id,
        &secrets,
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(
        ComponentConfigurationRepository::cleanup_retired(&pool, 4)
            .await
            .unwrap(),
        (0, 0)
    );
    assert!(ComponentConfigurationRepository::find(&*pool, old_id)
        .await
        .unwrap()
        .is_some());
    let mut connection = pool.acquire().await.unwrap();
    ExecutionSecretValueRepository::replace_entity_with_conn(
        &mut connection,
        "configuration_retention_fixture",
        pack.id,
        &[],
    )
    .await
    .unwrap();
    drop(connection);
    assert_eq!(
        ComponentConfigurationRepository::cleanup_retired(&pool, 4)
            .await
            .unwrap(),
        (0, 1)
    );
    assert!(ComponentConfigurationRepository::find(&*pool, old_id)
        .await
        .unwrap()
        .is_none());
    assert!(ComponentConfigurationRepository::find(&*pool, current.id)
        .await
        .unwrap()
        .is_some());
}

#[tokio::test]
async fn configuration_revisions_encrypt_secrets_and_require_pack_read_decrypt_for_delivery() {
    let pool = create_test_pool().await.unwrap();
    let pack = PackFixture::new_unique("bound_config")
        .create(&pool)
        .await
        .unwrap();
    let mut tx = pool.begin().await.unwrap();
    ComponentConfigurationRepository::lock_selection(&mut tx)
        .await
        .unwrap();
    let pack = PackRepository::update(&mut *tx, pack.id, UpdatePackInput {
        conf_schema:Some(json!({"token":{"type":"string","secret":true},"retries":{"type":"integer","default":0}})),
        config:Some(json!({"token":"private-configuration"})), ..Default::default()
    }).await.unwrap();
    let revision =
        ComponentConfigurationRepository::bind_pack(&mut tx, &pack, Some(&pack.config), Some(KEY))
            .await
            .unwrap();
    tx.commit().await.unwrap();
    assert!(!revision
        .config
        .to_string()
        .contains("private-configuration"));
    let current = PackRepository::find_by_id(&*pool, pack.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(current.config, revision.config);
    assert_eq!(current.config["retries"], 0);
    let stored = ExecutionSecretValueRepository::find_stored_by_entity(
        &*pool,
        ENTITY_COMPONENT_CONFIGURATION,
        revision.id,
    )
    .await
    .unwrap();
    assert_eq!(stored.len(), 1);
    let denied = DelegationAuthority::new(
        7,
        Default::default(),
        serde_json::from_value::<Vec<Grant>>(json!([
            {"resource":"packs","actions":["read","configure"]}
        ]))
        .unwrap(),
    );
    let allowed = DelegationAuthority::new(
        7,
        Default::default(),
        serde_json::from_value::<Vec<Grant>>(json!([
            {"resource":"packs","actions":["read","decrypt"],"constraints":{"refs":[pack.r#ref]}}
        ]))
        .unwrap(),
    );
    let mut connection = pool.acquire().await.unwrap();
    let (redacted, _, _) = ComponentConfigurationRepository::pack_template_values(
        &mut connection,
        &current,
        Some(&denied),
        Some(KEY),
    )
    .await
    .unwrap();
    assert!(!redacted.to_string().contains("private-configuration"));
    assert_eq!(redacted["retries"], 0);
    let (value, paths, id) = ComponentConfigurationRepository::pack_template_values(
        &mut connection,
        &current,
        Some(&allowed),
        Some(KEY),
    )
    .await
    .unwrap();
    assert_eq!(value["token"], "private-configuration");
    assert_eq!(paths, ["/token"]);
    assert_eq!(id, Some(revision.id));
    let error =
        sqlx::query("UPDATE component_configuration_revision SET config = '{}' WHERE id = $1")
            .bind(revision.id)
            .execute(&mut *connection)
            .await
            .unwrap_err();
    assert!(error.to_string().contains("immutable"));
}

#[tokio::test]
async fn configuration_schema_migration_defaults_preserved_values_and_rolls_back_incompatibility() {
    let pool = create_test_pool().await.unwrap();
    let pack = PackFixture::new_unique("config_migration")
        .create(&pool)
        .await
        .unwrap();
    let mut tx = pool.begin().await.unwrap();
    ComponentConfigurationRepository::lock_selection(&mut tx)
        .await
        .unwrap();
    let pack = PackRepository::update(
        &mut *tx,
        pack.id,
        UpdatePackInput {
            conf_schema: Some(
                json!({"token":{"type":"string","secret":true},"enabled":{"type":"boolean"}}),
            ),
            config: Some(json!({"token":"private-preserved-value","enabled":false})),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let original =
        ComponentConfigurationRepository::bind_pack(&mut tx, &pack, Some(&pack.config), Some(KEY))
            .await
            .unwrap();
    tx.commit().await.unwrap();
    let mut tx = pool.begin().await.unwrap();
    ComponentConfigurationRepository::lock_selection(&mut tx)
        .await
        .unwrap();
    let pack = PackRepository::update(&mut *tx, pack.id, UpdatePackInput {
        conf_schema:Some(json!({"token":{"type":"string","secret":true},"enabled":{"type":"boolean","default":true},
            "nested":{"type":"object","properties":{"value":{"type":"integer","default":1}}},
            "count":{"type":"integer","default":0}})), ..Default::default()
    }).await.unwrap();
    let migrated = ComponentConfigurationRepository::bind_pack(&mut tx, &pack, None, Some(KEY))
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert_ne!(migrated.id, original.id);
    let mut connection = pool.acquire().await.unwrap();
    let value = ComponentConfigurationRepository::restore(&mut connection, &migrated, Some(KEY))
        .await
        .unwrap();
    assert_eq!(
        value,
        json!({"token":"private-preserved-value","enabled":false,"count":0})
    );
    assert_eq!(
        ComponentConfigurationRepository::restore(&mut connection, &original, Some(KEY))
            .await
            .unwrap()["token"],
        "private-preserved-value"
    );
    drop(connection);
    let mut tx = pool.begin().await.unwrap();
    ComponentConfigurationRepository::lock_selection(&mut tx)
        .await
        .unwrap();
    let invalid = PackRepository::update(
        &mut *tx,
        pack.id,
        UpdatePackInput {
            conf_schema: Some(json!({"token":{"type":"string","secret":true,"minLength":100}})),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let error = ComponentConfigurationRepository::bind_pack(&mut tx, &invalid, None, Some(KEY))
        .await
        .unwrap_err();
    assert!(!error.to_string().contains("private-preserved-value"));
    tx.rollback().await.unwrap();
    let current = ComponentConfigurationRepository::current(&*pool, "pack", pack.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(current.id, migrated.id);
    assert_eq!(
        PackRepository::find_by_id(&*pool, pack.id)
            .await
            .unwrap()
            .unwrap()
            .conf_schema,
        pack.conf_schema
    );
}
