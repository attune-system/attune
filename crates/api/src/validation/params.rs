//! Common schema validation adapters for existing API validation boundaries.
//! Rule authoring defers symbolic values; queue/inquiry acceptance is strict.

use attune_common::models::{action::Action, trigger::Trigger, work_queue::WorkQueue};
use attune_common::value_schema::{compile_inline, CompiledContract, SchemaContext};
use serde_json::Value;

use crate::middleware::ApiError;

enum ValidationMode {
    Authoring,
    Runtime,
}

/// Rule authoring resolves installed types under the declaring component's
/// pack policy. Runtime rendering still validates its separately stored contract.
pub async fn validate_rule_params_with_registry(
    connection: &mut sqlx::PgConnection,
    reference: &str,
    pack_id: Option<i64>,
    schema: Option<&Value>,
    params: &Value,
) -> Result<(), ApiError> {
    use attune_common::{
        repositories::{pack::PackRepository, schema_contract::SchemaContractRepository, FindById},
        value_schema::SourceIdentity,
    };
    let Some(schema) = schema else { return Ok(()) };
    let references = attune_common::value_schema::inspect_definition(
        SchemaContext::FlatDocument,
        schema,
        Default::default(),
    )
    .map_err(|_| ApiError::ValidationError("Invalid rule parameter schema".into()))?;
    let contract = if references.is_empty() {
        compile_inline(reference, SchemaContext::FlatDocument, schema).map_err(|error| {
            ApiError::ValidationError(format!("Invalid rule parameter schema: {error}"))
        })?
    } else if let Some(pack_id) = pack_id {
        let pack = PackRepository::find_by_id(&mut *connection, pack_id)
            .await?
            .ok_or_else(|| {
                ApiError::ValidationError("Rule schema declaring pack is unavailable".into())
            })?;
        SchemaContractRepository::compile_owned_assignment(
            connection,
            SourceIdentity::Authoring {
                component_ref: reference.into(),
            },
            SchemaContext::FlatDocument,
            schema,
            &pack,
        )
        .await?
    } else {
        SchemaContractRepository::materialize_unowned(
            connection,
            SourceIdentity::Authoring {
                component_ref: reference.into(),
            },
            SchemaContext::FlatDocument,
            schema,
        )
        .await?
    };
    let compiled = CompiledContract::new(&contract)
        .map_err(|_| ApiError::ValidationError("Invalid rule rendering contract".into()))?;
    compiled
        .validate_rule_secret_templates(params)
        .map_err(|error| {
            ApiError::ValidationError(format!(
                "Rule secret parameters require whole Key templates: {error}"
            ))
        })?;
    compiled
        .apply_defaults(params)
        .and_then(|defaulted| compiled.validate_authoring(&defaulted))
        .map_err(|error| ApiError::ValidationError(format!("Invalid rule parameters: {error}")))?;
    Ok(())
}

pub async fn validate_action_params_with_registry(
    pool: &sqlx::PgPool,
    action: &Action,
    params: &Value,
) -> Result<(), ApiError> {
    let mut transaction = pool.begin().await?;
    validate_rule_params_with_registry(
        &mut transaction,
        &action.r#ref,
        Some(action.pack),
        action.param_schema.as_ref(),
        params,
    )
    .await?;
    transaction.commit().await?;
    Ok(())
}

pub async fn validate_trigger_params_with_registry(
    pool: &sqlx::PgPool,
    trigger: &Trigger,
    params: &Value,
) -> Result<(), ApiError> {
    let mut transaction = pool.begin().await?;
    validate_rule_params_with_registry(
        &mut transaction,
        &trigger.r#ref,
        trigger.pack,
        trigger.param_schema.as_ref(),
        params,
    )
    .await?;
    transaction.commit().await?;
    Ok(())
}

