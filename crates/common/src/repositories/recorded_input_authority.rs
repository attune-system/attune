//! Accepted human input authority is purpose-limited and independent of login
//! liveness. It never supplies execution API grants or a runtime sponsor.
use crate::{
    delegation::DelegationAuthority,
    models::Id,
    rbac::{Action, Grant, Resource},
    value_schema::digest,
    Error, Result,
};
use serde::{Deserialize, Serialize};
use sqlx::{types::Json, PgConnection};

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct InputAuthority {
    identity_id: Id,
    attributes: std::collections::HashMap<String, serde_json::Value>,
    grants: Vec<Grant>,
}

pub struct RecordedInputAuthorityRepository;

fn input_resource(resource: Resource) -> bool {
    matches!(
        resource,
        Resource::Keys
            | Resource::Packs
            | Resource::Executions
            | Resource::Artifacts
            | Resource::QueueItems
            | Resource::Queues
            | Resource::Events
            | Resource::Triggers
            | Resource::Inquiries
    )
}

fn valid_input(authority: &InputAuthority) -> bool {
    authority.identity_id > 0
        && authority.grants.iter().all(|grant| {
            input_resource(grant.resource)
                && grant
                    .actions
                    .iter()
                    .all(|action| matches!(action, Action::Read | Action::Decrypt))
        })
}

impl RecordedInputAuthorityRepository {
    pub async fn key_origin_allowed(
        connection: &mut PgConnection,
        authority: &DelegationAuthority,
        origin: &crate::secret_provenance::KeyOrigin,
        action: Action,
    ) -> Result<bool> {
        let mut ownership = super::iam_ownership::VerifiedKeyOrigins::default();
        if ownership
            .retain_recorded(connection, authority.identity_id, origin)
            .await?
            .is_none()
        {
            return Ok(false);
        }
        Ok(authority.key_allows_verified(origin, action, &ownership))
    }
    /// Editing selectors must not borrow an earlier requester's accepted input
    /// capabilities. This is a proof only; the immutable snapshot is unchanged.
    pub async fn authorize_queue_payload_edit(
        connection: &mut PgConnection,
        item: Id,
        editors: &[DelegationAuthority],
    ) -> Result<crate::models::WorkQueueItem> {
        // Item deletion cascades to this snapshot. Protect the parent first;
        // taking a snapshot lock before upgrading an item lock can deadlock a
        // concurrent deletion. FOR UPDATE also serializes competing editors.
        let target =
            super::work_queue::WorkQueueItemRepository::find_by_id_for_update(connection, item)
                .await?
                .ok_or_else(|| {
                    Error::PermissionDenied("Queue input edit target is unavailable".into())
                })?;
        let Some(accepted) = Self::item_snapshots(connection, item).await? else {
            // Without a snapshot there is no recorded input capability to reuse.
            // Dispatch still cannot resolve Keys using missing authority.
            return Ok(target);
        };
        for accepted in accepted {
            let effective = accepted
                .grants
                .iter()
                .filter(|grant| matches!(grant.resource, Resource::Keys | Resource::Packs))
                .filter(|grant| {
                    grant
                        .constraints
                        .as_ref()
                        .and_then(|scope| scope.attributes.as_ref())
                        .is_none_or(|conditions| {
                            conditions
                                .iter()
                                .all(|(key, value)| accepted.attributes.get(key) == Some(value))
                        })
                })
                .collect::<Vec<_>>();
            if effective.is_empty() {
                continue;
            }
            if editors.is_empty()
                || editors.iter().any(|editor| {
                    let requested = effective
                        .iter()
                        .map(|grant| {
                            let mut grant = (**grant).clone();
                            if let Some(scope) = &mut grant.constraints {
                                // The captured attribute predicate was evaluated against its
                                // original context above, not the new editor's attributes.
                                scope.attributes = None;
                                if editor.identity_id != accepted.identity_id {
                                    // Relative ownership/lineage belongs to the original
                                    // principal. Requiring coverage of the wider absolute
                                    // scope is conservative, unlike reinterpreting "self".
                                    if scope.owner == Some(crate::rbac::OwnerConstraint::SelfOnly) {
                                        scope.owner = None;
                                    }
                                    if matches!(
                                    scope.execution_scope,
                                    Some(
                                        crate::rbac::ExecutionScopeConstraint::SelfOnly
                                            | crate::rbac::ExecutionScopeConstraint::Descendants
                                    )
                                ) {
                                        scope.execution_scope = None;
                                    }
                                }
                            }
                            grant
                        })
                        .collect::<Vec<_>>();
                    !editor.covers(&requested)
                })
            {
                return Err(Error::PermissionDenied("Payload editing requires coverage of the item's accepted Key and Pack input authority".into()));
            }
        }
        Ok(target)
    }

