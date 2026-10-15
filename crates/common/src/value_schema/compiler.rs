use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Map, Value};

use super::{
    check_json, digest, error, pointer, CompiledContract, Contract, Dependency, ErrorCode, Limits,
    RegistryEntry, RegistryView, Result, SchemaContext, SchemaLocation, SourceIdentity,
    COMPILER_CONTRACT, DIALECT,
};

const MAP_SCHEMAS: &[&str] = &["properties", "$defs"];
const ARRAY_SCHEMAS: &[&str] = &["allOf", "anyOf", "oneOf", "prefixItems"];
const SINGLE_SCHEMAS: &[&str] = &["items", "additionalProperties", "not", "contains"];
const ANNOTATIONS: &[&str] = &[
    "title",
    "description",
    "default",
    "examples",
    "position",
    "secret",
];
const ASSERTIONS: &[&str] = &[
    "type",
    "enum",
    "const",
    "required",
    "minimum",
    "maximum",
    "exclusiveMinimum",
    "exclusiveMaximum",
    "multipleOf",
    "minLength",
    "maxLength",
    "pattern",
    "minItems",
    "maxItems",
    "uniqueItems",
    "minContains",
    "maxContains",
    "minProperties",
    "maxProperties",
];

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct Location {
    document: String,
    path: String,
}

struct Document {
    schema: Value,
    positions: BTreeSet<String>,
}

struct Compiler<'a> {
    registry: &'a RegistryView,
    limits: Limits,
    documents: BTreeMap<String, Document>,
    definitions: BTreeMap<String, Value>,
    heights: BTreeMap<Location, usize>,
    stack: Vec<Location>,
    dependencies: BTreeMap<String, Dependency>,
    source_bytes: usize,
    schema_nodes: usize,
    source_ref: String,
    allow_candidates: bool,
}

/// Check a current inline definition without claiming an executable-release pin.
/// Named refs fail closed until a caller supplies a permitted revision view to
/// `compile`. This result is a preview, not a runtime record's authoritative bind.
pub fn compile_inline(
    source_ref: impl Into<String>,
    context: SchemaContext,
    authored: &Value,
) -> Result<Contract> {
    compile(
        SourceIdentity::Authoring {
            component_ref: source_ref.into(),
        },
        context,
        authored,
        &RegistryView::new(),
        Limits::default(),
    )
}

/// Discover named edges and validate local definition syntax without resolving
/// registry state. This is not value validation or a materialized contract.
/// Full compilation is still required to check graph cycles, defaults and secrets.
pub fn inspect_definition(
    context: SchemaContext,
    authored: &Value,
    limits: Limits,
) -> Result<Vec<super::ReferenceUse>> {
    check_json(authored, limits, limits.max_source_bytes)?;
    let registry = RegistryView::new();
    let mut compiler = Compiler {
        registry: &registry,
        limits,
        documents: BTreeMap::new(),
        definitions: BTreeMap::new(),
        heights: BTreeMap::new(),
        stack: Vec::new(),
        dependencies: BTreeMap::new(),
        source_bytes: 0,
        schema_nodes: 0,
        source_ref: "authored".into(),
        allow_candidates: true,
    };
    compiler.add_document("authored", authored.clone(), context)?;
    let document = &compiler.documents["authored"];
    let mut refs = Vec::new();
    for path in &document.positions {
        let node = document
            .schema
            .pointer(path)
            .ok_or_else(|| error(ErrorCode::InvalidDefinition, path))?;
        let Some(mut map) = node.as_object().cloned() else {
            continue;
        };
        map.remove("$schema");
        if let Some(type_ref) = map
            .get("type")
            .and_then(Value::as_str)
            .filter(|name| !primitive(name))
        {
            crate::schema::RefValidator::validate_component_ref(type_ref)
                .map_err(|_| error(ErrorCode::InvalidReference, pointer(path, "type")))?;
            check_overrides(&map, "type", path)?;
            refs.push(super::ReferenceUse {
                type_ref: type_ref.into(),
                path: path.clone(),
            });
            map.remove("type");
        }
        if let Some(reference) = map.get("$ref") {
            let target = reference
                .as_str()
                .and_then(|reference| reference.strip_prefix('#'))
                .filter(|target| {
                    (target.is_empty() || target.starts_with('/'))
                        && !target.contains('%')
                        && valid_pointer(target)
                })
                .ok_or_else(|| error(ErrorCode::InvalidReference, pointer(path, "$ref")))?;
            if !document.positions.contains(target) {
                return Err(error(ErrorCode::InvalidReference, pointer(path, "$ref")));
            }
            check_overrides(&map, "$ref", path)?;
        }
        // Check each node's keyword shapes against the standard meta-schema.
        // Child bodies are checked independently above, not compiled as `true`.
        for keyword in MAP_SCHEMAS {
            if let Some(children) = map.get_mut(*keyword).and_then(Value::as_object_mut) {
                for child in children.values_mut() {
                    *child = Value::Bool(true);
                }
            }
        }
        for keyword in ARRAY_SCHEMAS {
            if let Some(children) = map.get_mut(*keyword).and_then(Value::as_array_mut) {
                for child in children {
                    *child = Value::Bool(true);
                }
            }
        }
        for keyword in SINGLE_SCHEMAS {
            if let Some(child) = map.get_mut(*keyword) {
                *child = Value::Bool(true);
            }
        }
        map.insert("$schema".into(), json!(DIALECT));
        jsonschema::meta::validate(&Value::Object(map)).map_err(|failure| {
            error(
                ErrorCode::InvalidDefinition,
                format!("{path}{}", failure.instance_path()),
            )
        })?;
    }
    Ok(refs)
}

