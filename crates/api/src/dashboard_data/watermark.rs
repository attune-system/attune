use std::collections::BTreeMap;

use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

use crate::dashboard_data::contracts::FreshnessMode;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum WatermarkError {
    #[error("time range must satisfy start < end")]
    InvalidRange,
    #[error("bucket size must be positive")]
    InvalidBucketSize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimeRange {
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
}

impl TimeRange {
    pub fn new(start: DateTime<Utc>, end: DateTime<Utc>) -> Result<Self, WatermarkError> {
        if start >= end {
            return Err(WatermarkError::InvalidRange);
        }
        Ok(Self { start, end })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WatermarkCutoverPlan {
    pub request_range: TimeRange,
    pub aggregate_watermark: Option<DateTime<Utc>>,
    pub freshness_mode: FreshnessMode,
    pub aggregate_range: Option<TimeRange>,
    pub raw_range: Option<TimeRange>,
}

impl WatermarkCutoverPlan {
    pub fn build(
        request_range: TimeRange,
        aggregate_watermark: Option<DateTime<Utc>>,
    ) -> Result<Self, WatermarkError> {
        let plan = match aggregate_watermark {
            None => Self {
                request_range,
                aggregate_watermark: None,
                freshness_mode: FreshnessMode::RawOnlyFallback,
                aggregate_range: None,
                raw_range: Some(request_range),
            },
            Some(watermark) if watermark <= request_range.start => Self {
                request_range,
                aggregate_watermark: Some(watermark),
                freshness_mode: FreshnessMode::RawOnly,
                aggregate_range: None,
                raw_range: Some(request_range),
            },
            Some(watermark) if watermark >= request_range.end => Self {
                request_range,
                aggregate_watermark: Some(watermark),
                freshness_mode: FreshnessMode::AggregateOnly,
                aggregate_range: Some(request_range),
                raw_range: None,
            },
            Some(watermark) => {
                let aggregate_range = TimeRange::new(request_range.start, watermark)?;
                let raw_range = TimeRange::new(watermark, request_range.end)?;
                Self {
                    request_range,
                    aggregate_watermark: Some(watermark),
                    freshness_mode: FreshnessMode::AggregatePlusTail,
                    aggregate_range: Some(aggregate_range),
                    raw_range: Some(raw_range),
                }
            }
        };
        Ok(plan)
    }

    pub fn aggregate_bucket_in_range(
        &self,
        bucket_start: DateTime<Utc>,
        bucket_size: Duration,
    ) -> Result<bool, WatermarkError> {
        if bucket_size <= Duration::zero() {
            return Err(WatermarkError::InvalidBucketSize);
        }
        let Some(aggregate_range) = self.aggregate_range else {
            return Ok(false);
        };
        let bucket_end = bucket_start + bucket_size;
        Ok(bucket_start >= aggregate_range.start && bucket_end <= aggregate_range.end)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BucketCountRow {
    pub bucket_start: DateTime<Utc>,
    pub series: String,
    pub count: i64,
}

pub fn merge_bucket_rows_deterministic(
    plan: &WatermarkCutoverPlan,
    aggregate_rows: &[BucketCountRow],
    raw_rows: &[BucketCountRow],
) -> Vec<BucketCountRow> {
    let mut merged = BTreeMap::<(DateTime<Utc>, String), i64>::new();

    for row in aggregate_rows {
        if let Some(range) = plan.aggregate_range {
            if row.bucket_start >= range.start && row.bucket_start < range.end {
                merged.insert((row.bucket_start, row.series.clone()), row.count);
            }
        }
    }
    for row in raw_rows {
        if let Some(range) = plan.raw_range {
            if row.bucket_start >= range.start && row.bucket_start < range.end {
                merged.insert((row.bucket_start, row.series.clone()), row.count);
            }
        }
    }

    merged
        .into_iter()
        .map(|((bucket_start, series), count)| BucketCountRow {
            bucket_start,
            series,
            count,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::{merge_bucket_rows_deterministic, BucketCountRow, TimeRange, WatermarkCutoverPlan};
    use crate::dashboard_data::contracts::FreshnessMode;

    #[test]
    fn watermark_unknown_downgrades_to_raw_only_fallback() {
        let range = TimeRange::new(
            chrono::Utc.with_ymd_and_hms(2026, 6, 1, 0, 0, 0).unwrap(),
            chrono::Utc.with_ymd_and_hms(2026, 6, 2, 0, 0, 0).unwrap(),
        )
        .expect("valid range");
        let plan = WatermarkCutoverPlan::build(range, None).expect("plan");
        assert_eq!(plan.freshness_mode, FreshnessMode::RawOnlyFallback);
        assert!(plan.aggregate_range.is_none());
        assert_eq!(plan.raw_range, Some(range));
    }

    #[test]
    fn watermark_boundary_uses_half_open_cutover() {
        let start = chrono::Utc.with_ymd_and_hms(2026, 6, 1, 0, 0, 0).unwrap();
        let wm = chrono::Utc.with_ymd_and_hms(2026, 6, 1, 12, 0, 0).unwrap();
        let end = chrono::Utc.with_ymd_and_hms(2026, 6, 2, 0, 0, 0).unwrap();
        let range = TimeRange::new(start, end).expect("valid range");
        let plan = WatermarkCutoverPlan::build(range, Some(wm)).expect("plan");

        assert_eq!(plan.freshness_mode, FreshnessMode::AggregatePlusTail);
        assert_eq!(
            plan.aggregate_range.expect("aggregate range"),
            TimeRange::new(start, wm).expect("valid aggregate")
        );
        assert_eq!(
            plan.raw_range.expect("raw range"),
            TimeRange::new(wm, end).expect("valid raw")
        );
    }

    #[test]
    fn merge_is_deterministic_and_no_double_count_at_cutover() {
        let start = chrono::Utc.with_ymd_and_hms(2026, 6, 1, 0, 0, 0).unwrap();
        let wm = chrono::Utc.with_ymd_and_hms(2026, 6, 1, 12, 0, 0).unwrap();
        let end = chrono::Utc.with_ymd_and_hms(2026, 6, 2, 0, 0, 0).unwrap();
        let range = TimeRange::new(start, end).expect("valid range");
        let plan = WatermarkCutoverPlan::build(range, Some(wm)).expect("plan");

        let aggregate_rows = vec![
            BucketCountRow {
                bucket_start: chrono::Utc.with_ymd_and_hms(2026, 6, 1, 10, 0, 0).unwrap(),
                series: "completed".to_string(),
                count: 10,
            },
            BucketCountRow {
                bucket_start: wm,
                series: "completed".to_string(),
                count: 11,
            },
        ];
        let raw_rows = vec![
            BucketCountRow {
                bucket_start: wm,
                series: "completed".to_string(),
                count: 12,
            },
            BucketCountRow {
                bucket_start: chrono::Utc.with_ymd_and_hms(2026, 6, 1, 13, 0, 0).unwrap(),
                series: "completed".to_string(),
                count: 13,
            },
        ];

        let merged = merge_bucket_rows_deterministic(&plan, &aggregate_rows, &raw_rows);

        assert_eq!(merged.len(), 3);
        assert_eq!(merged[0].count, 10);
        assert_eq!(merged[1].bucket_start, wm);
        assert_eq!(merged[1].count, 12);
        assert_eq!(merged[2].count, 13);
    }
}
