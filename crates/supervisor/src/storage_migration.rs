use std::{path::Path, sync::Arc};

use anyhow::{bail, Context, Result};
use attune_common::{
    blob_store::{body_from_file, hash_file, BlobStore, BlobStoreError, ObjectKey, StoredObject},
    repositories::{
        object_maintenance::ObjectMaintenanceRepository,
        storage_maintenance::StorageMaintenanceRepository,
    },
};
use chrono::{Duration, Utc};
use sqlx::PgPool;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MigrationReport {
    pub source_count: u64,
    pub source_bytes: u64,
    pub target_count: u64,
    pub target_bytes: u64,
    pub switched: u64,
}

struct StagedFile {
    kind: &'static str,
    id: i64,
    source_path: String,
    object: StoredObject,
    digest_hex: String,
}

pub async fn migrate(
    pool: &PgPool,
    blob_store: Arc<dyn BlobStore>,
    artifacts_dir: &Path,
    rollback_snapshot_seconds: u64,
) -> Result<MigrationReport> {
    let packs = StorageMaintenanceRepository::legacy_pack_files(pool).await?;
    let artifacts = StorageMaintenanceRepository::legacy_artifact_files(pool).await?;
    let mut staged = Vec::with_capacity(packs.len() + artifacts.len());
    let mut report = MigrationReport::default();

    for pack in &packs {
        let key = ObjectKey::new(format!("packs/blobs/sha256/{}.tar.gz", pack.digest))?;
        ObjectMaintenanceRepository::reserve_upload(pool, key.as_str(), "pack").await?;
        let file = stage_file(&blob_store, "pack", pack.id, &pack.archive_path, key).await?;
        ObjectMaintenanceRepository::record_uploaded(
            pool,
            file.object.key.as_str(),
            file.object.provider_version.as_stored(),
            file.object.size as i64,
        )
        .await?;
        if file.digest_hex != pack.digest || file.object.size != pack.archive_size as u64 {
            bail!(
                "pack release {} source does not match recorded size or SHA-256",
                pack.id
            );
        }
        add_verified(&mut report, &file);
        staged.push(file);
    }
    for artifact in &artifacts {
        let source = artifacts_dir.join(&artifact.file_path);
        let key = ObjectKey::new(format!(
            "artifacts/{}/v{}",
            artifact.artifact, artifact.version
        ))?;
        ObjectMaintenanceRepository::reserve_upload(pool, key.as_str(), "artifact").await?;
        let file = stage_file(
            &blob_store,
            "artifact",
            artifact.id,
            &source.to_string_lossy(),
            key,
        )
        .await?;
        ObjectMaintenanceRepository::record_uploaded(
            pool,
            file.object.key.as_str(),
            file.object.provider_version.as_stored(),
            file.object.size as i64,
        )
        .await?;
        if artifact
            .size_bytes
            .is_some_and(|size| size != file.object.size as i64)
        {
            bail!(
                "artifact version {} source does not match recorded size",
                artifact.id
            );
        }
        add_verified(&mut report, &file);
        staged.push(file);
    }

    if report.source_count != report.target_count || report.source_bytes != report.target_bytes {
        bail!("storage migration source and target totals do not match");
    }

    // Re-read every source after all uploads. Metadata never switches if a file
    // changed while the migration was staging the remaining objects.
    for file in &staged {
        let (size, digest) = hash_file(&file.source_path).await.with_context(|| {
            format!(
                "failed to re-read {} source {}",
                file.kind, file.source_path
            )
        })?;
        if size != file.object.size || encode_digest(&digest) != file.digest_hex {
            bail!(
                "{} source {} changed during migration",
                file.kind,
                file.source_path
            );
        }
    }

    let snapshot_expires_at =
        Utc::now() + Duration::seconds(rollback_snapshot_seconds.min(i64::MAX as u64) as i64);
    let mut tx = pool.begin().await?;
    for file in &staged {
        let switched = match file.kind {
            "pack" => {
                StorageMaintenanceRepository::switch_pack_file(
                    &mut tx,
                    file.id,
                    file.object.key.as_str(),
                    file.object.provider_version.as_stored(),
                    snapshot_expires_at,
                )
                .await?
            }
            "artifact" => {
                StorageMaintenanceRepository::switch_artifact_file(
                    &mut tx,
                    file.id,
                    file.object.key.as_str(),
                    file.object.provider_version.as_stored(),
                    file.object.size as i64,
                    &file.digest_hex,
                    snapshot_expires_at,
                )
                .await?
            }
            _ => unreachable!(),
        };
        if !switched {
            bail!(
                "{} row {} changed before metadata switch",
                file.kind,
                file.id
            );
        }
        report.switched += 1;
    }
    tx.commit().await?;
    Ok(report)
}