/// Audit every local schema edge, including unused definitions, without
/// inventing bodies for external refs. External resolution remains unverified.
pub fn check_local_graph(definitions: &BTreeMap<String, Value>, limits: Limits) -> Result<()> {
    let registry = RegistryView::new();
    let mut compiler = Compiler {
        registry: &registry,
        limits,
        documents: BTreeMap::new(),
        definitions: BTreeMap::new(),
        heights: BTreeMap::new(),
        stack: Vec::new(),
        dependencies: BTreeMap::new(),
        source_bytes: 0,
        schema_nodes: 0,
        source_ref: "authored".into(),
        allow_candidates: true,
    };
    for (type_ref, schema) in definitions {
        inspect_definition(SchemaContext::SingleValue, schema, limits)?;
        compiler.add_document(type_ref, schema.clone(), SchemaContext::SingleValue)?;
    }
    let mut edges = BTreeMap::<Location, Vec<Location>>::new();
    for (name, document) in &compiler.documents {
        for path in &document.positions {
            let location = Location {
                document: name.clone(),
                path: path.clone(),
            };
            let schema = document
                .schema
                .pointer(path)
                .ok_or_else(|| error(ErrorCode::InvalidReference, path))?;
            let mut children = Vec::new();
            if let Some(type_ref) = schema
                .get("type")
                .and_then(Value::as_str)
                .filter(|value| !primitive(value))
            {
                if compiler.documents.contains_key(type_ref) {
                    children.push(Location {
                        document: type_ref.into(),
                        path: String::new(),
                    });
                }
            } else if let Some(reference) = schema.get("$ref").and_then(Value::as_str) {
                children.push(Location {
                    document: name.clone(),
                    path: reference[1..].into(),
                });
            } else {
                for keyword in MAP_SCHEMAS.iter().filter(|keyword| **keyword != "$defs") {
                    if let Some(map) = schema.get(*keyword).and_then(Value::as_object) {
                        for key in map.keys() {
                            children.push(Location {
                                document: name.clone(),
                                path: pointer(&pointer(path, keyword), key),
                            });
                        }
                    }
                }
                for keyword in ARRAY_SCHEMAS {
                    if let Some(array) = schema.get(*keyword).and_then(Value::as_array) {
                        for index in 0..array.len() {
                            children.push(Location {
                                document: name.clone(),
                                path: pointer(&pointer(path, keyword), &index.to_string()),
                            });
                        }
                    }
                }
                for keyword in SINGLE_SCHEMAS {
                    if schema.get(*keyword).is_some() {
                        children.push(Location {
                            document: name.clone(),
                            path: pointer(path, keyword),
                        });
                    }
                }
            }
            edges.insert(location, children);
        }
    }
    fn visit(
        location: &Location,
        edges: &BTreeMap<Location, Vec<Location>>,
        done: &mut BTreeMap<Location, usize>,
        stack: &mut Vec<Location>,
        limit: usize,
    ) -> Result<usize> {
        if let Some(start) = stack.iter().position(|entry| entry == location) {
            let mut failure = error(
                ErrorCode::Cycle,
                stack.last().map(|entry| entry.path.as_str()).unwrap_or(""),
            );
            failure.reference_chain = stack[start..]
                .iter()
                .chain(std::iter::once(location))
                .map(|entry| format!("{}#{}", entry.document, entry.path))
                .collect();
            return Err(failure);
        }
        if stack.len() > limit {
            return Err(error(ErrorCode::ResourceLimit, &location.path));
        }
        if let Some(height) = done.get(location) {
            if stack.len() + height > limit {
                return Err(error(ErrorCode::ResourceLimit, &location.path));
            }
            return Ok(*height);
        }
        stack.push(location.clone());
        let mut height = 0;
        for child in edges.get(location).into_iter().flatten() {
            height = height.max(1 + visit(child, edges, done, stack, limit)?);
        }
        stack.pop();
        done.insert(location.clone(), height);
        Ok(height)
    }
    let mut done = BTreeMap::new();
    for location in edges.keys() {
        visit(
            location,
            &edges,
            &mut done,
            &mut Vec::new(),
            limits.max_depth,
        )?;
    }
    Ok(())
}

