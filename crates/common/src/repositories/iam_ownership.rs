//! Structural ownership expressions shared by IAM list compilers.
//!
//! Arguments are repository-owned SQL aliases, never request values. Keep these
//! proofs aligned with `RuntimeTargetRepository`; association and ref prefixes
//! are not managing ownership.

use std::collections::HashMap;

use crate::{models::Id, Result};

use super::runtime_target::ComponentTarget;

/// Columns supplied by the owning repository, never by a request. Missing
/// dimensions have the same meaning as an unpopulated AuthorizationContext.
pub struct IamTargetSql<'a> {
    pub id: &'a str,
    pub reference: &'a str,
    pub pack_ref: &'a str,
    pub managing_pack: &'a str,
    pub owner_identity: Option<&'a str>,
    pub execution_owner: Option<&'a str>,
    pub ancestor_identities: Option<&'a str>,
}

#[derive(Debug, Clone)]
pub struct ComponentReadFilter {
    pub identity: Id,
    pub attributes: HashMap<String, serde_json::Value>,
    pub grants: Vec<crate::rbac::Grant>,
}

/// `table` is a repository-owned identifier, never request input.
pub fn push_component_read(
    query: &mut sqlx::QueryBuilder<'_, sqlx::Postgres>,
    filter: &ComponentReadFilter,
    table: &str,
    resource: crate::rbac::Resource,
) {
    push_iam_grants(
        query,
        &filter.grants,
        resource,
        crate::rbac::Action::Read,
        filter.identity,
        &filter.attributes,
        &IamTargetSql {
            id: &format!("{table}.id"),
            reference: &format!("{table}.ref"),
            pack_ref: &format!("{table}.pack_ref"),
            managing_pack: &component_pack(table),
            owner_identity: None,
            execution_owner: None,
            ancestor_identities: None,
        },
    );
}

pub fn push_iam_grants(
    query: &mut sqlx::QueryBuilder<'_, sqlx::Postgres>,
    grants: &[crate::rbac::Grant],
    resource: crate::rbac::Resource,
    action: crate::rbac::Action,
    identity: Id,
    attributes: &HashMap<String, serde_json::Value>,
    target: &IamTargetSql<'_>,
) {
    use crate::rbac::{ExecutionScopeConstraint, OwnerConstraint};
    let mut wrote = false;
    query.push("(");
    for grant in grants
        .iter()
        .filter(|grant| grant.resource == resource && grant.actions.contains(&action))
    {
        if let Some(scope) = &grant.constraints {
            if scope.pack_state.is_some()
                || scope.pack_owner.is_some() && !crate::rbac::supports_pack_owner(resource)
                || scope.managing_pack_id.is_some()
                    && !crate::runtime_authority::pack_owned_resource(resource)
                || scope.owner_types.is_some()
                || scope.owner_refs.is_some()
                || scope.visibility.is_some()
                || scope.encrypted.is_some()
                || scope.attributes.as_ref().is_some_and(|expected| {
                    expected
                        .iter()
                        .any(|(key, value)| attributes.get(key) != Some(value))
                })
            {
                continue;
            }
        }
        if wrote {
            query.push(" OR ");
        }
        wrote = true;
        query.push("(TRUE");
        if let Some(scope) = &grant.constraints {
            if scope.pack_owner.is_some() {
                query
                    .push(" AND EXISTS (SELECT 1 FROM pack pack_owner WHERE pack_owner.id = ")
                    .push(target.managing_pack)
                    .push(" AND NOT pack_owner.is_standard AND pack_owner.installed_by = ")
                    .push_bind(identity)
                    .push(")");
            }
            if let Some(pack) = scope.managing_pack_id {
                query
                    .push(" AND ")
                    .push(target.managing_pack)
                    .push(" = ")
                    .push_bind(pack);
            }
            if let Some(ids) = &scope.ids {
                query
                    .push(" AND ")
                    .push(target.id)
                    .push(" = ANY(")
                    .push_bind(ids.clone())
                    .push(")");
            }
            if let Some(refs) = &scope.refs {
                query
                    .push(" AND ")
                    .push(target.reference)
                    .push(" = ANY(")
                    .push_bind(refs.clone())
                    .push(")");
            }
            if let Some(refs) = &scope.pack_refs {
                query
                    .push(" AND ")
                    .push(target.pack_ref)
                    .push(" = ANY(")
                    .push_bind(refs.clone())
                    .push(")");
            }
            if let Some(owner) = scope.owner {
                match owner {
                    OwnerConstraint::Any => {}
                    OwnerConstraint::None => {
                        query
                            .push(" AND ")
                            .push(target.owner_identity.unwrap_or("NULL::BIGINT"))
                            .push(" IS NULL");
                    }
                    OwnerConstraint::SelfOnly => {
                        query
                            .push(" AND ")
                            .push(target.owner_identity.unwrap_or("NULL::BIGINT"))
                            .push(" = ")
                            .push_bind(identity);
                    }
                }
            }
            match scope.execution_scope {
                None | Some(ExecutionScopeConstraint::Any) => {}
                Some(scope) => {
                    query
                        .push(" AND (")
                        .push(target.execution_owner.unwrap_or("NULL::BIGINT"))
                        .push(" = ")
                        .push_bind(identity);
                    if scope == ExecutionScopeConstraint::Descendants {
                        if let Some(ancestors) = target.ancestor_identities {
                            query
                                .push(" OR ")
                                .push_bind(identity)
                                .push(" = ANY(")
                                .push(ancestors)
                                .push(")");
                        }
                    }
                    query.push(")");
                }
            }
        }
        query.push(")");
    }
    if !wrote {
        query.push("FALSE");
    }
    query.push(")");
}

