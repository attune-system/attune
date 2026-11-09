// SQL-shape setup for synthetic repository/transport fixtures. This is not an
// authorization or byte-verification fixture; route tests exercise those paths.
use attune_common::repositories::{
    file_evidence_retention::FileEvidenceRetentionRepository as Evidence,
    log_stream::LogStreamRepository,
};

pub async fn seal(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    stream: i64,
    truncated: bool,
) -> attune_common::Result<()> {
    // Callers enter admission before any target lock, even for fixture setup.
    let held: bool = sqlx::query_scalar("SELECT file_evidence_graph_held(false)")
        .fetch_one(&mut **tx)
        .await?;
    assert!(held, "fixture requires graph admission before targets");
    let locked = LogStreamRepository::lock(tx, stream).await?;
    if locked.backend == attune_common::models::enums::LogStreamBackend::SharedFile {
        return LogStreamRepository::seal(tx, stream, truncated).await;
    }
    sqlx::query("INSERT INTO object_maintenance_ledger(object_key,provider_version,object_kind,size_bytes,state) SELECT object_key,provider_version,'log',size_bytes,'ready' FROM log_segment WHERE stream=$1 ON CONFLICT DO NOTHING")
        .bind(stream).execute(&mut **tx).await?;
    let revision = LogStreamRepository::close_revision(tx, stream).await?;
    let selected = LogStreamRepository::select_close(
        tx,
        stream,
        revision,
        &serde_json::json!({"kind":"database_fixture"}),
        &serde_json::json!({"kind":"plain"}),
        truncated,
        chrono::Utc::now() + chrono::Duration::minutes(5),
    )
    .await?;
    LogStreamRepository::publish_close(tx, stream, &selected).await
}

pub async fn admission(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
) -> attune_common::Result<()> {
    Evidence::lock_capture(tx).await
}
