//! Shared work queue definition parsing and validation helpers.

use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

use crate::{
    models::{
        ActionReferenceVisibility, WorkQueueBatchMode, WorkQueueConfig, WorkQueueTunableSource,
        WorkQueueTunableValue, WorkQueueUpdateStrategy,
    },
    schema::RefValidator,
    Error, Result,
};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct WorkQueueDefinition {
    #[serde(rename = "ref")]
    pub r#ref: String,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_true")]
    pub accepting_new_items: bool,
    pub dispatch_action: String,
    #[serde(default)]
    pub default_priority: i32,
    #[serde(default)]
    pub allow_pending_update: bool,
    #[serde(default)]
    pub update_strategy: WorkQueueUpdateStrategy,
    #[serde(default)]
    pub batch_mode: WorkQueueBatchMode,
    #[serde(default = "default_item_schema")]
    pub item_schema: JsonValue,
    #[serde(default = "default_action_params")]
    pub action_params: JsonValue,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trace_tag_template: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permission_set_refs: Option<Vec<String>>,
    #[serde(default = "default_config")]
    pub config: JsonValue,
    #[serde(default)]
    pub reference_visibility: ActionReferenceVisibility,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reference_allowed_pack_refs: Vec<String>,
}

fn default_true() -> bool {
    true
}

fn default_config() -> JsonValue {
    serde_json::json!({})
}

fn default_action_params() -> JsonValue {
    serde_json::json!({})
}

fn default_item_schema() -> JsonValue {
    serde_json::json!({})
}

pub fn parse_work_queue_definition_yaml(content: &str) -> Result<WorkQueueDefinition> {
    let definition: WorkQueueDefinition = serde_yaml_ng::from_str(content).map_err(|e| {
        Error::validation(format!("Failed to parse work queue YAML definition: {}", e))
    })?;
    validate_work_queue_definition(&definition)?;
    Ok(definition)
}

