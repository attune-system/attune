use std::collections::{BTreeMap, BTreeSet};

use jsonschema::{Draft, Retrieve, Uri, Validator};
use serde_json::{json, Value};

use super::{
    check_json, digest, error, pointer, Contract, ErrorCode, Limits, Result, COMPILER_CONTRACT,
};

mod authoring;
pub use authoring::AuthoringReport;

struct DenyRetrieval;

impl Retrieve for DenyRetrieval {
    fn retrieve(
        &self,
        _: &Uri<String>,
    ) -> std::result::Result<Value, Box<dyn std::error::Error + Send + Sync>> {
        Err("external schema retrieval is disabled".into())
    }
}

/// Prepared validation and graph-aware operations for one immutable contract.
/// Runtime consumers must not substitute current registry definitions.
pub struct CompiledContract<'a> {
    contract: &'a Contract,
    validator: Validator,
}

/// Borrow the target shape instead of cloning large shared objects at each alias.
struct Effective<'a> {
    schema: &'a Value,
    annotations: BTreeMap<&'a str, &'a Value>,
}

impl<'a> Effective<'a> {
    fn get(&self, keyword: &str) -> Option<&'a Value> {
        self.annotations
            .get(keyword)
            .copied()
            .or_else(|| self.schema.get(keyword))
    }
}

struct Budget {
    remaining: usize,
    max_depth: usize,
    bytes: usize,
    max_bytes: usize,
}

struct Walk {
    budget: Budget,
    secret_paths: Option<BTreeSet<String>>,
    rule_templates: bool,
}

impl Walk {
    fn validation(limits: Limits) -> Self {
        Self {
            budget: Budget::new(limits),
            secret_paths: None,
            rule_templates: false,
        }
    }

    fn secrets(limits: Limits) -> Self {
        Self {
            budget: Budget::new(limits),
            secret_paths: Some(BTreeSet::new()),
            rule_templates: false,
        }
    }
}

impl Budget {
    fn new(limits: Limits) -> Self {
        Self {
            remaining: limits.max_value_nodes,
            // The bundle's root ref is a transport edge, not an authored edge.
            max_depth: limits.max_depth.saturating_add(1),
            bytes: 0,
            max_bytes: limits.max_value_bytes,
        }
    }

    fn visit(&mut self, depth: usize, path: &str) -> Result<()> {
        if depth > self.max_depth || self.remaining == 0 {
            return Err(error(ErrorCode::ResourceLimit, path));
        }
        self.remaining -= 1;
        Ok(())
    }

    fn add_bytes(&mut self, bytes: usize, path: &str) -> Result<()> {
        self.bytes = self
            .bytes
            .checked_add(bytes)
            .ok_or_else(|| error(ErrorCode::ResourceLimit, path))?;
        if self.bytes > self.max_bytes {
            return Err(error(ErrorCode::ResourceLimit, path));
        }
        Ok(())
    }
}

impl<'a> CompiledContract<'a> {
    fn public_error_path(&self, path: &str, value: &Value) -> String {
        let mut public = String::new();
        let mut candidates = vec![&self.contract.annotated_schema];
        let mut budget = Budget::new(self.contract.limits);
        let Some(tail) = path.strip_prefix('/') else {
            return public;
        };
        for escaped in tail.split('/') {
            let segment = escaped.replace("~1", "/").replace("~0", "~");
            let mut pending = candidates;
            let mut next = Vec::new();
            let mut visited = BTreeSet::new();
            while let Some(node) = pending.pop() {
                if !visited.insert(node as *const Value as usize) {
                    continue;
                }
                let effective = match self.effective(node, "", 0, &mut budget) {
                    Ok(effective) => effective,
                    Err(_) => return public,
                };
                if effective.get("secret") == Some(&Value::Bool(true)) {
                    return public;
                }
                if let Some(property) = effective
                    .get("properties")
                    .and_then(Value::as_object)
                    .and_then(|properties| properties.get(&segment))
                {
                    next.push(property);
                }
                if value.pointer(&public).is_some_and(Value::is_array) {
                    if let Ok(index) = segment.parse::<usize>() {
                        if let Some(item) = item_schema(&effective, index) {
                            next.push(item);
                        }
                    }
                }
                for keyword in ["allOf", "anyOf", "oneOf"] {
                    if let Some(branches) = effective.get(keyword).and_then(Value::as_array) {
                        pending.extend(branches);
                    }
                }
            }
            // Dynamic object keys are data, not public schema field names.
            if next.is_empty() {
                return public;
            }
            public = pointer(&public, &segment);
            candidates = next;
        }
        public
    }