async fn stage_file(
    blob_store: &Arc<dyn BlobStore>,
    kind: &'static str,
    id: i64,
    source_path: &str,
    key: ObjectKey,
) -> Result<StagedFile> {
    let (size, digest) = hash_file(source_path)
        .await
        .with_context(|| format!("failed to hash {kind} source {source_path}"))?;
    let body = body_from_file(source_path)
        .await
        .with_context(|| format!("failed to open {kind} source {source_path}"))?;
    let object = match blob_store.put(&key, body, digest).await {
        Ok(object) => object,
        Err(BlobStoreError::Conflict) => blob_store
            .head(&key)
            .await?
            .ok_or_else(|| anyhow::anyhow!("object conflict disappeared for {}", key.as_str()))?,
        Err(error) => return Err(error.into()),
    };
    if object.size != size || object.sha256 != digest {
        bail!(
            "{kind} target {} failed size or SHA-256 verification",
            key.as_str()
        );
    }
    Ok(StagedFile {
        kind,
        id,
        source_path: source_path.to_string(),
        object,
        digest_hex: encode_digest(&digest),
    })
}

fn add_verified(report: &mut MigrationReport, file: &StagedFile) {
    report.source_count += 1;
    report.source_bytes += file.object.size;
    report.target_count += 1;
    report.target_bytes += file.object.size;
}