/// Compile using a fixed revision view. No I/O, ambient registry, or URL lookup.
pub fn compile(
    source: SourceIdentity,
    context: SchemaContext,
    authored: &Value,
    registry: &RegistryView,
    limits: Limits,
) -> Result<Contract> {
    let source_ref = source.component_ref().to_owned();
    compile_inner(source, context, authored, registry, limits).map_err(|mut failure| {
        if failure.source_ref.is_none() {
            failure.source_ref = Some(source_ref);
        }
        failure
    })
}

fn compile_inner(
    source: SourceIdentity,
    context: SchemaContext,
    authored: &Value,
    registry: &RegistryView,
    limits: Limits,
) -> Result<Contract> {
    check_json(authored, limits, limits.max_source_bytes)?;
    let mut compiler = Compiler {
        registry,
        limits,
        documents: BTreeMap::new(),
        definitions: BTreeMap::new(),
        heights: BTreeMap::new(),
        stack: Vec::new(),
        dependencies: BTreeMap::new(),
        source_bytes: 0,
        schema_nodes: 0,
        source_ref: source.component_ref().to_owned(),
        allow_candidates: matches!(source, SourceIdentity::Authoring { .. }),
    };
    // A distinct document key prevents a type from sharing the author's local scope.
    let document = "authored".to_string();
    compiler.add_document(&document, authored.clone(), context)?;
    let root = match context {
        SchemaContext::SingleValue => compiler.compile_node(
            Location {
                document: document.clone(),
                path: String::new(),
            },
            0,
        )?,
        SchemaContext::FlatDocument => {
            let fields = authored
                .as_object()
                .ok_or_else(|| error(ErrorCode::InvalidDefinition, ""))?;
            let mut properties = Map::new();
            let mut required = Vec::new();
            for (name, field) in fields {
                if field.get("required").and_then(Value::as_bool) == Some(true) {
                    required.push(Value::String(name.clone()));
                }
                let child = compiler.compile_node(
                    Location {
                        document: document.clone(),
                        path: pointer("", name),
                    },
                    1,
                )?;
                properties.insert(name.clone(), child);
            }
            json!({"type": "object", "properties": properties, "required": required})
        }
    };
    // Check unused definitions too, without treating lexical `$defs` containment
    // as a reference edge. A definition pointing at a non-recursive root is safe.
    loop {
        let pending = compiler
            .documents
            .iter()
            .flat_map(|(document, entry)| {
                entry.positions.iter().map(move |path| Location {
                    document: document.clone(),
                    path: path.clone(),
                })
            })
            .find(|location| !compiler.heights.contains_key(location));
        let Some(location) = pending else {
            break;
        };
        compiler.compile_node(location, 0)?;
    }
    let mut annotated = root.as_object().cloned().unwrap_or_default();
    annotated.insert("$schema".into(), json!(DIALECT));
    annotated.insert(
        "$defs".into(),
        serde_json::to_value(&compiler.definitions)
            .map_err(|_| error(ErrorCode::InvalidDefinition, ""))?,
    );
    let annotated_schema = Value::Object(annotated);
    let validator_schema = strip_annotations(&annotated_schema);
    let compiled_bytes = serde_json::to_vec(&annotated_schema)
        .map_err(|_| error(ErrorCode::InvalidDefinition, ""))?
        .len()
        + serde_json::to_vec(&validator_schema)
            .map_err(|_| error(ErrorCode::InvalidDefinition, ""))?
            .len();
    if compiled_bytes > limits.max_compiled_bytes {
        return Err(error(ErrorCode::ResourceLimit, ""));
    }
    let mut contract = Contract {
        compiler_contract: COMPILER_CONTRACT.into(),
        source: source.clone(),
        context,
        authored_digest: digest(authored)?,
        annotated_schema,
        validator_schema,
        dependencies: compiler.dependencies.into_values().collect(),
        node_locations: compiler
            .heights
            .keys()
            .map(|location| {
                Ok((
                    node_id(location)?,
                    SchemaLocation {
                        source_ref: if location.document == "authored" {
                            source.component_ref().to_owned()
                        } else {
                            location.document.clone()
                        },
                        path: location.path.clone(),
                    },
                ))
            })
            .collect::<Result<_>>()?,
        limits,
        binding_digest: String::new(),
    };
    // Include manifests and location metadata in the persisted bundle budget.
    // Reserve the fixed-size hex digest that will be filled after validation.
    if serde_json::to_vec(&contract)
        .map_err(|_| error(ErrorCode::InvalidDefinition, ""))?
        .len()
        .saturating_add(64)
        > limits.max_compiled_bytes
    {
        return Err(error(ErrorCode::ResourceLimit, ""));
    }
    // Compilation also verifies defaults and secret literals, including unused defs.
    CompiledContract::unsealed(&contract)?.check_literals()?;
    // The empty digest field is part of the canonical digest preimage.
    contract.binding_digest = digest(&contract)?;
    Ok(contract)
}