    fn sanitize_runtime_error(
        &self,
        mut failure: super::SchemaError,
        value: &Value,
    ) -> super::SchemaError {
        if failure.source_ref.is_none() {
            failure.path = self.public_error_path(&failure.path, value);
        }
        failure
    }
    /// Conservative declaration test for log/mirror protection before output
    /// values exist. Generated definitions contain schema nodes, not data keys.
    pub fn declares_secrets(&self) -> bool {
        self.contract.annotated_schema.get("secret") == Some(&Value::Bool(true))
            || self
                .contract
                .annotated_schema
                .get("$defs")
                .and_then(Value::as_object)
                .is_some_and(|definitions| {
                    definitions
                        .values()
                        .any(|node| node.get("secret") == Some(&Value::Bool(true)))
                })
    }
    pub fn new(contract: &'a Contract) -> Result<Self> {
        let mut preimage = contract.clone();
        preimage.binding_digest.clear();
        if digest(&preimage)? != contract.binding_digest {
            return Err(error(ErrorCode::InvalidDefinition, "/binding_digest"));
        }
        Self::unsealed(contract)
    }

    pub(super) fn unsealed(contract: &'a Contract) -> Result<Self> {
        if contract.compiler_contract != COMPILER_CONTRACT {
            return Err(error(ErrorCode::InvalidDefinition, "/compiler_contract"));
        }
        let validator = validator(&contract.validator_schema).map_err(|mut failure| {
            if let Some(rest) = failure.path.strip_prefix("/$defs/") {
                let (id, suffix) = rest.split_once('/').unwrap_or((rest, ""));
                if let Some(location) = contract.node_locations.get(id) {
                    failure.path = if suffix.is_empty() {
                        location.path.clone()
                    } else {
                        format!("{}/{suffix}", location.path)
                    };
                    failure.source_ref = Some(location.source_ref.clone());
                }
            }
            failure
        })?;
        Ok(Self {
            contract,
            validator,
        })
    }

    /// Strict validation only. Templates are ordinary strings in this mode.
    pub fn validate(&self, value: &Value) -> Result<()> {
        check_json(
            value,
            self.contract.limits,
            self.contract.limits.max_value_bytes,
        )?;
        // Even a tiny value can cause exponential evaluation through acyclic
        // composition DAGs. Bound graph/value work before invoking the validator.
        self.walk(
            &self.contract.annotated_schema,
            value,
            "",
            0,
            &mut Walk::validation(self.contract.limits),
        )
        .map_err(|failure| self.sanitize_runtime_error(failure, value))?;
        self.validator.validate(value).map_err(|failure| {
            // Never use Display: it includes the rejected instance in many cases.
            error(
                ErrorCode::InvalidValue,
                self.public_error_path(&failure.instance_path().to_string(), value),
            )
        })
    }

    /// Return a new value; failure never partially mutates the caller's input.
    /// Defaults fill missing properties, not null or other present values.
    pub fn apply_defaults(&self, value: &Value) -> Result<Value> {
        let bytes = check_json(
            value,
            self.contract.limits,
            self.contract.limits.max_value_bytes,
        )?;
        let mut value = value.clone();
        let mut budget = Budget::new(self.contract.limits);
        budget.add_bytes(bytes, "")?;
        self.defaults(
            &self.contract.annotated_schema,
            &mut value,
            "",
            0,
            &mut budget,
        )
        .map_err(|failure| self.sanitize_runtime_error(failure, &value))?;
        check_json(
            &value,
            self.contract.limits,
            self.contract.limits.max_value_bytes,
        )?;
        Ok(value)
    }

    /// Concrete JSON Pointers for existing values. Composition branches are
    /// unioned conservatively, independent of which branch validates the value.
    pub fn secret_paths(&self, value: &Value) -> Result<Vec<String>> {
        check_json(
            value,
            self.contract.limits,
            self.contract.limits.max_value_bytes,
        )?;
        let mut walk = Walk::secrets(self.contract.limits);
        self.walk(&self.contract.annotated_schema, value, "", 0, &mut walk)
            .map_err(|failure| self.sanitize_runtime_error(failure, value))?;
        let paths = walk.secret_paths.unwrap_or_default();
        // A secret parent covers descendants; emit a minimal deterministic set.
        Ok(paths
            .iter()
            .filter(|path| {
                if path.is_empty() {
                    return true;
                }
                !path
                    .match_indices('/')
                    .any(|(index, _)| paths.contains(&path[..index]))
            })
            .cloned()
            .collect())
    }

