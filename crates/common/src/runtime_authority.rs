//! Release-owned runtime policy, separate from human IAM and concurrency admission.
//!
//! These types are not a registrar bypass. Deployment and token consumers must
//! adopt recorded admission together before enabling pack-owned API authority.

use std::collections::{BTreeMap, BTreeSet, HashSet};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    auth::jwt::STANDARD_EXECUTION_ACCESS_REF,
    models::{Id, OwnerType},
    rbac::{can_delegate_grants, Action, AuthorizationContext, Grant, GrantConstraints, Resource},
    schema::RefValidator,
    Error, Result,
};

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionScope {
    #[default]
    Pack,
    External,
}

/// Canonical authored grant. A grant may override its set's default scope.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PermissionGrantDefinition {
    pub resource: Resource,
    pub actions: Vec<Action>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub constraints: Option<GrantConstraints>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<PermissionScope>,
}

impl PermissionGrantDefinition {
    /// Approval endpoints are inherently external. Do not silently accept a
    /// local-scope authoring declaration as an external capability grant.
    pub fn into_external_approval_grant(self) -> Result<Grant> {
        if self.scope == Some(PermissionScope::Pack) {
            return Err(Error::validation(
                "External approvals cannot contain pack-local scope declarations",
            ));
        }
        let grant = self.grant();
        validate_stable_grant(&grant)?;
        Ok(grant)
    }

    fn grant(&self) -> Grant {
        Grant {
            resource: self.resource,
            actions: self.actions.clone(),
            constraints: self.constraints.clone(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PermissionSetDefinition {
    #[serde(rename = "ref")]
    pub reference: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default)]
    pub scope: PermissionScope,
    pub grants: Vec<PermissionGrantDefinition>,
}

/// Supplied from the candidate graph or retained managing-pack records, not a ref prefix.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackAuthorityOwner {
    pub pack_id: Id,
    pub pack_ref: String,
    pub sponsor_identity_id: Id,
}

impl PackAuthorityOwner {
    pub fn validate(&self) -> Result<()> {
        positive_id(self.pack_id)?;
        positive_id(self.sponsor_identity_id)?;
        RefValidator::validate_pack_ref(&self.pack_ref)
    }
}

/// The explicit ownership envelope is retained alongside the effective grant.
/// Consumers must not flatten `PackOwned` to a naked IAM grant.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum AdmittedGrant {
    PackOwned { pack_id: Id, grant: Grant },
    External { approval_id: Id, grant: Grant },
}

impl AdmittedGrant {
    pub fn grant(&self) -> &Grant {
        match self {
            Self::PackOwned { grant, .. } | Self::External { grant, .. } => grant,
        }
    }

    pub fn approval_id(&self) -> Option<Id> {
        match self {
            Self::PackOwned { .. } => None,
            Self::External { approval_id, .. } => Some(*approval_id),
        }
    }

    /// `target_pack_id` must come from repository-owned target resolution.
    /// An external grant is usable only after its approval has passed live validation.
    pub fn allows(
        &self,
        validated_approvals: &BTreeSet<Id>,
        target_pack_id: Option<Id>,
        resource: Resource,
        action: Action,
        context: &AuthorizationContext,
    ) -> bool {
        let admitted = match self {
            Self::PackOwned { pack_id, .. } => {
                target_pack_id == Some(*pack_id)
                    && (!matches!(
                        resource,
                        Resource::Keys | Resource::Artifacts | Resource::Caches
                    ) || matches!(
                        context.owner_type,
                        Some(OwnerType::Pack | OwnerType::Action | OwnerType::Sensor)
                    ))
            }
            Self::External { approval_id, .. } => validated_approvals.contains(approval_id),
        };
        admitted && self.grant().allows(resource, action, context)
    }
}

/// Immutable approval data. Issuer activity is deliberately not a runtime input.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExternalApproval {
    pub id: Id,
    pub subject_pack_id: Id,
    pub grants: Vec<Grant>,
}

impl ExternalApproval {
    pub fn validate(&self) -> Result<()> {
        positive_id(self.id)?;
        positive_id(self.subject_pack_id)?;
        validate_external_approval_grants(&self.grants)
    }

