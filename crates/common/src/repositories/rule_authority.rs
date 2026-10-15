//! Deployment-owned rule input authority, separate from execution API access.
use super::{
    action::ActionRepository,
    identity::{
        CreateIdentityInput, CreatePermissionAssignmentInput, IdentityRepository,
        PermissionAssignmentRepository, PermissionSetRepository,
    },
    pack::PackRepository,
    rule::RuleRepository,
    Create, FindById,
};
use crate::{
    delegation::DelegationAuthority,
    rbac::{Action, Grant, Resource},
    value_schema::digest,
    Error, Result,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::{Executor, PgConnection, Postgres};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuleInputAuthority {
    pub rule_id: i64,
    pub rule_ref: String,
    pub pack_id: i64,
    pub pack_ref: String,
    pub release_id: i64,
    pub identity_id: i64,
    pub input_permission_set_refs: Vec<String>,
    pub input_grants: Vec<Grant>,
    pub execution_permission_set_refs: Vec<String>,
}

impl RuleInputAuthority {
    pub async fn input_authority(
        &self,
        connection: &mut PgConnection,
    ) -> Result<DelegationAuthority> {
        if self.input_grants.iter().any(|grant| {
            !matches!(grant.resource, Resource::Keys | Resource::Packs)
                || grant.actions.is_empty()
                || grant
                    .actions
                    .iter()
                    .any(|action| !matches!(action, Action::Read | Action::Decrypt))
        }) {
            return Err(Error::validation("Invalid deployment input authority"));
        }
        let identity = IdentityRepository::find_by_id(&mut *connection, self.identity_id)
            .await?
            .filter(|identity| !identity.frozen)
            .ok_or_else(|| {
                Error::PermissionDenied("Pack runtime identity is unavailable".into())
            })?;
        let owner: Option<i64> =
            sqlx::query_scalar("SELECT identity FROM pack_runtime_identity WHERE pack = $1")
                .bind(self.pack_id)
                .fetch_optional(&mut *connection)
                .await?;
        if owner != Some(identity.id)
            || PackRepository::find_by_id(&mut *connection, self.pack_id)
                .await?
                .is_none_or(|pack| pack.r#ref != self.pack_ref)
        {
            return Err(Error::PermissionDenied(
                "Pack input authority owner is unavailable".into(),
            ));
        }
        let attributes = std::collections::HashMap::from([
            ("type".into(), json!("pack")),
            ("pack_ref".into(), json!(self.pack_ref)),
        ]);
        Ok(DelegationAuthority::new(
            identity.id,
            attributes,
            self.input_grants.clone(),
        ))
    }
}

pub struct RuleAuthorityRepository;
impl RuleAuthorityRepository {
    pub async fn copy_execution(
        connection: &mut PgConnection,
        source: i64,
        target: i64,
    ) -> Result<()> {
        if let Some(authority) = Self::runtime(connection, "execution", source).await? {
            Self::attach_runtime(connection, "execution", target, &authority).await?;
        }
        Ok(())
    }
    pub async fn require_mutation_authority(
        connection: &mut PgConnection,
        rule: &crate::models::Rule,
        caller: &DelegationAuthority,
    ) -> Result<()> {
        if rule.is_adhoc {
            return Ok(());
        }
        let snapshot = Self::current(connection, rule).await?.ok_or_else(|| {
            Error::PermissionDenied(
                "Deployed rule authority is unavailable; redeploy its pack".into(),
            )
        })?;
        if !caller.covers(&snapshot.input_grants) {
            return Err(Error::PermissionDenied(
                "Changing deployed rule inputs requires authority over its captured input grants"
                    .into(),
            ));
        }
        Ok(())
    }

    pub async fn refresh_execution_refs(
        connection: &mut PgConnection,
        rule: &crate::models::Rule,
        caller: &DelegationAuthority,
    ) -> Result<()> {
        if rule.is_adhoc {
            return Ok(());
        }
        Self::require_mutation_authority(connection, rule, caller).await?;
        let mut snapshot = Self::current(connection, rule)
            .await?
            .ok_or_else(|| Error::validation("Deployed rule authority is unavailable"))?;
        let action = ActionRepository::find_by_id(
            &mut *connection,
            rule.action
                .ok_or_else(|| Error::validation("Rule action is unavailable"))?,
        )
        .await?
        .ok_or_else(|| Error::validation("Rule action is unavailable"))?;
        let refs = rule
            .permission_set_refs
            .clone()
            .unwrap_or(action.default_execution_permission_set_refs);
        caller.require_refs_for_share(connection, &refs).await?;
        let named = refs
            .iter()
            .filter(|reference| reference.as_str() != "standard")
            .cloned()
            .collect::<Vec<_>>();
        for set in PermissionSetRepository::find_by_refs(&mut *connection, &named).await? {
            let exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM permission_assignment WHERE identity = $1 AND permset = $2)")
                .bind(snapshot.identity_id).bind(set.id).fetch_one(&mut *connection).await?;
            if !exists {
                PermissionAssignmentRepository::create(
                    &mut *connection,
                    CreatePermissionAssignmentInput {
                        identity: snapshot.identity_id,
                        permset: set.id,
                    },
                )
                .await?;
            }
        }
        snapshot.execution_permission_set_refs = refs;
        let body = serde_json::to_value(&snapshot)?;
        let hash = digest(&body).map_err(|_| Error::validation("Invalid rule authority"))?;
        let revision: i64 = sqlx::query_scalar("INSERT INTO rule_runtime_authority_revision (rule,authority,authority_digest) VALUES ($1,$2,$3) RETURNING id")
            .bind(rule.id).bind(body).bind(hash).fetch_one(&mut *connection).await?;
        sqlx::query("UPDATE rule_runtime_authority_current SET revision = $1 WHERE rule = $2")
            .bind(revision)
            .bind(rule.id)
            .execute(connection)
            .await?;
        Ok(())
    }
    pub async fn execution_input_authority(
        connection: &mut PgConnection,
        execution: i64,
        identity: Option<i64>,
    ) -> Result<Option<DelegationAuthority>> {
        if let Some(snapshot) = Self::runtime(connection, "execution", execution).await? {
            if identity != Some(snapshot.identity_id) {
                return Err(Error::validation("Input authority identity mismatch"));
            }
            return Ok(Some(snapshot.input_authority(connection).await?));
        }
        let Some(identity) = identity else {
            return Ok(None);
        };
        let system_owned: bool = sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM pack_runtime_identity WHERE identity = $1)",
        )
        .bind(identity)
        .fetch_one(&mut *connection)
        .await?;
        if system_owned {
            return Err(Error::PermissionDenied(
                "Pack execution input authority is unavailable".into(),
            ));
        }
        Ok(Some(
            DelegationAuthority::load_for_share(connection, identity).await?,
        ))
    }
    pub async fn attach_runtime(
        connection: &mut PgConnection,
        kind: &str,
        id: i64,
        authority: &RuleInputAuthority,
    ) -> Result<()> {
        authority.input_authority(connection).await?;
        if kind == "enforcement" {
            let owner: Option<(Option<i64>, String)> =
                sqlx::query_as("SELECT rule,rule_ref FROM enforcement WHERE id = $1 FOR KEY SHARE")
                    .bind(id)
                    .fetch_optional(&mut *connection)
                    .await?;
            if owner.is_none_or(|(rule, reference)| {
                reference != authority.rule_ref
                    || rule.is_some_and(|rule| rule != authority.rule_id)
            }) {
                return Err(Error::validation(
                    "Enforcement input authority owner mismatch",
                ));
            }
        } else if kind == "execution" {
            let owner: Option<Option<i64>> =
                sqlx::query_scalar("SELECT executor FROM execution WHERE id = $1 FOR KEY SHARE")
                    .bind(id)
                    .fetch_optional(&mut *connection)
                    .await?;
            if owner != Some(Some(authority.identity_id)) {
                return Err(Error::validation(
                    "Execution input authority owner mismatch",
                ));
            }
        } else {
            return Err(Error::validation("Invalid input authority owner kind"));
        }
        let body = serde_json::to_value(authority)?;
        let hash = digest(&body).map_err(|_| Error::validation("Invalid input authority"))?;
        sqlx::query("INSERT INTO runtime_input_authority (owner_kind,owner_id,authority,authority_digest) VALUES ($1,$2,$3,$4) ON CONFLICT DO NOTHING")
            .bind(kind).bind(id).bind(body).bind(&hash).execute(&mut *connection).await?;
        let stored: String = sqlx::query_scalar("SELECT authority_digest FROM runtime_input_authority WHERE owner_kind = $1 AND owner_id = $2")
            .bind(kind).bind(id).fetch_one(connection).await?;
        if stored != hash {
            return Err(Error::validation(
                "Runtime input authority cannot be rebound",
            ));
        }
        Ok(())
    }

    pub async fn runtime(
        connection: &mut PgConnection,
        kind: &str,
        id: i64,
    ) -> Result<Option<RuleInputAuthority>> {
        let row: Option<(serde_json::Value,String)> = sqlx::query_as("SELECT authority,authority_digest FROM runtime_input_authority WHERE owner_kind = $1 AND owner_id = $2")
            .bind(kind).bind(id).fetch_optional(&mut *connection).await?;
        let Some((body, hash)) = row else {
            return Ok(None);
        };
        if digest(&body).map_err(|_| Error::validation("Invalid runtime input authority"))? != hash
        {
            return Err(Error::validation("Runtime input authority digest mismatch"));
        }
        let authority: RuleInputAuthority = serde_json::from_value(body)?;
        if kind == "execution" {
            let executor: Option<Option<i64>> =
                sqlx::query_scalar("SELECT executor FROM execution WHERE id = $1")
                    .bind(id)
                    .fetch_optional(&mut *connection)
                    .await?;
            if executor != Some(Some(authority.identity_id)) {
                return Err(Error::validation(
                    "Runtime input authority executor mismatch",
                ));
            }
        }
        Ok(Some(authority))
    }
    pub async fn set_input_refs(
        connection: &mut PgConnection,
        rule: i64,
        refs: &[String],
    ) -> Result<()> {
        if refs
            .iter()
            .any(|reference| reference.trim().is_empty() || reference == "standard")
        {
            return Err(Error::validation(
                "Rule input authority requires explicit named permission sets",
            ));
        }
        sqlx::query("INSERT INTO rule_input_permission_refs (rule,refs) VALUES ($1,$2) ON CONFLICT (rule) DO UPDATE SET refs = EXCLUDED.refs")
            .bind(rule).bind(refs).execute(connection).await?;
        Ok(())
    }

    pub async fn input_refs<'e, E>(executor: E, rule: i64) -> Result<Vec<String>>
    where
        E: Executor<'e, Database = Postgres>,
    {
        Ok(
            sqlx::query_scalar("SELECT refs FROM rule_input_permission_refs WHERE rule = $1")
                .bind(rule)
                .fetch_optional(executor)
                .await?
                .unwrap_or_default(),
        )
    }

    pub async fn capture_deployment(connection: &mut PgConnection, pack_id: i64) -> Result<()> {
        let pack = PackRepository::find_by_id(&mut *connection, pack_id)
            .await?
            .ok_or_else(|| Error::validation("Rule authority pack is unavailable"))?;
        let release_id = pack
            .active_release
            .ok_or_else(|| Error::validation("Rule authority requires a deployed release"))?;
        let identity_id = match sqlx::query_scalar::<_, i64>(
            "SELECT identity FROM pack_runtime_identity WHERE pack = $1",
        )
        .bind(pack_id)
        .fetch_optional(&mut *connection)
        .await?
        {
            Some(identity) => identity,
            None => {
                let identity = IdentityRepository::create(
                    &mut *connection,
                    CreateIdentityInput {
                        login: format!("attune-pack:{}:{}", pack.r#ref, uuid::Uuid::new_v4()),
                        display_name: Some(format!("Pack: {}", pack.r#ref)),
                        password_hash: None,
                        attributes: json!({"type":"pack","pack_ref":pack.r#ref}),
                    },
                )
                .await?;
                sqlx::query("INSERT INTO pack_runtime_identity (pack,identity) VALUES ($1,$2)")
                    .bind(pack_id)
                    .bind(identity.id)
                    .execute(&mut *connection)
                    .await?;
                identity.id
            }
        };
        for rule in RuleRepository::find_by_pack(&mut *connection, pack_id)
            .await?
            .into_iter()
            .filter(|rule| !rule.is_adhoc)
        {
            let refs = Self::input_refs(&mut *connection, rule.id).await?;
            let sets = PermissionSetRepository::find_by_refs(&mut *connection, &refs).await?;
            if sets.len() != refs.iter().collect::<std::collections::BTreeSet<_>>().len() {
                return Err(Error::validation(
                    "Rule input permission set is unavailable",
                ));
            }
            let mut input_grants = Vec::new();
            for set in sets {
                let grants: Vec<Grant> = serde_json::from_value(set.grants)?;
                for mut grant in grants {
                    if !matches!(grant.resource, Resource::Keys | Resource::Packs) {
                        continue;
                    }
                    grant
                        .actions
                        .retain(|action| matches!(action, Action::Read | Action::Decrypt));
                    if !grant.actions.is_empty() {
                        input_grants.push(grant);
                    }
                }
            }
            let action = ActionRepository::find_by_id(
                &mut *connection,
                rule.action
                    .ok_or_else(|| Error::validation("Rule action is unavailable"))?,
            )
            .await?
            .ok_or_else(|| Error::validation("Rule action is unavailable"))?;
            let execution_refs = rule
                .permission_set_refs
                .clone()
                .unwrap_or(action.default_execution_permission_set_refs);
            let named: Vec<String> = execution_refs
                .iter()
                .filter(|reference| reference.as_str() != "standard")
                .cloned()
                .collect();
            for set in PermissionSetRepository::find_by_refs(&mut *connection, &named).await? {
                let exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM permission_assignment WHERE identity = $1 AND permset = $2)")
                    .bind(identity_id).bind(set.id).fetch_one(&mut *connection).await?;
                if !exists {
                    PermissionAssignmentRepository::create(
                        &mut *connection,
                        CreatePermissionAssignmentInput {
                            identity: identity_id,
                            permset: set.id,
                        },
                    )
                    .await?;
                }
            }
            let authority = RuleInputAuthority {
                rule_id: rule.id,
                rule_ref: rule.r#ref,
                pack_id,
                pack_ref: pack.r#ref.clone(),
                release_id,
                identity_id,
                input_permission_set_refs: refs,
                input_grants,
                execution_permission_set_refs: execution_refs,
            };
            let body = serde_json::to_value(&authority)?;
            let hash =
                digest(&body).map_err(|_| Error::validation("Invalid rule input authority"))?;
            let revision: i64 = sqlx::query_scalar("INSERT INTO rule_runtime_authority_revision (rule,authority,authority_digest) VALUES ($1,$2,$3) RETURNING id")
                .bind(rule.id).bind(body).bind(hash).fetch_one(&mut *connection).await?;
            sqlx::query("INSERT INTO rule_runtime_authority_current (rule,revision) VALUES ($1,$2) ON CONFLICT (rule) DO UPDATE SET revision = EXCLUDED.revision")
                .bind(rule.id).bind(revision).execute(&mut *connection).await?;
            sqlx::query("UPDATE rule SET owner_identity = $1 WHERE id = $2")
                .bind(identity_id)
                .bind(rule.id)
                .execute(&mut *connection)
                .await?;
        }
        Ok(())
    }

    pub async fn current(
        connection: &mut PgConnection,
        rule: &crate::models::Rule,
    ) -> Result<Option<RuleInputAuthority>> {
        let row: Option<(serde_json::Value,String)> = sqlx::query_as("SELECT r.authority,r.authority_digest FROM rule_runtime_authority_revision r JOIN rule_runtime_authority_current c ON c.revision = r.id WHERE c.rule = $1 AND r.rule = c.rule")
            .bind(rule.id).fetch_optional(connection).await?;
        let Some((body, hash)) = row else {
            return Ok(None);
        };
        if digest(&body).map_err(|_| Error::validation("Invalid rule authority"))? != hash {
            return Err(Error::validation("Rule authority digest mismatch"));
        }
        let authority: RuleInputAuthority = serde_json::from_value(body)?;
        if authority.rule_id != rule.id
            || authority.rule_ref != rule.r#ref
            || authority.pack_id != rule.pack
            || authority.pack_ref != rule.pack_ref
            || rule.owner_identity != Some(authority.identity_id)
        {
            return Err(Error::validation("Rule input authority owner mismatch"));
        }
        Ok(Some(authority))
    }
}
