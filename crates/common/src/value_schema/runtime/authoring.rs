use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use super::{item_schema, validator, Budget, CompiledContract, Walk};
use crate::value_schema::{check_json, error, pointer, ErrorCode, Result};

/// Successful literal checks do not establish conformance of rendered values.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuthoringReport {
    pub deferred_paths: Vec<String>,
}

#[derive(Clone, Copy)]
enum Bound {
    Possible,
    Certain,
}

impl Bound {
    fn inverse(self) -> Self {
        match self {
            Self::Possible => Self::Certain,
            Self::Certain => Self::Possible,
        }
    }
}

struct Projection<'a, 'b> {
    compiled: &'b CompiledContract<'a>,
    symbolic: BTreeSet<String>,
    budget: Budget,
    evaluations: usize,
}

impl CompiledContract<'_> {
    /// Check literals while deferring symbolic values. No sample values are
    /// fabricated. Negation and exclusive branches use conservative lower and
    /// upper validity bounds so relaxing a branch cannot reject a valid template.
    pub fn validate_authoring(&self, value: &Value) -> Result<AuthoringReport> {
        self.validate_authoring_inner(value)
            .map_err(|failure| self.sanitize_runtime_error(failure, value))
    }

    fn validate_authoring_inner(&self, value: &Value) -> Result<AuthoringReport> {
        check_json(
            value,
            self.contract.limits,
            self.contract.limits.max_value_bytes,
        )?;
        let mut symbolic = BTreeSet::new();
        find_symbolic(value, "", &mut symbolic);
        if symbolic.is_empty() {
            self.validate(value)?;
            return Ok(AuthoringReport {
                deferred_paths: Vec::new(),
            });
        }
        self.walk(
            &self.contract.annotated_schema,
            value,
            "",
            0,
            &mut Walk::validation(self.contract.limits),
        )?;
        let mut budget = Budget::new(self.contract.limits);
        budget.max_bytes = self.contract.limits.max_compiled_bytes;
        let mut projection = Projection {
            compiled: self,
            symbolic,
            budget,
            evaluations: 0,
        };
        let schema = projection.project(
            &self.contract.annotated_schema,
            value,
            "",
            0,
            Bound::Possible,
        )?;
        projection.evaluate(&schema, value).map_err(|mut failure| {
            // A rejected projection reports only a submitted field location.
            failure.code = match failure.code {
                ErrorCode::ResourceLimit => ErrorCode::ResourceLimit,
                _ => ErrorCode::InvalidValue,
            };
            failure
        })?;
        Ok(AuthoringReport {
            deferred_paths: projection.symbolic.into_iter().collect(),
        })
    }
}

impl Projection<'_, '_> {
    fn has_symbolic(&self, path: &str) -> bool {
        if path.is_empty() {
            return !self.symbolic.is_empty();
        }
        if self.symbolic.contains(path) {
            return true;
        }
        let prefix = format!("{path}/");
        self.symbolic
            .range(prefix.clone()..)
            .next()
            .is_some_and(|candidate| candidate.starts_with(&prefix))
    }