pub fn validate_work_queue_definition(definition: &WorkQueueDefinition) -> Result<WorkQueueConfig> {
    RefValidator::validate_work_queue_ref(&definition.r#ref)?;
    RefValidator::validate_component_ref(&definition.dispatch_action)?;
    validate_work_queue_item_schema(&definition.item_schema)?;
    validate_work_queue_action_params(&definition.action_params)?;
    validate_permission_set_refs(definition.permission_set_refs.as_deref())?;
    validate_trace_tag_template(definition.trace_tag_template.as_deref())?;
    validate_queue_reference_visibility_config(
        definition.reference_visibility,
        &definition.reference_allowed_pack_refs,
    )?;

    if definition.label.trim().is_empty() {
        return Err(Error::validation("Work queue label cannot be empty"));
    }

    if definition
        .description
        .as_ref()
        .is_some_and(|value| value.trim().is_empty())
    {
        return Err(Error::validation(
            "Work queue description cannot be an empty string",
        ));
    }

    validate_work_queue_config_for_batch_mode(definition.batch_mode, &definition.config)
}

pub fn validate_queue_reference_visibility_config(
    visibility: ActionReferenceVisibility,
    allowed_pack_refs: &[String],
) -> Result<()> {
    for pack_ref in allowed_pack_refs {
        RefValidator::validate_pack_ref(pack_ref)?;
    }

    if visibility != ActionReferenceVisibility::Restricted && !allowed_pack_refs.is_empty() {
        return Err(Error::validation(
            "reference_allowed_pack_refs may only be set when reference_visibility is restricted",
        ));
    }

    Ok(())
}

fn validate_permission_set_refs(permission_set_refs: Option<&[String]>) -> Result<()> {
    let Some(permission_set_refs) = permission_set_refs else {
        return Ok(());
    };

    for permission_set_ref in permission_set_refs {
        if permission_set_ref.trim().is_empty() {
            return Err(Error::validation(
                "permission_set_refs cannot contain empty refs",
            ));
        }
    }
    Ok(())
}

fn validate_trace_tag_template(trace_tag_template: Option<&str>) -> Result<()> {
    let Some(trace_tag_template) = trace_tag_template else {
        return Ok(());
    };

    if trace_tag_template.trim().is_empty() {
        return Err(Error::validation("trace_tag_template cannot be empty"));
    }

    Ok(())
}

pub fn validate_work_queue_item_schema(item_schema: &JsonValue) -> Result<()> {
    if !item_schema.is_object() {
        return Err(Error::validation(
            "item_schema must be a JSON object using the flat trigger-style schema format",
        ));
    }
    Ok(())
}

pub fn validate_work_queue_action_params(action_params: &JsonValue) -> Result<()> {
    if !action_params.is_object() {
        return Err(Error::validation(
            "action_params must be a JSON object mapping action parameter names to values",
        ));
    }
    Ok(())
}

pub fn validate_work_queue_config(config: &JsonValue) -> Result<WorkQueueConfig> {
    let config: WorkQueueConfig = serde_json::from_value(config.clone())
        .map_err(|e| Error::validation(format!("Invalid work queue config structure: {}", e)))?;

    if let Some(dispatch) = &config.dispatch {
        if let Some(concurrency) = &dispatch.concurrency {
            validate_tunable_value("config.dispatch.concurrency", concurrency)?;
        }
        if let Some(batch_size) = &dispatch.batch_size {
            validate_tunable_value("config.dispatch.batch_size", batch_size)?;
        }
    }

    if let Some(ack_contract) = &config.ack_contract {
        if ack_contract.version < 1 {
            return Err(Error::validation(
                "config.ack_contract.version must be >= 1",
            ));
        }
    }

    Ok(config)
}

pub fn validate_work_queue_config_for_batch_mode(
    batch_mode: WorkQueueBatchMode,
    config: &JsonValue,
) -> Result<WorkQueueConfig> {
    let config = validate_work_queue_config(config)?;
    validate_work_queue_batch_settings(batch_mode, &config)?;
    Ok(config)
}

pub fn validate_work_queue_batch_settings(
    batch_mode: WorkQueueBatchMode,
    config: &WorkQueueConfig,
) -> Result<()> {
    let Some(coalescing) = config
        .dispatch
        .as_ref()
        .and_then(|dispatch| dispatch.coalescing.as_ref())
    else {
        return Ok(());
    };

    if batch_mode != WorkQueueBatchMode::Batch {
        return Err(Error::validation(
            "config.dispatch.coalescing is only supported when batch_mode is 'batch'",
        ));
    }

    if let Some(group_by_path) = coalescing.group_by_path.as_ref() {
        if group_by_path.trim().is_empty() {
            return Err(Error::validation(
                "config.dispatch.coalescing.group_by_path cannot be empty",
            ));
        }

        if group_by_path
            .split('.')
            .any(|segment| segment.trim().is_empty())
        {
            return Err(Error::validation(
                "config.dispatch.coalescing.group_by_path must use non-empty dot-separated segments",
            ));
        }
    }

    if coalescing.enabled
        && coalescing
            .group_by_path
            .as_ref()
            .is_none_or(|path| path.trim().is_empty())
    {
        return Err(Error::validation(
            "config.dispatch.coalescing.group_by_path is required when coalescing is enabled",
        ));
    }

    Ok(())
}

fn validate_tunable_value(path: &str, value: &WorkQueueTunableValue) -> Result<()> {
    match value.source {
        WorkQueueTunableSource::Literal => {
            if value.value.is_none() {
                return Err(Error::validation(format!(
                    "{path} requires a literal value"
                )));
            }
            if value.path.is_some() {
                return Err(Error::validation(format!(
                    "{path} cannot set 'path' when source=literal"
                )));
            }
            if value.key_ref.is_some() {
                return Err(Error::validation(format!(
                    "{path} cannot set 'key_ref' when source=literal"
                )));
            }
        }
        WorkQueueTunableSource::PackConfig => {
            if value.value.is_some() {
                return Err(Error::validation(format!(
                    "{path} cannot set 'value' when source=pack_config"
                )));
            }
            if value.key_ref.is_some() {
                return Err(Error::validation(format!(
                    "{path} cannot set 'key_ref' when source=pack_config"
                )));
            }
            if value
                .path
                .as_ref()
                .is_none_or(|path| path.trim().is_empty())
            {
                return Err(Error::validation(format!(
                    "{path} requires a non-empty 'path' when source=pack_config"
                )));
            }
        }
        WorkQueueTunableSource::Keystore => {
            if value.value.is_some() {
                return Err(Error::validation(format!(
                    "{path} cannot set 'value' when source=keystore"
                )));
            }
            if value
                .key_ref
                .as_ref()
                .is_none_or(|key_ref| key_ref.trim().is_empty())
            {
                return Err(Error::validation(format!(
                    "{path} requires a non-empty 'key_ref' when source=keystore"
                )));
            }
            if value
                .path
                .as_ref()
                .is_some_and(|path| path.trim().is_empty())
            {
                return Err(Error::validation(format!(
                    "{path}.path cannot be an empty string"
                )));
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{
        parse_work_queue_definition_yaml, validate_work_queue_action_params,
        validate_work_queue_batch_settings, validate_work_queue_config,
        validate_work_queue_config_for_batch_mode, validate_work_queue_item_schema,
    };
    use crate::models::{WorkQueueBatchMode, WorkQueueUpdateStrategy};

    #[test]
    fn parses_valid_work_queue_definition() {
        let definition = parse_work_queue_definition_yaml(
            r#"
ref: core.inbox
label: Core Inbox
accepting_new_items: false
dispatch_action: core.process_item
allow_pending_update: true
update_strategy: merge_patch
batch_mode: batch
permission_set_refs:
  - standard
  - queue_dispatch
item_schema:
  order_id:
    type: integer
    required: true
action_params:
  items: "{{ items }}"
  queue: "{{ queue }}"
config:
  dispatch:
    concurrency:
      source: literal
      value: 5
"#,
        )
        .expect("queue definition should parse");

        assert_eq!(definition.r#ref, "core.inbox");
        assert_eq!(definition.dispatch_action, "core.process_item");
        assert!(!definition.accepting_new_items);
        assert_eq!(definition.batch_mode, WorkQueueBatchMode::Batch);
        assert_eq!(
            definition.update_strategy,
            WorkQueueUpdateStrategy::MergePatch
        );
        assert_eq!(
            definition.permission_set_refs,
            Some(vec!["standard".to_string(), "queue_dispatch".to_string()])
        );
        assert_eq!(
            definition.action_params,
            json!({"items": "{{ items }}", "queue": "{{ queue }}"})
        );
        assert_eq!(definition.item_schema["order_id"]["type"], "integer");
    }

    #[test]
    fn rejects_empty_work_queue_permission_set_refs() {
        let error = parse_work_queue_definition_yaml(
            r#"
ref: core.inbox
label: Core Inbox
dispatch_action: core.process_item
permission_set_refs:
  - ""
"#,
        )
        .expect_err("empty permission set refs should be rejected");

        assert!(error.to_string().contains("permission_set_refs"));
    }

    #[test]
    fn rejects_invalid_tunable_config() {
        let error = validate_work_queue_config(&json!({
            "dispatch": {
                "concurrency": {
                    "source": "pack_config"
                }
            }
        }))
        .expect_err("config should be rejected");

        assert!(error.to_string().contains("config.dispatch.concurrency"));
    }

    #[test]
    fn rejects_removed_priority_config() {
        let error = validate_work_queue_config(&json!({
            "priority": {
                "default": {
                    "source": "literal",
                    "value": 10
                }
            }
        }))
        .expect_err("removed priority config should be rejected");

        assert!(error.to_string().contains("unknown field `priority`"));
    }

    #[test]
    fn validates_batch_coalescing_config() {
        let parsed = validate_work_queue_config_for_batch_mode(
            WorkQueueBatchMode::Batch,
            &json!({
                "dispatch": {
                    "coalescing": {
                        "enabled": true,
                        "group_by_path": "attributes.sobject_type",
                        "across_priorities": true
                    }
                }
            }),
        )
        .expect("config should validate");

        assert!(parsed
            .dispatch
            .and_then(|dispatch| dispatch.coalescing)
            .is_some());
    }

    #[test]
    fn validates_inter_execution_delay_config() {
        let parsed = validate_work_queue_config(&json!({
            "dispatch": {
                "retry_limit": 2,
                "inter_execution_delay_seconds": 15
            }
        }))
        .expect("config should validate");

        assert_eq!(
            parsed
                .dispatch
                .as_ref()
                .and_then(|dispatch| dispatch.retry_limit),
            Some(2)
        );
        assert_eq!(
            parsed
                .dispatch
                .and_then(|dispatch| dispatch.inter_execution_delay_seconds),
            Some(15)
        );
    }

    #[test]
    fn rejects_negative_retry_limit_config() {
        let error = validate_work_queue_config(&json!({
            "dispatch": {
                "retry_limit": -1
            }
        }))
        .expect_err("config should be rejected");

        assert!(error
            .to_string()
            .contains("Invalid work queue config structure"));
    }

    #[test]
    fn rejects_negative_inter_execution_delay_config() {
        let error = validate_work_queue_config(&json!({
            "dispatch": {
                "inter_execution_delay_seconds": -1
            }
        }))
        .expect_err("config should be rejected");

        assert!(error
            .to_string()
            .contains("Invalid work queue config structure"));
    }

    #[test]
    fn rejects_enabled_coalescing_without_group_path() {
        let error = validate_work_queue_config_for_batch_mode(
            WorkQueueBatchMode::Batch,
            &json!({
                "dispatch": {
                    "coalescing": {
                        "enabled": true
                    }
                }
            }),
        )
        .expect_err("config should be rejected");

        assert!(error
            .to_string()
            .contains("group_by_path is required when coalescing is enabled"));
    }

    #[test]
    fn rejects_coalescing_for_single_mode() {
        let config = validate_work_queue_config(&json!({
            "dispatch": {
                "coalescing": {
                    "enabled": false,
                    "group_by_path": "attributes.sobject_type"
                }
            }
        }))
        .expect("config shape should parse");

        let error = validate_work_queue_batch_settings(WorkQueueBatchMode::Single, &config)
            .expect_err("single mode should reject coalescing");

        assert!(error
            .to_string()
            .contains("only supported when batch_mode is 'batch'"));
    }

    #[test]
    fn rejects_non_object_action_params() {
        let error = validate_work_queue_action_params(&json!(["not", "an", "object"]))
            .expect_err("action_params should be rejected");

        assert!(error
            .to_string()
            .contains("action_params must be a JSON object"));
    }

    #[test]
    fn rejects_non_object_item_schema() {
        let error = validate_work_queue_item_schema(&json!(["not", "an", "object"]))
            .expect_err("item_schema should be rejected");

        assert!(error
            .to_string()
            .contains("item_schema must be a JSON object"));
    }
}