/// Ownership is accepted only for the exact retained origin tuple. A deleted
/// Key or a reused ref cannot acquire ownership from its replacement.
#[derive(Default)]
pub struct VerifiedKeyOrigins {
    packs: HashMap<crate::secret_provenance::KeyOrigin, Id>,
}

impl VerifiedKeyOrigins {
    /// Verify and hold the actual Key and owner, then key the ownership proof by
    /// the ORIGINAL authenticated origin. Classification edits are not source
    /// revocation and never replace the original Decrypt requirement.
    pub async fn retain_recorded(
        &mut self,
        conn: &mut sqlx::PgConnection,
        principal: Id,
        original: &crate::secret_provenance::KeyOrigin,
    ) -> Result<Option<super::runtime_target::ResolvedRuntimeTarget>> {
        use super::runtime_target::{RuntimeTarget, RuntimeTargetRepository};
        use crate::models::OwnerType;
        self.packs.remove(original);
        let Some(current) =
            RuntimeTargetRepository::resolve(conn, RuntimeTarget::Key(original.key_id), principal)
                .await?
        else {
            return Ok(None);
        };
        let Some(encrypted) = current.context().encrypted else {
            return Ok(None);
        };
        let owner = current.owner_id();
        let pack = current.pack_id();
        let Some(target) = current.recorded_key(original) else {
            return Ok(None);
        };
        match original.owner_type {
            OwnerType::Pack | OwnerType::Action | OwnerType::Sensor => {
                let Some(owner) = owner.filter(|id| *id > 0) else {
                    return Ok(None);
                };
                if matches!(original.owner_type, OwnerType::Action | OwnerType::Sensor) {
                    let kind = if original.owner_type == OwnerType::Action {
                        ComponentTarget::Action
                    } else {
                        ComponentTarget::Sensor
                    };
                    let parents = IamOwnershipRepository::component_refs_and_packs(
                        &mut *conn,
                        kind,
                        &[owner],
                    )
                    .await?;
                    if parents.len() != 1
                        || parents[0].0 != owner
                        || Some(parents[0].1.as_str()) != original.owner_ref.as_deref()
                        || parents[0].2 != pack
                    {
                        return Ok(None);
                    }
                }
                // Reuse the canonical concrete-owner proof. No ID-only cache,
                // textual-prefix inference or duplicate ownership SQL resolver.
                let current = Self::load(&mut *conn, &[original.key_id]).await?;
                let mut current_origin = original.clone();
                current_origin.encrypted = encrypted;
                let verified_pack = current.managing_pack(&current_origin);
                if verified_pack != pack
                    || verified_pack.is_some_and(|id| id <= 0)
                    || original.owner_type == OwnerType::Pack && verified_pack.is_none()
                {
                    return Ok(None);
                }
                if let Some(pack) = verified_pack {
                    self.packs.insert(original.clone(), pack);
                }
            }
            OwnerType::Identity => {
                if owner != original.owner_identity || owner.is_none_or(|id| id <= 0) {
                    return Ok(None);
                }
            }
            OwnerType::System => {
                if owner.is_some()
                    || pack.is_some()
                    || original.owner_identity.is_some()
                    || original.owner_ref.is_some()
                {
                    return Ok(None);
                }
            }
        }
        Ok(Some(target))
    }

