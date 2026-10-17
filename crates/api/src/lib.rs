//! Attune API Service Library
//!
//! This library provides the core components of the Attune API service,
//! including the server, routing, authentication, and state management.
//! It is primarily used by the binary target and integration tests.

mod artifact_reads;
pub mod auth;
pub mod authz;
pub mod dashboard_data;
mod disclosure_records;
pub mod dto;
pub mod execution_log_streams;
mod file_origins;
pub mod file_transfers;
mod http_range;
pub mod inquiry_response;
pub mod inquiry_timeout;
mod live_logs;
mod log_decoder;
pub mod log_stream_wakeups;
pub mod middleware;
pub mod openapi;
pub mod pack_release_upgrade;
pub mod postgres_listener;
mod receipt_disclosure;
pub mod recorded_consumers;
mod recorded_disclosure;
pub mod recorded_effect;
mod recorded_files;
mod recorded_reads;
pub mod routes;
pub mod secret_disclosure;
pub(crate) mod sensor_origin_iam;
pub mod server;
pub mod state;
pub mod validation;
pub mod webhook_security;

// Re-export commonly used items for convenience
pub use server::Server;
pub use state::AppState;
