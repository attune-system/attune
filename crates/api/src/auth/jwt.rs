//! JWT token generation and validation
//!
//! This module re-exports all JWT functionality from `attune_common::auth::jwt`.
//! The canonical implementation lives in the common crate so that all services
//! (API, worker, sensor) share the same token types and signing logic.

pub use attune_common::auth::jwt::{
    extract_token_from_header, generate_access_token, generate_execution_token,
    generate_integration_refresh_token, generate_refresh_token, generate_sensor_token,
    generate_sensor_token_with_cache_authority, generate_token, validate_token, Claims, JwtConfig,
    JwtError, TokenType,
};
