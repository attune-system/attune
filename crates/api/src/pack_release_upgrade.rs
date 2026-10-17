//! Detect legacy content without silently turning mutable metadata into authority.

use anyhow::Result;
use attune_common::{blob_store::BlobStore, repositories::pack::PackRepository};
use sqlx::PgPool;
use std::path::Path;

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

/// Startup remains available for controlled API bootstrap/readmission. This legacy
/// entry point cannot activate projections, freeze unadmitted metadata, or replace
/// an existing admitted release. API upload/register owns the admission transaction.
pub async fn upgrade_legacy_pack_releases(
    pool: &PgPool,
    _blob_store: &dyn BlobStore,
    _packs_base_dir: &Path,
) -> Result<PackReleaseUpgradeReport> {
    let mut scan = pool.begin().await?;
    let packs = PackRepository::list_requiring_release(&mut *scan).await?;
    scan.commit().await?;
    Ok(PackReleaseUpgradeReport {
        upgraded: vec![],
        failures: packs.into_iter().map(|pack| PackReleaseUpgradeFailure {
            pack_ref: pack.r#ref,
            error: "Explicit API upload/register readmission is required; direct release freezing cannot establish runtime authority".into(),
        }).collect(),
    })
}
