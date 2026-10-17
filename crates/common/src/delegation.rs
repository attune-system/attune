//! Identity-bound delegation proof shared by registration and execution consumers.

use std::collections::{BTreeSet, HashMap};

use sqlx::{PgConnection, PgPool};

use crate::{
    auth::jwt::STANDARD_EXECUTION_ACCESS_REF,
    models::{Id, PermissionSet},
    rbac::{can_delegate_grants, Action, AuthorizationContext, Grant, Resource},
    repositories::identity::{IdentityRepository, PermissionSetRepository},
    Error, Result,
};

#[derive(Clone)]
pub struct DelegationAuthority {
    pub identity_id: Id,
    context: AuthorizationContext,
    grants: Vec<Grant>,
}

impl DelegationAuthority {
    pub fn new(
        identity_id: Id,
        attributes: HashMap<String, serde_json::Value>,
        grants: Vec<Grant>,
    ) -> Self {
        let mut context = AuthorizationContext::new(identity_id);
        context.identity_attributes = attributes;
        Self {
            identity_id,
            context,
            grants,
        }
    }

    pub async fn load(pool: &PgPool, identity_id: Id) -> Result<Self> {
        let row = IdentityRepository::authorization_state(pool, identity_id)
            .await?
            .filter(|row| !row.frozen)
            .ok_or_else(|| {
                Error::AuthenticationFailed("Delegating identity is not active".into())
            })?;
        let attributes = match row.attributes {
            serde_json::Value::Object(attributes) => attributes.into_iter().collect(),
            _ => HashMap::new(),
        };
        Ok(Self::new(row.identity_id, attributes, row.grants.0))
    }

    pub async fn load_for_share(conn: &mut PgConnection, identity_id: Id) -> Result<Self> {
        let identity = IdentityRepository::find_by_id_for_share(conn, identity_id)
            .await?
            .filter(|identity| !identity.frozen)
            .ok_or_else(|| {
                Error::AuthenticationFailed("Delegating identity is not active".into())
            })?;
        let mut sets =
            PermissionSetRepository::find_by_identity_for_share(&mut *conn, identity_id).await?;
        sets.extend(
            PermissionSetRepository::find_by_identity_roles_for_share(&mut *conn, identity_id)
                .await?,
        );
        let attributes = match identity.attributes {
            serde_json::Value::Object(attributes) => attributes.into_iter().collect(),
            _ => HashMap::new(),
        };
        Ok(Self::new(identity_id, attributes, grants_from_sets(sets)?))
    }

    pub fn covers(&self, grants: &[Grant]) -> bool {
        can_delegate_grants(&self.grants, grants, &self.context)
    }

    pub fn allows(
        &self,
        resource: Resource,
        action: Action,
        mut context: AuthorizationContext,
    ) -> bool {
        context.identity_id = self.identity_id;
        context.identity_attributes = self.context.identity_attributes.clone();
        self.grants
            .iter()
            .any(|grant| grant.allows(resource, action, &context))
    }

    /// Purpose-specific generic input proof. Preserve private-artifact baseline
    /// semantics while using the complete, repository-resolved target context.
    pub fn allows_input_origin_target(
        &self,
        target: &crate::repositories::runtime_target::ResolvedRuntimeTarget,
        action: Action,
    ) -> bool {
        let mut context = target.context().clone();
        context.identity_id = self.identity_id;
        context.identity_attributes = self.context.identity_attributes.clone();
        let private = target.resource() == Resource::Artifacts
            && context.visibility == Some(crate::models::ArtifactVisibility::Private)
            && !(context.owner_type == Some(crate::models::OwnerType::Identity)
                && context.owner_identity_id == Some(self.identity_id));
        self.grants.iter().any(|grant| {
            let explicit = grant.constraints.as_ref().is_some_and(|scope| {
                scope.managing_pack_id.is_some()
                    || scope.owner.is_some()
                    || scope.owner_types.is_some()
                    || scope.owner_refs.is_some()
                    || scope.refs.is_some()
                    || scope.ids.is_some()
                    || scope.pack_refs.is_some()
                    || scope.visibility.is_some()
                    || scope.execution_scope.is_some()
            });
            (!private || explicit) && grant.allows(target.resource(), action, &context)
        })
    }

    pub fn manages_permissions(&self) -> bool {
        self.covers(&[Grant {
            resource: Resource::Permissions,
            actions: vec![Action::Manage],
            constraints: None,
        }])
    }

    pub fn key_allows(&self, origin: &crate::secret_provenance::KeyOrigin, action: Action) -> bool {
        crate::key_access::origin_action_allowed(&self.grants, &self.context, origin, action)
    }

