//! Authentication and authorization module

pub mod jwt;
pub mod ldap;
pub mod middleware;
pub mod oidc;
pub mod oidc_device;
pub mod password;

pub use jwt::{generate_token, validate_token, Claims};
pub use middleware::{AuthMiddleware, RequireAuth};
pub use password::{hash_password, verify_password};
