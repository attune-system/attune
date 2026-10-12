use attune_common::{
    blob_store::{ObjectKey, ProviderVersion, StoredObject},
    config::Config,
    models::PackInstallStatus,
    repositories::{object_maintenance::ObjectMaintenanceRepository, PackInstallRepository},
    test_database::TestDatabase,
};
use chrono::{Duration, Utc};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

fn candidate(id: i64) -> StoredObject {
    StoredObject {
        key: ObjectKey::new(format!("packs/candidates/{id}.tar.gz")).unwrap(),
        provider_version: ProviderVersion::from_stored("v:candidate-version").unwrap(),
        size: 123,
        sha256: [0xab; 32],
    }
}

async fn attach(pool: &sqlx::PgPool, id: i64, object: &StoredObject) -> Result<()> {
    let mut tx = pool.begin().await?;
    ObjectMaintenanceRepository::reserve_upload(&mut *tx, object.key.as_str(), "pack").await?;
    ObjectMaintenanceRepository::record_uploaded(
        &mut *tx,
        object.key.as_str(),
        object.provider_version.as_stored(),
        object.size as i64,
    )
    .await?;
    assert!(PackInstallRepository::attach_candidate_object(&mut tx, id, object).await?);
    tx.commit().await?;
    Ok(())
}

#[tokio::test]
async fn candidate_object_binding_is_write_once_and_pending_only() -> Result<()> {
    let database = TestDatabase::create(&Config::load()?.database).await?;
    let pool = database.pool();
    let repo = PackInstallRepository::new(pool.clone());
    let install = repo
        .create("candidate", "1.0.0", "install", None, None)
        .await?;
    let object = candidate(install.id);
    attach(pool, install.id, &object).await?;
    let stored = repo.find_by_id(install.id).await?.unwrap();
    assert_eq!(
        stored.candidate_object_key.as_deref(),
        Some(object.key.as_str())
    );
    assert_eq!(
        stored.candidate_provider_version.as_deref(),
        Some(object.provider_version.as_stored())
    );
    assert_eq!(stored.candidate_archive_size, Some(123));
    assert_eq!(stored.candidate_digest, Some("ab".repeat(32)));
    assert!(serde_json::to_value(&stored)?
        .get("candidateObjectKey")
        .is_none());
    let mut tx = pool.begin().await?;
    assert!(!PackInstallRepository::attach_candidate_object(&mut tx, install.id, &object).await?);
    tx.rollback().await?;

    let claimed = repo
        .create("claimed", "1.0.0", "manual", None, None)
        .await?;
    repo.claim_worker(claimed.id, 42, None).await?.unwrap();
    let mut tx = pool.begin().await?;
    assert!(!PackInstallRepository::attach_candidate_object(&mut tx, claimed.id, &object).await?);
    tx.rollback().await?;
    let expired = repo
        .create("expired", "1.0.0", "install", None, None)
        .await?;
    sqlx::query("UPDATE pack_install SET started_at = NOW() - INTERVAL '8 hours' WHERE id = $1")
        .bind(expired.id)
        .execute(pool)
        .await?;
    let mut tx = pool.begin().await?;
    assert!(!PackInstallRepository::attach_candidate_object(&mut tx, expired.id, &object).await?);
    tx.rollback().await?;
    database.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn candidate_objects_are_pinned_through_testing_and_activation_then_reclaimed() -> Result<()>
{
    let database = TestDatabase::create(&Config::load()?.database).await?;
    let pool = database.pool();
    let repo = PackInstallRepository::new(pool.clone());
    let install = repo
        .create("candidate", "1.0.0", "install", None, None)
        .await?;
    let object = candidate(install.id);
    attach(pool, install.id, &object).await?;
    for status in ["pending", "running", "activating"] {
        if status == "running" {
            repo.claim_worker(install.id, 42, Some("test-hash"))
                .await?
                .unwrap();
        } else if status == "activating" {
            repo.finish_running(install.id, PackInstallStatus::Activating, None, None, None)
                .await?
                .unwrap();
        }
        assert!(
            ObjectMaintenanceRepository::exact_reference_exists(
                pool,
                object.key.as_str(),
                object.provider_version.as_stored(),
            )
            .await?
        );
        assert!(
            !ObjectMaintenanceRepository::exact_reference_exists(
                pool,
                object.key.as_str(),
                "v:wrong-version",
            )
            .await?
        );
        assert_eq!(
            ObjectMaintenanceRepository::schedule_unreferenced(
                pool,
                Utc::now() + Duration::hours(1),
                Utc::now() - Duration::hours(1),
            )
            .await?,
            0,
            "{status} must pin the exact candidate"
        );
        // A stale queued deletion must also recheck the active reference.
        sqlx::query("UPDATE object_maintenance_ledger SET state = 'deletion_pending', eligible_at = NOW() - INTERVAL '1 hour' WHERE object_key = $1")
            .bind(object.key.as_str()).execute(pool).await?;
        assert!(
            ObjectMaintenanceRepository::claim_deletions(pool, Utc::now(), 10)
                .await?
                .is_empty()
        );
        sqlx::query("UPDATE object_maintenance_ledger SET state = 'ready', eligible_at = NULL WHERE object_key = $1")
            .bind(object.key.as_str()).execute(pool).await?;
    }
    repo.finish_activation(install.id, PackInstallStatus::Succeeded, None)
        .await?
        .unwrap();
    assert!(
        !ObjectMaintenanceRepository::exact_reference_exists(
            pool,
            object.key.as_str(),
            object.provider_version.as_stored(),
        )
        .await?
    );
    assert_eq!(
        ObjectMaintenanceRepository::schedule_unreferenced(
            pool,
            Utc::now() + Duration::hours(1),
            Utc::now() - Duration::hours(1),
        )
        .await?,
        1
    );
    assert_eq!(
        ObjectMaintenanceRepository::claim_deletions(pool, Utc::now(), 10)
            .await?
            .len(),
        1
    );
    database.cleanup().await?;
    Ok(())
}

#[tokio::test]
async fn abandoned_candidate_objects_do_not_pin_storage_forever() -> Result<()> {
    let database = TestDatabase::create(&Config::load()?.database).await?;
    let pool = database.pool();
    let repo = PackInstallRepository::new(pool.clone());
    for status in ["pending", "running", "activating", "failed", "rolled_back"] {
        let install = repo.create(status, "1.0.0", "install", None, None).await?;
        let object = candidate(install.id);
        attach(pool, install.id, &object).await?;
        sqlx::query("UPDATE pack_install SET status = $2, started_at = NOW() - INTERVAL '8 hours', updated_at = NOW() - INTERVAL '1 hour' WHERE id = $1")
            .bind(install.id).bind(status).execute(pool).await?;
        assert!(
            !ObjectMaintenanceRepository::exact_reference_exists(
                pool,
                object.key.as_str(),
                object.provider_version.as_stored(),
            )
            .await?,
            "{status} must release its expired candidate"
        );
    }
    assert_eq!(
        ObjectMaintenanceRepository::schedule_unreferenced(
            pool,
            Utc::now() + Duration::hours(1),
            Utc::now() - Duration::hours(1),
        )
        .await?,
        5
    );
    assert_eq!(
        ObjectMaintenanceRepository::claim_deletions(pool, Utc::now(), 10)
            .await?
            .len(),
        5
    );
    database.cleanup().await?;
    Ok(())
}
