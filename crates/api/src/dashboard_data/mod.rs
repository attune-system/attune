//! Dashboard data source planning primitives.
//!
//! Source contracts, registry, and query-safety helpers for dashboard endpoints.

pub mod contracts;
pub mod planner;
pub mod query_safety;

pub use contracts::{
    AuthorizationBasis, FreshnessMode, ParamSchema, SourceAvailability, SourceContract, SourceType,
};
pub use planner::{PlanError, SourcePlanner, SourcePlanningStatus};
pub use query_safety::{
    ActionResultPathAllowList, BoundedLimit, QuerySafetyError, SafeQueryBindings, SafeRef,
    TypedBindValue,
};
