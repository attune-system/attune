//! Owning worker acceptance through the concrete Docker adapter. No default
//! daemon/context or alternate execution path is used. CI supplies the same
//! once-built Echo/Runner used by its backend effects fixture.
#![cfg(unix)]
#[path = "support/owning_worker.rs"]
mod owning_worker;

use anyhow::{ensure, Context, Result};
use attune_common::{
    workload_config::{WorkloadBackendConfig, WorkloadConfig, WorkloadStorageLimits},
    workload_launch::{
        LaunchInputKind, LaunchPlatform, PinnedImage, PinnedLaunchInput, Sha256Digest,
    },
};
use attune_workload_backend::{configuration, engine::DockerEngine};
use owning_worker::{
    run_native_fixture, run_native_parameter_fixture, NativeOwningFixture, OwningCase,
};
use std::{os::unix::fs::PermissionsExt, path::PathBuf};

fn required(name: &str) -> Result<String> {
    std::env::var(name)
        .with_context(|| format!("missing owned Docker fixture prerequisite: {name}"))
}
fn image(name: &str) -> Result<PinnedImage> {
    let reference = required(name)?;
    let digest = reference
        .strip_prefix("sha256:")
        .or_else(|| reference.split_once("@sha256:").map(|(_, digest)| digest))
        .context("immutable candidate image required")?;
    let sha256 = Sha256Digest::parse(digest)?;
    Ok(PinnedImage { reference, sha256 })
}
async fn fixture() -> Result<NativeOwningFixture> {
    let root = tempfile::Builder::new()
        .prefix("attune-owning-docker-")
        .tempdir()?;
    let socket = PathBuf::from(required("ATTUNE_ISOLATION_TEST_DOCKER_SOCKET")?);
    let cache_root = PathBuf::from(required("ATTUNE_ISOLATION_TEST_CACHE_ROOT")?);
    ensure!(
        socket.is_absolute() && cache_root.is_absolute(),
        "explicit absolute owned paths required"
    );
    let platform = match required("ATTUNE_ISOLATION_TEST_PLATFORM")?.as_str() {
        "linux/amd64" => LaunchPlatform::LinuxAmd64,
        "linux/arm64" => LaunchPlatform::LinuxArm64,
        _ => anyhow::bail!("unsupported owned Docker platform"),
    };
    let echo = PathBuf::from(required("ATTUNE_ISOLATION_TEST_ECHO_PATH")?);
    let runner = PathBuf::from(required("ATTUNE_ISOLATION_TEST_RUNNER_PATH")?);
    ensure!(
        echo.is_absolute() && runner.is_absolute(),
        "once-built absolute binary paths required"
    );
    let archive = root.path().join("runner.zip");
    owning_worker::runner_zip(&archive, &runner, platform)?;
    std::fs::set_permissions(&archive, std::fs::Permissions::from_mode(0o444))?;
    let bytes = std::fs::read(&archive)?;
    let helper = PinnedLaunchInput {
        pin_id: 1,
        kind: LaunchInputKind::Runner,
        object_id: 2,
        object_version: format!(
            "owned-docker-runner-{}",
            Sha256Digest::of_bytes(&bytes).as_str()
        ),
        size_bytes: bytes.len() as u64,
        sha256: Sha256Digest::of_bytes(&bytes),
        max_expanded_bytes: 134_217_728,
        max_files: 8,
    };
    // This is an explicitly owned test daemon. Production operator bindings are
    // persisted admission inputs; neither path silently rebinds retained inputs.
    let engine = DockerEngine::connect(&socket)?;
    let binding = engine.observed_binding(1).await?;
    ensure!(
        binding.platform == platform,
        "owned daemon platform mismatch"
    );
    let payload_image = image("ATTUNE_ISOLATION_TEST_PAYLOAD_IMAGE")?;
    let config = WorkloadConfig {
        image: payload_image.clone(),
        platform,
        cache_root,
        memory_bytes: 536_870_912,
        scratch_bytes: 134_217_728,
        storage: WorkloadStorageLimits {
            compressed_bytes: 67_108_864,
            expanded_bytes: 134_217_728,
            entries: 4096,
            artifacts: Default::default(),
        },
        helper,
        helper_archive: archive,
        backend: WorkloadBackendConfig::Docker {
            socket,
            cache_volume: required("ATTUNE_ISOLATION_TEST_CACHE_VOLUME")?,
            network: required("ATTUNE_ISOLATION_TEST_NETWORK")?,
            binding,
            pids_limit: 128,
            helper_image: payload_image,
        },
    };
    let configured = configuration::configure(&config).await?;
    Ok(NativeOwningFixture {
        root,
        config,
        configured,
        echo,
        runner,
    })
}

#[tokio::test]
#[ignore = "requires explicitly owned Docker manager volume/socket/network, PostgreSQL and once-built candidate binaries"]
async fn docker_native_owning_worker() -> Result<()> {
    run_native_fixture(fixture().await?, OwningCase::Native).await
}
#[tokio::test]
#[ignore = "requires explicitly owned Docker manager volume/socket/network, PostgreSQL and once-built candidate binaries"]
async fn docker_native_cli_owning_worker() -> Result<()> {
    run_native_fixture(fixture().await?, OwningCase::NativeCli).await
}
#[tokio::test]
#[ignore = "requires explicitly owned Docker manager volume/socket/network, PostgreSQL and once-built candidate binaries"]
async fn docker_command_owning_worker() -> Result<()> {
    run_native_fixture(fixture().await?, OwningCase::Command).await
}
#[tokio::test]
#[ignore = "requires explicitly owned Docker manager volume/socket/network, PostgreSQL and once-built candidate binaries"]
async fn docker_interpreted_owning_worker() -> Result<()> {
    run_native_fixture(fixture().await?, OwningCase::Interpreted).await?;
    for (delivery, format) in [
        ("stdin", "yaml"),
        ("stdin", "dotenv"),
        ("file", "json"),
        ("file", "yaml"),
        ("file", "dotenv"),
        ("none", "json"),
    ] {
        run_native_parameter_fixture(fixture().await?, delivery, format).await?;
    }
    Ok(())
}