    /// Rule definitions store Key selectors, not plaintext secret values.
    /// Inspect effective defaults too, without persisting them into the rule.
    pub fn validate_rule_secret_templates(&self, value: &Value) -> Result<()> {
        let value = self.apply_defaults(value)?;
        let mut walk = Walk::secrets(self.contract.limits);
        walk.rule_templates = true;
        self.walk(&self.contract.annotated_schema, &value, "", 0, &mut walk)
            .map_err(|failure| self.sanitize_runtime_error(failure, &value))?;
        let mut budget = Budget::new(self.contract.limits);
        for path in walk.secret_paths.unwrap_or_default() {
            let child = value
                .pointer(&path)
                .ok_or_else(|| error(ErrorCode::InvalidValue, &path))?;
            Self::validate_key_template_tree(child, &path, 0, &mut budget)
                .map_err(|failure| self.sanitize_runtime_error(failure, &value))?;
        }
        Ok(())
    }

    fn validate_key_template_tree(
        value: &Value,
        path: &str,
        depth: usize,
        budget: &mut Budget,
    ) -> Result<()> {
        budget.visit(depth, path)?;
        match value {
            Value::String(value) if crate::template_resolver::is_key_template(value) => Ok(()),
            Value::Array(values) => {
                for value in values {
                    Self::validate_key_template_tree(value, path, depth + 1, budget)?;
                }
                Ok(())
            }
            Value::Object(values) => {
                for value in values.values() {
                    Self::validate_key_template_tree(value, path, depth + 1, budget)?;
                }
                Ok(())
            }
            _ => Err(error(ErrorCode::InvalidValue, path)),
        }
    }

    fn node_declares_secrets(
        &self,
        node: &Value,
        path: &str,
        depth: usize,
        budget: &mut Budget,
    ) -> Result<bool> {
        let effective = self.effective(node, path, depth, budget)?;
        if effective.get("secret") == Some(&Value::Bool(true)) {
            return Ok(true);
        }
        if let Some(properties) = effective.get("properties").and_then(Value::as_object) {
            for child in properties.values() {
                if self.node_declares_secrets(child, path, depth + 1, budget)? {
                    return Ok(true);
                }
            }
        }
        for keyword in ["items", "additionalProperties", "contains", "not"] {
            if let Some(child) = effective.get(keyword) {
                if self.node_declares_secrets(child, path, depth + 1, budget)? {
                    return Ok(true);
                }
            }
        }
        for keyword in ["prefixItems", "allOf", "anyOf", "oneOf"] {
            if let Some(children) = effective.get(keyword).and_then(Value::as_array) {
                for child in children {
                    if self.node_declares_secrets(child, path, depth + 1, budget)? {
                        return Ok(true);
                    }
                }
            }
        }
        Ok(false)
    }

