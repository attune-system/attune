//! Template Resolver
//!
//! Resolves template variables in rule action parameters using context from
//! event payloads, pack configuration, and system variables.
//!
//! Supports template syntax: `{{ source.path.to.value }}`
//!
//! ## Available Template Sources
//!
//! - `event.payload.*` — Fields from the event payload
//! - `event.id` — The event's database ID
//! - `event.trigger` — The trigger ref that generated the event
//! - `event.created` — The event's creation timestamp
//! - `pack.config.*` — Pack configuration values
//! - `system.*` — System-provided variables (timestamp, rule info, etc.)
//!
//! ## Example
//!
//! ```rust
//! use serde_json::json;
//! use attune_common::template_resolver::{TemplateContext, resolve_templates};
//!
//! let context = TemplateContext::new(
//!     json!({"service": "api-gateway"}),
//!     json!({}),
//!     json!({}),
//! )
//! .with_event_id(42)
//! .with_event_trigger("core.webhook")
//! .with_event_created("2026-02-05T10:00:00Z");
//!
//! let params = json!({
//!     "message": "Error in {{ event.payload.service }}",
//!     "trigger": "{{ event.trigger }}",
//!     "event_id": "{{ event.id }}"
//! });
//!
//! let resolved = resolve_templates(&params, &context).unwrap();
//! assert_eq!(resolved["message"], "Error in api-gateway");
//! assert_eq!(resolved["trigger"], "core.webhook");
//! assert_eq!(resolved["event_id"], 42);
//! ```

use anyhow::Result;
use regex::Regex;
use serde_json::Value as JsonValue;
use std::collections::HashMap;
use std::sync::LazyLock;
use tracing::{debug, warn};

use crate::secret_values::{
    pointer_from_dot_path, pointer_join, pointer_suffix, JsonPointer, RenderedJson,
    SecretPathSource, SecretSource,
};

/// Template context containing all available data sources for template resolution.
///
/// The context is structured around three namespaces:
/// - `event` — Event data including payload, id, trigger ref, and created timestamp
/// - `pack.config` — Pack configuration values
/// - `system` — System-provided variables
#[derive(Debug, Clone)]
pub struct TemplateContext {
    /// Event data (payload, id, trigger, created) — accessed as `event.*`
    pub event: JsonValue,
    /// Pack configuration — accessed as `pack.config.*`
    pub pack_config: JsonValue,
    /// System-provided variables — accessed as `system.*`
    pub system_vars: JsonValue,
    secret_sources: Vec<ContextSecretSource>,
}

#[derive(Debug, Clone)]
struct ContextSecretSource {
    path: String,
    pointer: JsonPointer,
    source: SecretSource,
}

impl TemplateContext {
    /// Create a new template context with an event payload.
    ///
    /// The payload is nested under `event.payload`. Use builder methods
    /// to add event metadata (`with_event_id`, `with_event_trigger`, `with_event_created`).
    pub fn new(event_payload: JsonValue, pack_config: JsonValue, system_vars: JsonValue) -> Self {
        let event = serde_json::json!({
            "payload": event_payload,
        });
        Self {
            event,
            pack_config,
            system_vars,
            secret_sources: Vec::new(),
        }
    }

    /// Set the event ID in the context (accessible as `{{ event.id }}`).
    pub fn with_event_id(mut self, id: i64) -> Self {
        if let Some(obj) = self.event.as_object_mut() {
            obj.insert("id".to_string(), serde_json::json!(id));
        }
        self
    }

    /// Set the trigger ref in the context (accessible as `{{ event.trigger }}`).
    pub fn with_event_trigger(mut self, trigger_ref: &str) -> Self {
        if let Some(obj) = self.event.as_object_mut() {
            obj.insert("trigger".to_string(), serde_json::json!(trigger_ref));
        }
        self
    }

