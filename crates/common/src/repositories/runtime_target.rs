//! Database-backed target ownership for recorded runtime authority.
//!
//! Keep the transaction open through the effect. IDs, structural ownership and
//! parent rows are locked; ref prefixes and historical display refs never confer
//! local authority. Missing parents retain target context but have no local owner.

use chrono::{DateTime, Utc};
use sqlx::PgConnection;

use crate::{
    models::{ArtifactVisibility, Id, OwnerType},
    rbac::{AuthorizationContext, Resource},
    runtime_authority::positive_id,
    Error, Result,
};

#[derive(Debug, Clone, Copy)]
pub enum ComponentTarget {
    Action,
    Policy,
    Queue,
    Rule,
    Trigger,
    Sensor,
    Runtime,
    Dashboard,
}

impl ComponentTarget {
    fn metadata(self) -> (&'static str, Resource, bool) {
        match self {
            Self::Action => ("action", Resource::Actions, true),
            Self::Policy => ("policy", Resource::Policies, true),
            Self::Queue => ("work_queue", Resource::Queues, true),
            Self::Rule => ("rule", Resource::Rules, true),
            Self::Trigger => ("trigger", Resource::Triggers, true),
            Self::Sensor => ("sensor", Resource::Triggers, true),
            Self::Runtime => ("runtime", Resource::Runtimes, false),
            Self::Dashboard => ("dashboard", Resource::Dashboards, true),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub enum RuntimeTarget {
    Pack(Id),
    Component(ComponentTarget, Id),
    Execution(Id),
    Enforcement(Id),
    QueueItem(Id),
    Inquiry(Id),
    Key(Id),
    Artifact(Id),
    CacheNamespace(Id),
    RuntimeVersion(Id),
    /// Event IDs are not globally unique. Resolve the exact partitioned row.
    Event {
        id: Id,
        created: DateTime<Utc>,
    },
    /// Event submission authorizes a live trigger, not an invented event row.
    EventTrigger(Id),
}

/// Constructed only by repository resolution. Callers cannot supply a pack ID
/// inferred from user-controlled refs and claim it is a resolved owner.
pub struct ResolvedRuntimeTarget {
    resource: Resource,
    pack_id: Option<Id>,
    owner_id: Option<Id>,
    context: AuthorizationContext,
    accepts_new_effects: bool,
}

impl ResolvedRuntimeTarget {
    pub fn resource(&self) -> Resource {
        self.resource
    }
    pub fn pack_id(&self) -> Option<Id> {
        self.pack_id
    }
    pub fn owner_id(&self) -> Option<Id> {
        self.owner_id
    }
    pub fn context(&self) -> &AuthorizationContext {
        &self.context
    }
    pub fn accepts_new_effects(&self) -> bool {
        self.accepts_new_effects
    }
}

pub struct RuntimeTargetRepository;

#[derive(Clone, Copy)]
pub enum ProspectiveDataOwner {
    System,
    Identity(Id),
    Pack(Id),
    Action(Id),
    Sensor(Id),
}

pub enum ProspectiveRuntimeTarget<'a> {
    Key { local_ref: &'a str, encrypted: bool },
    CacheNamespace(&'a str),
}

impl TryFrom<&super::cache::CacheOwnerScope> for ProspectiveDataOwner {
    type Error = Error;

    fn try_from(scope: &super::cache::CacheOwnerScope) -> Result<Self> {
        use crate::models::OwnerType;
        scope.canonical_owner()?;
        let missing = || Error::validation("Prospective data owner requires its canonical ID");
        Ok(match scope.owner_type {
            OwnerType::System => Self::System,
            OwnerType::Identity => Self::Identity(scope.owner_identity.ok_or_else(missing)?),
            OwnerType::Pack => Self::Pack(scope.owner_pack.ok_or_else(missing)?),
            OwnerType::Action => Self::Action(scope.owner_action.ok_or_else(missing)?),
            OwnerType::Sensor => Self::Sensor(scope.owner_sensor.ok_or_else(missing)?),
        })
    }
}

impl RuntimeTargetRepository {
    /// Resolve and lock the actual prospective owner before checking creation
    /// or uniqueness. A proposed ref alone never establishes pack ownership.
    pub async fn resolve_creation_target(
        connection: &mut PgConnection,
        owner: ProspectiveDataOwner,
        target: ProspectiveRuntimeTarget<'_>,
        sponsor_identity: Id,
    ) -> Result<Option<ResolvedRuntimeTarget>> {
        use crate::models::OwnerType;
        positive_id(sponsor_identity)?;
        super::schema_type::SchemaTypeRepository::lock_selection(connection).await?;
        let (kind, owner_id, owner_ref, canonical_owner, pack_id, available) = match owner {
            ProspectiveDataOwner::System => (OwnerType::System, None, None, None, None, true),
            ProspectiveDataOwner::Identity(id) => {
                positive_id(id)?;
                let Some(identity) =
                    super::identity::IdentityRepository::find_by_id_for_share(connection, id)
                        .await?
                else {
                    return Ok(None);
                };
                (
                    OwnerType::Identity,
                    Some(id),
                    Some(id.to_string()),
                    Some(identity.login),
                    None,
                    true,
                )
            }
            ProspectiveDataOwner::Pack(id) => {
                positive_id(id)?;
                let Some(reference) = pack_ref(connection, id).await? else {
                    return Ok(None);
                };
                (
                    OwnerType::Pack,
                    Some(id),
                    Some(reference.clone()),
                    Some(reference),
                    Some(id),
                    true,
                )
            }
            ProspectiveDataOwner::Action(id) | ProspectiveDataOwner::Sensor(id) => {
                let (kind, component_kind) = if matches!(owner, ProspectiveDataOwner::Action(_)) {
                    (OwnerType::Action, ComponentTarget::Action)
                } else {
                    (OwnerType::Sensor, ComponentTarget::Sensor)
                };
                let Some(component) = component(connection, component_kind, id).await? else {
                    return Ok(None);
                };
                (
                    kind,
                    Some(id),
                    Some(component.reference.clone()),
                    Some(component.reference),
                    component.pack,
                    component.owner_available,
                )
            }
        };
        let (resource, reference, encrypted) = match target {
            ProspectiveRuntimeTarget::Key {
                local_ref,
                encrypted,
            } => (
                Resource::Keys,
                crate::key_ref::canonical_key_ref(kind, canonical_owner.as_deref(), local_ref)
                    .map_err(Error::validation)?,
                Some(encrypted),
            ),
            ProspectiveRuntimeTarget::CacheNamespace(namespace) => {
                super::cache::validate_namespace_name(namespace)?;
                (Resource::Caches, namespace.to_string(), None)
            }
        };
        let mut context = AuthorizationContext::new(sponsor_identity);
        context.target_ref = Some(reference);
        context.owner_type = Some(kind);
        context.owner_ref = owner_ref;
        context.owner_identity_id = if kind == OwnerType::Identity {
            owner_id
        } else {
            None
        };
        context.encrypted = encrypted;
        let pack_id = if let Some(pack) = pack_id {
            context.pack_ref = pack_ref(connection, pack).await?;
            context.pack_ref.as_ref().map(|_| pack)
        } else {
            None
        };
        Ok(Some(ResolvedRuntimeTarget {
            resource,
            pack_id,
            owner_id,
            context,
            accepts_new_effects: available,
        }))
    }

    pub async fn resolve(
        connection: &mut PgConnection,
        target: RuntimeTarget,
        sponsor_identity: Id,
    ) -> Result<Option<ResolvedRuntimeTarget>> {
        positive_id(sponsor_identity)?;
        super::schema_type::SchemaTypeRepository::lock_selection(&mut *connection).await?;
        let mut context = AuthorizationContext::new(sponsor_identity);
        let mut owner_id = None;
        let (resource, id, pack_id, available) = match target {
            RuntimeTarget::Pack(id) => {
                positive_id(id)?;
                let Some(reference) = pack_ref(connection, id).await? else {
                    return Ok(None);
                };
                context.target_ref = Some(reference);
                (Resource::Packs, id, Some(id), true)
            }
            RuntimeTarget::Component(kind, id) => {
                let Some(component) = component(connection, kind, id).await? else {
                    return Ok(None);
                };
                context.target_ref = Some(component.reference);
                (kind.metadata().1, id, component.pack, component.available)
            }
            RuntimeTarget::EventTrigger(id) => {
                let Some(component) = component(connection, ComponentTarget::Trigger, id).await?
                else {
                    return Ok(None);
                };
                context.target_ref = Some(component.reference);
                (Resource::Events, id, component.pack, component.available)
            }
            RuntimeTarget::Execution(id) => {
                positive_id(id)?;
                let row: Option<(String, Option<Id>, Option<String>)> = sqlx::query_as(
                    "SELECT action_ref, pack_release, pack_release_digest FROM execution WHERE id = $1 FOR SHARE",
                ).bind(id).fetch_optional(&mut *connection).await?;
                let Some((reference, release, digest)) = row else {
                    return Ok(None);
                };
                context.target_ref = Some(reference);
                (
                    Resource::Executions,
                    id,
                    release_owner(connection, release, digest).await?,
                    true,
                )
            }
            RuntimeTarget::Enforcement(id) => {
                positive_id(id)?;
                let row: Option<(String, Option<Id>)> = sqlx::query_as(
                    "SELECT rule_ref, rule FROM enforcement WHERE id = $1 FOR SHARE",
                )
                .bind(id)
                .fetch_optional(&mut *connection)
                .await?;
                let Some((reference, rule)) = row else {
                    return Ok(None);
                };
                context.target_ref = Some(reference);
                let parent = match rule {
                    Some(rule) => component(connection, ComponentTarget::Rule, rule)
                        .await?
                        .and_then(|parent| parent.pack),
                    None => None,
                };
                (Resource::Enforcements, id, parent, true)
            }
            RuntimeTarget::QueueItem(id) => {
                positive_id(id)?;
                let row: Option<(String, Id)> = sqlx::query_as(
                    "SELECT queue_ref, queue FROM work_queue_item WHERE id = $1 FOR SHARE",
                )
                .bind(id)
                .fetch_optional(&mut *connection)
                .await?;
                let Some((reference, queue)) = row else {
                    return Ok(None);
                };
                context.target_ref = Some(reference);
                let parent = component(connection, ComponentTarget::Queue, queue)
                    .await?
                    .and_then(|parent| parent.pack);
                (Resource::QueueItems, id, parent, true)
            }
            RuntimeTarget::Inquiry(id) => {
                positive_id(id)?;
                let execution: Option<Option<Id>> = sqlx::query_scalar(
                    "SELECT created_by_execution FROM inquiry WHERE id = $1 FOR SHARE",
                )
                .bind(id)
                .fetch_optional(&mut *connection)
                .await?;
                let Some(execution) = execution else {
                    return Ok(None);
                };
                let parent = match execution {
                    Some(execution) => {
                        let row: Option<(Option<Id>, Option<String>)> = sqlx::query_as(
                            "SELECT pack_release, pack_release_digest FROM execution WHERE id = $1 FOR SHARE",
                        ).bind(execution).fetch_optional(&mut *connection).await?;
                        match row {
                            Some((release, digest)) => {
                                release_owner(connection, release, digest).await?
                            }
                            None => None,
                        }
                    }
                    None => None,
                };
                (Resource::Inquiries, id, parent, true)
            }
            RuntimeTarget::Key(id) => {
                positive_id(id)?;
                let row: Option<OwnerRow> = sqlx::query_as(
                    "SELECT ref AS reference, owner_type, owner_identity, owner_pack, owner_action, owner_sensor, \
                     owner_pack_ref, owner_action_ref, owner_sensor_ref, encrypted \
                     FROM key WHERE id = $1 FOR SHARE",
                ).bind(id).fetch_optional(&mut *connection).await?;
                let Some(row) = row else {
                    return Ok(None);
                };
                owner_context(&mut context, &row);
                owner_id = match row.owner_type {
                    OwnerType::Pack => row.owner_pack,
                    OwnerType::Action => row.owner_action,
                    OwnerType::Sensor => row.owner_sensor,
                    OwnerType::Identity => row.owner_identity,
                    OwnerType::System => None,
                };
                context.encrypted = Some(row.encrypted);
                let parent = owner_pack(
                    connection,
                    row.owner_type,
                    row.owner_pack,
                    row.owner_action,
                    row.owner_sensor,
                )
                .await?;
                (Resource::Keys, id, parent, true)
            }
            RuntimeTarget::CacheNamespace(id) => {
                positive_id(id)?;
                let row: Option<CacheOwnerRow> = sqlx::query_as(
                    "SELECT COALESCE(definition_ref, namespace) AS reference, owner_type, owner_identity, \
                     owner_pack, owner_action, owner_sensor, owner_pack_ref, owner_action_ref, owner_sensor_ref, \
                     false AS encrypted, managing_pack, management_origin = 'pack' AS managed, \
                     retired_at IS NULL AND tombstoned_at IS NULL AS available \
                     FROM cache_namespace WHERE id = $1 FOR SHARE",
                ).bind(id).fetch_optional(&mut *connection).await?;
                let Some(row) = row else {
                    return Ok(None);
                };
                owner_context(&mut context, &row.owner);
                owner_id = match row.owner.owner_type {
                    OwnerType::Pack => row.owner.owner_pack,
                    OwnerType::Action => row.owner.owner_action,
                    OwnerType::Sensor => row.owner.owner_sensor,
                    OwnerType::Identity => row.owner.owner_identity,
                    OwnerType::System => None,
                };
                let data_owner = owner_pack(
                    connection,
                    row.owner.owner_type,
                    row.owner.owner_pack,
                    row.owner.owner_action,
                    row.owner.owner_sensor,
                )
                .await?;
                // Management of a definition is not ownership of its data. Both
                // must agree for automatic pack-local cache authority.
                let parent = if !row.managed || row.managing_pack == data_owner {
                    data_owner
                } else {
                    None
                };
                (Resource::Caches, id, parent, row.available)
            }
            RuntimeTarget::Artifact(id) => {
                positive_id(id)?;
                let row: Option<(String, OwnerType, String, ArtifactVisibility)> = sqlx::query_as(
                    "SELECT ref, scope, owner, visibility FROM artifact WHERE id = $1 FOR SHARE",
                )
                .bind(id)
                .fetch_optional(&mut *connection)
                .await?;
                let Some((reference, scope, owner, visibility)) = row else {
                    return Ok(None);
                };
                context.target_ref = Some(reference);
                context.owner_type = Some(scope);
                context.owner_ref = Some(owner.clone());
                context.visibility = Some(visibility);
                let parent = match scope {
                    OwnerType::Pack => {
                        let pack =
                            sqlx::query_scalar("SELECT id FROM pack WHERE ref = $1 FOR SHARE")
                                .bind(&owner)
                                .fetch_optional(&mut *connection)
                                .await?;
                        owner_id = pack;
                        pack
                    }
                    OwnerType::Action | OwnerType::Sensor => {
                        let table = if scope == OwnerType::Action {
                            "action"
                        } else {
                            "sensor"
                        };
                        let query = format!("SELECT id, CASE WHEN management_origin = 'pack' THEN pack ELSE NULL END FROM {table} WHERE ref = $1 FOR SHARE");
                        let component: Option<(Id, Option<Id>)> = sqlx::query_as(&query)
                            .bind(&owner)
                            .fetch_optional(&mut *connection)
                            .await?;
                        owner_id = component.as_ref().map(|(id, _)| *id);
                        component.and_then(|(_, pack)| pack)
                    }
                    OwnerType::Identity => {
                        context.owner_identity_id = owner.parse::<Id>().ok();
                        None
                    }
                    OwnerType::System => None,
                };
                (Resource::Artifacts, id, parent, true)
            }
            RuntimeTarget::RuntimeVersion(id) => {
                positive_id(id)?;
                let row: Option<(Id, bool)> = sqlx::query_as(
                    "SELECT runtime, retired_at IS NULL FROM runtime_version WHERE id = $1 FOR SHARE",
                ).bind(id).fetch_optional(&mut *connection).await?;
                let Some((runtime, available)) = row else {
                    return Ok(None);
                };
                let parent = component(connection, ComponentTarget::Runtime, runtime).await?;
                if let Some(parent) = &parent {
                    context.target_ref = Some(parent.reference.clone());
                }
                let active = available && parent.as_ref().is_some_and(|parent| parent.available);
                (
                    Resource::Runtimes,
                    id,
                    parent.and_then(|parent| parent.pack),
                    active,
                )
            }
            RuntimeTarget::Event { id, created } => {
                positive_id(id)?;
                let row: Option<(String, Option<Id>)> = sqlx::query_as(
                    "SELECT trigger_ref, trigger FROM event WHERE id = $1 AND created = $2 FOR SHARE",
                ).bind(id).bind(created).fetch_optional(&mut *connection).await?;
                let Some((reference, trigger)) = row else {
                    return Ok(None);
                };
                context.target_ref = Some(reference);
                let parent = match trigger {
                    Some(trigger) => component(connection, ComponentTarget::Trigger, trigger)
                        .await?
                        .and_then(|parent| parent.pack),
                    None => None,
                };
                (Resource::Events, id, parent, true)
            }
        };
        context.target_id = Some(id);
        let pack_id = match pack_id {
            Some(id) => match pack_ref(connection, id).await? {
                Some(reference) => {
                    context.pack_ref = Some(reference);
                    Some(id)
                }
                None => None,
            },
            None => None,
        };
        Ok(Some(ResolvedRuntimeTarget {
            resource,
            pack_id,
            owner_id,
            context,
            accepts_new_effects: available,
        }))
    }
}

#[derive(sqlx::FromRow)]
struct ComponentRow {
    reference: String,
    pack: Option<Id>,
    available: bool,
    owner_available: bool,
}

async fn component(
    connection: &mut PgConnection,
    kind: ComponentTarget,
    id: Id,
) -> Result<Option<ComponentRow>> {
    positive_id(id)?;
    let (table, _, enabled) = kind.metadata();
    let active = if enabled {
        "retired_at IS NULL AND effective_enabled"
    } else {
        "retired_at IS NULL"
    };
    let query = format!("SELECT ref AS reference, CASE WHEN management_origin = 'pack' THEN pack ELSE NULL END AS pack, \
        {active} AS available, retired_at IS NULL AS owner_available FROM {table} WHERE id = $1 FOR SHARE");
    Ok(sqlx::query_as(&query)
        .bind(id)
        .fetch_optional(connection)
        .await?)
}

async fn pack_ref(connection: &mut PgConnection, id: Id) -> Result<Option<String>> {
    Ok(
        sqlx::query_scalar("SELECT ref FROM pack WHERE id = $1 FOR SHARE")
            .bind(id)
            .fetch_optional(connection)
            .await?,
    )
}

async fn release_owner(
    connection: &mut PgConnection,
    release: Option<Id>,
    digest: Option<String>,
) -> Result<Option<Id>> {
    let (Some(release), Some(digest)) = (release, digest) else {
        return Ok(None);
    };
    Ok(
        sqlx::query_scalar("SELECT pack FROM pack_release WHERE id = $1 AND digest = $2 FOR SHARE")
            .bind(release)
            .bind(digest)
            .fetch_optional(connection)
            .await?,
    )
}

#[derive(sqlx::FromRow)]
struct OwnerRow {
    reference: String,
    owner_type: OwnerType,
    owner_identity: Option<Id>,
    owner_pack: Option<Id>,
    owner_action: Option<Id>,
    owner_sensor: Option<Id>,
    owner_pack_ref: Option<String>,
    owner_action_ref: Option<String>,
    owner_sensor_ref: Option<String>,
    encrypted: bool,
}

#[derive(sqlx::FromRow)]
struct CacheOwnerRow {
    #[sqlx(flatten)]
    owner: OwnerRow,
    managing_pack: Option<Id>,
    managed: bool,
    available: bool,
}

fn owner_context(context: &mut AuthorizationContext, row: &OwnerRow) {
    context.target_ref = Some(row.reference.clone());
    context.owner_type = Some(row.owner_type);
    context.owner_identity_id = row.owner_identity;
    context.owner_ref = match row.owner_type {
        OwnerType::Pack => row.owner_pack_ref.clone(),
        OwnerType::Action => row.owner_action_ref.clone(),
        OwnerType::Sensor => row.owner_sensor_ref.clone(),
        OwnerType::Identity => row.owner_identity.map(|id| id.to_string()),
        OwnerType::System => None,
    };
}

async fn owner_pack(
    connection: &mut PgConnection,
    kind: OwnerType,
    pack: Option<Id>,
    action: Option<Id>,
    sensor: Option<Id>,
) -> Result<Option<Id>> {
    match kind {
        OwnerType::Pack => Ok(pack),
        OwnerType::Action => match action {
            Some(id) => Ok(component(connection, ComponentTarget::Action, id)
                .await?
                .and_then(|parent| parent.pack)),
            None => Ok(None),
        },
        OwnerType::Sensor => match sensor {
            Some(id) => Ok(component(connection, ComponentTarget::Sensor, id)
                .await?
                .and_then(|parent| parent.pack)),
            None => Ok(None),
        },
        OwnerType::System | OwnerType::Identity => Ok(None),
    }
}