    pub fn key_allows_verified(
        &self,
        origin: &crate::secret_provenance::KeyOrigin,
        action: Action,
        ownership: &crate::repositories::iam_ownership::VerifiedKeyOrigins,
    ) -> bool {
        crate::key_access::origin_action_allowed_verified(
            &self.grants,
            &self.context,
            origin,
            action,
            ownership,
        )
    }

    pub fn needs_key_ownership(&self) -> bool {
        self.grants.iter().any(|grant| {
            grant.resource == Resource::Keys
                && grant
                    .constraints
                    .as_ref()
                    .is_some_and(|scope| scope.managing_pack_id.is_some())
        })
    }

    pub async fn require_refs(&self, pool: &PgPool, refs: &[String]) -> Result<()> {
        let refs = named_refs(refs)?;
        if refs.is_empty() {
            return Ok(());
        }
        let sets = PermissionSetRepository::find_by_refs(pool, &refs).await?;
        require_resolved(&refs, &sets)?;
        self.require_grants(&grants_from_sets(sets)?)
    }

    pub async fn require_refs_for_share(
        &self,
        conn: &mut PgConnection,
        refs: &[String],
    ) -> Result<()> {
        let refs = named_refs(refs)?;
        if refs.is_empty() {
            return Ok(());
        }
        let sets = PermissionSetRepository::find_by_refs_for_share(conn, &refs).await?;
        require_resolved(&refs, &sets)?;
        self.require_grants(&grants_from_sets(sets)?)
    }

    pub fn require_grants(&self, requested: &[Grant]) -> Result<()> {
        if !self.covers(requested) {
            return Err(Error::PermissionDenied(
                "Execution permission grants exceed the delegating identity's authority".into(),
            ));
        }
        Ok(())
    }
}

pub fn named_refs(refs: &[String]) -> Result<Vec<String>> {
    let mut named = BTreeSet::new();
    for reference in refs {
        if reference.trim().is_empty() {
            return Err(Error::validation("Permission set refs cannot be empty"));
        }
        if reference != STANDARD_EXECUTION_ACCESS_REF {
            named.insert(reference.clone());
        }
    }
    Ok(named.into_iter().collect())
}

pub fn grants_from_sets(sets: Vec<PermissionSet>) -> Result<Vec<Grant>> {
    let mut grants = Vec::new();
    let mut seen = BTreeSet::new();
    for set in sets {
        if seen.insert(set.id) {
            grants.extend(serde_json::from_value::<Vec<Grant>>(set.grants)?);
        }
    }
    Ok(grants)
}

fn require_resolved(refs: &[String], sets: &[PermissionSet]) -> Result<()> {
    if refs.len() != sets.len() {
        return Err(Error::PermissionDenied(
            "One or more execution permission sets are unavailable".into(),
        ));
    }
    Ok(())
}

pub async fn require_execution_refs(
    pool: &PgPool,
    identity_id: Option<Id>,
    refs: &[String],
) -> Result<()> {
    if refs.is_empty() {
        return Ok(());
    }
    let identity_id = identity_id.ok_or_else(|| {
        Error::PermissionDenied(
            "Execution API access requires an explicit executor identity".into(),
        )
    })?;
    DelegationAuthority::load(pool, identity_id)
        .await?
        .require_refs(pool, refs)
        .await
}

pub async fn require_execution_refs_for_share(
    conn: &mut PgConnection,
    identity_id: Option<Id>,
    refs: &[String],
) -> Result<()> {
    if refs.is_empty() {
        return Ok(());
    }
    let identity_id = identity_id.ok_or_else(|| {
        Error::PermissionDenied(
            "Execution API access requires an explicit executor identity".into(),
        )
    })?;
    DelegationAuthority::load_for_share(conn, identity_id)
        .await?
        .require_refs_for_share(conn, refs)
        .await
}

pub fn standard_sensor_cache_grants(sensor_ref: &str, pack_ref: &str) -> Vec<Grant> {
    [
        (crate::models::OwnerType::Sensor, sensor_ref),
        (crate::models::OwnerType::Pack, pack_ref),
    ]
    .into_iter()
    .map(|(owner_type, owner_ref)| Grant {
        resource: Resource::Caches,
        actions: vec![Action::Read],
        constraints: Some(crate::rbac::GrantConstraints {
            owner_types: Some(vec![owner_type]),
            owner_refs: Some(vec![owner_ref.into()]),
            ..Default::default()
        }),
    })
    .collect()
}
