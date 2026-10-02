mod helpers;

use attune_api::{server::Server, state::AppState};
use attune_common::{
    config::{OidcConfig, OidcDeviceClientConfig},
    repositories::{
        identity::{
            CreateIdentityRoleAssignmentInput, IdentityRepository,
            IdentityRoleAssignmentRepository, UpdateIdentityInput,
        },
        Create, FindById, Update,
    },
};
use axum::http::StatusCode;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use helpers::{Result, TestContext};
use rand08::{rngs::StdRng, SeedableRng};
use rsa::{
    signature::{SignatureEncoding, Signer},
    traits::PublicKeyParts,
    RsaPrivateKey,
};
use serde_json::{json, Value};
use std::sync::Arc;
use wiremock::{
    matchers::{body_string_contains, method, path},
    Mock, MockServer, ResponseTemplate,
};

async fn context(provider: &MockServer) -> Result<TestContext> {
    let mut ctx = TestContext::new().await?;
    let mut config = (*ctx.state.config).clone();
    config.security.oidc = Some(OidcConfig {
        enabled: true,
        discovery_url: Some(format!("{}/discovery", provider.uri())),
        client_id: Some("attune-web".into()),
        provider_name: "test".into(),
        provider_label: None,
        provider_icon_url: None,
        client_secret: Some("web-secret-must-not-be-forwarded".into()),
        redirect_uri: Some("https://app.example.com/auth/callback".into()),
        post_logout_redirect_uri: None,
        scopes: vec!["groups".into()],
        require_groups: false,
        device_client: Some(OidcDeviceClientConfig {
            client_id: "attune-native".into(),
            client_secret: None,
        }),
    });
    ctx.state = Arc::new(AppState::new_with_audit(
        ctx.pool.clone(),
        config,
        ctx.state.audit_emitter.clone(),
    ));
    ctx.app = Server::new(ctx.state.clone()).router();
    Ok(ctx)
}

async fn discovery(provider: &MockServer, device: bool) {
    let mut body = json!({"issuer":provider.uri(), "authorization_endpoint":format!("{}/authorize",provider.uri()),
        "token_endpoint":format!("{}/token",provider.uri()), "jwks_uri":format!("{}/jwks",provider.uri()),
        "userinfo_endpoint":format!("{}/userinfo",provider.uri()), "response_types_supported":["code"],
        "subject_types_supported":["public"], "id_token_signing_alg_values_supported":["RS256"]});
    if device {
        body["device_authorization_endpoint"] = json!(format!("{}/device", provider.uri()));
    }
    Mock::given(method("GET"))
        .and(path("/discovery"))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .mount(provider)
        .await;
}

async fn start(ctx: &TestContext, provider: &MockServer) -> Result<Value> {
    Mock::given(method("POST"))
        .and(path("/device"))
        .and(body_string_contains("client_id=attune-native"))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            json!({"device_code":"provider-device-secret", "user_code":"ABCD-1234",
            "verification_uri":format!("{}/verify",provider.uri()), "expires_in":60,"interval":1}),
        ))
        .mount(provider)
        .await;
    let response = ctx.post("/auth/oidc/device/start", json!({}), None).await?;
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response.json().await?;
    assert!(!body.to_string().contains("provider-device-secret"));
    assert_eq!(body["data"]["user_code"], "ABCD-1234");
    Ok(body["data"].clone())
}

async fn redeem(ctx: &TestContext, provider: &MockServer) -> Result<helpers::TestResponse> {
    let auth = start(ctx, provider).await?;
    let key = ctx.state.config.security.encryption_key.as_deref().unwrap();
    let mut session: Value = serde_json::from_str(&attune_common::crypto::decrypt(
        auth["device_code"].as_str().unwrap(),
        key,
    )?)?;
    session["next_poll_at_ms"] = json!(0);
    let code = attune_common::crypto::encrypt(&session.to_string(), key)?;
    ctx.post("/auth/oidc/device/poll", json!({"device_code":code}), None)
        .await
}

