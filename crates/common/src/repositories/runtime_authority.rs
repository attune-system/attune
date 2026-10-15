//! Persistence and live fences for recorded pack-owned authority.
//!
//! Callers retain the transaction through the authorized effect. Lock order is
//! schema admission, pack, runtime sponsor, policy, runtime owner, approvals.
//! Human identities are checked only when issuing an approval, not at consumption.

use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::{types::Json, PgConnection};
use uuid::Uuid;

use super::{identity::IdentityRepository, schema_type::SchemaTypeRepository};
use crate::{
    delegation::DelegationAuthority,
    models::Id,
    rbac::{Action, AuthorizationContext, Grant, Resource},
    runtime_authority::{
        positive_id, validate_external_approval_grants, AdmittedGrant, ExternalApproval,
        PackAuthorityOwner, RuntimePolicy, SelectedRuntimeAuthority,
    },
    value_schema::digest,
    Error, Result,
};

const POLICY_COLUMNS: &str = "id, policy, policy_digest, revoked_at";
const APPROVAL_COLUMNS: &str = "id, subject_pack, issuer_identity, issuer_login, grants, approval_digest, expires_at, revoked_at";
const ADMISSION_COLUMNS: &str =
    "id, owner_kind, owner_id, policy_revision, authority, authority_digest, revoked_at";

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct RuntimePolicyRevision {
    pub id: Id,
    pub policy: Json<RuntimePolicy>,
    pub policy_digest: String,
    pub revoked_at: Option<DateTime<Utc>>,
}

impl RuntimePolicyRevision {
    fn validate(&self) -> Result<()> {
        positive_id(self.id)?;
        self.policy.validate()?;
        require_digest(&self.policy.0, &self.policy_digest)
    }
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ExternalApprovalRecord {
    pub id: Id,
    pub subject_pack: Id,
    pub issuer_identity: Id,
    pub issuer_login: String,
    pub grants: Json<Vec<Grant>>,
    pub approval_digest: String,
    pub expires_at: Option<DateTime<Utc>>,
    pub revoked_at: Option<DateTime<Utc>>,
}

impl ExternalApprovalRecord {
    pub fn approval(&self) -> Result<ExternalApproval> {
        let approval = ExternalApproval {
            id: self.id,
            subject_pack_id: self.subject_pack,
            grants: self.grants.0.clone(),
        };
        approval.validate()?;
        let body = approval_body(self.subject_pack, &self.grants.0);
        require_digest(&body, &self.approval_digest)?;
        Ok(approval)
    }

