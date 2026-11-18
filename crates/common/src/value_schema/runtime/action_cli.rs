//! Schema-domain and secrecy queries for explicitly authored action bindings.
//! Keep these graph operations with the existing defaults/secret traversal.

use super::*;
use crate::action_cli::{CliBinding, Encoding};

impl CompiledContract<'_> {
    /// Returns true for any possibly secret selected value or ancestor. Domain
    /// queries are conservative: an untyped scalar is not assumed to be safe.
    pub(crate) fn check_argument_binding(
        &self,
        schema_path: &str,
        input_path: &[String],
        binding: &CliBinding,
    ) -> Result<bool> {
        let mut budget = Budget::new(self.contract.limits);
        if self.argument_path_is_secret(
            &self.contract.annotated_schema,
            input_path,
            0,
            schema_path,
            0,
            &mut budget,
        )? {
            return Ok(true);
        }
        let id = crate::value_schema::compiler::authored_node_id(schema_path)?;
        let schema = self
            .contract
            .annotated_schema
            .get("$defs")
            .and_then(|definitions| definitions.get(&id))
            .ok_or_else(|| error(ErrorCode::InvalidDefinition, schema_path))?;
        let effective = self.effective(schema, schema_path, 0, &mut budget)?;
        if matches!(binding, CliBinding::Flag { .. }) {
            check_types(&effective, &["boolean"], schema_path)?;
        } else if binding.expands() {
            check_types(&effective, &["array"], schema_path)?;
            if binding.encoding() == Encoding::Scalar {
                if let Some(prefix) = effective.get("prefixItems").and_then(Value::as_array) {
                    for item in prefix {
                        let item = self.effective(item, schema_path, 1, &mut budget)?;
                        check_types(&item, SCALAR_TYPES, schema_path)?;
                    }
                }
                match effective.get("items") {
                    Some(Value::Bool(false)) => {}
                    Some(item) => {
                        let item = self.effective(item, schema_path, 1, &mut budget)?;
                        check_types(&item, SCALAR_TYPES, schema_path)?;
                    }
                    None => return Err(error(ErrorCode::InvalidDefinition, schema_path)),
                }
            }
        } else if binding.encoding() == Encoding::Scalar {
            check_types(&effective, SCALAR_TYPES, schema_path)?;
        }
        Ok(false)
    }

    fn argument_path_is_secret(
        &self,
        schema: &Value,
        segments: &[String],
        index: usize,
        path: &str,
        depth: usize,
        budget: &mut Budget,
    ) -> Result<bool> {
        let effective = self.effective(schema, path, depth, budget)?;
        if effective.get("secret") == Some(&Value::Bool(true)) {
            return Ok(true);
        }
        let Some(segment) = segments.get(index) else {
            return self.node_declares_secrets(schema, path, depth, budget);
        };
        let child = effective
            .get("properties")
            .and_then(Value::as_object)
            .and_then(|properties| properties.get(segment))
            .or_else(|| effective.get("additionalProperties"));
        if let Some(child) = child {
            if self.argument_path_is_secret(child, segments, index + 1, path, depth + 1, budget)? {
                return Ok(true);
            }
        }
        for keyword in ["allOf", "anyOf", "oneOf"] {
            if let Some(branches) = effective.get(keyword).and_then(Value::as_array) {
                for branch in branches {
                    if self.argument_path_is_secret(
                        branch,
                        segments,
                        index,
                        path,
                        depth + 1,
                        budget,
                    )? {
                        return Ok(true);
                    }
                }
            }
        }
        if let Some(negated) = effective.get("not") {
            if self.argument_path_is_secret(negated, segments, index, path, depth + 1, budget)? {
                return Ok(true);
            }
        }
        Ok(false)
    }
}

const SCALAR_TYPES: &[&str] = &["string", "number", "integer", "boolean"];

fn check_types(effective: &Effective<'_>, allowed: &[&str], path: &str) -> Result<()> {
    let types = match effective.get("type") {
        Some(Value::String(kind)) => vec![kind.as_str()],
        Some(Value::Array(kinds)) => kinds.iter().filter_map(Value::as_str).collect(),
        _ => return Err(error(ErrorCode::InvalidDefinition, pointer(path, "cli"))),
    };
    let nonnull: Vec<_> = types.into_iter().filter(|kind| *kind != "null").collect();
    if nonnull.is_empty() || nonnull.iter().any(|kind| !allowed.contains(kind)) {
        return Err(error(ErrorCode::InvalidDefinition, pointer(path, "cli")));
    }
    Ok(())
}
