//! PostgreSQL authorization and freshness boundaries for the candidate transport.
use super::*;
use attune_common::repositories::{
    recorded_consumer_observations::{
        RecordedConsumerIdentity, RecordedConsumerObservations as Observations,
    },
    rollout_admin::RolloutAdminRepository,
};
use futures::FutureExt;

async fn observer_login(f: &mut Fixture, can_report: bool) -> PgPool {
    let name = format!("ws2_observer_{}", Uuid::new_v4().simple());
    let password = format!("dummy_{}", Uuid::new_v4().simple());
    let ddl: String =
        sqlx::query_scalar("SELECT format('CREATE ROLE %I LOGIN PASSWORD %L',$1::TEXT,$2::TEXT)")
            .bind(&name)
            .bind(&password)
            .fetch_one(&*f.db)
            .await
            .unwrap();
    sqlx::query(&ddl).execute(&*f.db).await.unwrap();
    f.roles.push(name.clone());
    std::fs::write(&f.manifest, serde_json::to_vec(&f.roles).unwrap()).unwrap();
    let function = if can_report {
        "report_recorded_consumer(JSONB, UUID, UUID, UUID, INTEGER, BIGINT, BOOLEAN)"
    } else {
        "current_recorded_consumer_evidence(UUID)"
    };
    sqlx::raw_sql(&format!(
        "GRANT USAGE ON SCHEMA \"{}\" TO {name}; GRANT EXECUTE ON FUNCTION {function} TO {name}",
        f.db.schema()
    ))
    .execute(&*f.db)
    .await
    .unwrap();

    let mut url = url::Url::parse(f.db.database_url()).unwrap();
    url.set_username(&name).unwrap();
    url.set_password(Some(&password)).unwrap();
    url.query_pairs_mut().append_pair(
        "options",
        &format!("-csearch_path={},public", f.db.schema()),
    );
    let pool = PgPoolOptions::new()
        .max_connections(if can_report { 2 } else { 1 })
        .connect(url.as_str())
        .await
        .unwrap();
    f.pools.push(pool.clone());
    pool
}

async fn report(
    pool: &PgPool,
    process: &RecordedConsumerIdentity,
    role: Option<ConsumerRole>,
) -> attune_common::Result<()> {
    process.publish(&mut *pool.acquire().await?, role).await
}

fn process_identity(
    snapshot: &ContractSnapshot,
    member: &ExpectedMember,
) -> RecordedConsumerIdentity {
    serde_json::from_value(serde_json::json!({
        "snapshot": snapshot,
        "instance": member.instance,
        "boot": member.boot,
    }))
    .unwrap()
}

async fn evidence_ready(pool: &PgPool) -> bool {
    let mut tx = pool.begin().await.unwrap();
    let result = Observations::require_current(&mut tx).await;
    tx.rollback().await.unwrap();
    match result {
        Ok(_) => true,
        Err(attune_common::Error::PermissionDenied(_)) => false,
        Err(error) => panic!("unexpected evidence error: {error:?}"),
    }
}