fn validate_params_against_flat_schema(
    resource_kind: &str,
    resource_ref: &str,
    flat_schema: Option<&Value>,
    params: &Value,
    mode: ValidationMode,
) -> Result<(), ApiError> {
    let Some(flat_schema) = flat_schema else {
        return Ok(());
    };

    let contract =
        compile_inline(resource_ref, SchemaContext::FlatDocument, flat_schema).map_err(|e| {
            ApiError::InternalServerError(format!(
                "Invalid parameter schema for {} '{}': {}",
                resource_kind, resource_ref, e
            ))
        })?;

    let compiled = CompiledContract::new(&contract).map_err(|e| {
        ApiError::InternalServerError(format!(
            "Invalid parameter schema for {resource_kind} '{resource_ref}': {e}"
        ))
    })?;
    let result = match mode {
        ValidationMode::Authoring => {
            compiled
                .validate_rule_secret_templates(params)
                .map_err(|error| {
                    ApiError::ValidationError(format!(
                        "Rule secret parameters require whole Key templates: {error}"
                    ))
                })?;
            compiled
                .apply_defaults(params)
                .and_then(|defaulted| compiled.validate_authoring(&defaulted))
                .map(|_| ())
        }
        ValidationMode::Runtime => compiled.validate(params),
    };
    result.map_err(|e| {
        ApiError::ValidationError(format!(
            "Invalid parameters for {} '{}': {}",
            resource_kind, resource_ref, e
        ))
    })
}

/// Validate trigger parameters against the trigger's parameter schema.
/// Template expressions (`{{ ... }}`) are accepted for any field type.
///
/// The schema is expected in flat StackStorm format and is converted to
/// JSON Schema internally for validation.
pub fn validate_trigger_params(trigger: &Trigger, params: &Value) -> Result<(), ApiError> {
    validate_params_against_flat_schema(
        "trigger",
        &trigger.r#ref,
        trigger.param_schema.as_ref(),
        params,
        ValidationMode::Authoring,
    )
}

/// Validate action parameters against the action's parameter schema.
/// Template expressions (`{{ ... }}`) are accepted for any field type.
///
/// The schema is expected in flat StackStorm format and is converted to
/// JSON Schema internally for validation.
pub fn validate_action_params(action: &Action, params: &Value) -> Result<(), ApiError> {
    validate_params_against_flat_schema(
        "action",
        &action.r#ref,
        action.param_schema.as_ref(),
        params,
        ValidationMode::Authoring,
    )
}

pub fn validate_queue_item_payload(queue: &WorkQueue, payload: &Value) -> Result<(), ApiError> {
    validate_params_against_flat_schema(
        "queue item payload for queue",
        &queue.r#ref,
        Some(&queue.item_schema),
        payload,
        ValidationMode::Runtime,
    )
}