    pub async fn keys_by_refs<'e, E>(
        executor: E,
        refs: &[String],
    ) -> Result<(Vec<crate::models::Key>, Self)>
    where
        E: sqlx::Executor<'e, Database = sqlx::Postgres>,
    {
        #[derive(sqlx::FromRow)]
        struct Row {
            #[sqlx(flatten)]
            key: crate::models::Key,
            managing_pack: Option<Id>,
        }
        let columns = super::key::SELECT_COLUMNS
            .split(", ")
            .map(|column| format!("k.{column}"))
            .collect::<Vec<_>>()
            .join(", ");
        let rows: Vec<Row> = sqlx::query_as(&format!(
            "SELECT {columns}, {} AS managing_pack FROM key k WHERE k.ref = ANY($1) ORDER BY k.ref",
            data_pack("k", "owner_type")
        ))
        .bind(refs)
        .fetch_all(executor)
        .await?;
        let mut ownership = Self::default();
        let keys = rows
            .into_iter()
            .map(|row| {
                if let Some(pack) = row.managing_pack {
                    ownership
                        .packs
                        .insert(crate::key_access::key_origin(&row.key), pack);
                }
                row.key
            })
            .collect();
        Ok((keys, ownership))
    }

    pub fn managing_pack(&self, origin: &crate::secret_provenance::KeyOrigin) -> Option<Id> {
        self.packs.get(origin).copied()
    }

    pub fn extend(&mut self, other: Self) {
        self.packs.extend(other.packs);
    }

    pub async fn load<'e, E>(executor: E, ids: &[Id]) -> Result<Self>
    where
        E: sqlx::Executor<'e, Database = sqlx::Postgres>,
    {
        if ids.is_empty() {
            return Ok(Self::default());
        }
        #[derive(sqlx::FromRow)]
        struct Row {
            id: Id,
            reference: String,
            owner_type: crate::models::OwnerType,
            owner_identity: Option<Id>,
            owner_ref: Option<String>,
            encrypted: bool,
            managing_pack: Option<Id>,
        }
        let rows: Vec<Row> = sqlx::query_as(&format!(
            "SELECT k.id, k.ref AS reference, k.owner_type, k.owner_identity, \
             CASE k.owner_type WHEN 'pack' THEN k.owner_pack_ref WHEN 'action' THEN k.owner_action_ref \
             WHEN 'sensor' THEN k.owner_sensor_ref WHEN 'identity' THEN k.owner ELSE NULL END AS owner_ref, \
             k.encrypted, {} AS managing_pack FROM key k WHERE k.id = ANY($1)",
            data_pack("k", "owner_type")
        ))
        .bind(ids)
        .fetch_all(executor)
        .await?;
        let packs = rows
            .into_iter()
            .filter_map(|row| {
                Some((
                    crate::secret_provenance::KeyOrigin {
                        key_id: row.id,
                        key_ref: row.reference,
                        owner_type: row.owner_type,
                        owner_identity: row.owner_identity,
                        owner_ref: row.owner_ref,
                        encrypted: row.encrypted,
                    },
                    row.managing_pack?,
                ))
            })
            .collect();
        Ok(Self { packs })
    }
}

pub struct IamOwnershipRepository;

