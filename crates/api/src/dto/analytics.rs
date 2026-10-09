//! Analytics DTOs for API requests and responses
//!
//! Analytics over retained records, grouped into UTC hours without refresh delay.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

use super::dashboard::DashboardFreshnessMode;
use attune_common::repositories::analytics::{
    AnalyticsTimeRange, EnforcementVolumeBucket, EventVolumeBucket, ExecutionStatusBucket,
    ExecutionThroughputBucket, FailureRateSummary, WorkerStatusBucket,
};
use attune_common::repositories::native_maintenance::read::{
    AnalyticsRead, ReadMetadata, ReadRange,
};

/// Coverage describes only this read's source-time bounds, including ledger holes.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct AnalyticsReadMetadata {
    pub mode: DashboardFreshnessMode,
    pub summary_ranges: Vec<AnalyticsReadRange>,
    pub raw_ranges: Vec<AnalyticsReadRange>,
    /// Oldest refresh actually used. This does not guarantee global coverage.
    pub oldest_refresh: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct AnalyticsReadRange {
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
}

impl From<ReadRange> for AnalyticsReadRange {
    fn from(range: ReadRange) -> Self {
        Self {
            start: range.start,
            end: range.end,
        }
    }
}

impl From<ReadMetadata> for AnalyticsReadMetadata {
    fn from(meta: ReadMetadata) -> Self {
        Self {
            mode: meta.mode.into(),
            summary_ranges: meta.summary_ranges.into_iter().map(Into::into).collect(),
            raw_ranges: meta.raw_ranges.into_iter().map(Into::into).collect(),
            oldest_refresh: meta.oldest_refresh,
        }
    }
}

// ---------------------------------------------------------------------------
// Query parameters
// ---------------------------------------------------------------------------

/// Common query parameters for analytics endpoints.
#[derive(Debug, Clone, Deserialize, IntoParams)]
pub struct AnalyticsQueryParams {
    /// Inclusive earliest UTC bucket start (ISO 8601). Defaults to 24 hours ago.
    #[param(example = "2026-02-25T00:00:00Z")]
    pub since: Option<DateTime<Utc>>,

    /// Inclusive latest UTC bucket start (ISO 8601). Defaults to now.
    #[param(example = "2026-02-26T00:00:00Z")]
    pub until: Option<DateTime<Utc>>,

    /// Number of hours to look back from now (alternative to since/until).
    /// Ignored if `since` is provided.
    #[param(example = 24, minimum = 1, maximum = 8760)]
    pub hours: Option<i64>,
}

impl AnalyticsQueryParams {
    /// Convert to the repository-level time range.
    pub fn to_time_range(&self) -> AnalyticsTimeRange {
        match (&self.since, &self.until) {
            (Some(since), Some(until)) => AnalyticsTimeRange {
                since: *since,
                until: *until,
            },
            (Some(since), None) => AnalyticsTimeRange {
                since: *since,
                until: Utc::now(),
            },
            (None, Some(until)) => {
                let hours = self.hours.unwrap_or(24).clamp(1, 8760);
                AnalyticsTimeRange {
                    since: *until - chrono::Duration::hours(hours),
                    until: *until,
                }
            }
            (None, None) => {
                let hours = self.hours.unwrap_or(24).clamp(1, 8760);
                AnalyticsTimeRange::last_hours(hours)
            }
        }
    }
}

/// Path parameter for filtering analytics by a specific entity ref.
#[derive(Debug, Clone, Deserialize, IntoParams)]
pub struct AnalyticsRefParam {
    /// Optional entity ref filter (action_ref, trigger_ref, rule_ref, or worker name)
    #[param(example = "core.http_request")]
    pub entity_ref: Option<String>,
}

// ---------------------------------------------------------------------------
// Response types
// ---------------------------------------------------------------------------

/// A single data point in an hourly time series.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct TimeSeriesPoint {
    /// Start of the 1-hour bucket (ISO 8601)
    #[schema(example = "2026-02-26T10:00:00Z")]
    pub bucket: DateTime<Utc>,

    /// The series label (e.g., status name, action ref). Null for aggregate totals.
    #[schema(example = "completed")]
    pub label: Option<String>,

    /// The count value for this bucket
    #[schema(example = 42)]
    pub value: i64,
}

