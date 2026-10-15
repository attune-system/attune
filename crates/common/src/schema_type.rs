//! Pack-managed definitions of one reusable value schema.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::models::ActionReferenceVisibility;
use crate::schema::RefValidator;
use crate::{Error, Result};

pub const PARAM_SCHEMA_FIELDS: &[&str] = &["parameters", "param_schema"];
pub const OUTPUT_SCHEMA_FIELDS: &[&str] = &["output", "out_schema", "output_schema"];

/// The existing authoring and metadata names describe one field. Reject
/// ambiguous declarations instead of allowing checking and loading to disagree.
pub fn read_schema_field(data: &serde_yaml_ng::Value, fields: &[&str]) -> Result<Option<Value>> {
    let present: Vec<_> = fields
        .iter()
        .filter_map(|field| data.get(*field).map(|value| (*field, value)))
        .collect();
    if present.len() > 1 {
        return Err(Error::validation(
            "Schema field is declared using multiple authoring/metadata names",
        ));
    }
    present
        .first()
        .map(|(_, value)| {
            serde_json::to_value(value)
                .map_err(|_| Error::validation("Schema field must contain JSON-compatible values"))
        })
        .transpose()
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SchemaTypeDefinition {
    pub r#ref: String,
    pub label: String,
    #[serde(default)]
    pub description: Option<String>,
    pub schema: Value,
    #[serde(default)]
    pub reference_visibility: ActionReferenceVisibility,
    #[serde(default)]
    pub reference_allowed_pack_refs: Vec<String>,
}

impl SchemaTypeDefinition {
    pub fn parse(content: &str, pack_ref: &str) -> Result<Self> {
        // YAML errors can quote literal values. Keep them out of diagnostics.
        let definition: Self = serde_yaml_ng::from_str(content)
            .map_err(|_| Error::validation("Invalid named type YAML definition"))?;
        definition.validate(pack_ref)?;
        Ok(definition)
    }

    pub fn validate(&self, pack_ref: &str) -> Result<()> {
        RefValidator::validate_component_ref(&self.r#ref)?;
        if self.r#ref.split_once('.').map(|(owner, _)| owner) != Some(pack_ref) {
            return Err(Error::validation(
                "Named type ref must belong to its declaring pack",
            ));
        }
        if self.label.trim().is_empty() || self.label.len() > 255 {
            return Err(Error::validation(
                "Named type label must contain 1-255 bytes",
            ));
        }
        if !self.schema.is_object() && !self.schema.is_boolean() {
            return Err(Error::validation(
                "Named type schema must describe one JSON Schema value",
            ));
        }
        for allowed in &self.reference_allowed_pack_refs {
            RefValidator::validate_pack_ref(allowed)?;
        }
        if self.reference_visibility != ActionReferenceVisibility::Restricted
            && !self.reference_allowed_pack_refs.is_empty()
        {
            return Err(Error::validation(
                "Named type allowed pack refs require restricted visibility",
            ));
        }
        Ok(())
    }

    pub fn reference_allowed(&self, declaring_pack: &str) -> bool {
        let owner = self.r#ref.split_once('.').map(|(owner, _)| owner);
        match self.reference_visibility {
            ActionReferenceVisibility::Public => true,
            ActionReferenceVisibility::Private => owner == Some(declaring_pack),
            ActionReferenceVisibility::Restricted => {
                owner == Some(declaring_pack)
                    || self
                        .reference_allowed_pack_refs
                        .iter()
                        .any(|allowed| allowed == declaring_pack)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn schema_aliases_are_unambiguous() {
        let data = serde_yaml_ng::from_str("param_schema: {value: {type: string}}").unwrap();
        assert!(read_schema_field(&data, PARAM_SCHEMA_FIELDS)
            .unwrap()
            .is_some());
        let data = serde_yaml_ng::from_str("parameters: {}\nparam_schema: {}").unwrap();
        assert!(read_schema_field(&data, PARAM_SCHEMA_FIELDS).is_err());
    }
    use super::*;

    fn compile_test_definition(
        schema: &Value,
    ) -> crate::value_schema::Result<crate::value_schema::Contract> {
        crate::value_schema::compile_inline(
            "inline-consumer",
            crate::value_schema::SchemaContext::FlatDocument,
            schema,
        )
    }

    #[test]
    fn inline_consumer_writes_reject_invalid_defaults_and_secret_literals() {
        use serde_json::json;
        assert!(compile_test_definition(&json!({
            "count":{"type":"integer","required":true,"default":5}
        }))
        .is_ok());
        assert!(compile_test_definition(&json!({
            "count":{"type":"integer","default":"wrong type"}
        }))
        .is_err());
        for keyword in ["default", "const", "examples"] {
            let value = if keyword == "examples" {
                json!(["private-schema-fixture"])
            } else {
                json!("private-schema-fixture")
            };
            let error = compile_test_definition(&json!({
                "token":{"type":"string","secret":true,(keyword):value}
            }))
            .unwrap_err();
            assert!(!error.to_string().contains("private-schema-fixture"));
        }
    }

    #[test]
    fn types_are_namespaced_single_values_and_visibility_is_explicit() {
        let definition = SchemaTypeDefinition::parse("ref: demo.address\nlabel: Address\nschema:\n  type: object\nreference_visibility: private\n", "demo").unwrap();
        assert!(definition.reference_allowed("demo"));
        assert!(!definition.reference_allowed("other"));
        assert!(SchemaTypeDefinition::parse(
            "ref: other.address\nlabel: Address\nschema: true\n",
            "demo"
        )
        .is_err());
        assert!(SchemaTypeDefinition::parse(
            "ref: address\nlabel: Address\nschema: true\n",
            "demo"
        )
        .is_err());
        assert!(SchemaTypeDefinition::parse(
            "ref: demo.address\nlabel: Address\nschema: []\n",
            "demo"
        )
        .is_err());
        assert!(SchemaTypeDefinition::parse(
            "ref: demo.address\nlabel: Address\nschema: true\nextra: SECRET_LITERAL\n",
            "demo"
        )
        .unwrap_err()
        .to_string()
        .contains("Invalid named type YAML"));
    }
}