    /// Set the event created timestamp in the context (accessible as `{{ event.created }}`).
    pub fn with_event_created(mut self, created: &str) -> Self {
        if let Some(obj) = self.event.as_object_mut() {
            obj.insert("created".to_string(), serde_json::json!(created));
        }
        self
    }

    pub fn with_pack_config_secret_paths(
        mut self,
        pack_ref: Option<String>,
        secret_paths: Vec<JsonPointer>,
    ) -> Self {
        for path in secret_paths {
            let dotted_suffix = pointer_to_dot_path(&path);
            let source_path = if dotted_suffix.is_empty() {
                "pack.config".to_string()
            } else {
                format!("pack.config.{dotted_suffix}")
            };
            self.secret_sources.push(ContextSecretSource {
                path: source_path,
                pointer: path.clone(),
                source: SecretSource::PackConfig {
                    pack_ref: pack_ref.clone(),
                    path,
                },
            });
        }
        self
    }

    pub fn with_event_payload_secret_paths(
        mut self,
        trigger_ref: Option<String>,
        secret_paths: Vec<JsonPointer>,
    ) -> Self {
        for path in secret_paths {
            let dotted_suffix = pointer_to_dot_path(&path);
            let source_path = if dotted_suffix.is_empty() {
                "event.payload".to_string()
            } else {
                format!("event.payload.{dotted_suffix}")
            };
            self.secret_sources.push(ContextSecretSource {
                path: source_path,
                pointer: pointer_join("/payload", &path),
                source: SecretSource::TriggerSchema {
                    trigger_ref: trigger_ref.clone(),
                    section: "payload",
                    path,
                },
            });
        }
        self
    }

    /// Get a value from the context using a dotted path.
    ///
    /// Supports paths like:
    /// - `event.payload.field` — event payload data
    /// - `event.id` — event ID
    /// - `event.trigger` — trigger ref
    /// - `event.created` — creation timestamp
    /// - `pack.config.setting` — pack configuration
    /// - `system.timestamp` — system variables
    pub fn get_value(&self, path: &str) -> Option<JsonValue> {
        let parts: Vec<&str> = path.split('.').collect();

        if parts.is_empty() {
            return None;
        }

        // Determine the root source and how many path segments to skip
        let (root, skip_count) = match parts[0] {
            "event" => {
                // event.* paths navigate directly into the event JSON object
                // e.g. event.id, event.trigger, event.created, event.payload.field
                (&self.event, 1)
            }
            "pack" => {
                // pack.config.* paths
                if parts.len() < 2 || parts[1] != "config" {
                    warn!("Invalid pack path: {}, expected 'pack.config.*'", path);
                    return None;
                }
                (&self.pack_config, 2)
            }
            "system" => (&self.system_vars, 1),
            _ => {
                warn!("Unknown template source: {}", parts[0]);
                return None;
            }
        };

        extract_nested_value(root, &parts[skip_count..])
    }

    fn secret_path_sources_for_expression(
        &self,
        expr: &str,
        dest_pointer: &str,
        pure_expression: bool,
    ) -> Vec<SecretPathSource> {
        let expr = normalize_expression_path(expr);
        let mut sources = Vec::new();

        for source in &self.secret_sources {
            if source.path == expr || expr.starts_with(&format!("{}.", source.path)) {
                sources.push(SecretPathSource {
                    path: dest_pointer.to_string(),
                    source: source.source.clone(),
                });
                continue;
            }

            if pure_expression && source.path.starts_with(&format!("{expr}.")) {
                let expr_pointer = template_source_pointer(&expr);
                if let Some(suffix) = pointer_suffix(&source.pointer, &expr_pointer) {
                    sources.push(SecretPathSource {
                        path: pointer_join(dest_pointer, &suffix),
                        source: source.source.clone(),
                    });
                }
            } else if !pure_expression && source.path.starts_with(&format!("{expr}.")) {
                sources.push(SecretPathSource {
                    path: dest_pointer.to_string(),
                    source: source.source.clone(),
                });
            }
        }

        sources
    }
}