    /// Preserve the redactions of every contributor. A later contributor with
    /// broader access cannot disclose a field hidden from an earlier one.
    pub async fn pack_template_values(
        connection: &mut PgConnection,
        pack: &crate::models::Pack,
        authorities: &[DelegationAuthority],
        encryption_key: Option<&str>,
    ) -> Result<(serde_json::Value, Vec<String>, Option<Id>)> {
        let mut redactions = std::collections::BTreeSet::new();
        let mut selected = None;
        for authority in authorities {
            let values = super::component_configuration::ComponentConfigurationRepository::pack_template_values(connection, pack, Some(authority), encryption_key).await?;
            redactions.extend(crate::secret_values::redacted_paths(&values.0));
            selected = Some(values);
        }
        let mut selected = match selected {
            Some(values) => values,
            None => super::component_configuration::ComponentConfigurationRepository::pack_template_values(connection, pack, None, encryption_key).await?,
        };
        for path in redactions {
            let value = selected.0.pointer_mut(&path).ok_or_else(|| {
                Error::PermissionDenied("Input configuration changed during admission".into())
            })?;
            *value = crate::secret_values::redaction_marker();
        }
        Ok(selected)
    }

    pub async fn resolve_keys(
        connection: &mut PgConnection,
        authorities: &[DelegationAuthority],
        refs: &[String],
        encryption_key: Option<&str>,
    ) -> Result<std::collections::BTreeMap<String, crate::key_access::ResolvedKey>> {
        let mut selected = None;
        for authority in authorities {
            selected = Some(
                crate::key_access::resolve_explicit_keys(
                    &mut *connection,
                    authority,
                    refs,
                    encryption_key,
                )
                .await?,
            );
        }
        let keys = selected.ok_or_else(|| {
            Error::PermissionDenied("Recorded input authority is unavailable".into())
        })?;
        for key in keys.values() {
            for authority in authorities {
                if !Self::key_origin_allowed(connection, authority, &key.origin, Action::Read)
                    .await?
                    || key.origin.encrypted
                        && !Self::key_origin_allowed(
                            connection,
                            authority,
                            &key.origin,
                            Action::Decrypt,
                        )
                        .await?
                {
                    return Err(Error::PermissionDenied(
                        "A contributor did not approve the input Key origin".into(),
                    ));
                }
            }
        }
        Ok(keys)
    }

    pub async fn capture_queue_execution(
        connection: &mut PgConnection,
        execution: Id,
        items: &[Id],
    ) -> Result<()> {
        if items.is_empty() || items.len() > 256 {
            return Err(Error::validation("Queue input batch is empty or too large"));
        }
        // Validate nested evidence before copying, including an explicit gap
        // for an item with no accepted input snapshot. Missing contributors
        // cannot disappear because another item has broader rights.
        for item in items {
            Self::item_snapshots(connection, *item).await?;
        }
        sqlx::query("WITH selected(item) AS (SELECT unnest($2::BIGINT[])), candidates AS (SELECT owner_id AS item,''::TEXT AS contributor,authority,authority_digest FROM recorded_execution_input_authority WHERE owner_kind = 'queue_item' AND owner_id = ANY($2) UNION ALL SELECT item,contributor,authority,authority_digest FROM recorded_queue_item_input WHERE item = ANY($2)), expected AS (SELECT SUM(GREATEST((SELECT COUNT(*) FROM candidates c WHERE c.item = s.item),1))::INTEGER AS n FROM selected s) INSERT INTO recorded_queue_execution_input(execution,item,contributor,expected_items,authority,authority_digest) SELECT $1,c.item,c.contributor,e.n,c.authority,c.authority_digest FROM candidates c CROSS JOIN expected e")
            .bind(execution).bind(items).execute(connection).await?;
        Ok(())
    }

