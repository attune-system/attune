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
            ordering: vec!["ref"],
            response_shape: "object",
            notes: Some("Requires keys:decrypt when encrypted values are requested."),
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
            ordering: vec!["action_ref", "execution_id"],
            response_shape: "array",
            notes: Some("Latest terminal execution result per action."),
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
            ordering: vec!["action_ref", "execution_id"],
            response_shape: "array",
            notes: Some("Path extraction must use an allow-list."),
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
            ordering: vec!["bucket_start", "series", "ref"],
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
            ordering: vec!["bucket_start", "status", "action_ref"],
            response_shape: "array",
            notes: Some("Defaults to terminal outcome breakdown."),
        },
        SourceContract {
            source_type: ExecutionDurationStats,
            availability: Partial,
            authorization_basis: Executions,
            default_freshness_mode: RawOnly,
            param_schema: ParamSchema {
                required: vec![],
                optional: vec!["action_ref", "pack_ref", "bucket_size"],
            },
            ordering: vec!["bucket_start", "series"],
            response_shape: "array",
            notes: Some("Partial: canonical terminal timestamp contract pending."),
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
            ordering: vec!["action_ref"],
            response_shape: "array",
            notes: Some("Defaults to latest terminal execution."),
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
            ordering: vec!["bucket_start", "trigger_ref"],
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
            notes: None,
        },
        SourceContract {
            source_type: EnforcementCount,
            availability: Partial,
            authorization_basis: Enforcements,
            default_freshness_mode: RawOnly,
            param_schema: ParamSchema {
                required: vec![],
                optional: vec!["rule_ref", "pack_ref", "bucket_size"],
            },
            ordering: vec!["bucket_start", "rule_ref"],
            response_shape: "array",
            notes: Some("Partial: hourly aggregate strategy not finalized."),
        },
        SourceContract {
            source_type: EnforcementTimeseries,
            availability: Partial,
            authorization_basis: Enforcements,
            default_freshness_mode: RawOnly,
            param_schema: ParamSchema {
                required: vec![],
                optional: vec!["rule_ref", "pack_ref", "bucket_size"],
            },
            ordering: vec!["bucket_start", "series"],
            response_shape: "array",
            notes: Some("Partial/high-cost until materialized aggregate exists."),
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
            notes: None,
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
            availability: Planned,
            authorization_basis: QueueItems,
            default_freshness_mode: RawOnly,
            param_schema: ParamSchema::empty(),
            ordering: vec!["bucket_start", "queue_ref"],
            response_shape: "array",
            notes: Some("Planned: requires queue transition history/aggregate."),
        },
        SourceContract {
            source_type: QueueDispatchStats,
            availability: Partial,
            authorization_basis: Queues,
            default_freshness_mode: RawOnly,
            param_schema: ParamSchema {
                required: vec![],
                optional: vec!["queue_ref", "pack_ref", "bucket_size"],
            },
            ordering: vec!["bucket_start", "queue_ref", "status"],
            response_shape: "array",
            notes: Some("Partial: retention/trend strategy still evolving."),
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
            ordering: vec!["assigned_to", "pack_ref"],
            response_shape: "array",
            notes: None,
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
            ordering: vec!["bucket_start", "series"],
            response_shape: "array",
            notes: None,
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
            notes: None,
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
