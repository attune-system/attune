use serde::Deserialize;
use sqlx::postgres::{PgListener, PgPool};
use tracing::{debug, error, info, warn};

use crate::metadata_cache::{
    repositories::CachedMetadataRepository, MetadataCache, MetadataEntity,
};
use crate::repositories::{
    action::{ActionRepository, PolicyRepository},
    identity::PermissionSetRepository,
    rule::RuleRepository,
    runtime::RuntimeRepository,
    runtime_version::RuntimeVersionRepository,
    trigger::{SensorRepository, TriggerRepository},
    work_queue::WorkQueueRepository,
    workflow::WorkflowDefinitionRepository,
    FindById,
};
use crate::Result;

const METADATA_CHANGED_CHANNELS: &[&str] = &["metadata_changed"];

#[derive(Debug, Deserialize, Default)]
struct MetadataChangeEvent {
    entity: String,
    operation: String,
    id: i64,
    #[serde(default, rename = "ref")]
    ref_name: Option<String>,
    #[serde(default)]
    pack: Option<i64>,
    #[serde(default)]
    enabled: Option<bool>,
    #[serde(default)]
    action: Option<i64>,
    #[serde(default)]
    trigger: Option<i64>,
    #[serde(default)]
    sensor: Option<i64>,
    #[serde(default)]
    old_ref: Option<String>,
    #[serde(default)]
    old_pack: Option<i64>,
    #[serde(default)]
    old_enabled: Option<bool>,
    #[serde(default)]
    old_action: Option<i64>,
    #[serde(default)]
    old_trigger: Option<i64>,
    #[serde(default)]
    old_sensor: Option<i64>,
    #[serde(default)]
    workflow_def: Option<i64>,
    #[serde(default)]
    old_workflow_def: Option<i64>,
    #[serde(default)]
    runtime: Option<i64>,
    #[serde(default)]
    old_runtime: Option<i64>,
    #[serde(default)]
    runtime_ref: Option<String>,
    #[serde(default)]
    old_runtime_ref: Option<String>,
    #[serde(default)]
    version: Option<String>,
    #[serde(default)]
    old_version: Option<String>,
    #[serde(default)]
    webhook_key: Option<String>,
    #[serde(default)]
    old_webhook_key: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    old_name: Option<String>,
    #[serde(default)]
    aliases: Vec<String>,
    #[serde(default)]
    old_aliases: Vec<String>,
    #[serde(default)]
    available: Option<bool>,
    #[serde(default)]
    old_available: Option<bool>,
    #[serde(default)]
    accepting_new_items: Option<bool>,
    #[serde(default)]
    old_accepting_new_items: Option<bool>,
    #[serde(default)]
    identity: Option<i64>,
    #[serde(default)]
    old_identity: Option<i64>,
    #[serde(default)]
    role: Option<String>,
    #[serde(default)]
    old_role: Option<String>,
}

pub async fn start_metadata_cache_sync(db: PgPool, cache: MetadataCache) -> Result<()> {
    if !cache.is_enabled() {
        return Ok(());
    }

    info!(
        "Starting metadata cache sync listener on channels: {:?}",
        METADATA_CHANGED_CHANNELS
    );

    let mut listener = PgListener::connect_with(&db).await?;
    listener
        .listen_all(METADATA_CHANGED_CHANNELS.iter().copied())
        .await?;

    loop {
        match listener.recv().await {
            Ok(notification) => {
                if let Err(e) = process_notification(&db, &cache, notification.payload()).await {
                    error!("Failed to process metadata cache notification: {e}");
                }
            }
            Err(e) => {
                error!("Metadata cache listener receive error: {e}");
                warn!("Attempting to reconnect metadata cache listener...");

                loop {
                    match PgListener::connect_with(&db).await {
                        Ok(mut new_listener) => {
                            match new_listener
                                .listen_all(METADATA_CHANGED_CHANNELS.iter().copied())
                                .await
                            {
                                Ok(_) => {
                                    listener = new_listener;
                                    info!("Reconnected metadata cache listener");
                                    break;
                                }
                                Err(err) => {
                                    error!(
                                        "Failed to resubscribe metadata cache listener after reconnect: {err}"
                                    );
                                }
                            }
                        }
                        Err(err) => {
                            error!("Failed to reconnect metadata cache listener: {err}");
                        }
                    }
                    tokio::time::sleep(tokio::time::Duration::from_secs(5)).await;
                }
            }
        }
    }
}

