//! Concrete target selectors for immutable policy capture and consumption.

use super::runtime_target::{
    ComponentTarget, ResolvedRuntimeTarget, RuntimeTarget, RuntimeTargetRepository,
};
use crate::{
    models::{Id, OwnerType},
    rbac::{Action, Resource},
    runtime_authority::{AdmittedGrant, BoundRuntimeTarget, GrantTargetBindings},
    value_schema::digest,
    Error, Result,
};
use sqlx::PgConnection;
use std::collections::{BTreeMap, BTreeSet};

pub(crate) fn grant_key(grant: &AdmittedGrant) -> Result<String> {
    digest(&serde_json::to_value(grant)?)
        .map_err(|_| Error::validation("Invalid concrete target grant digest"))
}

pub(crate) async fn validate_approval_bindings(
    connection: &mut PgConnection,
    policy: &crate::runtime_authority::RuntimePolicy,
) -> Result<()> {
    for grant in policy
        .invocations
        .iter()
        .flat_map(|entry| entry.permission_sets.values())
        .flat_map(|set| &set.grants)
    {
        let AdmittedGrant::External {
            approval_id,
            grant: requested,
        } = grant
        else {
            continue;
        };
        let approval = super::runtime_authority::RuntimeAuthorityRepository::find_approval(
            connection,
            *approval_id,
        )
        .await?
        .ok_or_else(|| Error::PermissionDenied("External approval is unavailable".into()))?;
        let desired = &policy.concrete_targets[&grant_key(grant)?];
        if !approval_covers_binding(connection, &approval, &policy.owner, requested, desired)
            .await?
        {
            return Err(Error::PermissionDenied("Fixed external approval does not cover these concrete target identities; issue a replacement approval".into()));
        }
    }
    Ok(())
}

pub(crate) async fn approval_covers_binding(
    connection: &mut PgConnection,
    approval: &super::runtime_authority::ExternalApprovalRecord,
    owner: &crate::runtime_authority::PackAuthorityOwner,
    requested: &crate::rbac::Grant,
    desired: &GrantTargetBindings,
) -> Result<bool> {
    use crate::rbac::{can_delegate_grants, AuthorizationContext};
    let rows:Vec<(String,sqlx::types::Json<GrantTargetBindings>)> = sqlx::query_as(
            "SELECT grant_digest,binding FROM runtime_external_approval_target_binding WHERE approval = $1 ORDER BY grant_digest",
        ).bind(approval.id).fetch_all(&mut *connection).await?;
    let bindings = rows
        .into_iter()
        .map(|(key, value)| (key, value.0))
        .collect::<BTreeMap<_, _>>();
    for action in &requested.actions {
        let mut operation = requested.clone();
        operation.actions = vec![*action];
        let mut covered = false;
        for approved in approval.grants.iter() {
            if !can_delegate_grants(
                std::slice::from_ref(approved),
                std::slice::from_ref(&operation),
                &AuthorizationContext::new(owner.sponsor_identity_id),
            ) {
                continue;
            }
            let key = digest(&serde_json::to_value(approved)?)
                .map_err(|_| Error::validation("Invalid approval target digest"))?;
            let Some(bound) = bindings.get(&key) else {
                continue;
            };
            if selector_subset(desired.targets.as_ref(), bound.targets.as_ref())
                && selector_subset(desired.owners.as_ref(), bound.owners.as_ref())
                && selector_subset(desired.packs.as_ref(), bound.packs.as_ref())
            {
                covered = true;
                break;
            }
        }
        if !covered {
            return Ok(false);
        }
    }
    Ok(true)
}

fn selector_subset<T: Ord>(
    requested: Option<&BTreeSet<T>>,
    approved: Option<&BTreeSet<T>>,
) -> bool {
    match (requested, approved) {
        (_, None) => true,
        (Some(requested), Some(approved)) => requested.is_subset(approved),
        (None, Some(_)) => false,
    }
}

pub(crate) async fn capture(
    connection: &mut PgConnection,
    policy: &mut crate::runtime_authority::RuntimePolicy,
) -> Result<()> {
    let grants = policy
        .invocations
        .iter()
        .flat_map(|entry| entry.permission_sets.values())
        .flat_map(|set| set.grants.iter().cloned())
        .collect::<Vec<_>>();
    policy.concrete_targets =
        capture_grants(connection, &policy.owner, &grants, &policy.concrete_targets).await?;
    Ok(())
}

