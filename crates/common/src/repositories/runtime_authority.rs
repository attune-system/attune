//! Persistence and live fences for recorded pack-owned authority.
//!
//! Callers retain the transaction through the authorized effect. Lock order is
//! schema admission, pack, runtime sponsor, policy, runtime owner, approvals.
//! Human identities are checked only when issuing an approval, not at consumption.

pub mod recorded_collections;

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
    concrete_targets: BTreeMap<String, crate::runtime_authority::GrantTargetBindings>,
    effect_fence: ApprovalEffectFence,
}

/// Approval rows remain locked by the source-validation transaction. Row locks
/// fence revocation, but cannot stop wall-clock expiry while waiting on a target.
/// Carry this opaque deadline through every target/version/capture wait and check
/// it immediately before the effect or disclosure. Reacquire per transaction.
#[derive(Debug, Clone)]
pub struct ApprovalEffectFence {
    earliest_expiry: Option<DateTime<Utc>>,
}

impl ApprovalEffectFence {
    pub(crate) fn from_input_approval(expiry: Option<DateTime<Utc>>) -> Result<Self> {
        let fence = Self {
            earliest_expiry: expiry,
        };
        fence.ensure_fresh()?;
        Ok(fence)
    }
    pub fn approval_deadline(&self) -> Option<DateTime<Utc>> {
        self.earliest_expiry
    }
    pub fn ensure_fresh(&self) -> Result<()> {
        self.check_at(Utc::now())
    }

    /// Uses PostgreSQL's current wall clock, never transaction-start `now()`.
    /// This acquires no source, target or approval row lock and cannot invert
    /// their ordering. Call AFTER the actual target/version/capture locks.
    pub async fn ensure_fresh_on(&self, connection: &mut PgConnection) -> Result<()> {
        let now: DateTime<Utc> = sqlx::query_scalar("SELECT clock_timestamp()")
            .fetch_one(connection)
            .await?;
        self.check_at(now)
    }

    pub fn intersect(&self, other: &Self) -> Self {
        Self {
            earliest_expiry: self
                .earliest_expiry
                .into_iter()
                .chain(other.earliest_expiry)
                .min(),
        }
    }

    fn check_at(&self, now: DateTime<Utc>) -> Result<()> {
        if self.earliest_expiry.is_some_and(|expiry| expiry <= now) {
            return Err(Error::PermissionDenied(
                "Validated external approval expired before effect or disclosure".into(),
            ));
        }
        Ok(())
    }
}

struct ValidatedApprovals {
    ids: BTreeSet<Id>,
    fence: ApprovalEffectFence,
}

impl ValidatedRuntimeAuthority {
    pub fn effect_fence(&self) -> &ApprovalEffectFence {
        &self.effect_fence
    }

    pub fn ensure_fresh(&self) -> Result<()> {
        self.effect_fence.ensure_fresh()
    }

    pub async fn ensure_fresh_on(&self, connection: &mut PgConnection) -> Result<()> {
        self.effect_fence.ensure_fresh_on(connection).await
    }

    pub fn cache_read_filters(&self) -> Vec<super::cache::CacheNamespaceGrantFilter> {
        if self.ensure_fresh().is_err() {
            return Vec::new();
        }
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
                    let binding = self
                        .concrete_targets
                        .get(&super::runtime_target_binding::grant_key(grant).ok()?)?;
                    let mut filter =
                        CacheNamespaceGrantFilter::from_recorded_grant(grant.grant(), ownership)?;
                    if let Some(targets) = &binding.targets {
                        filter.pinned_targets = Some(
                            targets
                                .iter()
                                .filter(|target| target.kind == "cache_namespace")
                                .cloned()
                                .collect(),
                        );
                        let ids = targets
                            .iter()
                            .filter(|target| target.kind == "cache_namespace")
                            .map(|target| target.id)
                            .filter(|id| filter.ids.as_ref().is_none_or(|ids| ids.contains(id)))
                            .collect();
                        filter.ids = Some(ids);
                    }
                    filter.pinned_owners = binding
                        .owners
                        .as_ref()
                        .map(|owners| owners.iter().cloned().collect());
                    filter.pinned_packs = binding
                        .packs
                        .as_ref()
                        .map(|packs| packs.iter().copied().collect());
                    Some(filter)
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
        if self.ensure_fresh().is_err() {
            return Vec::new();
        }
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
                    let binding = self
                        .concrete_targets
                        .get(&super::runtime_target_binding::grant_key(grant).ok()?)?;
                    let mut filter = KeyGrantFilter::from_recorded_grant(grant.grant(), ownership)?;
                    if let Some(targets) = &binding.targets {
                        filter.pinned_targets = Some(
                            targets
                                .iter()
                                .filter(|target| target.kind == "key")
                                .cloned()
                                .collect(),
                        );
                        let ids = targets
                            .iter()
                            .filter(|target| target.kind == "key")
                            .map(|target| target.id)
                            .filter(|id| filter.ids.as_ref().is_none_or(|ids| ids.contains(id)))
                            .collect();
                        filter.ids = Some(ids);
                    }
                    filter.pinned_owners = binding
                        .owners
                        .as_ref()
                        .map(|owners| owners.iter().cloned().collect());
                    filter.pinned_packs = binding
                        .packs
                        .as_ref()
                        .map(|packs| packs.iter().copied().collect());
                    Some(filter)
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
        if self.ensure_fresh().is_err() {
            return false;
        }
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
            // An unscoped external Artifact grant is the public baseline, not
            // permission to inspect another owner's private data. A local
            // ownership envelope is itself an explicit structural constraint.
            let private_artifact = target.resource() == Resource::Artifacts
                && context.visibility == Some(crate::models::ArtifactVisibility::Private)
                && matches!(grant, AdmittedGrant::External { .. })
                && grant.grant().constraints.is_none()
                && !(context.owner_type == Some(crate::models::OwnerType::Identity)
                    && context.owner_identity_id == Some(context.identity_id));
            if private_artifact {
                return false;
            }
            let Ok(key) = super::runtime_target_binding::grant_key(grant) else {
                return false;
            };
            let Some(binding) = self.concrete_targets.get(&key) else {
                return false;
            };
            if !super::runtime_target_binding::matches(binding, target) {
                return false;
            }
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
        if self.ensure_fresh().is_err() {
            return false;
        }
        self.record.authority.grants.iter().any(|grant| {
            // Concrete selectors require the opaque repository-resolved
            // target API. A hand-built context cannot prove stable identity.
            let scoped = grant.grant().constraints.as_ref().is_some_and(|scope| {
                scope.refs.is_some() || scope.owner_refs.is_some() || scope.pack_refs.is_some()
            });
            !scoped && grant.allows(&self.approvals, target_pack_id, resource, action, context)
        })
    }
}

