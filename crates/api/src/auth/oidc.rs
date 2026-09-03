//! OpenID Connect helpers for browser login.

use attune_common::{
    config::OidcConfig,
    repositories::identity::{
        IdentityRepository, IdentityRoleAssignmentRepository, OidcUpsertInput,
    },
};
use axum::{
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Redirect, Response},
};
use axum_extra::extract::cookie::{Cookie, SameSite};
use cookie::time::Duration as CookieDuration;
use jsonwebtoken::{
    decode, decode_header,
    jwk::{AlgorithmParameters, JwkSet},
    Algorithm, DecodingKey, Validation,
};
use openidconnect::{
    core::{CoreAuthenticationFlow, CoreClient, CoreProviderMetadata, CoreUserInfoClaims},
    AuthType, AuthorizationCode, ClientId, ClientSecret, CsrfToken, HttpRequest, HttpResponse,
    LocalizedClaim, Nonce, OAuth2TokenResponse, PkceCodeChallenge, PkceCodeVerifier, RedirectUrl,
    Scope, TokenResponse as OidcTokenResponse,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value as JsonValue};
use sha2::{Digest, Sha256};
use std::sync::LazyLock;
use url::{form_urlencoded::byte_serialize, Url};

use crate::{
    auth::jwt::{generate_access_token, generate_refresh_token, validate_token},
    dto::{CurrentUserResponse, TokenResponse},
    middleware::error::ApiError,
    state::SharedState,
};

pub const ACCESS_COOKIE_NAME: &str = "attune_access_token";
pub const REFRESH_COOKIE_NAME: &str = "attune_refresh_token";
pub const OIDC_ID_TOKEN_COOKIE_NAME: &str = "attune_oidc_id_token";
pub const OIDC_STATE_COOKIE_NAME: &str = "attune_oidc_state";
pub const OIDC_NONCE_COOKIE_NAME: &str = "attune_oidc_nonce";
pub const OIDC_PKCE_COOKIE_NAME: &str = "attune_oidc_pkce_verifier";
pub const OIDC_REDIRECT_COOKIE_NAME: &str = "attune_oidc_redirect_to";
/// Cookie that carries the CLI's local callback URI (http://localhost:{port}/callback).
/// When present, the OIDC callback redirects to this URI with tokens as query params
/// instead of the usual web-app fragment redirect.
pub const OIDC_CLI_REDIRECT_COOKIE_NAME: &str = "attune_oidc_cli_redirect";

const LOGIN_CALLBACK_PATH: &str = "/login/callback";

/// Validate that a CLI redirect URI is a localhost HTTP URL (security guard against
/// open redirect abuse — only loopback addresses are accepted).
pub fn validate_cli_redirect_uri(uri: &str) -> Result<(), ApiError> {
    let url = Url::parse(uri)
        .map_err(|_| ApiError::BadRequest("Invalid CLI redirect URI".to_string()))?;
    let host = url.host_str().unwrap_or("");
    if url.scheme() != "http" || !matches!(host, "localhost" | "127.0.0.1" | "[::1]") {
        return Err(ApiError::BadRequest(
            "CLI redirect URI must use http://localhost, http://127.0.0.1, or http://[::1]"
                .to_string(),
        ));
    }
    Ok(())
}