impl IamOwnershipRepository {
    /// Request-local narrowing for IAM matching and SQL visibility only. Keep
    /// authored grants intact for serialization, assignment and delegation.
    pub async fn resolve_pack_owner_grants<'e, E>(
        executor: E,
        identity: Id,
        grants: Vec<crate::rbac::Grant>,
        allow_owner_roles: bool,
    ) -> Result<Vec<crate::rbac::Grant>>
    where
        E: sqlx::Executor<'e, Database = sqlx::Postgres>,
    {
        if !grants.iter().any(|grant| {
            grant.constraints.as_ref().is_some_and(|scope| {
                scope.pack_owner.is_some()
                    || grant.resource == crate::rbac::Resource::Packs
                        && scope.owner == Some(crate::rbac::OwnerConstraint::SelfOnly)
            })
        }) {
            return Ok(grants);
        }
        let packs: Vec<(Id, bool)> = sqlx::query_as(
            "SELECT p.id, p.is_standard FROM pack p WHERE EXISTS (SELECT 1 FROM identity i WHERE i.id = $1 AND NOT i.frozen) AND (p.installed_by = $1 OR ($2 AND NOT p.is_standard AND NOT EXISTS (SELECT 1 FROM pack_runtime_identity runtime WHERE runtime.identity = $1) AND EXISTS (SELECT 1 FROM identity_role_assignment member WHERE member.identity = $1 AND member.role = ANY(p.owner_roles) FOR SHARE))) ORDER BY p.id",
        )
        .bind(identity)
        .bind(allow_owner_roles)
        .fetch_all(executor)
        .await?;
        let pack_ids = packs.iter().map(|(id, _)| *id).collect::<Vec<_>>();
        let component_ids = packs
            .iter()
            .filter(|(_, standard)| !standard)
            .map(|(id, _)| *id)
            .collect::<Vec<_>>();
        Ok(narrow_pack_owner_grants(
            narrow_pack_self_grants(grants, &pack_ids),
            &component_ids,
        ))
    }

    /// Membership removals and principal freezing cannot cross a protected
    /// deployment/configuration authorization boundary before its commit.
    pub async fn lock_pack_owner_memberships(
        connection: &mut sqlx::PgConnection,
        identity: Id,
    ) -> Result<()> {
        super::identity::IdentityRepository::find_by_id_for_share(connection, identity)
            .await?
            .filter(|identity| !identity.frozen)
            .ok_or_else(|| {
                crate::Error::AuthenticationFailed("Pack owner principal is unavailable".into())
            })?;
        let _: Vec<Id> = sqlx::query_scalar(
            "SELECT id FROM identity_role_assignment WHERE identity = $1 ORDER BY id FOR SHARE",
        )
        .bind(identity)
        .fetch_all(connection)
        .await?;
        Ok(())
    }

    pub async fn populate_pack_role_membership<'e, E>(
        executor: E,
        resource: crate::rbac::Resource,
        context: &mut crate::rbac::AuthorizationContext,
        allow_owner_roles: bool,
    ) -> Result<()>
    where
        E: sqlx::Executor<'e, Database = sqlx::Postgres>,
    {
        context.pack_owner_role_member = false;
        if !allow_owner_roles || !crate::rbac::supports_pack_owner(resource) {
            return Ok(());
        }
        let Some(pack) = context.managing_pack_id else {
            return Ok(());
        };
        context.pack_owner_role_member = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS (SELECT 1 FROM identity_role_assignment member WHERE member.identity = i.id AND member.role = ANY(p.owner_roles) FOR SHARE) FROM pack p JOIN identity i ON i.id = $2 AND NOT i.frozen WHERE p.id = $1 AND NOT p.is_standard AND NOT EXISTS (SELECT 1 FROM pack_runtime_identity runtime WHERE runtime.identity = i.id)")
            .bind(pack).bind(context.identity_id).fetch_optional(executor).await?.unwrap_or(false);
        Ok(())
    }

    /// `managing_pack_id` must already be repository-resolved structural
    /// ownership. An associated pack and a namespace prefix are not accepted.
    pub async fn populate_pack_owner<'e, E>(
        executor: E,
        resource: crate::rbac::Resource,
        context: &mut crate::rbac::AuthorizationContext,
    ) -> Result<()>
    where
        E: sqlx::Executor<'e, Database = sqlx::Postgres>,
    {
        context.pack_owner_identity_id = None;
        if !crate::rbac::supports_pack_owner(resource) {
            return Ok(());
        }
        if let Some(pack) = context.managing_pack_id {
            let owner: Option<Option<Id>> = sqlx::query_scalar(
                "SELECT installed_by FROM pack WHERE id = $1 AND NOT is_standard",
            )
            .bind(pack)
            .fetch_optional(executor)
            .await?;
            context.pack_owner_identity_id = owner.flatten();
        }
        Ok(())
    }

    /// Resolve source metadata by its actual identity. Never use its declared
    /// pack_ref or association-only pack FK as managing ownership.
    pub async fn target_pack<'e, E>(
        executor: E,
        resource: crate::rbac::Resource,
        id: Option<Id>,
        reference: Option<&str>,
    ) -> Result<Option<Id>>
    where
        E: sqlx::Executor<'e, Database = sqlx::Postgres>,
    {
        use crate::rbac::Resource;
        if id.is_none() && reference.is_none() {
            return Ok(None);
        }
        let tables: &[&str] = match resource {
            Resource::Packs => &["pack"],
            Resource::Actions => &["action"],
            Resource::Rules => &["rule"],
            Resource::Policies => &["policy"],
            Resource::Queues => &["work_queue"],
            // Trigger and sensor IDs/refs can overlap. These callers must pass
            // ownership resolved using their concrete ComponentTarget instead.
            Resource::Triggers => return Ok(None),
            Resource::Runtimes => &["runtime"],
            Resource::Dashboards => &["dashboard"],
            _ => return Ok(None),
        };
        let query = tables.iter().map(|table| {
            if *table == "pack" {
                "SELECT c.id FROM pack c WHERE ($1::BIGINT IS NULL OR c.id = $1) AND ($2::TEXT IS NULL OR c.ref = $2)".to_owned()
            } else {
                format!("SELECT c.pack AS id FROM {table} c JOIN pack p ON p.id = c.pack WHERE c.management_origin = 'pack' AND ($1::BIGINT IS NULL OR c.id = $1) AND ($2::TEXT IS NULL OR c.ref = $2)")
            }
        }).collect::<Vec<_>>().join(" UNION ALL ");
        let owners: Vec<Id> = sqlx::query_scalar(&query)
            .bind(id)
            .bind(reference)
            .fetch_all(executor)
            .await?;
        Ok(if owners.len() == 1 {
            owners.first().copied()
        } else {
            None
        })
    }

    pub async fn trigger_visibility_owners<'e, E>(
        executor: E,
        ids: &[Id],
    ) -> Result<Vec<(Id, String, Option<Id>, bool)>>
    where
        E: sqlx::Executor<'e, Database = sqlx::Postgres>,
    {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        Ok(sqlx::query_as("SELECT t.id, t.ref, CASE WHEN t.management_origin = 'pack' AND p.id IS NOT NULL THEN t.pack END, \
            t.reference_visibility = 'public' FROM trigger t LEFT JOIN pack p ON p.id = t.pack WHERE t.id = ANY($1)")
            .bind(ids).fetch_all(executor).await?)
    }

    pub async fn component_refs_and_packs<'e, E>(
        executor: E,
        kind: ComponentTarget,
        ids: &[Id],
    ) -> Result<Vec<(Id, String, Option<Id>)>>
    where
        E: sqlx::Executor<'e, Database = sqlx::Postgres>,
    {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let table = kind.metadata().0;
        Ok(sqlx::query_as(&format!("SELECT c.id, c.ref, CASE WHEN c.management_origin = 'pack' AND p.id IS NOT NULL THEN c.pack END \
            FROM {table} c LEFT JOIN pack p ON p.id = c.pack WHERE c.id = ANY($1)"))
            .bind(ids).fetch_all(executor).await?)
    }

    pub async fn enforcement_trigger_owners<'e, E>(
        executor: E,
        ids: &[Id],
    ) -> Result<Vec<(Id, Id, String, Option<Id>)>>
    where
        E: sqlx::Executor<'e, Database = sqlx::Postgres>,
    {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        Ok(sqlx::query_as("SELECT ef.id, t.id, t.ref, CASE WHEN t.management_origin = 'pack' AND p.id IS NOT NULL THEN t.pack END \
            FROM enforcement ef JOIN LATERAL (SELECT MIN(e.trigger) AS trigger, MIN(e.trigger_ref) AS reference \
                FROM event e WHERE e.id = ef.event HAVING COUNT(*) = 1) source ON source.reference = ef.trigger_ref \
            JOIN trigger t ON t.id = source.trigger AND t.ref = source.reference LEFT JOIN pack p ON p.id = t.pack WHERE ef.id = ANY($1)")
            .bind(ids).fetch_all(executor).await?)
    }

    pub async fn workflow_packs<'e, E>(executor: E, ids: &[Id]) -> Result<HashMap<Id, Id>>
    where
        E: sqlx::Executor<'e, Database = sqlx::Postgres>,
    {
        if ids.is_empty() {
            return Ok(HashMap::new());
        }
        let rows: Vec<(Id, Id)> = sqlx::query_as(
            "SELECT w.id, w.pack FROM workflow_definition w JOIN pack p ON p.id = w.pack WHERE w.id = ANY($1) AND w.management_origin = 'pack'"
        ).bind(ids).fetch_all(executor).await?;
        Ok(rows.into_iter().collect())
    }

    pub async fn execution_packs<'e, E>(executor: E, ids: &[Id]) -> Result<HashMap<Id, Id>>
    where
        E: sqlx::Executor<'e, Database = sqlx::Postgres>,
    {
        if ids.is_empty() {
            return Ok(HashMap::new());
        }
        let rows: Vec<(Id, Id)> = sqlx::query_as(
            "SELECT e.id, p.pack FROM execution e JOIN pack_release p ON p.id = e.pack_release AND p.digest = e.pack_release_digest WHERE e.id = ANY($1)"
        ).bind(ids).fetch_all(executor).await?;
        Ok(rows.into_iter().collect())
    }

    pub async fn queue_origin_packs<'e, E>(
        executor: E,
        ids: &[Id],
    ) -> Result<HashMap<(Id, String, String), Id>>
    where
        E: sqlx::Executor<'e, Database = sqlx::Postgres>,
    {
        if ids.is_empty() {
            return Ok(HashMap::new());
        }
        let rows: Vec<(Id, String, String, Id)> = sqlx::query_as(
            "SELECT q.id, q.ref, p.ref, q.pack FROM work_queue q JOIN pack p ON p.id = q.pack WHERE q.id = ANY($1) AND q.management_origin = 'pack'"
        ).bind(ids).fetch_all(executor).await?;
        Ok(rows
            .into_iter()
            .map(|(id, reference, pack_ref, pack)| ((id, reference, pack_ref), pack))
            .collect())
    }

    /// One metadata query per batch, independent of the number of IAM checks.
    pub async fn component_packs<'e, E>(
        executor: E,
        kind: ComponentTarget,
        ids: &[Id],
    ) -> Result<HashMap<Id, Id>>
    where
        E: sqlx::Executor<'e, Database = sqlx::Postgres>,
    {
        if ids.is_empty() {
            return Ok(HashMap::new());
        }
        let table = kind.metadata().0;
        let rows: Vec<(Id, Id)> = sqlx::query_as(&format!(
            "SELECT c.id, c.pack FROM {table} c JOIN pack p ON p.id = c.pack \
             WHERE c.id = ANY($1) AND c.management_origin = 'pack'"
        ))
        .bind(ids)
        .fetch_all(executor)
        .await?;
        Ok(rows.into_iter().collect())
    }

    pub async fn component_pack<'e, E>(
        executor: E,
        kind: ComponentTarget,
        id: Id,
    ) -> Result<Option<Id>>
    where
        E: sqlx::Executor<'e, Database = sqlx::Postgres>,
    {
        Ok(Self::component_packs(executor, kind, &[id])
            .await?
            .remove(&id))
    }
}