fn encode_digest(digest: &[u8; 32]) -> String {
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use attune_common::blob_store::{FilesystemBlobStore, ProviderVersion};
    use attune_common::{
        config::Config,
        models::enums::{
            ArtifactClassification, ArtifactType, ArtifactVisibility, OwnerType,
            RetentionPolicyType,
        },
        repositories::{
            artifact::{
                ArtifactRepository, ArtifactVersionRepository, CreateArtifactInput,
                CreateArtifactVersionInput,
            },
            Create,
        },
        test_database::TestDatabase,
    };
    use futures::TryStreamExt;

    #[tokio::test]
    async fn staging_is_restartable_and_rejects_changed_target() {
        let source = tempfile::NamedTempFile::new().unwrap();
        tokio::fs::write(source.path(), b"source bytes")
            .await
            .unwrap();
        let target = tempfile::tempdir().unwrap();
        let store: Arc<dyn BlobStore> = Arc::new(FilesystemBlobStore::new(target.path()).unwrap());
        let key = ObjectKey::new("migration/restart").unwrap();

        let first = stage_file(
            &store,
            "artifact",
            1,
            &source.path().to_string_lossy(),
            key.clone(),
        )
        .await
        .unwrap();
        let second = stage_file(
            &store,
            "artifact",
            1,
            &source.path().to_string_lossy(),
            key.clone(),
        )
        .await
        .unwrap();
        assert_eq!(first.digest_hex, second.digest_hex);

        let version =
            ProviderVersion::from_stored(second.object.provider_version.as_stored().to_string())
                .unwrap();
        let bytes = store
            .get(&key, &version, None)
            .await
            .unwrap()
            .try_collect::<Vec<_>>()
            .await
            .unwrap()
            .concat();
        assert_eq!(bytes, b"source bytes");

        tokio::fs::write(source.path(), b"different").await.unwrap();
        assert!(
            stage_file(&store, "artifact", 1, &source.path().to_string_lossy(), key,)
                .await
                .is_err()
        );
    }

    async fn test_database() -> TestDatabase {
        let path = format!("{}/../../config.test.yaml", env!("CARGO_MANIFEST_DIR"));
        let config = Config::load_from_file(&path).unwrap();
        TestDatabase::create(&config.database)
            .await
            .unwrap()
            .with_cleanup_on_drop()
    }

    async fn legacy_artifact(pool: &PgPool, file_path: &str, size_bytes: Option<i64>) -> i64 {
        let unique = uuid::Uuid::new_v4().simple().to_string();
        let artifact = ArtifactRepository::create(
            pool,
            CreateArtifactInput {
                r#ref: format!("migration_{unique}"),
                scope: OwnerType::System,
                owner: "migration-test".to_string(),
                r#type: ArtifactType::FileBinary,
                visibility: ArtifactVisibility::Private,
                classification: ArtifactClassification::General,
                retention_policy: RetentionPolicyType::Versions,
                retention_limit: 5,
                name: None,
                description: None,
                content_type: None,
                data: None,
            },
        )
        .await
        .unwrap();
        let version = ArtifactVersionRepository::create(
            pool,
            CreateArtifactVersionInput {
                artifact: artifact.id,
                execution: None,
                content_type: Some("application/octet-stream".to_string()),
                content: None,
                content_json: None,
                file_path: Some(file_path.to_string()),
                meta: None,
                created_by: None,
            },
        )
        .await
        .unwrap();
        if let Some(size_bytes) = size_bytes {
            ArtifactVersionRepository::update_size_bytes(pool, version.id, size_bytes)
                .await
                .unwrap();
        }
        version.id
    }

    #[tokio::test]
    #[ignore = "integration test - requires database"]
    async fn migration_switches_only_after_verification_and_restart_is_a_noop() {
        let database = test_database().await;
        let artifacts = tempfile::tempdir().unwrap();
        tokio::fs::create_dir(artifacts.path().join("legacy"))
            .await
            .unwrap();
        tokio::fs::write(artifacts.path().join("legacy/v1.bin"), b"seven!!")
            .await
            .unwrap();
        let version_id = legacy_artifact(&database, "legacy/v1.bin", Some(7)).await;
        let target = tempfile::tempdir().unwrap();
        let store: Arc<dyn BlobStore> = Arc::new(FilesystemBlobStore::new(target.path()).unwrap());

        let report = migrate(&database, store.clone(), artifacts.path(), 3600)
            .await
            .unwrap();
        assert_eq!(
            report,
            MigrationReport {
                source_count: 1,
                source_bytes: 7,
                target_count: 1,
                target_bytes: 7,
                switched: 1,
            }
        );
        assert!(artifacts.path().join("legacy/v1.bin").exists());
        let state: String =
            sqlx::query_scalar("SELECT body_state::TEXT FROM artifact_version WHERE id = $1")
                .bind(version_id)
                .fetch_one(&*database)
                .await
                .unwrap();
        assert_eq!(state, "ready");

        assert_eq!(
            migrate(&database, store, artifacts.path(), 3600)
                .await
                .unwrap(),
            MigrationReport::default()
        );
    }

    #[tokio::test]
    #[ignore = "integration test - requires database"]
    async fn failed_verification_leaves_metadata_unswitched() {
        let database = test_database().await;
        let artifacts = tempfile::tempdir().unwrap();
        tokio::fs::create_dir(artifacts.path().join("legacy"))
            .await
            .unwrap();
        tokio::fs::write(artifacts.path().join("legacy/bad.bin"), b"body")
            .await
            .unwrap();
        let version_id = legacy_artifact(&database, "legacy/bad.bin", Some(999)).await;
        let target = tempfile::tempdir().unwrap();
        let store: Arc<dyn BlobStore> = Arc::new(FilesystemBlobStore::new(target.path()).unwrap());

        assert!(migrate(&database, store, artifacts.path(), 3600)
            .await
            .is_err());
        let state: Option<String> =
            sqlx::query_scalar("SELECT body_state::TEXT FROM artifact_version WHERE id = $1")
                .bind(version_id)
                .fetch_one(&*database)
                .await
                .unwrap();
        assert_eq!(state, None);
    }
}