    pub(crate) fn covers(&self, owner: &PackAuthorityOwner, grant: &Grant) -> bool {
        self.subject_pack_id == owner.pack_id
            && can_delegate_grants(
                &self.grants,
                std::slice::from_ref(grant),
                &AuthorizationContext::new(owner.sponsor_identity_id),
            )
    }
}

pub fn validate_external_approval_grants(grants: &[Grant]) -> Result<()> {
    if grants.is_empty() || grants.len() > 256 {
        return Err(Error::validation(
            "External approvals require 1 to 256 grants",
        ));
    }
    for grant in grants {
        validate_stable_grant(grant)?;
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdmittedPermissionSet {
    pub reference: String,
    pub grants: Vec<AdmittedGrant>,
}

impl AdmittedPermissionSet {
    /// A referenced foreign set never inherits a local envelope merely because
    /// its ref was selected. Record approvals for its fixed effective grants.
    pub fn external(
        reference: &str,
        owner: &PackAuthorityOwner,
        grants: &[Grant],
        approvals: &[ExternalApproval],
    ) -> Result<Self> {
        owner.validate()?;
        RefValidator::validate_component_ref(reference)?;
        for approval in approvals {
            approval.validate()?;
        }
        let mut admitted = Vec::with_capacity(grants.len());
        for grant in grants {
            validate_stable_grant(grant)?;
            let approval = approvals
                .iter()
                .filter(|approval| approval.covers(owner, grant))
                .min_by_key(|approval| approval.id)
                .ok_or_else(|| {
                    Error::PermissionDenied(
                        "Referenced external policy requires a covering approval".into(),
                    )
                })?;
            admitted.push(AdmittedGrant::External {
                approval_id: approval.id,
                grant: grant.clone(),
            });
        }
        Ok(Self {
            reference: reference.into(),
            grants: admitted,
        })
    }
}

impl PermissionSetDefinition {
    pub fn compile(
        &self,
        owner: &PackAuthorityOwner,
        approvals: &[ExternalApproval],
    ) -> Result<AdmittedPermissionSet> {
        owner.validate()?;
        validate_owned_ref(&self.reference, &owner.pack_ref)?;
        for approval in approvals {
            approval.validate()?;
        }
        let mut grants = Vec::with_capacity(self.grants.len());
        for authored in &self.grants {
            let grant = authored.grant();
            validate_stable_grant(&grant)?;
            if grant.constraints.as_ref().is_some_and(|scope| {
                scope.ids.is_some() || scope.pack_refs.is_some() && scope.refs.is_some()
            }) {
                return Err(Error::validation(
                    "Authored permissions use pack or component refs, not database IDs or both ref scopes",
                ));
            }
            match authored.scope.unwrap_or(self.scope) {
                PermissionScope::Pack => {
                    if !pack_owned_resource(grant.resource) {
                        return Err(Error::validation(
                            "System administration requires explicit external authority",
                        ));
                    }
                    if grant.constraints.as_ref().is_some_and(|scope| {
                        scope.pack_refs.as_ref().is_some_and(|refs| {
                            refs.iter().any(|reference| reference != &owner.pack_ref)
                        })
                    }) {
                        return Err(Error::validation(
                            "A local permission cannot select another pack",
                        ));
                    }
                    if matches!(
                        grant.resource,
                        Resource::Keys | Resource::Artifacts | Resource::Caches
                    ) && grant
                        .constraints
                        .as_ref()
                        .and_then(|scope| scope.owner_types.as_ref())
                        .is_some_and(|owners| {
                            owners.iter().any(|owner| {
                                !matches!(
                                    owner,
                                    OwnerType::Pack | OwnerType::Action | OwnerType::Sensor
                                )
                            })
                        })
                    {
                        return Err(Error::validation(
                            "System and human-owned resources require explicit external authority",
                        ));
                    }
                    grants.push(AdmittedGrant::PackOwned {
                        pack_id: owner.pack_id,
                        grant,
                    });
                }
                PermissionScope::External => {
                    let approval = approvals
                        .iter()
                        .filter(|approval| approval.covers(owner, &grant))
                        .min_by_key(|approval| approval.id)
                        .ok_or_else(|| {
                            Error::PermissionDenied(
                                "External runtime authority requires a covering approval".into(),
                            )
                        })?;
                    grants.push(AdmittedGrant::External {
                        approval_id: approval.id,
                        grant,
                    });
                }
            }
        }
        Ok(AdmittedPermissionSet {
            reference: self.reference.clone(),
            grants,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum InvocationBinding {
    Action {
        action_ref: String,
    },
    WorkflowTask {
        workflow_ref: String,
        task_path: String,
        action_ref: String,
    },
    Rule {
        rule_ref: String,
        action_ref: String,
    },
    Queue {
        queue_ref: String,
        action_ref: String,
    },
}

impl InvocationBinding {
    pub fn action_ref(&self) -> &str {
        match self {
            Self::Action { action_ref }
            | Self::WorkflowTask { action_ref, .. }
            | Self::Rule { action_ref, .. }
            | Self::Queue { action_ref, .. } => action_ref,
        }
    }

    fn validate(&self, pack_ref: &str) -> Result<()> {
        RefValidator::validate_component_ref(self.action_ref())?;
        match self {
            Self::Action { action_ref } => validate_owned_ref(action_ref, pack_ref),
            Self::WorkflowTask {
                workflow_ref,
                task_path,
                ..
            } => {
                validate_owned_ref(workflow_ref, pack_ref)?;
                if !task_path.starts_with("/tasks/") || task_path.contains("//") {
                    return Err(Error::validation("Workflow authority requires a task path"));
                }
                Ok(())
            }
            Self::Rule { rule_ref, .. } => validate_owned_ref(rule_ref, pack_ref),
            Self::Queue { queue_ref, .. } => {
                RefValidator::validate_work_queue_ref(queue_ref)?;
                if queue_ref.split('.').next() != Some(pack_ref) {
                    return Err(Error::validation("Queue binding belongs to another pack"));
                }
                Ok(())
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InvocationPolicy {
    pub binding: InvocationBinding,
    pub target_release_id: Id,
    pub target_release_digest: String,
    pub permission_sets: BTreeMap<String, AdmittedPermissionSet>,
    pub standard_access: bool,
}

impl InvocationPolicy {
    /// Assemble one static invocation from pinned code and already-admitted sets.
    /// Omission uses the pinned action's defaults; an explicit empty list disables
    /// API access. This does not consult live action metadata or personal IAM.
    pub fn from_executable(
        binding: InvocationBinding,
        executable: &crate::models::ExecutionExecutableSnapshot,
        requested_refs: Option<&[String]>,
        admitted_sets: &BTreeMap<String, AdmittedPermissionSet>,
    ) -> Result<Self> {
        positive_id(executable.release.id)?;
        validate_digest(&executable.release.digest)?;
        if binding.action_ref() != executable.executable.action.r#ref {
            return Err(Error::validation(
                "Invocation binding does not match the pinned action",
            ));
        }
        let refs: BTreeSet<_> = requested_refs
            .unwrap_or(
                &executable
                    .executable
                    .action
                    .default_execution_permission_set_refs,
            )
            .iter()
            .collect();
        let mut permission_sets = BTreeMap::new();
        let mut standard_access = false;
        for reference in refs {
            if reference == STANDARD_EXECUTION_ACCESS_REF {
                standard_access = true;
                continue;
            }
            RefValidator::validate_component_ref(reference)?;
            let set = admitted_sets.get(reference).ok_or_else(|| {
                Error::PermissionDenied(
                    "Invocation references a permission set without recorded admission".into(),
                )
            })?;
            if set.reference != *reference {
                return Err(Error::validation(
                    "Admitted permission set reference mismatch",
                ));
            }
            permission_sets.insert(reference.clone(), set.clone());
        }
        Ok(Self {
            binding,
            target_release_id: executable.release.id,
            target_release_digest: executable.release.digest.clone(),
            permission_sets,
            standard_access,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimePolicy {
    pub owner: PackAuthorityOwner,
    pub release_id: Id,
    pub release_digest: String,
    pub invocations: Vec<InvocationPolicy>,
}

impl RuntimePolicy {
    pub fn validate(&self) -> Result<()> {
        self.owner.validate()?;
        positive_id(self.release_id)?;
        validate_digest(&self.release_digest)?;
        let mut bindings = HashSet::new();
        for invocation in &self.invocations {
            invocation.binding.validate(&self.owner.pack_ref)?;
            positive_id(invocation.target_release_id)?;
            validate_digest(&invocation.target_release_digest)?;
            if !bindings.insert(&invocation.binding) {
                return Err(Error::validation("Duplicate runtime invocation binding"));
            }
            for (reference, set) in &invocation.permission_sets {
                RefValidator::validate_component_ref(reference)?;
                if reference != &set.reference {
                    return Err(Error::validation(
                        "Permission set binding reference mismatch",
                    ));
                }
                for grant in &set.grants {
                    validate_stable_grant(grant.grant())?;
                    match grant {
                        AdmittedGrant::PackOwned { pack_id, grant } => {
                            if *pack_id != self.owner.pack_id
                                || !pack_owned_resource(grant.resource)
                            {
                                return Err(Error::validation("Runtime grant owner mismatch"));
                            }
                        }
                        AdmittedGrant::External { approval_id, .. } => positive_id(*approval_id)?,
                    }
                }
            }
        }
        Ok(())
    }

    pub fn select(
        &self,
        policy_id: Id,
        binding: &InvocationBinding,
        requested_refs: &[String],
        attribution: RequestAttribution,
    ) -> Result<SelectedRuntimeAuthority> {
        self.validate()?;
        positive_id(policy_id)?;
        attribution.validate()?;
        let invocation = self
            .invocations
            .iter()
            .find(|invocation| &invocation.binding == binding)
            .ok_or_else(|| {
                Error::PermissionDenied("Invocation has no admitted runtime policy".into())
            })?;
        let selected_refs: Vec<_> = requested_refs
            .iter()
            .cloned()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        let mut grants = Vec::new();
        for reference in &selected_refs {
            if reference == STANDARD_EXECUTION_ACCESS_REF {
                if !invocation.standard_access {
                    return Err(Error::PermissionDenied(
                        "Standard access was not admitted".into(),
                    ));
                }
            } else {
                let set = invocation.permission_sets.get(reference).ok_or_else(|| {
                    Error::PermissionDenied(
                        "Requested runtime permission set was not admitted".into(),
                    )
                })?;
                grants.extend(set.grants.iter().cloned());
            }
        }
        Ok(SelectedRuntimeAuthority {
            policy_id,
            binding: binding.clone(),
            owner: self.owner.clone(),
            target_release_id: invocation.target_release_id,
            target_release_digest: invocation.target_release_digest.clone(),
            attempt: Uuid::new_v4(),
            selected_refs,
            grants,
            attribution,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HumanAttribution {
    pub identity_id: Id,
    pub login: String,
}

/// Historical attribution, never a lookup of current human activity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum RequestAttribution {
    Human {
        actor: HumanAttribution,
    },
    Rule {
        rule_ref: String,
    },
    QueueBatch {
        queue_ref: String,
        items: BTreeMap<Id, Option<HumanAttribution>>,
    },
    ParentExecution {
        execution_id: Id,
    },
}

impl RequestAttribution {
    fn validate(&self) -> Result<()> {
        let validate_human = |actor: &HumanAttribution| {
            positive_id(actor.identity_id)?;
            if actor.login.trim().is_empty() {
                return Err(Error::validation("Requester attribution requires a login"));
            }
            Ok(())
        };
        match self {
            Self::Human { actor } => validate_human(actor),
            Self::Rule { rule_ref } => RefValidator::validate_component_ref(rule_ref),
            Self::QueueBatch { queue_ref, items } => {
                RefValidator::validate_work_queue_ref(queue_ref)?;
                if items.is_empty() {
                    return Err(Error::validation("Batch attribution requires items"));
                }
                for (id, actor) in items {
                    positive_id(*id)?;
                    if let Some(actor) = actor {
                        validate_human(actor)?;
                    }
                }
                Ok(())
            }
            Self::ParentExecution { execution_id } => positive_id(*execution_id),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SelectedRuntimeAuthority {
    pub policy_id: Id,
    pub binding: InvocationBinding,
    pub owner: PackAuthorityOwner,
    pub target_release_id: Id,
    pub target_release_digest: String,
    pub attempt: Uuid,
    pub selected_refs: Vec<String>,
    pub grants: Vec<AdmittedGrant>,
    pub attribution: RequestAttribution,
}

impl SelectedRuntimeAuthority {
    /// Arbitrary pack code may print any plaintext fetched through its SDK.
    /// Suppress service-log mirroring before the process makes its first read.
    pub fn may_read_key_material(&self) -> bool {
        self.selected_refs
            .iter()
            .any(|reference| reference == STANDARD_EXECUTION_ACCESS_REF)
            || self.grants.iter().any(|grant| {
                grant.grant().resource == Resource::Keys
                    && grant.grant().actions.contains(&Action::Read)
            })
    }

    pub fn has_api_access(&self) -> bool {
        !self.selected_refs.is_empty()
    }

    /// Reconstructs the selection from immutable policy, detecting modified stored grants.
    pub fn validate_against(&self, policy: &RuntimePolicy) -> Result<()> {
        if self.attempt.is_nil() || self.owner != policy.owner {
            return Err(Error::validation(
                "Runtime authority owner or attempt mismatch",
            ));
        }
        let selected = policy.select(
            self.policy_id,
            &self.binding,
            &self.selected_refs,
            self.attribution.clone(),
        )?;
        if selected.selected_refs != self.selected_refs
            || selected.grants != self.grants
            || selected.target_release_id != self.target_release_id
            || selected.target_release_digest != self.target_release_digest
        {
            return Err(Error::validation(
                "Stored runtime authority differs from admitted policy",
            ));
        }
        Ok(())
    }

    pub fn approval_ids(&self) -> BTreeSet<Id> {
        self.grants
            .iter()
            .filter_map(AdmittedGrant::approval_id)
            .collect()
    }
}

pub(crate) fn positive_id(id: Id) -> Result<()> {
    if id <= 0 {
        return Err(Error::validation(
            "Runtime authority requires positive database IDs",
        ));
    }
    Ok(())
}

fn validate_owned_ref(reference: &str, pack_ref: &str) -> Result<()> {
    RefValidator::validate_component_ref(reference)?;
    if reference.split('.').next() != Some(pack_ref) {
        return Err(Error::validation(
            "Managed policy reference belongs to another pack",
        ));
    }
    Ok(())
}

fn validate_digest(digest: &str) -> Result<()> {
    if digest.len() != 64
        || !digest
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(Error::validation(
            "Runtime policy requires a SHA-256 release digest",
        ));
    }
    Ok(())
}

pub(crate) fn validate_stable_grant(grant: &Grant) -> Result<()> {
    if grant.actions.is_empty() {
        return Err(Error::validation(
            "Runtime permission grants require actions",
        ));
    }
    if let Some(scope) = &grant.constraints {
        for references in [&scope.pack_refs, &scope.refs, &scope.owner_refs] {
            if references.as_ref().is_some_and(|references| {
                references.is_empty()
                    || references
                        .iter()
                        .any(|reference| reference.trim().is_empty())
            }) {
                return Err(Error::validation(
                    "Runtime permission scopes must be nonempty",
                ));
            }
        }
        if scope
            .ids
            .as_ref()
            .is_some_and(|ids| ids.is_empty() || ids.iter().any(|id| *id <= 0))
            || scope.owner_types.as_ref().is_some_and(Vec::is_empty)
            || scope.visibility.as_ref().is_some_and(Vec::is_empty)
        {
            return Err(Error::validation(
                "Runtime permission scopes must be valid and nonempty",
            ));
        }
        if matches!(scope.owner, Some(crate::rbac::OwnerConstraint::SelfOnly))
            || matches!(
                scope.execution_scope,
                Some(
                    crate::rbac::ExecutionScopeConstraint::SelfOnly
                        | crate::rbac::ExecutionScopeConstraint::Descendants
                )
            )
            || scope
                .attributes
                .as_ref()
                .is_some_and(|attributes| !attributes.is_empty())
        {
            return Err(Error::validation(
                "Admitted runtime grants require explicit resource scope, not human-relative conditions",
            ));
        }
        if grant.resource == Resource::Caches {
            crate::rbac::validate_cache_grant_constraints(scope).map_err(Error::validation)?;
        }
    }
    Ok(())
}

fn pack_owned_resource(resource: Resource) -> bool {
    !matches!(
        resource,
        Resource::Workers
            | Resource::Retention
            | Resource::Identities
            | Resource::Permissions
            | Resource::AuditLog
    )
}

#[cfg(test)]
#[path = "runtime_authority_tests.rs"]
mod tests;