fn narrow_pack_self_grants(
    grants: Vec<crate::rbac::Grant>,
    owned_packs: &[Id],
) -> Vec<crate::rbac::Grant> {
    let mut resolved = Vec::new();
    for grant in grants {
        if grant.resource != crate::rbac::Resource::Packs
            || !grant
                .constraints
                .as_ref()
                .is_some_and(|scope| scope.owner == Some(crate::rbac::OwnerConstraint::SelfOnly))
        {
            resolved.push(grant);
            continue;
        }
        let scope = grant
            .constraints
            .as_ref()
            .expect("owner-self pack constraint");
        for pack in owned_packs
            .iter()
            .copied()
            .filter(|pack| scope.managing_pack_id.is_none_or(|held| held == *pack))
        {
            let mut narrowed = grant.clone();
            let constraints = narrowed
                .constraints
                .as_mut()
                .expect("owner-self pack constraint");
            constraints.owner = None;
            constraints.managing_pack_id = Some(pack);
            resolved.push(narrowed);
        }
    }
    resolved
}

fn narrow_pack_owner_grants(
    grants: Vec<crate::rbac::Grant>,
    owned_packs: &[Id],
) -> Vec<crate::rbac::Grant> {
    let mut resolved = Vec::new();
    for grant in grants {
        let Some(scope) = grant
            .constraints
            .as_ref()
            .filter(|scope| scope.pack_owner.is_some())
        else {
            resolved.push(grant);
            continue;
        };
        if !crate::rbac::supports_pack_owner(grant.resource) {
            continue;
        }
        for pack in owned_packs
            .iter()
            .copied()
            .filter(|pack| scope.managing_pack_id.is_none_or(|held| held == *pack))
        {
            let mut narrowed = grant.clone();
            // Only policy/runtime creation persists structural pack ownership.
            // Other component APIs create ad hoc metadata; an owned association
            // cannot satisfy this constraint.
            if !matches!(
                grant.resource,
                crate::rbac::Resource::Policies | crate::rbac::Resource::Runtimes
            ) {
                narrowed
                    .actions
                    .retain(|action| *action != crate::rbac::Action::Create);
            }
            if narrowed.actions.is_empty() {
                continue;
            }
            let constraints = narrowed
                .constraints
                .as_mut()
                .expect("owner-constrained grant");
            constraints.pack_owner = None;
            constraints.managing_pack_id = Some(pack);
            resolved.push(narrowed);
        }
    }
    resolved
}

