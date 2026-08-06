//! Common utilities, models, and database layer for Attune services
//!
//! This crate provides shared functionality used across all Attune services including:
//! - Database models and schema
//! - Error types
//! - Configuration
//! - Utilities

pub mod action_visibility;
pub mod agent_bootstrap;
pub mod agent_runtime_detection;
pub mod artifact_transport;
pub mod audit;
pub mod auth;
pub mod config;
pub mod crypto;
pub mod dashboard_spec;
pub mod db;
pub mod error;
pub mod metadata_cache;
pub mod models;
pub mod mq;
pub mod observability;
mod pack_cache_definition;
pub mod pack_check;
pub mod pack_environment;
pub mod pack_registry;
pub mod pack_transport;
mod policy_control;
pub mod queue_definition;
pub mod rbac;
pub mod repositories;
pub mod runtime_detection;
pub mod scheduling;
pub mod schema;
pub mod secret_values;
pub mod system_alert;
pub mod template_resolver;
pub mod test_executor;
pub mod trace_tag;
pub mod utils;
pub mod version_matching;
pub mod workflow;

// Re-export commonly used types
pub use error::{Error, Result};
pub use template_resolver::{resolve_templates, TemplateContext};

/// Library version
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_version() {
        assert!(!VERSION.is_empty());
    }
}