pub(crate) async fn capture_grants(
    connection: &mut PgConnection,
    owner: &crate::runtime_authority::PackAuthorityOwner,
    grants: &[AdmittedGrant],
    previous: &BTreeMap<String, GrantTargetBindings>,
) -> Result<BTreeMap<String, GrantTargetBindings>> {
    let mut bindings = BTreeMap::new();
    for grant in grants {
        let key = grant_key(grant)?;
        if bindings.contains_key(&key) {
            continue;
        }
        // An unrelated metadata edit must not follow a deleted/recreated ref.
        if let Some(existing) = previous.get(&key) {
            bindings.insert(key, existing.clone());
            continue;
        }
        let mut binding = GrantTargetBindings::default();
        if let Some(scope) = &grant.grant().constraints {
            if let Some(refs) = &scope.refs {
                let tables: &[(&str, &str)] = match grant.grant().resource {
                    Resource::Packs => &[("pack", "ref")],
                    Resource::Actions => &[("action", "ref")],
                    Resource::Policies => &[("policy", "ref")],
                    Resource::Queues => &[("work_queue", "ref")],
                    Resource::Rules => &[("rule", "ref")],
                    Resource::Triggers => &[("trigger", "ref"), ("sensor", "ref")],
                    Resource::Events if grant.grant().actions.iter().all(|action| *action == Action::Create) => &[("trigger", "ref")],
                    Resource::Keys => &[("key", "ref")],
                    Resource::Artifacts => &[("artifact", "ref")],
                    Resource::Caches => &[("cache_namespace", "COALESCE(definition_ref,namespace)")],
                    Resource::Runtimes => &[("runtime", "ref")],
                    Resource::Dashboards => &[("dashboard", "ref")],
                    _ => return Err(Error::validation("This resource requires an ownership scope rather than concrete ref selectors")),
                };
                let mut targets = BTreeSet::new();
                for reference in refs {
                    let mut matched = false;
                    for (table, column) in tables {
                        // Table and expression names come exclusively from the
                        // resource mapping above, never from authored input.
                        let query = format!("SELECT id FROM {table} WHERE {column} = $1 FOR SHARE");
                        let ids: Vec<Id> = sqlx::query_scalar(&query)
                            .bind(reference)
                            .fetch_all(&mut *connection)
                            .await?;
                        for id in ids {
                            let target = runtime_target(table, id)?;
                            let resolved = RuntimeTargetRepository::resolve(
                                connection,
                                target,
                                owner.sponsor_identity_id,
                            )
                            .await?
                            .ok_or_else(|| {
                                Error::validation("Concrete policy target is unavailable")
                            })?;
                            if let AdmittedGrant::PackOwned { pack_id, .. } = grant {
                                if resolved.pack_id() != Some(*pack_id) {
                                    continue;
                                }
                            }
                            if resolved.pack_id() != Some(owner.pack_id) {
                                use super::FindById;
                                if *table == "trigger" {
                                    let trigger = super::trigger::TriggerRepository::find_by_id(
                                        &mut *connection,
                                        id,
                                    )
                                    .await?
                                    .ok_or_else(|| {
                                        Error::validation("Concrete trigger target is unavailable")
                                    })?;
                                    crate::action_visibility::ensure_trigger_reference_allowed(
                                        &trigger,
                                        Some(&owner.pack_ref),
                                        "permission set",
                                        "runtime target",
                                    )?;
                                } else if *table == "action" {
                                    let action = super::action::ActionRepository::find_by_id(
                                        &mut *connection,
                                        id,
                                    )
                                    .await?
                                    .ok_or_else(|| {
                                        Error::validation("Concrete action target is unavailable")
                                    })?;
                                    crate::action_visibility::ensure_action_reference_allowed(
                                        &action,
                                        Some(&owner.pack_ref),
                                        "permission set",
                                        "runtime target",
                                    )?;
                                }
                            }
                            targets.insert(bound_target(&resolved).ok_or_else(|| {
                                Error::validation("Concrete target has no stable identity")
                            })?);
                            matched = true;
                        }
                    }
                    if !matched {
                        return Err(Error::validation("Concrete permission selector has no target under its admitted ownership"));
                    }
                }
                binding.targets = Some(targets);
            }
            if let Some(refs) = &scope.owner_refs {
                let mut owners = BTreeSet::new();
                for reference in refs {
                    let mut matched = false;
                    for kind in scope.owner_types.as_deref().unwrap_or(&[
                        OwnerType::Pack,
                        OwnerType::Action,
                        OwnerType::Sensor,
                        OwnerType::Identity,
                    ]) {
                        let table = match kind {
                            OwnerType::Pack => "pack",
                            OwnerType::Action => "action",
                            OwnerType::Sensor => "sensor",
                            OwnerType::Identity => "identity",
                            OwnerType::System => continue,
                        };
                        let column = if *kind == OwnerType::Identity {
                            "id::text"
                        } else {
                            "ref"
                        };
                        let query = format!("SELECT id FROM {table} WHERE {column} = $1 FOR SHARE");
                        let ids: Vec<Id> = sqlx::query_scalar(&query)
                            .bind(reference)
                            .fetch_all(&mut *connection)
                            .await?;
                        for id in ids {
                            let managing_pack = if *kind == OwnerType::Identity {
                                None
                            } else {
                                RuntimeTargetRepository::resolve(
                                    connection,
                                    runtime_target(table, id)?,
                                    owner.sponsor_identity_id,
                                )
                                .await?
                                .ok_or_else(|| {
                                    Error::validation("Concrete policy owner is unavailable")
                                })?
                                .pack_id()
                            };
                            if let AdmittedGrant::PackOwned { pack_id, .. } = grant {
                                if *kind == OwnerType::Identity {
                                    continue;
                                }
                                if managing_pack != Some(*pack_id) {
                                    continue;
                                }
                            }
                            owners.insert((table.into(), id, managing_pack));
                            matched = true;
                        }
                    }
                    if !matched {
                        return Err(Error::validation(
                            "Concrete policy owner selector escapes its admitted ownership",
                        ));
                    }
                }
                if owners.is_empty() && !refs.is_empty() {
                    return Err(Error::validation(
                        "Concrete policy owner selector is unavailable",
                    ));
                }
                binding.owners = Some(owners);
            }
            if let Some(refs) = &scope.pack_refs {
                let ids: Vec<Id> = sqlx::query_scalar(
                    "SELECT id FROM pack WHERE ref = ANY($1) ORDER BY id FOR SHARE",
                )
                .bind(refs)
                .fetch_all(&mut *connection)
                .await?;
                if ids.len() != refs.iter().collect::<BTreeSet<_>>().len() {
                    return Err(Error::validation(
                        "Concrete policy pack scope is unavailable",
                    ));
                }
                binding.packs = Some(ids.into_iter().collect());
            }
        }
        bindings.insert(key, binding);
    }
    Ok(bindings)
}