async fn process_notification(db: &PgPool, cache: &MetadataCache, payload: &str) -> Result<()> {
    let event: MetadataChangeEvent = serde_json::from_str(payload)?;
    if process_permission_assignment_notification(cache, &event).await? {
        return Ok(());
    }

    let Some(entity) = parse_entity(&event.entity) else {
        warn!(
            "Ignoring metadata cache notification for unknown entity '{}'",
            event.entity
        );
        return Ok(());
    };

    if !cache.entity_enabled(entity) {
        return Ok(());
    }

    async fn process_permission_assignment_notification(
        cache: &MetadataCache,
        event: &MetadataChangeEvent,
    ) -> Result<bool> {
        if !cache.entity_enabled(MetadataEntity::PermissionSet) {
            return Ok(matches!(
                event.entity.as_str(),
                "permission_assignment" | "permission_set_role_assignment"
            ));
        }

        let indexes = match event.entity.as_str() {
            "permission_assignment" => event
                .old_identity
                .or(event.identity)
                .map(|identity_id| vec![format!("identity:{identity_id}:refs")])
                .unwrap_or_default(),
            "permission_set_role_assignment" => event
                .old_role
                .as_deref()
                .or(event.role.as_deref())
                .map(|role| vec![format!("role:{role}:refs")])
                .unwrap_or_default(),
            _ => return Ok(false),
        };

        for index in indexes {
            let keys = [
                cache.index_key(MetadataEntity::PermissionSet, &index),
                cache.empty_index_key(MetadataEntity::PermissionSet, &index),
            ]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>();
            if let Err(e) = cache.delete_keys(keys).await {
                cache.log_best_effort_error("metadata.sync.permission_assignment_index", &e);
            }
        }

        Ok(true)
    }

    debug!(
        entity = %event.entity,
        operation = %event.operation,
        id = event.id,
        "Processing metadata cache notification"
    );

    match event.operation.as_str() {
        "INSERT" => refresh_current_entity(db, cache, entity, event.id).await?,
        "UPDATE" => {
            evict_previous_entity(cache, entity, &event).await;
            refresh_current_entity(db, cache, entity, event.id).await?;
        }
        "DELETE" => {
            evict_previous_entity(cache, entity, &event).await;
        }
        other => {
            warn!(
                "Ignoring metadata cache notification with unsupported operation '{}'",
                other
            );
        }
    }

    Ok(())
}

async fn refresh_current_entity(
    db: &PgPool,
    cache: &MetadataCache,
    entity: MetadataEntity,
    id: i64,
) -> Result<()> {
    let cached = CachedMetadataRepository::new(db, cache);

    match entity {
        MetadataEntity::Action => {
            if let Some(row) = ActionRepository::find_by_id(db, id).await? {
                cached.put_action_best_effort(&row).await;
            }
        }
        MetadataEntity::Rule => {
            if let Some(row) = RuleRepository::find_by_id(db, id).await? {
                cached.put_rule_best_effort(&row).await;
            }
        }
        MetadataEntity::Trigger => {
            if let Some(row) = TriggerRepository::find_by_id(db, id).await? {
                cached.put_trigger_best_effort(&row).await;
            }
        }
        MetadataEntity::Sensor => {
            if let Some(row) = SensorRepository::find_by_id(db, id).await? {
                cached.put_sensor_best_effort(&row).await;
            }
        }
        MetadataEntity::WorkQueue => {
            if let Some(row) = WorkQueueRepository::find_by_id(db, id).await? {
                cached.put_work_queue_best_effort(&row).await;
            }
        }
        MetadataEntity::WorkflowDefinition => {
            if let Some(row) = WorkflowDefinitionRepository::find_by_id(db, id).await? {
                cached.put_workflow_definition_best_effort(&row).await;
            }
        }
        MetadataEntity::Policy => {
            if let Some(row) = PolicyRepository::find_by_id(db, id).await? {
                cached.put_policy_best_effort(&row).await;
            }
        }
        MetadataEntity::PermissionSet => {
            if let Some(row) = PermissionSetRepository::find_by_id(db, id).await? {
                cached.put_permission_set_best_effort(&row).await;
            }
        }
        MetadataEntity::Runtime => {
            if let Some(row) = RuntimeRepository::find_by_id(db, id).await? {
                cached.put_runtime_best_effort(&row).await;
            }
        }
        MetadataEntity::RuntimeVersion => {
            if let Some(row) = RuntimeVersionRepository::find_by_id(db, id).await? {
                cached.put_runtime_version_best_effort(&row).await;
            }
        }
    }

    Ok(())
}

