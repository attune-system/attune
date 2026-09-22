//! Upgrade installed packs to immutable releases after the release schema is added.

use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use attune_common::{
    blob_store::{body_from_file, hash_file, BlobStore, BlobStoreError, ObjectKey},
    models::Pack,
    pack_registry::PackStorage,
    repositories::{
        component_lifecycle::ComponentLifecycleRepository,
        object_maintenance::ObjectMaintenanceRepository,
        pack::PackRepository,
        pack_release::{CreatePackReleaseInput, PackReleaseRepository},
        FindById,
    },
};
use sqlx::PgPool;

#[derive(Debug, Default, PartialEq, Eq)]
pub struct PackReleaseUpgradeReport {
    pub upgraded: Vec<String>,
    pub failures: Vec<PackReleaseUpgradeFailure>,
}

#[derive(Debug, PartialEq, Eq)]
pub struct PackReleaseUpgradeFailure {
    pub pack_ref: String,
    pub error: String,
}

/// Freeze each legacy pack's current installed directory into a real release.
///
/// A failed pack remains unchanged so readiness can keep the service out of
/// traffic until an operator restores and re-registers its bytes.
pub async fn upgrade_legacy_pack_releases(
    pool: &PgPool,
    blob_store: &dyn BlobStore,
    packs_base_dir: &Path,
) -> Result<PackReleaseUpgradeReport> {
    let mut scan = pool.begin().await?;
    let packs = PackRepository::list_requiring_release(&mut *scan).await?;
    scan.commit().await?;

    let storage = PackStorage::new(packs_base_dir);
    let mut report = PackReleaseUpgradeReport::default();
    for pack in packs {
        let pack_ref = pack.r#ref.clone();
        match upgrade_pack(pool, blob_store, &storage, pack).await {
            Ok(()) => report.upgraded.push(pack_ref),
            Err(error) => report.failures.push(PackReleaseUpgradeFailure {
                pack_ref,
                error: format!("{error:#}"),
            }),
        }
    }
    Ok(report)
}

async fn upgrade_pack(
    pool: &PgPool,
    blob_store: &dyn BlobStore,
    storage: &PackStorage,
    pack: Pack,
) -> Result<()> {
    let source = match (pack.active_release, pack.storage_path.as_deref()) {
        (Some(_), _) | (None, None) => storage.get_pack_path(&pack.r#ref, None)?,
        (None, Some(path)) => PathBuf::from(path),
    };
    let metadata = std::fs::symlink_metadata(&source).map_err(|error| {
        anyhow!(
            "installed directory '{}' for pack '{}' is unavailable; restore the exact installed bytes and force-register it: {error}",
            source.display(),
            pack.r#ref
        )
    })?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(anyhow!(
            "installed directory '{}' for pack '{}' is not a real directory; restore the exact installed bytes and force-register it",
            source.display(),
            pack.r#ref
        ));
    }

    let published = storage
        .publish_release(&source, &pack.r#ref, &pack.version)
        .with_context(|| format!("failed to freeze pack '{}@{}'", pack.r#ref, pack.version))?;
    let (archive_size, archive_digest) = hash_file(&published.archive_path).await?;
    if hex::encode(archive_digest) != published.digest {
        return Err(anyhow!("published archive changed before upload"));
    }

    let object_key = ObjectKey::new(format!("packs/blobs/sha256/{}.tar.gz", published.digest))?;
    ObjectMaintenanceRepository::reserve_upload(pool, object_key.as_str(), "pack").await?;
    let stored = match blob_store
        .put(
            &object_key,
            body_from_file(&published.archive_path).await?,
            archive_digest,
        )
        .await
    {
        Ok(stored) => stored,
        Err(BlobStoreError::Conflict) => blob_store
            .head(&object_key)
            .await?
            .ok_or_else(|| anyhow!("pack release object write conflicted but no object exists"))?,
        Err(error) => return Err(error.into()),
    };
    if stored.size != archive_size || stored.sha256 != archive_digest {
        return Err(anyhow!("pack release object contains different bytes"));
    }
    let archive_size = i64::try_from(stored.size).context("pack release archive is too large")?;
    ObjectMaintenanceRepository::record_uploaded(
        pool,
        object_key.as_str(),
        stored.provider_version.as_stored(),
        archive_size,
    )
    .await?;

    let mut tx = pool.begin().await?;
    PackRepository::acquire_mutation_lock(&mut tx, &pack.r#ref).await?;
    let current = PackRepository::find_by_id(&mut *tx, pack.id)
        .await?
        .ok_or_else(|| anyhow!("pack '{}' was deleted during upgrade", pack.r#ref))?;
    if current.version != pack.version || current.storage_path != pack.storage_path {
        return Err(anyhow!("pack '{}' changed during upgrade", pack.r#ref));
    }
    if PackReleaseRepository::find_active_by_pack_ref(&mut *tx, &pack.r#ref)
        .await?
        .is_some_and(|release| release.version == pack.version)
    {
        tx.commit().await?;
        return Ok(());
    }

    let release = PackReleaseRepository::create_or_get(
        &mut tx,
        CreatePackReleaseInput {
            pack: pack.id,
            pack_ref: pack.r#ref,
            version: pack.version,
            digest: published.digest,
            object_key: object_key.as_str().to_string(),
            provider_version: stored.provider_version.as_stored().to_string(),
            content_path: published.pack_path.to_string_lossy().into_owned(),
            archive_size,
            manifest: serde_json::to_value(published.manifest)?,
        },
    )
    .await?;
    let projections = ComponentLifecycleRepository::active_projection_ids(&mut tx, pack.id).await?;
    PackReleaseRepository::activate_projected(&mut tx, pack.id, release.id, &projections).await?;
    tx.commit().await?;
    Ok(())
}