    pub(super) fn check_literals(&self) -> Result<()> {
        let definitions = self
            .contract
            .annotated_schema
            .get("$defs")
            .and_then(Value::as_object)
            .ok_or_else(|| error(ErrorCode::InvalidDefinition, "/$defs"))?;
        let inherited_secret_nodes = self.secret_ancestor_nodes(definitions)?;
        let mut checks = 0;
        // Aliases need checks too: a use-site default may override a valid type
        // default, or extra secret marking may prohibit an inherited literal.
        for (id, node) in definitions {
            let path = self
                .contract
                .node_locations
                .get(id)
                .map(|location| location.path.clone())
                .ok_or_else(|| error(ErrorCode::InvalidDefinition, pointer("/$defs", id)))?;
            let mut budget = Budget::new(self.contract.limits);
            let effective = self.effective(node, &path, 0, &mut budget)?;
            if inherited_secret_nodes.contains(&(node as *const Value as usize)) {
                if let Some(types) = effective.get("type") {
                    let allowed = |value: &Value| {
                        matches!(value.as_str(), Some("string" | "array" | "object"))
                    };
                    if !match types {
                        Value::Array(types) => types.iter().all(allowed),
                        value => allowed(value),
                    } {
                        return Err(self.definition_error(
                            ErrorCode::InvalidDefinition,
                            id,
                            "type",
                        )?);
                    }
                }
                for keyword in ["const", "enum", "default", "examples"] {
                    if effective.get(keyword).is_some() {
                        return Err(self.definition_error(
                            ErrorCode::SecretLiteral,
                            id,
                            keyword,
                        )?);
                    }
                }
            }
            // A public enclosing literal must not embed a secret descendant.
            for keyword in ["const", "enum"] {
                if let Some(literal) = effective.get(keyword) {
                    let values = if keyword == "enum" {
                        literal.as_array().map(Vec::as_slice).unwrap_or(&[])
                    } else {
                        std::slice::from_ref(literal)
                    };
                    for value in values {
                        checks += 1;
                        if checks > self.contract.limits.max_literal_checks {
                            return Err(error(ErrorCode::ResourceLimit, &path));
                        }
                        self.no_secret_literal(node, value, &pointer(&path, keyword))
                            .map_err(|mut failure| {
                                failure.source_ref = self
                                    .contract
                                    .node_locations
                                    .get(id)
                                    .map(|location| location.source_ref.clone());
                                failure
                            })?;
                    }
                }
            }
            for keyword in ["allOf", "anyOf", "oneOf", "not", "contains"] {
                if let Some(branches) = effective.get(keyword) {
                    let branches: Vec<&Value> = match branches {
                        Value::Array(branches) => branches.iter().collect(),
                        branch => vec![branch],
                    };
                    for branch in branches {
                        let mut seen = BTreeSet::new();
                        if self.branch_has_default(branch, &mut seen, &mut budget, 0)? {
                            return Err(self.definition_error(
                                ErrorCode::InvalidDefault,
                                id,
                                keyword,
                            )?);
                        }
                    }
                }
            }
            if let Some(default) = effective.get("default") {
                checks += 1;
                if checks > self.contract.limits.max_literal_checks {
                    return Err(error(ErrorCode::ResourceLimit, &path));
                }
                let schema = json!({"$schema": super::DIALECT, "$ref": format!("#/$defs/{id}"), "$defs": self.contract.validator_schema.get("$defs")});
                let validator = validator(&schema)?;
                self.walk(
                    node,
                    default,
                    "",
                    0,
                    &mut Walk::validation(self.contract.limits),
                )?;
                if !validator.is_valid(default) {
                    return Err(self.definition_error(ErrorCode::InvalidDefault, id, "default")?);
                }
                self.no_secret_literal(node, default, &pointer(&path, "default"))
                    .map_err(|mut failure| {
                        failure.source_ref = self
                            .contract
                            .node_locations
                            .get(id)
                            .map(|location| location.source_ref.clone());
                        failure
                    })?;
            }
            if let Some(examples) = effective.get("examples").and_then(Value::as_array) {
                for (index, example) in examples.iter().enumerate() {
                    checks += 1;
                    if checks > self.contract.limits.max_literal_checks {
                        return Err(error(ErrorCode::ResourceLimit, &path));
                    }
                    self.no_secret_literal(
                        node,
                        example,
                        &pointer(&pointer(&path, "examples"), &index.to_string()),
                    )
                    .map_err(|mut failure| {
                        failure.source_ref = self
                            .contract
                            .node_locations
                            .get(id)
                            .map(|location| location.source_ref.clone());
                        failure
                    })?;
                }
            }
        }
        Ok(())
    }

    fn secret_ancestor_nodes(
        &self,
        definitions: &serde_json::Map<String, Value>,
    ) -> Result<BTreeSet<usize>> {
        let mut pending = definitions
            .values()
            .filter(|node| node.get("secret") == Some(&Value::Bool(true)))
            .collect::<Vec<_>>();
        let mut sensitive = BTreeSet::new();
        let mut budget = Budget::new(self.contract.limits);
        while let Some(node) = pending.pop() {
            if !sensitive.insert(node as *const Value as usize) {
                continue;
            }
            budget.visit(0, "")?;
            if let Some(reference) = node.get("$ref").and_then(Value::as_str) {
                let target = reference
                    .strip_prefix('#')
                    .and_then(|path| self.contract.annotated_schema.pointer(path))
                    .ok_or_else(|| error(ErrorCode::InvalidReference, ""))?;
                pending.push(target);
            }
            if let Some(properties) = node.get("properties").and_then(Value::as_object) {
                pending.extend(properties.values());
            }
            for keyword in ["items", "additionalProperties", "not", "contains"] {
                if let Some(child) = node.get(keyword) {
                    pending.push(child);
                }
            }
            for keyword in ["allOf", "anyOf", "oneOf", "prefixItems"] {
                if let Some(children) = node.get(keyword).and_then(Value::as_array) {
                    pending.extend(children);
                }
            }
        }
        Ok(sensitive)
    }