/// Regex pattern to match template variables: {{ ... }}
static TEMPLATE_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\{\{\s*([^}]+?)\s*\}\}").expect("Failed to compile template regex")
});

/// Resolve all template variables in a JSON value.
///
/// Recursively processes objects and arrays, replacing template strings
/// with values from the context.
pub fn resolve_templates(value: &JsonValue, context: &TemplateContext) -> Result<JsonValue> {
    match value {
        JsonValue::String(s) => resolve_string_template(s, context),
        JsonValue::Object(map) => {
            let mut resolved = serde_json::Map::new();
            for (key, val) in map {
                resolved.insert(key.clone(), resolve_templates(val, context)?);
            }
            Ok(JsonValue::Object(resolved))
        }
        JsonValue::Array(arr) => {
            let resolved: Result<Vec<JsonValue>> =
                arr.iter().map(|v| resolve_templates(v, context)).collect();
            Ok(JsonValue::Array(resolved?))
        }
        // Pass through other types unchanged
        other => Ok(other.clone()),
    }
}

pub fn resolve_templates_with_sensitivity(
    value: &JsonValue,
    context: &TemplateContext,
) -> Result<RenderedJson> {
    resolve_templates_with_sensitivity_at(value, context, "")
}

fn resolve_templates_with_sensitivity_at(
    value: &JsonValue,
    context: &TemplateContext,
    pointer: &str,
) -> Result<RenderedJson> {
    match value {
        JsonValue::String(s) => resolve_string_template_with_sensitivity(s, context, pointer),
        JsonValue::Object(map) => {
            let mut resolved = serde_json::Map::new();
            let mut path_sources = Vec::new();
            for (key, val) in map {
                let child_pointer = format!("{}/{}", pointer, escape_pointer_segment(key));
                let rendered = resolve_templates_with_sensitivity_at(val, context, &child_pointer)?;
                resolved.insert(key.clone(), rendered.value);
                path_sources.extend(rendered.secret_path_sources);
            }
            Ok(rendered_from_path_sources(
                JsonValue::Object(resolved),
                path_sources,
            ))
        }
        JsonValue::Array(arr) => {
            let mut resolved = Vec::new();
            let mut path_sources = Vec::new();
            for (idx, val) in arr.iter().enumerate() {
                let child_pointer = format!("{pointer}/{idx}");
                let rendered = resolve_templates_with_sensitivity_at(val, context, &child_pointer)?;
                resolved.push(rendered.value);
                path_sources.extend(rendered.secret_path_sources);
            }
            Ok(rendered_from_path_sources(
                JsonValue::Array(resolved),
                path_sources,
            ))
        }
        other => Ok(RenderedJson::plain(other.clone())),
    }
}

/// Resolve templates in a string value.
///
/// If the string contains a single template that matches the entire string,
/// returns the value with its original type (preserving numbers, booleans, etc).
///
/// If the string contains multiple templates or mixed content, performs
/// string interpolation.
fn resolve_string_template(s: &str, context: &TemplateContext) -> Result<JsonValue> {
    // Check if the entire string is a single template (for type preservation)
    if let Some(captures) = TEMPLATE_REGEX.captures(s) {
        let full_match = captures.get(0).unwrap();
        if full_match.start() == 0 && full_match.end() == s.len() {
            // Single template - preserve type
            let path = captures.get(1).unwrap().as_str().trim();
            debug!("Resolving single template: {}", path);

            return match context.get_value(path) {
                Some(value) => {
                    debug!("Resolved {} -> {:?}", path, value);
                    Ok(value)
                }
                None => {
                    warn!("Template variable not found: {}", path);
                    Ok(JsonValue::Null)
                }
            };
        }
    }

    // Multiple templates or mixed content - perform string interpolation
    let mut result = s.to_string();
    let mut any_replaced = false;

    for captures in TEMPLATE_REGEX.captures_iter(s) {
        let full_match = captures.get(0).unwrap().as_str();
        let path = captures.get(1).unwrap().as_str().trim();

        debug!("Resolving template in string: {}", path);

        match context.get_value(path) {
            Some(value) => {
                let replacement = value_to_string(&value);
                debug!("Resolved {} -> {}", path, replacement);
                result = result.replace(full_match, &replacement);
                any_replaced = true;
            }
            None => {
                warn!("Template variable not found: {}", path);
                result = result.replace(full_match, "");
            }
        }
    }

    if any_replaced {
        debug!("String interpolation result: {}", result);
    }

    Ok(JsonValue::String(result))
}