    pub fn is_active(&self, now: DateTime<Utc>) -> bool {
        self.revoked_at.is_none() && self.expires_at.is_none_or(|expires| expires > now)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeOwner {
    Execution(Id),
    Enforcement(Id),
}

impl RuntimeOwner {
    fn parts(self) -> (&'static str, Id) {
        match self {
            Self::Execution(id) => ("execution", id),
            Self::Enforcement(id) => ("enforcement", id),
        }
    }
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct RuntimeAdmissionRecord {
    pub id: Id,
    pub owner_kind: String,
    pub owner_id: Id,
    pub policy_revision: Id,
    pub authority: Json<SelectedRuntimeAuthority>,
    pub authority_digest: String,
    pub revoked_at: Option<DateTime<Utc>>,
}

impl RuntimeAdmissionRecord {
    fn validate(&self, policy: &RuntimePolicyRevision) -> Result<()> {
        positive_id(self.id)?;
        positive_id(self.owner_id)?;
        if self.policy_revision != policy.id || self.authority.policy_id != policy.id {
            return Err(Error::validation(
                "Runtime admission policy reference mismatch",
            ));
        }
        require_digest(&self.authority.0, &self.authority_digest)?;
        self.authority.validate_against(&policy.policy)
    }
}

/// This is the server-side fence carried by an execution credential.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeCredentialBinding {
    pub execution_id: Id,
    pub admission_id: Id,
    pub sponsor_identity_id: Id,
    pub action_ref: String,
    pub attempt: Uuid,
}

/// Constructed only after repository-owned liveness and approval validation.
pub struct ValidatedRuntimeAuthority {
    record: RuntimeAdmissionRecord,
    approvals: BTreeSet<Id>,
    current_attempt: Uuid,
    standard_owners: BTreeSet<(String, Id)>,
}

impl ValidatedRuntimeAuthority {
    pub fn cache_read_filters(&self) -> Vec<super::cache::CacheNamespaceGrantFilter> {
        use super::cache::{CacheGrantContext, CacheNamespaceGrantFilter, CacheOwnershipScope};
        let mut filters: Vec<_> =
            self.record
                .authority
                .grants
                .iter()
                .filter_map(|grant| {
                    let ownership = match grant {
                        crate::runtime_authority::AdmittedGrant::PackOwned { pack_id, .. } => {
                            CacheOwnershipScope::ManagedPack(*pack_id)
                        }
                        crate::runtime_authority::AdmittedGrant::External {
                            approval_id, ..
                        } if self.approvals.contains(approval_id) => CacheOwnershipScope::Any,
                        _ => return None,
                    };
                    CacheNamespaceGrantFilter::from_recorded_grant(grant.grant(), ownership)
                })
                .collect();
        filters.extend(self.standard_owners.iter().filter_map(|(kind, id)| {
            let ownership = match kind.as_str() {
                "pack" => CacheOwnershipScope::Pack(*id),
                "action" => CacheOwnershipScope::Action(*id),
                _ => return None,
            };
            Some(CacheNamespaceGrantFilter {
                ownership,
                context: CacheGrantContext::Recorded,
                ..Default::default()
            })
        }));
        filters
    }

    pub fn key_read_filters(&self) -> Vec<super::key::KeyGrantFilter> {
        use super::key::{KeyGrantFilter, KeyOwnershipScope};
        let mut filters: Vec<_> =
            self.record
                .authority
                .grants
                .iter()
                .filter_map(|grant| {
                    let ownership = match grant {
                        crate::runtime_authority::AdmittedGrant::PackOwned { pack_id, .. } => {
                            KeyOwnershipScope::ManagedPack(*pack_id)
                        }
                        crate::runtime_authority::AdmittedGrant::External {
                            approval_id, ..
                        } if self.approvals.contains(approval_id) => KeyOwnershipScope::Any,
                        _ => return None,
                    };
                    KeyGrantFilter::from_recorded_grant(grant.grant(), ownership)
                })
                .collect();
        filters.extend(self.standard_owners.iter().filter_map(|(kind, id)| {
            let ownership = match kind.as_str() {
                "pack" => KeyOwnershipScope::Pack(*id),
                "action" => KeyOwnershipScope::Action(*id),
                _ => return None,
            };
            Some(KeyGrantFilter {
                ownership,
                owner_scoped: true,
                ..Default::default()
            })
        }));
        filters
    }

    pub fn credential_binding(&self) -> RuntimeCredentialBinding {
        RuntimeCredentialBinding {
            execution_id: self.record.owner_id,
            admission_id: self.record.id,
            sponsor_identity_id: self.record.authority.owner.sponsor_identity_id,
            action_ref: self.record.authority.binding.action_ref().into(),
            attempt: self.current_attempt,
        }
    }
    pub fn authority(&self) -> &SelectedRuntimeAuthority {
        &self.record.authority
    }

    /// Match grants against repository-resolved ownership and context rather than
    /// caller-supplied pack IDs. Read/disclosure checks still need origin policy.
    pub fn allows_target(
        &self,
        target: &super::runtime_target::ResolvedRuntimeTarget,
        action: Action,
    ) -> bool {
        let context = target.context();
        let standard_operation = match target.resource() {
            Resource::Keys => matches!(action, Action::Read | Action::Decrypt),
            Resource::Artifacts => matches!(
                action,
                Action::Read | Action::Create | Action::Update | Action::Delete
            ),
            Resource::Caches => action == Action::Read,
            _ => false,
        };
        let owner_kind = match context.owner_type {
            Some(crate::models::OwnerType::Pack) => Some("pack"),
            Some(crate::models::OwnerType::Action) => Some("action"),
            _ => None,
        };
        if standard_operation
            && self.standard_owners.iter().any(|(kind, id)| {
                Some(kind.as_str()) == owner_kind && Some(*id) == target.owner_id()
            })
        {
            return true;
        }
        self.record.authority.grants.iter().any(|grant| {
            let private_key = target.resource() == Resource::Keys
                && context.owner_type == Some(crate::models::OwnerType::Identity)
                && context.owner_identity_id != Some(context.identity_id);
            if private_key
                && !grant.grant().constraints.as_ref().is_some_and(|scope| {
                    scope.owner.is_some()
                        || scope.owner_types.is_some()
                        || scope.owner_refs.is_some()
                        || scope.refs.is_some()
                        || scope.ids.is_some()
                })
            {
                return false;
            }
            grant.allows(
                &self.approvals,
                target.pack_id(),
                target.resource(),
                action,
                context,
            )
        })
    }

    /// New effects also require the resolved target's current enabled lifecycle.
    pub fn allows_new_effect(
        &self,
        target: &super::runtime_target::ResolvedRuntimeTarget,
        action: Action,
    ) -> bool {
        target.accepts_new_effects() && self.allows_target(target, action)
    }

    pub fn allows(
        &self,
        target_pack_id: Option<Id>,
        resource: Resource,
        action: Action,
        context: &AuthorizationContext,
    ) -> bool {
        self.record
            .authority
            .grants
            .iter()
            .any(|grant| grant.allows(&self.approvals, target_pack_id, resource, action, context))
    }
}

pub struct RuntimeAuthorityRepository;

impl RuntimeAuthorityRepository {
    /// Historical, validated selection for worker protection and disclosure.
    /// This is not an effect permit and deliberately includes revoked records.
    pub async fn execution_admission(
        connection: &mut PgConnection,
        execution: Id,
    ) -> Result<Option<RuntimeAdmissionRecord>> {
        let query = format!(
            "SELECT {ADMISSION_COLUMNS} FROM runtime_authority_admission \
            WHERE owner_kind = 'execution' AND owner_id = $1"
        );
        let record: Option<RuntimeAdmissionRecord> = sqlx::query_as(&query)
            .bind(execution)
            .fetch_optional(&mut *connection)
            .await?;
        if let Some(record) = &record {
            let policy = Self::find_policy(connection, record.policy_revision)
                .await?
                .ok_or_else(|| Error::validation("Runtime admission policy is unavailable"))?;
            record.validate(&policy)?;
        }
        Ok(record)
    }

    /// Deployment helper: reuse fixed active approvals for this subject, then
    /// automatically record only missing grants that the installer actually
    /// covers. Permissions/Manage alone is not automatic runtime delegation;
    /// administrators can issue an explicit approval through the control API.
    /// The caller must separately prove scoped pack deployment authority.
    pub async fn admit_external_grants(
        connection: &mut PgConnection,
        subject_pack: Id,
        installer_identity: Id,
        grants: &[Grant],
    ) -> Result<Vec<ExternalApproval>> {
        if grants.is_empty() {
            return Ok(Vec::new());
        }
        validate_external_approval_grants(grants)?;
        SchemaTypeRepository::lock_selection(&mut *connection).await?;
        let owner = Self::find_owner(connection, subject_pack)
            .await?
            .ok_or_else(|| {
                Error::PermissionDenied("Deployment runtime sponsor is unavailable".into())
            })?;
        lock_sponsor(connection, &owner).await?;
        let installer = DelegationAuthority::load_for_share(connection, installer_identity).await?;
        let query = format!(
            "SELECT {APPROVAL_COLUMNS} FROM runtime_external_approval \
             WHERE subject_pack = $1 AND revoked_at IS NULL \
               AND (expires_at IS NULL OR expires_at > clock_timestamp()) ORDER BY id FOR SHARE",
        );
        let records: Vec<ExternalApprovalRecord> = sqlx::query_as(&query)
            .bind(subject_pack)
            .fetch_all(&mut *connection)
            .await?;
        let now: DateTime<Utc> = sqlx::query_scalar("SELECT clock_timestamp()")
            .fetch_one(&mut *connection)
            .await?;
        let mut approvals = records
            .into_iter()
            .filter(|record| record.is_active(now))
            .map(|record| record.approval())
            .collect::<Result<Vec<_>>>()?;
        let missing: Vec<_> = grants
            .iter()
            .filter(|grant| {
                !approvals
                    .iter()
                    .any(|approval| approval.covers(&owner, grant))
            })
            .cloned()
            .collect();
        if !missing.is_empty() {
            installer.require_grants(&missing).map_err(|_| Error::PermissionDenied(
                "External runtime scope requires separate approval by an authorized owner or administrator".into(),
            ))?;
            approvals.push(
                Self::issue_approval(connection, subject_pack, installer_identity, missing, None)
                    .await?
                    .approval()?,
            );
        }
        Ok(approvals)
    }

    pub async fn list_approvals(
        connection: &mut PgConnection,
        pack_id: Id,
        limit: i64,
        offset: i64,
    ) -> Result<(Vec<ExternalApprovalRecord>, i64)> {
        positive_id(pack_id)?;
        let query = format!(
            "SELECT {APPROVAL_COLUMNS} FROM runtime_external_approval \
             WHERE subject_pack = $1 ORDER BY id DESC LIMIT $2 OFFSET $3",
        );
        let records: Vec<ExternalApprovalRecord> = sqlx::query_as(&query)
            .bind(pack_id)
            .bind(limit.clamp(1, 100))
            .bind(offset.max(0))
            .fetch_all(&mut *connection)
            .await?;
        for record in &records {
            record.approval()?;
        }
        let total = sqlx::query_scalar(
            "SELECT count(*) FROM runtime_external_approval WHERE subject_pack = $1",
        )
        .bind(pack_id)
        .fetch_one(connection)
        .await?;
        Ok((records, total))
    }

    /// Replacement creates a new fixed approval and revokes the predecessor in
    /// the caller's transaction. It never edits grants already pinned by a run.
    pub async fn replace_approval(
        connection: &mut PgConnection,
        previous_id: Id,
        issuer_identity: Id,
        grants: Vec<Grant>,
        expires_at: Option<DateTime<Utc>>,
    ) -> Result<ExternalApprovalRecord> {
        let previous = Self::find_approval(connection, previous_id)
            .await?
            .ok_or_else(|| Error::validation("External approval is unavailable"))?;
        SchemaTypeRepository::lock_selection(&mut *connection).await?;
        sqlx::query_scalar::<_, Id>("SELECT id FROM pack WHERE id = $1 FOR SHARE")
            .bind(previous.subject_pack)
            .fetch_one(&mut *connection)
            .await?;
        let issuer = DelegationAuthority::load_for_share(connection, issuer_identity).await?;
        if !issuer.manages_permissions() {
            issuer.require_grants(&previous.grants)?;
        }
        let replacement = Self::issue_approval(
            connection,
            previous.subject_pack,
            issuer_identity,
            grants,
            expires_at,
        )
        .await?;
        let revoked = sqlx::query(
            "UPDATE runtime_external_approval SET revoked_at = NOW() WHERE id = $1 AND revoked_at IS NULL",
        ).bind(previous_id).execute(connection).await?;
        if revoked.rows_affected() != 1 {
            return Err(Error::PermissionDenied(
                "Revoked approvals cannot be replaced; issue a new approval explicitly".into(),
            ));
        }
        Ok(replacement)
    }

    pub async fn find_owner(
        connection: &mut PgConnection,
        pack_id: Id,
    ) -> Result<Option<PackAuthorityOwner>> {
        let row: Option<(Id, String, Id)> = sqlx::query_as(
            "SELECT p.id, p.ref, r.identity FROM pack p \
             JOIN pack_runtime_identity r ON r.pack = p.id WHERE p.id = $1",
        )
        .bind(pack_id)
        .fetch_optional(connection)
        .await?;
        row.map(|(pack_id, pack_ref, sponsor_identity_id)| {
            let owner = PackAuthorityOwner {
                pack_id,
                pack_ref,
                sponsor_identity_id,
            };
            owner.validate()?;
            Ok(owner)
        })
        .transpose()
    }

    /// Establishes a fixed external approval. Freezing/removing its issuer later
    /// does not revoke it. A permission administrator may explicitly approve it.
    pub async fn issue_approval(
        connection: &mut PgConnection,
        subject_pack: Id,
        issuer_identity: Id,
        grants: Vec<Grant>,
        expires_at: Option<DateTime<Utc>>,
    ) -> Result<ExternalApprovalRecord> {
        positive_id(subject_pack)?;
        positive_id(issuer_identity)?;
        if grants.is_empty() || expires_at.is_some_and(|expiry| expiry <= Utc::now()) {
            return Err(Error::validation(
                "External approval requires grants and a future expiry",
            ));
        }
        validate_external_approval_grants(&grants)?;
        SchemaTypeRepository::lock_selection(&mut *connection).await?;
        let exists: Option<Id> = sqlx::query_scalar("SELECT id FROM pack WHERE id = $1 FOR SHARE")
            .bind(subject_pack)
            .fetch_optional(&mut *connection)
            .await?;
        if exists.is_none() {
            return Err(Error::validation(
                "External approval subject pack is unavailable",
            ));
        }
        let issuer = DelegationAuthority::load_for_share(connection, issuer_identity).await?;
        if !issuer.manages_permissions() {
            issuer.require_grants(&grants)?;
        }
        let identity = IdentityRepository::find_by_id_for_share(connection, issuer_identity)
            .await?
            .ok_or_else(|| Error::PermissionDenied("Approval issuer is unavailable".into()))?;
        let body = approval_body(subject_pack, &grants);
        let digest = digest(&body).map_err(|_| Error::validation("Invalid approval metadata"))?;
        let query = format!(
            "INSERT INTO runtime_external_approval \
             (subject_pack, issuer_identity, issuer_login, grants, approval_digest, expires_at) \
             VALUES ($1,$2,$3,$4,$5,$6) RETURNING {APPROVAL_COLUMNS}"
        );
        let record: ExternalApprovalRecord = sqlx::query_as(&query)
            .bind(subject_pack)
            .bind(issuer_identity)
            .bind(identity.login)
            .bind(Json(grants))
            .bind(digest)
            .bind(expires_at)
            .fetch_one(connection)
            .await?;
        record.approval()?;
        Ok(record)
    }

    pub async fn find_approval(
        connection: &mut PgConnection,
        id: Id,
    ) -> Result<Option<ExternalApprovalRecord>> {
        let query =
            format!("SELECT {APPROVAL_COLUMNS} FROM runtime_external_approval WHERE id = $1");
        let record: Option<ExternalApprovalRecord> = sqlx::query_as(&query)
            .bind(id)
            .fetch_optional(connection)
            .await?;
        if let Some(record) = &record {
            record.approval()?;
        }
        Ok(record)
    }

    /// Captures an immutable policy after candidate graph admission. It does not
    /// grant deployment authority or bypass registrar validation on its own.
    pub async fn capture_policy(
        connection: &mut PgConnection,
        policy: &RuntimePolicy,
    ) -> Result<RuntimePolicyRevision> {
        policy.validate()?;
        SchemaTypeRepository::lock_selection(&mut *connection).await?;
        lock_sponsor(connection, &policy.owner).await?;
        let release: Option<String> = sqlx::query_scalar(
            "SELECT digest FROM pack_release WHERE id = $1 AND pack = $2 FOR SHARE",
        )
        .bind(policy.release_id)
        .bind(policy.owner.pack_id)
        .fetch_optional(&mut *connection)
        .await?;
        if release.as_deref() != Some(policy.release_digest.as_str()) {
            return Err(Error::validation(
                "Runtime policy release owner or digest mismatch",
            ));
        }
        let mut standard_bindings = BTreeSet::new();
        for invocation in &policy.invocations {
            let target_release: Option<(String, Id)> =
                sqlx::query_as("SELECT digest, pack FROM pack_release WHERE id = $1 FOR SHARE")
                    .bind(invocation.target_release_id)
                    .fetch_optional(&mut *connection)
                    .await?;
            if !target_release
                .as_ref()
                .is_some_and(|(digest, _)| digest == &invocation.target_release_digest)
            {
                return Err(Error::validation(
                    "Invocation executable release is unavailable",
                ));
            }
            let (_, target_pack) = target_release
                .ok_or_else(|| Error::validation("Invocation executable release is unavailable"))?;
            let executable: Option<serde_json::Value> = sqlx::query_scalar(
                "SELECT snapshot FROM pack_release_executable \
                 WHERE release = $1 AND component_kind = 'action' AND component_ref = $2 FOR SHARE",
            )
            .bind(invocation.target_release_id)
            .bind(invocation.binding.action_ref())
            .fetch_optional(&mut *connection)
            .await?;
            let executable = executable.ok_or_else(|| {
                Error::validation(
                    "Invocation action is not present in the pinned immutable release",
                )
            })?;
            let executable: crate::models::ActionExecutableSnapshot =
                serde_json::from_value(executable)
                    .map_err(|_| Error::validation("Invalid pinned invocation executable"))?;
            if executable.action.r#ref != invocation.binding.action_ref()
                || executable.action.pack != target_pack
                || matches!(
                    &invocation.binding,
                    crate::runtime_authority::InvocationBinding::Action { .. }
                ) && target_pack != policy.owner.pack_id
            {
                return Err(Error::validation(
                    "Invocation executable ownership does not match its policy binding",
                ));
            }
            if invocation.standard_access {
                let invocation_digest = digest(&serde_json::to_value(&invocation.binding)?)
                    .map_err(|_| Error::validation("Invalid standard invocation binding"))?;
                for (kind, id) in [
                    ("action", executable.action.id),
                    ("pack", executable.action.pack),
                ] {
                    positive_id(id)?;
                    standard_bindings.insert((invocation_digest.clone(), kind.to_string(), id));
                }
                if let crate::runtime_authority::InvocationBinding::WorkflowTask {
                    workflow_ref,
                    ..
                } = &invocation.binding
                {
                    let source: serde_json::Value = sqlx::query_scalar(
                        "SELECT snapshot FROM pack_release_executable WHERE release = $1 \
                            AND component_kind = 'action' AND component_ref = $2 FOR SHARE",
                    )
                    .bind(policy.release_id)
                    .bind(workflow_ref)
                    .fetch_optional(&mut *connection)
                    .await?
                    .ok_or_else(|| {
                        Error::validation(
                            "Standard workflow owner is absent from its pinned release",
                        )
                    })?;
                    let source: crate::models::ActionExecutableSnapshot =
                        serde_json::from_value(source).map_err(|_| {
                            Error::validation("Invalid pinned standard workflow owner")
                        })?;
                    if source.action.r#ref != *workflow_ref
                        || source.action.pack != policy.owner.pack_id
                        || source.workflow_definition.is_none()
                    {
                        return Err(Error::validation(
                            "Pinned standard workflow ownership mismatch",
                        ));
                    }
                    for (kind, id) in [("action", source.action.id), ("pack", source.action.pack)] {
                        positive_id(id)?;
                        standard_bindings.insert((invocation_digest.clone(), kind.to_string(), id));
                    }
                }
            }
        }
        let grants: Vec<_> = policy
            .invocations
            .iter()
            .flat_map(|invocation| invocation.permission_sets.values())
            .flat_map(|set| set.grants.iter())
            .collect();
        validate_approvals(connection, &policy.owner, &grants).await?;
        let body_digest = digest(&serde_json::to_value(policy)?)
            .map_err(|_| Error::validation("Invalid runtime policy"))?;
        sqlx::query(
            "INSERT INTO pack_runtime_policy_revision (pack,release,policy,policy_digest) \
             VALUES ($1,$2,$3,$4) \
             ON CONFLICT (pack,release,policy_digest) WHERE revoked_at IS NULL DO NOTHING",
        )
        .bind(policy.owner.pack_id)
        .bind(policy.release_id)
        .bind(Json(policy))
        .bind(&body_digest)
        .execute(&mut *connection)
        .await?;
        let query = format!(
            "SELECT {POLICY_COLUMNS} FROM pack_runtime_policy_revision \
             WHERE pack = $1 AND release = $2 AND policy_digest = $3 AND revoked_at IS NULL FOR SHARE"
        );
        let record: RuntimePolicyRevision = sqlx::query_as(&query)
            .bind(policy.owner.pack_id)
            .bind(policy.release_id)
            .bind(body_digest)
            .fetch_one(&mut *connection)
            .await?;
        record.validate()?;
        for (invocation, kind, id) in standard_bindings {
            sqlx::query(
                "INSERT INTO runtime_standard_owner_binding \
                (policy_revision,invocation_digest,owner_type,owner_id) VALUES ($1,$2,$3,$4) \
                ON CONFLICT DO NOTHING",
            )
            .bind(record.id)
            .bind(invocation)
            .bind(kind)
            .bind(id)
            .execute(&mut *connection)
            .await?;
        }
        let releases: BTreeSet<_> = policy
            .invocations
            .iter()
            .map(|invocation| invocation.target_release_id)
            .chain(std::iter::once(policy.release_id))
            .collect();
        sqlx::query(
            "INSERT INTO runtime_authority_release_dependency (policy_revision,release) \
             SELECT $1, release FROM unnest($2::BIGINT[]) release \
             ON CONFLICT (policy_revision,release) DO NOTHING",
        )
        .bind(record.id)
        .bind(releases.into_iter().collect::<Vec<_>>())
        .execute(connection)
        .await?;
        Ok(record)
    }

    pub async fn find_policy(
        connection: &mut PgConnection,
        id: Id,
    ) -> Result<Option<RuntimePolicyRevision>> {
        let query =
            format!("SELECT {POLICY_COLUMNS} FROM pack_runtime_policy_revision WHERE id = $1");
        let record: Option<RuntimePolicyRevision> = sqlx::query_as(&query)
            .bind(id)
            .fetch_optional(connection)
            .await?;
        if let Some(record) = &record {
            record.validate()?;
        }
        Ok(record)
    }

    pub async fn activate_policy(connection: &mut PgConnection, id: Id) -> Result<()> {
        SchemaTypeRepository::lock_selection(&mut *connection).await?;
        let policy = lock_policy(connection, id).await?;
        let active_release: Option<Id> =
            sqlx::query_scalar("SELECT active_release FROM pack WHERE id = $1")
                .bind(policy.policy.owner.pack_id)
                .fetch_one(&mut *connection)
                .await?;
        if active_release != Some(policy.policy.release_id) {
            return Err(Error::validation(
                "Only the active executable release can acquire current policy",
            ));
        }
        let grants = policy
            .policy
            .invocations
            .iter()
            .flat_map(|invocation| invocation.permission_sets.values())
            .flat_map(|set| set.grants.iter())
            .collect::<Vec<_>>();
        validate_approvals(connection, &policy.policy.owner, &grants).await?;
        sqlx::query(
            "INSERT INTO pack_runtime_policy_current (pack,revision) VALUES ($1,$2) \
             ON CONFLICT (pack) DO UPDATE SET revision = EXCLUDED.revision",
        )
        .bind(policy.policy.owner.pack_id)
        .bind(id)
        .execute(connection)
        .await?;
        Ok(())
    }

    /// Resolves the current admission revision, not the requesting human's IAM.
    /// Existing admissions continue to use their recorded revision after replacement.
    pub async fn current_policy(
        connection: &mut PgConnection,
        pack_id: Id,
    ) -> Result<Option<RuntimePolicyRevision>> {
        positive_id(pack_id)?;
        SchemaTypeRepository::lock_selection(&mut *connection).await?;
        let owner = Self::find_owner(connection, pack_id)
            .await?
            .ok_or_else(|| Error::PermissionDenied("Runtime sponsor pack is unavailable".into()))?;
        lock_sponsor(connection, &owner).await?;
        let revision: Option<Id> = sqlx::query_scalar(
            "SELECT revision FROM pack_runtime_policy_current WHERE pack = $1 FOR SHARE",
        )
        .bind(pack_id)
        .fetch_optional(&mut *connection)
        .await?;
        let Some(revision) = revision else {
            return Ok(None);
        };
        let policy = lock_policy(connection, revision).await?;
        let release: Option<Id> =
            sqlx::query_scalar("SELECT active_release FROM pack WHERE id = $1")
                .bind(pack_id)
                .fetch_one(connection)
                .await?;
        if policy.policy.owner != owner || release != Some(policy.policy.release_id) {
            return Err(Error::PermissionDenied(
                "Current runtime policy does not match the active pack release".into(),
            ));
        }
        Ok(Some(policy))
    }

    pub async fn record_admission(
        connection: &mut PgConnection,
        owner: RuntimeOwner,
        authority: &SelectedRuntimeAuthority,
    ) -> Result<RuntimeAdmissionRecord> {
        let (owner_kind, owner_id) = owner.parts();
        positive_id(owner_id)?;
        SchemaTypeRepository::lock_selection(&mut *connection).await?;
        let policy = lock_policy(connection, authority.policy_id).await?;
        authority.validate_against(&policy.policy)?;
        lock_runtime_owner(connection, owner, authority).await?;
        validate_approvals(
            connection,
            &authority.owner,
            &authority.grants.iter().collect::<Vec<_>>(),
        )
        .await?;
        let body_digest = digest(&serde_json::to_value(authority)?)
            .map_err(|_| Error::validation("Invalid selected runtime authority"))?;
        sqlx::query(
            "INSERT INTO runtime_authority_admission \
             (owner_kind,owner_id,policy_revision,authority,authority_digest) VALUES ($1,$2,$3,$4,$5) \
             ON CONFLICT (owner_kind,owner_id) DO NOTHING",
        )
        .bind(owner_kind)
        .bind(owner_id)
        .bind(authority.policy_id)
        .bind(Json(authority))
        .bind(body_digest)
        .execute(&mut *connection)
        .await?;
        let query = format!(
            "SELECT {ADMISSION_COLUMNS} FROM runtime_authority_admission \
             WHERE owner_kind = $1 AND owner_id = $2 FOR SHARE"
        );
        let record: RuntimeAdmissionRecord = sqlx::query_as(&query)
            .bind(owner_kind)
            .bind(owner_id)
            .fetch_one(&mut *connection)
            .await?;
        record.validate(&policy)?;
        let mut expected = authority.clone();
        // Idempotent admission keeps the first attempt fence; it never rebinds it.
        expected.attempt = record.authority.attempt;
        if record.revoked_at.is_some() || expected != record.authority.0 {
            return Err(Error::PermissionDenied(
                "Runtime owner already has different or revoked authority".into(),
            ));
        }
        if let RuntimeOwner::Execution(execution_id) = owner {
            sqlx::query(
                "INSERT INTO runtime_authority_execution_attempt (execution,admission,attempt) \
                 VALUES ($1,$2,$3) ON CONFLICT (execution,attempt) DO NOTHING",
            )
            .bind(execution_id)
            .bind(record.id)
            .bind(record.authority.attempt)
            .execute(&mut *connection)
            .await?;
            sqlx::query(
                "INSERT INTO runtime_authority_execution_fence (execution,admission,attempt) \
                 VALUES ($1,$2,$3) ON CONFLICT (execution) DO NOTHING",
            )
            .bind(execution_id)
            .bind(record.id)
            .bind(record.authority.attempt)
            .execute(connection)
            .await?;
        }
        Ok(record)
    }

    /// Called by the currently assigned worker before starting an actual attempt.
    /// Retrying this operation with the same attempt ID is idempotent.
    pub async fn begin_execution_attempt(
        connection: &mut PgConnection,
        execution_id: Id,
        worker_id: Id,
        attempt: Uuid,
    ) -> Result<RuntimeCredentialBinding> {
        positive_id(execution_id)?;
        positive_id(worker_id)?;
        if attempt.is_nil() {
            return Err(Error::validation(
                "Runtime attempt requires a nonempty fence",
            ));
        }
        SchemaTypeRepository::lock_selection(&mut *connection).await?;
        let query = format!(
            "SELECT {ADMISSION_COLUMNS} FROM runtime_authority_admission \
             WHERE owner_kind = 'execution' AND owner_id = $1"
        );
        let initial: RuntimeAdmissionRecord = sqlx::query_as(&query)
            .bind(execution_id)
            .fetch_optional(&mut *connection)
            .await?
            .ok_or_else(|| {
                Error::PermissionDenied("Execution runtime admission is unavailable".into())
            })?;
        let policy = lock_policy(connection, initial.policy_revision).await?;
        let worker: Option<(Option<Id>, String)> =
            sqlx::query_as("SELECT worker, status::text FROM execution WHERE id = $1 FOR UPDATE")
                .bind(execution_id)
                .fetch_optional(&mut *connection)
                .await?;
        if worker != Some((Some(worker_id), "running".into())) {
            return Err(Error::PermissionDenied(
                "Runtime attempt requires the assigned running worker".into(),
            ));
        }
        lock_runtime_owner(
            connection,
            RuntimeOwner::Execution(execution_id),
            &initial.authority,
        )
        .await?;
        let query = format!("{query} FOR SHARE");
        let record: RuntimeAdmissionRecord = sqlx::query_as(&query)
            .bind(execution_id)
            .fetch_one(&mut *connection)
            .await?;
        record.validate(&policy)?;
        if record.revoked_at.is_some() || !record.authority.has_api_access() {
            return Err(Error::PermissionDenied(
                "Execution has no active runtime API admission".into(),
            ));
        }
        let current: Option<Uuid> = sqlx::query_scalar(
            "SELECT attempt FROM runtime_authority_execution_fence \
             WHERE execution = $1 AND admission = $2 FOR UPDATE",
        )
        .bind(execution_id)
        .bind(record.id)
        .fetch_optional(&mut *connection)
        .await?;
        if current.is_none() {
            return Err(Error::PermissionDenied(
                "Runtime attempt fence is unavailable".into(),
            ));
        }
        if current != Some(attempt) {
            let inserted = sqlx::query(
                "INSERT INTO runtime_authority_execution_attempt (execution,admission,attempt) \
                 VALUES ($1,$2,$3) ON CONFLICT (execution,attempt) DO NOTHING",
            )
            .bind(execution_id)
            .bind(record.id)
            .bind(attempt)
            .execute(&mut *connection)
            .await?;
            if inserted.rows_affected() != 1 {
                return Err(Error::PermissionDenied(
                    "Retired runtime attempts cannot be reused".into(),
                ));
            }
        }
        let updated = sqlx::query(
            "UPDATE runtime_authority_execution_fence SET attempt = $3 WHERE execution = $1 AND admission = $2",
        )
        .bind(execution_id)
        .bind(record.id)
        .bind(attempt)
        .execute(&mut *connection)
        .await?;
        if updated.rows_affected() != 1 {
            return Err(Error::PermissionDenied(
                "Runtime attempt fence is unavailable".into(),
            ));
        }
        validate_approvals(
            connection,
            &record.authority.owner,
            &record.authority.grants.iter().collect::<Vec<_>>(),
        )
        .await?;
        Ok(RuntimeCredentialBinding {
            execution_id,
            admission_id: record.id,
            sponsor_identity_id: record.authority.owner.sponsor_identity_id,
            action_ref: record.authority.binding.action_ref().into(),
            attempt,
        })
    }

    /// Must be called within the transaction that performs the protected effect.
    /// It never loads the requesting human or the approval issuer.
    pub async fn validate_execution_credential(
        connection: &mut PgConnection,
        credential: &RuntimeCredentialBinding,
    ) -> Result<ValidatedRuntimeAuthority> {
        positive_id(credential.execution_id)?;
        positive_id(credential.admission_id)?;
        SchemaTypeRepository::lock_selection(&mut *connection).await?;
        let query = format!(
            "SELECT {ADMISSION_COLUMNS} FROM runtime_authority_admission \
             WHERE id = $1 AND owner_kind = 'execution' AND owner_id = $2"
        );
        let initial: RuntimeAdmissionRecord = sqlx::query_as(&query)
            .bind(credential.admission_id)
            .bind(credential.execution_id)
            .fetch_optional(&mut *connection)
            .await?
            .ok_or_else(|| {
                Error::PermissionDenied("Execution runtime admission is unavailable".into())
            })?;
        let policy = lock_policy(connection, initial.policy_revision).await?;
        let authority = &initial.authority;
        let status = lock_runtime_owner(
            connection,
            RuntimeOwner::Execution(initial.owner_id),
            authority,
        )
        .await?;
        if status != "running" {
            return Err(Error::PermissionDenied(
                "Execution no longer permits runtime effects".into(),
            ));
        }
        let query = format!("{query} FOR SHARE");
        let record: RuntimeAdmissionRecord = sqlx::query_as(&query)
            .bind(credential.admission_id)
            .bind(credential.execution_id)
            .fetch_optional(&mut *connection)
            .await?
            .ok_or_else(|| {
                Error::PermissionDenied("Execution runtime admission is unavailable".into())
            })?;
        record.validate(&policy)?;
        if record.revoked_at.is_some()
            || record.authority.owner.sponsor_identity_id != credential.sponsor_identity_id
            || record.authority.binding.action_ref() != credential.action_ref
            || !record.authority.has_api_access()
        {
            return Err(Error::PermissionDenied(
                "Execution credential does not match admitted authority".into(),
            ));
        }
        let attempt: Option<Uuid> = sqlx::query_scalar(
            "SELECT attempt FROM runtime_authority_execution_fence \
             WHERE execution = $1 AND admission = $2 FOR SHARE",
        )
        .bind(credential.execution_id)
        .bind(credential.admission_id)
        .fetch_optional(&mut *connection)
        .await?;
        if attempt != Some(credential.attempt) {
            return Err(Error::PermissionDenied(
                "Execution credential references a stale attempt".into(),
            ));
        }
        let approvals = validate_approvals(
            connection,
            &record.authority.owner,
            &record.authority.grants.iter().collect::<Vec<_>>(),
        )
        .await?;
        let standard_owners = if record
            .authority
            .selected_refs
            .iter()
            .any(|reference| reference == "standard")
        {
            let invocation = digest(&serde_json::to_value(&record.authority.binding)?)
                .map_err(|_| Error::validation("Invalid standard invocation binding"))?;
            let owners: Vec<(String, Id)> = sqlx::query_as(
                "SELECT owner_type,owner_id FROM runtime_standard_owner_binding \
                    WHERE policy_revision = $1 AND invocation_digest = $2",
            )
            .bind(record.policy_revision)
            .bind(invocation)
            .fetch_all(&mut *connection)
            .await?;
            if owners.is_empty() {
                return Err(Error::PermissionDenied(
                    "Standard access has no pinned owners".into(),
                ));
            }
            owners.into_iter().collect()
        } else {
            BTreeSet::new()
        };
        Ok(ValidatedRuntimeAuthority {
            record,
            approvals,
            current_attempt: credential.attempt,
            standard_owners,
        })
    }

    pub async fn revoke_policy(connection: &mut PgConnection, id: Id) -> Result<bool> {
        let result = sqlx::query(
            "UPDATE pack_runtime_policy_revision SET revoked_at = COALESCE(revoked_at,NOW()) WHERE id = $1",
        ).bind(id).execute(connection).await?;
        Ok(result.rows_affected() > 0)
    }

    pub async fn revoke_approval(connection: &mut PgConnection, id: Id) -> Result<bool> {
        let result = sqlx::query(
            "UPDATE runtime_external_approval SET revoked_at = COALESCE(revoked_at,NOW()) WHERE id = $1",
        ).bind(id).execute(connection).await?;
        Ok(result.rows_affected() > 0)
    }

    pub async fn revoke_admission(connection: &mut PgConnection, id: Id) -> Result<bool> {
        let result = sqlx::query(
            "UPDATE runtime_authority_admission SET revoked_at = COALESCE(revoked_at,NOW()) WHERE id = $1",
        ).bind(id).execute(connection).await?;
        Ok(result.rows_affected() > 0)
    }
}

async fn lock_sponsor(connection: &mut PgConnection, owner: &PackAuthorityOwner) -> Result<()> {
    let reference: Option<String> =
        sqlx::query_scalar("SELECT ref FROM pack WHERE id = $1 FOR SHARE")
            .bind(owner.pack_id)
            .fetch_optional(&mut *connection)
            .await?;
    if reference.as_deref() != Some(owner.pack_ref.as_str()) {
        return Err(Error::PermissionDenied(
            "Runtime sponsor pack is unavailable".into(),
        ));
    }
    let identity: Option<Id> =
        sqlx::query_scalar("SELECT identity FROM pack_runtime_identity WHERE pack = $1 FOR SHARE")
            .bind(owner.pack_id)
            .fetch_optional(&mut *connection)
            .await?;
    if identity != Some(owner.sponsor_identity_id) {
        return Err(Error::PermissionDenied(
            "Runtime sponsor ownership is unavailable".into(),
        ));
    }
    let sponsor =
        IdentityRepository::find_by_id_for_share(connection, owner.sponsor_identity_id).await?;
    if sponsor.is_none_or(|identity| identity.frozen) {
        return Err(Error::PermissionDenied(
            "System runtime sponsor is suspended".into(),
        ));
    }
    Ok(())
}

async fn lock_policy(connection: &mut PgConnection, id: Id) -> Result<RuntimePolicyRevision> {
    let initial = RuntimeAuthorityRepository::find_policy(connection, id)
        .await?
        .ok_or_else(|| Error::PermissionDenied("Runtime policy is unavailable".into()))?;
    lock_sponsor(connection, &initial.policy.owner).await?;
    let query = format!(
        "SELECT {POLICY_COLUMNS} FROM pack_runtime_policy_revision WHERE id = $1 FOR SHARE"
    );
    let record: RuntimePolicyRevision = sqlx::query_as(&query)
        .bind(id)
        .fetch_optional(connection)
        .await?
        .ok_or_else(|| Error::PermissionDenied("Runtime policy is unavailable".into()))?;
    record.validate()?;
    if record.revoked_at.is_some() {
        return Err(Error::PermissionDenied(
            "Runtime policy has been explicitly revoked".into(),
        ));
    }
    Ok(record)
}

async fn lock_runtime_owner(
    connection: &mut PgConnection,
    owner: RuntimeOwner,
    authority: &SelectedRuntimeAuthority,
) -> Result<String> {
    let (query, id) = match owner {
        RuntimeOwner::Execution(id) => (
            "SELECT action_ref, pack_release, pack_release_digest, status::text, permission_set_refs FROM execution WHERE id = $1 FOR SHARE", id
        ),
        RuntimeOwner::Enforcement(id) => (
            "SELECT executable_snapshot->'executable'->'action'->>'ref', pack_release, pack_release_digest, status::text, NULL::TEXT[] FROM enforcement WHERE id = $1 FOR SHARE", id
        ),
    };
    let row: Option<(
        Option<String>,
        Option<Id>,
        Option<String>,
        String,
        Option<Vec<String>>,
    )> = sqlx::query_as(query)
        .bind(id)
        .fetch_optional(connection)
        .await?;
    let (action, release, digest, status, refs) =
        row.ok_or_else(|| Error::PermissionDenied("Runtime owner is unavailable".into()))?;
    if action.as_deref() != Some(authority.binding.action_ref())
        || release != Some(authority.target_release_id)
        || digest.as_deref() != Some(authority.target_release_digest.as_str())
        || refs.is_some_and(|refs| {
            refs.into_iter()
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect::<Vec<_>>()
                != authority.selected_refs
        })
    {
        return Err(Error::PermissionDenied(
            "Runtime owner executable differs from admitted binding".into(),
        ));
    }
    Ok(status)
}

async fn validate_approvals(
    connection: &mut PgConnection,
    owner: &PackAuthorityOwner,
    grants: &[&AdmittedGrant],
) -> Result<BTreeSet<Id>> {
    let ids: BTreeSet<_> = grants
        .iter()
        .filter_map(|grant| grant.approval_id())
        .collect();
    if ids.is_empty() {
        return Ok(ids);
    }
    let query = format!(
        "SELECT {APPROVAL_COLUMNS} FROM runtime_external_approval WHERE id = ANY($1) ORDER BY id FOR SHARE"
    );
    let records: Vec<ExternalApprovalRecord> = sqlx::query_as(&query)
        .bind(ids.iter().copied().collect::<Vec<_>>())
        .fetch_all(connection)
        .await?;
    let mut approvals = BTreeMap::new();
    for record in records {
        if !record.is_active(Utc::now()) {
            return Err(Error::PermissionDenied(
                "External approval expired or was explicitly revoked".into(),
            ));
        }
        approvals.insert(record.id, record.approval()?);
    }
    for grant in grants {
        if let AdmittedGrant::External { approval_id, grant } = grant {
            if !approvals
                .get(approval_id)
                .is_some_and(|approval| approval.covers(owner, grant))
            {
                return Err(Error::PermissionDenied(
                    "External grant has no covering active approval".into(),
                ));
            }
        }
    }
    Ok(ids)
}

fn approval_body(subject_pack: Id, grants: &[Grant]) -> serde_json::Value {
    json!({"subject_pack": subject_pack, "grants": grants})
}

fn require_digest(value: &impl Serialize, expected: &str) -> Result<()> {
    let actual = digest(&serde_json::to_value(value)?)
        .map_err(|_| Error::validation("Invalid runtime authority metadata"))?;
    if actual != expected {
        return Err(Error::validation("Runtime authority digest mismatch"));
    }
    Ok(())
}
