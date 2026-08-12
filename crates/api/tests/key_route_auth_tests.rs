use attune_common::{
    auth::jwt::{
        generate_execution_token_with_permission_sets, generate_sensor_token, generate_token,
        JwtConfig, TokenType,
    },
    crypto::{encrypt_json, hash_encryption_key},
    models::OwnerType,
    repositories::{
        identity::{
            CreateIdentityInput, CreatePermissionAssignmentInput, CreatePermissionSetInput,
            IdentityRepository, PermissionAssignmentRepository, PermissionSetRepository,
        },
        key::{CreateKeyInput, KeyRepository},
        Create,
    },
};
use axum::http::StatusCode;
use serde_json::{json, Value};

mod helpers;
use helpers::{Result, TestContext};

const JWT_SECRET: &str = "test-secret-for-testing-only-not-secure";
const ENCRYPTION_KEY: &str = "test-encryption-key-32-chars-okay";

fn jwt_config() -> JwtConfig {
    JwtConfig {
        secret: JWT_SECRET.to_string(),
        access_token_expiration: 3600,
        refresh_token_expiration: 3600,
    }
}

async fn create_encrypted_key(ctx: &TestContext, key_ref: &str, value: Value) -> Result<()> {
    KeyRepository::create(
        &ctx.pool,
        CreateKeyInput {
            r#ref: key_ref.to_string(),
            owner_type: OwnerType::System,
            owner: None,
            owner_identity: None,
            owner_pack: None,
            owner_pack_ref: None,
            owner_action: None,
            owner_action_ref: None,
            owner_sensor: None,
            owner_sensor_ref: None,
            name: key_ref.to_string(),
            encrypted: true,
            encryption_key_hash: Some(hash_encryption_key(ENCRYPTION_KEY)),
            value: encrypt_json(&value, ENCRYPTION_KEY)?,
        },
    )
    .await?;
    Ok(())
}

#[tokio::test]
#[ignore = "integration test - requires database"]
async fn every_key_route_enforces_its_explicit_token_type_contract() -> Result<()> {
    let ctx = TestContext::new().await?;
    let suffix = uuid::Uuid::new_v4().simple().to_string();
    let identity = IdentityRepository::create(
        &ctx.pool,
        CreateIdentityInput {
            login: format!("key_route_auth_{suffix}"),
            display_name: Some("Key route auth test".to_string()),
            attributes: json!({}),
            password_hash: None,
        },
    )
    .await?;

    let permission_ref = format!("test.key_route_auth_{suffix}");
    let permission_set = PermissionSetRepository::create(
        &ctx.pool,
        CreatePermissionSetInput {
            r#ref: permission_ref.clone(),
            pack: None,
            pack_ref: None,
            label: Some("Key route auth".to_string()),
            description: None,
            grants: json!([{
                "resource": "keys",
                "actions": ["read", "create", "update", "delete", "decrypt"]
            }]),
        },
    )
    .await?;
    PermissionAssignmentRepository::create(
        &ctx.pool,
        CreatePermissionAssignmentInput {
            identity: identity.id,
            permset: permission_set.id,
        },
    )
    .await?;

    let config = jwt_config();
    let tokens = [
        (
            TokenType::Access,
            generate_token(identity.id, &identity.login, &config, TokenType::Access)?,
            true,
        ),
        (
            TokenType::Execution,
            generate_execution_token_with_permission_sets(
                identity.id,
                123,
                "test.key_route_auth",
                &config,
                Some(3600),
                std::slice::from_ref(&permission_ref),
            )?,
            true,
        ),
        (
            TokenType::Sensor,
            generate_sensor_token(
                identity.id,
                "sensor:test.key_route_auth",
                vec!["test.event".to_string()],
                &config,
                Some(3600),
            )?,
            false,
        ),
        (
            TokenType::Worker,
            generate_token(identity.id, "worker:test", &config, TokenType::Worker)?,
            false,
        ),
        (
            TokenType::Refresh,
            generate_token(identity.id, &identity.login, &config, TokenType::Refresh)?,
            false,
        ),
    ];

    let get_ref = format!("key_route_get_{suffix}");
    create_encrypted_key(&ctx, &get_ref, json!("route-secret")).await?;

    for (index, (token_type, token, allowed)) in tokens.iter().enumerate() {
        let expected = if *allowed {
            StatusCode::OK
        } else {
            StatusCode::UNAUTHORIZED
        };

        let list = ctx.get("/api/v1/keys", Some(token)).await?;
        assert_eq!(list.status(), expected, "list with {token_type:?}");

        let get = ctx
            .get(&format!("/api/v1/keys/{get_ref}?decrypt=true"), Some(token))
            .await?;
        assert_eq!(get.status(), expected, "get/decrypt with {token_type:?}");
        if *allowed {
            let body: Value = get.json().await?;
            assert_eq!(body["data"]["value"], json!("route-secret"));
        }

        let create_ref = format!("key_route_create_{index}_{token_type:?}_{suffix}").to_lowercase();
        let create = ctx
            .post(
                "/api/v1/keys",
                json!({
                    "ref": create_ref,
                    "owner_type": "system",
                    "name": create_ref,
                    "value": "created-secret",
                    "encrypted": true
                }),
                Some(token),
            )
            .await?;
        let create_expected = if *allowed {
            StatusCode::CREATED
        } else {
            StatusCode::UNAUTHORIZED
        };
        let create_status = create.status();
        let create_body: Value = create.json().await?;
        assert_eq!(
            create_status, create_expected,
            "create with {token_type:?}: {create_body}"
        );

        let update_ref = format!("key_route_update_{index}_{token_type:?}_{suffix}").to_lowercase();
        create_encrypted_key(&ctx, &update_ref, json!("old-secret")).await?;
        let update = ctx
            .put(
                &format!("/api/v1/keys/{update_ref}"),
                json!({"name": update_ref, "value": "new-secret"}),
                Some(token),
            )
            .await?;
        assert_eq!(update.status(), expected, "update with {token_type:?}");
        if *allowed {
            let body: Value = update.json().await?;
            assert!(
                body["data"]["value"].is_null(),
                "encrypted update response exposed value for {token_type:?}"
            );
        }

        let delete_ref = format!("key_route_delete_{index}_{token_type:?}_{suffix}").to_lowercase();
        create_encrypted_key(&ctx, &delete_ref, json!("delete-secret")).await?;
        let delete = ctx
            .delete(&format!("/api/v1/keys/{delete_ref}"), Some(token))
            .await?;
        assert_eq!(delete.status(), expected, "delete with {token_type:?}");
    }

    Ok(())
}
