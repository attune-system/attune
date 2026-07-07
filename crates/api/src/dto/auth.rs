//! Authentication DTOs

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use validator::Validate;

/// Login request
#[derive(Debug, Clone, Serialize, Deserialize, Validate, ToSchema)]
pub struct LoginRequest {
    /// Identity login (username)
    #[validate(length(min = 1, max = 255))]
    #[schema(example = "admin")]
    pub login: String,

    /// Password
    #[validate(length(min = 1))]
    #[schema(example = "changeme123")]
    pub password: String,
}

/// Passwordless integration-token login request.
#[derive(Debug, Clone, Serialize, Deserialize, Validate, ToSchema)]
pub struct TokenLoginRequest {
    /// Opaque integration token secret. Returned only once when an administrator creates the token.
    #[validate(length(min = 1))]
    #[schema(example = "attune_it_abc123...")]
    pub token: String,
}

/// Register request
#[derive(Debug, Clone, Serialize, Deserialize, Validate, ToSchema)]
pub struct RegisterRequest {
    /// Identity login (username)
    #[validate(length(min = 3, max = 255))]
    #[schema(example = "newuser")]
    pub login: String,

    /// Password
    #[validate(length(min = 8, max = 128))]
    #[schema(example = "SecurePass123!")]
    pub password: String,

    /// Display name (optional)
    #[validate(length(max = 255))]
    #[schema(example = "New User")]
    pub display_name: Option<String>,
}

/// Token response
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct TokenResponse {
    /// Access token (JWT)
    #[schema(example = "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9...")]
    pub access_token: String,

    /// Refresh token
    #[schema(example = "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9...")]
    pub refresh_token: String,

    /// Token type (always "Bearer")
    #[schema(example = "Bearer")]
    pub token_type: String,

    /// Access token expiration in seconds
    #[schema(example = 3600)]
    pub expires_in: i64,

    /// User information
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user: Option<UserInfo>,
}

/// User information included in token response
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct UserInfo {
    /// Identity ID
    #[schema(example = 1)]
    pub id: i64,

    /// Identity login
    #[schema(example = "admin")]
    pub login: String,

    /// Display name
    #[schema(example = "Administrator")]
    pub display_name: Option<String>,
}

impl TokenResponse {
    pub fn new(access_token: String, refresh_token: String, expires_in: i64) -> Self {
        Self {
            access_token,
            refresh_token,
            token_type: "Bearer".to_string(),
            expires_in,
            user: None,
        }
    }

    pub fn with_user(mut self, id: i64, login: String, display_name: Option<String>) -> Self {
        self.user = Some(UserInfo {
            id,
            login,
            display_name,
        });
        self
    }
}

/// Refresh token request
#[derive(Debug, Clone, Serialize, Deserialize, Validate, ToSchema)]
pub struct RefreshTokenRequest {
    /// Refresh token
    #[validate(length(min = 1))]
    #[schema(example = "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9...")]
    pub refresh_token: String,
}

/// Change password request
#[derive(Debug, Clone, Serialize, Deserialize, Validate, ToSchema)]
pub struct ChangePasswordRequest {
    /// Current password
    #[validate(length(min = 1))]
    #[schema(example = "OldPassword123!")]
    pub current_password: String,

    /// New password
    #[validate(length(min = 8, max = 128))]
    #[schema(example = "NewPassword456!")]
    pub new_password: String,
}

/// Update current user profile request
#[derive(Debug, Clone, Serialize, Deserialize, Validate, ToSchema)]
pub struct UpdateCurrentUserRequest {
    /// Display name. Empty or whitespace-only values clear the display name.
    #[validate(length(max = 255))]
    #[schema(example = "Jane Operator")]
    pub display_name: Option<String>,
}

