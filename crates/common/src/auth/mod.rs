//! Authentication primitives shared across Attune services.
//!
//! This module provides JWT token types, generation, and validation functions
//! that are used by the API (for all token types), the worker (for execution-scoped
//! tokens), and the sensor service (for sensor tokens).

pub mod crypto_provider;
pub mod integration_token;
pub mod jwt;
pub mod worker_token;

pub use crypto_provider::install as install_crypto_provider;
pub use integration_token::{
    generate_integration_token, hash_integration_token, token_display_prefix, token_display_suffix,
    GeneratedIntegrationToken, INTEGRATION_TOKEN_PREFIX,
};
pub use jwt::{
    extract_token_from_header, generate_access_token, generate_execution_token,
    generate_execution_token_with_permission_sets, generate_integration_refresh_token,
    generate_refresh_token, generate_sensor_token, generate_token, validate_token, Claims,
    JwtConfig, JwtError, TokenType,
};
pub use worker_token::WorkerTokenProvider;