pub fn component_pack(alias: &str) -> String {
    format!("CASE WHEN {alias}.management_origin = 'pack' THEN {alias}.pack END")
}

pub fn execution_pack(alias: &str) -> String {
    format!("(SELECT p.pack FROM pack_release p WHERE p.id = {alias}.pack_release AND p.digest = {alias}.pack_release_digest)")
}

pub fn execution_ancestor_ids(alias: &str) -> String {
    format!("ARRAY(WITH RECURSIVE owners AS (SELECT p.id, p.parent, p.executor, 1 AS depth FROM execution p WHERE p.id = {alias}.parent \
        UNION ALL SELECT p.id, p.parent, p.executor, o.depth + 1 FROM execution p JOIN owners o ON p.id = o.parent WHERE o.depth < 64) \
        SELECT DISTINCT executor FROM owners WHERE executor IS NOT NULL)")
}

pub fn artifact_pack(alias: &str) -> String {
    format!("(SELECT b.managing_pack FROM artifact_runtime_owner_binding b \
        WHERE b.artifact = {alias}.id AND b.owner_type = {alias}.scope AND b.owner_ref = {alias}.owner \
        AND CASE b.owner_type \
          WHEN 'pack' THEN EXISTS (SELECT 1 FROM pack p WHERE p.id = b.owner_id AND p.id = b.managing_pack AND p.ref = b.owner_ref) \
          WHEN 'action' THEN EXISTS (SELECT 1 FROM action a JOIN pack p ON p.id = a.pack WHERE a.id = b.owner_id AND a.ref = b.owner_ref AND a.pack = b.managing_pack AND a.management_origin = 'pack') \
          WHEN 'sensor' THEN EXISTS (SELECT 1 FROM sensor s JOIN pack p ON p.id = s.pack WHERE s.id = b.owner_id AND s.ref = b.owner_ref AND s.pack = b.managing_pack AND s.management_origin = 'pack') \
          ELSE FALSE END)")
}

