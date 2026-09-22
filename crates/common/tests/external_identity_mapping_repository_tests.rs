mod helpers;

use attune_common::repositories::external_identity_mapping::{
    CreateExternalIdentityMappingInput, ExternalIdentityMappingRepository,
    UpdateExternalIdentityMappingInput,
};
use attune_common::repositories::identity::{CreateIdentityInput, IdentityRepository};
use attune_common::repositories::{Create, Update};
use attune_common::Error;
use helpers::{create_test_pool, unique_pack_ref};
use serde_json::json;

async fn create_identity(
    pool: &sqlx::PgPool,
    prefix: &str,
) -> attune_common::models::identity::Identity {
    IdentityRepository::create(
        pool,
        CreateIdentityInput {
            login: unique_pack_ref(prefix),
            display_name: None,
            password_hash: None,
            attributes: json!({}),
        },
    )
    .await
    .unwrap()
}

#[tokio::test]
async fn crud_is_scoped_to_the_integration_identity_and_normalizes_keys() {
    let database = create_test_pool().await.unwrap();
    let integration = create_identity(&database, "mapping_integration").await;
    let other_integration = create_identity(&database, "mapping_other_integration").await;
    let mapped = create_identity(&database, "mapping_target").await;
    let replacement = create_identity(&database, "mapping_replacement").await;

    let created = ExternalIdentityMappingRepository::create(
        &database,
        integration.id,
        CreateExternalIdentityMappingInput {
            mapped_identity: mapped.id,
            provider: "  GitHub  ".to_string(),
            tenant: "  Acme  ".to_string(),
            subject_kind: "  User  ".to_string(),
            external_subject: "  User-42  ".to_string(),
            created_by: Some(integration.id),
        },
    )
    .await
    .unwrap();

    assert_eq!(created.provider, "github");
    assert_eq!(created.tenant, "Acme");
    assert_eq!(created.subject_kind, "user");
    assert_eq!(created.external_subject, "User-42");
    assert!(ExternalIdentityMappingRepository::find_by_id(
        &database,
        other_integration.id,
        created.id
    )
    .await
    .unwrap()
    .is_none());
    assert!(
        ExternalIdentityMappingRepository::list(&database, other_integration.id)
            .await
            .unwrap()
            .is_empty()
    );

    let updated = ExternalIdentityMappingRepository::update(
        &database,
        integration.id,
        created.id,
        UpdateExternalIdentityMappingInput {
            mapped_identity: replacement.id,
            provider: " OIDC ".to_string(),
            tenant: " Tenant-A ".to_string(),
            subject_kind: " Service_Account ".to_string(),
            external_subject: " Subject-A ".to_string(),
        },
    )
    .await
    .unwrap();
    assert_eq!(updated.mapped_identity, replacement.id);
    assert_eq!(updated.provider, "oidc");
    assert_eq!(updated.subject_kind, "service_account");
    assert!(!ExternalIdentityMappingRepository::delete(
        &database,
        other_integration.id,
        created.id
    )
    .await
    .unwrap());
    assert!(
        ExternalIdentityMappingRepository::delete(&database, integration.id, created.id)
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn exact_resolution_normalizes_tokens_and_rejects_frozen_identities() {
    let database = create_test_pool().await.unwrap();
    let integration = create_identity(&database, "resolve_integration").await;
    let mapped = create_identity(&database, "resolve_target").await;

    ExternalIdentityMappingRepository::create(
        &database,
        integration.id,
        CreateExternalIdentityMappingInput {
            mapped_identity: mapped.id,
            provider: "GitHub".to_string(),
            tenant: "Acme".to_string(),
            subject_kind: "User".to_string(),
            external_subject: "User-42".to_string(),
            created_by: None,
        },
    )
    .await
    .unwrap();

    let mut transaction = database.begin().await.unwrap();
    let resolved = ExternalIdentityMappingRepository::resolve_exact_for_share(
        &mut *transaction,
        integration.id,
        " GITHUB ",
        "Acme",
        " USER ",
        "User-42",
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(resolved.id, mapped.id);
    assert!(ExternalIdentityMappingRepository::resolve_exact_for_share(
        &mut *transaction,
        integration.id,
        "github",
        "acme",
        "user",
        "User-42",
    )
    .await
    .unwrap()
    .is_none());
    assert!(ExternalIdentityMappingRepository::resolve_exact_for_share(
        &mut *transaction,
        integration.id,
        "github",
        "Acme",
        "group",
        "User-42",
    )
    .await
    .unwrap()
    .is_none());
    transaction.commit().await.unwrap();

    IdentityRepository::update(
        &database,
        mapped.id,
        attune_common::repositories::identity::UpdateIdentityInput {
            frozen: Some(true),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let mut transaction = database.begin().await.unwrap();
    assert!(ExternalIdentityMappingRepository::resolve_exact_for_share(
        &mut *transaction,
        integration.id,
        "github",
        "Acme",
        "user",
        "User-42",
    )
    .await
    .unwrap()
    .is_none());
    transaction.rollback().await.unwrap();
}

#[tokio::test]
async fn duplicate_and_invalid_keys_are_rejected() {
    let database = create_test_pool().await.unwrap();
    let integration = create_identity(&database, "constraint_integration").await;
    let mapped = create_identity(&database, "constraint_target").await;

    let input = || CreateExternalIdentityMappingInput {
        mapped_identity: mapped.id,
        provider: "github".to_string(),
        tenant: "Acme".to_string(),
        subject_kind: "user".to_string(),
        external_subject: "User-42".to_string(),
        created_by: None,
    };
    ExternalIdentityMappingRepository::create(&database, integration.id, input())
        .await
        .unwrap();
    assert!(ExternalIdentityMappingRepository::create(
        &database,
        integration.id,
        CreateExternalIdentityMappingInput {
            subject_kind: " USER ".to_string(),
            ..input()
        },
    )
    .await
    .is_err());
    ExternalIdentityMappingRepository::create(
        &database,
        integration.id,
        CreateExternalIdentityMappingInput {
            tenant: "acme".to_string(),
            ..input()
        },
    )
    .await
    .expect("tenant and subject keys must remain case-sensitive");
    ExternalIdentityMappingRepository::create(
        &database,
        integration.id,
        CreateExternalIdentityMappingInput {
            subject_kind: "group".to_string(),
            ..input()
        },
    )
    .await
    .expect("subject kind must be part of the unique key");

    let error = ExternalIdentityMappingRepository::create(
        &database,
        integration.id,
        CreateExternalIdentityMappingInput {
            provider: "bad provider".to_string(),
            subject_kind: "user".to_string(),
            external_subject: "subject".to_string(),
            tenant: "tenant".to_string(),
            mapped_identity: mapped.id,
            created_by: None,
        },
    )
    .await
    .unwrap_err();
    assert!(matches!(error, Error::Validation(_)));

    let error = ExternalIdentityMappingRepository::create(
        &database,
        integration.id,
        CreateExternalIdentityMappingInput {
            subject_kind: "bad kind".to_string(),
            ..input()
        },
    )
    .await
    .unwrap_err();
    assert!(matches!(error, Error::Validation(_)));
}

#[tokio::test]
async fn inquiry_external_actor_is_limited_to_4096_bytes() {
    let database = create_test_pool().await.unwrap();

    sqlx::query(
        "INSERT INTO inquiry (created_by_execution, prompt, response_options, external_actor) VALUES ($1, $2, $3, $4)",
    )
        .bind(i64::MAX - 1)
        .bind("bounded actor")
        .bind(json!([{"ref": "continue", "label": "Continue", "style": "default", "response": {}}]))
        .bind(json!({"actor": "x".repeat(4080)}))
        .execute(&database)
        .await
        .unwrap();

    let oversized =
        sqlx::query(
            "INSERT INTO inquiry (created_by_execution, prompt, response_options, external_actor) VALUES ($1, $2, $3, $4)",
        )
            .bind(i64::MAX)
            .bind("oversized actor")
            .bind(json!([{"ref": "continue", "label": "Continue", "style": "default", "response": {}}]))
            .bind(json!({"actor": "x".repeat(4096)}))
            .execute(&database)
            .await
            .unwrap_err();
    assert!(matches!(oversized, sqlx::Error::Database(error) if error.is_check_violation()));
}