impl Compiler<'_> {
    fn add_document(&mut self, key: &str, mut schema: Value, context: SchemaContext) -> Result<()> {
        self.source_bytes += check_json(&schema, self.limits, self.limits.max_source_bytes)?;
        if self.source_bytes > self.limits.max_source_bytes {
            return Err(error(ErrorCode::ResourceLimit, ""));
        }
        let mut positions = BTreeSet::new();
        match context {
            SchemaContext::SingleValue => self.index(&schema, "", &mut positions, 0)?,
            SchemaContext::FlatDocument => {
                let fields = schema
                    .as_object_mut()
                    .ok_or_else(|| error(ErrorCode::InvalidDefinition, ""))?;
                for (name, field) in fields {
                    let path = pointer("", name);
                    let field = field
                        .as_object_mut()
                        .ok_or_else(|| error(ErrorCode::InvalidDefinition, &path))?;
                    // Only flat occurrences accept boolean requiredness. Object
                    // required arrays remain structural JSON Schema constraints.
                    if field.get("required").is_some_and(Value::is_boolean) {
                        field.remove("required");
                    }
                    self.index(&Value::Object(field.clone()), &path, &mut positions, 1)?;
                }
            }
        }
        self.documents
            .insert(key.to_owned(), Document { schema, positions });
        Ok(())
    }

    fn index(
        &mut self,
        schema: &Value,
        path: &str,
        positions: &mut BTreeSet<String>,
        depth: usize,
    ) -> Result<()> {
        self.schema_nodes += 1;
        if depth > self.limits.max_depth || self.schema_nodes > self.limits.max_schema_nodes {
            return Err(error(ErrorCode::ResourceLimit, path));
        }
        positions.insert(path.to_owned());
        if schema.is_boolean() {
            return Ok(());
        }
        let map = schema
            .as_object()
            .ok_or_else(|| error(ErrorCode::InvalidDefinition, path))?;
        for (keyword, value) in map {
            let child_path = pointer(path, keyword);
            if MAP_SCHEMAS.contains(&keyword.as_str()) {
                let children = value
                    .as_object()
                    .ok_or_else(|| error(ErrorCode::InvalidDefinition, &child_path))?;
                for (name, child) in children {
                    self.index(child, &pointer(&child_path, name), positions, depth + 1)?;
                }
            } else if ARRAY_SCHEMAS.contains(&keyword.as_str()) {
                let children = value
                    .as_array()
                    .ok_or_else(|| error(ErrorCode::InvalidDefinition, &child_path))?;
                for (index, child) in children.iter().enumerate() {
                    self.index(
                        child,
                        &pointer(&child_path, &index.to_string()),
                        positions,
                        depth + 1,
                    )?;
                }
            } else if SINGLE_SCHEMAS.contains(&keyword.as_str()) {
                self.index(value, &child_path, positions, depth + 1)?;
            } else if !ANNOTATIONS.contains(&keyword.as_str())
                && !ASSERTIONS.contains(&keyword.as_str())
                && !matches!(keyword.as_str(), "$ref" | "$schema" | "format")
            {
                return Err(error(ErrorCode::UnsupportedKeyword, child_path));
            }
        }
        if map
            .get("$schema")
            .is_some_and(|value| value.as_str() != Some(DIALECT))
        {
            return Err(error(
                ErrorCode::UnsupportedKeyword,
                pointer(path, "$schema"),
            ));
        }
        for keyword in ["secret"] {
            if map.get(keyword).is_some_and(|value| !value.is_boolean()) {
                return Err(error(ErrorCode::InvalidDefinition, pointer(path, keyword)));
            }
        }
        if map
            .get("position")
            .is_some_and(|value| value.as_u64().is_none())
        {
            return Err(error(
                ErrorCode::InvalidDefinition,
                pointer(path, "position"),
            ));
        }
        for keyword in ["title", "description", "format"] {
            if map.get(keyword).is_some_and(|value| !value.is_string()) {
                return Err(error(ErrorCode::InvalidDefinition, pointer(path, keyword)));
            }
        }
        if map.get("examples").is_some_and(|value| !value.is_array()) {
            return Err(error(
                ErrorCode::InvalidDefinition,
                pointer(path, "examples"),
            ));
        }
        Ok(())
    }

    fn compile_node(&mut self, location: Location, depth: usize) -> Result<Value> {
        self.compile_node_inner(location.clone(), depth)
            .map_err(|mut failure| {
                if failure.source_ref.is_none() {
                    failure.source_ref = Some(if location.document == "authored" {
                        self.source_ref.clone()
                    } else {
                        location.document
                    });
                }
                failure
            })
    }

    fn compile_node_inner(&mut self, location: Location, depth: usize) -> Result<Value> {
        if depth > self.limits.max_depth {
            return Err(error(ErrorCode::ResourceLimit, &location.path));
        }
        if let Some(start) = self.stack.iter().position(|entry| entry == &location) {
            let mut failure = error(
                ErrorCode::Cycle,
                self.stack
                    .last()
                    .map(|entry| entry.path.as_str())
                    .unwrap_or(""),
            );
            failure.reference_chain = self.stack[start..]
                .iter()
                .chain(std::iter::once(&location))
                .map(|entry| {
                    if entry.path.is_empty() {
                        entry.document.clone()
                    } else {
                        format!("{}#{}", entry.document, entry.path)
                    }
                })
                .collect();
            return Err(failure);
        }
        if let Some(height) = self.heights.get(&location) {
            if depth + height > self.limits.max_depth {
                return Err(error(ErrorCode::ResourceLimit, &location.path));
            }
            return reference(&location);
        }
        let document = self
            .documents
            .get(&location.document)
            .ok_or_else(|| error(ErrorCode::MissingReference, &location.path))?;
        if !document.positions.contains(&location.path) {
            return Err(error(ErrorCode::InvalidReference, &location.path));
        }
        let schema = document
            .schema
            .pointer(&location.path)
            .cloned()
            .ok_or_else(|| error(ErrorCode::InvalidReference, &location.path))?;
        self.stack.push(location.clone());
        let mut height = 0;
        let resolved = if schema.is_boolean() {
            schema
        } else {
            let mut map = schema
                .as_object()
                .cloned()
                .ok_or_else(|| error(ErrorCode::InvalidDefinition, &location.path))?;
            map.remove("$schema");
            let named = map
                .get("type")
                .and_then(Value::as_str)
                .filter(|value| !primitive(value))
                .map(str::to_owned);
            if named.is_some() && map.contains_key("$ref") {
                return Err(error(ErrorCode::StructuralOverride, &location.path));
            }
            if let Some(type_ref) = named {
                crate::schema::RefValidator::validate_component_ref(&type_ref).map_err(|_| {
                    error(ErrorCode::InvalidReference, pointer(&location.path, "type"))
                })?;
                check_overrides(&map, "type", &location.path)?;
                self.load_type(&type_ref, &location.path)?;
                let target = Location {
                    document: type_ref,
                    path: String::new(),
                };
                map.remove("type");
                self.bind_reference(&mut map, target, depth, &mut height, &location.path)?;
            } else if let Some(local_ref) = map.get("$ref") {
                let local_ref = local_ref.as_str().ok_or_else(|| {
                    error(ErrorCode::InvalidReference, pointer(&location.path, "$ref"))
                })?;
                let target_path = local_ref
                    .strip_prefix('#')
                    .filter(|value| value.is_empty() || value.starts_with('/'))
                    .ok_or_else(|| {
                        error(ErrorCode::InvalidReference, pointer(&location.path, "$ref"))
                    })?;
                // Only plain JSON Pointer fragments. URI escaping/anchors are not
                // supported and must not accidentally acquire retrieval semantics.
                if target_path.contains('%') || !valid_pointer(target_path) {
                    return Err(error(
                        ErrorCode::InvalidReference,
                        pointer(&location.path, "$ref"),
                    ));
                }
                let target = Location {
                    document: location.document.clone(),
                    path: target_path.to_owned(),
                };
                check_overrides(&map, "$ref", &location.path)?;
                map.remove("$defs");
                self.bind_reference(&mut map, target, depth, &mut height, &location.path)?;
            } else {
                for keyword in MAP_SCHEMAS {
                    if *keyword == "$defs" {
                        continue;
                    }
                    if let Some(children) = map.get_mut(*keyword).and_then(Value::as_object_mut) {
                        for (name, child) in children {
                            let target = Location {
                                document: location.document.clone(),
                                path: pointer(&pointer(&location.path, keyword), name),
                            };
                            *child = self.child(target, depth, &mut height)?;
                        }
                    }
                }
                for keyword in ARRAY_SCHEMAS {
                    if let Some(children) = map.get_mut(*keyword).and_then(Value::as_array_mut) {
                        for (index, child) in children.iter_mut().enumerate() {
                            let target = Location {
                                document: location.document.clone(),
                                path: pointer(
                                    &pointer(&location.path, keyword),
                                    &index.to_string(),
                                ),
                            };
                            *child = self.child(target, depth, &mut height)?;
                        }
                    }
                }
                for keyword in SINGLE_SCHEMAS {
                    if let Some(child) = map.get_mut(*keyword) {
                        let target = Location {
                            document: location.document.clone(),
                            path: pointer(&location.path, keyword),
                        };
                        *child = self.child(target, depth, &mut height)?;
                    }
                }
                // Definitions are compiled and checked, but are not assertions.
                map.remove("$defs");
            }
            Value::Object(map)
        };
        self.definitions.insert(node_id(&location)?, resolved);
        self.heights.insert(location.clone(), height);
        self.stack.pop();
        reference(&location)
    }

    fn child(&mut self, target: Location, depth: usize, height: &mut usize) -> Result<Value> {
        let reference = self.compile_node(target.clone(), depth + 1)?;
        *height = (*height).max(1 + self.heights[&target]);
        Ok(reference)
    }

    fn bind_reference(
        &mut self,
        map: &mut Map<String, Value>,
        target: Location,
        depth: usize,
        height: &mut usize,
        path: &str,
    ) -> Result<()> {
        let child = self.child(target.clone(), depth, height)?;
        // Secret marking at the target's root is inherited through aliases.
        if map.get("secret") == Some(&Value::Bool(false)) && self.root_secret(&target)? {
            return Err(error(ErrorCode::SecretDowngrade, pointer(path, "secret")));
        }
        map.insert("$ref".into(), child["$ref"].clone());
        Ok(())
    }

    fn root_secret(&self, target: &Location) -> Result<bool> {
        let mut node = &self.definitions[&node_id(target)?];
        loop {
            if node.get("secret") == Some(&Value::Bool(true)) {
                return Ok(true);
            }
            let Some(reference) = node.get("$ref").and_then(Value::as_str) else {
                return Ok(false);
            };
            let key = reference
                .strip_prefix("#/$defs/")
                .ok_or_else(|| error(ErrorCode::InvalidReference, ""))?;
            node = self
                .definitions
                .get(key)
                .ok_or_else(|| error(ErrorCode::InvalidReference, ""))?;
        }
    }

    fn load_type(&mut self, type_ref: &str, path: &str) -> Result<()> {
        if self.documents.contains_key(type_ref) {
            return Ok(());
        }
        let (source, revision_id, schema) = match self.registry.get(type_ref) {
            Some(RegistryEntry::Available(revision)) => {
                if revision.source.component_ref() != type_ref
                    || !matches!(revision.source, SourceIdentity::Released { .. })
                {
                    return Err(error(ErrorCode::InvalidReference, path));
                }
                (
                    revision.source.clone(),
                    Some(revision.revision_id),
                    &revision.schema,
                )
            }
            Some(RegistryEntry::Candidate(schema)) if self.allow_candidates => (
                SourceIdentity::Authoring {
                    component_ref: type_ref.into(),
                },
                None,
                schema,
            ),
            Some(RegistryEntry::Candidate(_)) => {
                return Err(error(ErrorCode::InvalidReference, path))
            }
            Some(RegistryEntry::Inaccessible) => {
                return Err(error(ErrorCode::InaccessibleReference, path))
            }
            Some(RegistryEntry::Retired) => return Err(error(ErrorCode::RetiredReference, path)),
            None => return Err(error(ErrorCode::MissingReference, path)),
        };
        check_json(schema, self.limits, self.limits.max_source_bytes)?;
        let dependency = Dependency {
            type_ref: type_ref.to_owned(),
            source,
            revision_id,
            content_digest: digest(schema)?,
        };
        self.add_document(type_ref, schema.clone(), SchemaContext::SingleValue)
            .map_err(|mut failure| {
                failure.source_ref = Some(type_ref.into());
                failure
            })?;
        self.dependencies.insert(type_ref.to_owned(), dependency);
        Ok(())
    }
}