pub fn data_pack(alias: &str, owner_type_column: &str) -> String {
    format!("CASE {alias}.{owner_type_column} \
        WHEN 'pack' THEN (SELECT p.id FROM pack p WHERE p.id = {alias}.owner_pack AND p.ref = {alias}.owner_pack_ref) \
        WHEN 'action' THEN (SELECT a.pack FROM action a JOIN pack p ON p.id = a.pack WHERE a.id = {alias}.owner_action AND a.ref = {alias}.owner_action_ref AND a.management_origin = 'pack') \
        WHEN 'sensor' THEN (SELECT s.pack FROM sensor s JOIN pack p ON p.id = s.pack WHERE s.id = {alias}.owner_sensor AND s.ref = {alias}.owner_sensor_ref AND s.management_origin = 'pack') END")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pack_owner_narrowing_keeps_selectors_and_matches_canonical_evaluation() {
        use crate::rbac::{Action, AuthorizationContext, Grant, Resource};
        let grant: Grant = serde_json::from_value(serde_json::json!({
            "resource": "actions", "actions": ["read", "update", "create"],
            "constraints": {"pack_owner": "self", "refs": ["foreign_prefix.run"]}
        }))
        .unwrap();
        let narrowed = narrow_pack_owner_grants(vec![grant.clone()], &[17]);
        assert_eq!(narrowed.len(), 1);
        assert!(narrowed[0]
            .constraints
            .as_ref()
            .unwrap()
            .pack_owner
            .is_none());
        assert!(!narrowed[0].actions.contains(&Action::Create));
        for (pack, owner) in [
            (Some(17), Some(7)),
            (Some(18), Some(8)),
            (Some(19), None),
            (None, None),
        ] {
            let mut context = AuthorizationContext::new(7);
            context.target_ref = Some("foreign_prefix.run".into());
            context.managing_pack_id = pack;
            context.pack_owner_identity_id = owner;
            assert_eq!(
                grant.allows(Resource::Actions, Action::Read, &context),
                narrowed.iter().any(|grant| grant.allows(
                    Resource::Actions,
                    Action::Read,
                    &context
                ))
            );
        }
        assert!(narrow_pack_owner_grants(vec![grant.clone()], &[]).is_empty());
        let mut intersected = grant.clone();
        intersected.constraints.as_mut().unwrap().managing_pack_id = Some(18);
        assert!(narrow_pack_owner_grants(vec![intersected], &[17]).is_empty());
        let mut unsupported = grant.clone();
        unsupported.resource = Resource::Keys;
        assert!(narrow_pack_owner_grants(vec![unsupported], &[17]).is_empty());
        let mut policy = grant;
        policy.resource = Resource::Policies;
        assert!(narrow_pack_owner_grants(vec![policy], &[17])[0]
            .actions
            .contains(&Action::Create));
    }

    #[test]
    fn pack_owner_sql_uses_structural_management_and_primary_owner_not_prefix() {
        let grants = serde_json::from_value(serde_json::json!([{
            "resource": "rules", "actions": ["read"], "constraints": {"pack_owner": "self"}
        }]))
        .unwrap();
        let filter = ComponentReadFilter {
            identity: 7,
            attributes: HashMap::new(),
            grants,
        };
        let mut query = sqlx::QueryBuilder::new("");
        push_component_read(&mut query, &filter, "rule", crate::rbac::Resource::Rules);
        assert!(query.sql().contains("rule.management_origin = 'pack'"));
        assert!(query.sql().contains("pack_owner.installed_by = "));
        assert!(query.sql().contains("NOT pack_owner.is_standard"));
        assert!(!query.sql().contains("split_part"));
    }

    #[test]
    fn uninstalled_state_never_matches_an_existing_sql_target() {
        let grants: Vec<crate::rbac::Grant> = serde_json::from_value(serde_json::json!([{
            "resource": "packs", "actions": ["read"],
            "constraints": {"pack_state": "uninstalled"}
        }]))
        .unwrap();
        let mut query = sqlx::QueryBuilder::new("");
        push_iam_grants(
            &mut query,
            &grants,
            crate::rbac::Resource::Packs,
            crate::rbac::Action::Read,
            7,
            &HashMap::new(),
            &IamTargetSql {
                id: "p.id",
                reference: "p.ref",
                pack_ref: "p.ref",
                managing_pack: "p.id",
                owner_identity: Some("p.installed_by"),
                execution_owner: None,
                ancestor_identities: None,
            },
        );
        assert_eq!(query.sql(), "(FALSE)");
    }

    #[test]
    fn iam_sql_keeps_local_ownership_selectors_attributes_and_self_scopes_in_one_query() {
        let grants: Vec<crate::rbac::Grant> = serde_json::from_value(serde_json::json!([{
            "resource":"executions", "actions":["read"], "constraints":{
                "managing_pack_id":17,"refs":["local.run"],"ids":[8],"owner":"self",
                "execution_scope":"descendants","attributes":{"team":"ops"}
            }
        }]))
        .unwrap();
        let mut query = sqlx::QueryBuilder::new("SELECT e.id FROM execution e WHERE ");
        push_iam_grants(
            &mut query,
            &grants,
            crate::rbac::Resource::Executions,
            crate::rbac::Action::Read,
            42,
            &HashMap::from([("team".into(), serde_json::json!("ops"))]),
            &IamTargetSql {
                id: "e.id",
                reference: "e.action_ref",
                pack_ref: "split_part(e.action_ref,'.',1)",
                managing_pack: &execution_pack("e"),
                owner_identity: Some("e.executor"),
                execution_owner: Some("e.executor"),
                ancestor_identities: Some(&execution_ancestor_ids("e")),
            },
        );
        assert!(query.sql().contains("p.digest = e.pack_release_digest"));
        assert!(query.sql().contains("e.id = ANY("));
        assert!(query.sql().contains("e.action_ref = ANY("));
        assert!(query.sql().contains("owners o"));
        assert_eq!(query.sql().matches("SELECT e.id FROM execution").count(), 1);
        let mut denied = sqlx::QueryBuilder::new("");
        push_iam_grants(
            &mut denied,
            &grants,
            crate::rbac::Resource::Executions,
            crate::rbac::Action::Read,
            42,
            &HashMap::new(),
            &IamTargetSql {
                id: "e.id",
                reference: "e.action_ref",
                pack_ref: "NULL::TEXT",
                managing_pack: &execution_pack("e"),
                owner_identity: None,
                execution_owner: None,
                ancestor_identities: None,
            },
        );
        assert_eq!(denied.sql(), "(FALSE)");
    }

    #[test]
    fn ownership_expressions_do_not_infer_ownership_from_prefixes_or_json() {
        for sql in [
            component_pack("c"),
            execution_pack("e"),
            artifact_pack("a"),
            data_pack("k", "owner_type"),
        ] {
            assert!(!sql.contains("split_part"));
            assert!(!sql.contains("json"));
        }
        assert!(execution_pack("e").contains("pack_release_digest"));
        assert!(data_pack("k", "owner_type").contains("management_origin = 'pack'"));
        assert!(artifact_pack("a").contains("b.owner_id"));
    }
}
