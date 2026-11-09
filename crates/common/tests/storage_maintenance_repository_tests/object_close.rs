use super::*;
use serde_json::json;

async fn fixture() -> (attune_common::test_database::TestDatabase, i64, i64, i64) {
    // Origin-less log: no fabricated evidence owner or execution retention FK.
    let pool = create_test_pool().await.unwrap();
    let (artifact, version, stream) = create_log(&pool).await;
    (pool, artifact, version, stream)
}

async fn create_log(pool: &sqlx::PgPool) -> (i64, i64, i64) {
    let artifact = ArtifactRepository::create(pool, artifact_input("close"))
        .await
        .unwrap();
    let version = ArtifactVersionRepository::create_log_pending(
        pool,
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
    let stream = LogStreamRepository::create(pool, version.id, 1024, 500)
        .await
        .unwrap();
    (artifact.id, version.id, stream.id)
}

async fn expired_grant(pool: &sqlx::PgPool, version: i64, key: &str) -> i64 {
    sqlx::query_scalar("INSERT INTO artifact_upload_grant(artifact_version,segment_sequence,object_key,expected_size,expected_sha256,content_type,expires_at,settle_until) VALUES($1,0,$2,1,repeat('a',64),'application/octet-stream',NOW()-interval '2 minutes',NOW()-interval '1 minute') RETURNING id")
        .bind(version).bind(key).fetch_one(pool).await.unwrap()
}

#[tokio::test]
async fn expiry_and_stream_first_renewal_do_not_reverse_lock_order() {
    use attune_common::repositories::artifact_upload_grant::ArtifactUploadGrantRepository as Grants;
    use std::time::Duration as StdDuration;
    use tokio::time::timeout;

    let (pool, _, version, stream) = fixture().await;
    let grant = expired_grant(&pool, version, "expiry-renewal").await;
    // Pause actual maintenance after it locks the grant, before the production
    // close guard. This creates the precise inverse-order interleaving without
    // sleeps or relying on the deadlock detector to arrange the race.
    sqlx::raw_sql("CREATE FUNCTION test_expiry_barrier() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN IF OLD.state='issued' AND NEW.state='expired' THEN PERFORM pg_advisory_xact_lock(48127,1); END IF; RETURN NEW; END $$; CREATE TRIGGER aaa_test_expiry_barrier BEFORE UPDATE ON artifact_upload_grant FOR EACH ROW EXECUTE FUNCTION test_expiry_barrier();")
        .execute(&*pool).await.unwrap();
    let mut barrier = pool.begin().await.unwrap();
    sqlx::query("SELECT pg_advisory_xact_lock(48127,1)")
        .execute(&mut *barrier)
        .await
        .unwrap();
    let barrier_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *barrier)
        .await
        .unwrap();
    let mut renewal = pool.begin().await.unwrap();
    LogStreamRepository::lock(&mut renewal, stream)
        .await
        .unwrap();
    let renewal_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *renewal)
        .await
        .unwrap();
    let expiry_pool = pool.pool().clone();
    let expiry = tokio::spawn(async move { Grants::mark_expired(&expiry_pool, 10).await });
    let expiry_pid = timeout(StdDuration::from_secs(5), async {
        loop {
            let pid: Option<i32> = sqlx::query_scalar("SELECT pid FROM pg_locks WHERE NOT granted AND locktype='advisory' AND $1=ANY(pg_blocking_pids(pid)) LIMIT 1")
                .bind(barrier_pid).fetch_optional(&*pool).await.unwrap();
            if let Some(pid) = pid { break pid; }
            tokio::task::yield_now().await;
        }
    }).await.unwrap();
    let renewal = tokio::spawn(async move {
        Grants::find_log_segment_for_update(&mut renewal, version, 0).await?;
        let renewed = Grants::renew(
            &mut renewal,
            grant,
            Utc::now() + Duration::minutes(1),
            Utc::now() + Duration::minutes(2),
        )
        .await?;
        renewal.commit().await?;
        attune_common::Result::Ok(renewed)
    });
    let waiting = timeout(StdDuration::from_secs(5), async {
        loop {
            let waiting: bool = sqlx::query_scalar("SELECT $1=ANY(pg_blocking_pids($2))")
                .bind(expiry_pid)
                .bind(renewal_pid)
                .fetch_one(&*pool)
                .await
                .unwrap();
            if waiting {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await;
    // Always release the barrier and join both owners before asserting outcomes.
    barrier.rollback().await.unwrap();
    let (expired, renewed) = tokio::join!(expiry, renewal);
    waiting.unwrap();
    assert_eq!(expired.unwrap().unwrap(), 1);
    assert!(renewed.unwrap().unwrap());
    let state: String =
        sqlx::query_scalar("SELECT state::text FROM artifact_upload_grant WHERE id=$1")
            .bind(grant)
            .fetch_one(&*pool)
            .await
            .unwrap();
    assert_eq!(state, "issued");
}

#[tokio::test]
async fn expiry_batches_survive_closing_and_abandoned_streams_without_reopening() {
    use attune_common::repositories::artifact_upload_grant::ArtifactUploadGrantRepository as Grants;

    for abandon in [false, true] {
        let (pool, _, version, stream) = fixture().await;
        let grant = expired_grant(&pool, version, "closed-expiry").await;
        let (_, other_version, _) = create_log(&pool).await;
        let other = expired_grant(&pool, other_version, "unrelated-expiry").await;
        let selected = select(&pool, stream, 0).await.unwrap();
        if abandon {
            let mut tx = pool.begin().await.unwrap();
            Evidence::lock_capture(&mut tx).await.unwrap();
            assert!(
                LogStreamRepository::abandon_close(&mut tx, stream, selected.revision())
                    .await
                    .unwrap()
            );
            tx.commit().await.unwrap();
        }
        // The exemption cannot change deadlines or identity, even alongside expiry.
        for mutation in [
            "object_key='changed'",
            "expected_size=2",
            "settle_until=NOW()+interval '1 hour'",
            "segment_sequence=1",
            "expected_sha256=repeat('b',64)",
            "token=gen_random_uuid()",
        ] {
            rejected(
                &pool,
                format!(
                    "UPDATE artifact_upload_grant SET state='expired',{mutation} WHERE id={grant}"
                ),
            )
            .await;
        }
        assert_eq!(Grants::mark_expired(&pool, 10).await.unwrap(), 2);
        let states: Vec<String> = sqlx::query_scalar(
            "SELECT state::text FROM artifact_upload_grant WHERE id=ANY($1) ORDER BY id",
        )
        .bind(vec![grant, other])
        .fetch_all(&*pool)
        .await
        .unwrap();
        assert_eq!(states, vec!["expired", "expired"]);
        rejected(
            &pool,
            format!("UPDATE artifact_upload_grant SET state='issued' WHERE id={grant}"),
        )
        .await;
        rejected(&pool, format!("INSERT INTO log_segment(stream,sequence,byte_start,byte_end,size_bytes,sha256,object_key,provider_version) VALUES({stream},0,0,1,1,repeat('a',64),'tail','v:1')")).await;
        let (phase, revision, input): (String, i64, serde_json::Value) = sqlx::query_as(
            "SELECT close_phase,close_revision,close_input FROM log_stream WHERE id=$1",
        )
        .bind(stream)
        .fetch_one(&*pool)
        .await
        .unwrap();
        assert_eq!(phase, if abandon { "abandoned" } else { "closing" });
        assert_eq!(revision, if abandon { 2 } else { 1 });
        assert_eq!(&input, selected.input());
    }
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

#[tokio::test]
async fn selected_snapshot_retry_is_independent_of_session_timezone() {
    let (pool, _, _, stream) = fixture().await;
    let mut tx = pool.begin().await.unwrap();
    Evidence::lock_capture(&mut tx).await.unwrap();
    sqlx::query("INSERT INTO object_maintenance_ledger(object_key,provider_version,object_kind,size_bytes,state) VALUES('timezone-close','v:1','log',1,'ready')")
        .execute(&mut *tx).await.unwrap();
    let locked = LogStreamRepository::lock(&mut tx, stream).await.unwrap();
    LogStreamRepository::commit_segment(
        &mut tx,
        &locked,
        0,
        1,
        &"a".repeat(64),
        "timezone-close",
        "v:1",
    )
    .await
    .unwrap();
    sqlx::query("SET LOCAL TimeZone='Pacific/Honolulu'")
        .execute(&mut *tx)
        .await
        .unwrap();
    let first = LogStreamRepository::select_close(
        &mut tx,
        stream,
        0,
        &json!({"kind":"native_test"}),
        &json!({"encoding":"plain"}),
        false,
        Utc::now() + Duration::minutes(5),
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    let mut tx = pool.begin().await.unwrap();
    Evidence::lock_capture(&mut tx).await.unwrap();
    LogStreamRepository::lock(&mut tx, stream).await.unwrap();
    sqlx::query("SET LOCAL TimeZone='Asia/Kathmandu'")
        .execute(&mut *tx)
        .await
        .unwrap();
    let retry = LogStreamRepository::select_close(
        &mut tx,
        stream,
        0,
        &json!({"kind":"native_test"}),
        &json!({"encoding":"plain"}),
        false,
        Utc::now() + Duration::minutes(5),
    )
    .await
    .unwrap();
    assert_eq!(first.revision(), retry.revision());
    assert_eq!(first.input(), retry.input());
    assert_eq!(first.input_sha256(), retry.input_sha256());
    tx.rollback().await.unwrap();
    pool.cleanup().await.unwrap();
}