    fn no_secret_literal(&self, node: &Value, value: &Value, path: &str) -> Result<()> {
        let mut walk = Walk::secrets(self.contract.limits);
        self.walk(node, value, "", 0, &mut walk)?;
        if walk.secret_paths.is_some_and(|paths| !paths.is_empty()) {
            return Err(error(ErrorCode::SecretLiteral, path));
        }
        Ok(())
    }

    fn definition_error(
        &self,
        code: ErrorCode,
        id: &str,
        keyword: &str,
    ) -> Result<super::SchemaError> {
        let location = self
            .contract
            .node_locations
            .get(id)
            .ok_or_else(|| error(ErrorCode::InvalidDefinition, pointer("/$defs", id)))?;
        let mut failure = error(code, pointer(&location.path, keyword));
        failure.source_ref = Some(location.source_ref.clone());
        Ok(failure)
    }

    fn effective<'b>(
        &'b self,
        mut node: &'b Value,
        path: &str,
        depth: usize,
        budget: &mut Budget,
    ) -> Result<Effective<'b>> {
        let mut annotations = BTreeMap::new();
        let mut hops = 0;
        loop {
            budget.visit(depth + hops, path)?;
            if node.is_boolean() {
                return Ok(Effective {
                    schema: node,
                    annotations,
                });
            }
            let map = node
                .as_object()
                .ok_or_else(|| error(ErrorCode::InvalidDefinition, path))?;
            for keyword in [
                "title",
                "description",
                "default",
                "examples",
                "position",
                "secret",
            ] {
                if let Some(value) = map.get(keyword) {
                    if keyword == "secret" && value == &Value::Bool(true) {
                        annotations.insert(keyword, value);
                    } else {
                        annotations.entry(keyword).or_insert(value);
                    }
                }
            }
            let Some(reference) = map.get("$ref").and_then(Value::as_str) else {
                return Ok(Effective {
                    schema: node,
                    annotations,
                });
            };
            if !reference.starts_with("#/$defs/") {
                return Err(error(ErrorCode::InvalidReference, path));
            }
            node = self
                .contract
                .annotated_schema
                .pointer(&reference[1..])
                .ok_or_else(|| error(ErrorCode::InvalidReference, path))?;
            hops += 1;
        }
    }

    fn defaults(
        &self,
        node: &Value,
        value: &mut Value,
        path: &str,
        depth: usize,
        budget: &mut Budget,
    ) -> Result<()> {
        let effective = self.effective(node, path, depth, budget)?;
        if let Some(object) = value.as_object_mut() {
            let properties = effective.get("properties").and_then(Value::as_object);
            if let Some(properties) = properties {
                for (name, schema) in properties {
                    let child_path = pointer(path, name);
                    if !object.contains_key(name) {
                        let child = self.effective(schema, &child_path, depth + 1, budget)?;
                        if let Some(default) = child.get("default") {
                            let bytes = check_json(
                                default,
                                self.contract.limits,
                                self.contract.limits.max_value_bytes,
                            )?;
                            budget.add_bytes(bytes + name.len() + 4, &child_path)?;
                            object.insert(name.clone(), default.clone());
                        }
                    }
                    if let Some(child) = object.get_mut(name) {
                        self.defaults(schema, child, &child_path, depth + 1, budget)?;
                    }
                }
            }
            if let Some(additional) = effective.get("additionalProperties") {
                for (name, child) in object.iter_mut() {
                    if !properties.is_some_and(|properties| properties.contains_key(name)) {
                        self.defaults(additional, child, &pointer(path, name), depth + 1, budget)?;
                    }
                }
            }
        }
        if let Some(array) = value.as_array_mut() {
            for (index, child) in array.iter_mut().enumerate() {
                if let Some(schema) = item_schema(&effective, index) {
                    self.defaults(
                        schema,
                        child,
                        &pointer(path, &index.to_string()),
                        depth + 1,
                        budget,
                    )?;
                }
            }
        }
        Ok(())
    }

    fn walk(
        &self,
        node: &Value,
        value: &Value,
        path: &str,
        depth: usize,
        walk: &mut Walk,
    ) -> Result<()> {
        let effective = self.effective(node, path, depth, &mut walk.budget)?;
        if walk.rule_templates
            && value.is_string()
            && self.node_declares_secrets(node, path, depth, &mut walk.budget)?
        {
            if let Some(paths) = &mut walk.secret_paths {
                paths.insert(path.to_owned());
            }
            return Ok(());
        }
        if effective.get("secret") == Some(&Value::Bool(true)) {
            if let Some(paths) = &mut walk.secret_paths {
                paths.insert(path.to_owned());
                return Ok(());
            }
            // Unconstrained properties/items and composition-only schemas must
            // not bypass the secret value domain. Redaction still accepts any
            // shape so malformed sensitive outputs can be protected.
            Self::validate_secret_tree(value, path, depth, &mut walk.budget)?;
        }
        if let Some(object) = value.as_object() {
            let properties = effective.get("properties").and_then(Value::as_object);
            for (name, child) in object {
                let schema = properties
                    .and_then(|properties| properties.get(name))
                    .or_else(|| effective.get("additionalProperties"));
                if let Some(schema) = schema {
                    self.walk(schema, child, &pointer(path, name), depth + 1, walk)?;
                }
            }
        }
        if let Some(array) = value.as_array() {
            for (index, child) in array.iter().enumerate() {
                let child_path = pointer(path, &index.to_string());
                if let Some(schema) = item_schema(&effective, index) {
                    self.walk(schema, child, &child_path, depth + 1, walk)?;
                }
                if let Some(schema) = effective.get("contains") {
                    self.walk(schema, child, &child_path, depth + 1, walk)?;
                }
            }
        }
        for keyword in ["allOf", "anyOf", "oneOf"] {
            if let Some(branches) = effective.get(keyword).and_then(Value::as_array) {
                for branch in branches {
                    self.walk(branch, value, path, depth + 1, walk)?;
                }
            }
        }
        if let Some(schema) = effective.get("not") {
            self.walk(schema, value, path, depth + 1, walk)?;
        }
        Ok(())
    }

    fn validate_secret_tree(
        value: &Value,
        path: &str,
        depth: usize,
        budget: &mut Budget,
    ) -> Result<()> {
        budget.visit(depth, path)?;
        match value {
            Value::String(_) => Ok(()),
            Value::Array(values) => {
                for value in values {
                    Self::validate_secret_tree(value, path, depth + 1, budget)?;
                }
                Ok(())
            }
            Value::Object(values) => {
                for value in values.values() {
                    Self::validate_secret_tree(value, path, depth + 1, budget)?;
                }
                Ok(())
            }
            _ => Err(error(ErrorCode::InvalidValue, path)),
        }
    }

    fn branch_has_default(
        &self,
        node: &Value,
        seen: &mut BTreeSet<String>,
        budget: &mut Budget,
        depth: usize,
    ) -> Result<bool> {
        budget.visit(depth, "")?;
        if let Some(reference) = node.get("$ref").and_then(Value::as_str) {
            if !seen.insert(reference.to_owned()) {
                return Ok(false);
            }
        }
        let effective = self.effective(node, "", depth, budget)?;
        if effective.get("default").is_some() {
            return Ok(true);
        }
        if let Some(properties) = effective.get("properties").and_then(Value::as_object) {
            for child in properties.values() {
                if self.branch_has_default(child, seen, budget, depth + 1)? {
                    return Ok(true);
                }
            }
        }
        for keyword in ["items", "additionalProperties", "not", "contains"] {
            if let Some(child) = effective.get(keyword) {
                if self.branch_has_default(child, seen, budget, depth + 1)? {
                    return Ok(true);
                }
            }
        }
        for keyword in ["allOf", "anyOf", "oneOf", "prefixItems"] {
            if let Some(children) = effective.get(keyword).and_then(Value::as_array) {
                for child in children {
                    if self.branch_has_default(child, seen, budget, depth + 1)? {
                        return Ok(true);
                    }
                }
            }
        }
        Ok(false)
    }
}

fn item_schema<'a>(schema: &Effective<'a>, index: usize) -> Option<&'a Value> {
    schema
        .get("prefixItems")
        .and_then(Value::as_array)
        .and_then(|items| items.get(index))
        .or_else(|| schema.get("items"))
}

fn validator(schema: &Value) -> Result<Validator> {
    jsonschema::options()
        .with_draft(Draft::Draft202012)
        .should_validate_formats(false)
        .with_retriever(DenyRetrieval)
        .build(schema)
        .map_err(|failure| {
            error(
                ErrorCode::InvalidDefinition,
                failure.instance_path().to_string(),
            )
        })
}
