use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum SourceAvailability {
    AvailableNow,
    Partial,
    Planned,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum FreshnessMode {
    RawOnly,
    AggregateOnly,
    AggregatePlusTail,
    RawOnlyFallback,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum AuthorizationBasis {
    Keys,
    Executions,
    Events,
    Enforcements,
    Queues,
    QueueItems,
    Inquiries,
    Workers,
    Sensors,
    Dashboards,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, ToSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum SourceType {
    KeyValue,
    LatestActionResult,
    ActionResultPath,
    ExecutionCount,
    ExecutionTimeseries,
    ExecutionStatusBreakdown,
    ExecutionDurationStats,
    LastExecution,
    EventCount,
    EventTimeseries,
    LastEvent,
    EnforcementCount,
    EnforcementTimeseries,
    LastEnforcement,
    QueueBacklog,
    QueueThroughput,
    QueueDispatchStats,
    InquiryBacklog,
    InquirySla,
    WorkerHealth,
    SensorHealth,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ParamSchema {
    pub required: Vec<&'static str>,
    pub optional: Vec<&'static str>,
}

impl ParamSchema {
    pub fn empty() -> Self {
        Self {
            required: Vec::new(),
            optional: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SourceContract {
    pub source_type: SourceType,
    pub availability: SourceAvailability,
    pub authorization_basis: AuthorizationBasis,
    pub default_freshness_mode: FreshnessMode,
    pub param_schema: ParamSchema,
    pub ordering: Vec<&'static str>,
    pub response_shape: &'static str,
    pub notes: Option<&'static str>,
}

pub fn default_source_contracts() -> BTreeMap<SourceType, SourceContract> {
    use AuthorizationBasis::*;
    use FreshnessMode::*;
    use SourceAvailability::*;
    use SourceType::*;

    [
        SourceContract {
            source_type: KeyValue,
            availability: AvailableNow,
            authorization_basis: Keys,
            default_freshness_mode: RawOnly,
            param_schema: ParamSchema {
                required: vec!["ref"],
                optional: vec!["owner_type", "owner_ref", "decrypt"],
            },
            ordering: vec!["ref", "name", "value", "owner_type", "owner_ref", "updated_at"],
            response_shape: "object",
            notes: Some("Returns one key payload; encrypted values stay null unless decrypt=true and keys:decrypt is allowed."),
        },
        SourceContract {
            source_type: LatestActionResult,
            availability: AvailableNow,
            authorization_basis: Executions,
            default_freshness_mode: RawOnly,
            param_schema: ParamSchema {
                required: vec![],
                optional: vec!["action_ref", "pack_ref", "status"],
            },
            ordering: vec!["action_ref", "execution_id", "status", "updated_at", "result"],
            response_shape: "array",
            notes: Some("Latest execution result row per action; defaults to terminal statuses."),
        },
        SourceContract {
            source_type: ActionResultPath,
            availability: AvailableNow,
            authorization_basis: Executions,
            default_freshness_mode: RawOnly,
            param_schema: ParamSchema {
                required: vec!["path"],
                optional: vec!["action_ref", "pack_ref"],
            },
            ordering: vec!["action_ref", "execution_id", "status", "updated_at", "path", "value"],
            response_shape: "array",
            notes: Some("Extracts an allow-listed dot path from the latest terminal result per action."),
        },
        SourceContract {
            source_type: ExecutionCount,
            availability: AvailableNow,
            authorization_basis: Executions,
            default_freshness_mode: AggregatePlusTail,
            param_schema: ParamSchema {
                required: vec![],
                optional: vec!["action_ref", "pack_ref", "status", "bucket_size"],
            },
            ordering: vec!["bucket_start", "series"],
            response_shape: "array",
            notes: Some("Counts terminal outcomes by default semantics."),
        },
        SourceContract {
            source_type: ExecutionTimeseries,
            availability: AvailableNow,
            authorization_basis: Executions,
            default_freshness_mode: AggregatePlusTail,
            param_schema: ParamSchema {
                required: vec![],
                optional: vec!["action_ref", "pack_ref", "status", "bucket_size"],
            },
            ordering: vec!["bucket_start", "series"],
            response_shape: "array",
            notes: Some("Same semantics as execution_count."),
        },
        SourceContract {
            source_type: ExecutionStatusBreakdown,
            availability: AvailableNow,
            authorization_basis: Executions,
            default_freshness_mode: AggregatePlusTail,
            param_schema: ParamSchema {
                required: vec![],
                optional: vec![
                    "action_ref",
                    "pack_ref",
                    "mode",
                    "include_cancelled",
                    "bucket_size",
                ],
            },
            ordering: vec!["bucket_start", "status"],
            response_shape: "array",
            notes: Some("Defaults to terminal outcome breakdown."),
        },
        SourceContract {
            source_type: ExecutionDurationStats,
            availability: AvailableNow,
            authorization_basis: Executions,
            default_freshness_mode: RawOnly,
            param_schema: ParamSchema {
                required: vec![],
                optional: vec!["action_ref", "pack_ref", "bucket_size"],
            },
            ordering: vec!["bucket_start", "series"],
            response_shape: "array",
            notes: Some(
                "Hourly terminal execution duration stats grouped by action_ref series; duration uses updated - started_at and bucket_size is currently fixed to 1h.",
            ),
        },
        SourceContract {
            source_type: LastExecution,
            availability: AvailableNow,
            authorization_basis: Executions,
            default_freshness_mode: RawOnly,
            param_schema: ParamSchema {
                required: vec![],
                optional: vec!["action_ref", "pack_ref", "include_in_flight"],
            },
            ordering: vec![
                "action_ref",
                "execution_id",
                "status",
                "created_at",
                "started_at",
                "updated_at",
                "trace_tag",
                "result",
            ],
            response_shape: "array",
            notes: Some("Latest execution snapshot per action; include_in_flight=true widens beyond terminal statuses."),
        },
        SourceContract {
            source_type: EventCount,
            availability: AvailableNow,
            authorization_basis: Events,
            default_freshness_mode: AggregatePlusTail,
            param_schema: ParamSchema {
                required: vec![],
                optional: vec!["trigger_ref", "pack_ref", "bucket_size"],
            },
            ordering: vec!["bucket_start", "series"],
            response_shape: "array",
            notes: None,
        },
        SourceContract {
            source_type: EventTimeseries,
            availability: AvailableNow,
            authorization_basis: Events,
            default_freshness_mode: AggregatePlusTail,
            param_schema: ParamSchema {
                required: vec![],
                optional: vec!["trigger_ref", "pack_ref", "bucket_size"],
            },
            ordering: vec!["bucket_start", "series"],
            response_shape: "array",
            notes: None,
        },
        SourceContract {
            source_type: LastEvent,
            availability: AvailableNow,
            authorization_basis: Events,
            default_freshness_mode: RawOnly,
            param_schema: ParamSchema {
                required: vec![],
                optional: vec!["trigger_ref", "pack_ref"],
            },
            ordering: vec!["trigger_ref", "event_id"],
            response_shape: "array",
            notes: Some("Latest event in the requested time range per trigger."),
        },
        SourceContract {
            source_type: EnforcementCount,
            availability: AvailableNow,
            authorization_basis: Enforcements,
            default_freshness_mode: RawOnly,
            param_schema: ParamSchema {
                required: vec![],
                optional: vec!["rule_ref", "pack_ref", "bucket_size"],
            },
            ordering: vec!["bucket_start", "series"],
            response_shape: "array",
            notes: Some("Hourly terminal enforcement counts grouped by rule."),
        },
        SourceContract {
            source_type: EnforcementTimeseries,
            availability: AvailableNow,
            authorization_basis: Enforcements,
            default_freshness_mode: RawOnly,
            param_schema: ParamSchema {
                required: vec![],
                optional: vec!["rule_ref", "pack_ref", "bucket_size"],
            },
            ordering: vec!["bucket_start", "series"],
            response_shape: "array",
            notes: Some("Hourly terminal enforcement timeseries grouped by rule."),
        },
        SourceContract {
            source_type: LastEnforcement,
            availability: AvailableNow,
            authorization_basis: Enforcements,
            default_freshness_mode: RawOnly,
            param_schema: ParamSchema {
                required: vec![],
                optional: vec!["rule_ref", "pack_ref"],
            },
            ordering: vec!["rule_ref", "enforcement_id"],
            response_shape: "array",
            notes: Some("Latest enforcement in the requested time range per rule."),
        },
        SourceContract {
            source_type: QueueBacklog,
            availability: AvailableNow,
            authorization_basis: QueueItems,
            default_freshness_mode: RawOnly,
            param_schema: ParamSchema {
                required: vec![],
                optional: vec!["queue_ref", "pack_ref"],
            },
            ordering: vec!["queue_ref"],
            response_shape: "array",
            notes: Some("Snapshot over queued/retry/leased statuses."),
        },
        SourceContract {
            source_type: QueueThroughput,
            availability: AvailableNow,
            authorization_basis: QueueItems,
            default_freshness_mode: RawOnly,
            param_schema: ParamSchema {
                required: vec![],
                optional: vec!["queue_ref", "pack_ref"],
            },
            ordering: vec!["bucket_start", "queue_ref"],
            response_shape: "array",
            notes: Some("Hourly terminal queue-item throughput grouped by queue."),
        },
        SourceContract {
            source_type: QueueDispatchStats,
            availability: AvailableNow,
            authorization_basis: Queues,
            default_freshness_mode: RawOnly,
            param_schema: ParamSchema {
                required: vec![],
                optional: vec!["queue_ref", "pack_ref"],
            },
            ordering: vec!["bucket_start", "queue_ref", "status"],
            response_shape: "array",
            notes: Some("Hourly terminal dispatch execution outcomes grouped by queue and status."),
        },
        SourceContract {
            source_type: InquiryBacklog,
            availability: AvailableNow,
            authorization_basis: Inquiries,
            default_freshness_mode: RawOnly,
            param_schema: ParamSchema {
                required: vec![],
                optional: vec!["assigned_to", "pack_ref"],
            },
            ordering: vec!["pack_ref", "assigned_to"],
            response_shape: "array",
            notes: Some(
                "Snapshot of pending inquiries grouped by pack and assignee; overdue_count uses timeout_at < now().",
            ),
        },
        SourceContract {
            source_type: InquirySla,
            availability: AvailableNow,
            authorization_basis: Inquiries,
            default_freshness_mode: RawOnly,
            param_schema: ParamSchema {
                required: vec![],
                optional: vec![
                    "assigned_to",
                    "pack_ref",
                    "sla_target_seconds",
                    "bucket_size",
                ],
            },
            ordering: vec!["bucket_start", "pack_ref", "assigned_to"],
            response_shape: "array",
            notes: Some(
                "Hourly inquiry SLA cohorts grouped by pack and assignee; pending inquiries use current age and bucket_size is currently fixed to 1h.",
            ),
        },
        SourceContract {
            source_type: WorkerHealth,
            availability: AvailableNow,
            authorization_basis: Workers,
            default_freshness_mode: RawOnly,
            param_schema: ParamSchema {
                required: vec![],
                optional: vec!["worker_role", "status", "history", "bucket_size"],
            },
            ordering: vec!["worker_role", "worker_id"],
            response_shape: "array",
            notes: Some("History mode can use aggregate_plus_tail where configured."),
        },
        SourceContract {
            source_type: SensorHealth,
            availability: AvailableNow,
            authorization_basis: Sensors,
            default_freshness_mode: RawOnly,
            param_schema: ParamSchema {
                required: vec![],
                optional: vec!["sensor_ref", "worker_id", "window"],
            },
            ordering: vec!["sensor_ref", "worker_id"],
            response_shape: "array",
            notes: Some(
                "Latest durable sensor-process state per sensor/worker; window filters by recent updates.",
            ),
        },
    ]
    .into_iter()
    .map(|c| (c.source_type, c))
    .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_contracts_have_unique_source_types() {
        let contracts = default_source_contracts();
        assert!(!contracts.is_empty());
        assert_eq!(contracts.len(), 21);
    }

    #[test]
    fn all_partial_sources_have_notes() {
        let contracts = default_source_contracts();
        let partial_without_notes = contracts
            .values()
            .filter(|c| c.availability == SourceAvailability::Partial && c.notes.is_none())
            .count();
        assert_eq!(partial_without_notes, 0);
    }
}
