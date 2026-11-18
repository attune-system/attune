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
    Error, Result,
};
use std::{
    collections::BTreeSet,
    fs::OpenOptions,
    os::unix::fs::{MetadataExt, OpenOptionsExt},
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
            helper,
            helper_archive,
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
            // The public archive is operator-selected, immutable-pin verified,
            // never loaded from a pack/caller path or treated as private context.
            let selected = helper.clone();
            let path = helper_archive.clone();
            let store = cache.clone();
            tokio::task::spawn_blocking(move || {
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
                store
                    .install_input(&selected, &mut file)
                    .map_err(|_| invalid())?;
                Ok::<_, attune_common::Error>(())
            })
            .await
            .map_err(|_| {
                Error::validation("Docker public Runner archive installation did not complete")
            })?
            .map_err(|_| {
                Error::validation(
                    "Docker public Runner archive failed ownership or immutable ZIP pin validation",
                )
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
                    helper.clone(),
                )
                .map_err(|_| invalid())?,
            ))
        }
    };
    Ok(ConfiguredBackend { backend, cache })
}