pub fn validate_inquiry_response(
    inquiry_id: i64,
    response_schema: Option<&Value>,
    response: &Value,
) -> Result<(), ApiError> {
    validate_params_against_flat_schema(
        "inquiry response for inquiry",
        &inquiry_id.to_string(),
        response_schema,
        response,
        ValidationMode::Runtime,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn rule_authoring_accepts_required_defaults_without_rewriting_selectors() {
        let schema = json!({
            "message":{"type":"string","required":true,"default":"default message"},
            "count":{"type":"integer","required":true,"default":5},
            "enabled":{"type":"boolean","required":true,"default":true},
            "nullable":{"type":["string","null"],"required":true,"default":"fallback"},
            "token":{"type":"string","secret":true}
        });
        let action = make_action(Some(schema.clone()));
        let trigger = make_trigger(Some(schema));
        let params = json!({"message":"", "count":0, "enabled":false, "nullable":null,
            "token":"{{ keystore['system.token'] }}"});
        validate_action_params(&action, &params).unwrap();
        validate_trigger_params(&trigger, &params).unwrap();
        assert_eq!(
            params,
            json!({"message":"", "count":0, "enabled":false, "nullable":null,
                "token":"{{ keystore['system.token'] }}"})
        );
        validate_action_params(&action, &json!({})).unwrap();
        validate_trigger_params(&trigger, &json!({})).unwrap();
        assert!(validate_action_params(&action, &json!({"count":null})).is_err());
    }

    #[test]
    fn rule_authoring_rejects_literal_secret_defaults_even_when_omitted() {
        let schema = json!({"token":{"type":"string","secret":true,"required":true,
            "default":"private-default-fixture"}});
        let error = validate_action_params(&make_action(Some(schema)), &json!({})).unwrap_err();
        assert!(error.to_string().contains("Key template"));
        assert!(!error.to_string().contains("private-default-fixture"));
    }

    #[test]
    fn rule_authoring_rejects_direct_secrets_for_actions_and_triggers() {
        let schema = json!({"token":{"type":"string","secret":true}});
        let action = make_action(Some(schema.clone()));
        let trigger = make_trigger(Some(schema));
        for params in [
            json!({"token":"private-literal"}),
            json!({"token":"prefix {{ keystore.system.token }}"}),
        ] {
            for result in [
                validate_action_params(&action, &params),
                validate_trigger_params(&trigger, &params),
            ] {
                let error = result.unwrap_err();
                assert!(matches!(error, ApiError::ValidationError(_)));
                assert!(!error.to_string().contains("private-literal"));
                assert!(error.to_string().contains("Key templates"));
            }
        }
        let params = json!({"token":"{{ keystore['system.token'] }}"});
        validate_action_params(&action, &params).unwrap();
        validate_trigger_params(&trigger, &params).unwrap();
    }

    // ── Helper builders ──────────────────────────────────────────────

    fn make_trigger(schema: Option<Value>) -> Trigger {
        Trigger {
            id: 1,
            r#ref: "test.trigger".to_string(),
            pack: Some(1),
            pack_ref: Some("test".to_string()),
            label: "Test Trigger".to_string(),
            description: None,
            enabled: true,
            enabled_override: None,
            param_schema: schema,
            event_config_schema: None,
            out_schema: None,
            webhook_enabled: false,
            webhook_key: None,
            webhook_config: None,
            sensor: None,
            sensor_ref: None,
            is_adhoc: false,
            reference_visibility: Default::default(),
            reference_allowed_pack_refs: Vec::new(),
            retired_at: None,
            created: chrono::Utc::now(),
            updated: chrono::Utc::now(),
        }
    }

    fn make_action(schema: Option<Value>) -> Action {
        Action {
            id: 1,
            r#ref: "test.action".to_string(),
            pack: 1,
            pack_ref: "test".to_string(),
            label: "Test Action".to_string(),
            description: Some("Test action".to_string()),
            entrypoint: "test.sh".to_string(),
            runtime: Some(1),
            enabled: true,
            enabled_override: None,
            runtime_version_constraint: None,
            required_worker_runtimes: serde_json::json!({}),
            worker_selector: serde_json::json!({}),
            worker_tolerations: serde_json::json!([]),
            worker_affinity: serde_json::json!({}),
            param_schema: schema,
            out_schema: None,
            workflow_def: None,
            is_adhoc: false,
            accesses_mcp: false,
            default_execution_permission_set_refs: Vec::new(),
            reference_visibility: Default::default(),
            reference_allowed_pack_refs: Vec::new(),
            artifact_retention_policy: None,
            artifact_retention_limit: None,
            log_retention_policy: None,
            log_retention_limit: None,
            timeout_seconds: None,
            parameter_delivery: attune_common::models::ParameterDelivery::default(),
            parameter_format: attune_common::models::ParameterFormat::default(),
            output_format: attune_common::models::OutputFormat::default(),
            retired_at: None,
            created: chrono::Utc::now(),
            updated: chrono::Utc::now(),
        }
    }

    // ── No schema ────────────────────────────────────────────────────

    #[test]
    fn test_validate_trigger_params_with_no_schema() {
        let trigger = make_trigger(None);
        let params = json!({ "any": "value" });
        assert!(validate_trigger_params(&trigger, &params).is_ok());
    }

    // ── Basic trigger validation (no templates) ──────────────────────

    #[test]
    fn malformed_and_raw_json_schemas_fail_closed() {
        for schema in [
            json!(null),
            json!({"field": "string"}),
            json!({"type": "object", "properties": {}}),
        ] {
            let action = make_action(Some(schema));
            assert!(matches!(
                validate_action_params(&action, &json!({})),
                Err(ApiError::InternalServerError(_))
            ));
        }
    }

    // ── Basic trigger validation (flat format) ──────────────────────

    #[test]
    fn test_validate_trigger_params_with_valid_params() {
        let schema = json!({
            "unit": { "type": "string", "enum": ["seconds", "minutes", "hours"], "required": true },
            "delta": { "type": "integer", "minimum": 1, "required": true }
        });

        let trigger = make_trigger(Some(schema));
        let params = json!({ "unit": "seconds", "delta": 10 });
        assert!(validate_trigger_params(&trigger, &params).is_ok());
    }

    #[test]
    fn test_validate_trigger_params_with_invalid_params() {
        let schema = json!({
            "unit": { "type": "string", "enum": ["seconds", "minutes", "hours"], "required": true },
            "delta": { "type": "integer", "minimum": 1, "required": true }
        });

        let trigger = make_trigger(Some(schema));

        // Missing required field 'delta'
        let params = json!({ "unit": "seconds" });
        assert!(validate_trigger_params(&trigger, &params).is_err());

        // Invalid enum value for 'unit'
        let params = json!({ "unit": "days", "delta": 10 });
        assert!(validate_trigger_params(&trigger, &params).is_err());

        // Invalid type for 'delta'
        let params = json!({ "unit": "seconds", "delta": "10" });
        assert!(validate_trigger_params(&trigger, &params).is_err());
    }

    // ── Basic action validation (flat format) ───────────────────────

    #[test]
    fn test_validate_action_params_with_valid_params() {
        let schema = json!({
            "message": { "type": "string", "required": true }
        });

        let action = make_action(Some(schema));
        let params = json!({ "message": "Hello, world!" });
        assert!(validate_action_params(&action, &params).is_ok());
    }

    #[test]
    fn test_validate_action_params_with_empty_params_but_required_fields() {
        let schema = json!({
            "message": { "type": "string", "required": true }
        });

        let action = make_action(Some(schema));
        let params = json!({});
        assert!(validate_action_params(&action, &params).is_err());
    }

    // ── Template-aware validation (flat format) ──────────────────────

    #[test]
    fn test_template_in_integer_field_passes() {
        let schema = json!({
            "counter": { "type": "integer", "required": true }
        });

        let action = make_action(Some(schema));
        let params = json!({ "counter": "{{ event.payload.counter }}" });
        assert!(validate_action_params(&action, &params).is_ok());
    }

    #[test]
    fn test_template_in_boolean_field_passes() {
        let schema = json!({
            "verbose": { "type": "boolean", "required": true }
        });

        let action = make_action(Some(schema));
        let params = json!({ "verbose": "{{ event.payload.debug }}" });
        assert!(validate_action_params(&action, &params).is_ok());
    }

    #[test]
    fn test_template_in_number_field_passes() {
        let schema = json!({
            "threshold": { "type": "number", "minimum": 0.0, "required": true }
        });

        let action = make_action(Some(schema));
        let params = json!({ "threshold": "{{ event.payload.threshold }}" });
        assert!(validate_action_params(&action, &params).is_ok());
    }

    #[test]
    fn test_template_in_enum_field_passes() {
        let schema = json!({
            "level": { "type": "string", "enum": ["info", "warn", "error"], "required": true }
        });

        let action = make_action(Some(schema));
        let params = json!({ "level": "{{ event.payload.severity }}" });
        assert!(validate_action_params(&action, &params).is_ok());
    }

    #[test]
    fn test_template_in_array_field_passes() {
        let schema = json!({
            "recipients": { "type": "array", "items": { "type": "string" }, "required": true }
        });

        let action = make_action(Some(schema));
        let params = json!({ "recipients": "{{ event.payload.emails }}" });
        assert!(validate_action_params(&action, &params).is_ok());
    }

    #[test]
    fn test_template_in_object_field_passes() {
        let schema = json!({
            "metadata": { "type": "object", "required": true }
        });

        let action = make_action(Some(schema));
        let params = json!({ "metadata": "{{ event.payload.meta }}" });
        assert!(validate_action_params(&action, &params).is_ok());
    }

    #[test]
    fn test_mixed_template_and_literal_values() {
        let schema = json!({
            "message": { "type": "string", "required": true },
            "count": { "type": "integer", "required": true },
            "verbose": { "type": "boolean", "required": true }
        });

        let action = make_action(Some(schema));

        // Mix of literal and template values
        let params = json!({
            "message": "Hello",
            "count": "{{ event.payload.count }}",
            "verbose": true
        });
        assert!(validate_action_params(&action, &params).is_ok());
    }

    // Secret rule fields require Key templates and retain required checks.

    #[test]
    fn test_required_secret_field_requires_a_key_template() {
        let schema = json!({
            "api_key": { "type": "string", "required": true, "secret": true },
            "endpoint": { "type": "string" }
        });

        let action = make_action(Some(schema));

        // Literal credentials must never enter the authoring row.
        let params = json!({ "api_key": "sk-1234", "endpoint": "https://api.example.com" });
        assert!(validate_action_params(&action, &params).is_err());
        let params = json!({ "api_key": "{{ keystore['system.api_key'] }}", "endpoint": "https://api.example.com" });
        assert!(validate_action_params(&action, &params).is_ok());

        // Invalid: secret field missing but required
        let params = json!({ "endpoint": "https://api.example.com" });
        assert!(validate_action_params(&action, &params).is_err());
    }

    #[test]
    fn test_literal_values_still_validated() {
        let schema = json!({
            "message": { "type": "string", "required": true },
            "count": { "type": "integer", "required": true }
        });

        let action = make_action(Some(schema));

        // Template for message is fine, but literal "not_a_number" for integer is not
        let params = json!({
            "message": "{{ event.payload.msg }}",
            "count": "not_a_number"
        });
        assert!(validate_action_params(&action, &params).is_err());
    }

    #[test]
    fn test_required_field_still_enforced_with_templates() {
        let schema = json!({
            "message": { "type": "string", "required": true },
            "count": { "type": "integer", "required": true }
        });

        let action = make_action(Some(schema));

        // Only message provided (even as template), count is missing
        let params = json!({ "message": "{{ event.payload.msg }}" });
        assert!(validate_action_params(&action, &params).is_err());
    }

    #[test]
    fn test_pack_config_template_passes() {
        let schema = json!({
            "api_key": { "type": "string", "required": true },
            "timeout": { "type": "integer", "required": true }
        });

        let action = make_action(Some(schema));
        let params = json!({
            "api_key": "{{ pack.config.api_key }}",
            "timeout": "{{ pack.config.default_timeout }}"
        });
        assert!(validate_action_params(&action, &params).is_ok());
    }

    #[test]
    fn test_system_template_passes() {
        let schema = json!({
            "timestamp": { "type": "string", "required": true },
            "rule_id": { "type": "integer", "required": true }
        });

        let action = make_action(Some(schema));
        let params = json!({
            "timestamp": "{{ system.timestamp }}",
            "rule_id": "{{ system.rule.id }}"
        });
        assert!(validate_action_params(&action, &params).is_ok());
    }

    #[test]
    fn test_trigger_params_template_aware() {
        let schema = json!({
            "unit": { "type": "string", "enum": ["seconds", "minutes", "hours"], "required": true },
            "delta": { "type": "integer", "minimum": 1, "required": true }
        });

        let trigger = make_trigger(Some(schema));

        // Both fields as templates
        let params = json!({
            "unit": "{{ pack.config.timer_unit }}",
            "delta": "{{ pack.config.timer_delta }}"
        });
        assert!(validate_trigger_params(&trigger, &params).is_ok());
    }

    #[test]
    fn nested_object_templates_keep_literal_sibling_validation() {
        let schema = json!({
            "outer": {
                "type": "object",
                "properties": {
                    "inner_count": { "type": "integer" },
                    "literal": { "type": "integer" }
                }
            }
        });

        let params = json!({
            "outer": {
                "inner_count": "{{ event.payload.count }}"
            }
        });

        let action = make_action(Some(schema));
        assert!(validate_action_params(&action, &params).is_ok());
        assert!(validate_action_params(
            &action,
            &json!({"outer": {"inner_count": "{{ event.payload.count }}", "literal": "wrong"}})
        )
        .is_err());
    }

    #[test]
    fn array_element_templates_keep_literal_item_validation() {
        let schema = json!({
            "tags": {
                "type": "array",
                "items": { "type": "string" }
            }
        });

        let params = json!({
            "tags": ["literal", "{{ event.payload.tag }}"]
        });

        let action = make_action(Some(schema));
        assert!(validate_action_params(&action, &params).is_ok());
        assert!(
            validate_action_params(&action, &json!({"tags": [1, "{{ event.payload.tag }}"]}))
                .is_err()
        );
    }

    #[test]
    fn constrained_whole_object_templates_are_deferred() {
        let action = make_action(Some(
            json!({"object": {"type": "object", "properties": {"child": {"type": "integer"}}, "required": ["child"]}}),
        ));
        assert!(
            validate_action_params(&action, &json!({"object": "{{ event.payload.object }}"}))
                .is_ok()
        );
    }

    #[test]
    fn acceptance_is_strict_and_errors_do_not_include_secret_values() {
        let schema = json!({"token": {"type": "string", "minLength": 100, "secret": true, "required": true}});
        let failure = validate_inquiry_response(
            1,
            Some(&schema),
            &json!({"token": "PRIVATE_SUBMITTED_VALUE"}),
        )
        .unwrap_err();
        assert!(!failure.to_string().contains("PRIVATE_SUBMITTED_VALUE"));
        assert!(
            validate_inquiry_response(1, Some(&schema), &json!({"token": "{{ value }}"})).is_err()
        );
        assert!(validate_params_against_flat_schema(
            "queue",
            "test.queue",
            Some(&schema),
            &json!({"token": "{{ value }}"}),
            ValidationMode::Runtime
        )
        .is_err());
    }
}
