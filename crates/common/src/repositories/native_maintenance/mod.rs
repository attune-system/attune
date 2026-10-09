//! Typed identifiers shared by partition maintenance, materialization and reads.
//! Names are internal constants. Never interpolate an operator-supplied identifier.

use serde::{Deserialize, Serialize};

pub mod partitions;
pub mod read;
pub mod schedule;
pub mod summaries;

/// Daily UTC RANGE parents managed by the supervisor.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, sqlx::Type, Serialize, Deserialize, utoipa::ToSchema,
)]
#[sqlx(type_name = "native_partition_parent", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum ManagedTable {
    Event,
    ExecutionHistory,
    AuditEvent,
}

impl ManagedTable {
    pub const ALL: [Self; 3] = [Self::Event, Self::ExecutionHistory, Self::AuditEvent];

    /// Unqualified source relation, resolved through the connection search_path.
    pub const fn table_name(self) -> &'static str {
        match self {
            Self::Event => "event",
            Self::ExecutionHistory => "execution_history",
            Self::AuditEvent => "audit_event",
        }
    }

    /// Non-null timestamp used for half-open UTC partition bounds.
    pub const fn time_column(self) -> &'static str {
        match self {
            Self::ExecutionHistory => "time",
            Self::Event | Self::AuditEvent => "created",
        }
    }

    /// The permanent fallback leaf. It is never eligible for partition expiry.
    pub const fn default_name(self) -> &'static str {
        match self {
            Self::Event => "event_default",
            Self::ExecutionHistory => "execution_history_default",
            Self::AuditEvent => "audit_event_default",
        }
    }
}

/// Independent hourly materializations. Enum order is also the state-lock order.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, sqlx::Type, Serialize, Deserialize, utoipa::ToSchema,
)]
#[sqlx(type_name = "native_summary_kind", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum SummaryKind {
    ExecutionStatus,
    ExecutionCreation,
    EventVolume,
    WorkerStatus,
}

impl SummaryKind {
    pub const ALL: [Self; 4] = [
        Self::ExecutionStatus,
        Self::ExecutionCreation,
        Self::EventVolume,
        Self::WorkerStatus,
    ];

    /// Source parent to lock before reading data or locking builder state.
    pub const fn source_table(self) -> &'static str {
        match self {
            Self::ExecutionStatus | Self::ExecutionCreation => "execution_history",
            Self::EventVolume => "event",
            Self::WorkerStatus => "worker_history",
        }
    }

    /// Timestamp grouped into complete UTC hours.
    pub const fn time_column(self) -> &'static str {
        match self {
            Self::EventVolume => "created",
            _ => "time",
        }
    }

    /// Ordinary persisted count table, distinct from the raw hourly views.
    pub const fn summary_table(self) -> &'static str {
        match self {
            Self::ExecutionStatus => "execution_status_hourly_summary",
            Self::ExecutionCreation => "execution_creation_hourly_summary",
            Self::EventVolume => "event_volume_hourly_summary",
            Self::WorkerStatus => "worker_status_hourly_summary",
        }
    }
}
