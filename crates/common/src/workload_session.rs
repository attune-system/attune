//! Dedicated machine sessions. These credentials have target-aware consumers;
//! they never become IAM grants, engine credentials, or pack invocation fields.
use crate::workload_launch::LaunchToken;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkloadSession {
    pub token: LaunchToken,
    pub expires_at: DateTime<Utc>,
}
#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct RenewWorkloadSession {}
