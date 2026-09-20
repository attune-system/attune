//! Workflow orchestration module
//!
//! This module provides workflow execution context, graph building, and
//! orchestration capabilities for the Attune workflow engine.
//!
//! # Modules
//!
//! - `context`: Manage workflow execution context and variables
//! - `graph`: Build executable task graphs from workflow definitions

pub mod context;
pub mod graph;
pub mod log;
pub mod task_wait;
