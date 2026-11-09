use super::*;
use serde_json::json;

async fn fixture() -> (attune_common::test_database::TestDatabase, i64, i64, i64) {
    // Origin-less log: no fabricated evidence owner or execution retention FK.
    let pool = create_test_pool().await.unwrap();
    let artifact = ArtifactRepository::create(&pool, artifact_input("close"))
        .await
        .unwrap();
    let version = ArtifactVersionRepository::create_log_pending(
        &pool,
        artifact.id,
        &artifact.r#ref,
        LogStreamBackend::ObjectSegments,
        "text/plain".into(),
        None,
        None,
        None,
    )
    .await
    .unwrap();
    let stream = LogStreamRepository::create(&pool, version.id, 1024, 500)
        .await
        .unwrap();
    (pool, artifact.id, version.id, stream.id)
}

async fn select(
    pool: &sqlx::PgPool,
    sid: i64,
    revision: i64,
) -> attune_common::Result<attune_common::repositories::log_stream::SelectedLogClose> {
    let mut tx = pool.begin().await?;
    Evidence::lock_capture(&mut tx).await?;
    LogStreamRepository::lock(&mut tx, sid).await?;
    let result = LogStreamRepository::select_close(
        &mut tx,
        sid,
        revision,
        &json!({"kind":"native_test"}),
        &json!({"encoding":"plain"}),
        false,
        Utc::now() + Duration::minutes(5),
    )
    .await;
    if result.is_ok() {
        tx.commit().await?;
    } else {
        tx.rollback().await?;
    }
    result
}

async fn rejected(pool: &sqlx::PgPool, query: String) {
    let mut tx = pool.begin().await.unwrap();
    Evidence::lock_reclamation(&mut tx).await.unwrap();
    let result = sqlx::query(&query).execute(&mut *tx).await;
    assert!(result.is_err(), "unexpected accepted mutation: {query}");
    tx.rollback().await.unwrap();
}

#[tokio::test]
async fn close_permanence_revision_and_originless_cascades() {
    let (pool, artifact, version, stream) = fixture().await;
    let selected = select(&pool, stream, 0).await.unwrap();
    assert_eq!(selected.revision(), 1);
    assert_eq!(select(&pool, stream, 0).await.unwrap().revision(), 1);
    assert_eq!(selected.input()["snapshot"]["version"], version);
    assert_eq!(selected.input_sha256().len(), 64);
    for query in [
        format!("DELETE FROM artifact WHERE id={artifact}"),
        format!("DELETE FROM artifact_version WHERE id={version}"),
        format!("DELETE FROM log_stream WHERE id={stream}"),
        format!("UPDATE artifact_version SET body_state='cleanup_claimed' WHERE id={version}"),
        format!("UPDATE artifact_version SET body_state='ready',size_bytes=0,sha256=repeat('a',64) WHERE id={version}"),
        format!("UPDATE log_stream SET sealed=true WHERE id={stream}"),
        format!("UPDATE log_stream SET close_phase=NULL,close_revision=0,close_input=NULL,close_input_sha256=NULL,close_deadline=NULL WHERE id={stream}"),
        format!("INSERT INTO log_segment(stream,sequence,byte_start,byte_end,size_bytes,sha256,object_key,provider_version) VALUES({stream},0,0,1,1,repeat('a',64),'new','v:1')"),
    ] { rejected(&pool,query).await; }
    let mut tx = pool.begin().await.unwrap();
    Evidence::lock_capture(&mut tx).await.unwrap();
    assert!(LogStreamRepository::abandon_close(&mut tx, stream, 1)
        .await
        .unwrap());
    tx.commit().await.unwrap();
    // Permanent closure remains after pin release; recreation cannot erase it.
    rejected(&pool, format!("DELETE FROM log_stream WHERE id={stream}")).await;
    rejected(
        &pool,
        format!("UPDATE log_stream SET total_bytes=1 WHERE id={stream}"),
    )
    .await;
    rejected(
        &pool,
        format!("UPDATE log_stream SET sealed=true WHERE id={stream}"),
    )
    .await;
    let resumed = select(&pool, stream, 2).await.unwrap();
    assert_eq!(resumed.revision(), 3);
    assert_eq!(resumed.input(), selected.input());
    assert_eq!(select(&pool, stream, 2).await.unwrap().revision(), 3);
    assert!(select(&pool, stream, 0).await.is_err());
    let mut tx = pool.begin().await.unwrap();
    Evidence::lock_capture(&mut tx).await.unwrap();
    assert!(!LogStreamRepository::abandon_close(&mut tx, stream, 1)
        .await
        .unwrap());
    tx.rollback().await.unwrap();
}

