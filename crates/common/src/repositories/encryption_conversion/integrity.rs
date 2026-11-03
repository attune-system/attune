//! Authenticate the retained storage contracts without granting plaintext reads,
//! reviving expired authority, changing pins, or manufacturing missing history.
use super::*;
use crate::repositories::{
    accepted_input::AcceptedInputRepository,
    event_origin::EventOriginRepository,
    file_evidence_retention::FileEvidenceRetentionRepository,
    inherited_input::{InheritedInputRepository, InputEntityKind, InputSite, InputSitePurpose},
    native_input_snapshot::NativeInputSnapshotRepository,
};

pub(super) async fn validate(conn: &mut PgConnection) -> Result<Vec<(&'static str, i64, usize)>> {
    let pending: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM file_evidence_put)")
        .fetch_one(&mut *conn)
        .await?;
    if pending {
        return Err(Error::validation(
            "File PUT operations must be settled or write-fenced before encryption conversion",
        ));
    }
    let mut evidence = Vec::new();
    let accepted: Vec<(i64, i64)> =
        sqlx::query_as("SELECT id,execution FROM runtime_accepted_input ORDER BY id")
            .fetch_all(&mut *conn)
            .await?;
    for (id, execution) in accepted {
        let site = InputSite {
            execution,
            kind: InputEntityKind::ExecutionConfig,
            purpose: InputSitePurpose::Value,
            path: String::new(),
        };
        let (_, _, bytes, _) = AcceptedInputRepository::retention_evidence(conn, id, &site).await?;
        evidence.push(("accepted", id, bytes));
    }
    let receipts: Vec<i64> = sqlx::query_scalar("SELECT id FROM runtime_input_receipt ORDER BY id")
        .fetch_all(&mut *conn)
        .await?;
    for id in receipts {
        let parents = InheritedInputRepository::retention_parents(conn, id).await?;
        evidence.push(("receipt", id, parents.retained_bytes()));
    }
    let snapshots: Vec<i64> =
        sqlx::query_scalar("SELECT id FROM runtime_native_input_snapshot ORDER BY id")
            .fetch_all(&mut *conn)
            .await?;
    for id in snapshots {
        let storage = NativeInputSnapshotRepository::storage_evidence(conn, id).await?;
        evidence.push(("native", id, storage.retained_bytes()));
    }
    let events: Vec<i64> = sqlx::query_scalar("SELECT id FROM event_origin_snapshot ORDER BY id")
        .fetch_all(&mut *conn)
        .await?;
    for id in events {
        let snapshot = EventOriginRepository::find(&mut *conn, id)
            .await
            .map_err(|_| Error::validation("Corrupt event_origin_snapshot evidence"))?;
        let recorded = EventOriginRepository::evidence(&mut *conn, id).await?;
        if serde_json::to_value(snapshot.evidence()?)? != serde_json::to_value(recorded)? {
            return Err(graph::corrupt());
        }
        evidence.push(("event", id, serde_json::to_vec(&snapshot)?.len()));
    }
    let files: Vec<i64> =
        sqlx::query_scalar("SELECT version FROM file_evidence_owner ORDER BY version")
            .fetch_all(&mut *conn)
            .await?;
    for id in files {
        let bytes =
            FileEvidenceRetentionRepository::encryption_conversion_evidence(conn, id).await?;
        evidence.push(("file", id, bytes));
    }
    Ok(evidence)
}