fn runtime_target(table: &str, id: Id) -> Result<RuntimeTarget> {
    Ok(match table {
        "pack" => RuntimeTarget::Pack(id),
        "action" => RuntimeTarget::Component(ComponentTarget::Action, id),
        "policy" => RuntimeTarget::Component(ComponentTarget::Policy, id),
        "work_queue" => RuntimeTarget::Component(ComponentTarget::Queue, id),
        "rule" => RuntimeTarget::Component(ComponentTarget::Rule, id),
        "trigger" => RuntimeTarget::Component(ComponentTarget::Trigger, id),
        "sensor" => RuntimeTarget::Component(ComponentTarget::Sensor, id),
        "runtime" => RuntimeTarget::Component(ComponentTarget::Runtime, id),
        "dashboard" => RuntimeTarget::Component(ComponentTarget::Dashboard, id),
        "key" => RuntimeTarget::Key(id),
        "artifact" => RuntimeTarget::Artifact(id),
        "cache_namespace" => RuntimeTarget::CacheNamespace(id),
        _ => return Err(Error::validation("Unsupported concrete policy target kind")),
    })
}

pub(crate) fn matches(binding: &GrantTargetBindings, target: &ResolvedRuntimeTarget) -> bool {
    binding
        .targets
        .as_ref()
        .is_none_or(|targets| bound_target(target).is_some_and(|bound| targets.contains(&bound)))
        && binding.owners.as_ref().is_none_or(|owners| {
            let kind = match target.context().owner_type {
                Some(OwnerType::Pack) => "pack",
                Some(OwnerType::Action) => "action",
                Some(OwnerType::Sensor) => "sensor",
                Some(OwnerType::Identity) => "identity",
                _ => return false,
            };
            target
                .owner_id()
                .is_some_and(|id| owners.contains(&(kind.into(), id, target.pack_id())))
        })
        && binding
            .packs
            .as_ref()
            .is_none_or(|packs| target.pack_id().is_some_and(|id| packs.contains(&id)))
}

