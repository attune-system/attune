use attune_common::workload_launch::*;
use std::collections::{BTreeMap, BTreeSet};
use uuid::Uuid;

pub fn spec(image: String) -> LaunchSpec {
    let digest = image
        .rsplit_once("sha256:")
        .expect("pinned fixture image")
        .1;
    LaunchSpec {
        manifest: LaunchManifest {
            launch_id: 1,
            purpose: LaunchPurpose::Action,
            target: LaunchTarget::Execution {
                execution: 2,
                worker: 3,
                worker_instance: Uuid::new_v4(),
                attempt: Uuid::new_v4(),
            },
            owner_identity_id: 4,
            engine_id: 5,
            engine_name_nonce: Uuid::new_v4(),
            image: PinnedImage {
                reference: image.clone(),
                sha256: Sha256Digest::parse(digest).unwrap(),
            },
            context: FrozenContextRef {
                id: 6,
                version: 1,
                ciphertext_sha256: Sha256Digest::of_bytes(b"encrypted-context"),
            },
            inputs: vec![],
            pack_access: PackAccess::ReadOnly,
            stage: None,
            timeout_ms: Some(10000),
            lease_deadline: chrono::Utc::now() + chrono::Duration::seconds(30),
        },
        invocation: LaunchInvocation {
            argv: vec!["/bin/true".into()],
            env: BTreeMap::new(),
            working_root: LaunchRoot::Scratch,
        },
    }
}
pub fn policy(image: String, network: String) -> crate::engine::EnginePolicy {
    crate::engine::EnginePolicy {
        engine_id: 5,
        cache_volume: "owned-fixture-cache".into(),
        network,
        allowed_images: BTreeSet::from([image]),
        memory_bytes: 256 * 1024 * 1024,
        pids_limit: 64,
        scratch_bytes: 32 * 1024 * 1024,
    }
}
