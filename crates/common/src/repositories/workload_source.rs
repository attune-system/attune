//! Permanent source epochs. Always lock live sources before these rows, and
//! these rows before operation rows. Restoration cannot undo an intervening
//! revocation. No source FK or normal retention may remove this evidence.
use crate::{Error, Result};
use serde::{Deserialize, Serialize};
use sqlx::PgConnection;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SourceEpoch {
    pub kind: String,
    pub subject: i64,
    pub epoch: i64,
}
pub(crate) async fn snapshot(
    conn: &mut PgConnection,
    mut sources: Vec<(&str, i64)>,
) -> Result<Vec<SourceEpoch>> {
    sources.sort_unstable();
    sources.dedup();
    let mut result = Vec::with_capacity(sources.len());
    for (kind, subject) in sources {
        if subject <= 0 {
            return Err(Error::PermissionDenied(
                "Workload source unavailable".into(),
            ));
        }
        sqlx::query(
            "INSERT INTO workload_source_epoch(kind,subject) VALUES($1,$2) ON CONFLICT DO NOTHING",
        )
        .bind(kind)
        .bind(subject)
        .execute(&mut *conn)
        .await?;
        let epoch = sqlx::query_scalar(
            "SELECT epoch FROM workload_source_epoch WHERE kind=$1 AND subject=$2 FOR SHARE",
        )
        .bind(kind)
        .bind(subject)
        .fetch_one(&mut *conn)
        .await?;
        result.push(SourceEpoch {
            kind: kind.into(),
            subject,
            epoch,
        });
    }
    Ok(result)
}