fn bound_target(target: &ResolvedRuntimeTarget) -> Option<BoundRuntimeTarget> {
    Some(BoundRuntimeTarget {
        kind: target.identity_kind().into(),
        id: target.context().target_id?,
        pack_id: target.pack_id(),
        owner_type: target.context().owner_type.map(|owner| {
            match owner {
                OwnerType::System => "system",
                OwnerType::Identity => "identity",
                OwnerType::Pack => "pack",
                OwnerType::Action => "action",
                OwnerType::Sensor => "sensor",
            }
            .into()
        }),
        owner_id: target.owner_id(),
    })
}

pub(crate) fn push_owner_clause(
    query: &mut sqlx::QueryBuilder<'_, sqlx::Postgres>,
    alias: &str,
    pack_expression: &str,
    owners: &[(String, Id, Option<Id>)],
) {
    query.push("(");
    // The aliases are static repository SQL, not caller or metadata input.
    let mut any = false;
    for (kind, id, pack) in owners {
        let column = match kind.as_str() {
            "pack" => "owner_pack",
            "action" => "owner_action",
            "sensor" => "owner_sensor",
            "identity" => "owner_identity",
            _ => continue,
        };
        if any {
            query.push(" OR ");
        }
        any = true;
        query
            .push(format!("({alias}.owner_type = '"))
            .push(kind.clone())
            .push(format!("' AND {alias}.{column} = "))
            .push_bind(*id)
            .push(" AND ");
        push_pack_identity(query, pack_expression, *pack);
        query.push(")");
    }
    if !any {
        query.push("FALSE");
    }
    query.push(")");
}

fn push_pack_identity(
    query: &mut sqlx::QueryBuilder<'_, sqlx::Postgres>,
    expression: &str,
    pack: Option<Id>,
) {
    query.push(expression);
    match pack {
        Some(id) => {
            query.push(" = ").push_bind(id);
        }
        None => {
            query.push(" IS NULL");
        }
    }
}

pub(crate) fn push_target_clause(
    query: &mut sqlx::QueryBuilder<'_, sqlx::Postgres>,
    alias: &str,
    pack_expression: &str,
    targets: &[BoundRuntimeTarget],
) {
    query.push("(");
    for (index, target) in targets.iter().enumerate() {
        if index > 0 {
            query.push(" OR ");
        }
        query
            .push(format!("({alias}.id = "))
            .push_bind(target.id)
            .push(" AND ");
        push_pack_identity(query, pack_expression, target.pack_id);
        if let Some(kind) = &target.owner_type {
            let column = match kind.as_str() {
                "pack" => Some("owner_pack"),
                "action" => Some("owner_action"),
                "sensor" => Some("owner_sensor"),
                "identity" => Some("owner_identity"),
                "system" => None,
                _ => {
                    query.push(" AND FALSE");
                    None
                }
            };
            query
                .push(format!(" AND {alias}.owner_type::text = "))
                .push_bind(kind.clone());
            if let Some(column) = column {
                query.push(format!(" AND {alias}.{column}"));
                if let Some(id) = target.owner_id {
                    query.push(" = ").push_bind(id);
                } else {
                    query.push(" IS NULL");
                }
            }
        }
        query.push(")");
    }
    if targets.is_empty() {
        query.push("FALSE");
    }
    query.push(")");
}
