//! Trace report DTOs.

use chrono::{DateTime, Utc};
use serde::Serialize;
use utoipa::ToSchema;

use attune_common::models::{enums::WorkQueueDispatchStatus, work_queue::WorkQueueDispatch, Id};

use crate::dto::{
    event::EventSummary, execution::ExecutionSummary, work_queue::WorkQueueItemResponse,
};

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct TraceWorkQueueDispatchSummary {
    #[schema(example = 1)]
    pub id: Id,
    #[schema(example = 1)]
    pub queue: Id,
    #[schema(example = "core.my_queue")]
    pub queue_ref: String,
    #[schema(example = 123)]
    pub execution: Id,
    pub status: WorkQueueDispatchStatus,
    #[schema(example = 5)]
    pub leased_item_count: i32,
    #[schema(example = "2024-01-13T10:30:00Z")]
    pub created: DateTime<Utc>,
    #[schema(example = "2024-01-13T10:31:00Z")]
    pub updated: DateTime<Utc>,
}

impl From<WorkQueueDispatch> for TraceWorkQueueDispatchSummary {
    fn from(dispatch: WorkQueueDispatch) -> Self {
        Self {
            id: dispatch.id,
            queue: dispatch.queue,
            queue_ref: dispatch.queue_ref,
            execution: dispatch.execution,
            status: dispatch.status,
            leased_item_count: dispatch.leased_item_count,
            created: dispatch.created,
            updated: dispatch.updated,
        }
    }
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct TraceEnforcementSummary {
    #[schema(example = 1)]
    pub id: Id,
    #[schema(example = 1)]
    pub rule: Option<Id>,
    #[schema(example = "core.on_timer")]
    pub rule_ref: String,
    #[schema(example = "core.timer")]
    pub trigger_ref: String,
    #[schema(example = 123, nullable = true)]
    pub event: Option<Id>,
    pub status: attune_common::models::enums::EnforcementStatus,
    pub condition: attune_common::models::enums::EnforcementCondition,
    #[schema(example = "2024-01-13T10:30:00Z")]
    pub created: DateTime<Utc>,
    #[schema(example = "2024-01-13T10:31:00Z", nullable = true)]
    pub resolved_at: Option<DateTime<Utc>>,
}

impl From<attune_common::models::Enforcement> for TraceEnforcementSummary {
    fn from(enforcement: attune_common::models::Enforcement) -> Self {
        Self {
            id: enforcement.id,
            rule: enforcement.rule,
            rule_ref: enforcement.rule_ref,
            trigger_ref: enforcement.trigger_ref,
            event: enforcement.event,
            status: enforcement.status,
            condition: enforcement.condition,
            created: enforcement.created,
            resolved_at: enforcement.resolved_at,
        }
    }
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct TraceReportResponse {
    #[schema(example = "core.timer.1234")]
    pub trace_tag: String,
    #[schema(example = json!(["event", "work_queue_item"]))]
    pub origins: Vec<String>,
    pub executions: Vec<ExecutionSummary>,
    pub enforcements: Vec<TraceEnforcementSummary>,
    pub events: Vec<EventSummary>,
    pub queue_dispatches: Vec<TraceWorkQueueDispatchSummary>,
    pub queue_items: Vec<WorkQueueItemResponse>,
}
