//! Coordinated deployment admission, shared by ordinary uploads and core bootstrap.
//! The caller owns the transaction and must roll it back on every error.

use super::{
    loader::PackLoadResult, runtime_policy::CandidateRuntimePolicyPlan, PackComponentLoader,
};
use crate::{
    delegation::DelegationAuthority,
    models::Id,
    repositories::{
        component_configuration::ComponentConfigurationRepository, pack::PackRepository,
        pack_release::PackReleaseRepository, rule_authority::RuleAuthorityRepository,
        runtime_authority::RuntimeAuthorityRepository, FindById,
    },
    Error, Result,
};
use serde_json::Value;
use std::path::{Path, PathBuf};

/// Release switch only. Never enable independently of recorded runtime consumers.
pub const BOOTSTRAP_ADMISSION_ENABLED: bool = false;

/// Ordinary authenticated publication remains available while the broader
/// managed runtime contract and registrar exemptions are disabled.
pub fn publication_descriptor() -> Value {
    serde_json::json!({
        "protocol_version": 1,
        "client_version": CLIENT_VERSION,
        "revision": "authenticated-pack-publication-1",
        "authorization": "registrar",
        "components": ["immutable_release", "atomic_deployment"],
    })
}
pub const CONTRACT_VERSION: u32 = 1;
pub const CLIENT_VERSION: u32 = 1;
pub const COMPONENTS: &[&str] = &[
    "immutable_release",
    "runtime_policy",
    "workflow_graphs",
    "declared_inputs",
    "concrete_targets",
    "rule_authority",
    "configuration_layers",
    "sensor_metadata",
    "component_readmission",
    "candidate_effect_fence",
];

pub fn descriptor() -> Value {
    serde_json::json!({
        "contract_version": CONTRACT_VERSION,
        "client_version": CLIENT_VERSION,
        "revision": "managed-pack-admission-1",
        "enabled": BOOTSTRAP_ADMISSION_ENABLED,
        "components": COMPONENTS,
    })
}

/// Only this owner can defer registrar coverage to the canonical policy compiler.
/// This is pending admission, not a runtime grant or a commit permit.
#[must_use = "finish admission in the same transaction before committing"]
pub struct DeploymentAdmission {
    plan: CandidateRuntimePolicyPlan,
    source: PathBuf,
    pack_ref: String,
    authored_defaults: Value,
}

impl DeploymentAdmission {
    pub fn read(source: &Path, pack_ref: &str) -> Result<Self> {
        let source_metadata = std::fs::read_to_string(source.join("pack.yaml"))
            .map_err(|_| Error::validation("Deployment pack metadata source is unavailable"))?;
        let metadata: Value = serde_yaml_ng::from_str(&source_metadata)
            .map_err(|_| Error::validation("Deployment pack metadata is invalid"))?;
        Ok(Self {
            plan: CandidateRuntimePolicyPlan::read(source, pack_ref)?,
            source: source.into(),
            pack_ref: pack_ref.into(),
            authored_defaults: metadata
                .get("config")
                .cloned()
                .unwrap_or_else(|| serde_json::json!({})),
        })
    }

    /// Must precede cache, pack, sponsor and component locks, even on fresh install.
    pub async fn lock(connection: &mut sqlx::PgConnection) -> Result<()> {
        crate::repositories::schema_type::SchemaTypeRepository::lock_reclamation(connection).await
    }

    /// Capture ORIGINAL layers before any deployment projection is mutated.
    pub async fn preserve(
        connection: &mut sqlx::PgConnection,
        pack_id: Id,
        encryption_key: Option<&str>,
    ) -> Result<()> {
        ComponentConfigurationRepository::preserve_before_deployment(
            connection,
            pack_id,
            encryption_key,
        )
        .await
    }

    pub async fn load(
        &self,
        loader: PackComponentLoader<'_>,
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    ) -> Result<PackLoadResult> {
        loader
            .with_pending_admission()
            .load_all_in_transaction(tx, &self.source)
            .await
    }

    /// Run after structural sensor assessment and immutable release activation,
    /// but before finish_activation and the single outer commit. A/H capture graph,
    /// input declarations and concrete targets here. D binds preserved layers and
    /// readmits sensor metadata. No installer's internal runtime grants are copied.
    pub async fn finish(
        self,
        connection: &mut sqlx::PgConnection,
        pack_id: Id,
        release_id: Id,
        encryption_key: Option<&str>,
        installer: &DelegationAuthority,
    ) -> Result<()> {
        let pack = PackRepository::find_by_id(&mut *connection, pack_id)
            .await?
            .ok_or_else(|| Error::validation("Deployment pack is unavailable"))?;
        if pack.r#ref != self.pack_ref || pack.active_release != Some(release_id) {
            return Err(Error::validation(
                "Deployment admission release binding changed",
            ));
        }
        let release = PackReleaseRepository::find_by_id(&mut *connection, release_id)
            .await?
            .ok_or_else(|| Error::validation("Deployment code release is unavailable"))?;
        if release.pack != pack_id || Path::new(&release.content_path) != self.source {
            return Err(Error::validation(
                "Deployment plan differs from its immutable code source",
            ));
        }
        RuntimeAuthorityRepository::ensure_owner(connection, pack_id).await?;
        let policy = self
            .plan
            .admit_deployment(connection, pack_id, installer.identity_id)
            .await?;
        RuleAuthorityRepository::capture_deployment(connection, pack_id).await?;
        ComponentConfigurationRepository::bind_deployment(
            connection,
            pack_id,
            &self.authored_defaults,
            encryption_key,
            installer,
        )
        .await?;
        RuntimeAuthorityRepository::require_deployment_policy(connection, policy.id).await?;
        Ok(())
    }
}
