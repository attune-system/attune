//! Database schema utilities
//!
//! This module provides utilities for working with database schemas,
//! including query builders and schema validation.

use serde_json::Value as JsonValue;

use crate::error::{Error, Result};

/// Database schema name
pub const SCHEMA_NAME: &str = "attune";

/// Table identifiers
#[derive(Debug, Clone, Copy)]
pub enum Table {
    Pack,
    Runtime,
    Worker,
    Trigger,
    Sensor,
    Action,
    Rule,
    Event,
    Enforcement,
    Execution,
    Inquiry,
    Identity,
    PermissionSet,
    PermissionAssignment,
    Policy,
    Key,
    Notification,
    Artifact,
}

impl Table {
    /// Get the table name as a string
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Pack => "pack",
            Self::Runtime => "runtime",
            Self::Worker => "worker",
            Self::Trigger => "trigger",
            Self::Sensor => "sensor",
            Self::Action => "action",
            Self::Rule => "rule",
            Self::Event => "event",
            Self::Enforcement => "enforcement",
            Self::Execution => "execution",
            Self::Inquiry => "inquiry",
            Self::Identity => "identity",
            Self::PermissionSet => "permission_set",
            Self::PermissionAssignment => "permission_assignment",
            Self::Policy => "policy",
            Self::Key => "key",
            Self::Notification => "notification",
            Self::Artifact => "artifact",
        }
    }
}

/// Common column identifiers
#[derive(Debug, Clone, Copy)]
pub enum Column {
    Id,
    Ref,
    Pack,
    PackRef,
    Label,
    Description,
    Version,
    Name,
    Status,
    Created,
    Updated,
    Enabled,
    Config,
    Meta,
    Tags,
    RuntimeType,
    WorkerType,
    Entrypoint,
    Runtime,
    RuntimeRef,
    Trigger,
    TriggerRef,
    Action,
    ActionRef,
    Rule,
    RuleRef,
    ParamSchema,
    OutSchema,
    ConfSchema,
    Payload,
    Response,
    ResponseSchema,
    Result,
    Execution,
    Enforcement,
    Executor,
    Prompt,
    AssignedTo,
    TimeoutAt,
    RespondedAt,
    Login,
    DisplayName,
    Attributes,
    Owner,
    OwnerType,
    Encrypted,
    Value,
    Channel,
    Entity,
    EntityType,
    Activity,
    State,
    Content,
}

/// JSON Schema validator
pub struct SchemaValidator {
    schema: JsonValue,
}

impl SchemaValidator {
    /// Create a new schema validator
    pub fn new(schema: JsonValue) -> Result<Self> {
        // Validate that the schema itself is valid JSON Schema
        if !schema.is_object() {
            return Err(Error::schema_validation("Schema must be a JSON object"));
        }

        Ok(Self { schema })
    }

    /// Validate data against the schema
    pub fn validate(&self, data: &JsonValue) -> Result<()> {
        // Use jsonschema crate for validation
        let compiled = jsonschema::validator_for(&self.schema)
            .map_err(|e| Error::schema_validation(format!("Failed to compile schema: {}", e)))?;

        if let Err(error) = compiled.validate(data) {
            return Err(Error::schema_validation(format!(
                "Validation failed: {}",
                error
            )));
        }
        Ok(())
    }

    /// Get the underlying schema
    pub fn schema(&self) -> &JsonValue {
        &self.schema
    }
}

/// Reference format validator
pub struct RefValidator;

impl RefValidator {
    /// Validate pack.component format (e.g., "core.webhook")
    pub fn validate_component_ref(ref_str: &str) -> Result<()> {
        let parts: Vec<&str> = ref_str.split('.').collect();
        if parts.len() != 2 {
            return Err(Error::validation(format!(
                "Invalid component reference format: '{}'. Expected 'pack.component'",
                ref_str
            )));
        }

        Self::validate_identifier(parts[0])?;
        Self::validate_identifier(parts[1])?;

        Ok(())
    }

    /// Validate pack.name format (e.g., "core.python", "core.shell")
    pub fn validate_runtime_ref(ref_str: &str) -> Result<()> {
        let parts: Vec<&str> = ref_str.split('.').collect();
        if parts.len() != 2 {
            return Err(Error::validation(format!(
                "Invalid runtime reference format: '{}'. Expected 'pack.name' (e.g., 'core.python')",
                ref_str
            )));
        }

        Self::validate_identifier(parts[0])?;
        Self::validate_identifier(parts[1])?;

        Ok(())
    }

    /// Validate pack reference format (simple identifier)
    pub fn validate_pack_ref(ref_str: &str) -> Result<()> {
        Self::validate_identifier(ref_str)
    }

    /// Validate work queue reference format (`segment[.segment...]`).
    pub fn validate_work_queue_ref(ref_str: &str) -> Result<()> {
        let mut parts = ref_str.split('.');
        let Some(first) = parts.next() else {
            return Err(Error::validation(format!(
                "Invalid work queue reference format: '{}'",
                ref_str
            )));
        };

        let is_valid_segment = |segment: &str, must_start_with_letter: bool| {
            !segment.is_empty()
                && (!must_start_with_letter
                    || segment
                        .chars()
                        .next()
                        .is_some_and(|ch| ch.is_ascii_lowercase()))
                && segment.chars().all(|ch| {
                    ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-' || ch == '_'
                })
        };

        if !is_valid_segment(first, true) || !parts.all(|part| is_valid_segment(part, false)) {
            return Err(Error::validation(format!(
                "Invalid work queue reference format: '{}'",
                ref_str
            )));
        }

        Ok(())
    }