pub struct RuntimeAuthorityRepository;

impl RuntimeAuthorityRepository {
    /// Deployment-only lookup of the previous projection. It is not authority
    /// for new runtime admission after the pack's active release changes.
    pub(crate) async fn retained_deployment_policy(
        connection: &mut PgConnection,
        pack_id: Id,
    ) -> Result<Option<RuntimePolicyRevision>> {
        let revision: Option<Id> = sqlx::query_scalar(
            "SELECT revision FROM pack_runtime_policy_current WHERE pack = $1 FOR SHARE",
        )
        .bind(pack_id)
        .fetch_optional(&mut *connection)
        .await?;
        match revision {
            Some(id) => Ok(Self::find_policy(connection, id)
                .await?
                .filter(|policy| policy.revoked_at.is_none())),
            None => Ok(None),
        }
    }

    /// Stable system-managed sponsor creation is serialized independently of
    /// human ownership. No IAM grants are assigned by this operation.
    pub async fn ensure_owner(
        connection: &mut PgConnection,
        pack_id: Id,
    ) -> Result<PackAuthorityOwner> {
        use super::Create;
        positive_id(pack_id)?;
        SchemaTypeRepository::lock_selection(connection).await?;
        let reference: String = sqlx::query_scalar("SELECT ref FROM pack WHERE id = $1 FOR SHARE")
            .bind(pack_id)
            .fetch_optional(&mut *connection)
            .await?
            .ok_or_else(|| Error::validation("Runtime sponsor pack is unavailable"))?;
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended('runtime-authority-owner:' || $1::text, 0))")
            .bind(pack_id).execute(&mut *connection).await?;
        if let Some(owner) = Self::find_owner(connection, pack_id).await? {
            return Ok(owner);
        }
        let identity = IdentityRepository::create(
            &mut *connection,
            super::identity::CreateIdentityInput {
                login: format!("attune-pack:{}:{}", reference, Uuid::new_v4()),
                display_name: Some(format!("Pack: {reference}")),
                password_hash: None,
                attributes: json!({"type":"pack","pack_ref":reference}),
            },
        )
        .await?;
        sqlx::query("INSERT INTO pack_runtime_identity (pack,identity) VALUES ($1,$2)")
            .bind(pack_id)
            .bind(identity.id)
            .execute(connection)
            .await?;
        Ok(PackAuthorityOwner {
            pack_id,
            pack_ref: reference,
            sponsor_identity_id: identity.id,
        })
    }

    /// Readmit future uses of a managed policy edit. Existing admissions keep
    /// their old immutable set bodies and approval IDs. Invoke in the set-write
    /// transaction after ordinary Permissions/Manage authorization.
    pub async fn readmit_permission_set(
        connection: &mut PgConnection,
        reference: &str,
        editor_identity: Id,
    ) -> Result<Vec<RuntimePolicyRevision>> {
        SchemaTypeRepository::lock_reclamation(connection).await?;
        let definition =
            super::identity::PermissionSetRepository::authored_definition(connection, reference)
                .await?
                .ok_or_else(|| Error::validation("Edited policy authoring is unavailable"))?;
        let declaring_pack = super::identity::PermissionSetRepository::find_by_refs_for_share(
            connection,
            &[reference.into()],
        )
        .await?
        .into_iter()
        .next()
        .and_then(|set| set.pack);
        let packs: Vec<Id> = sqlx::query_scalar(
            "SELECT current.pack FROM pack_runtime_policy_current current \
             JOIN pack_runtime_policy_revision revision ON revision.id = current.revision \
             WHERE EXISTS (SELECT 1 FROM jsonb_array_elements(revision.policy->'invocations') invocation \
                 WHERE invocation->'permission_sets' ? $1) ORDER BY current.pack",
        ).bind(reference).fetch_all(&mut *connection).await?;
        let mut revisions = Vec::new();
        for pack in packs {
            let current = Self::current_policy(connection, pack)
                .await?
                .ok_or_else(|| Error::validation("Edited policy admission is unavailable"))?;
            let mut policy = current.policy.0;
            if declaring_pack != Some(policy.owner.pack_id) {
                // Foreign consumers hold fixed external approval and policy,
                // not a subscription to the publisher's mutable definition.
                continue;
            }
            let editor = DelegationAuthority::load_for_share(connection, editor_identity).await?;
            let mut context = AuthorizationContext::new(editor_identity);
            context.target_id = Some(policy.owner.pack_id);
            context.target_ref = Some(policy.owner.pack_ref.clone());
            context.pack_ref = Some(policy.owner.pack_ref.clone());
            context.managing_pack_id = Some(policy.owner.pack_id);
            if !editor.allows(Resource::Packs, Action::Configure, context) {
                return Err(Error::PermissionDenied("Changing an admitted managed policy requires configuration authority on its pack".into()));
            }
            let external = definition
                .grants
                .iter()
                .filter(|grant| {
                    grant.scope.unwrap_or(definition.scope)
                        == crate::runtime_authority::PermissionScope::External
                })
                .cloned()
                .map(|grant| grant.into_external_approval_grant())
                .collect::<Result<Vec<_>>>()?;
            let approvals =
                Self::admit_external_grants(connection, pack, editor_identity, &external).await?;
            let admitted = definition.compile(&policy.owner, &approvals)?;
            for invocation in &mut policy.invocations {
                if invocation.permission_sets.contains_key(reference) {
                    invocation
                        .permission_sets
                        .insert(reference.into(), admitted.clone());
                }
            }
            let revision = Self::capture_policy(connection, &policy).await?;
            Self::activate_policy(connection, revision.id).await?;
            revisions.push(revision);
        }
        Ok(revisions)
    }

    /// Readiness/activation needs the root's FULL admitted ceiling, rather than
    /// a runtime selection's subset. `require_policy` already checks retained
    /// publisher dependencies; explicitly check root approvals as well.
    pub async fn require_deployment_policy(
        connection: &mut PgConnection,
        id: Id,
    ) -> Result<RuntimePolicyRevision> {
        let policy = Self::require_policy(connection, id).await?;
        let grants = policy
            .policy
            .invocations
            .iter()
            .flat_map(|entry| entry.permission_sets.values())
            .flat_map(|set| set.grants.iter())
            .collect::<Vec<_>>();
        let approvals = validate_approvals(connection, &policy.policy.owner, &grants).await?;
        approvals.fence.ensure_fresh_on(connection).await?;
        Ok(policy)
    }
}

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
               AND NOT EXISTS(SELECT 1 FROM runtime_input_only_approval input WHERE input.approval = runtime_external_approval.id) \
               AND (expires_at IS NULL OR expires_at > clock_timestamp()) ORDER BY id FOR SHARE",
        );
        let records: Vec<ExternalApprovalRecord> = sqlx::query_as(&query)
            .bind(subject_pack)
            .fetch_all(&mut *connection)
            .await?;
        let now: DateTime<Utc> = sqlx::query_scalar("SELECT clock_timestamp()")
            .fetch_one(&mut *connection)
            .await?;
        let records = records
            .into_iter()
            .filter(|record| record.is_active(now))
            .collect::<Vec<_>>();
        let mut projected = BTreeMap::<Id, ExternalApproval>::new();
        let mut missing = Vec::new();
        for grant in grants {
            let mut covered = false;
            for record in &records {
                if !record.approval()?.covers(&owner, grant) {
                    continue;
                }
                let admitted = AdmittedGrant::External {
                    approval_id: record.id,
                    grant: grant.clone(),
                };
                let bindings = super::runtime_target_binding::capture_grants(
                    connection,
                    &owner,
                    std::slice::from_ref(&admitted),
                    &BTreeMap::new(),
                )
                .await?;
                let binding = &bindings[&super::runtime_target_binding::grant_key(&admitted)?];
                if !super::runtime_target_binding::approval_covers_binding(
                    connection, record, &owner, grant, binding,
                )
                .await?
                {
                    continue;
                }
                // Return only the requested, identity-bound portion currently
                // proven by this fixed approval. The persisted approval remains
                // unchanged; pure compilation cannot accidentally pick a stale
                // broader approval merely because its numeric ID is lower.
                let approval = projected
                    .entry(record.id)
                    .or_insert_with(|| ExternalApproval {
                        id: record.id,
                        subject_pack_id: subject_pack,
                        grants: Vec::new(),
                    });
                if !approval.grants.contains(grant) {
                    approval.grants.push(grant.clone());
                }
                covered = true;
            }
            if !covered {
                missing.push(grant.clone());
            }
        }
        let mut approvals = projected.into_values().collect::<Vec<_>>();
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
        let input_only: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM runtime_input_only_approval WHERE approval = $1)",
        )
        .bind(previous_id)
        .fetch_one(&mut *connection)
        .await?;
        if input_only {
            sqlx::query("INSERT INTO runtime_input_only_approval(approval) VALUES ($1)")
                .bind(replacement.id)
                .execute(&mut *connection)
                .await?;
            sqlx::query("INSERT INTO runtime_declared_input_approval(declaration,approval) SELECT declaration,$2 FROM runtime_declared_input_approval WHERE approval = $1")
                .bind(previous_id).bind(replacement.id).execute(&mut *connection).await?;
        }
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
        let body_digest =
            digest(&body).map_err(|_| Error::validation("Invalid approval metadata"))?;
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
            .bind(body_digest)
            .bind(expires_at)
            .fetch_one(&mut *connection)
            .await?;
        record.approval()?;
        let owner = Self::ensure_owner(connection, subject_pack).await?;
        let admitted = record
            .grants
            .iter()
            .map(|grant| AdmittedGrant::External {
                approval_id: record.id,
                grant: grant.clone(),
            })
            .collect::<Vec<_>>();
        let bindings = super::runtime_target_binding::capture_grants(
            connection,
            &owner,
            &admitted,
            &BTreeMap::new(),
        )
        .await?;
        for grant in &admitted {
            let key = super::runtime_target_binding::grant_key(grant)?;
            let grant_digest = digest(&serde_json::to_value(grant.grant())?)
                .map_err(|_| Error::validation("Invalid approval grant digest"))?;
            sqlx::query("INSERT INTO runtime_external_approval_target_binding (approval,grant_digest,binding) VALUES ($1,$2,$3) ON CONFLICT DO NOTHING")
                .bind(record.id).bind(grant_digest).bind(Json(&bindings[&key])).execute(&mut *connection).await?;
        }
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
        SchemaTypeRepository::lock_reclamation(&mut *connection).await?;
        lock_sponsor(connection, &policy.owner).await?;
        let mut captured = policy.clone();
        super::runtime_target_binding::capture(connection, &mut captured).await?;
        super::managed_executable::ManagedExecutableRepository::capture_sources(
            connection,
            &mut captured,
        )
        .await?;
        super::workflow_policy::WorkflowPolicyRepository::capture(connection, &mut captured)
            .await?;
        let policy = &captured;
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
            let executable =
                super::managed_executable::ManagedExecutableRepository::invocation_snapshot(
                    connection, invocation,
                )
                .await?
                .executable;
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
                    let source = super::managed_executable::ManagedExecutableRepository::workflow_source_snapshot(connection,policy,workflow_ref).await?;
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
        let approvals = validate_approvals(connection, &policy.owner, &grants).await?;
        super::runtime_target_binding::validate_approval_bindings(connection, policy).await?;
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
        for invocation in &policy.invocations {
            if let Some(pin) = &invocation.foreign_workflow_policy {
                if pin.revision >= record.id {
                    return Err(Error::validation(
                        "Workflow policy dependencies must precede their consumer revision",
                    ));
                }
                sqlx::query("INSERT INTO runtime_workflow_policy_dependency (policy_revision,dependency) VALUES ($1,$2) ON CONFLICT DO NOTHING")
                    .bind(record.id).bind(pin.revision).execute(&mut *connection).await?;
            }
        }
        let metadata_revisions = policy
            .invocations
            .iter()
            .filter_map(|entry| entry.metadata_revision)
            .collect::<BTreeSet<_>>();
        for metadata in metadata_revisions {
            sqlx::query("INSERT INTO runtime_authority_metadata_dependency (policy_revision,metadata_revision) VALUES ($1,$2) ON CONFLICT DO NOTHING")
                .bind(record.id).bind(metadata).execute(&mut *connection).await?;
        }
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
        .execute(&mut *connection)
        .await?;
        super::declared_input::DeclaredInputRepository::capture(connection, &record).await?;
        // The exact policy ID is known now. Publish all dispatch source bodies
        // before activation or any rule/sensor producer can observe this policy.
        super::managed_component_readmission::ManagedComponentReadmission::capture_deployment_dispatch(
            connection, &record,
        ).await?;
        let (_, dependencies) = lock_policy_with_deadline(connection, record.id).await?;
        approvals
            .fence
            .intersect(&dependencies)
            .ensure_fresh_on(connection)
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

    /// Live policy fence also validates every retained foreign graph dependency.
    /// No current-pointer/ordinary enablement check is applied to dependencies.
    pub async fn require_policy(
        connection: &mut PgConnection,
        id: Id,
    ) -> Result<RuntimePolicyRevision> {
        SchemaTypeRepository::lock_selection(connection).await?;
        lock_policy(connection, id).await
    }

    /// Retained origin/dependency validation, not permission for a new runtime
    /// effect. Parent completion or execution-row retention is not revocation.
    pub(crate) async fn require_retained_admission(
        connection: &mut PgConnection,
        id: Id,
    ) -> Result<RuntimeAdmissionRecord> {
        let (record, _) = Self::retained_admission_with_fence(connection, id).await?;
        Ok(record)
    }

    /// Source-stage capture for bounded worker completion/native/historical
    /// permits. This checks retained admission dependencies without granting a
    /// Running execution credential or terminal process API access.
    pub async fn retained_admission_fence(
        connection: &mut PgConnection,
        id: Id,
    ) -> Result<ApprovalEffectFence> {
        let (_, fence) = Self::retained_admission_with_fence(connection, id).await?;
        Ok(fence)
    }

    pub(crate) async fn retained_admission_with_fence(
        connection: &mut PgConnection,
        id: Id,
    ) -> Result<(RuntimeAdmissionRecord, ApprovalEffectFence)> {
        SchemaTypeRepository::lock_selection(connection).await?;
        let query =
            format!("SELECT {ADMISSION_COLUMNS} FROM runtime_authority_admission WHERE id = $1");
        let record: RuntimeAdmissionRecord = sqlx::query_as(&query)
            .bind(id)
            .fetch_optional(&mut *connection)
            .await?
            .ok_or_else(|| Error::PermissionDenied("Retained admission is unavailable".into()))?;
        let (policy, policy_fence) =
            lock_policy_with_deadline(connection, record.policy_revision).await?;
        let record: RuntimeAdmissionRecord = sqlx::query_as(&format!("{query} FOR SHARE"))
            .bind(id)
            .fetch_one(&mut *connection)
            .await?;
        record.validate(&policy)?;
        if record.revoked_at.is_some() {
            return Err(Error::PermissionDenied(
                "Retained admission is revoked".into(),
            ));
        }
        let approvals = validate_approvals(
            connection,
            &record.authority.owner,
            &record.authority.grants.iter().collect::<Vec<_>>(),
        )
        .await?;
        let dependencies = validate_admission_dependencies(connection, &record).await?;
        let fence = policy_fence
            .intersect(&approvals.fence)
            .intersect(&dependencies);
        fence.ensure_fresh_on(connection).await?;
        Ok((record, fence))
    }

    /// B captures this while selecting a task, before waiting on native targets.
    /// It validates only the selected refs plus already pinned graph dependencies;
    /// neither current permission bodies nor human IAM supply extra grants.
    pub async fn selected_effect_fence(
        connection: &mut PgConnection,
        authority: &SelectedRuntimeAuthority,
    ) -> Result<ApprovalEffectFence> {
        SchemaTypeRepository::lock_selection(connection).await?;
        let (policy, policy_fence) =
            lock_policy_with_deadline(connection, authority.policy_id).await?;
        authority.validate_against(&policy.policy)?;
        let approvals = validate_approvals(
            connection,
            &authority.owner,
            &authority.grants.iter().collect::<Vec<_>>(),
        )
        .await?;
        let mut fence = policy_fence.intersect(&approvals.fence);
        if let crate::runtime_authority::RequestAttribution::ParentExecution { execution_id } =
            &authority.attribution
        {
            let parent = Self::execution_admission(connection, *execution_id)
                .await?
                .ok_or_else(|| {
                    Error::PermissionDenied(
                        "Selected effect parent admission is unavailable".into(),
                    )
                })?;
            let parent_fence = Self::retained_admission_fence(connection, parent.id).await?;
            fence = fence.intersect(&parent_fence);
        }
        fence.ensure_fresh_on(connection).await?;
        Ok(fence)
    }

    /// B attaches this in the retry creation transaction after recording the new
    /// owner. It retains the original admission's independent revocation fence.
    pub async fn retain_inherited_admission(
        connection: &mut PgConnection,
        source: Id,
        target: Id,
    ) -> Result<()> {
        let source = Self::require_retained_admission(connection, source).await?;
        let target = Self::require_retained_admission(connection, target).await?;
        let mut expected = source.authority.0.clone();
        expected.attempt = target.authority.attempt;
        if source.id >= target.id || expected != target.authority.0 {
            return Err(Error::PermissionDenied(
                "Retry admission does not preserve its original selection".into(),
            ));
        }
        sqlx::query("INSERT INTO runtime_admission_dependency (admission,dependency,dependency_kind) VALUES ($1,$2,'retry') ON CONFLICT DO NOTHING")
            .bind(target.id).bind(source.id).execute(&mut *connection).await?;
        validate_admission_dependencies(connection, &target)
            .await?
            .ensure_fresh()
    }

    pub async fn activate_policy(connection: &mut PgConnection, id: Id) -> Result<()> {
        SchemaTypeRepository::lock_reclamation(&mut *connection).await?;
        let (policy, policy_fence) = lock_policy_with_deadline(connection, id).await?;
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
        let previous: Option<Id> = sqlx::query_scalar(
            "SELECT revision FROM pack_runtime_policy_current WHERE pack = $1 FOR SHARE",
        )
        .bind(policy.policy.owner.pack_id)
        .fetch_optional(&mut *connection)
        .await?;
        if previous != Some(id) {
            let pending:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM work_queue_item item JOIN work_queue queue ON queue.id = item.queue WHERE queue.pack = $1 AND queue.management_origin = 'pack' AND item.status IN ('queued','leased','retry'))")
                .bind(policy.policy.owner.pack_id).fetch_one(&mut *connection).await?;
            if pending {
                return Err(Error::validation(
                    "Drain accepted queue items before changing the admitted pack policy revision",
                ));
            }
        }
        let grants = policy
            .policy
            .invocations
            .iter()
            .flat_map(|invocation| invocation.permission_sets.values())
            .flat_map(|set| set.grants.iter())
            .collect::<Vec<_>>();
        let approvals = validate_approvals(connection, &policy.policy.owner, &grants).await?;
        sqlx::query(
            "INSERT INTO pack_runtime_policy_current (pack,revision) VALUES ($1,$2) \
             ON CONFLICT (pack) DO UPDATE SET revision = EXCLUDED.revision",
        )
        .bind(policy.policy.owner.pack_id)
        .bind(id)
        .execute(&mut *connection)
        .await?;
        policy_fence
            .intersect(&approvals.fence)
            .ensure_fresh_on(connection)
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
        let (policy, policy_fence) =
            lock_policy_with_deadline(connection, authority.policy_id).await?;
        authority.validate_against(&policy.policy)?;
        lock_runtime_owner(connection, owner, authority).await?;
        let admitted_executable =
            super::managed_executable::ManagedExecutableRepository::resolve_invocation(
                connection,
                &policy.policy,
                &authority.binding,
            )
            .await?;
        let snapshot_query = match owner {
            RuntimeOwner::Execution(_) => {
                "SELECT executable_snapshot,action FROM execution WHERE id = $1 FOR SHARE"
            }
            RuntimeOwner::Enforcement(_) => {
                "SELECT executable_snapshot,NULL::BIGINT FROM enforcement WHERE id = $1 FOR SHARE"
            }
        };
        let (actual, action_id): (
            Option<Json<crate::models::ExecutionExecutableSnapshot>>,
            Option<Id>,
        ) = sqlx::query_as(snapshot_query)
            .bind(owner_id)
            .fetch_one(&mut *connection)
            .await?;
        let actual = actual.ok_or_else(|| {
            Error::PermissionDenied("Runtime owner has no immutable executable metadata".into())
        })?;
        if action_id.is_some_and(|id| id != admitted_executable.executable.action.id)
            || serde_json::to_value(&actual.executable)?
                != serde_json::to_value(&admitted_executable.executable)?
        {
            return Err(Error::PermissionDenied(
                "Runtime owner metadata differs from its immutable admitted executable".into(),
            ));
        }
        let approvals = validate_approvals(
            connection,
            &authority.owner,
            &authority.grants.iter().collect::<Vec<_>>(),
        )
        .await?;
        let body_digest = digest(&serde_json::to_value(authority)?)
            .map_err(|_| Error::validation("Invalid selected runtime authority"))?;
        let inserted = sqlx::query(
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
        .await?.rows_affected() == 1;
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
        if let crate::runtime_authority::RequestAttribution::ParentExecution { execution_id } =
            &record.authority.attribution
        {
            let dependency:Id = sqlx::query_scalar("SELECT id FROM runtime_authority_admission WHERE owner_kind = 'execution' AND owner_id = $1 FOR SHARE")
                .bind(execution_id).fetch_optional(&mut *connection).await?
                .ok_or_else(|| Error::PermissionDenied("Parent admission dependency is unavailable".into()))?;
            if dependency >= record.id {
                return Err(Error::validation("Runtime admission dependency cycle"));
            }
            sqlx::query("INSERT INTO runtime_admission_dependency (admission,dependency) VALUES ($1,$2) ON CONFLICT DO NOTHING")
                .bind(record.id).bind(dependency).execute(&mut *connection).await?;
        }
        let dependency_fence = validate_admission_dependencies_inner(
            connection,
            &record,
            inserted.then_some(record.id),
        )
        .await?;
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
            .execute(&mut *connection)
            .await?;
        }
        policy_fence
            .intersect(&approvals.fence)
            .intersect(&dependency_fence)
            .ensure_fresh_on(connection)
            .await?;
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
        let (policy, policy_fence) =
            lock_policy_with_deadline(connection, initial.policy_revision).await?;
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
        let dependency_fence = validate_admission_dependencies(connection, &record).await?;
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
        let approvals = validate_approvals(
            connection,
            &record.authority.owner,
            &record.authority.grants.iter().collect::<Vec<_>>(),
        )
        .await?;
        policy_fence
            .intersect(&approvals.fence)
            .intersect(&dependency_fence)
            .ensure_fresh_on(connection)
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
        let (policy, policy_fence) =
            lock_policy_with_deadline(connection, initial.policy_revision).await?;
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
        let dependency_fence = validate_admission_dependencies(connection, &record).await?;
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
        let effect_fence = policy_fence
            .intersect(&dependency_fence)
            .intersect(&approvals.fence);
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
        effect_fence.ensure_fresh_on(connection).await?;
        Ok(ValidatedRuntimeAuthority {
            record,
            approvals: approvals.ids,
            current_attempt: credential.attempt,
            standard_owners,
            concrete_targets: policy.policy.concrete_targets.clone(),
            effect_fence,
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

pub(crate) async fn lock_sponsor(
    connection: &mut PgConnection,
    owner: &PackAuthorityOwner,
) -> Result<()> {
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
    let (policy, fence) = lock_policy_with_deadline(connection, id).await?;
    fence.ensure_fresh()?;
    Ok(policy)
}

async fn lock_policy_with_deadline(
    connection: &mut PgConnection,
    id: Id,
) -> Result<(RuntimePolicyRevision, ApprovalEffectFence)> {
    let root = lock_policy_one(connection, id).await?;
    let mut fence = ApprovalEffectFence {
        earliest_expiry: None,
    };
    let mut pending = vec![root.clone()];
    let mut visited = BTreeSet::new();
    while let Some(record) = pending.pop() {
        if !visited.insert(record.id) {
            continue;
        }
        if visited.len() > 256 {
            return Err(Error::PermissionDenied(
                "Workflow policy dependency graph exceeds admission bound".into(),
            ));
        }
        let expected = record
            .policy
            .invocations
            .iter()
            .filter_map(|entry| entry.foreign_workflow_policy.as_ref())
            .map(|pin| pin.revision)
            .collect::<BTreeSet<_>>();
        let actual:Vec<Id> = sqlx::query_scalar("SELECT dependency FROM runtime_workflow_policy_dependency WHERE policy_revision = $1 ORDER BY dependency FOR SHARE")
            .bind(record.id).fetch_all(&mut *connection).await?;
        if actual.into_iter().collect::<BTreeSet<_>>() != expected {
            return Err(Error::PermissionDenied(
                "Workflow policy dependency evidence is incomplete".into(),
            ));
        }
        for pin in record
            .policy
            .invocations
            .iter()
            .filter_map(|entry| entry.foreign_workflow_policy.as_ref())
        {
            if pin.revision >= record.id {
                return Err(Error::validation("Workflow policy dependency cycle"));
            }
            let dependency = lock_policy_one(connection, pin.revision).await?;
            pin.verify(&dependency)?;
            let grants = dependency
                .policy
                .invocations
                .iter()
                .flat_map(|entry| entry.permission_sets.values())
                .flat_map(|set| set.grants.iter())
                .collect::<Vec<_>>();
            let approvals =
                validate_approvals(connection, &dependency.policy.owner, &grants).await?;
            fence = fence.intersect(&approvals.fence);
            pending.push(dependency);
        }
    }
    fence.ensure_fresh()?;
    Ok((root, fence))
}

async fn lock_policy_one(connection: &mut PgConnection, id: Id) -> Result<RuntimePolicyRevision> {
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

async fn validate_admission_dependencies(
    connection: &mut PgConnection,
    root: &RuntimeAdmissionRecord,
) -> Result<ApprovalEffectFence> {
    validate_admission_dependencies_inner(connection, root, None).await
}

async fn validate_admission_dependencies_inner(
    connection: &mut PgConnection,
    root: &RuntimeAdmissionRecord,
    staged_record: Option<Id>,
) -> Result<ApprovalEffectFence> {
    let mut fence = ApprovalEffectFence {
        earliest_expiry: None,
    };
    let mut pending = vec![root.clone()];
    let mut visited = BTreeSet::new();
    while let Some(record) = pending.pop() {
        if !visited.insert(record.id) {
            continue;
        }
        if visited.len() > 256 {
            return Err(Error::PermissionDenied(
                "Runtime admission lineage exceeds bound".into(),
            ));
        }
        if staged_record != Some(record.id) {
            for accepted in
                super::accepted_batch_dependency::AcceptedBatchDependencyRepository::validate(
                    connection, &record,
                )
                .await?
            {
                let (policy, deadline) =
                    lock_policy_with_deadline(connection, accepted.policy_id).await?;
                accepted.validate_against(&policy.policy)?;
                let approvals = validate_approvals(
                    connection,
                    &accepted.owner,
                    &accepted.grants.iter().collect::<Vec<_>>(),
                )
                .await?;
                fence = fence.intersect(&deadline).intersect(&approvals.fence);
            }
        }
        let dependencies:Vec<(Id,String)> = sqlx::query_as("SELECT dependency,dependency_kind FROM runtime_admission_dependency WHERE admission = $1 ORDER BY dependency FOR SHARE")
            .bind(record.id).fetch_all(&mut *connection).await?;
        let parent = match &record.authority.attribution {
            crate::runtime_authority::RequestAttribution::ParentExecution { execution_id } => {
                Some(*execution_id)
            }
            _ => None,
        };
        if dependencies
            .iter()
            .filter(|(_, kind)| kind == "parent")
            .count()
            != usize::from(parent.is_some())
        {
            return Err(Error::PermissionDenied(
                "Runtime admission lineage evidence is incomplete".into(),
            ));
        }
        for (id, kind) in dependencies {
            if id >= record.id {
                return Err(Error::validation("Runtime admission lineage cycle"));
            }
            let query = format!(
                "SELECT {ADMISSION_COLUMNS} FROM runtime_authority_admission WHERE id = $1"
            );
            let dependency: RuntimeAdmissionRecord = sqlx::query_as(&query)
                .bind(id)
                .fetch_one(&mut *connection)
                .await?;
            let (policy, policy_fence) =
                lock_policy_with_deadline(connection, dependency.policy_revision).await?;
            let dependency: RuntimeAdmissionRecord = sqlx::query_as(&format!("{query} FOR SHARE"))
                .bind(id)
                .fetch_one(&mut *connection)
                .await?;
            if dependency.revoked_at.is_some()
                || kind == "parent"
                    && (dependency.owner_kind != "execution" || Some(dependency.owner_id) != parent)
            {
                return Err(Error::PermissionDenied(
                    "Originating runtime admission is unavailable or revoked".into(),
                ));
            }
            fence = fence.intersect(&policy_fence);
            dependency.validate(&policy)?;
            if kind == "retry" {
                let mut expected = dependency.authority.0.clone();
                expected.attempt = record.authority.attempt;
                if expected != record.authority.0 {
                    return Err(Error::PermissionDenied(
                        "Retry dependency selection mismatch".into(),
                    ));
                }
            } else if kind == "accepted_item" {
                if !matches!(
                    record.authority.binding,
                    crate::runtime_authority::InvocationBinding::Action { .. }
                ) || !matches!(
                    record.authority.attribution,
                    crate::runtime_authority::RequestAttribution::QueueBatch { .. }
                ) {
                    return Err(Error::PermissionDenied(
                        "Accepted batch dependency has no Action batch attribution".into(),
                    ));
                }
            } else if kind != "parent" {
                return Err(Error::validation("Unknown admission dependency purpose"));
            }
            let approvals = validate_approvals(
                connection,
                &dependency.authority.owner,
                &dependency.authority.grants.iter().collect::<Vec<_>>(),
            )
            .await?;
            fence = fence.intersect(&approvals.fence);
            pending.push(dependency);
        }
    }
    fence.ensure_fresh()?;
    Ok(fence)
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
    type RetainedExecutionRow = (
        Option<String>,
        Option<Id>,
        Option<String>,
        String,
        Option<Vec<String>>,
    );
    let row: Option<RetainedExecutionRow> = sqlx::query_as(query)
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
) -> Result<ValidatedApprovals> {
    let ids: BTreeSet<_> = grants
        .iter()
        .filter_map(|grant| grant.approval_id())
        .collect();
    if ids.is_empty() {
        return Ok(ValidatedApprovals {
            ids,
            fence: ApprovalEffectFence {
                earliest_expiry: None,
            },
        });
    }
    let input_only: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM runtime_input_only_approval WHERE approval = ANY($1))",
    )
    .bind(ids.iter().copied().collect::<Vec<_>>())
    .fetch_one(&mut *connection)
    .await?;
    if input_only {
        return Err(Error::PermissionDenied(
            "Declared-input approvals cannot authorize runtime API grants".into(),
        ));
    }
    let query = format!(
        "SELECT {APPROVAL_COLUMNS} FROM runtime_external_approval WHERE id = ANY($1) ORDER BY id FOR SHARE"
    );
    let records: Vec<ExternalApprovalRecord> = sqlx::query_as(&query)
        .bind(ids.iter().copied().collect::<Vec<_>>())
        .fetch_all(connection)
        .await?;
    let mut approvals = BTreeMap::new();
    let mut earliest_expiry = None;
    for record in records {
        if !record.is_active(Utc::now()) {
            return Err(Error::PermissionDenied(
                "External approval expired or was explicitly revoked".into(),
            ));
        }
        earliest_expiry = earliest_expiry.into_iter().chain(record.expires_at).min();
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
    let fence = ApprovalEffectFence { earliest_expiry };
    fence.ensure_fresh()?;
    Ok(ValidatedApprovals { ids, fence })
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

#[cfg(test)]
mod expiry_fence_tests {
    use super::*;

    #[test]
    fn deadline_intersection_keeps_earliest_selected_or_dependency_expiry() {
        let now = Utc::now();
        let direct = ApprovalEffectFence {
            earliest_expiry: Some(now + chrono::Duration::seconds(30)),
        };
        let graph = ApprovalEffectFence {
            earliest_expiry: Some(now + chrono::Duration::seconds(10)),
        };
        let parent = ApprovalEffectFence {
            earliest_expiry: Some(now + chrono::Duration::seconds(20)),
        };
        let combined = direct.intersect(&graph).intersect(&parent);
        assert_eq!(combined.approval_deadline(), graph.approval_deadline());
        assert!(combined
            .check_at(now + chrono::Duration::seconds(9))
            .is_ok());
        assert!(combined
            .check_at(now + chrono::Duration::seconds(10))
            .is_err());
        assert!(combined
            .check_at(now + chrono::Duration::seconds(11))
            .is_err());
    }

    #[test]
    fn no_expiry_never_discards_another_fences_deadline() {
        let unbounded = ApprovalEffectFence {
            earliest_expiry: None,
        };
        let expired = ApprovalEffectFence {
            earliest_expiry: Some(Utc::now() - chrono::Duration::seconds(1)),
        };
        assert!(unbounded.ensure_fresh().is_ok());
        assert!(unbounded.intersect(&expired).ensure_fresh().is_err());
        assert!(expired.intersect(&unbounded).ensure_fresh().is_err());
        assert_eq!(unbounded.intersect(&unbounded).approval_deadline(), None);
    }
}
