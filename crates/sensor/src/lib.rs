//! Attune Sensor Service Library
//!
//! This library provides the core functionality for the Attune Sensor Service,
//! including event generation, rule matching, and template resolution.

pub mod api_client;
pub mod manager_sensor;
pub mod rule_lifecycle_listener;
pub mod sensor_log;
pub mod sensor_manager;
pub mod sensor_worker_registration;
pub mod service;
pub mod startup;

// Re-export template resolver from common crate
pub mod template_resolver {
    pub use attune_common::template_resolver::*;
}

// Re-export commonly used types
pub use rule_lifecycle_listener::RuleLifecycleListener;
pub use sensor_worker_registration::SensorWorkerRegistration;
pub use service::SensorService;
pub use template_resolver::{resolve_templates, TemplateContext};
