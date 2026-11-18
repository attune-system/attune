//! Concrete manager-side construction. No caller launch plan, enrollment,
//! controller session or implicit developer Kubernetes context is accepted.
use crate::{
    cache::{CacheLimits, CacheStore},
    engine::{DockerEngine, DockerLauncher, EnginePolicy},
    kubernetes::{KubernetesLauncher, KubernetesPolicy},
    process_policy::invalid,
};
use attune_common::{
    isolated_process::{ProcessLauncher, ProcessSpec, ResourceIdentity},
    workload_config::{WorkloadBackendConfig, WorkloadConfig},
    workload_launch::{LaunchInputKind, LaunchPlatform, PinnedLaunchInput},
    Error, Result,
};
use std::{
    collections::BTreeSet,
    fs::OpenOptions,
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::Path,
    sync::Arc,
};

/// A closed pair of concrete adapters, not a provider/plugin registry. The
/// owning service retains this value for factual transfer/removal effects.
#[derive(Clone)]
pub enum NativeBackend {
    Kubernetes(Arc<KubernetesLauncher>),
    Docker(Arc<DockerLauncher>),
}
impl NativeBackend {
    pub fn launcher(&self) -> Arc<dyn ProcessLauncher> {
        match self {
            Self::Kubernetes(v) => v.clone(),
            Self::Docker(v) => v.clone(),
        }
    }
    pub async fn remove_stopped(
        &self,
        spec: &ProcessSpec,
        resource: &ResourceIdentity,
    ) -> Result<()> {
        match self {
            Self::Kubernetes(v) => v.remove_stopped(spec, resource).await,
            Self::Docker(v) => v.remove_stopped(spec, resource).await,
        }
    }
    pub async fn read_stopped_artifact(
        &self,
        spec: &ProcessSpec,
        resource: &ResourceIdentity,
        relative: &attune_common::artifact_transport::ValidatedRelativePath,
        remaining_bytes: u64,
    ) -> Result<Option<std::fs::File>> {
        match self {
            Self::Kubernetes(v) => {
                v.read_stopped_artifact(spec, resource, relative, remaining_bytes)
                    .await
            }
            Self::Docker(v) => {
                v.read_stopped_artifact(spec, resource, relative, remaining_bytes)
                    .await
            }
        }
    }
    pub async fn archive_stopped_stage(
        &self,
        spec: &ProcessSpec,
        resource: &ResourceIdentity,
    ) -> Result<std::fs::File> {
        match self {
            Self::Kubernetes(v) => v.archive_stopped_stage(spec, resource).await,
            Self::Docker(v) => v.archive_stopped_stage(spec, resource).await,
        }
    }
}
pub struct ConfiguredBackend {
    pub backend: NativeBackend,
    pub cache: Arc<CacheStore>,
    /// Exact public pin already installed for the trusted producer's Runner input.
    pub runner: PinnedLaunchInput,
}
fn install_runner(
    store: &CacheStore,
    path: &Path,
    pin: &PinnedLaunchInput,
    platform: LaunchPlatform,
) -> Result<()> {
    if pin.kind != LaunchInputKind::Runner {
        return Err(invalid());
    }
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
        .map_err(|_| invalid())?;
    let metadata = file.metadata().map_err(|_| invalid())?;
    if !metadata.is_file()
        || metadata.nlink() != 1
        || metadata.mode() & 0o022 != 0
        || !(metadata.uid() == 0 || metadata.uid() == unsafe { libc::geteuid() })
    {
        return Err(invalid());
    }
    let body = store.install_input(pin, &mut file).map_err(|_| invalid())?;
    let selected = body.join(format!("linux-{}", platform.native_arch()));
    let metadata = std::fs::symlink_metadata(selected).map_err(|_| invalid())?;
    if !metadata.is_file() || metadata.nlink() != 1 || metadata.mode() & 0o111 == 0 {
        return Err(invalid());
    }
    Ok(())
}
pub async fn configure(config: &WorkloadConfig) -> Result<ConfiguredBackend> {
    config.validate()?;
    let root = config.cache_root.clone();
    let limits = config.storage;
    let cache = tokio::task::spawn_blocking(move || {
        CacheStore::open(
            root,
            CacheLimits {
                compressed_bytes: limits.compressed_bytes,
                expanded_bytes: limits.expanded_bytes,
                files: limits.entries,
            },
        )?
        .with_artifact_limits(
            limits.artifacts.max_total_bytes,
            limits.artifacts.max_entries,
        )
    })
    .await
    .map_err(|_| {
        Error::validation("Native workload private cache initialization did not complete")
    })?
    .map_err(|_| {
        Error::validation("Native workload private cache ownership or limits are invalid")
    })?;
    let cache = Arc::new(cache);
    let selected = config.helper.clone();
    let path = config.helper_archive.clone();
    let platform = config.platform;
    let store = cache.clone();
    tokio::task::spawn_blocking(move || install_runner(&store, &path, &selected, platform))
        .await.map_err(|_| Error::validation("Public Runner archive installation did not complete"))?
        .map_err(|_| Error::validation("Public Runner archive failed ownership, immutable ZIP pin or platform entry validation"))?;
    let backend = match &config.backend {
        WorkloadBackendConfig::Kubernetes {
            cluster,
            namespace,
            helper_image,
        } => {
            // Deliberately do not use Config::infer/Client::try_default. Only
            // this trusted manager's projected in-cluster identity is read.
            let cluster_config = kube::Config::incluster_env().map_err(|_| Error::validation(
                "Kubernetes workload backend requires the manager's in-cluster service account and TLS configuration"))?;
            let client = kube::Client::try_from(cluster_config).map_err(|_| {
                Error::validation("Kubernetes workload client TLS initialization failed")
            })?;
            NativeBackend::Kubernetes(Arc::new(KubernetesLauncher::new(
                client,
                KubernetesPolicy {
                    cluster: cluster.clone(),
                    namespace: namespace.clone(),
                    helper_image: helper_image.clone(),
                    allowed_images: BTreeSet::from([config.image.reference.clone()]),
                    memory_bytes: config.memory_bytes,
                    scratch_bytes: config.scratch_bytes,
                },
                cache.clone(),
            )))
        }
        WorkloadBackendConfig::Docker {
            socket,
            cache_volume,
            network,
            binding,
            pids_limit,
            helper_image,
        } => {
            let policy = EnginePolicy {
                engine_id: binding.engine_id,
                cache_volume: cache_volume.clone(),
                network: network.clone(),
                allowed_images: BTreeSet::from([config.image.reference.clone()]),
                memory_bytes: config.memory_bytes,
                pids_limit: *pids_limit,
                scratch_bytes: config.scratch_bytes,
            };
            let engine = DockerEngine::connect(socket).map_err(|_| {
                Error::validation("Docker workload backend requires a valid configured Unix socket")
            })?;
            engine
                .verify_engine_binding(binding, &policy)
                .await
                .map_err(|_| {
                    Error::validation("Docker daemon/platform binding verification failed")
                })?;
            engine
                .image_environment_for_binding(binding, &policy, &config.image)
                .await
                .map_err(|_| {
                    Error::validation("Docker admitted runtime image/platform preparation failed")
                })?;
            let mut probe_policy = policy.clone();
            probe_policy
                .allowed_images
                .insert(helper_image.reference.clone());
            engine
                .image_environment_for_binding(binding, &probe_policy, helper_image)
                .await
                .map_err(|_| {
                    Error::validation("Docker public helper image/platform preparation failed")
                })?;
            engine
                .verify_cache_binding(&probe_policy, &cache, &helper_image.reference)
                .await
                .map_err(|_| {
                    Error::validation(
                        "Docker fixed named cache volume binding probe did not complete",
                    )
                })?;
            NativeBackend::Docker(Arc::new(
                DockerLauncher::new(
                    engine,
                    policy,
                    binding.clone(),
                    cache.clone(),
                    config.helper.clone(),
                )
                .map_err(|_| invalid())?,
            ))
        }
    };
    Ok(ConfiguredBackend {
        backend,
        cache,
        runner: config.helper.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use attune_common::workload_launch::Sha256Digest;
    use std::{io::Write, os::unix::fs::PermissionsExt};
    fn archive(
        root: &Path,
        name: &str,
        mode: u32,
        link: bool,
    ) -> (std::path::PathBuf, PinnedLaunchInput) {
        let path = root.join("runner.zip");
        let mut writer = zip::ZipWriter::new(std::fs::File::create(&path).unwrap());
        let options = zip::write::SimpleFileOptions::default().unix_permissions(mode);
        let (expanded, entries) = if link {
            writer.start_file("real-runner", options).unwrap();
            writer.write_all(b"public-helper").unwrap();
            writer.add_symlink(name, "real-runner", options).unwrap();
            (24, 2)
        } else {
            writer.start_file(name, options).unwrap();
            writer.write_all(b"public-helper").unwrap();
            (13, 1)
        };
        writer.finish().unwrap().sync_all().unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        (
            path,
            PinnedLaunchInput {
                pin_id: 1,
                kind: LaunchInputKind::Runner,
                object_id: 2,
                object_version: "public-package-fixture".into(),
                size_bytes: bytes.len() as u64,
                sha256: Sha256Digest::of_bytes(&bytes),
                max_expanded_bytes: expanded,
                max_files: entries,
            },
        )
    }
    fn cache(root: &Path) -> CacheStore {
        CacheStore::open(
            root.join("cache"),
            CacheLimits {
                compressed_bytes: 65536,
                expanded_bytes: 65536,
                files: 16,
            },
        )
        .unwrap()
    }
    #[test]
    fn shared_runner_is_installed_from_exact_public_archive_for_selected_platform() {
        for platform in [LaunchPlatform::LinuxAmd64, LaunchPlatform::LinuxArm64] {
            let root = tempfile::tempdir().unwrap();
            let name = format!("linux-{}", platform.native_arch());
            let (path, pin) = archive(root.path(), &name, 0o755, false);
            let store = cache(root.path());
            install_runner(&store, &path, &pin, platform).unwrap();
            let mut retained = store.open_input_archive(&pin).unwrap();
            use std::io::Read;
            let mut bytes = Vec::new();
            retained.read_to_end(&mut bytes).unwrap();
            assert_eq!(Sha256Digest::of_bytes(&bytes), pin.sha256);
            // Same pin is exposed to the owning producer, with no image lookup
            // or synthetic object/version change in the installation path.
            install_runner(&store, &path, &pin, platform).unwrap();
        }
    }
    #[test]
    fn public_runner_rejects_wrong_platform_modes_links_and_archive_aliases() {
        for case in 0..6 {
            let root = tempfile::tempdir().unwrap();
            let (mut path, mut pin) = archive(
                root.path(),
                if case == 0 {
                    "linux-arm64"
                } else {
                    "linux-amd64"
                },
                if case == 1 { 0o644 } else { 0o755 },
                case == 2,
            );
            match case {
                3 => pin.sha256 = Sha256Digest::of_bytes(b"wrong-public-pin"),
                4 => {
                    let alias = root.path().join("alias.zip");
                    std::os::unix::fs::symlink(&path, &alias).unwrap();
                    path = alias;
                }
                5 => std::fs::hard_link(&path, root.path().join("other.zip")).unwrap(),
                _ => {}
            }
            assert!(
                install_runner(&cache(root.path()), &path, &pin, LaunchPlatform::LinuxAmd64)
                    .is_err(),
                "case {case}"
            );
        }
    }
}