    /// Validate identifier (lowercase alphanumeric with hyphens/underscores)
    fn validate_identifier(identifier: &str) -> Result<()> {
        if identifier.is_empty() {
            return Err(Error::validation("Identifier cannot be empty"));
        }

        // Must start with lowercase letter
        if !identifier.chars().next().unwrap().is_ascii_lowercase() {
            return Err(Error::validation(format!(
                "Identifier '{}' must start with a lowercase letter",
                identifier
            )));
        }

        // Must contain only lowercase alphanumeric, hyphens, or underscores
        for ch in identifier.chars() {
            if !ch.is_ascii_lowercase() && !ch.is_ascii_digit() && ch != '-' && ch != '_' {
                return Err(Error::validation(format!(
                    "Identifier '{}' contains invalid character '{}'. Only lowercase letters, digits, hyphens, and underscores are allowed",
                    identifier, ch
                )));
            }
        }

        Ok(())
    }
}

/// Build a qualified table name with schema
pub fn qualified_table(table: Table) -> String {
    format!("{}.{}", SCHEMA_NAME, table.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_table_as_str() {
        assert_eq!(Table::Pack.as_str(), "pack");
        assert_eq!(Table::Action.as_str(), "action");
        assert_eq!(Table::Execution.as_str(), "execution");
    }

    #[test]
    fn test_qualified_table() {
        assert_eq!(qualified_table(Table::Pack), "attune.pack");
        assert_eq!(qualified_table(Table::Action), "attune.action");
    }

    #[test]
    fn test_ref_validator_component() {
        assert!(RefValidator::validate_component_ref("core.webhook").is_ok());
        assert!(RefValidator::validate_component_ref("my-pack.my-action").is_ok());
        assert!(RefValidator::validate_component_ref("pack_name.component_name").is_ok());

        // Invalid formats
        assert!(RefValidator::validate_component_ref("nopack").is_err());
        assert!(RefValidator::validate_component_ref("too.many.parts").is_err());
        assert!(RefValidator::validate_component_ref("Capital.name").is_err());
        assert!(RefValidator::validate_component_ref("pack.Name").is_err());
    }

    #[test]
    fn test_ref_validator_runtime() {
        assert!(RefValidator::validate_runtime_ref("core.python").is_ok());
        assert!(RefValidator::validate_runtime_ref("core.shell").is_ok());
        assert!(RefValidator::validate_runtime_ref("mypack.nodejs").is_ok());
        assert!(RefValidator::validate_runtime_ref("core.native").is_ok());

        // Invalid formats
        assert!(RefValidator::validate_runtime_ref("core.action.webhook").is_err()); // 3-part no longer valid
        assert!(RefValidator::validate_runtime_ref("python").is_err()); // missing pack
        assert!(RefValidator::validate_runtime_ref("Core.python").is_err()); // uppercase
    }

    #[test]
    fn pack_ref_validation_accepts_and_rejects_the_supported_grammar() {
        for valid in [
            "core",
            "simple",
            "my-pack",
            "pack_name",
            "with_underscores",
            "with-hyphens",
            "mixed_all-together-123",
        ] {
            assert!(
                RefValidator::validate_pack_ref(valid).is_ok(),
                "expected '{valid}' to be a valid pack ref"
            );
        }

        for invalid in [
            "",
            "Core",
            "pack.name",
            "pack name",
            "invalid pack!@#",
            "1starts_with_digit",
        ] {
            assert!(
                matches!(
                    RefValidator::validate_pack_ref(invalid),
                    Err(Error::Validation { .. })
                ),
                "expected '{invalid}' to produce a validation error"
            );
        }
    }

    #[test]
    fn test_ref_validator_work_queue() {
        assert!(RefValidator::validate_work_queue_ref("core.inbox").is_ok());
        assert!(RefValidator::validate_work_queue_ref("ops.retries.high").is_ok());
        assert!(RefValidator::validate_work_queue_ref("queue_1").is_ok());

        assert!(RefValidator::validate_work_queue_ref("").is_err());
        assert!(RefValidator::validate_work_queue_ref("1queue").is_err());
        assert!(RefValidator::validate_work_queue_ref("queue.").is_err());
        assert!(RefValidator::validate_work_queue_ref(".queue").is_err());
        assert!(RefValidator::validate_work_queue_ref("queue..part").is_err());
    }

    #[test]
    fn test_schema_validator() {
        let schema = json!({
            "type": "object",
            "properties": {
                "name": {"type": "string"},
                "age": {"type": "number"}
            },
            "required": ["name"]
        });

        let validator = SchemaValidator::new(schema).unwrap();

        // Valid data
        let valid_data = json!({"name": "John", "age": 30});
        assert!(validator.validate(&valid_data).is_ok());

        // Missing required field
        let invalid_data = json!({"age": 30});
        assert!(validator.validate(&invalid_data).is_err());

        // Wrong type
        let invalid_data = json!({"name": "John", "age": "thirty"});
        assert!(validator.validate(&invalid_data).is_err());
    }

    #[test]
    fn test_schema_validator_invalid_schema() {
        let invalid_schema = json!("not an object");
        assert!(SchemaValidator::new(invalid_schema).is_err());
    }
}
