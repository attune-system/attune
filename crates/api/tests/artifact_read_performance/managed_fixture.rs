//! Workload fixtures use the same admission boundary as a pack deployment.
use super::*;
use attune_common::{
    delegation::DelegationAuthority,
    pack_registry::{deployment_admission::DeploymentAdmission, PackComponentLoader},
    repositories::{
        pack_release::{CreatePackReleaseInput, PackReleaseRepository},
        FindByRef,
    },
};

pub(super) async fn deploy(
    ctx: &TestContext,
    pack_ref: &str,
    files: &[(&str, String)],
) -> Result<()> {
    let source = ctx.test_packs_dir.join(pack_ref);
    std::fs::create_dir_all(&source)?;
    std::fs::write(
        source.join("pack.yaml"),
        format!("ref: {pack_ref}\nversion: '1.0.0'\n"),
    )?;
    for (path, content) in files {
        let path = source.join(path);
        std::fs::create_dir_all(path.parent().ok_or("fixture path has no parent")?)?;
        std::fs::write(&path, content)?;
        #[cfg(unix)]
        if content.starts_with("#!") {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))?;
        }
    }
    let admission = DeploymentAdmission::read(&source, pack_ref)?;
    let pack = helpers::create_test_pack(&ctx.pool, pack_ref).await?;
    let mut tx = ctx.pool.begin().await?;
    DeploymentAdmission::lock(&mut tx).await?;
    DeploymentAdmission::preserve(
        &mut tx,
        pack.id,
        ctx.state.config.security.encryption_key.as_deref(),
    )
    .await?;
    let installer = DelegationAuthority::load_for_share(
        &mut tx,
        ctx.user.as_ref().ok_or("installer missing")?.id,
    )
    .await?;
    let release = PackReleaseRepository::create_or_get(
        &mut tx,
        CreatePackReleaseInput {
            pack: pack.id,
            pack_ref: pack_ref.into(),
            version: pack.version,
            digest: {
                use sha2::{Digest, Sha256};
                Sha256::digest(serde_jcs::to_vec(&json!(files))?)
                    .iter()
                    .map(|byte| format!("{byte:02x}"))
                    .collect()
            },
            object_key: format!("packs/blobs/{pack_ref}.tar.gz"),
            provider_version: "e:workload-fixture".into(),
            content_path: source.to_string_lossy().into_owned(),
            archive_size: 1,
            manifest: json!({}),
        },
    )
    .await?;
    let loader = PackComponentLoader::new(
        &ctx.pool,
        pack.id,
        pack_ref,
        &ctx.state.config.cache_admission,
        installer.clone(),
    );
    let loaded = admission.load(loader, &mut tx).await?;
    assert_eq!(loaded.total_skipped(), 0, "{:?}", loaded.warnings);
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
    tx.commit().await?;
    Ok(())
}

pub(super) async fn action_pack(ctx: &TestContext) -> Result<String> {
    deploy(ctx, "artifact_perf", &[
        ("runtimes/native.yaml", "ref: artifact_perf.native\nname: native\nexecution_config: {}\n".into()),
        ("actions/run.yaml", "ref: artifact_perf.run\nrunner_type: native\nentry_point: run.sh\n".into()),
        ("actions/run.sh", "#!/bin/sh\nprintf '{}\\n'\n".into()),
        ("sensors/sensor.yaml", "ref: artifact_perf.sensor\nrunner_type: native\nentry_point: sensor\ntrigger_types: [artifact_perf.sensor_event]\n".into()),
        ("sensors/sensor", "#!/bin/sh\nexit 0\n".into()),
        ("triggers/sensor_event.yaml", "ref: artifact_perf.sensor_event\nreference_visibility: public\n".into()),
    ]).await?;
    let action = attune_common::repositories::action::ActionRepository::find_by_ref(
        &ctx.pool,
        "artifact_perf.run",
    )
    .await?
    .ok_or("admitted action missing")?;
    Ok(action.r#ref)
}

/// Retained rows model independent historical generations, but keep the original
/// metadata pin produced by deployment. Never synthesize a replacement revision.
pub(super) async fn retained_generation(
    connection: &mut sqlx::PgConnection,
    sequence: i64,
    binding: &attune_common::sensor_schema::SensorGenerationBinding,
    initial_rules: Value,
) -> Result<i64> {
    binding.validate()?;
    let revision = binding
        .metadata_revision
        .ok_or("admitted metadata pin missing")?;
    // These are retained, unassigned workloads, not live worker leases. Keep
    // their real sensor/code ownership so the generation-pin database guard runs.
    let workload: i64 = sqlx::query_scalar("INSERT INTO sensor_workload(sensor,workload_key,pack_release,pack_release_digest,executable_snapshot) VALUES ($1,$2,$3,$4,$5) RETURNING id")
        .bind(binding.executable.sensor.id).bind(format!("retained-{sequence}"))
        .bind(binding.executable.release.id).bind(&binding.executable.release.digest)
        .bind(serde_json::to_value(&binding.executable)?).fetch_one(&mut *connection).await?;
    sqlx::query("INSERT INTO sensor_workload_assignment(workload,generation) VALUES ($1,$2)")
        .bind(workload)
        .bind(sequence)
        .execute(&mut *connection)
        .await?;
    sqlx::query(
        "INSERT INTO managed_sensor_generation(workload,generation,revision) VALUES ($1,$2,$3)",
    )
    .bind(workload)
    .bind(sequence)
    .bind(revision)
    .execute(&mut *connection)
    .await?;
    Ok(sqlx::query_scalar("INSERT INTO sensor_generation_schema_binding(workload,generation,binding,binding_digest,config,initial_rules) VALUES ($1,$2,$3,$4,'{}',$5) RETURNING id")
        .bind(workload).bind(sequence).bind(serde_json::to_value(binding)?).bind(binding.binding_digest()?).bind(initial_rules).fetch_one(connection).await?)
}
