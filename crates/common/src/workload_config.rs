//! Operator configuration for the two concrete in-process execution adapters.
//! These are manager settings, never pack metadata or a remote launch request.
use crate::{
    workload_artifact::ArtifactLimits,
    workload_launch::{
        LaunchEngineBinding, LaunchInputKind, LaunchPlatform, PinnedImage, PinnedLaunchInput,
    },
    Error, Result,
};
use serde::{Deserialize, Serialize};
use std::path::{Component, Path, PathBuf};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkloadConfig {
    pub image: PinnedImage,
    pub platform: LaunchPlatform,
    pub cache_root: PathBuf,
    pub memory_bytes: u64,
    pub scratch_bytes: u64,
    pub storage: WorkloadStorageLimits,
    /// Same public Runner ZIP supplies the selected execution/setup helper on
    /// both providers. Kubernetes bootstrap/transfer has its own image pin.
    pub helper: PinnedLaunchInput,
    pub helper_archive: PathBuf,
    pub backend: WorkloadBackendConfig,
}

/// Retained input/archive/tree ceilings, not a hard live filesystem quota.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkloadStorageLimits {
    pub compressed_bytes: u64,
    pub expanded_bytes: u64,
    pub entries: u64,
    #[serde(default)]
    pub artifacts: ArtifactLimits,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum WorkloadBackendConfig {
    Kubernetes {
        cluster: String,
        namespace: String,
        helper_image: PinnedImage,
    },
    Docker {
        socket: PathBuf,
        cache_volume: String,
        network: String,
        /// An observed replacement daemon requires explicit new operator
        /// binding; restart must not silently rebind retained runtime inputs.
        binding: LaunchEngineBinding,
        pids_limit: i64,
        /// Trusted public helper image supplies the fixed startup binding probe;
        /// an otherwise valid native workload image need not contain a shell.
        helper_image: PinnedImage,
    },
}

