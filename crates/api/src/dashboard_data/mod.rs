//! Dashboard data source planning primitives.
//!
//! This module implements source contracts/registry, strict query-safety helpers,
//! and watermark cutover planning used by dashboard data endpoints.

pub mod contracts;
pub mod planner;
pub mod query_safety;
pub mod watermark;

pub use contracts::{
    AuthorizationBasis, FreshnessMode, ParamSchema, SourceAvailability, SourceContract, SourceType,
};
pub use planner::{PlanError, SourcePlanner, SourcePlanningStatus};
pub use query_safety::{
    ActionResultPathAllowList, BoundedLimit, QuerySafetyError, SafeQueryBindings, SafeRef,
    TypedBindValue,
};
pub use watermark::{BucketCountRow, TimeRange, WatermarkCutoverPlan};
