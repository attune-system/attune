use attune_common::{
    delegation::DelegationAuthority,
    models::Pack,
    pack_registry::{deployment_admission::DeploymentAdmission, PackComponentLoader},
    repositories::{
        pack::{PackRepository, UpdatePackInput},
        pack_release::{CreatePackReleaseInput, PackReleaseRepository},
        runtime_authority::{RuntimeAuthorityRepository, RuntimePolicyRevision},
        sensor_admission::{SensorAdmissionRepository, SensorAdmissionRequirement},
    },
};

use crate::helpers::{Result, TestContext};

fn source_digest(source: &std::path::Path) -> Result<String> {
    use sha2::Digest;
    let mut pending = vec![source.to_path_buf()];
    let mut files = Vec::new();
    while let Some(directory) = pending.pop() {
        for entry in std::fs::read_dir(directory)? {
            let entry = entry?;
            if entry.file_type()?.is_dir() {
                pending.push(entry.path());
            } else {
                files.push((
                    entry
                        .path()
                        .strip_prefix(source)?
                        .to_string_lossy()
                        .into_owned(),
                    std::fs::read(entry.path())?,
                ));
            }
        }
    }
    files.sort_by(|left, right| left.0.cmp(&right.0));
    Ok(sha2::Sha256::digest(serde_jcs::to_vec(&files)?)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

/// Use the production admission lifecycle, not a projection-only release or a
/// fabricated runtime identity. The caller owns these source files through cleanup.
pub async fn deploy(
    ctx: &TestContext,
    pack: &Pack,
    source: &std::path::Path,
    installer_id: i64,
) -> Result<RuntimePolicyRevision> {
    let admission = DeploymentAdmission::read(source, &pack.r#ref)?;
    let code_digest = source_digest(source)?;
    let mut tx = ctx.pool.begin().await?;
    DeploymentAdmission::lock(&mut tx).await?;
    SensorAdmissionRepository::lock_mutations(&mut tx).await?;
    DeploymentAdmission::preserve(
        &mut tx,
        pack.id,
        ctx.state.config.security.encryption_key.as_deref(),
    )
    .await?;
    PackRepository::update(
        &mut *tx,
        pack.id,
        UpdatePackInput {
            version: Some(pack.version.clone()),
            ..Default::default()
        },
    )
    .await?;
    let installer = DelegationAuthority::load_for_share(&mut tx, installer_id).await?;
    let release = PackReleaseRepository::create_or_get(
        &mut tx,
        CreatePackReleaseInput {
            pack: pack.id,
            pack_ref: pack.r#ref.clone(),
            version: pack.version.clone(),
            digest: code_digest.clone(),
            object_key: format!("packs/blobs/{code_digest}.tar.gz"),
            provider_version: format!("e:{code_digest}"),
            content_path: source.to_string_lossy().into_owned(),
            archive_size: 1,
            manifest: serde_json::json!({}),
        },
    )
    .await?;
    let loader = PackComponentLoader::new(
        &ctx.pool,
        pack.id,
        &pack.r#ref,
        &ctx.state.config.cache_admission,
        installer.clone(),
    );
    let loaded = admission.load(loader, &mut tx).await?;
    assert_eq!(loaded.total_skipped(), 0, "{:?}", loaded.warnings);
    let failures = SensorAdmissionRepository::assess_pack(
        &mut tx,
        pack.id,
        SensorAdmissionRequirement::Structural,
    )
    .await?;
    assert!(failures.is_empty(), "{failures:?}");
    PackReleaseRepository::activate_projected(&mut tx, pack.id, release.id, &loaded.projections)
        .await?;
    admission
        .finish(
            &mut tx,
            pack.id,
            release.id,
            ctx.state.config.security.encryption_key.as_deref(),
            &installer,
        )
        .await?;
    let policy = RuntimeAuthorityRepository::current_policy(&mut tx, pack.id)
        .await?
        .ok_or("deployment did not activate a policy")?;
    assert_eq!(policy.policy.owner.pack_id, pack.id);
    assert_eq!(policy.policy.owner.pack_ref, pack.r#ref);
    assert_eq!(policy.policy.release_id, release.id);
    assert_ne!(policy.policy.owner.sponsor_identity_id, installer_id);
    tx.commit().await?;
    Ok(policy)
}