    pub async fn queue_authorities(
        connection: &mut PgConnection,
        execution: Id,
    ) -> Result<Option<Vec<DelegationAuthority>>> {
        let rows: Vec<(i32, Json<InputAuthority>, String)> = sqlx::query_as("SELECT expected_items,authority,authority_digest FROM recorded_queue_execution_input WHERE execution = $1 ORDER BY item,contributor LIMIT 257 FOR SHARE").bind(execution).fetch_all(connection).await?;
        if rows.is_empty() {
            return Ok(None);
        }
        let count = rows.len();
        if count > 256 {
            return Err(Error::validation(
                "Queue input contributors exceed their bound",
            ));
        }
        let mut authorities = Vec::with_capacity(count);
        for (expected_items, authority, expected_digest) in rows {
            if usize::try_from(expected_items).ok() != Some(count) {
                return Ok(Some(Vec::new()));
            }
            if Self::digest(&authority)? != expected_digest || !valid_input(&authority) {
                return Err(Error::PermissionDenied(
                    "Queue input authority is incomplete or invalid".into(),
                ));
            }
            authorities.push(DelegationAuthority::new(
                authority.identity_id,
                authority.attributes.clone(),
                authority.grants.clone(),
            ));
        }
        Ok(Some(authorities))
    }

    async fn item_snapshots(
        connection: &mut PgConnection,
        item: Id,
    ) -> Result<Option<Vec<InputAuthority>>> {
        let mut values = Self::read(connection, "queue_item", item)
            .await?
            .into_iter()
            .collect::<Vec<_>>();
        let rows: Vec<(i32, Json<InputAuthority>, String)> = sqlx::query_as("SELECT expected_contributors,authority,authority_digest FROM recorded_queue_item_input WHERE item = $1 ORDER BY contributor LIMIT 257 FOR SHARE")
            .bind(item).fetch_all(connection).await?;
        if rows.len() > 256 {
            return Err(Error::validation(
                "Item input contributors exceed their bound",
            ));
        }
        for (expected, authority, hash) in &rows {
            if usize::try_from(*expected).ok() != Some(rows.len())
                || !valid_input(authority)
                || Self::digest(authority)? != *hash
            {
                return Err(Error::PermissionDenied(
                    "Item input contributor evidence is incomplete or invalid".into(),
                ));
            }
            values.push(authority.0.clone());
        }
        if values.len() > 256 {
            return Err(Error::validation(
                "Item input contributors exceed their bound",
            ));
        }
        Ok((!values.is_empty()).then_some(values))
    }

    pub async fn item_authorities(
        connection: &mut PgConnection,
        item: Id,
    ) -> Result<Option<Vec<DelegationAuthority>>> {
        Ok(Self::item_snapshots(connection, item).await?.map(|values| {
            values
                .into_iter()
                .map(|value| {
                    DelegationAuthority::new(value.identity_id, value.attributes, value.grants)
                })
                .collect()
        }))
    }
    /// Server-side acceptance only. The authenticated human is active here;
    /// later delivery never reloads their account or permission assignments.
    pub async fn capture_human(
        connection: &mut PgConnection,
        kind: &str,
        owner: Id,
        human: Id,
    ) -> Result<()> {
        let identity = super::identity::IdentityRepository::find_by_id_for_share(connection, human)
            .await?
            .filter(|identity| !identity.frozen)
            .ok_or_else(|| Error::PermissionDenied("Input requester is not active".into()))?;
        let mut sets =
            super::identity::PermissionSetRepository::find_by_identity_for_share(connection, human)
                .await?;
        sets.extend(
            super::identity::PermissionSetRepository::find_by_identity_roles_for_share(
                connection, human,
            )
            .await?,
        );
        let mut grants = crate::delegation::grants_from_sets(sets)?;
        // Read-only purpose authority can prove later generic input origins. It
        // never supplies an execution API selection or operation authorization.
        grants.retain(|grant| input_resource(grant.resource));
        for grant in &mut grants {
            grant
                .actions
                .retain(|action| matches!(action, Action::Read | Action::Decrypt));
        }
        grants.retain(|grant| !grant.actions.is_empty());
        let attributes = match identity.attributes {
            serde_json::Value::Object(values) => values.into_iter().collect(),
            _ => Default::default(),
        };
        Self::store(
            connection,
            kind,
            owner,
            &InputAuthority {
                identity_id: human,
                attributes,
                grants,
            },
        )
        .await
    }