    fn project(
        &mut self,
        node: &Value,
        value: &Value,
        path: &str,
        depth: usize,
        bound: Bound,
    ) -> Result<Value> {
        self.budget.visit(depth, path)?;
        if !self.has_symbolic(path) {
            return self.copy(node, path);
        }
        let effective = self
            .compiled
            .effective(node, path, depth, &mut self.budget)?;
        if effective.schema.is_boolean() {
            return Ok(effective.schema.clone());
        }
        let mut map = effective
            .schema
            .as_object()
            .cloned()
            .ok_or_else(|| error(ErrorCode::InvalidDefinition, path))?;
        for keyword in [
            "title",
            "description",
            "default",
            "examples",
            "position",
            "secret",
            "format",
            "$defs",
            "$schema",
        ] {
            map.remove(keyword);
        }
        if self.symbolic.contains(path) {
            return Ok(Value::Bool(
                map.is_empty() || matches!(bound, Bound::Possible),
            ));
        }

        for keyword in ["const", "enum"] {
            if let Some(constraint) = map.remove(keyword) {
                let compatible = if keyword == "const" {
                    self.compatible(value, &constraint, path, depth + 1)?
                } else {
                    let mut compatible = false;
                    for candidate in constraint.as_array().into_iter().flatten() {
                        if self.compatible(value, candidate, path, depth + 1)? {
                            compatible = true;
                            break;
                        }
                    }
                    compatible
                };
                if !compatible || matches!(bound, Bound::Certain) {
                    return Ok(Value::Bool(false));
                }
            }
        }

        if let Some(object) = value.as_object() {
            let mut properties = effective
                .get("properties")
                .and_then(Value::as_object)
                .cloned()
                .unwrap_or_default();
            for (name, child) in object {
                let schema = effective
                    .get("properties")
                    .and_then(Value::as_object)
                    .and_then(|properties| properties.get(name))
                    .or_else(|| effective.get("additionalProperties"));
                if let Some(schema) = schema {
                    properties.insert(
                        name.clone(),
                        self.project(schema, child, &pointer(path, name), depth + 1, bound)?,
                    );
                }
            }
            if !properties.is_empty() {
                map.insert("properties".into(), Value::Object(properties));
            }
        }
        if let Some(array) = value.as_array() {
            let mut items = Vec::new();
            for (index, child) in array.iter().enumerate() {
                items.push(match item_schema(&effective, index) {
                    Some(schema) => self.project(
                        schema,
                        child,
                        &pointer(path, &index.to_string()),
                        depth + 1,
                        bound,
                    )?,
                    None => Value::Bool(true),
                });
            }
            if !items.is_empty() {
                map.insert("prefixItems".into(), Value::Array(items));
            }
            if map.get("uniqueItems") == Some(&Value::Bool(true)) {
                let literals: Vec<&Value> = array
                    .iter()
                    .enumerate()
                    .filter(|(index, _)| !self.has_symbolic(&pointer(path, &index.to_string())))
                    .map(|(_, item)| item)
                    .collect();
                if !validator(&json!({"uniqueItems": true}))?.is_valid(&json!(literals)) {
                    return Ok(Value::Bool(false));
                }
                if matches!(bound, Bound::Certain) && array.len() > 1 {
                    return Ok(Value::Bool(false));
                }
                map.remove("uniqueItems");
            }
            if let Some(contains) = effective.get("contains") {
                let mut certain = 0;
                let mut possible = 0;
                for (index, child) in array.iter().enumerate() {
                    let child_path = pointer(path, &index.to_string());
                    let upper =
                        self.project(contains, child, &child_path, depth + 1, Bound::Possible)?;
                    if self.matches(&upper, child)? {
                        possible += 1;
                        let lower =
                            self.project(contains, child, &child_path, depth + 1, Bound::Certain)?;
                        if self.matches(&lower, child)? {
                            certain += 1;
                        }
                    }
                }
                let minimum = effective
                    .get("minContains")
                    .and_then(Value::as_u64)
                    .unwrap_or(1);
                let maximum = effective
                    .get("maxContains")
                    .and_then(Value::as_u64)
                    .unwrap_or(u64::MAX);
                let valid = match bound {
                    Bound::Possible => possible >= minimum && certain <= maximum,
                    Bound::Certain => certain >= minimum && possible <= maximum,
                };
                if !valid {
                    return Ok(Value::Bool(false));
                }
                for keyword in ["contains", "minContains", "maxContains"] {
                    map.remove(keyword);
                }
            }
        }

        for keyword in ["allOf", "anyOf"] {
            if let Some(branches) = effective.get(keyword).and_then(Value::as_array) {
                let mut projected = Vec::new();
                for branch in branches {
                    projected.push(self.project(branch, value, path, depth + 1, bound)?);
                }
                map.insert(keyword.into(), Value::Array(projected));
            }
        }
        if let Some(branches) = effective.get("oneOf").and_then(Value::as_array) {
            let mut chosen = Vec::new();
            let mut excluded = Vec::new();
            for branch in branches {
                chosen.push(self.project(branch, value, path, depth + 1, bound)?);
                excluded.push(self.project(branch, value, path, depth + 1, bound.inverse())?);
            }
            let mut alternatives = Vec::new();
            for (index, branch) in chosen.iter().enumerate() {
                let mut conditions = vec![self.copy(branch, path)?];
                for (other, branch) in excluded.iter().enumerate() {
                    if index != other {
                        conditions.push(json!({"not": self.copy(branch, path)?}));
                    }
                }
                alternatives.push(json!({"allOf": conditions}));
            }
            map.remove("oneOf");
            // Keep independent anyOf constraints; don't overwrite their branches.
            let exclusive = json!({"anyOf": alternatives});
            let conjunction = map.entry("allOf").or_insert_with(|| json!([]));
            conjunction
                .as_array_mut()
                .ok_or_else(|| error(ErrorCode::InvalidDefinition, path))?
                .push(exclusive);
        }
        if let Some(branch) = effective.get("not") {
            map.insert(
                "not".into(),
                self.project(branch, value, path, depth + 1, bound.inverse())?,
            );
        }
        let result = Value::Object(map);
        self.charge(&result, path)?;
        Ok(result)
    }