/// Response for execution status transitions over time.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct ExecutionStatusTimeSeriesResponse {
    pub read_coverage: AnalyticsReadMetadata,
    /// Time range start
    pub since: DateTime<Utc>,
    /// Time range end
    pub until: DateTime<Utc>,
    /// Data points: one per (bucket, status) pair
    pub data: Vec<TimeSeriesPoint>,
}

/// Response for execution throughput over time.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct ExecutionThroughputResponse {
    pub read_coverage: AnalyticsReadMetadata,
    /// Time range start
    pub since: DateTime<Utc>,
    /// Time range end
    pub until: DateTime<Utc>,
    /// Data points: one per bucket (total executions created)
    pub data: Vec<TimeSeriesPoint>,
}

/// Response for event volume over time.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct EventVolumeResponse {
    pub read_coverage: AnalyticsReadMetadata,
    /// Time range start
    pub since: DateTime<Utc>,
    /// Time range end
    pub until: DateTime<Utc>,
    /// Data points: one per bucket (total events created)
    pub data: Vec<TimeSeriesPoint>,
}

/// Response for worker status transitions over time.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct WorkerStatusTimeSeriesResponse {
    pub read_coverage: AnalyticsReadMetadata,
    /// Time range start
    pub since: DateTime<Utc>,
    /// Time range end
    pub until: DateTime<Utc>,
    /// Data points: one per (bucket, status) pair
    pub data: Vec<TimeSeriesPoint>,
}

/// Response for enforcement volume over time.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct EnforcementVolumeResponse {
    /// Time range start
    pub since: DateTime<Utc>,
    /// Time range end
    pub until: DateTime<Utc>,
    /// Data points: one per bucket (total enforcements created)
    pub data: Vec<TimeSeriesPoint>,
}

/// Response for the execution failure rate summary.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct FailureRateResponse {
    pub read_coverage: AnalyticsReadMetadata,
    /// Time range start
    pub since: DateTime<Utc>,
    /// Time range end
    pub until: DateTime<Utc>,
    /// Total transitions to completed, failed, or timeout in the included hours
    #[schema(example = 100)]
    pub total_terminal: i64,
    /// Number of transitions to failed, including retry attempts
    #[schema(example = 12)]
    pub failed_count: i64,
    /// Number of transitions to timeout, including retry attempts
    #[schema(example = 3)]
    pub timeout_count: i64,
    /// Number of transitions to completed
    #[schema(example = 85)]
    pub completed_count: i64,
    /// Failure rate as a percentage (0.0 – 100.0)
    #[schema(example = 15.0)]
    pub failure_rate_pct: f64,
}

/// Combined dashboard analytics response.
///
/// Returns all key metrics in a single response for the dashboard page,
/// avoiding multiple round-trips.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct DashboardAnalyticsResponse {
    /// Separate per-metric coverage, because sources can have different holes.
    pub read_coverage: DashboardAnalyticsCoverage,
    /// Time range start
    pub since: DateTime<Utc>,
    /// Time range end
    pub until: DateTime<Utc>,
    /// Execution throughput per hour
    pub execution_throughput: Vec<TimeSeriesPoint>,
    /// Execution status transitions per hour
    pub execution_status: Vec<TimeSeriesPoint>,
    /// Event volume per hour
    pub event_volume: Vec<TimeSeriesPoint>,
    /// Enforcement volume per hour
    pub enforcement_volume: Vec<TimeSeriesPoint>,
    /// Worker status transitions per hour
    pub worker_status: Vec<TimeSeriesPoint>,
    /// Execution failure rate summary
    pub failure_rate: FailureRateResponse,
}

// ---------------------------------------------------------------------------
// Conversion helpers
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct DashboardAnalyticsCoverage {
    pub execution_throughput: AnalyticsReadMetadata,
    pub execution_status: AnalyticsReadMetadata,
    pub event_volume: AnalyticsReadMetadata,
    pub worker_status: AnalyticsReadMetadata,
}

impl From<ExecutionStatusBucket> for TimeSeriesPoint {
    fn from(b: ExecutionStatusBucket) -> Self {
        Self {
            bucket: b.bucket,
            label: b.new_status,
            value: b.transition_count,
        }
    }
}

impl From<ExecutionThroughputBucket> for TimeSeriesPoint {
    fn from(b: ExecutionThroughputBucket) -> Self {
        Self {
            bucket: b.bucket,
            label: b.action_ref,
            value: b.execution_count,
        }
    }
}