async fn evict_previous_entity(
    cache: &MetadataCache,
    entity: MetadataEntity,
    event: &MetadataChangeEvent,
) {
    let derived_runtime_version_ref = runtime_version_event_ref(
        event.old_runtime_ref.as_deref(),
        event.old_version.as_deref(),
    )
    .or_else(|| runtime_version_event_ref(event.runtime_ref.as_deref(), event.version.as_deref()));
    let ref_name = event
        .old_ref
        .as_deref()
        .or(event.ref_name.as_deref())
        .or(derived_runtime_version_ref.as_deref());
    let keys = [
        cache.key_for_id(entity, event.id),
        ref_name.and_then(|value| cache.key_for_ref(entity, value)),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>();

    if let Err(e) = cache.delete_keys(keys).await {
        cache.log_best_effort_error("metadata.sync.delete_keys", &e);
    }

    let Some(ref_name) = ref_name else {
        return;
    };

    let mut indexes = previous_indexes(entity, event);
    indexes.push("all_refs".to_string());
    for index in indexes {
        if let Some(index_key) = cache.index_key(entity, &index) {
            if let Err(e) = cache.remove_index_members(&index_key, [ref_name]).await {
                cache.log_best_effort_error("metadata.sync.remove_index", &e);
            }
        }
    }
}

fn previous_indexes(entity: MetadataEntity, event: &MetadataChangeEvent) -> Vec<String> {
    match entity {
        MetadataEntity::Action => {
            let mut indexes = pack_indexes(event.old_pack.or(event.pack));
            if let Some(workflow_def_id) = event.old_workflow_def.or(event.workflow_def) {
                indexes.push(format!("workflow_def:{workflow_def_id}:refs"));
            }
            indexes
        }
        MetadataEntity::Rule => {
            let mut indexes = pack_indexes(event.old_pack.or(event.pack));
            if event.old_enabled.or(event.enabled).unwrap_or(false) {
                indexes.push("enabled_refs".to_string());
            }
            if let Some(action_id) = event.old_action.or(event.action) {
                indexes.push(format!("action:{action_id}:refs"));
            }
            if let Some(trigger_id) = event.old_trigger.or(event.trigger) {
                indexes.push(format!("trigger:{trigger_id}:refs"));
            }
            indexes
        }
        MetadataEntity::Trigger | MetadataEntity::Sensor => {
            let mut indexes = pack_indexes(event.old_pack.or(event.pack));
            if event.old_enabled.or(event.enabled).unwrap_or(false) {
                indexes.push("enabled_refs".to_string());
            }
            if matches!(entity, MetadataEntity::Trigger) {
                if let Some(sensor_id) = event.old_sensor.or(event.sensor) {
                    indexes.push(format!("sensor:{sensor_id}:refs"));
                }
                if let Some(webhook_key) = event
                    .old_webhook_key
                    .as_deref()
                    .or(event.webhook_key.as_deref())
                    .filter(|value| !value.is_empty())
                {
                    indexes.push(format!("webhook_key:{webhook_key}:refs"));
                }
            }
            indexes
        }
        MetadataEntity::WorkQueue => {
            let mut indexes = pack_indexes(event.old_pack.or(event.pack));
            if event.old_enabled.or(event.enabled).unwrap_or(false) {
                indexes.push("enabled_refs".to_string());
            }
            if event
                .old_accepting_new_items
                .or(event.accepting_new_items)
                .unwrap_or(false)
            {
                indexes.push("accepting_new_items_refs".to_string());
            }
            indexes
        }
        MetadataEntity::WorkflowDefinition | MetadataEntity::PermissionSet => {
            pack_indexes(event.old_pack.or(event.pack))
        }
        MetadataEntity::Policy => {
            let mut indexes = pack_indexes(event.old_pack.or(event.pack));
            if let Some(action_id) = event.old_action.or(event.action) {
                indexes.push(format!("action:{action_id}:refs"));
            }
            if event.old_pack.or(event.pack).is_none()
                && event.old_action.or(event.action).is_none()
            {
                indexes.push("global_refs".to_string());
            }
            indexes
        }
        MetadataEntity::Runtime => {
            let mut indexes = pack_indexes(event.old_pack.or(event.pack));
            if let Some(name) = event.old_name.as_deref().or(event.name.as_deref()) {
                indexes.push(format!("name:{}:refs", normalized_index_value(name)));
            }
            let aliases = if event.old_aliases.is_empty() {
                &event.aliases
            } else {
                &event.old_aliases
            };
            for alias in aliases {
                indexes.push(format!("alias:{}:refs", normalized_index_value(alias)));
            }
            indexes
        }
        MetadataEntity::RuntimeVersion => {
            let mut indexes = Vec::new();
            if let Some(runtime_id) = event.old_runtime.or(event.runtime) {
                indexes.push(format!("runtime:{runtime_id}:refs"));
                indexes.push(format!("default_runtime:{runtime_id}:refs"));
            }
            if let Some(runtime_ref) = event
                .old_runtime_ref
                .as_deref()
                .or(event.runtime_ref.as_deref())
            {
                indexes.push(format!("runtime_ref:{runtime_ref}:refs"));
            }
            if event.old_available.or(event.available).unwrap_or(false) {
                indexes.push("available_refs".to_string());
            }
            indexes
        }
    }
}

fn pack_indexes(pack_id: Option<i64>) -> Vec<String> {
    pack_id
        .map(|pack_id| vec![format!("pack:{pack_id}:refs")])
        .unwrap_or_default()
}

fn runtime_version_event_ref(runtime_ref: Option<&str>, version: Option<&str>) -> Option<String> {
    Some(format!("{}:{}", runtime_ref?, version?))
}

fn normalized_index_value(value: &str) -> String {
    value.trim().to_ascii_lowercase()
}

fn parse_entity(entity: &str) -> Option<MetadataEntity> {
    match entity {
        "action" => Some(MetadataEntity::Action),
        "rule" => Some(MetadataEntity::Rule),
        "trigger" => Some(MetadataEntity::Trigger),
        "sensor" => Some(MetadataEntity::Sensor),
        "work_queue" => Some(MetadataEntity::WorkQueue),
        "workflow_definition" => Some(MetadataEntity::WorkflowDefinition),
        "policy" => Some(MetadataEntity::Policy),
        "permission_set" => Some(MetadataEntity::PermissionSet),
        "runtime" => Some(MetadataEntity::Runtime),
        "runtime_version" => Some(MetadataEntity::RuntimeVersion),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_entity_maps_supported_tables() {
        assert_eq!(parse_entity("action"), Some(MetadataEntity::Action));
        assert_eq!(
            parse_entity("workflow_definition"),
            Some(MetadataEntity::WorkflowDefinition)
        );
        assert_eq!(
            parse_entity("permission_set"),
            Some(MetadataEntity::PermissionSet)
        );
        assert_eq!(parse_entity("unknown_table"), None);
    }

    #[test]
    fn previous_indexes_include_trigger_sensor_scope() {
        let event = MetadataChangeEvent {
            entity: "trigger".to_string(),
            operation: "UPDATE".to_string(),
            id: 1,
            ref_name: Some("core.tick".to_string()),
            pack: Some(2),
            enabled: Some(true),
            action: None,
            trigger: None,
            sensor: Some(9),
            old_ref: None,
            old_pack: None,
            old_enabled: None,
            old_action: None,
            old_trigger: None,
            old_sensor: None,
            workflow_def: None,
            old_workflow_def: None,
            ..Default::default()
        };

        let indexes = previous_indexes(MetadataEntity::Trigger, &event);
        assert!(indexes.contains(&"pack:2:refs".to_string()));
        assert!(indexes.contains(&"enabled_refs".to_string()));
        assert!(indexes.contains(&"sensor:9:refs".to_string()));
    }
}
