//! Attune API Service Library
//!
//! This library provides the core components of the Attune API service,
//! including the server, routing, authentication, and state management.
//! It is primarily used by the binary target and integration tests.

pub mod auth;
pub mod authz;
pub mod dashboard_data;
pub mod dto;
pub mod inquiry_timeout;
pub mod middleware;
pub mod openapi;
pub mod postgres_listener;
pub mod routes;
pub mod server;
pub mod state;
pub mod validation;
pub mod webhook_security;

// Re-export commonly used items for convenience
pub use server::Server;
pub use state::AppState;
