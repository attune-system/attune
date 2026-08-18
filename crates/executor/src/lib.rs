//! Attune Executor Service Library
//!
//! This library exposes internal modules for testing purposes.
//! The actual executor service is a binary in main.rs.

pub mod completion_listener;
pub mod dead_letter_handler;
pub mod enforcement_processor;
pub mod event_processor;
pub mod execution_manager;
pub mod inquiry_handler;
pub mod pack_test_processor;
pub mod policy_enforcer;
pub mod queue_dispatcher;
pub mod queue_manager;
pub mod retry_manager;
pub mod scheduler;
pub mod service;
pub mod timeout_monitor;
pub mod work_queue_events;
pub mod worker_health;
pub mod workflow;

// Re-export commonly used types for convenience
pub use dead_letter_handler::{create_dlq_consumer_config, DeadLetterHandler};
pub use inquiry_handler::{InquiryHandler, InquiryRequest, INQUIRY_RESULT_KEY};
pub use policy_enforcer::{
    ExecutionPolicy, PolicyEnforcer, PolicyScope, PolicyViolation, RateLimit,
};
pub use queue_manager::{ExecutionQueueManager, QueueConfig, QueueStats};
pub use retry_manager::{RetryAnalysis, RetryConfig, RetryManager, RetryReason};
pub use timeout_monitor::{ExecutionTimeoutMonitor, TimeoutMonitorConfig};
pub use worker_health::{HealthMetrics, HealthProbeConfig, HealthStatus, WorkerHealthProbe};