#[tokio::test]
async fn accepted_identity_and_provider_deletion_are_fenced() {
    let (pool, _, version, stream) = fixture().await;
    let mut tx = pool.begin().await.unwrap();
    Evidence::lock_capture(&mut tx).await.unwrap();
    sqlx::query("INSERT INTO object_maintenance_ledger(object_key,provider_version,object_kind,size_bytes,state) VALUES('close-key','v:1','log',1,'ready')").execute(&mut *tx).await.unwrap();
    let locked = LogStreamRepository::lock(&mut tx, stream).await.unwrap();
    let segment = LogStreamRepository::commit_segment(
        &mut tx,
        &locked,
        0,
        1,
        &"a".repeat(64),
        "close-key",
        "v:1",
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    rejected(
        &pool,
        format!(
            "UPDATE log_segment SET provider_version='v:2' WHERE id={}",
            segment.id
        ),
    )
    .await;
    // Collector-first: committed deletion ownership must defeat prepare.
    let mut claim = pool.begin().await.unwrap();
    Evidence::lock_reclamation(&mut claim).await.unwrap();
    sqlx::query(
        "UPDATE object_maintenance_ledger SET state='deleting' WHERE object_key='close-key'",
    )
    .execute(&mut *claim)
    .await
    .unwrap();
    claim.commit().await.unwrap();
    assert!(select(&pool, stream, 0).await.is_err());
    let mut restore = pool.begin().await.unwrap();
    Evidence::lock_reclamation(&mut restore).await.unwrap();
    sqlx::query("UPDATE object_maintenance_ledger SET state='ready' WHERE object_key='close-key'")
        .execute(&mut *restore)
        .await
        .unwrap();
    restore.commit().await.unwrap();
    sqlx::query("INSERT INTO artifact_upload_grant(artifact_version,segment_sequence,object_key,expected_size,expected_sha256,content_type,state,expires_at,settle_until,completed_provider_version,completed_at) VALUES($1,0,'close-key',1,repeat('a',64),'application/octet-stream','completed',NOW(),NOW(),'v:1',NOW())")
        .bind(version).execute(&*pool).await.unwrap();
    select(&pool, stream, 0).await.unwrap();
    sqlx::query("UPDATE artifact_upload_grant SET state=state WHERE artifact_version=$1")
        .bind(version)
        .execute(&*pool)
        .await
        .unwrap();
    rejected(&pool,format!("UPDATE artifact_upload_grant SET state='issued',completed_provider_version=NULL,completed_at=NULL WHERE artifact_version={version}")).await;
    rejected(&pool,format!("UPDATE artifact_upload_grant SET completed_provider_version='v:2' WHERE artifact_version={version}")).await;
    rejected(&pool,format!("INSERT INTO artifact_upload_grant(artifact_version,segment_sequence,object_key,expected_size,expected_sha256,content_type,expires_at,settle_until) VALUES({version},1,'new-grant',1,repeat('a',64),'application/octet-stream',NOW(),NOW())")).await;

    rejected(
        &pool,
        format!("DELETE FROM log_segment WHERE id={}", segment.id),
    )
    .await;
    rejected(
        &pool,
        "UPDATE object_maintenance_ledger SET state='deleting' WHERE object_key='close-key'".into(),
    )
    .await;
    rejected(
        &pool,
        "DELETE FROM object_maintenance_ledger WHERE object_key='close-key'".into(),
    )
    .await;
    let mut tx = pool.begin().await.unwrap();
    LogStreamRepository::check_close_append(
        &mut tx,
        stream,
        0,
        1,
        &"a".repeat(64),
        "close-key",
        Some("v:1"),
    )
    .await
    .unwrap();
    assert!(LogStreamRepository::check_close_append(
        &mut tx,
        stream,
        0,
        1,
        &"a".repeat(64),
        "close-key",
        Some("v:2")
    )
    .await
    .is_err());
    assert!(LogStreamRepository::check_close_append(
        &mut tx,
        stream,
        1,
        1,
        &"a".repeat(64),
        "close-key",
        None
    )
    .await
    .is_err());
    tx.rollback().await.unwrap();
}

#[tokio::test]
async fn abandoned_close_allows_version_teardown_but_cannot_reopen() {
    let (pool, _, version, stream) = fixture().await;
    select(&pool, stream, 0).await.unwrap();
    let mut tx = pool.begin().await.unwrap();
    Evidence::lock_reclamation(&mut tx).await.unwrap();
    assert!(Evidence::request_cleanup(&mut tx, &[version])
        .await
        .unwrap()
        .is_empty());
    assert!(LogStreamRepository::abandon_close(&mut tx, stream, 1)
        .await
        .unwrap());
    assert_eq!(
        Evidence::request_cleanup(&mut tx, &[version])
            .await
            .unwrap(),
        vec![version]
    );
    assert!(ArtifactVersionRepository::delete(&mut *tx, version)
        .await
        .unwrap());
    assert!(
        LogStreamRepository::find_by_artifact_version_with_conn(&mut tx, version)
            .await
            .unwrap()
            .is_none()
    );
    tx.commit().await.unwrap();
}

#[tokio::test]
async fn collector_first_barrier_fences_selection() {
    let (pool, _, _, stream) = fixture().await;
    let mut tx = pool.begin().await.unwrap();
    Evidence::lock_capture(&mut tx).await.unwrap();
    sqlx::query("INSERT INTO object_maintenance_ledger(object_key,provider_version,object_kind,size_bytes,state) VALUES('barrier-key','v:1','log',1,'ready')").execute(&mut *tx).await.unwrap();
    let locked = LogStreamRepository::lock(&mut tx, stream).await.unwrap();
    LogStreamRepository::commit_segment(
        &mut tx,
        &locked,
        0,
        1,
        &"b".repeat(64),
        "barrier-key",
        "v:1",
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    let mut collector = pool.begin().await.unwrap();
    Evidence::lock_reclamation(&mut collector).await.unwrap();
    let contender_pool: sqlx::PgPool = (*pool).clone();
    let (started, ready) = tokio::sync::oneshot::channel();
    let contender = tokio::spawn(async move {
        started.send(()).unwrap();
        select(&contender_pool, stream, 0).await
    });
    ready.await.unwrap();
    // The contender has started while reclamation owns exclusive admission.
    sqlx::query(
        "UPDATE object_maintenance_ledger SET state='deleting' WHERE object_key='barrier-key'",
    )
    .execute(&mut *collector)
    .await
    .unwrap();
    collector.commit().await.unwrap();
    assert!(
        tokio::time::timeout(std::time::Duration::from_secs(10), contender)
            .await
            .unwrap()
            .unwrap()
            .is_err()
    );
}