#[derive(Debug, thiserror::Error)]
enum OidcHttpClientError {
    #[error("failed to send OIDC HTTP request: {0}")]
    Request(#[from] reqwest::Error),
    #[error("OIDC provider returned HTTP {status}: {body}")]
    HttpStatus { status: StatusCode, body: String },
    #[error("failed to build OIDC HTTP response: {0}")]
    Response(#[from] axum::http::Error),
}

#[derive(Debug, Clone, Deserialize)]
pub struct OidcDiscoveryDocument {
    #[serde(flatten)]
    pub metadata: CoreProviderMetadata,
    #[serde(default)]
    pub end_session_endpoint: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OidcIdentityClaims {
    pub issuer: String,
    pub sub: String,
    pub client_id: String,
    pub email: Option<String>,
    pub email_verified: Option<bool>,
    pub name: Option<String>,
    pub preferred_username: Option<String>,
    pub groups: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct VerifiedIdTokenClaims {
    iss: String,
    sub: String,
    #[serde(default)]
    nonce: Option<String>,
    #[serde(default)]
    email: Option<String>,
    #[serde(default)]
    email_verified: Option<bool>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    preferred_username: Option<String>,
    #[serde(default)]
    groups: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct OidcAuthenticatedIdentity {
    pub current_user: CurrentUserResponse,
    pub token_response: TokenResponse,
    pub id_token: String,
}

#[derive(Debug, Clone)]
pub struct OidcLoginRedirect {
    pub authorization_url: String,
    pub cookies: Vec<Cookie<'static>>,
}

#[derive(Debug, Clone)]
pub struct OidcLogoutRedirect {
    pub redirect_url: String,
    pub cookies: Vec<Cookie<'static>>,
}

#[derive(Debug, Deserialize)]
pub struct OidcCallbackQuery {
    pub code: Option<String>,
    pub state: Option<String>,
    pub error: Option<String>,
    pub error_description: Option<String>,
}

pub async fn build_login_redirect(
    state: &SharedState,
    redirect_to: Option<&str>,
    cli_redirect_uri: Option<&str>,
) -> Result<OidcLoginRedirect, ApiError> {
    let oidc = oidc_config(state)?;
    let discovery = fetch_discovery_document(&oidc).await?;
    let redirect_uri_str = oidc.redirect_uri.clone().unwrap_or_default();
    let redirect_uri = RedirectUrl::new(redirect_uri_str).map_err(|err| {
        ApiError::InternalServerError(format!("Invalid OIDC redirect URI: {err}"))
    })?;
    let client_secret = oidc
        .client_secret
        .clone()
        .filter(|s| !s.trim().is_empty())
        .map(ClientSecret::new);
    let is_public_client = client_secret.is_none();
    let client_id = oidc.client_id.clone().unwrap_or_default();
    let client = CoreClient::from_provider_metadata(
        discovery.metadata.clone(),
        ClientId::new(client_id),
        client_secret,
    )
    .set_redirect_uri(redirect_uri)
    .set_auth_type(if is_public_client {
        AuthType::RequestBody
    } else {
        AuthType::BasicAuth
    });

    let redirect_target = sanitize_redirect_target(redirect_to);
    let pkce = PkceCodeChallenge::new_random_sha256();
    let (auth_url, csrf_state, nonce) = client
        .authorize_url(
            CoreAuthenticationFlow::AuthorizationCode,
            CsrfToken::new_random,
            Nonce::new_random,
        )
        .add_scope(Scope::new("openid".to_string()))
        .add_scope(Scope::new("email".to_string()))
        .add_scope(Scope::new("profile".to_string()))
        .add_scopes(
            oidc.scopes
                .iter()
                .filter(|scope| !matches!(scope.as_str(), "openid" | "email" | "profile"))
                .cloned()
                .map(Scope::new),
        )
        .set_pkce_challenge(pkce.0)
        .url();

    Ok(OidcLoginRedirect {
        authorization_url: auth_url.to_string(),
        cookies: {
            let mut cookies = vec![
                build_cookie(
                    state,
                    OIDC_STATE_COOKIE_NAME,
                    csrf_state.secret().to_string(),
                    600,
                    true,
                ),
                build_cookie(
                    state,
                    OIDC_NONCE_COOKIE_NAME,
                    nonce.secret().to_string(),
                    600,
                    true,
                ),
                build_cookie(
                    state,
                    OIDC_PKCE_COOKIE_NAME,
                    pkce.1.secret().to_string(),
                    600,
                    true,
                ),
                build_cookie(
                    state,
                    OIDC_REDIRECT_COOKIE_NAME,
                    redirect_target,
                    600,
                    false,
                ),
            ];
            if let Some(cli_uri) = cli_redirect_uri {
                validate_cli_redirect_uri(cli_uri)?;
                cookies.push(build_cookie(
                    state,
                    OIDC_CLI_REDIRECT_COOKIE_NAME,
                    cli_uri.to_string(),
                    600,
                    false,
                ));
            }
            cookies
        },
    })
}

pub async fn handle_callback(
    state: &SharedState,
    headers: &HeaderMap,
    query: &OidcCallbackQuery,
) -> Result<OidcAuthenticatedIdentity, ApiError> {
    if let Some(error) = &query.error {
        let description = query
            .error_description
            .as_deref()
            .unwrap_or("OpenID Connect login failed");
        return Err(ApiError::Unauthorized(format!("{error}: {description}")));
    }

    let code = query
        .code
        .as_ref()
        .ok_or_else(|| ApiError::BadRequest("Missing authorization code".to_string()))?;
    let returned_state = query
        .state
        .as_ref()
        .ok_or_else(|| ApiError::BadRequest("Missing OIDC state".to_string()))?;

    let expected_state = get_cookie_value(headers, OIDC_STATE_COOKIE_NAME)
        .ok_or_else(|| ApiError::Unauthorized("Missing OIDC state cookie".to_string()))?;
    let expected_nonce = get_cookie_value(headers, OIDC_NONCE_COOKIE_NAME)
        .ok_or_else(|| ApiError::Unauthorized("Missing OIDC nonce cookie".to_string()))?;
    let pkce_verifier = get_cookie_value(headers, OIDC_PKCE_COOKIE_NAME)
        .ok_or_else(|| ApiError::Unauthorized("Missing OIDC PKCE verifier cookie".to_string()))?;

    if returned_state != &expected_state {
        return Err(ApiError::Unauthorized(
            "OIDC state validation failed".to_string(),
        ));
    }

    let oidc = oidc_config(state)?;
    let discovery = fetch_discovery_document(&oidc).await?;
    let redirect_uri_str = oidc.redirect_uri.clone().unwrap_or_default();
    let redirect_uri = RedirectUrl::new(redirect_uri_str).map_err(|err| {
        ApiError::InternalServerError(format!("Invalid OIDC redirect URI: {err}"))
    })?;
    let client_secret = oidc
        .client_secret
        .clone()
        .filter(|s| !s.trim().is_empty())
        .map(ClientSecret::new);
    let is_public_client = client_secret.is_none();
    let client_id = oidc.client_id.clone().unwrap_or_default();
    let client = CoreClient::from_provider_metadata(
        discovery.metadata.clone(),
        ClientId::new(client_id),
        client_secret,
    )
    .set_redirect_uri(redirect_uri)
    .set_auth_type(if is_public_client {
        AuthType::RequestBody
    } else {
        AuthType::BasicAuth
    });

    let token_response = client
        .exchange_code(AuthorizationCode::new(code.clone()))
        .map_err(|err| {
            ApiError::InternalServerError(format!("OIDC token request is misconfigured: {err}"))
        })?
        .set_pkce_verifier(PkceCodeVerifier::new(pkce_verifier))
        .request_async(&oidc_async_http_client)
        .await
        .map_err(|err| ApiError::Unauthorized(format!("OIDC token exchange failed: {err}")))?;

    let id_token = token_response.id_token().ok_or_else(|| {
        ApiError::Unauthorized("OIDC provider did not return an ID token".to_string())
    })?;

    let raw_id_token = id_token.to_string();
    let claims = verify_id_token(&raw_id_token, &discovery, &oidc, &expected_nonce).await?;

    let mut oidc_claims = OidcIdentityClaims {
        issuer: claims.iss,
        sub: claims.sub,
        client_id: oidc.client_id.clone().unwrap_or_default(),
        email: claims.email,
        email_verified: claims.email_verified,
        name: claims.name,
        preferred_username: claims.preferred_username,
        groups: claims.groups,
    };

    if let Ok(userinfo_request) = client.user_info(token_response.access_token().to_owned(), None) {
        if let Ok(userinfo) = userinfo_request
            .request_async(&oidc_async_http_client)
            .await
        {
            merge_userinfo_claims(&mut oidc_claims, &userinfo);
        }
    }

    let identity = upsert_identity(state, &oidc_claims).await?;
    if identity.frozen {
        return Err(ApiError::Forbidden(
            "Identity is frozen and cannot authenticate".to_string(),
        ));
    }
    let access_token = generate_access_token(identity.id, &identity.login, &state.jwt_config)?;
    let refresh_token = generate_refresh_token(identity.id, &identity.login, &state.jwt_config)?;

    let token_response = TokenResponse::new(
        access_token,
        refresh_token,
        state.jwt_config.access_token_expiration,
    )
    .with_user(
        identity.id,
        identity.login.clone(),
        identity.display_name.clone(),
    );

    Ok(OidcAuthenticatedIdentity {
        current_user: CurrentUserResponse {
            id: identity.id,
            login: identity.login.clone(),
            display_name: identity.display_name.clone(),
            auth_provider: "oidc".to_string(),
            is_local: false,
            can_change_password: false,
            provider_profile: None,
            effective_permissions: Vec::new(),
            assigned_permission_set_refs: Vec::new(),
        },
        id_token: raw_id_token,
        token_response,
    })
}

pub async fn build_logout_redirect(
    state: &SharedState,
    headers: &HeaderMap,
) -> Result<OidcLogoutRedirect, ApiError> {
    let oidc = oidc_config(state)?;
    let discovery = fetch_discovery_document(&oidc).await?;
    let post_logout_redirect_uri = oidc
        .post_logout_redirect_uri
        .clone()
        .unwrap_or_else(|| "/login".to_string());

    let redirect_url = if let Some(end_session_endpoint) = discovery.end_session_endpoint {
        let mut url = Url::parse(&end_session_endpoint).map_err(|err| {
            ApiError::InternalServerError(format!("Invalid end_session_endpoint: {err}"))
        })?;
        {
            let mut pairs = url.query_pairs_mut();
            if let Some(id_token_hint) = get_cookie_value(headers, OIDC_ID_TOKEN_COOKIE_NAME) {
                pairs.append_pair("id_token_hint", &id_token_hint);
            }
            pairs.append_pair("post_logout_redirect_uri", &post_logout_redirect_uri);
            pairs.append_pair("client_id", oidc.client_id.as_deref().unwrap_or_default());
        }
        String::from(url)
    } else {
        post_logout_redirect_uri
    };

    Ok(OidcLogoutRedirect {
        redirect_url,
        cookies: clear_auth_cookies(state),
    })
}

pub fn clear_auth_cookies(state: &SharedState) -> Vec<Cookie<'static>> {
    [
        ACCESS_COOKIE_NAME,
        REFRESH_COOKIE_NAME,
        OIDC_ID_TOKEN_COOKIE_NAME,
        OIDC_STATE_COOKIE_NAME,
        OIDC_NONCE_COOKIE_NAME,
        OIDC_PKCE_COOKIE_NAME,
        OIDC_REDIRECT_COOKIE_NAME,
        OIDC_CLI_REDIRECT_COOKIE_NAME,
    ]
    .into_iter()
    .map(|name| remove_cookie(state, name))
    .collect()
}

pub fn build_auth_cookies(
    state: &SharedState,
    token_response: &TokenResponse,
    id_token: &str,
) -> Vec<Cookie<'static>> {
    let mut cookies = vec![
        build_cookie(
            state,
            ACCESS_COOKIE_NAME,
            token_response.access_token.clone(),
            state.jwt_config.access_token_expiration,
            true,
        ),
        build_cookie(
            state,
            REFRESH_COOKIE_NAME,
            token_response.refresh_token.clone(),
            state.jwt_config.refresh_token_expiration,
            true,
        ),
    ];

    if !id_token.is_empty() {
        cookies.push(build_cookie(
            state,
            OIDC_ID_TOKEN_COOKIE_NAME,
            id_token.to_string(),
            state.jwt_config.refresh_token_expiration,
            true,
        ));
    }

    cookies
}

pub fn apply_cookies_to_headers(
    headers: &mut HeaderMap,
    cookies: &[Cookie<'static>],
) -> Result<(), ApiError> {
    for cookie in cookies {
        let value = HeaderValue::from_str(&cookie.to_string()).map_err(|err| {
            ApiError::InternalServerError(format!("Failed to serialize cookie header: {err}"))
        })?;
        headers.append(header::SET_COOKIE, value);
    }
    Ok(())
}

pub fn oidc_callback_redirect_response(
    state: &SharedState,
    token_response: &TokenResponse,
    redirect_to: Option<String>,
    id_token: &str,
    cli_redirect_uri: Option<String>,
) -> Result<Response, ApiError> {
    // Always clear OIDC flow cookies regardless of redirect mode.
    let clear_cookies = [
        remove_cookie(state, OIDC_STATE_COOKIE_NAME),
        remove_cookie(state, OIDC_NONCE_COOKIE_NAME),
        remove_cookie(state, OIDC_PKCE_COOKIE_NAME),
        remove_cookie(state, OIDC_REDIRECT_COOKIE_NAME),
        remove_cookie(state, OIDC_CLI_REDIRECT_COOKIE_NAME),
    ];

    if let Some(cli_uri) = cli_redirect_uri {
        // CLI mode: serve an auto-submitting form that POSTs tokens to the local
        // callback server. This keeps tokens out of the browser URL bar and history
        // (unlike a 302 redirect with query params). Per RFC 8252 §7.3, browsers
        // correctly follow HTTPS → HTTP loopback redirects/form posts for native apps.
        let html = format!(
            r#"<!DOCTYPE html>
<html><head><title>Attune SSO</title></head>
<body>
<form id="f" method="POST" action="{}">
<input type="hidden" name="access_token" value="{}">
<input type="hidden" name="refresh_token" value="{}">
<input type="hidden" name="expires_in" value="{}">
</form>
<script>document.getElementById("f").submit();</script>
<noscript><p>JavaScript is required. Please enable it and try again.</p></noscript>
</body></html>"#,
            cli_uri,
            html_escape(&token_response.access_token),
            html_escape(&token_response.refresh_token),
            token_response.expires_in,
        );
        let mut response = (StatusCode::OK, axum::response::Html(html)).into_response();
        apply_cookies_to_headers(response.headers_mut(), &clear_cookies)?;
        return Ok(response);
    }

    // Web mode: redirect to the SPA callback page with tokens in the URL fragment.
    let redirect_target = sanitize_redirect_target(redirect_to.as_deref());
    let redirect_url = format!(
        "{LOGIN_CALLBACK_PATH}#access_token={}&refresh_token={}&expires_in={}&redirect_to={}",
        encode_fragment_value(&token_response.access_token),
        encode_fragment_value(&token_response.refresh_token),
        token_response.expires_in,
        encode_fragment_value(&redirect_target),
    );

    let mut response = Redirect::temporary(&redirect_url).into_response();
    let mut cookies = build_auth_cookies(state, token_response, id_token);
    cookies.push(remove_cookie(state, OIDC_STATE_COOKIE_NAME));
    cookies.push(remove_cookie(state, OIDC_NONCE_COOKIE_NAME));
    cookies.push(remove_cookie(state, OIDC_PKCE_COOKIE_NAME));
    cookies.push(remove_cookie(state, OIDC_REDIRECT_COOKIE_NAME));
    cookies.push(remove_cookie(state, OIDC_CLI_REDIRECT_COOKIE_NAME));
    apply_cookies_to_headers(response.headers_mut(), &cookies)?;
    Ok(response)
}

pub fn cookie_authenticated_user(
    headers: &HeaderMap,
    state: &SharedState,
) -> Result<Option<crate::auth::middleware::AuthenticatedUser>, ApiError> {
    let Some(token) = get_cookie_value(headers, ACCESS_COOKIE_NAME) else {
        return Ok(None);
    };

    let claims = validate_token(&token, &state.jwt_config).map_err(ApiError::from)?;
    Ok(Some(
        crate::auth::middleware::AuthenticatedUser::from_claims(claims),
    ))
}

pub fn get_cookie_value(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(';'))
        .filter_map(|part| {
            let mut pieces = part.trim().splitn(2, '=');
            let key = pieces.next()?.trim();
            let value = pieces.next()?.trim();
            if key == name {
                Some(value.to_string())
            } else {
                None
            }
        })
        .next()
}

pub fn has_oidc_session(headers: &HeaderMap) -> bool {
    get_cookie_value(headers, OIDC_ID_TOKEN_COOKIE_NAME)
        .is_some_and(|value| !value.trim().is_empty())
}

fn oidc_config(state: &SharedState) -> Result<OidcConfig, ApiError> {
    state
        .config
        .security
        .oidc
        .clone()
        .filter(|oidc| oidc.enabled)
        .ok_or_else(|| {
            ApiError::NotImplemented("OIDC authentication is not configured".to_string())
        })
}

fn oidc_http_client() -> &'static reqwest::Client {
    static OIDC_HTTP_CLIENT: LazyLock<reqwest::Client> = LazyLock::new(|| {
        reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .expect("OIDC HTTP client should build")
    });

    &OIDC_HTTP_CLIENT
}

async fn oidc_async_http_client(request: HttpRequest) -> Result<HttpResponse, OidcHttpClientError> {
    let (parts, body) = request.into_parts();
    let mut req = oidc_http_client()
        .request(parts.method, parts.uri.to_string())
        .body(body);
    for (name, value) in &parts.headers {
        req = req.header(name, value);
    }

    let response = req.send().await?;
    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        return Err(OidcHttpClientError::HttpStatus { status, body });
    }
    let headers = response.headers().clone();
    let body = response.bytes().await?.to_vec();

    let mut builder = axum::http::Response::builder().status(status);
    for (name, value) in &headers {
        builder = builder.header(name, value);
    }

    Ok(builder.body(body)?)
}

async fn fetch_discovery_document(oidc: &OidcConfig) -> Result<OidcDiscoveryDocument, ApiError> {
    let discovery_url = oidc.discovery_url.as_deref().unwrap_or_default();
    let discovery = reqwest::get(discovery_url).await.map_err(|err| {
        ApiError::InternalServerError(format!("Failed to fetch OIDC discovery document: {err}"))
    })?;

    if !discovery.status().is_success() {
        return Err(ApiError::InternalServerError(format!(
            "OIDC discovery request failed with status {}",
            discovery.status()
        )));
    }

    discovery
        .json::<OidcDiscoveryDocument>()
        .await
        .map_err(|err| {
            ApiError::InternalServerError(format!("Failed to parse OIDC discovery document: {err}"))
        })
}

async fn upsert_identity(
    state: &SharedState,
    oidc_claims: &OidcIdentityClaims,
) -> Result<attune_common::models::identity::Identity, ApiError> {
    let desired_login = derive_login(oidc_claims);
    let fallback_login = fallback_subject_login(oidc_claims);
    let display_name = derive_display_name(oidc_claims);
    let attributes = json!({
        "oidc": oidc_claims,
    });

    // Race-safe upsert keyed by (issuer, sub) with strict three-way match
    // on (issuer, sub, client_id). Concurrency is handled inside the
    // repository: the partial unique index `uq_identity_oidc_issuer_sub`
    // guarantees one identity per (issuer, sub), and the legacy upgrade
    // path uses a guarded UPDATE so only one concurrent caller wins.
    let identity = IdentityRepository::upsert_oidc_identity(
        &state.db,
        OidcUpsertInput {
            issuer: oidc_claims.issuer.clone(),
            sub: oidc_claims.sub.clone(),
            client_id: oidc_claims.client_id.clone(),
            desired_login,
            fallback_login,
            display_name,
            attributes,
        },
    )
    .await
    .map_err(ApiError::from)?;

    sync_roles(&state.db, identity.id, "oidc", &oidc_claims.groups).await?;
    Ok(identity)
}

async fn sync_roles(
    db: &sqlx::PgPool,
    identity_id: i64,
    source: &str,
    roles: &[String],
) -> Result<(), ApiError> {
    IdentityRoleAssignmentRepository::replace_managed_roles(db, identity_id, source, roles)
        .await
        .map_err(Into::into)
}

fn derive_login(oidc_claims: &OidcIdentityClaims) -> String {
    oidc_claims
        .email
        .clone()
        .or_else(|| oidc_claims.preferred_username.clone())
        .unwrap_or_else(|| fallback_subject_login(oidc_claims))
}

async fn verify_id_token(
    raw_id_token: &str,
    discovery: &OidcDiscoveryDocument,
    oidc: &OidcConfig,
    expected_nonce: &str,
) -> Result<VerifiedIdTokenClaims, ApiError> {
    let header = decode_header(raw_id_token).map_err(|err| {
        ApiError::Unauthorized(format!("OIDC ID token header decode failed: {err}"))
    })?;

    let algorithm = match header.alg {
        Algorithm::RS256 => Algorithm::RS256,
        Algorithm::RS384 => Algorithm::RS384,
        Algorithm::RS512 => Algorithm::RS512,
        other => {
            return Err(ApiError::Unauthorized(format!(
                "OIDC ID token uses unsupported signing algorithm: {other:?}"
            )))
        }
    };

    let jwks = reqwest::get(discovery.metadata.jwks_uri().url().as_str())
        .await
        .map_err(|err| ApiError::InternalServerError(format!("Failed to fetch OIDC JWKS: {err}")))?
        .json::<JwkSet>()
        .await
        .map_err(|err| {
            ApiError::InternalServerError(format!("Failed to parse OIDC JWKS: {err}"))
        })?;

    let jwk = jwks
        .keys
        .iter()
        .find(|jwk| {
            jwk.common.key_id == header.kid
                && matches!(
                    jwk.common.public_key_use,
                    Some(jsonwebtoken::jwk::PublicKeyUse::Signature)
                )
                && matches!(
                    jwk.algorithm,
                    AlgorithmParameters::RSA(_) | AlgorithmParameters::EllipticCurve(_)
                )
        })
        .ok_or_else(|| ApiError::Unauthorized("OIDC signing key not found in JWKS".to_string()))?;

    let decoding_key = DecodingKey::from_jwk(jwk)
        .map_err(|err| ApiError::Unauthorized(format!("OIDC JWK decode failed: {err}")))?;

    let issuer = discovery.metadata.issuer().to_string();
    let mut validation = Validation::new(algorithm);
    validation.set_issuer(&[issuer.as_str()]);
    validation.set_audience(&[oidc.client_id.as_deref().unwrap_or_default()]);
    validation.set_required_spec_claims(&["exp", "iat", "iss", "sub", "aud"]);
    validation.validate_nbf = false;

    let token = decode::<VerifiedIdTokenClaims>(raw_id_token, &decoding_key, &validation)
        .map_err(|err| ApiError::Unauthorized(format!("OIDC ID token validation failed: {err}")))?;

    if token.claims.nonce.as_deref() != Some(expected_nonce) {
        return Err(ApiError::Unauthorized(
            "OIDC nonce validation failed".to_string(),
        ));
    }

    Ok(token.claims)
}

fn derive_display_name(oidc_claims: &OidcIdentityClaims) -> Option<String> {
    oidc_claims
        .name
        .clone()
        .or_else(|| oidc_claims.preferred_username.clone())
        .or_else(|| oidc_claims.email.clone())
}

fn fallback_subject_login(oidc_claims: &OidcIdentityClaims) -> String {
    let mut hasher = Sha256::new();
    hasher.update(oidc_claims.issuer.as_bytes());
    hasher.update(b":");
    hasher.update(oidc_claims.sub.as_bytes());
    let digest = hex::encode(hasher.finalize());
    format!("oidc:{}", &digest[..24])
}

fn extract_groups_from_claims<T>(claims: &T) -> Vec<String>
where
    T: Serialize,
{
    let Ok(json) = serde_json::to_value(claims) else {
        return Vec::new();
    };
    match json.get("groups") {
        Some(JsonValue::Array(values)) => values
            .iter()
            .filter_map(|value| value.as_str().map(ToString::to_string))
            .collect(),
        Some(JsonValue::String(value)) => vec![value.to_string()],
        _ => Vec::new(),
    }
}

fn merge_userinfo_claims(oidc_claims: &mut OidcIdentityClaims, userinfo: &CoreUserInfoClaims) {
    if oidc_claims.email.is_none() {
        oidc_claims.email = userinfo.email().map(|email| email.as_str().to_string());
    }
    if oidc_claims.name.is_none() {
        oidc_claims.name = userinfo.name().and_then(first_localized_claim);
    }
    if oidc_claims.preferred_username.is_none() {
        oidc_claims.preferred_username = userinfo
            .preferred_username()
            .map(|username| username.as_str().to_string());
    }
    if oidc_claims.groups.is_empty() {
        oidc_claims.groups = extract_groups_from_claims(userinfo.additional_claims());
    }
}

fn first_localized_claim<T>(claim: &LocalizedClaim<T>) -> Option<String>
where
    T: std::ops::Deref<Target = String>,
{
    claim
        .iter()
        .next()
        .map(|(_, value)| value.as_str().to_string())
}

fn build_cookie(
    state: &SharedState,
    name: &'static str,
    value: String,
    max_age_seconds: i64,
    http_only: bool,
) -> Cookie<'static> {
    let mut cookie = Cookie::build((name, value))
        .path("/")
        .same_site(SameSite::Lax)
        .http_only(http_only)
        .max_age(CookieDuration::seconds(max_age_seconds))
        .build();