    fn compatible(
        &mut self,
        value: &Value,
        candidate: &Value,
        path: &str,
        depth: usize,
    ) -> Result<bool> {
        self.budget.visit(depth, path)?;
        if self.symbolic.contains(path) {
            return Ok(true);
        }
        match (value, candidate) {
            (Value::Object(value), Value::Object(candidate)) => {
                if value.len() != candidate.len() {
                    return Ok(false);
                }
                for (name, child) in value {
                    let Some(candidate) = candidate.get(name) else {
                        return Ok(false);
                    };
                    if !self.compatible(child, candidate, &pointer(path, name), depth + 1)? {
                        return Ok(false);
                    }
                }
                Ok(true)
            }
            (Value::Array(value), Value::Array(candidate)) => {
                if value.len() != candidate.len() {
                    return Ok(false);
                }
                for (index, (child, candidate)) in value.iter().zip(candidate).enumerate() {
                    if !self.compatible(
                        child,
                        candidate,
                        &pointer(path, &index.to_string()),
                        depth + 1,
                    )? {
                        return Ok(false);
                    }
                }
                Ok(true)
            }
            (Value::Number(_), Value::Number(_)) => {
                Ok(validator(&json!({"const": candidate}))?.is_valid(value))
            }
            _ => Ok(value == candidate),
        }
    }

    fn charge(&mut self, schema: &Value, path: &str) -> Result<()> {
        let bytes = check_json(
            schema,
            self.compiled.contract.limits,
            self.compiled.contract.limits.max_compiled_bytes,
        )?;
        self.budget.add_bytes(bytes, path)
    }

    fn copy(&mut self, schema: &Value, path: &str) -> Result<Value> {
        self.charge(schema, path)?;
        Ok(schema.clone())
    }

    fn evaluate(&mut self, schema: &Value, value: &Value) -> Result<()> {
        self.evaluations += 1;
        if self.evaluations > self.compiled.contract.limits.max_literal_checks {
            return Err(error(ErrorCode::ResourceLimit, ""));
        }
        let mut map = Map::new();
        map.insert("allOf".into(), json!([schema]));
        map.insert(
            "$defs".into(),
            self.compiled.contract.validator_schema["$defs"].clone(),
        );
        let schema = Value::Object(map);
        check_json(
            &schema,
            self.compiled.contract.limits,
            self.compiled.contract.limits.max_compiled_bytes,
        )?;
        validator(&schema)?
            .validate(value)
            .map_err(|failure| error(ErrorCode::InvalidValue, failure.instance_path().to_string()))
    }

    fn matches(&mut self, schema: &Value, value: &Value) -> Result<bool> {
        match self.evaluate(schema, value) {
            Ok(()) => Ok(true),
            Err(failure) if failure.code == ErrorCode::InvalidValue => Ok(false),
            Err(failure) => Err(failure),
        }
    }
}

fn find_symbolic(value: &Value, path: &str, paths: &mut BTreeSet<String>) {
    match value {
        Value::String(text) if text.contains("{{") && text.contains("}}") => {
            paths.insert(path.into());
        }
        Value::Object(object) => {
            for (name, value) in object {
                find_symbolic(value, &pointer(path, name), paths);
            }
        }
        Value::Array(array) => {
            for (index, value) in array.iter().enumerate() {
                find_symbolic(value, &pointer(path, &index.to_string()), paths);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests;
