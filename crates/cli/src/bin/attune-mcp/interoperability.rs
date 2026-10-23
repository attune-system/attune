//! Small MCP parity operations using the same public API and authored definitions.
use anyhow::{ensure, Result};
use serde_json::{json, Value};

pub fn reset_body(fields: &Value, confirmed: bool) -> Result<Value> {
    ensure!(confirmed, "Configuration reset requires confirm_reset=true");
    let fields = fields
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("fields must be an array"))?;
    ensure!(!fields.is_empty(), "At least one reset field is required");
    let mut names = std::collections::BTreeSet::new();
    for field in fields {
        let field = field
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Reset fields must be strings"))?;
        ensure!(
            !field.is_empty() && names.insert(field),
            "Reset fields must be nonempty and unique"
        );
    }
    Ok(json!({"config_reset_fields": fields}))
}

pub fn grouping_body(config: Value, grouping: &str) -> Result<Value> {
    ensure!(
        matches!(grouping, "aggregate" | "separate"),
        "grouping must be aggregate or separate"
    );
    let mut config = match config {
        Value::Null => serde_json::Map::new(),
        Value::Object(config) => config,
        _ => anyhow::bail!("Queue config must be an object"),
    };
    let dispatch = config.entry("dispatch").or_insert_with(|| json!({}));
    let dispatch = dispatch
        .as_object_mut()
        .ok_or_else(|| anyhow::anyhow!("Queue dispatch must be an object"))?;
    dispatch.insert("separate_requesters".into(), json!(grouping == "separate"));
    Ok(json!({"config": config}))
}

pub fn schema(operation: &str) -> Value {
    let mut properties = json!({"ref":{"type":"string"}});
    let mut required = vec!["ref"];
    match operation {
        "reset" => {
            properties["fields"] = json!({"type":"array","minItems":1,"uniqueItems":true,"items":{"type":"string","minLength":1}});
            properties["confirm_reset"] = json!({"type":"boolean","const":true});
            required.extend(["fields", "confirm_reset"]);
        }
        "grouping" => {
            properties["grouping"] = json!({"type":"string","enum":["aggregate","separate"]});
            required.push("grouping");
        }
        "import" => {
            properties["definition"] = json!({"type":"object","description":"Exact canonical authorable definition from permission_sets_export, not effective grants"});
            properties["confirm_import"] = json!({"type":"boolean","const":true});
            required.extend(["definition", "confirm_import"]);
        }
        _ => {}
    }
    json!({"type":"object","properties":properties,"required":required,"additionalProperties":false})
}

pub fn reset_schema() -> Value {
    schema("reset")
}
pub fn grouping_schema() -> Value {
    schema("grouping")
}
pub fn export_schema() -> Value {
    schema("export")
}
pub fn import_schema() -> Value {
    schema("import")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grouping_preserves_unrelated_configuration_and_requires_explicit_optin() {
        let original = json!({"ack_contract":{"mode":"manual"},"dispatch":{"batch_size":3}});
        let separate = grouping_body(original.clone(), "separate").unwrap();
        assert_eq!(separate["config"]["dispatch"]["separate_requesters"], true);
        assert_eq!(separate["config"]["dispatch"]["batch_size"], 3);
        assert_eq!(separate["config"]["ack_contract"], original["ack_contract"]);
        let aggregate = grouping_body(separate["config"].clone(), "aggregate").unwrap();
        assert_eq!(
            aggregate["config"]["dispatch"]["separate_requesters"],
            false
        );
        assert!(grouping_body(original, "implicit").is_err());
        assert!(grouping_body(json!({"dispatch":false}), "separate").is_err());
    }

    #[test]
    fn reset_is_literal_confirmed_nonempty_and_unique() {
        assert_eq!(
            reset_body(&json!(["literal.dot"]), true).unwrap(),
            json!({"config_reset_fields":["literal.dot"]})
        );
        for fields in [json!([]), json!([""]), json!([1]), json!(["a", "a"])] {
            assert!(reset_body(&fields, true).is_err());
        }
        assert!(reset_body(&json!(["a"]), false).is_err());
    }
}
