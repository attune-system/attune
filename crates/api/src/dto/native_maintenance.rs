//! Operator observations of native maintenance. Each repository samples its own
//! state; this response does not assert a single cross-job transaction snapshot.

use attune_common::repositories::native_maintenance::{
    partitions::PartitionStatus, schedule::MaintenanceScheduleStatus, summaries::SummaryStatus,
};
use chrono::{DateTime, Utc};
use serde::Serialize;
use utoipa::ToSchema;

#[derive(Debug, Serialize, ToSchema)]
pub struct NativeMaintenanceStatus {
    /// UTC time at which the API began collecting these observations.
    pub observed_at: DateTime<Utc>,
    /// Persisted native-maintenance switch, independent of row retention.
    pub enabled: bool,
    pub partitions: Vec<PartitionStatus>,
    /// Coverage extrema do not imply continuous coverage. Dirty hours use raw reads.
    pub summaries: Vec<SummaryStatus>,
    pub schedule: Vec<MaintenanceScheduleStatus>,
}