async fn signed_id_token(provider: &MockServer, groups: Option<Value>) -> Result<String> {
    attune_common::auth::install_crypto_provider();
    let key = RsaPrivateKey::new(&mut StdRng::seed_from_u64(8628), 2048)?;
    let public = key.to_public_key();
    Mock::given(method("GET")).and(path("/jwks")).respond_with(ResponseTemplate::new(200).set_body_json(json!({"keys":[{
        "kty":"RSA", "kid":"fixture", "use":"sig", "alg":"RS256", "n":URL_SAFE_NO_PAD.encode(public.n().to_bytes_be()), "e":URL_SAFE_NO_PAD.encode(public.e().to_bytes_be())
    }]}))).mount(provider).await;
    let mut claims = json!({"iss":provider.uri(), "sub":"same-subject", "aud":"attune-native", "iat":chrono::Utc::now().timestamp(),
        "exp":chrono::Utc::now().timestamp()+300, "email":"device@example.com"});
    if let Some(groups) = groups {
        claims["groups"] = groups;
    }
    let unsigned = format!(
        "{}.{}",
        URL_SAFE_NO_PAD.encode(json!({"alg":"RS256","kid":"fixture"}).to_string()),
        URL_SAFE_NO_PAD.encode(claims.to_string())
    );
    let signature =
        rsa::pkcs1v15::SigningKey::<sha2_10::Sha256>::new(key).sign(unsigned.as_bytes());
    Ok(format!(
        "{unsigned}.{}",
        URL_SAFE_NO_PAD.encode(signature.to_bytes())
    ))
}

