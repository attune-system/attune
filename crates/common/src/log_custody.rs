//! Durable, exact-target execution log custody. A preparation acknowledgement
//! records history; delivery authority is obtained separately and freshly.
use crate::{log_delivery::ControlRequest, models::log_delivery::DeliverySegment};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

pub const MAX_CUSTODY_REQUEST_BYTES: usize = 4 * 1024 * 1024;
#[derive(Clone, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ReserveCustody {
    pub binding: ControlRequest,
    pub finalize_by: DateTime<Utc>,
}
#[derive(Clone, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct PrepareCustody {
    pub reservation: ReserveCustody,
    pub segments: Vec<DeliverySegment>,
    pub total_bytes: i64,
    pub sha256: String,
    pub truncated: bool,
    pub authenticated_manifest: Option<String>,
}
impl std::fmt::Debug for PrepareCustody {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PrepareCustody")
            .field("custody", &self.reservation.binding.custody)
            .field("segment_count", &self.segments.len())
            .finish_non_exhaustive()
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum CustodyOperationState {
    Reserved,
    Prepared,
    Cancelled,
}
#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CustodyOperationAck {
    pub custody: Uuid,
    pub state: CustodyOperationState,
}
#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CustodyAbsence {
    pub custody: Uuid,
    /// True only after an acknowledged permanent tombstone/history fence.
    /// False means an existing intent must be reconciled, not deleted locally.
    pub absent: bool,
}