/// Current user response
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct CurrentUserResponse {
    /// Identity ID
    #[schema(example = 1)]
    pub id: i64,

    /// Identity login
    #[schema(example = "admin")]
    pub login: String,

    /// Display name
    #[schema(example = "Administrator")]
    pub display_name: Option<String>,

    /// Authentication provider backing this identity.
    #[schema(example = "local")]
    pub auth_provider: String,

    /// Whether this identity is managed locally by Attune.
    #[schema(example = true)]
    pub is_local: bool,

    /// Whether this identity can change its password through Attune.
    #[schema(example = true)]
    pub can_change_password: bool,

    /// Sanitized user information supplied by the external identity provider.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_profile: Option<ProviderProfileResponse>,

    /// Effective resource-level permissions assigned to this identity.
    pub effective_permissions: Vec<EffectivePermissionResponse>,

    /// Permission set refs assigned to this identity, including role-derived assignments.
    pub assigned_permission_set_refs: Vec<String>,
}

/// Effective permissions assigned to an identity.
///
/// Each entry corresponds to one effective grant and can include optional
/// constraints when the grant is scoped (for example to specific packs or refs).
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct EffectivePermissionResponse {
    /// RBAC resource name.
    #[schema(example = "queues")]
    pub resource: String,

    /// Actions allowed for the resource.
    #[schema(example = json!(["read", "update"]))]
    pub actions: Vec<String>,

    /// Optional grant constraints describing permission scope granularity.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(example = json!({"pack_refs": ["core"]}), value_type = Object, nullable = true)]
    pub constraints: Option<serde_json::Value>,
}

/// Sanitized user information supplied by an external identity provider.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ProviderProfileResponse {
    /// Provider backing this identity.
    #[schema(example = "oidc")]
    pub provider: String,

    /// Provider-issued display name.
    #[schema(example = "Jane Operator")]
    pub display_name: Option<String>,

    /// Provider-issued login or preferred username.
    #[schema(example = "jane.operator")]
    pub login: Option<String>,

    /// Provider-issued email address.
    #[schema(example = "jane.operator@example.com")]
    pub email: Option<String>,

    /// Whether the provider reported the email address as verified.
    #[schema(example = true)]
    pub email_verified: Option<bool>,

    /// OIDC subject identifier, when available.
    #[schema(example = "00u123456789")]
    pub subject: Option<String>,

    /// OIDC issuer URL, when available.
    #[schema(example = "https://idp.example.com")]
    pub issuer: Option<String>,

    /// LDAP distinguished name, when available.
    #[schema(example = "uid=jane,ou=people,dc=example,dc=com")]
    pub distinguished_name: Option<String>,

    /// Provider groups associated with this identity.
    pub groups: Vec<String>,
}

/// Public authentication settings for the login page.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct AuthSettingsResponse {
    /// Whether authentication is enabled for the server.
    #[schema(example = true)]
    pub authentication_enabled: bool,

    /// Whether local username/password login is configured.
    #[schema(example = true)]
    pub local_password_enabled: bool,

    /// Whether local username/password login should be shown by default.
    #[schema(example = true)]
    pub local_password_visible_by_default: bool,

    /// Whether OIDC login is configured and enabled.
    #[schema(example = false)]
    pub oidc_enabled: bool,

    /// Whether OIDC login should be shown by default.
    #[schema(example = false)]
    pub oidc_visible_by_default: bool,

    /// Provider name for `?auth=<provider>`.
    #[schema(example = "sso")]
    pub oidc_provider_name: Option<String>,

    /// User-facing provider label for the login button.
    #[schema(example = "Example SSO")]
    pub oidc_provider_label: Option<String>,

    /// Optional icon URL shown beside the provider label.
    #[schema(example = "https://auth.example.com/assets/logo.svg")]
    pub oidc_provider_icon_url: Option<String>,

    /// Whether LDAP login is configured and enabled.
    #[schema(example = false)]
    pub ldap_enabled: bool,

    /// Whether LDAP login should be shown by default.
    #[schema(example = false)]
    pub ldap_visible_by_default: bool,

    /// Provider name for `?auth=<provider>`.
    #[schema(example = "ldap")]
    pub ldap_provider_name: Option<String>,

    /// User-facing provider label for the login button.
    #[schema(example = "Company LDAP")]
    pub ldap_provider_label: Option<String>,

    /// Optional icon URL shown beside the provider label.
    #[schema(example = "https://ldap.example.com/assets/logo.svg")]
    pub ldap_provider_icon_url: Option<String>,

    /// Whether unauthenticated self-service registration is allowed.
    #[schema(example = false)]
    pub self_registration_enabled: bool,
}