    if should_use_secure_cookies(state) {
        cookie.set_secure(true);
    }

    cookie
}

fn remove_cookie(state: &SharedState, name: &'static str) -> Cookie<'static> {
    let mut cookie = Cookie::build((name, String::new()))
        .path("/")
        .same_site(SameSite::Lax)
        .http_only(true)
        .max_age(CookieDuration::seconds(0))
        .build();
    cookie.make_removal();
    if should_use_secure_cookies(state) {
        cookie.set_secure(true);
    }
    cookie
}

fn should_use_secure_cookies(state: &SharedState) -> bool {
    state.config.is_production()
        || state
            .config
            .security
            .oidc
            .as_ref()
            .and_then(|oidc| oidc.redirect_uri.as_deref())
            .map(|uri| uri.starts_with("https://"))
            .unwrap_or(false)
}

fn sanitize_redirect_target(redirect_to: Option<&str>) -> String {
    let fallback = "/".to_string();
    let Some(redirect_to) = redirect_to else {
        return fallback;
    };
    if redirect_to.starts_with('/') && !redirect_to.starts_with("//") {
        redirect_to.to_string()
    } else {
        fallback
    }
}

pub fn unauthorized_redirect(location: &str) -> Response {
    let mut response = Redirect::to(location).into_response();
    *response.status_mut() = StatusCode::FOUND;
    response
}

fn encode_fragment_value(value: &str) -> String {
    byte_serialize(value.as_bytes()).collect()
}

/// Minimal HTML attribute escaping for token values embedded in hidden form fields.
fn html_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::header;
    use axum::http::HeaderValue;

    #[test]
    fn sanitize_redirect_target_rejects_external_urls() {
        assert_eq!(sanitize_redirect_target(Some("https://example.com")), "/");
        assert_eq!(sanitize_redirect_target(Some("//example.com")), "/");
        assert_eq!(
            sanitize_redirect_target(Some("/executions/42")),
            "/executions/42"
        );
    }

    #[test]
    fn extract_groups_from_claims_accepts_array_and_string() {
        let array_claims = serde_json::json!({ "groups": ["admins", "operators"] });
        let string_claims = serde_json::json!({ "groups": "admins" });

        assert_eq!(
            extract_groups_from_claims(&array_claims),
            vec!["admins".to_string(), "operators".to_string()]
        );
        assert_eq!(
            extract_groups_from_claims(&string_claims),
            vec!["admins".to_string()]
        );
    }

    #[test]
    fn has_oidc_session_requires_non_empty_id_token_cookie() {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::COOKIE,
            HeaderValue::from_static("other=value; attune_oidc_id_token=oidc-token"),
        );
        assert!(has_oidc_session(&headers));

        let mut empty_cookie_headers = HeaderMap::new();
        empty_cookie_headers.insert(
            header::COOKIE,
            HeaderValue::from_static("attune_oidc_id_token=   ; other=value"),
        );
        assert!(!has_oidc_session(&empty_cookie_headers));

        let headers_without_cookie = HeaderMap::new();
        assert!(!has_oidc_session(&headers_without_cookie));
    }
}