fn primitive(value: &str) -> bool {
    matches!(
        value,
        "string" | "number" | "integer" | "boolean" | "object" | "array" | "null"
    )
}

fn valid_pointer(value: &str) -> bool {
    let mut characters = value.chars();
    while let Some(character) = characters.next() {
        if character == '~' && !matches!(characters.next(), Some('0' | '1')) {
            return false;
        }
    }
    true
}

fn check_overrides(map: &Map<String, Value>, reference_keyword: &str, path: &str) -> Result<()> {
    for keyword in map.keys() {
        if keyword != reference_keyword
            && !ANNOTATIONS.contains(&keyword.as_str())
            && !(reference_keyword == "$ref" && keyword == "$defs")
        {
            return Err(error(ErrorCode::StructuralOverride, pointer(path, keyword)));
        }
    }
    Ok(())
}

fn node_id(location: &Location) -> Result<String> {
    digest(&(&location.document, &location.path))
}

fn reference(location: &Location) -> Result<Value> {
    Ok(json!({"$ref": format!("#/$defs/{}", node_id(location)?)}))
}

// Walk only schema-bearing positions; defaults/examples/const/enum remain data.
fn strip_annotations(schema: &Value) -> Value {
    let Some(map) = schema.as_object() else {
        return schema.clone();
    };
    let mut clean = map.clone();
    for keyword in ANNOTATIONS {
        clean.remove(*keyword);
    }
    clean.remove("format");
    for keyword in MAP_SCHEMAS {
        if let Some(children) = clean.get_mut(*keyword).and_then(Value::as_object_mut) {
            for child in children.values_mut() {
                *child = strip_annotations(child);
            }
        }
    }
    for keyword in ARRAY_SCHEMAS {
        if let Some(children) = clean.get_mut(*keyword).and_then(Value::as_array_mut) {
            for child in children {
                *child = strip_annotations(child);
            }
        }
    }
    for keyword in SINGLE_SCHEMAS {
        if let Some(child) = clean.get_mut(*keyword) {
            *child = strip_annotations(child);
        }
    }
    Value::Object(clean)
}
