mod helpers;

use attune_common::{
    auth::jwt::{generate_execution_token_with_permission_sets, validate_token, JwtConfig},
    crypto,
    models::{
        ArtifactClassification, ArtifactType, ArtifactVisibility, OwnerType, RetentionPolicyType,
    },
    repositories::{
        artifact::{
            ArtifactRepository, ArtifactVersionRepository, CreateArtifactInput,
            CreateArtifactVersionInput,
        },
        execution_secret_value::ExecutionSecretValueRepository,
        identity::{
            CreatePermissionAssignmentInput, CreatePermissionSetInput, IdentityRepository,
            PermissionAssignmentRepository, PermissionSetRepository, UpdateIdentityInput,
        },
        key::{CreateKeyInput, KeyRepository},
        pack::{PackRepository, UpdatePackInput},
        Create, FindByRef, Update,
    },
    secret_provenance::{SecretOrigin, SecretProvenance},
    secret_values::{
        prepare_secret_values, redaction_marker, SecretValueInput, ENTITY_EXECUTION_CONFIG,
    },
};
use axum::http::StatusCode;
use helpers::{Result, TestContext};
use serde_json::{json, Value};

fn jwt_config(ctx: &TestContext) -> JwtConfig {
    JwtConfig {
        secret: ctx.state.config.security.jwt_secret.clone().unwrap(),
        access_token_expiration: 300,
        refresh_token_expiration: 3600,
    }
}

async fn grant(ctx: &TestContext, identity: i64, reference: &str, grants: Value) -> Result<()> {
    let set = PermissionSetRepository::create(
        &ctx.pool,
        CreatePermissionSetInput {
            r#ref: reference.into(),
            pack: None,
            pack_ref: None,
            label: None,
            description: None,
            grants,
        },
    )
    .await?;
    PermissionAssignmentRepository::create(
        &ctx.pool,
        CreatePermissionAssignmentInput {
            identity,
            permset: set.id,
        },
    )
    .await?;
    attune_api::authz::AuthorizationService::invalidate_identity_authz_cache(identity).await;
    Ok(())
}

async fn signing_key(ctx: &TestContext) -> Result<attune_common::models::Key> {
    let value = json!({
        "private_key_pem": include_str!("../../common/tests/fixtures/jwt_signing_test_private.pem"),
        "profiles": {"salesforce": {"issuer":"approved-client", "audience":"https://login.salesforce.com",
            "subjects":["sales@example.com","support@example.com"], "pack_refs":["sales","support"], "max_ttl_seconds":120}}
    });
    system_key(ctx, "salesforce_signer", value).await
}

async fn system_key(
    ctx: &TestContext,
    local_ref: &str,
    value: Value,
) -> Result<attune_common::models::Key> {
    let encryption = ctx.state.config.security.encryption_key.as_ref().unwrap();
    Ok(KeyRepository::create(
        &ctx.pool,
        CreateKeyInput {
            local_ref: local_ref.into(),
            owner_type: OwnerType::System,
            owner_identity: None,
            owner_pack: None,
            owner_pack_ref: None,
            owner_action: None,
            owner_action_ref: None,
            owner_sensor: None,
            owner_sensor_ref: None,
            name: format!("{local_ref} fixture key"),
            encrypted: true,
            encryption_key_hash: Some(crypto::hash_encryption_key(encryption)),
            value: crypto::encrypt_json(&value, encryption)?,
        },
    )
    .await?)
}

