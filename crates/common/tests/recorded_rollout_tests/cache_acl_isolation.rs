//! Real separate-session CREATE attempts while a private GRANT is suspended.
use super::*;
use attune_common::repositories::cache::{CacheEntryRepository, CacheTransactionMode};
use sqlx::{Connection, PgConnection};
use std::time::Duration;

async fn acl(f: &Fixture) -> (i64, i64, Option<String>) {
    sqlx::query_as(
        "SELECT oid::BIGINT,nspowner::BIGINT,nspacl::TEXT FROM pg_namespace WHERE nspname=$1",
    )
    .bind(f.db.schema())
    .fetch_one(&*f.db)
    .await
    .unwrap()
}
async fn wait_blocked(f: &Fixture, pid: i32, blocker: i32) -> Result<(), String> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
    loop {
        let held: bool = sqlx::query_scalar("SELECT $2=ANY(pg_blocking_pids($1))")
            .bind(pid)
            .bind(blocker)
            .fetch_one(&*f.db)
            .await
            .map_err(|e| e.to_string())?;
        if held {
            return Ok(());
        }
        if tokio::time::Instant::now() >= deadline {
            return Err("owned GRANT barrier not reached".into());
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

pub(super) async fn probe(f: &mut Fixture, namespace: i64) {
    let cache_owner = f.roles[6].clone();
    let role = format!("ws2_cache_observer_{}", Uuid::new_v4().simple());
    let password = format!("dummy_{}", Uuid::new_v4().simple());
    assert_eq!(
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM pg_roles WHERE rolname=$1")
            .bind(&role)
            .fetch_one(&*f.db)
            .await
            .unwrap(),
        0
    );
    let ddl: String =
        sqlx::query_scalar("SELECT format('CREATE ROLE %I LOGIN PASSWORD %L',$1::TEXT,$2::TEXT)")
            .bind(&role)
            .bind(&password)
            .fetch_one(&*f.db)
            .await
            .unwrap();
    sqlx::query(&ddl).execute(&*f.db).await.unwrap();
    f.roles.push(role.clone());
    std::fs::write(&f.manifest, serde_json::to_vec(&f.roles).unwrap()).unwrap();
    sqlx::query(&format!(
        "GRANT {cache_owner} TO {role} WITH INHERIT TRUE, SET TRUE"
    ))
    .execute(&*f.db)
    .await
    .unwrap();
    let control_schema = format!("ws2_control_{}", Uuid::new_v4().simple());
    sqlx::query(&format!(
        "CREATE SCHEMA {control_schema} AUTHORIZATION {cache_owner}"
    ))
    .execute(&*f.db)
    .await
    .unwrap();
    let mut url = url::Url::parse(f.db.database_url()).unwrap();
    url.set_username(&role).unwrap();
    url.set_password(Some(&password)).unwrap();
    url.query_pairs_mut().append_pair(
        "options",
        &format!("-csearch_path={},public", f.db.schema()),
    );
    let observer = PgPoolOptions::new()
        .max_connections(1)
        .connect(url.as_str())
        .await
        .unwrap();
    f.pools.push(observer.clone());
    // Positive control: this real session can CREATE as the exact cache owner.
    let mut positive = observer.begin().await.unwrap();
    sqlx::query(&format!("SET LOCAL ROLE {cache_owner}"))
        .execute(&mut *positive)
        .await
        .unwrap();
    sqlx::query(&format!(
        "CREATE TABLE {control_schema}.positive(id BIGINT)"
    ))
    .execute(&mut *positive)
    .await
    .unwrap();
    positive.rollback().await.unwrap();
    observer.close().await;
    for fault in [
        Fault::Success,
        Fault::ExceptionSavepoint,
        Fault::Cancel,
        Fault::StatementTimeout,
        Fault::LockTimeout,
        Fault::Kill,
        Fault::Deadlock,
    ] {
        transition_case(f, namespace, url.as_str(), &cache_owner, fault).await;
    }
    lost_ack(f, namespace, url.as_str(), &cache_owner).await;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Fault {
    Success,
    ExceptionSavepoint,
    Cancel,
    StatementTimeout,
    LockTimeout,
    Kill,
    Deadlock,
}

async fn wait_unblocked(f: &Fixture, pid: i32, blocker: i32) -> Result<(), String> {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(8);
    loop {
        let blocked: bool = sqlx::query_scalar("SELECT $2=ANY(pg_blocking_pids($1))")
            .bind(pid)
            .bind(blocker)
            .fetch_one(&*f.db)
            .await
            .map_err(|e| e.to_string())?;
        if !blocked {
            return Ok(());
        }
        if tokio::time::Instant::now() >= deadline {
            return Err("owned backend did not leave fault barrier".into());
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}
async fn transition_case(f: &Fixture, namespace: i64, url: &str, cache_owner: &str, fault: Fault) {
    let baseline = acl(f).await;
    let api_id: i64 = sqlx::query_scalar("SELECT oid::BIGINT FROM pg_roles WHERE rolname=$1")
        .bind(&f.roles[2])
        .fetch_one(&*f.db)
        .await
        .unwrap();
    let key = (Uuid::new_v4().as_u128() as i64) & i64::MAX;
    let tag = if fault == Fault::ExceptionSavepoint {
        "ALTER TABLE"
    } else {
        "GRANT"
    };
    let raise = if fault == Fault::ExceptionSavepoint {
        "RAISE EXCEPTION 'owned exception after owner transfer';"
    } else {
        ""
    };
    let hook=format!("CREATE FUNCTION ws2_cache_acl_barrier() RETURNS EVENT_TRIGGER LANGUAGE plpgsql AS $$ BEGIN IF session_user::regrole::OID={api_id} AND has_schema_privilege('{cache_owner}','{}','CREATE') THEN PERFORM pg_advisory_xact_lock({key}::BIGINT); {raise} END IF; END $$; CREATE EVENT TRIGGER ws2_cache_acl_barrier ON ddl_command_end WHEN TAG IN ('{tag}') EXECUTE FUNCTION ws2_cache_acl_barrier();",f.db.schema());
    sqlx::raw_sql(&hook).execute(&*f.db).await.unwrap();
    let mut barrier = f.db.begin().await.unwrap();
    sqlx::raw_sql("SET LOCAL deadlock_timeout='10s'; SET LOCAL statement_timeout='12s'")
        .execute(&mut *barrier)
        .await
        .unwrap();
    sqlx::query("SELECT pg_advisory_xact_lock($1)")
        .bind(key)
        .execute(&mut *barrier)
        .await
        .unwrap();
    let blocker: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *barrier)
        .await
        .unwrap();
    let mut connection = f.pools[1].acquire().await.unwrap();
    let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *connection)
        .await
        .unwrap();
    let backend_start: chrono::DateTime<chrono::Utc> = sqlx::query_scalar("SELECT backend_start FROM pg_stat_activity WHERE pid=$1 AND datname=current_database() AND usename=$2").bind(pid).bind(&f.roles[2]).fetch_one(&*f.db).await.unwrap();
    let statistics: i64 =
        sqlx::query_scalar("SELECT partitions_created FROM cache_entry_statistics_state WHERE id")
            .fetch_one(&*f.db)
            .await
            .unwrap();
    let mut tx = connection.begin().await.unwrap();
    CacheEntryRepository::protect_transaction(&mut tx, CacheTransactionMode::Attach)
        .await
        .unwrap();
    sqlx::query("SAVEPOINT transition")
        .execute(&mut *tx)
        .await
        .unwrap();
    let generation:i64=sqlx::query_scalar("INSERT INTO cache_generation(namespace,client_refresh_id,expected_chunk_count,expected_count) VALUES ($1,$2,0,0) RETURNING id").bind(namespace).bind(format!("acl-{fault:?}")).fetch_one(&mut *tx).await.unwrap();
    let settings = match fault {
        Fault::StatementTimeout => {
            "SET LOCAL lock_timeout='8s'; SET LOCAL statement_timeout='1500ms'"
        }
        Fault::LockTimeout => "SET LOCAL lock_timeout='1500ms'; SET LOCAL statement_timeout='10s'",
        _ => "SET LOCAL lock_timeout='8s'; SET LOCAL statement_timeout='10s'",
    };
    sqlx::raw_sql(settings).execute(&mut *tx).await.unwrap();
    let operation = async {
        let result = sqlx::query("SELECT create_cache_generation_partition($1)")
            .bind(generation)
            .execute(&mut *tx)
            .await;
        match result {
            Ok(_) => tx.commit().await,
            Err(e) => {
                if fault == Fault::ExceptionSavepoint {
                    sqlx::raw_sql("ROLLBACK TO SAVEPOINT transition; RELEASE SAVEPOINT transition")
                        .execute(&mut *tx)
                        .await?;
                    tx.commit().await?;
                } else if fault == Fault::Kill {
                    let _ = tx.rollback().await;
                } else {
                    tx.rollback().await?;
                }
                Err(e)
            }
        }
    };
    let control = async {
        let result: Result<(), String> = async {
            wait_blocked(f, pid, blocker).await?;
            if acl(f).await != baseline {
                return Err("outside session observed temporary ACL".into());
            }
            observer_attempt(url, cache_owner, f.db.schema()).await?;
            match fault {
                Fault::Cancel | Fault::Kill => {
                    let sql = if fault == Fault::Kill {
                        "SELECT pg_terminate_backend(pid) FROM pg_stat_activity WHERE pid=$1 AND backend_start=$2 AND datname=current_database() AND usename=$3"
                    } else {
                        "SELECT pg_cancel_backend(pid) FROM pg_stat_activity WHERE pid=$1 AND backend_start=$2 AND datname=current_database() AND usename=$3"
                    };
                    let sent: bool = sqlx::query_scalar(sql)
                        .bind(pid)
                        .bind(backend_start)
                        .bind(&f.roles[2])
                        .fetch_one(&*f.db)
                        .await
                        .map_err(|e| e.to_string())?;
                    if !sent {
                        return Err("owned backend fault was not delivered".into());
                    }
                    wait_unblocked(f, pid, blocker).await?;
                }
                Fault::StatementTimeout | Fault::LockTimeout => {
                    wait_unblocked(f, pid, blocker).await?
                }
                Fault::Deadlock => {
                    // API holds the namespace; blocker holds the advisory gate.
                    // The API's shorter deadlock detector must choose its query.
                    sqlx::query("SELECT id FROM cache_namespace WHERE id=$1 FOR UPDATE")
                        .bind(namespace)
                        .fetch_one(&mut *barrier)
                        .await
                        .map_err(|e| e.to_string())?;
                }
                _ => {}
            }
            Ok(())
        }
        .await;
        barrier.rollback().await.map_err(|e| e.to_string())?;
        result
    };
    let (operation, control) = tokio::join!(operation, control);
    // Dispose the exact killed connection, never hand uncertain state to the pool.
    let closed = connection.close().await;
    if fault != Fault::Kill {
        closed.unwrap();
    }
    if fault == Fault::Kill {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        loop {
            let present:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_stat_activity WHERE pid=$1 AND backend_start=$2 AND datname=current_database() AND usename=$3)").bind(pid).bind(backend_start).bind(&f.roles[2]).fetch_one(&*f.db).await.unwrap();
            if !present {
                break;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "owned killed backend not joined"
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }
    sqlx::raw_sql(
        "DROP EVENT TRIGGER ws2_cache_acl_barrier; DROP FUNCTION ws2_cache_acl_barrier()",
    )
    .execute(&*f.db)
    .await
    .unwrap();
    assert_eq!(
        acl(f).await,
        baseline,
        "exact ACL/OID restoration failed for {fault:?}"
    );
    observer_attempt(url, cache_owner, f.db.schema())
        .await
        .unwrap();
    assert!(
        control.is_ok(),
        "Option A observer/barrier failure {fault:?}: {control:?}"
    );
    let code = operation
        .as_ref()
        .err()
        .and_then(|e| e.as_database_error())
        .and_then(|e| e.code())
        .map(|s| s.into_owned());
    match fault {
        Fault::Success => operation.unwrap(),
        Fault::ExceptionSavepoint => assert_eq!(code.as_deref(), Some("P0001")),
        Fault::Cancel | Fault::StatementTimeout => assert_eq!(code.as_deref(), Some("57014")),
        Fault::LockTimeout => assert_eq!(code.as_deref(), Some("55P03")),
        Fault::Deadlock => assert_eq!(code.as_deref(), Some("40P01")),
        Fault::Kill => assert!(operation.is_err()),
    }
    let (rows,leaf):(i64,Option<String>)=sqlx::query_as("SELECT (SELECT count(*) FROM cache_generation WHERE id=$1),to_regclass(format('%I.cache_entry_g_%s',current_schema(),$1))::TEXT").bind(generation).fetch_one(&*f.db).await.unwrap();
    let after: i64 =
        sqlx::query_scalar("SELECT partitions_created FROM cache_entry_statistics_state WHERE id")
            .fetch_one(&*f.db)
            .await
            .unwrap();
    if fault == Fault::Success {
        assert_eq!(rows, 1);
        assert!(leaf.is_some());
        assert_eq!(after, statistics + 1);
        let (consumed,owner,schema,parent,bound):(bool,i64,i64,i64,String)=sqlx::query_as("SELECT i.consumed,c.relowner::BIGINT,c.relnamespace::BIGINT,h.inhparent::BIGINT,pg_get_expr(c.relpartbound,c.oid) FROM cache_partition_creation_intent i JOIN pg_class c ON c.oid=to_regclass(format('%I.cache_entry_g_%s',current_schema(),i.generation)) JOIN pg_inherits h ON h.inhrelid=c.oid WHERE i.generation=$1").bind(generation).fetch_one(&*f.db).await.unwrap();
        let expected_owner: i64 =
            sqlx::query_scalar("SELECT oid::BIGINT FROM pg_roles WHERE rolname=$1")
                .bind(cache_owner)
                .fetch_one(&*f.db)
                .await
                .unwrap();
        let expected_parent: i64 =
            sqlx::query_scalar("SELECT 'cache_entry'::regclass::OID::BIGINT")
                .fetch_one(&*f.db)
                .await
                .unwrap();
        assert!(consumed);
        assert_eq!(owner, expected_owner);
        assert_eq!(schema, baseline.0);
        assert_eq!(parent, expected_parent);
        assert_eq!(bound, format!("FOR VALUES IN ('{generation}')"));
    } else {
        assert_eq!(rows, 0);
        assert!(leaf.is_none());
        assert_eq!(after, statistics);
        let residue:i64=sqlx::query_scalar("SELECT (SELECT count(*) FROM cache_partition_creation_intent WHERE generation=$1)+(SELECT count(*) FROM cache_generation_entry_usage WHERE generation=$1)").bind(generation).fetch_one(&*f.db).await.unwrap();
        assert_eq!(residue, 0);
    }
    eprintln!(
        "Option A {fault:?}: exact ACL/OIDs restored; fresh cache-owner CREATE denied before/after"
    );
}

async fn observer_attempt(url: &str, role: &str, schema: &str) -> Result<(), String> {
    let mut fresh = PgConnection::connect(url)
        .await
        .map_err(|e| e.to_string())?;
    let observed = async {
        let mut attempt = fresh.begin().await.map_err(|e| e.to_string())?;
        sqlx::query(&format!("SET LOCAL ROLE {role}"))
            .execute(&mut *attempt)
            .await
            .map_err(|e| e.to_string())?;
        let identity: bool =
            sqlx::query_scalar("SELECT current_user::TEXT=$1 AND session_user<>current_user")
                .bind(role)
                .fetch_one(&mut *attempt)
                .await
                .map_err(|e| e.to_string())?;
        if !identity {
            return Err("wrong cache-owner observer identity".into());
        }
        sqlx::raw_sql("SET LOCAL lock_timeout='500ms'; SET LOCAL statement_timeout='1s'")
            .execute(&mut *attempt)
            .await
            .map_err(|e| e.to_string())?;
        let result = sqlx::query(&format!(
            "CREATE TABLE \"{schema}\".ws2_observer_must_deny(id BIGINT)"
        ))
        .execute(&mut *attempt)
        .await;
        let code = result
            .as_ref()
            .err()
            .and_then(|e| e.as_database_error())
            .and_then(|e| e.code())
            .map(|s| s.into_owned());
        attempt.rollback().await.map_err(|e| e.to_string())?;
        if code.as_deref() != Some("42501") {
            return Err(format!("CREATE observer isolation failed: {code:?}"));
        }
        Ok(())
    }
    .await;
    fresh.close().await.map_err(|e| e.to_string())?;
    observed
}

async fn lost_ack(f: &Fixture, namespace: i64, observer_url: &str, cache_owner: &str) {
    use super::commit_relay::{Class, Decision, OwnedCommitRelay};
    let baseline = acl(f).await;
    let generation: i64 = sqlx::query_scalar("SELECT nextval('cache_generation_id_seq')")
        .fetch_one(&*f.db)
        .await
        .unwrap();
    let mut url = url::Url::parse(f.db.database_url()).unwrap();
    let target = format!("{}:{}", url.host_str().unwrap(), url.port().unwrap_or(5432));
    let mut relay = OwnedCommitRelay::start(target, generation, vec![Class::Publication])
        .await
        .unwrap();
    url.set_username(&f.roles[2]).unwrap();
    url.set_password(Some(&f.passwords[1])).unwrap();
    url.set_port(Some(relay.port)).unwrap();
    url.query_pairs_mut()
        .append_pair(
            "options",
            &format!("-csearch_path={},public", f.db.schema()),
        )
        .append_pair("sslmode", "disable");
    let mut connection = PgConnection::connect(url.as_str()).await.unwrap();
    let mut tx = connection.begin().await.unwrap();
    CacheEntryRepository::protect_transaction(&mut tx, CacheTransactionMode::Attach)
        .await
        .unwrap();
    // Relay is bound to this exact server-reserved generation ID, not a wildcard.
    sqlx::query("INSERT INTO cache_generation(id,namespace,client_refresh_id,expected_chunk_count,expected_count) VALUES ($1,$2,'lost-ack',0,0)").bind(generation).bind(namespace).execute(&mut *tx).await.unwrap();
    sqlx::query("SELECT create_cache_generation_partition($1)")
        .bind(generation)
        .execute(&mut *tx)
        .await
        .unwrap();
    let operation = async { tx.commit().await };
    let control = async {
        let event = tokio::time::timeout(Duration::from_secs(10), relay.next_event())
            .await
            .map_err(|e| e.to_string())?
            .map_err(|e| e.to_string())?;
        if event.class != Class::Publication || event.status != b'I' {
            return Err("wrong durable cache COMMIT event".into());
        }
        let valid = acl(f).await == baseline;
        let observer = observer_attempt(observer_url, cache_owner, f.db.schema()).await;
        event
            .decide(Decision::Disconnect)
            .map_err(|e| e.to_string())?;
        if !valid {
            return Err("durable lost-ack ACL differs".into());
        }
        observer?;
        Ok::<(), String>(())
    };
    let (commit, control) = tokio::join!(operation, control);
    relay.release_holds();
    let _ = connection.close().await;
    let report = relay.shutdown().await.unwrap();
    assert!(control.is_ok(), "{control:?}");
    assert!(commit.is_err());
    assert_eq!(report.publications, 1);
    assert_eq!(report.disconnected, 1);
    assert_eq!(acl(f).await, baseline);
    let consumed: bool = sqlx::query_scalar(
        "SELECT consumed FROM cache_partition_creation_intent WHERE generation=$1",
    )
    .bind(generation)
    .fetch_one(&*f.db)
    .await
    .unwrap();
    assert!(consumed);
}
