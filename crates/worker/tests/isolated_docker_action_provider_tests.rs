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
use owning_worker::{run_interpreted_suite, run_native_fixture, NativeOwningFixture, OwningCase};
use std::{
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
};

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
fn runner_input(
    archive: &Path,
    runner: &Path,
    platform: LaunchPlatform,
) -> Result<PinnedLaunchInput> {
    let metadata = std::fs::metadata(runner)?;
    ensure!(
        metadata.is_file() && metadata.len() > 0 && metadata.len() <= 64 * 1024 * 1024,
        "owned public Runner exceeds fixture input bound"
    );
    owning_worker::runner_zip(archive, runner, platform)?;
    std::fs::set_permissions(archive, std::fs::Permissions::from_mode(0o444))?;
    let bytes = std::fs::read(archive)?;
    Ok(PinnedLaunchInput {
        pin_id: 1,
        kind: LaunchInputKind::Runner,
        object_id: 2,
        object_version: format!(
            "owned-docker-runner-{}",
            Sha256Digest::of_bytes(&bytes).as_str()
        ),
        size_bytes: bytes.len() as u64,
        sha256: Sha256Digest::of_bytes(&bytes),
        // The preceding Engine case installs this same ZIP into the same
        // digest-addressed cache. Its receipt binds these exact observed bounds.
        max_expanded_bytes: metadata.len(),
        max_files: 1,
    })
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
    let helper = runner_input(&archive, &runner, platform)?;
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

// ZIP/cache proof only: these public bytes are never launched as a Runner.
#[test]
fn docker_runner_pin_reuses_engine_cache_receipt() -> Result<()> {
    use attune_workload_backend::cache::{CacheError, CacheLimits, CacheStore};
    use std::{fs::File, io::Write};

    for platform in [LaunchPlatform::LinuxAmd64, LaunchPlatform::LinuxArm64] {
        let root = tempfile::tempdir()?;
        let runner = root.path().join("once-built-runner");
        let payload = b"owned public Runner packaging regression";
        std::fs::write(&runner, payload)?;
        let engine_archive = root.path().join("engine-runner.zip");
        // Match engine_tests::real_engine_cache_binding_and_python_publication,
        // including its ZIP defaults and exact expanded-byte/entry ceilings.
        let mut engine_file = File::create_new(&engine_archive)?;
        {
            let mut zip = zip::ZipWriter::new(&mut engine_file);
            zip.start_file(
                format!("linux-{}", platform.native_arch()),
                zip::write::SimpleFileOptions::default().unix_permissions(0o755),
            )?;
            zip.write_all(payload)?;
            zip.finish()?;
        }
        let engine_pin = PinnedLaunchInput {
            pin_id: 20,
            ..owning_worker::pin(
                LaunchInputKind::Runner,
                21,
                "owned-public-helper".into(),
                &engine_archive,
                payload.len() as u64,
                1,
            )?
        };
        let cache = CacheStore::open(
            root.path().join("shared-cache"),
            CacheLimits {
                compressed_bytes: 64 << 20,
                expanded_bytes: 128 << 20,
                files: 4096,
            },
        )?;
        let existing = cache.install_input(&engine_pin, &mut File::open(&engine_archive)?)?;
        let receipt = existing
            .parent()
            .context("missing cache parent")?
            .join("receipt");
        let retained = std::fs::read(&receipt)?;
        let old_worker_pin = PinnedLaunchInput {
            max_expanded_bytes: 128 << 20,
            max_files: 8,
            ..engine_pin.clone()
        };
        ensure!(matches!(
            cache.install_input(&old_worker_pin, &mut File::open(&engine_archive)?),
            Err(CacheError::PinMismatch)
        ));
        ensure!(
            std::fs::read(&receipt)? == retained,
            "mismatched pin changed receipt"
        );

        let archive = root.path().join("worker-runner.zip");
        let helper = runner_input(&archive, &runner, platform)?;
        ensure!(helper.sha256 == engine_pin.sha256, "Runner ZIP changed");
        ensure!(
            helper.size_bytes == engine_pin.size_bytes,
            "Runner ZIP size changed"
        );
        ensure!(helper.max_expanded_bytes == engine_pin.max_expanded_bytes);
        ensure!(helper.max_files == engine_pin.max_files);
        let installed = cache.install_input(&helper, &mut File::open(&archive)?)?;
        ensure!(
            installed == existing,
            "Runner was not reused from the same cache"
        );
        ensure!(
            std::fs::read(&receipt)? == retained,
            "reuse changed receipt"
        );
        let selected = installed.join(format!("linux-{}", platform.native_arch()));
        ensure!(std::fs::read(&selected)? == payload);
        ensure!(std::fs::metadata(selected)?.permissions().mode() & 0o111 != 0);
        cache.open_input_archive(&helper)?;
    }
    Ok(())
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
    run_interpreted_suite(fixture().await?).await
}