#[tokio::test]
async fn disclosure_uses_original_key_bindings_and_requires_every_combined_origin() -> Result<()> {
    let ctx = TestContext::new().await?.with_auth().await?;
    let identity = validate_token(ctx.token().unwrap(), &jwt_config(&ctx))?
        .sub
        .parse::<i64>()?;
    grant(&ctx, identity, "fixture.key_read", json!([
        {"resource":"keys","actions":["read"],"constraints":{"owner_types":["system"],"refs":["system.original","system.replacement"]}},
        {"resource":"actions","actions":["read","execute"]},
        {"resource":"executions","actions":["read","decrypt"]}
    ])).await?;
    let original = system_key(&ctx, "original", json!("original-fixture-only")).await?;
    let replacement = system_key(&ctx, "replacement", json!("replacement-fixture-only")).await?;
    let id = execution(&ctx, "sales", json!({"bound":redaction_marker(), "combined":redaction_marker(), "unknown":redaction_marker()})).await?;
    let make_secret =
        |path: &str, value: &str, origins: Vec<SecretOrigin>| -> Result<SecretValueInput> {
            Ok(SecretValueInput {
                json_path: path.into(),
                value: json!(value),
                source_kind: "provenance".into(),
                source_ref: Some(serde_json::to_string(&SecretProvenance {
                    origins,
                    templates: vec![attune_common::secret_provenance::TemplateOrigin {
                        component_type: "rule".into(),
                        component_ref: "sales.login".into(),
                        input_path: path.into(),
                        expression: "keystore[pack.config.signer_ref]".into(),
                    }],
                })?),
            })
        };
    let inputs = vec![
        make_secret(
            "/bound",
            "original-fixture-only",
            vec![SecretOrigin::Key(attune_common::key_access::key_origin(
                &original,
            ))],
        )?,
        make_secret(
            "/combined",
            "original:replacement",
            vec![
                SecretOrigin::Key(attune_common::key_access::key_origin(&original)),
                SecretOrigin::Key(attune_common::key_access::key_origin(&replacement)),
            ],
        )?,
        SecretValueInput {
            json_path: "/unknown".into(),
            value: json!("must-remain-hidden"),
            source_kind: "keystore".into(),
            source_ref: Some("system.original".into()),
        },
    ];
    let prepared = prepare_secret_values(
        inputs,
        ctx.state.config.security.encryption_key.as_ref().unwrap(),
    )?;
    ExecutionSecretValueRepository::upsert_many(&ctx.pool, ENTITY_EXECUTION_CONFIG, id, &prepared)
        .await?;
    let pack = PackRepository::find_by_ref(&ctx.pool, "sales")
        .await?
        .unwrap();
    PackRepository::update(
        &ctx.pool,
        pack.id,
        UpdatePackInput {
            config: Some(json!({"signer_ref":"system.replacement"})),
            ..Default::default()
        },
    )
    .await?;
    grant(
        &ctx,
        identity,
        "fixture.replacement_decrypt",
        json!([
            {"resource":"keys","actions":["decrypt"],"constraints":{"refs":["system.replacement"]}}
        ]),
    )
    .await?;
    let url = format!("/api/v1/executions/{id}?include_secret_values=true");
    let response = ctx.get(&url, ctx.token()).await?;
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response.json().await?;
    for field in ["bound", "combined", "unknown"] {
        assert!(attune_common::secret_values::is_redaction_marker(
            &body["data"]["config"][field]
        ));
    }
    let known = ExecutionSecretValueRepository::find_stored_by_entity(
        &ctx.pool,
        ENTITY_EXECUTION_CONFIG,
        id,
    )
    .await?
    .into_iter()
    .filter(|secret| secret.json_path != "/unknown")
    .collect::<Vec<_>>();
    assert!(attune_common::key_access::authorize_parameter_key_origins(
        &ctx.pool,
        Some(identity),
        &known
    )
    .await
    .is_err());
    grant(
        &ctx,
        identity,
        "fixture.original_decrypt",
        json!([
            {"resource":"keys","actions":["decrypt"],"constraints":{"refs":["system.original"]}}
        ]),
    )
    .await?;
    attune_common::key_access::authorize_parameter_key_origins(&ctx.pool, Some(identity), &known)
        .await?;
    assert!(
        attune_common::key_access::authorize_parameter_key_origins(&ctx.pool, None, &known)
            .await
            .is_err()
    );
    let mut local = known[0].clone();
    local.source_ref = Some(serde_json::to_string(
        &attune_common::secret_values::SecretSource::ParameterSchema {
            path: local.json_path.clone(),
        }
        .provenance(),
    )?);
    attune_common::key_access::authorize_parameter_key_origins(&ctx.pool, None, &[local]).await?;
    let response = ctx.get(&url, ctx.token()).await?;
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response.json().await?;
    assert_eq!(body["data"]["config"]["bound"], "original-fixture-only");
    assert_eq!(body["data"]["config"]["combined"], "original:replacement");
    assert!(attune_common::secret_values::is_redaction_marker(
        &body["data"]["config"]["unknown"]
    ));
    ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn named_execution_token_authority_is_bounded_by_current_active_executor() -> Result<()> {
    let ctx = TestContext::new().await?.with_auth().await?;
    let config = jwt_config(&ctx);
    let identity = validate_token(ctx.token().unwrap(), &config)?
        .sub
        .parse::<i64>()?;
    grant(&ctx, identity, "fixture.key_read", json!([
        {"resource":"keys","actions":["read"],"constraints":{"refs":["system.salesforce_signer"]}},
        {"resource":"actions","actions":["read","execute"]},
        {"resource":"executions","actions":["read"]}
    ])).await?;
    signing_key(&ctx).await?;
    let id = execution(&ctx, "sales", json!({})).await?;
    let mut token = generate_execution_token_with_permission_sets(
        identity,
        id,
        "sales.login",
        &config,
        Some(60),
        &["fixture.key_read".into()],
    )?;
    let response = ctx
        .get("/api/v1/keys/system.salesforce_signer", Some(&token))
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    PermissionSetRepository::create(
        &ctx.pool,
        CreatePermissionSetInput {
            r#ref: "fixture.escalated".into(),
            pack: None,
            pack_ref: None,
            label: None,
            description: None,
            grants: json!([{"resource":"identities","actions":["create","delete"]}]),
        },
    )
    .await?;
    token = generate_execution_token_with_permission_sets(
        identity,
        id,
        "sales.login",
        &config,
        Some(60),
        &["fixture.escalated".into()],
    )?;
    let response = ctx
        .get("/api/v1/keys/system.salesforce_signer", Some(&token))
        .await?;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    IdentityRepository::update(
        &ctx.pool,
        identity,
        UpdateIdentityInput {
            frozen: Some(true),
            ..Default::default()
        },
    )
    .await?;
    token = generate_execution_token_with_permission_sets(
        identity,
        id,
        "sales.login",
        &config,
        Some(60),
        &["fixture.key_read".into()],
    )?;
    let response = ctx
        .get("/api/v1/keys/system.salesforce_signer", Some(&token))
        .await?;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    ctx.cleanup().await?;
    Ok(())
}

async fn execution(ctx: &TestContext, pack_ref: &str, parameters: Value) -> Result<i64> {
    let pack = helpers::create_test_pack(&ctx.pool, pack_ref).await?;
    let action =
        helpers::create_test_action(&ctx.pool, pack.id, pack_ref, &format!("{pack_ref}.login"))
            .await?;
    helpers::activate_test_pack_release_with_projections(
        &ctx.pool,
        &pack,
        &attune_common::repositories::component_lifecycle::PackProjectionIds {
            actions: vec![action.id],
            ..Default::default()
        },
    )
    .await?;
    let response = ctx.post("/api/v1/executions/execute",json!({"action_ref":action.r#ref,"parameters":parameters,"permission_set_refs":["fixture.key_read"]}),ctx.token()).await?;
    let status = response.status();
    let body: Value = response.json().await?;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    Ok(body["data"]["id"].as_i64().unwrap())
}

#[tokio::test]
async fn read_only_users_can_use_approved_signer_but_cannot_retrieve_private_material() -> Result<()>
{
    let ctx = TestContext::new().await?.with_auth().await?;
    let config = jwt_config(&ctx);
    let identity = validate_token(ctx.token().unwrap(), &config)?
        .sub
        .parse::<i64>()?;
    grant(&ctx,identity,"fixture.key_read",json!([
        {"resource":"keys","actions":["read"],"constraints":{"owner_types":["system"],"refs":["system.salesforce_signer"]}},
        {"resource":"actions","actions":["read","execute"]},
        {"resource":"executions","actions":["read","decrypt"]}
    ])).await?;
    let key = signing_key(&ctx).await?;
    let response = ctx
        .get("/api/v1/keys/system.salesforce_signer", ctx.token())
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response.json().await?;
    assert!(body["data"]["value"].is_null());
    for (pack_ref, subject) in [
        ("sales", "sales@example.com"),
        ("support", "support@example.com"),
    ] {
        let id = execution(&ctx, pack_ref, json!({})).await?;
        let token = generate_execution_token_with_permission_sets(
            identity,
            id,
            &format!("{pack_ref}.login"),
            &config,
            Some(60),
            &["fixture.key_read".into()],
        )?;
        let response = ctx
            .post(
                "/api/v1/keys/system.salesforce_signer/sign-jwt",
                json!({"profile_ref":"salesforce","subject":subject,"ttl_seconds":60}),
                Some(&token),
            )
            .await?;
        assert_eq!(response.status(), StatusCode::OK);
        let body: Value = response.json().await?;
        assert!(
            body["data"]["assertion"]
                .as_str()
                .unwrap()
                .split('.')
                .count()
                == 3
        );
        assert!(body["data"].get("private_key_pem").is_none());
        let response = ctx
            .post(
                "/api/v1/keys/system.salesforce_signer/sign-jwt",
                json!({"profile_ref":"salesforce","subject":"admin@example.com","ttl_seconds":60}),
                Some(&token),
            )
            .await?;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        let response = ctx
            .get("/api/v1/keys/system.salesforce_signer", Some(&token))
            .await?;
        let body: Value = response.json().await?;
        assert!(body["data"]["value"].is_null());
    }
    let response = ctx
        .post(
            "/api/v1/keys/system.salesforce_signer/sign-jwt",
            json!({"profile_ref":"salesforce","subject":"sales@example.com","ttl_seconds":60}),
            ctx.token(),
        )
        .await?;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert!(key.encrypted);
    ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn execution_decrypt_does_not_override_recorded_key_disclosure_authority() -> Result<()> {
    let ctx = TestContext::new().await?.with_auth().await?;
    let identity = validate_token(ctx.token().unwrap(), &jwt_config(&ctx))?
        .sub
        .parse::<i64>()?;
    grant(&ctx,identity,"fixture.key_read",json!([
        {"resource":"keys","actions":["read"],"constraints":{"owner_types":["system"],"refs":["system.salesforce_signer"]}},
        {"resource":"actions","actions":["read","execute"]},
        {"resource":"executions","actions":["read","decrypt"]}
    ])).await?;
    let key = signing_key(&ctx).await?;
    let id = execution(&ctx, "sales", json!({"signing_key":redaction_marker()})).await?;
    let provenance = SecretProvenance {
        origins: vec![SecretOrigin::Key(attune_common::key_access::key_origin(
            &key,
        ))],
        templates: Vec::new(),
    };
    let prepared = prepare_secret_values(
        vec![SecretValueInput {
            json_path: "/signing_key".into(),
            value: json!("dummy-private-material"),
            source_kind: "provenance".into(),
            source_ref: Some(serde_json::to_string(&provenance)?),
        }],
        ctx.state.config.security.encryption_key.as_ref().unwrap(),
    )?;
    ExecutionSecretValueRepository::upsert_many(&ctx.pool, ENTITY_EXECUTION_CONFIG, id, &prepared)
        .await?;
    let response = ctx
        .get(
            &format!("/api/v1/executions/{id}?include_secret_values=true"),
            ctx.token(),
        )
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response.json().await?;
    assert!(attune_common::secret_values::is_redaction_marker(
        &body["data"]["config"]["signing_key"]
    ));
    let response = ctx
        .get(
            &format!("/api/v1/executions/{id}/logs/stdout/stream"),
            ctx.token(),
        )
        .await?;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    grant(
        &ctx,
        identity,
        "fixture.log_read",
        json!([{"resource":"artifacts","actions":["read"]}]),
    )
    .await?;
    let artifact = ArtifactRepository::create(
        &ctx.pool,
        CreateArtifactInput {
            r#ref: format!("execution.{id}.stdout"),
            scope: OwnerType::Pack,
            owner: "sales".into(),
            r#type: ArtifactType::FileText,
            visibility: ArtifactVisibility::Private,
            classification: ArtifactClassification::RuntimeLog,
            retention_policy: RetentionPolicyType::Days,
            retention_limit: 1,
            name: None,
            description: None,
            content_type: Some("text/plain".into()),
            data: None,
        },
    )
    .await?;
    ArtifactVersionRepository::create(
        &ctx.pool,
        CreateArtifactVersionInput {
            artifact: artifact.id,
            execution: Some(id),
            content_type: Some("text/plain".into()),
            content: None,
            content_json: None,
            file_path: Some(format!("execution/{id}/stdout/1/log.txt")),
            meta: None,
            created_by: None,
        },
    )
    .await?;
    let response = ctx
        .get(&format!("/api/v1/artifacts/{}", artifact.id), ctx.token())
        .await?;
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    grant(&ctx,identity,"fixture.key_decrypt",json!([{ "resource":"keys","actions":["decrypt"],"constraints":{"owner_types":["system"],"refs":["system.salesforce_signer"]}}])).await?;
    let response = ctx
        .get(&format!("/api/v1/artifacts/{}", artifact.id), ctx.token())
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    let response = ctx
        .get(
            &format!("/api/v1/executions/{id}?include_secret_values=true"),
            ctx.token(),
        )
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response.json().await?;
    assert_eq!(
        body["data"]["config"]["signing_key"],
        "dummy-private-material"
    );
    assert!(KeyRepository::find_by_ref(&ctx.pool, &key.r#ref)
        .await?
        .is_some());
    ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn key_write_authority_does_not_disclose_existing_encrypted_material() -> Result<()> {
    let ctx = TestContext::new().await?.with_auth().await?;
    let identity = validate_token(ctx.token().unwrap(), &jwt_config(&ctx))?
        .sub
        .parse::<i64>()?;
    IdentityRepository::update(
        &ctx.pool,
        identity,
        UpdateIdentityInput {
            attributes: Some(json!({"team":"ops"})),
            ..Default::default()
        },
    )
    .await?;
    grant(&ctx, identity, "fixture.key_writer", json!([
        {"resource":"keys","actions":["read","update"],"constraints":{"refs":["system.probe"],"attributes":{"team":"ops"}}}
    ])).await?;
    system_key(&ctx, "probe", json!("fixture-private")).await?;
    let response = ctx
        .put(
            "/api/v1/keys/system.probe",
            json!({"name":"renamed key"}),
            ctx.token(),
        )
        .await?;
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response.json().await?;
    assert!(body["data"]["value"].is_null());
    assert!(!body.to_string().contains("fixture-private"));
    let response = ctx
        .put(
            "/api/v1/keys/system.probe",
            json!({"encrypted":false}),
            ctx.token(),
        )
        .await?;
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert!(
        KeyRepository::find_by_ref(&ctx.pool, "system.probe")
            .await?
            .unwrap()
            .encrypted
    );
    grant(
        &ctx,
        identity,
        "fixture.key_open",
        json!([
            {"resource":"keys","actions":["decrypt"],"constraints":{"refs":["system.probe"]}}
        ]),
    )
    .await?;
    for encrypted in [false, true] {
        let response = ctx
            .put(
                "/api/v1/keys/system.probe",
                json!({"encrypted":encrypted}),
                ctx.token(),
            )
            .await?;
        assert_eq!(response.status(), StatusCode::OK);
        let body: Value = response.json().await?;
        assert_eq!(body["data"]["value"], "fixture-private");
        let stored = KeyRepository::find_by_ref(&ctx.pool, "system.probe")
            .await?
            .unwrap();
        assert_eq!(stored.encrypted, encrypted);
        assert_eq!(stored.encryption_key_hash.is_some(), encrypted);
    }
    ctx.cleanup().await?;
    Ok(())
}