impl From<EventVolumeBucket> for TimeSeriesPoint {
    fn from(b: EventVolumeBucket) -> Self {
        Self {
            bucket: b.bucket,
            label: b.trigger_ref,
            value: b.event_count,
        }
    }
}

impl From<WorkerStatusBucket> for TimeSeriesPoint {
    fn from(b: WorkerStatusBucket) -> Self {
        Self {
            bucket: b.bucket,
            label: b.new_status,
            value: b.transition_count,
        }
    }
}

impl From<EnforcementVolumeBucket> for TimeSeriesPoint {
    fn from(b: EnforcementVolumeBucket) -> Self {
        Self {
            bucket: b.bucket,
            label: b.rule_ref,
            value: b.enforcement_count,
        }
    }
}

impl FailureRateResponse {
    /// Create from the repository summary plus the query time range.
    pub fn from_summary(
        read: AnalyticsRead<FailureRateSummary>,
        range: &AnalyticsTimeRange,
    ) -> Self {
        let summary = read.data;
        Self {
            read_coverage: read.metadata.into(),
            since: range.since,
            until: range.until,
            total_terminal: summary.total_terminal,
            failed_count: summary.failed_count,
            timeout_count: summary.timeout_count,
            completed_count: summary.completed_count,
            failure_rate_pct: summary.failure_rate_pct,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_query_params_defaults() {
        let params = AnalyticsQueryParams {
            since: None,
            until: None,
            hours: None,
        };
        let range = params.to_time_range();
        let diff = range.until - range.since;
        assert!((diff.num_hours() - 24).abs() <= 1);
    }

    #[test]
    fn test_query_params_custom_hours() {
        let params = AnalyticsQueryParams {
            since: None,
            until: None,
            hours: Some(6),
        };
        let range = params.to_time_range();
        let diff = range.until - range.since;
        assert!((diff.num_hours() - 6).abs() <= 1);
    }

    #[test]
    fn test_query_params_hours_clamped() {
        let params = AnalyticsQueryParams {
            since: None,
            until: None,
            hours: Some(99999),
        };
        let range = params.to_time_range();
        let diff = range.until - range.since;
        // Clamped to 8760 hours (1 year)
        assert!((diff.num_hours() - 8760).abs() <= 1);
    }

    #[test]
    fn test_query_params_explicit_range() {
        let since = Utc::now() - chrono::Duration::hours(48);
        let until = Utc::now();
        let params = AnalyticsQueryParams {
            since: Some(since),
            until: Some(until),
            hours: Some(6), // ignored when since is provided
        };
        let range = params.to_time_range();
        assert_eq!(range.since, since);
        assert_eq!(range.until, until);
    }

    #[test]
    fn test_failure_rate_response_from_summary() {
        let summary = FailureRateSummary {
            total_terminal: 100,
            failed_count: 12,
            timeout_count: 3,
            completed_count: 85,
            failure_rate_pct: 15.0,
        };
        let range = AnalyticsTimeRange::last_hours(24);
        let response = FailureRateResponse::from_summary(
            AnalyticsRead {
                data: summary,
                metadata: ReadMetadata {
                    mode: attune_common::repositories::native_maintenance::read::ReadMode::RawOnly,
                    summary_ranges: vec![],
                    raw_ranges: vec![],
                    oldest_refresh: None,
                },
            },
            &range,
        );
        assert_eq!(response.total_terminal, 100);
        assert_eq!(response.failed_count, 12);
        assert_eq!(response.failure_rate_pct, 15.0);
    }

    #[test]
    fn test_time_series_point_from_execution_status_bucket() {
        let bucket = ExecutionStatusBucket {
            bucket: Utc::now(),
            action_ref: Some("core.http".into()),
            new_status: Some("completed".into()),
            transition_count: 10,
        };
        let point: TimeSeriesPoint = bucket.into();
        assert_eq!(point.label.as_deref(), Some("completed"));
        assert_eq!(point.value, 10);
    }

    #[test]
    fn test_time_series_point_from_event_volume_bucket() {
        let bucket = EventVolumeBucket {
            bucket: Utc::now(),
            trigger_ref: Some("core.timer".into()),
            event_count: 25,
        };
        let point: TimeSeriesPoint = bucket.into();
        assert_eq!(point.label.as_deref(), Some("core.timer"));
        assert_eq!(point.value, 25);
    }
}