fn resolve_string_template_with_sensitivity(
    s: &str,
    context: &TemplateContext,
    pointer: &str,
) -> Result<RenderedJson> {
    if let Some(captures) = TEMPLATE_REGEX.captures(s) {
        let full_match = captures.get(0).unwrap();
        if full_match.start() == 0 && full_match.end() == s.len() {
            let path = captures.get(1).unwrap().as_str().trim();
            let value = context.get_value(path).unwrap_or(JsonValue::Null);
            let path_sources = context.secret_path_sources_for_expression(path, pointer, true);
            return Ok(rendered_from_path_sources(value, path_sources));
        }
    }

    let value = resolve_string_template(s, context)?;
    let mut path_sources = Vec::new();
    for captures in TEMPLATE_REGEX.captures_iter(s) {
        let path = captures.get(1).unwrap().as_str().trim();
        path_sources.extend(context.secret_path_sources_for_expression(path, pointer, false));
    }

    Ok(rendered_from_path_sources(value, path_sources))
}

/// Extract a nested value from JSON using a path.
fn extract_nested_value(root: &JsonValue, path: &[&str]) -> Option<JsonValue> {
    if path.is_empty() {
        return Some(root.clone());
    }

    let mut current = root;

    for part in path {
        match current {
            JsonValue::Object(map) => {
                current = map.get(*part)?;
            }
            JsonValue::Array(arr) => {
                // Try to parse part as array index
                if let Ok(index) = part.parse::<usize>() {
                    current = arr.get(index)?;
                } else {
                    return None;
                }
            }
            _ => return None,
        }
    }

    Some(current.clone())
}

/// Convert a JSON value to a string for interpolation.
fn value_to_string(value: &JsonValue) -> String {
    match value {
        JsonValue::String(s) => s.clone(),
        JsonValue::Number(n) => n.to_string(),
        JsonValue::Bool(b) => b.to_string(),
        JsonValue::Null => String::new(),
        JsonValue::Array(_) | JsonValue::Object(_) => {
            // For complex types, serialize as JSON
            serde_json::to_string(value).unwrap_or_default()
        }
    }
}

fn rendered_from_path_sources(
    value: JsonValue,
    path_sources: Vec<SecretPathSource>,
) -> RenderedJson {
    let mut unique_by_path_source = HashMap::new();
    for path_source in path_sources {
        unique_by_path_source
            .entry((path_source.path.clone(), path_source.source.clone()))
            .or_insert(path_source);
    }
    let mut secret_path_sources = unique_by_path_source.into_values().collect::<Vec<_>>();
    secret_path_sources.sort_by(|a, b| a.path.cmp(&b.path));

    let mut secret_paths = secret_path_sources
        .iter()
        .map(|source| source.path.clone())
        .collect::<Vec<_>>();
    secret_paths.sort();
    secret_paths.dedup();

    let mut sources = secret_path_sources
        .iter()
        .map(|source| source.source.clone())
        .collect::<Vec<_>>();
    sources.sort_by(|a, b| format!("{a:?}").cmp(&format!("{b:?}")));
    sources.dedup();

    RenderedJson {
        value,
        secret_paths,
        sources,
        secret_path_sources,
    }
}