fn invalid() -> Error {
    Error::validation("Invalid native workload operator configuration")
}
fn absolute_child(path: &Path) -> bool {
    path.is_absolute()
        && path.components().any(|c| matches!(c, Component::Normal(_)))
        && !path
            .components()
            .any(|c| matches!(c, Component::ParentDir | Component::CurDir))
        && path
            .to_str()
            .is_some_and(|s| !s.chars().any(char::is_control))
}
fn image(image: &PinnedImage) -> bool {
    image
        .reference
        .split_once("@sha256:")
        .is_some_and(|(name, digest)| {
            !name.is_empty()
                && !name.contains(['@', '?', '#', '\\'])
                && !name.contains("://")
                && !name.chars().any(char::is_whitespace)
                && digest == image.sha256.as_str()
        })
}
fn engine_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && name
            .bytes()
            .next()
            .is_some_and(|b| b.is_ascii_alphanumeric())
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
}
impl WorkloadConfig {
    pub fn validate(&self) -> Result<()> {
        if !image(&self.image)
            || !absolute_child(&self.cache_root)
            || self.memory_bytes < 32 * 1024 * 1024
            || self.memory_bytes > i64::MAX as u64
            || self.scratch_bytes == 0
            || self.scratch_bytes > self.memory_bytes
            || self.storage.compressed_bytes == 0
            || self.storage.compressed_bytes > 1024 * 1024 * 1024
            || self.storage.expanded_bytes == 0
            || self.storage.expanded_bytes > 4 * 1024 * 1024 * 1024
            || self.storage.entries == 0
            || self.storage.entries > 100_000
        {
            return Err(invalid());
        }
        crate::workload_artifact::validate_inventory(&[], self.storage.artifacts)
            .map_err(|_| invalid())?;
        let helper = &self.helper;
        if !absolute_child(&self.helper_archive)
            || helper.kind != LaunchInputKind::Runner
            || helper.pin_id <= 0
            || helper.object_id <= 0
            || helper.object_version.is_empty()
            || helper.size_bytes == 0
            || helper.size_bytes > self.storage.compressed_bytes
            || helper.max_expanded_bytes == 0
            || helper.max_expanded_bytes > self.storage.expanded_bytes
            || helper.max_files == 0
            || helper.max_files > self.storage.entries
        {
            return Err(invalid());
        }
        match &self.backend {
            WorkloadBackendConfig::Kubernetes {
                cluster,
                namespace,
                helper_image,
            } => {
                if cluster.is_empty()
                    || cluster.len() > 128
                    || cluster.chars().any(char::is_control)
                    || namespace.is_empty()
                    || namespace.len() > 63
                    || namespace.starts_with('-')
                    || namespace.ends_with('-')
                    || !namespace
                        .bytes()
                        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
                    || !image(helper_image)
                {
                    return Err(invalid());
                }
            }
            WorkloadBackendConfig::Docker {
                socket,
                cache_volume,
                network,
                binding,
                pids_limit,
                helper_image,
            } => {
                binding.validate_shape().map_err(|_| invalid())?;
                if binding.platform != self.platform
                    || !absolute_child(socket)
                    || !engine_name(cache_volume)
                    || !engine_name(network)
                    || matches!(network.as_str(), "host" | "none")
                    || *pids_limit < 16
                    || !image(helper_image)
                {
                    return Err(invalid());
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn fixture() -> serde_json::Value {
        let hash = "a".repeat(64);
        let image =
            json!({"reference":format!("fixture.invalid/runtime@sha256:{hash}"),"sha256":hash});
        json!({"image":image,"platform":"linux/amd64","cache_root":"/var/lib/attune/workloads",
            "memory_bytes":268435456,"scratch_bytes":67108864,
            "storage":{"compressed_bytes":67108864,"expanded_bytes":134217728,"entries":4096},
            "helper_archive":"/opt/attune/public/runner.zip",
            "helper":{"pin_id":1,"kind":"runner","object_id":2,"object_version":"owned-helper",
                "size_bytes":1024,"sha256":"a".repeat(64),"max_expanded_bytes":4096,"max_files":1},
            "backend":{"kind":"kubernetes","cluster":"owned-cluster","namespace":"workloads","helper_image":image}})
    }
    #[test]
    fn closed_native_config_has_no_relay_or_implicit_developer_context() {
        let value = fixture();
        serde_json::from_value::<WorkloadConfig>(value.clone())
            .unwrap()
            .validate()
            .unwrap();
        for field in [
            "enrollment_token",
            "controller_url",
            "session",
            "kubeconfig",
            "dispatch",
        ] {
            let mut invalid = value.clone();
            invalid["backend"][field] = json!("must-not-be-accepted");
            assert!(serde_json::from_value::<WorkloadConfig>(invalid).is_err());
        }
        for path in ["/", "relative", "/var/../etc"] {
            let mut invalid = value.clone();
            invalid["cache_root"] = json!(path);
            assert!(serde_json::from_value::<WorkloadConfig>(invalid)
                .unwrap()
                .validate()
                .is_err());
        }
        for field in ["namespace", "helper_image"] {
            let mut invalid = value.clone();
            invalid["backend"].as_object_mut().unwrap().remove(field);
            assert!(serde_json::from_value::<WorkloadConfig>(invalid).is_err());
        }
        for field in ["helper", "helper_archive"] {
            let mut invalid = value.clone();
            invalid.as_object_mut().unwrap().remove(field);
            assert!(serde_json::from_value::<WorkloadConfig>(invalid).is_err());
        }
        let mut invalid = value;
        invalid["image"]["reference"] = json!("fixture.invalid/runtime:latest");
        assert!(serde_json::from_value::<WorkloadConfig>(invalid)
            .unwrap()
            .validate()
            .is_err());
    }
    #[test]
    fn docker_configuration_requires_exact_engine_platform_and_fixed_public_helper() {
        let mut value = fixture();
        let helper_image = value["image"].clone();
        value["backend"] = json!({"kind":"docker","socket":"/var/run/docker.sock",
            "cache_volume":"owned-cache","network":"owned-network",
            "binding":{"engine_id":1,"daemon_id":"owned-daemon","platform":"linux/amd64"},
            "pids_limit":64,"helper_image":helper_image});
        serde_json::from_value::<WorkloadConfig>(value.clone())
            .unwrap()
            .validate()
            .unwrap();
        for (field, bad) in [
            ("network", json!("host")),
            ("socket", json!("tcp://remote:2375")),
            ("pids_limit", json!(0)),
            ("cache_volume", json!("../other-volume")),
        ] {
            let mut invalid = value.clone();
            invalid["backend"][field] = bad;
            assert!(
                serde_json::from_value::<WorkloadConfig>(invalid)
                    .unwrap()
                    .validate()
                    .is_err(),
                "{field}"
            );
        }
        let mut wrong_platform = value.clone();
        wrong_platform["backend"]["binding"]["platform"] = json!("linux/arm64");
        assert!(serde_json::from_value::<WorkloadConfig>(wrong_platform)
            .unwrap()
            .validate()
            .is_err());
        let mut wrong_helper = value.clone();
        wrong_helper["helper"]["kind"] = json!("pack");
        assert!(serde_json::from_value::<WorkloadConfig>(wrong_helper)
            .unwrap()
            .validate()
            .is_err());
        for field in ["compressed_bytes", "expanded_bytes", "entries"] {
            let mut invalid = value.clone();
            invalid["storage"][field] = json!(0);
            assert!(serde_json::from_value::<WorkloadConfig>(invalid)
                .unwrap()
                .validate()
                .is_err());
        }
    }
}
