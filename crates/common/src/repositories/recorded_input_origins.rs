//! Compatibility name for B's accepted-contributor callers. All traversal is in
//! C's canonical immutable_input_origin module, shared with worker restoration.
use crate::{delegation::DelegationAuthority, secret_provenance::SecretOrigin, Result};
use sqlx::PgConnection;

pub struct RecordedInputOriginRepository;
impl RecordedInputOriginRepository {
    pub async fn require_runtime_reads(
        conn: &mut PgConnection,
        contributors: &[DelegationAuthority],
        origins: &[SecretOrigin],
    ) -> Result<()> {
        super::immutable_input_origin::ImmutableInputOriginRepository::require(
            conn,
            contributors,
            origins,
        )
        .await
    }
}