    pub async fn load(
        connection: &mut PgConnection,
        kind: &str,
        owner: Id,
    ) -> Result<Option<DelegationAuthority>> {
        let Some(authority) = Self::read(connection, kind, owner).await? else {
            return Ok(None);
        };
        Ok(Some(DelegationAuthority::new(
            authority.identity_id,
            authority.attributes,
            authority.grants,
        )))
    }

    pub async fn copy(
        connection: &mut PgConnection,
        source_kind: &str,
        source: Id,
        target_kind: &str,
        target: Id,
    ) -> Result<()> {
        if let Some(authority) = Self::read(connection, source_kind, source).await? {
            Self::store(connection, target_kind, target, &authority).await?;
        }
        if source_kind == "execution" && target_kind == "queue_item" {
            // Keep the original records as separate contributors under the new
            // item. Do not choose one authority or union their grants.
            if Self::queue_authorities(connection, source)
                .await?
                .is_some_and(|values| values.is_empty())
            {
                return Err(Error::PermissionDenied(
                    "Originating queue input contributor proof is incomplete".into(),
                ));
            }
            sqlx::query("INSERT INTO recorded_queue_item_input(item,contributor,expected_contributors,authority,authority_digest) SELECT $1,item::TEXT || ':' || contributor,expected_items,authority,authority_digest FROM recorded_queue_execution_input WHERE execution = $2")
                .bind(target).bind(source).execute(&mut *connection).await?;
        }
        if source_kind == "execution" && target_kind == "execution" {
            sqlx::query("INSERT INTO recorded_queue_execution_input (execution,item,contributor,expected_items,authority,authority_digest) SELECT $1,item,contributor,expected_items,authority,authority_digest FROM recorded_queue_execution_input WHERE execution = $2 ON CONFLICT (execution,item,contributor) DO NOTHING")
                .bind(target).bind(source).execute(connection).await?;
        }
        Ok(())
    }

    async fn read(
        connection: &mut PgConnection,
        kind: &str,
        owner: Id,
    ) -> Result<Option<InputAuthority>> {
        let row: Option<(Json<InputAuthority>, String)> = sqlx::query_as("SELECT authority,authority_digest FROM recorded_execution_input_authority WHERE owner_kind = $1 AND owner_id = $2 FOR SHARE")
            .bind(kind).bind(owner).fetch_optional(connection).await?;
        let Some((authority, expected)) = row else {
            return Ok(None);
        };
        if Self::digest(&authority)? != expected || !valid_input(&authority) {
            return Err(Error::PermissionDenied(
                "Recorded input authority is invalid".into(),
            ));
        }
        Ok(Some(authority.0))
    }

    async fn store(
        connection: &mut PgConnection,
        kind: &str,
        owner: Id,
        authority: &InputAuthority,
    ) -> Result<()> {
        if !matches!(kind, "execution" | "queue_item") || owner <= 0 {
            return Err(Error::validation("Invalid input authority owner"));
        }
        let digest = Self::digest(authority)?;
        sqlx::query("INSERT INTO recorded_execution_input_authority (owner_kind,owner_id,authority,authority_digest) VALUES ($1,$2,$3,$4) ON CONFLICT (owner_kind,owner_id) DO NOTHING")
            .bind(kind).bind(owner).bind(Json(authority)).bind(&digest).execute(&mut *connection).await?;
        let retained: String = sqlx::query_scalar("SELECT authority_digest FROM recorded_execution_input_authority WHERE owner_kind = $1 AND owner_id = $2 FOR SHARE")
            .bind(kind).bind(owner).fetch_one(connection).await?;
        if retained != digest {
            return Err(Error::PermissionDenied(
                "Input owner already has different authority".into(),
            ));
        }
        Ok(())
    }

    fn digest(authority: &InputAuthority) -> Result<String> {
        digest(&serde_json::to_value(authority)?)
            .map_err(|_| Error::validation("Invalid input authority"))
    }
}
