//! LDAP authentication helpers for username/password login.

use attune_common::{
    config::LdapConfig,
    repositories::{
        identity::{
            CreateIdentityInput, IdentityRepository, IdentityRoleAssignmentRepository,
            UpdateIdentityInput,
        },
        Create, Update,
    },
};
use ldap3::{dn_escape, ldap_escape, Ldap, LdapConnAsync, LdapConnSettings, Scope, SearchEntry};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};

use crate::{dto::TokenResponse, middleware::error::ApiError, state::SharedState};

/// Claims extracted from the LDAP directory for an authenticated user.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LdapUserClaims {
    /// The LDAP server URL the user was authenticated against.
    pub server_url: String,
    /// The user's full distinguished name.
    pub dn: String,
    /// Login attribute value (uid, sAMAccountName, etc.).
    pub login: Option<String>,
    /// Email address.
    pub email: Option<String>,
    /// Display name (cn).
    pub display_name: Option<String>,
    /// Group memberships (memberOf values).
    pub groups: Vec<String>,
}

/// The result of a successful LDAP authentication.
#[derive(Debug, Clone)]
pub struct LdapAuthenticatedIdentity {
    pub token_response: TokenResponse,
}

/// Authenticate a user against the configured LDAP directory.
///
/// This performs a bind (either direct or search+bind) to verify
/// the user's credentials, then fetches their attributes and upserts
/// the identity in the database.
pub async fn authenticate(
    state: &SharedState,
    login: &str,
    password: &str,
) -> Result<LdapAuthenticatedIdentity, ApiError> {
    let ldap_config = ldap_config(state)?;

    // Connect and authenticate
    let claims = if ldap_config.bind_dn_template.is_some() {
        direct_bind(&ldap_config, login, password).await?
    } else {
        search_and_bind(&ldap_config, login, password).await?
    };

    // Upsert identity in DB and issue JWT tokens
    let identity = upsert_identity(state, &claims).await?;
    if identity.frozen {
        return Err(ApiError::Forbidden(
            "Identity is frozen and cannot authenticate".to_string(),
        ));
    }
    let token_response = crate::auth::session::issue(state, &identity)
        .await?
        .with_user(
            identity.id,
            identity.login.clone(),
            identity.display_name.clone(),
        );

    Ok(LdapAuthenticatedIdentity { token_response })
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

fn ldap_config(state: &SharedState) -> Result<LdapConfig, ApiError> {
    let config = state
        .config
        .security
        .ldap
        .clone()
        .filter(|ldap| ldap.enabled)
        .ok_or_else(|| {
            ApiError::NotImplemented("LDAP authentication is not configured".to_string())
        })?;

    // Reject partial service-account configuration: having exactly one of
    // search_bind_dn / search_bind_password is almost certainly a config
    // error and would silently fall back to anonymous search, which is a
    // very different security posture than the admin intended.
    let has_dn = config.search_bind_dn.is_some();
    let has_pw = config.search_bind_password.is_some();
    if has_dn != has_pw {
        let missing = if has_dn {
            "search_bind_password"
        } else {
            "search_bind_dn"
        };
        return Err(ApiError::InternalServerError(format!(
            "LDAP misconfiguration: search_bind_dn and search_bind_password must both be set \
             or both be omitted (missing {missing})"
        )));
    }

    Ok(config)
}

/// Build an `LdapConnSettings` from the config.
fn conn_settings(config: &LdapConfig) -> LdapConnSettings {
    let mut settings = LdapConnSettings::new();
    if config.starttls {
        settings = settings.set_starttls(true);
    }
    if config.danger_skip_tls_verify {
        settings = settings.set_no_tls_verify(true);
    }
    settings
}

/// Open a new LDAP connection.
async fn connect(config: &LdapConfig) -> Result<Ldap, ApiError> {
    let settings = conn_settings(config);
    let url = config.url.as_deref().unwrap_or_default();
    let (conn, ldap) = LdapConnAsync::with_settings(settings, url)
        .await
        .map_err(|err| {
            ApiError::InternalServerError(format!("Failed to connect to LDAP server: {err}"))
        })?;
    // Drive the connection in the background
    ldap3::drive!(conn);
    Ok(ldap)
}

/// Direct-bind authentication: construct the DN from the template and bind.
async fn direct_bind(
    config: &LdapConfig,
    login: &str,
    password: &str,
) -> Result<LdapUserClaims, ApiError> {
    let template = config.bind_dn_template.as_deref().unwrap_or_default();
    // Escape the login value for safe interpolation into a Distinguished Name
    // (RFC 4514). Without this, characters like `,`, `+`, `"`, `\`, `<`, `>`,
    // `;`, `=`, NUL, `#` (leading), or space (leading/trailing) in the username
    // would alter the DN structure.
    let escaped_login = dn_escape(login);
    let bind_dn = template.replace("{login}", &escaped_login);

    let mut ldap = connect(config).await?;

    // Bind as the user
    let result = ldap
        .simple_bind(&bind_dn, password)
        .await
        .map_err(|err| ApiError::InternalServerError(format!("LDAP bind failed: {err}")))?;

    if result.rc != 0 {
        let _ = ldap.unbind().await;
        return Err(ApiError::Unauthorized(
            "Invalid LDAP credentials".to_string(),
        ));
    }

    // Fetch user attributes
    let claims = fetch_user_attributes(config, &mut ldap, &bind_dn).await?;

    let _ = ldap.unbind().await;
    Ok(claims)
}

/// Search-and-bind authentication:
/// 1. Bind as the service account (or anonymous)
/// 2. Search for the user entry (must match exactly one)
/// 3. Re-bind as the user with their DN + password
async fn search_and_bind(
    config: &LdapConfig,
    login: &str,
    password: &str,
) -> Result<LdapUserClaims, ApiError> {
    let search_base = config.user_search_base.as_deref().ok_or_else(|| {
        ApiError::InternalServerError(
            "LDAP user_search_base is required when bind_dn_template is not set".to_string(),
        )
    })?;

    let mut ldap = connect(config).await?;

    // Step 1: Bind as service account or anonymous.
    // Partial config (only one of dn/password) is already rejected by
    // ldap_config(), so this match is exhaustive over valid states.
    if let (Some(bind_dn), Some(bind_pw)) = (
        config.search_bind_dn.as_deref(),
        config.search_bind_password.as_deref(),
    ) {
        let result = ldap.simple_bind(bind_dn, bind_pw).await.map_err(|err| {
            ApiError::InternalServerError(format!("LDAP service bind failed: {err}"))
        })?;
        if result.rc != 0 {
            let _ = ldap.unbind().await;
            return Err(ApiError::InternalServerError(
                "LDAP service account bind failed — check search_bind_dn and search_bind_password"
                    .to_string(),
            ));
        }
    }
    // If no service account, we proceed with an anonymous connection (already connected)

    // Step 2: Search for the user.
    // Escape the login value for safe interpolation into an LDAP search filter
    // (RFC 4515). Without this, characters like `(`, `)`, `*`, `\`, and NUL in
    // the username could broaden the filter, match unintended entries, or break
    // the search entirely.
    let escaped_login = ldap_escape(login);
    let filter = config.user_filter.replace("{login}", &escaped_login);
    let attrs = vec![
        config.login_attr.as_str(),
        config.email_attr.as_str(),
        config.display_name_attr.as_str(),
        config.group_attr.as_str(),
        "dn",
    ];

    let (results, _result) = ldap
        .search(search_base, Scope::Subtree, &filter, attrs)
        .await
        .map_err(|err| ApiError::InternalServerError(format!("LDAP user search failed: {err}")))?
        .success()
        .map_err(|err| ApiError::InternalServerError(format!("LDAP search error: {err}")))?;

    // The search must return exactly one entry. Zero means the user was not
    // found; more than one means the filter or directory layout is ambiguous
    // and we must not guess which identity to authenticate.
    let result_count = results.len();
    if result_count == 0 {
        let _ = ldap.unbind().await;
        return Err(ApiError::Unauthorized(
            "Invalid LDAP credentials".to_string(),
        ));
    }
    if result_count > 1 {
        let _ = ldap.unbind().await;
        return Err(ApiError::InternalServerError(format!(
            "LDAP user search returned {result_count} entries (expected exactly 1) — \
             tighten the user_filter or user_search_base to ensure uniqueness"
        )));
    }

    // SAFETY: result_count == 1 guaranteed by the checks above.
    let entry = results
        .into_iter()
        .next()
        .expect("checked result_count == 1");
    let search_entry = SearchEntry::construct(entry);
    let user_dn = search_entry.dn.clone();

    // Step 3: Re-bind as the user
    let result = ldap
        .simple_bind(&user_dn, password)
        .await
        .map_err(|err| ApiError::InternalServerError(format!("LDAP user bind failed: {err}")))?;
    if result.rc != 0 {
        let _ = ldap.unbind().await;
        return Err(ApiError::Unauthorized(
            "Invalid LDAP credentials".to_string(),
        ));
    }

    let claims = extract_claims(config, &search_entry);
    let _ = ldap.unbind().await;
    Ok(claims)
}

/// Fetch the user's LDAP attributes after a successful bind.
async fn fetch_user_attributes(
    config: &LdapConfig,
    ldap: &mut Ldap,
    user_dn: &str,
) -> Result<LdapUserClaims, ApiError> {
    let attrs = vec![
        config.login_attr.as_str(),
        config.email_attr.as_str(),
        config.display_name_attr.as_str(),
        config.group_attr.as_str(),
    ];

    let (results, _result) = ldap
        .search(user_dn, Scope::Base, "(objectClass=*)", attrs)
        .await
        .map_err(|err| {
            ApiError::InternalServerError(format!(
                "LDAP attribute fetch failed for DN {user_dn}: {err}"
            ))
        })?
        .success()
        .map_err(|err| {
            ApiError::InternalServerError(format!("LDAP attribute search error: {err}"))
        })?;

    let entry = results.into_iter().next().ok_or_else(|| {
        ApiError::InternalServerError(format!("LDAP entry not found for DN: {user_dn}"))
    })?;
    let search_entry = SearchEntry::construct(entry);

    Ok(extract_claims(config, &search_entry))
}

/// Extract user claims from an LDAP search entry.
fn extract_claims(config: &LdapConfig, entry: &SearchEntry) -> LdapUserClaims {
    let first_attr =
        |name: &str| -> Option<String> { entry.attrs.get(name).and_then(|v| v.first()).cloned() };

    let groups = entry
        .attrs
        .get(&config.group_attr)
        .cloned()
        .unwrap_or_default();

    LdapUserClaims {
        server_url: config.url.clone().unwrap_or_default(),
        dn: entry.dn.clone(),
        login: first_attr(&config.login_attr),
        email: first_attr(&config.email_attr),
        display_name: first_attr(&config.display_name_attr),
        groups,
    }
}

/// Upsert an identity row for the LDAP-authenticated user.
async fn upsert_identity(
    state: &SharedState,
    claims: &LdapUserClaims,
) -> Result<attune_common::models::identity::Identity, ApiError> {
    let existing =
        IdentityRepository::find_by_ldap_dn(&state.db, &claims.server_url, &claims.dn).await?;
    let desired_login = derive_login(claims);
    let display_name = claims.display_name.clone();
    let attributes = json!({ "ldap": claims });

    match existing {
        Some(identity) => {
            let updated = UpdateIdentityInput {
                display_name,
                password_hash: None,
                attributes: Some(attributes),
                frozen: None,
            };
            let identity = IdentityRepository::update(&state.db, identity.id, updated)
                .await
                .map_err(ApiError::from)?;
            sync_roles(&state.db, identity.id, "ldap", &claims.groups).await?;
            Ok(identity)
        }
        None => {
            // Avoid login collisions
            let login = match IdentityRepository::find_by_login(&state.db, &desired_login).await? {
                Some(_) => fallback_dn_login(claims),
                None => desired_login,
            };

            let identity = IdentityRepository::create(
                &state.db,
                CreateIdentityInput {
                    login,
                    display_name,
                    password_hash: None,
                    attributes,
                },
            )
            .await
            .map_err(ApiError::from)?;
            sync_roles(&state.db, identity.id, "ldap", &claims.groups).await?;
            Ok(identity)
        }
    }
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

/// Derive the login name from LDAP claims.
fn derive_login(claims: &LdapUserClaims) -> String {
    claims
        .login
        .clone()
        .or_else(|| claims.email.clone())
        .unwrap_or_else(|| fallback_dn_login(claims))
}

/// Generate a deterministic fallback login from the LDAP server URL + DN.
fn fallback_dn_login(claims: &LdapUserClaims) -> String {
    let mut hasher = Sha256::new();
    hasher.update(claims.server_url.as_bytes());
    hasher.update(b":");
    hasher.update(claims.dn.as_bytes());
    let digest = hex::encode(hasher.finalize());
    format!("ldap:{}", &digest[..24])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn direct_bind_dn_escapes_special_characters() {
        // Simulate what direct_bind does with the template
        let template = "uid={login},ou=users,dc=example,dc=com";
        let malicious_login = "admin,ou=admins,dc=evil,dc=com";
        let escaped = dn_escape(malicious_login);
        let bind_dn = template.replace("{login}", &escaped);
        // The commas in the login value must be escaped so they don't
        // introduce additional RDN components.
        assert!(
            bind_dn.contains("\\2c"),
            "commas in login must be escaped in DN: {bind_dn}"
        );
        assert!(
            bind_dn.starts_with("uid=admin\\2cou\\3dadmins\\2cdc\\3devil\\2cdc\\3dcom,ou=users"),
            "DN structure must be preserved: {bind_dn}"
        );
    }

    #[test]
    fn search_filter_escapes_special_characters() {
        let filter_template = "(uid={login})";
        let malicious_login = "admin)(|(uid=*))";
        let escaped = ldap_escape(malicious_login);
        let filter = filter_template.replace("{login}", &escaped);
        // The parentheses and asterisk must be escaped so they don't
        // alter the filter structure.
        assert!(
            !filter.contains(")("),
            "parentheses in login must be escaped in filter: {filter}"
        );
        assert!(
            filter.contains("\\28"),
            "open-paren must be hex-escaped: {filter}"
        );
        assert!(
            filter.contains("\\29"),
            "close-paren must be hex-escaped: {filter}"
        );
        assert!(
            filter.contains("\\2a"),
            "asterisk must be hex-escaped: {filter}"
        );
    }

    #[test]
    fn dn_escape_preserves_safe_usernames() {
        let safe = "jdoe";
        let escaped = dn_escape(safe);
        assert_eq!(escaped.as_ref(), "jdoe");
    }

    #[test]
    fn filter_escape_preserves_safe_usernames() {
        let safe = "jdoe";
        let escaped = ldap_escape(safe);
        assert_eq!(escaped.as_ref(), "jdoe");
    }

    #[test]
    fn fallback_dn_login_is_deterministic() {
        let claims = LdapUserClaims {
            server_url: "ldap://ldap.example.com".to_string(),
            dn: "uid=test,ou=users,dc=example,dc=com".to_string(),
            login: None,
            email: None,
            display_name: None,
            groups: vec![],
        };
        let a = fallback_dn_login(&claims);
        let b = fallback_dn_login(&claims);
        assert_eq!(a, b);
        assert!(a.starts_with("ldap:"));
        assert_eq!(a.len(), "ldap:".len() + 24);
    }
}
