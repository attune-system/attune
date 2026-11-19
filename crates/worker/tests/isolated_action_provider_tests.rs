//! Owning Kubernetes wrappers; the real domain/custody assertions are shared
//! with Docker without injecting a replacement process/preparer path.
#![cfg(unix)]
#[path = "support/owning_worker.rs"]
mod owning_worker;
use anyhow::{ensure, Context, Result};
use attune_common::{
    workload_config::{WorkloadBackendConfig, WorkloadConfig, WorkloadStorageLimits},
    workload_launch::*,
};
use attune_workload_backend::{
    cache::{CacheLimits, CacheStore},
    configuration::{ConfiguredBackend, NativeBackend},
    kubernetes::{KubernetesLauncher, KubernetesPolicy},
};
use owning_worker::{run_native_fixture, NativeOwningFixture, OwningCase};
use std::{collections::BTreeSet, fs::File, path::PathBuf, sync::Arc};
fn required(name: &str) -> Result<String> {
    std::env::var(name).with_context(|| format!("missing owned fixture prerequisite: {name}"))
}
fn image(name: &str) -> Result<PinnedImage> {
    let reference = required(name)?;
    let (_, digest) = reference
        .split_once("@sha256:")
        .context("immutable image required")?;
    let sha256 = Sha256Digest::parse(digest)?;
    Ok(PinnedImage { reference, sha256 })
}
async fn kubernetes_fixture() -> Result<NativeOwningFixture> {
    let root = tempfile::tempdir()?;
    let platform = match required("ATTUNE_ISOLATION_TEST_PLATFORM")?.as_str() {
        "linux/amd64" => LaunchPlatform::LinuxAmd64,
        "linux/arm64" => LaunchPlatform::LinuxArm64,
        _ => anyhow::bail!("unsupported admitted platform"),
    };
    let echo = PathBuf::from(required("ATTUNE_ISOLATION_TEST_ECHO_PATH")?);
    let runner = PathBuf::from(required("ATTUNE_ISOLATION_TEST_RUNNER_PATH")?);
    ensure!(
        echo.is_absolute() && runner.is_absolute(),
        "owned binaries must be absolute"
    );
    let image = image("ATTUNE_ISOLATION_TEST_PAYLOAD_IMAGE")?;
    let helper_image = self::image("ATTUNE_ISOLATION_TEST_HELPER_IMAGE")?;
    let cluster = required("ATTUNE_ISOLATION_TEST_CLUSTER")?;
    let namespace = required("ATTUNE_ISOLATION_TEST_NAMESPACE")?;
    let cache = Arc::new(CacheStore::open(
        root.path().join("cache"),
        CacheLimits {
            compressed_bytes: 64 << 20,
            expanded_bytes: 128 << 20,
            files: 4096,
        },
    )?);
    let helper_archive = root.path().join("runner.zip");
    owning_worker::runner_zip(&helper_archive, &runner, platform)?;
    let helper = owning_worker::pin(
        LaunchInputKind::Runner,
        1,
        "owned-once-built-helper".into(),
        &helper_archive,
        64 << 20,
        16,
    )?;
    cache.install_input(&helper, &mut File::open(&helper_archive)?)?;
    let launcher = Arc::new(KubernetesLauncher::new(
        owning_worker::owned_kubernetes_client().await?,
        KubernetesPolicy {
            cluster: cluster.clone(),
            namespace: namespace.clone(),
            helper_image: helper_image.clone(),
            allowed_images: BTreeSet::from([image.reference.clone()]),
            memory_bytes: 128 << 20,
            scratch_bytes: 64 << 20,
        },
        cache.clone(),
    ));
    let config = WorkloadConfig {
        image,
        platform,
        cache_root: root.path().join("cache"),
        memory_bytes: 128 << 20,
        scratch_bytes: 64 << 20,
        storage: WorkloadStorageLimits {
            compressed_bytes: 64 << 20,
            expanded_bytes: 128 << 20,
            entries: 4096,
            artifacts: Default::default(),
        },
        helper: helper.clone(),
        helper_archive,
        backend: WorkloadBackendConfig::Kubernetes {
            cluster,
            namespace,
            helper_image,
        },
    };
    Ok(NativeOwningFixture {
        root,
        config,
        configured: ConfiguredBackend {
            backend: NativeBackend::Kubernetes(launcher),
            cache,
            runner: helper,
        },
        echo,
        runner,
    })
}
#[tokio::test]
#[ignore = "requires explicit owned Kubernetes namespace, pinned binaries/images and disposable PostgreSQL"]
async fn kubernetes_native_action_custody_completion_and_restart() -> Result<()> {
    let _provider = KUBERNETES_CASE.lock().await;
    run_native_fixture(kubernetes_fixture().await?, OwningCase::Native).await
}

// No provider effect is claimed: this catches actual preparer/custody/completion
// regressions on PostgreSQL before the owning namespace job executes the Pod.
#[tokio::test]
async fn trusted_worker_completion_and_custody_repository_case() -> Result<()> {
    owning_worker::run_repository_fixture().await
}

// Each real case requires the owned node's fixed payload CPU budget. Retain
// four database-test threads while serializing only these shared-provider cases.
static KUBERNETES_CASE: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

#[tokio::test]
#[ignore = "requires explicit owned Kubernetes namespace, pinned binaries/images and disposable PostgreSQL"]
async fn kubernetes_native_cli_action_custody_completion_and_restart() -> Result<()> {
    let _provider = KUBERNETES_CASE.lock().await;
    run_native_fixture(kubernetes_fixture().await?, OwningCase::NativeCli).await
}

#[tokio::test]
#[ignore = "requires explicit owned Kubernetes namespace, pinned binaries/images and disposable PostgreSQL"]
async fn kubernetes_command_action_custody_completion_and_restart() -> Result<()> {
    let _provider = KUBERNETES_CASE.lock().await;
    run_native_fixture(kubernetes_fixture().await?, OwningCase::Command).await
}

#[tokio::test]
#[ignore = "requires explicit owned Kubernetes namespace, pinned binaries/images and disposable PostgreSQL"]
async fn kubernetes_interpreted_setup_custody_completion_and_restart() -> Result<()> {
    let _provider = KUBERNETES_CASE.lock().await;
    run_native_fixture(kubernetes_fixture().await?, OwningCase::Interpreted).await
}