#[tokio::test]
async fn oidc_device_pending_denial_tamper_and_unsupported_provider_are_explicit() -> Result<()> {
    let provider = MockServer::start().await;
    discovery(&provider, true).await;
    let ctx = context(&provider).await?;
    let authorization = start(&ctx, &provider).await?;
    let code = authorization["device_code"].as_str().unwrap();
    let rejected = ctx
        .post(
            "/auth/oidc/device/poll",
            json!({"device_code":"tampered"}),
            None,
        )
        .await?;
    assert_eq!(rejected.status(), StatusCode::BAD_REQUEST);
    let early = ctx
        .post("/auth/oidc/device/poll", json!({"device_code":code}), None)
        .await?;
    let early: Value = early.json().await?;
    assert_eq!(early["data"]["reason"], "slow_down");
    assert_eq!(early["data"]["interval"], 6);

    // Make the original server-issued session eligible without sleeping through an IdP interval.
    let mut session: Value = serde_json::from_str(&attune_common::crypto::decrypt(
        code,
        ctx.state.config.security.encryption_key.as_deref().unwrap(),
    )?)
    .unwrap();
    session["next_poll_at_ms"] = json!(0);
    let eligible = attune_common::crypto::encrypt(
        &session.to_string(),
        ctx.state.config.security.encryption_key.as_deref().unwrap(),
    )?;
    Mock::given(method("POST"))
        .and(path("/token"))
        .and(body_string_contains(
            "grant_type=urn%3Aietf%3Aparams%3Aoauth%3Agrant-type%3Adevice_code",
        ))
        .respond_with(ResponseTemplate::new(400).set_body_json(
            json!({"error":"authorization_pending", "error_description":"provider-device-secret"}),
        ))
        .mount(&provider)
        .await;
    let pending: Value = ctx
        .post(
            "/auth/oidc/device/poll",
            json!({"device_code":eligible}),
            None,
        )
        .await?
        .json()
        .await?;
    assert_eq!(pending["data"]["reason"], "authorization_pending");
    assert!(!pending.to_string().contains("provider-device-secret"));
    for (priority, error, expected) in [
        (2, "access_denied", "access_denied"),
        (1, "expired_token", "expired"),
    ] {
        Mock::given(method("POST"))
            .and(path("/token"))
            .respond_with(ResponseTemplate::new(400).set_body_json(json!({"error":error})))
            .with_priority(priority)
            .mount(&provider)
            .await;
        let status: Value = ctx
            .post(
                "/auth/oidc/device/poll",
                json!({"device_code":eligible}),
                None,
            )
            .await?
            .json()
            .await?;
        assert_eq!(status["data"]["status"], expected);
    }
    let mut expired_session = session.clone();
    expired_session["expires_at_ms"] = json!(0);
    let expired_code = attune_common::crypto::encrypt(
        &expired_session.to_string(),
        ctx.state.config.security.encryption_key.as_deref().unwrap(),
    )?;
    let expired: Value = ctx
        .post(
            "/auth/oidc/device/poll",
            json!({"device_code":expired_code}),
            None,
        )
        .await?
        .json()
        .await?;
    assert_eq!(expired["data"]["status"], "expired");
    for request in provider.received_requests().await.unwrap() {
        assert!(
            !request.headers.contains_key("authorization"),
            "public device client must not inherit the web secret"
        );
    }

    provider.reset().await;
    discovery(&provider, false).await;
    let unsupported = ctx.post("/auth/oidc/device/start", json!({}), None).await?;
    assert_eq!(unsupported.status(), StatusCode::NOT_IMPLEMENTED);
    ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn oidc_device_verified_subject_reuses_web_identity_and_frozen_users_cannot_login(
) -> Result<()> {
    let provider = MockServer::start().await;
    discovery(&provider, true).await;
    let ctx = context(&provider).await?;
    let id_token = signed_id_token(&provider, None).await?;
    Mock::given(method("POST"))
        .and(path("/token"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(
                json!({"access_token":"provider-access-secret", "id_token":id_token}),
            ),
        )
        .mount(&provider)
        .await;
    Mock::given(method("GET"))
        .and(path("/userinfo"))
        .respond_with(ResponseTemplate::new(200).set_body_json(
            json!({"sub":"same-subject", "groups":["test-role"], "name":"Device user"}),
        ))
        .mount(&provider)
        .await;

    let original = IdentityRepository::upsert_oidc_identity(&ctx.pool, attune_common::repositories::identity::OidcUpsertInput {
        issuer:provider.uri(), sub:"same-subject".into(), client_id:"attune-web".into(), desired_login:"existing@example.com".into(), fallback_login:"oidc:fixture".into(), display_name:None,
        attributes:json!({"oidc":{"issuer":provider.uri(), "sub":"same-subject", "client_id":"attune-web"}}),
        roles: Vec::new(),
    }).await?;
    let response = redeem(&ctx, &provider).await?;
    assert_eq!(response.status(), StatusCode::OK);
    let body: Value = response.json().await?;
    assert_eq!(body["data"]["tokens"]["user"]["id"], original.id);
    assert!(!body.to_string().contains("provider-access-secret"));
    let updated = IdentityRepository::find_by_id(&ctx.pool, original.id)
        .await?
        .unwrap();
    assert_eq!(updated.attributes["oidc"]["client_id"], "attune-web");
    assert_eq!(
        updated.attributes["oidc"]["authentication_client_id"],
        "attune-native"
    );
    assert_eq!(updated.attributes["oidc"]["groups"], json!(["test-role"]));
    IdentityRepository::update(
        &ctx.pool,
        original.id,
        UpdateIdentityInput {
            frozen: Some(true),
            ..Default::default()
        },
    )
    .await?;
    assert_eq!(
        redeem(&ctx, &provider).await?.status(),
        StatusCode::FORBIDDEN
    );
    ctx.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn oidc_device_required_groups_fail_before_identity_or_roles_change() -> Result<()> {
    let provider = MockServer::start().await;
    discovery(&provider, true).await;
    let mut ctx = context(&provider).await?;
    let id_token = signed_id_token(&provider, None).await?;
    Mock::given(method("POST"))
        .and(path("/token"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"access_token":"provider-secret", "id_token":id_token})),
        )
        .mount(&provider)
        .await;

    assert_eq!(
        redeem(&ctx, &provider).await?.status(),
        StatusCode::UNAUTHORIZED
    );
    assert!(
        IdentityRepository::find_oidc_by_issuer_sub(&ctx.pool, &provider.uri(), "same-subject")
            .await?
            .is_none(),
        "missing required groups must not create an identity"
    );
    let original = IdentityRepository::upsert_oidc_identity(&ctx.pool, attune_common::repositories::identity::OidcUpsertInput {
        issuer:provider.uri(), sub:"same-subject".into(), client_id:"attune-web".into(), desired_login:"existing@example.com".into(), fallback_login:"oidc:fixture".into(), display_name:None,
        attributes:json!({"oidc":{"issuer":provider.uri(), "sub":"same-subject", "client_id":"attune-web", "groups":["existing-role"]}}),
        roles: vec!["existing-role".into()],
    }).await?;
    IdentityRoleAssignmentRepository::create(
        &ctx.pool,
        CreateIdentityRoleAssignmentInput {
            identity: original.id,
            role: "local-role".into(),
            source: "local".into(),
            managed: false,
        },
    )
    .await?;

    for response in [
        ResponseTemplate::new(503),
        ResponseTemplate::new(200).set_body_json(json!({"sub":"same-subject"})),
        ResponseTemplate::new(200)
            .set_body_json(json!({"sub":"other-subject", "groups":["admins"]})),
        ResponseTemplate::new(200)
            .set_body_json(json!({"sub":"same-subject", "groups":["admins", 42]})),
    ] {
        let _userinfo = Mock::given(method("GET"))
            .and(path("/userinfo"))
            .respond_with(response)
            .mount_as_scoped(&provider)
            .await;
        let rejected = redeem(&ctx, &provider).await?;
        assert_eq!(rejected.status(), StatusCode::UNAUTHORIZED);
        assert!(!rejected
            .json::<Value>()
            .await?
            .to_string()
            .contains("access_token"));
        let unchanged = IdentityRepository::find_by_id(&ctx.pool, original.id)
            .await?
            .unwrap();
        assert_eq!(unchanged.attributes, original.attributes);
        assert_eq!(
            IdentityRoleAssignmentRepository::find_role_names_by_identity(&ctx.pool, original.id)
                .await?,
            vec!["existing-role", "local-role"]
        );
    }

    // Providers can deliver mandatory groups without a scope named "groups".
    let mut config = (*ctx.state.config).clone();
    let oidc = config.security.oidc.as_mut().unwrap();
    oidc.scopes.clear();
    oidc.require_groups = true;
    ctx.state = Arc::new(AppState::new_with_audit(
        ctx.pool.clone(),
        config,
        ctx.state.audit_emitter.clone(),
    ));
    ctx.app = Server::new(ctx.state.clone()).router();
    assert_eq!(
        redeem(&ctx, &provider).await?.status(),
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        IdentityRoleAssignmentRepository::find_role_names_by_identity(&ctx.pool, original.id)
            .await?,
        vec!["existing-role", "local-role"]
    );

    let _userinfo = Mock::given(method("GET"))
        .and(path("/userinfo"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({"sub":"same-subject", "groups":[]})),
        )
        .mount_as_scoped(&provider)
        .await;
    assert_eq!(redeem(&ctx, &provider).await?.status(), StatusCode::OK);
    assert_eq!(
        IdentityRoleAssignmentRepository::find_role_names_by_identity(&ctx.pool, original.id)
            .await?,
        vec!["local-role"]
    );
    let updated = IdentityRepository::find_by_id(&ctx.pool, original.id)
        .await?
        .unwrap();
    assert_eq!(updated.attributes["oidc"]["groups"], json!([]));

    // An explicit empty signed claim must win over a nonempty UserInfo list.
    let id_token = signed_id_token(&provider, Some(json!([]))).await?;
    let _tokens = Mock::given(method("POST"))
        .and(path("/token"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"access_token":"provider-secret", "id_token":id_token})),
        )
        .with_priority(1)
        .mount_as_scoped(&provider)
        .await;
    let _nonempty_userinfo = Mock::given(method("GET"))
        .and(path("/userinfo"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({"sub":"same-subject", "groups":["admins"]})),
        )
        .with_priority(1)
        .mount_as_scoped(&provider)
        .await;
    assert_eq!(redeem(&ctx, &provider).await?.status(), StatusCode::OK);
    assert_eq!(
        IdentityRoleAssignmentRepository::find_role_names_by_identity(&ctx.pool, original.id)
            .await?,
        vec!["local-role"]
    );
    ctx.cleanup().await?;
    Ok(())
}