fn normalize_expression_path(expr: &str) -> String {
    expr.trim()
        .replace("result()", "result")
        .replace("[\"", ".")
        .replace("']", "")
        .replace("[\'", ".")
        .replace("\"]", "")
}

fn pointer_to_dot_path(pointer: &str) -> String {
    pointer
        .trim_start_matches('/')
        .split('/')
        .filter(|segment| !segment.is_empty())
        .map(unescape_pointer_segment)
        .collect::<Vec<_>>()
        .join(".")
}

fn template_source_pointer(expr: &str) -> String {
    for root in ["pack.config", "event", "system"] {
        if expr == root {
            return String::new();
        }
        if let Some(suffix) = expr.strip_prefix(&format!("{root}.")) {
            return pointer_from_dot_path(suffix);
        }
    }
    pointer_from_dot_path(expr)
}

fn escape_pointer_segment(segment: &str) -> String {
    segment.replace('~', "~0").replace('/', "~1")
}

fn unescape_pointer_segment(segment: &str) -> String {
    segment.replace("~1", "/").replace("~0", "~")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn create_test_context() -> TemplateContext {
        TemplateContext::new(
            json!({
                "service": "api-gateway",
                "message": "Connection timeout",
                "severity": "critical",
                "count": 42,
                "enabled": true,
                "metadata": {
                    "host": "web-01",
                    "port": 8080
                },
                "tags": ["production", "backend"]
            }),
            json!({
                "api_token": "secret123",
                "alert_channel": "#incidents",
                "timeout": 30
            }),
            json!({
                "timestamp": "2026-01-17T15:30:00Z",
                "rule": {
                    "id": 42,
                    "ref": "test.rule"
                }
            }),
        )
        .with_event_id(123)
        .with_event_trigger("core.error_event")
        .with_event_created("2026-01-17T15:30:00Z")
    }

    #[test]
    fn test_simple_string_substitution() {
        let context = create_test_context();
        let template = json!({
            "message": "Hello {{ event.payload.service }}"
        });

        let result = resolve_templates(&template, &context).unwrap();
        assert_eq!(result["message"], "Hello api-gateway");
    }

    #[test]
    fn test_single_template_type_preservation() {
        let context = create_test_context();

        // Number
        let template = json!({"count": "{{ event.payload.count }}"});
        let result = resolve_templates(&template, &context).unwrap();
        assert_eq!(result["count"], 42);

        // Boolean
        let template = json!({"enabled": "{{ event.payload.enabled }}"});
        let result = resolve_templates(&template, &context).unwrap();
        assert_eq!(result["enabled"], true);
    }

    #[test]
    fn test_nested_object_access() {
        let context = create_test_context();
        let template = json!({
            "host": "{{ event.payload.metadata.host }}",
            "port": "{{ event.payload.metadata.port }}"
        });

        let result = resolve_templates(&template, &context).unwrap();
        assert_eq!(result["host"], "web-01");
        assert_eq!(result["port"], 8080);
    }

    #[test]
    fn test_array_access() {
        let context = create_test_context();
        let template = json!({
            "first_tag": "{{ event.payload.tags.0 }}",
            "second_tag": "{{ event.payload.tags.1 }}"
        });

        let result = resolve_templates(&template, &context).unwrap();
        assert_eq!(result["first_tag"], "production");
        assert_eq!(result["second_tag"], "backend");
    }

    #[test]
    fn secret_pack_config_source_marks_rendered_destination() {
        let context = TemplateContext::new(
            json!({}),
            json!({"api": {"token": "secret-token"}}),
            json!({}),
        )
        .with_pack_config_secret_paths(Some("demo".to_string()), vec!["/api/token".to_string()]);
        let template = json!({"password": "{{ pack.config.api.token }}"});

        let rendered = resolve_templates_with_sensitivity(&template, &context).unwrap();

        assert_eq!(rendered.value["password"], "secret-token");
        assert_eq!(rendered.secret_paths, vec!["/password"]);
        assert_eq!(rendered.secret_path_sources[0].path, "/password");
    }

    #[test]
    fn secret_event_payload_source_marks_rendered_destination() {
        let context = TemplateContext::new(
            json!({"service": "billing", "api_key": "secret-key"}),
            json!({}),
            json!({}),
        )
        .with_event_payload_secret_paths(
            Some("demo.secret_trigger".to_string()),
            vec!["/api_key".to_string()],
        );
        let template = json!({"api_key": "{{ event.payload.api_key }}"});

        let rendered = resolve_templates_with_sensitivity(&template, &context).unwrap();

        assert_eq!(rendered.value["api_key"], "secret-key");
        assert_eq!(rendered.secret_paths, vec!["/api_key"]);
        assert_eq!(rendered.secret_path_sources[0].path, "/api_key");
        assert!(matches!(
            rendered.secret_path_sources[0].source,
            SecretSource::TriggerSchema {
                section: "payload",
                ..
            }
        ));
    }

    #[test]
    fn test_pack_config_reference() {
        let context = create_test_context();
        let template = json!({
            "token": "{{ pack.config.api_token }}",
            "channel": "{{ pack.config.alert_channel }}"
        });

        let result = resolve_templates(&template, &context).unwrap();
        assert_eq!(result["token"], "secret123");
        assert_eq!(result["channel"], "#incidents");
    }

    #[test]
    fn test_system_variables() {
        let context = create_test_context();
        let template = json!({
            "timestamp": "{{ system.timestamp }}",
            "rule_id": "{{ system.rule.id }}",
        });

        let result = resolve_templates(&template, &context).unwrap();
        assert_eq!(result["timestamp"], "2026-01-17T15:30:00Z");
        assert_eq!(result["rule_id"], 42);
    }

    #[test]
    fn test_event_metadata_id() {
        let context = create_test_context();
        let template = json!({
            "event_id": "{{ event.id }}"
        });

        let result = resolve_templates(&template, &context).unwrap();
        assert_eq!(result["event_id"], 123);
    }

    #[test]
    fn test_event_metadata_trigger() {
        let context = create_test_context();
        let template = json!({
            "trigger_ref": "{{ event.trigger }}"
        });

        let result = resolve_templates(&template, &context).unwrap();
        assert_eq!(result["trigger_ref"], "core.error_event");
    }

    #[test]
    fn test_event_metadata_created() {
        let context = create_test_context();
        let template = json!({
            "created_at": "{{ event.created }}"
        });

        let result = resolve_templates(&template, &context).unwrap();
        assert_eq!(result["created_at"], "2026-01-17T15:30:00Z");
    }

    #[test]
    fn test_event_metadata_in_interpolation() {
        let context = create_test_context();
        let template = json!({
            "summary": "Event {{ event.id }} from {{ event.trigger }} at {{ event.created }}"
        });

        let result = resolve_templates(&template, &context).unwrap();
        assert_eq!(
            result["summary"],
            "Event 123 from core.error_event at 2026-01-17T15:30:00Z"
        );
    }

    #[test]
    fn test_missing_value_returns_null() {
        let context = create_test_context();
        let template = json!({
            "missing": "{{ event.payload.nonexistent }}"
        });

        let result = resolve_templates(&template, &context).unwrap();
        assert!(result["missing"].is_null());
    }

    #[test]
    fn test_multiple_templates_in_string() {
        let context = create_test_context();
        let template = json!({
            "message": "Error in {{ event.payload.service }}: {{ event.payload.message }}"
        });

        let result = resolve_templates(&template, &context).unwrap();
        assert_eq!(
            result["message"],
            "Error in api-gateway: Connection timeout"
        );
    }

    #[test]
    fn test_static_values_unchanged() {
        let context = create_test_context();
        let template = json!({
            "static": "This is static",
            "number": 123,
            "boolean": false
        });

        let result = resolve_templates(&template, &context).unwrap();
        assert_eq!(result["static"], "This is static");
        assert_eq!(result["number"], 123);
        assert_eq!(result["boolean"], false);
    }

    #[test]
    fn test_nested_objects_and_arrays() {
        let context = create_test_context();
        let template = json!({
            "nested": {
                "field1": "{{ event.payload.service }}",
                "field2": "{{ pack.config.timeout }}"
            },
            "array": [
                "{{ event.payload.severity }}",
                "static value"
            ]
        });

        let result = resolve_templates(&template, &context).unwrap();
        assert_eq!(result["nested"]["field1"], "api-gateway");
        assert_eq!(result["nested"]["field2"], 30);
        assert_eq!(result["array"][0], "critical");
        assert_eq!(result["array"][1], "static value");
    }

    #[test]
    fn test_empty_template_context() {
        let context = TemplateContext::new(json!({}), json!({}), json!({}));

        let template = json!({
            "message": "{{ event.payload.missing }}"
        });

        let result = resolve_templates(&template, &context).unwrap();
        assert!(result["message"].is_null());
    }

    #[test]
    fn test_whitespace_in_templates() {
        let context = create_test_context();
        let template = json!({
            "message": "{{  event.payload.service  }}"
        });

        let result = resolve_templates(&template, &context).unwrap();
        assert_eq!(result["message"], "api-gateway");
    }

    #[test]
    fn test_complex_real_world_example() {
        let context = create_test_context();
        let template = json!({
            "channel": "{{ pack.config.alert_channel }}",
            "message": "🚨 Error in {{ event.payload.service }}: {{ event.payload.message }}",
            "severity": "{{ event.payload.severity }}",
            "details": {
                "host": "{{ event.payload.metadata.host }}",
                "count": "{{ event.payload.count }}",
                "tags": "{{ event.payload.tags }}",
                "event_id": "{{ event.id }}",
                "trigger": "{{ event.trigger }}"
            },
            "timestamp": "{{ system.timestamp }}"
        });

        let result = resolve_templates(&template, &context).unwrap();
        assert_eq!(result["channel"], "#incidents");
        assert_eq!(
            result["message"],
            "🚨 Error in api-gateway: Connection timeout"
        );
        assert_eq!(result["severity"], "critical");
        assert_eq!(result["details"]["host"], "web-01");
        assert_eq!(result["details"]["count"], 42);
        assert_eq!(result["details"]["event_id"], 123);
        assert_eq!(result["details"]["trigger"], "core.error_event");
        assert_eq!(result["timestamp"], "2026-01-17T15:30:00Z");
    }

    #[test]
    fn test_context_without_event_metadata() {
        // Context with only a payload — no id, trigger, or created
        let context = TemplateContext::new(json!({"service": "test"}), json!({}), json!({}));

        let template = json!({
            "service": "{{ event.payload.service }}",
            "id": "{{ event.id }}",
            "trigger": "{{ event.trigger }}"
        });

        let result = resolve_templates(&template, &context).unwrap();
        assert_eq!(result["service"], "test");
        // Missing metadata returns null
        assert!(result["id"].is_null());
        assert!(result["trigger"].is_null());
    }

    #[test]
    fn test_unknown_source() {
        let context = create_test_context();
        let template = json!({
            "value": "{{ unknown.field }}"
        });

        let result = resolve_templates(&template, &context).unwrap();
        assert!(result["value"].is_null());
    }

    #[test]
    fn test_invalid_pack_path() {
        let context = create_test_context();
        let template = json!({
            "value": "{{ pack.invalid.field }}"
        });

        let result = resolve_templates(&template, &context).unwrap();
        assert!(result["value"].is_null());
    }
}