async fn exercise(f: &mut Fixture) {
    super::admin_process::provision(f).await;
    let mut install = f.db.begin().await.unwrap();
    sqlx::query(&format!("SET LOCAL ROLE {}", f.roles[4]))
        .execute(&mut *install)
        .await
        .unwrap();
    sqlx::raw_sql(include_str!(
        "../../../../deployment/rollout-admin/consumer-observations.candidate.sql"
    ))
    .execute(&mut *install)
    .await
    .unwrap();
    install.commit().await.unwrap();

    let mut expected = membership();
    let mut sibling = expected.members[0].clone();
    sibling.instance = Uuid::new_v4();
    sibling.boot = Uuid::new_v4();
    expected.members.push(sibling);
    let body = serde_json::to_value(&expected).unwrap();
    let operator = f.pools[0].clone();
    let receipt = RolloutAdminRepository::stage(&mut operator.acquire().await.unwrap(), &body)
        .await
        .unwrap();
    let reader = observer_login(f, false).await;
    assert!(!evidence_ready(&reader).await);

    let mut reporters = Vec::new();
    for member in &expected.members {
        let reporter = observer_login(f, true).await;
        let role = f.roles.last().unwrap();
        sqlx::query("SELECT bind_recorded_consumer($1,$2,$3,$4::NAME)")
            .bind(expected.snapshot.rollout_generation)
            .bind(member.instance)
            .bind(member.boot)
            .bind(role)
            .execute(&*f.db)
            .await
            .unwrap();
        reporters.push(reporter);
    }

    let first = &expected.members[0];
    let peer = expected.members.last().unwrap();
    let first_login = &reporters[0];
    let identities: Vec<_> = expected
        .members
        .iter()
        .map(|member| process_identity(&expected.snapshot, member))
        .collect();
    let first_identity = &identities[0];
    let peer_identity = identities.last().unwrap();
    assert!(
        serde_json::from_value::<RecordedConsumerIdentity>(serde_json::json!({
            "snapshot": expected.snapshot,
            "instance": first.instance,
            "boot": first.boot,
            "process_claim": Uuid::new_v4(),
        }))
        .is_err()
    );
    let worker_index = expected
        .members
        .iter()
        .position(|member| member.role == ConsumerRole::Worker)
        .unwrap();
    let worker = &expected.members[worker_index];
    let worker_identity = &identities[worker_index];
    let worker_login = &reporters[worker_index];
    let worker_capability = serde_json::json!({
        (attune_common::execution_authority::RECORDED_EXECUTION_CAPABILITY): {
            "snapshot": expected.snapshot,
            "instance": worker.instance,
            "boot": worker.boot,
        }
    });
    let worker_id: i64 = sqlx::query_scalar(
        "INSERT INTO worker (name, worker_type, worker_role, status, capabilities, meta, last_heartbeat) \
         VALUES ('consumer-observation-fixture','local','action','active',$1, \
             jsonb_build_object('registration_incarnation',$2::uuid),clock_timestamp()) RETURNING id",
    )
    .bind(&worker_capability)
    .bind(worker.boot)
    .fetch_one(&*f.db)
    .await
    .unwrap();
    let duplicate = sqlx::query(
        "INSERT INTO worker (name, worker_type, worker_role, status, capabilities, meta, last_heartbeat) \
         SELECT 'duplicate-observation-fixture', worker_type, worker_role, status, capabilities, meta, last_heartbeat \
         FROM worker WHERE id=$1",
    )
    .bind(worker_id)
    .execute(&*f.db)
    .await
    .unwrap_err();
    assert_eq!(
        duplicate.as_database_error().unwrap().code().as_deref(),
        Some("23505")
    );
    assert!(
        report(worker_login, worker_identity, Some(worker.role))
            .await
            .is_err(),
        "a worker report always names a compatible registration"
    );
    let publish_worker = |pool: PgPool, id| async move {
        worker_identity
            .publish_worker(&mut *pool.acquire().await?, id)
            .await
    };
    assert!(publish_worker(first_login.clone(), worker_id)
        .await
        .is_err());
    assert!(
        publish_worker(worker_login.clone(), worker_id)
            .await
            .is_err(),
        "publication requires a prior worker reservation"
    );
    let mut tx = first_login.begin().await.unwrap();
    assert!(first_identity.reserve_worker_boot(&mut tx).await.is_err());
    tx.rollback().await.unwrap();
    let mut tx = worker_login.begin().await.unwrap();
    assert_eq!(
        worker_identity.reserve_worker_boot(&mut tx).await.unwrap(),
        worker.boot
    );
    tx.rollback().await.unwrap();
    assert!(
        publish_worker(worker_login.clone(), worker_id)
            .await
            .is_err(),
        "a rolled-back reservation cannot publish"
    );
    let mut tx = worker_login.begin().await.unwrap();
    worker_identity.reserve_worker_boot(&mut tx).await.unwrap();
    tx.commit().await.unwrap();
    assert!(
        !evidence_ready(&reader).await,
        "reservation is not readiness"
    );
    let replacement_worker = process_identity(&expected.snapshot, worker);
    let mut tx = worker_login.begin().await.unwrap();
    assert!(replacement_worker
        .reserve_worker_boot(&mut tx)
        .await
        .is_err());
    tx.rollback().await.unwrap();
    assert!(publish_worker(worker_login.clone(), i64::MAX)
        .await
        .is_err());
    publish_worker(worker_login.clone(), worker_id)
        .await
        .unwrap();
    assert!(report(first_login, peer_identity, Some(peer.role))
        .await
        .is_err());
    assert!(
        report(first_login, first_identity, Some(ConsumerRole::Worker))
            .await
            .is_err()
    );
    let mut wrong_snapshot = expected.snapshot.clone();
    wrong_snapshot.release_digest = "b".repeat(64);
    assert!(report(
        first_login,
        &process_identity(&wrong_snapshot, first),
        Some(first.role)
    )
    .await
    .is_err());
    let mut wrong_boot = first.clone();
    wrong_boot.boot = Uuid::new_v4();
    assert!(report(
        first_login,
        &process_identity(&expected.snapshot, &wrong_boot),
        Some(first.role)
    )
    .await
    .is_err());
    assert!(
        sqlx::query("UPDATE recorded_consumer_observation SET observed = clock_timestamp()")
            .execute(first_login)
            .await
            .is_err()
    );
    assert!(
        sqlx::query("SELECT current_recorded_consumer_evidence(NULL)")
            .execute(first_login)
            .await
            .is_err()
    );

    for (index, member) in expected.members[..expected.members.len() - 1]
        .iter()
        .enumerate()
    {
        if member.role != ConsumerRole::Worker {
            report(&reporters[index], &identities[index], Some(member.role))
                .await
                .unwrap();
        }
    }
    RolloutAdminRepository::activate(&mut operator.acquire().await.unwrap(), &receipt)
        .await
        .unwrap();
    assert!(
        !evidence_ready(&reader).await,
        "every replica is required, including a second API"
    );
    let peer_login = reporters.last().unwrap();
    let second_process = process_identity(&expected.snapshot, peer);
    let (first_claim, second_claim) = tokio::join!(
        report(peer_login, peer_identity, Some(peer.role)),
        report(peer_login, &second_process, Some(peer.role)),
    );
    assert_ne!(first_claim.is_ok(), second_claim.is_ok());
    let (peer_identity, restarted_peer) = if first_claim.is_ok() {
        (peer_identity, &second_process)
    } else {
        (&second_process, peer_identity)
    };
    assert!(evidence_ready(&reader).await);

    let first_name: String = sqlx::query_scalar("SELECT session_user::TEXT")
        .fetch_one(first_login)
        .await
        .unwrap();
    let permit = format!(
        "GRANT EXECUTE ON FUNCTION current_recorded_consumer_evidence(UUID) TO {first_name}"
    );
    sqlx::query(&permit).execute(&*f.db).await.unwrap();
    let mut tx = first_login.begin().await.unwrap();
    let evidence = first_identity
        .require_current(&mut tx, first.role)
        .await
        .unwrap();
    assert!(evidence.matches_worker(worker_id, worker.boot, chrono::Utc::now()));
    assert!(!evidence.matches_worker(i64::MAX, worker.boot, chrono::Utc::now()));
    assert!(!evidence.matches_worker(worker_id, peer.boot, chrono::Utc::now()));
    assert!(!evidence.matches_worker(
        worker_id,
        worker.boot,
        chrono::Utc::now() + chrono::TimeDelta::seconds(30),
    ));
    assert!(peer_identity
        .require_current(&mut tx, first.role)
        .await
        .is_err());
    assert!(first_identity
        .require_current(&mut tx, ConsumerRole::Executor)
        .await
        .is_err());
    let restarted_first = process_identity(&expected.snapshot, first);
    assert!(restarted_first
        .require_current(&mut tx, first.role)
        .await
        .is_err());
    tx.rollback().await.unwrap();

    sqlx::query(
        "UPDATE worker SET last_heartbeat=clock_timestamp()-interval '25 seconds' WHERE id=$1",
    )
    .bind(worker_id)
    .execute(&*f.db)
    .await
    .unwrap();
    publish_worker(worker_login.clone(), worker_id)
        .await
        .unwrap();
    let mut tx = first_login.begin().await.unwrap();
    let evidence = first_identity
        .require_current(&mut tx, first.role)
        .await
        .unwrap();
    assert!(!evidence.matches_worker(
        worker_id,
        worker.boot,
        chrono::Utc::now() + chrono::TimeDelta::seconds(10),
    ));
    tx.rollback().await.unwrap();

    // A new runtime process can replace the row while the observation is still
    // fresh. Replacement or heartbeat loss must invalidate it immediately.
    sqlx::query(
        "UPDATE worker SET last_heartbeat=clock_timestamp()-interval '31 seconds' WHERE id=$1",
    )
    .bind(worker_id)
    .execute(&*f.db)
    .await
    .unwrap();
    assert!(!evidence_ready(&reader).await);
    assert!(publish_worker(worker_login.clone(), worker_id)
        .await
        .is_err());
    sqlx::query("UPDATE worker SET last_heartbeat=clock_timestamp(),meta=jsonb_build_object('registration_incarnation',$2::uuid) WHERE id=$1")
        .bind(worker_id).bind(Uuid::new_v4()).execute(&*f.db).await.unwrap();
    assert!(!evidence_ready(&reader).await);
    assert!(publish_worker(worker_login.clone(), worker_id)
        .await
        .is_err());
    sqlx::query("UPDATE worker SET last_heartbeat=clock_timestamp(),meta=jsonb_build_object('registration_incarnation',$2::uuid),capabilities='{}'::jsonb WHERE id=$1")
        .bind(worker_id).bind(worker.boot).execute(&*f.db).await.unwrap();
    assert!(!evidence_ready(&reader).await);
    assert!(publish_worker(worker_login.clone(), worker_id)
        .await
        .is_err());
    sqlx::query("UPDATE worker SET capabilities=$2 WHERE id=$1")
        .bind(worker_id)
        .bind(&worker_capability)
        .execute(&*f.db)
        .await
        .unwrap();
    publish_worker(worker_login.clone(), worker_id)
        .await
        .unwrap();
    assert!(evidence_ready(&reader).await);
    let mut tx = reader.begin().await.unwrap();
    assert!(first_identity
        .require_current(&mut tx, first.role)
        .await
        .is_err());
    tx.rollback().await.unwrap();

    // An active manifest only describes this catalog revision.
    sqlx::query("UPDATE platform_catalog_state SET revision=revision+1 WHERE singleton")
        .execute(&*f.db)
        .await
        .unwrap();
    assert!(!evidence_ready(&reader).await);
    sqlx::query("UPDATE platform_catalog_state SET revision=$1 WHERE singleton")
        .bind(expected.snapshot.catalog_revision)
        .execute(&*f.db)
        .await
        .unwrap();
    assert!(evidence_ready(&reader).await);

    let peer_role = f.roles.last().unwrap();
    let mut impersonation = f.pools[1].begin().await.unwrap();
    // Explicit EXECUTE and SET are still insufficient: report is bound to session_user.
    sqlx::query(&format!("GRANT {peer_role} TO {}", f.roles[2]))
        .execute(&*f.db)
        .await
        .unwrap();
    sqlx::query(&format!("SET LOCAL ROLE {peer_role}"))
        .execute(&mut *impersonation)
        .await
        .unwrap();
    assert!(peer_identity
        .publish(&mut impersonation, Some(peer.role))
        .await
        .is_err());
    impersonation.rollback().await.unwrap();
    assert!(
        !evidence_ready(&reader).await,
        "a shared reporter login cannot be counted"
    );
    sqlx::query(&format!("REVOKE {peer_role} FROM {}", f.roles[2]))
        .execute(&*f.db)
        .await
        .unwrap();

    sqlx::query("UPDATE recorded_consumer_observation SET observed = clock_timestamp() - interval '30 seconds' WHERE instance=$1")
        .bind(peer.instance).execute(&*f.db).await.unwrap();
    assert!(!evidence_ready(&reader).await);
    assert!(report(peer_login, restarted_peer, Some(peer.role))
        .await
        .is_err());
    report(peer_login, peer_identity, Some(peer.role))
        .await
        .unwrap();
    assert!(evidence_ready(&reader).await);
    assert!(report(peer_login, restarted_peer, None).await.is_err());
    assert!(evidence_ready(&reader).await);
    report(peer_login, peer_identity, None).await.unwrap();
    assert!(!evidence_ready(&reader).await);
    assert!(report(peer_login, restarted_peer, Some(peer.role))
        .await
        .is_err());
    report(peer_login, peer_identity, Some(peer.role))
        .await
        .unwrap();

    let mut held = reader.begin().await.unwrap();
    Observations::require_current(&mut held).await.unwrap();
    let mut dba = f.db.begin().await.unwrap();
    let blocked = sqlx::query(
        "SELECT generation FROM recorded_rollout_active WHERE singleton FOR UPDATE NOWAIT",
    )
    .execute(&mut *dba)
    .await
    .unwrap_err();
    assert_eq!(
        blocked
            .as_database_error()
            .and_then(|e| e.code())
            .as_deref(),
        Some("55P03")
    );
    dba.rollback().await.unwrap();
    held.rollback().await.unwrap();

    let mut next = expected.clone();
    next.snapshot.rollout_generation = Uuid::new_v4();
    let next_receipt = RolloutAdminRepository::stage(
        &mut operator.acquire().await.unwrap(),
        &serde_json::to_value(&next).unwrap(),
    )
    .await
    .unwrap();
    RolloutAdminRepository::activate(&mut operator.acquire().await.unwrap(), &next_receipt)
        .await
        .unwrap();
    assert!(
        !evidence_ready(&reader).await,
        "observations from the prior rollout never count"
    );
}

#[tokio::test]
async fn authenticated_observations_require_every_exact_fresh_replica() {
    let mut fixture = Fixture::new(false).await;
    let result = std::panic::AssertUnwindSafe(exercise(&mut fixture))
        .catch_unwind()
        .await;
    fixture.cleanup().await;
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
}
