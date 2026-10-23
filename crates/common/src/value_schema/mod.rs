//! Pure compilation of Attune flat documents and single-value schemas.
//!
//! Repository callers select and authorize immutable revisions before constructing
//! a registry view. Compilation never consults mutable state or retrieves URLs.
//! See `docs/plans/named-type-schema-compiler.md` for the compiler contract.

mod compiler;
mod runtime;

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

pub use compiler::{check_local_graph, compile, compile_inline, inspect_definition};
pub use runtime::{AuthoringReport, CompiledContract};

pub const COMPILER_CONTRACT: &str = "attune.value-schema/1";
pub const DIALECT: &str = "https://json-schema.org/draft/2020-12/schema";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SchemaContext {
    FlatDocument,
    SingleValue,
}

/// A selected immutable release identity or an explicitly unpinned authoring source.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SourceIdentity {
    Released {
        component_ref: String,
        release_id: i64,
        release_digest: String,
    },
    /// A current-schema check or preview, not an authoritative runtime binding.
    Authoring { component_ref: String },
    /// A server-created schema assignment on a mutable value or dynamic
    /// schema-bearing runtime record. Its complete authored/resolved body is
    /// retained with the assignment; referenced types still use release revisions.
    Assigned {
        component_ref: String,
        assignment_id: uuid::Uuid,
    },
}

impl SourceIdentity {
    pub fn component_ref(&self) -> &str {
        match self {
            Self::Released { component_ref, .. }
            | Self::Authoring { component_ref }
            | Self::Assigned { component_ref, .. } => component_ref,
        }
    }
}

#[derive(Debug, Clone)]
pub struct TypeRevision {
    pub source: SourceIdentity,
    pub revision_id: i64,
    pub schema: Value,
}

/// Unavailable entries let adapters fail closed without exposing definition bodies.
#[derive(Debug, Clone)]
pub enum RegistryEntry {
    Available(TypeRevision),
    /// Pack candidate bodies are permitted only in authoring/deployment checks.
    Candidate(Value),
    Inaccessible,
    Retired,
}

/// A fixed, already-authorized revision view. Missing entries never mean `true`.
pub type RegistryView = BTreeMap<String, RegistryEntry>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Limits {
    pub max_depth: usize,
    pub max_schema_nodes: usize,
    pub max_source_bytes: usize,
    pub max_compiled_bytes: usize,
    pub max_value_nodes: usize,
    pub max_value_bytes: usize,
    pub max_literal_checks: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_depth: 64,
            max_schema_nodes: 10_000,
            max_source_bytes: 1 << 20,
            max_compiled_bytes: 4 << 20,
            max_value_nodes: 100_000,
            max_value_bytes: 4 << 20,
            max_literal_checks: 256,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Dependency {
    pub type_ref: String,
    pub source: SourceIdentity,
    pub revision_id: Option<i64>,
    pub content_digest: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SchemaLocation {
    pub source_ref: String,
    pub path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReferenceUse {
    pub type_ref: String,
    pub path: String,
}

/// Self-contained, immutable compiler output. Keep the authored source separately.
///
/// The annotated bundle uses standard local `$ref`s and shared `$defs`. Consumers
/// must use graph-aware traversal, not expand it into an unbounded tree.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Contract {
    pub compiler_contract: String,
    pub source: SourceIdentity,
    pub context: SchemaContext,
    pub authored_digest: String,
    pub annotated_schema: Value,
    pub validator_schema: Value,
    pub dependencies: Vec<Dependency>,
    pub node_locations: BTreeMap<String, SchemaLocation>,
    pub limits: Limits,
    pub binding_digest: String,
}

impl Contract {
    /// Compare preview/acceptance semantics independently of the server-owned
    /// assignment identity, while covering resolved graphs and named revisions.
    pub fn semantic_digest(&self) -> Result<String> {
        digest(&(
            &self.compiler_contract,
            self.context,
            &self.authored_digest,
            &self.annotated_schema,
            &self.validator_schema,
            &self.dependencies,
            self.limits,
        ))
    }
}

/// An immutable value contract plus its lossless authored schema. Runtime
/// records use this for queue payloads, inquiry responses, and dynamic schemas.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ValueSchemaBinding {
    pub authored_schema: Value,
    pub contract: Contract,
}

impl ValueSchemaBinding {
    pub fn validate(&self, context: SchemaContext) -> Result<()> {
        if matches!(self.contract.source, SourceIdentity::Authoring { .. })
            || self.contract.context != context
            || digest(&self.authored_schema)? != self.contract.authored_digest
        {
            return Err(error(ErrorCode::InvalidDefinition, ""));
        }
        CompiledContract::new(&self.contract)?;
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ErrorCode {
    InvalidDefinition,
    UnsupportedKeyword,
    InvalidReference,
    MissingReference,
    InaccessibleReference,
    RetiredReference,
    StructuralOverride,
    SecretDowngrade,
    Cycle,
    ResourceLimit,
    InvalidDefault,
    SecretLiteral,
    InvalidValue,
}

impl ErrorCode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::InvalidDefinition => "schema.invalid_definition",
            Self::UnsupportedKeyword => "schema.unsupported_keyword",
            Self::InvalidReference => "schema.invalid_reference",
            Self::MissingReference => "schema.missing_reference",
            Self::InaccessibleReference => "schema.inaccessible_reference",
            Self::RetiredReference => "schema.retired_reference",
            Self::StructuralOverride => "schema.structural_override",
            Self::SecretDowngrade => "schema.secret_downgrade",
            Self::Cycle => "schema.cycle",
            Self::ResourceLimit => "schema.resource_limit",
            Self::InvalidDefault => "schema.invalid_default",
            Self::SecretLiteral => "schema.secret_literal",
            Self::InvalidValue => "schema.invalid_value",
        }
    }
}

/// Diagnostics contain locations and schema reference chains, never values or
/// validator display strings. Paths use escaped JSON Pointer segments.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, thiserror::Error)]
#[error("{code} at {path}")]
pub struct SchemaError {
    pub code: ErrorCode,
    pub path: String,
    pub source_ref: Option<String>,
    pub reference_chain: Vec<String>,
}

impl std::fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::InvalidDefinition => "Invalid schema definition; check the field type and supported schema structure",
            Self::UnsupportedKeyword => "Unsupported schema keyword; remove it or use an Attune flat schema field",
            Self::InvalidReference => "Invalid type reference; use a valid named type or local definition reference",
            Self::MissingReference => "Referenced type is unavailable; define it or declare and install its pack dependency",
            Self::InaccessibleReference => "Type reference is not allowed; check the dependency and reference policy",
            Self::RetiredReference => "Referenced type is retired; select an active type",
            Self::StructuralOverride => "Structural overrides of referenced types are not allowed; remove the override",
            Self::SecretDowngrade => "Secret protection cannot be removed from a referenced type",
            Self::Cycle => "Circular type references are not allowed; break the reference cycle",
            Self::ResourceLimit => "Schema exceeds the supported size, depth, or reference budget; simplify it",
            Self::InvalidDefault => "Default does not satisfy the schema; adjust or remove the default",
            Self::SecretLiteral => "Secret literals are not allowed; use a whole Key template",
            Self::InvalidValue => "Value does not satisfy the declared schema; adjust the value or schema",
        })
    }
}

pub type Result<T> = std::result::Result<T, SchemaError>;

pub(super) fn error(code: ErrorCode, path: impl Into<String>) -> SchemaError {
    SchemaError {
        code,
        path: path.into(),
        source_ref: None,
        reference_chain: Vec::new(),
    }
}

pub(super) fn pointer(base: &str, segment: &str) -> String {
    format!("{base}/{}", segment.replace('~', "~0").replace('/', "~1"))
}

pub(super) fn digest(value: &impl Serialize) -> Result<String> {
    let bytes = serde_jcs::to_vec(value).map_err(|_| error(ErrorCode::InvalidDefinition, ""))?;
    Ok(Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

/// Bound arbitrary JSON data as well as schema traversal. Annotations, defaults,
/// and examples are data, but they still have to fit the compiler's budget.
pub(super) fn check_json(value: &Value, limits: Limits, byte_limit: usize) -> Result<usize> {
    let mut pending = vec![(value, 0)];
    let mut nodes = 0;
    while let Some((value, depth)) = pending.pop() {
        nodes += 1;
        if depth > limits.max_depth || nodes > limits.max_value_nodes {
            return Err(error(ErrorCode::ResourceLimit, ""));
        }
        match value {
            Value::Object(map) => pending.extend(map.values().map(|value| (value, depth + 1))),
            Value::Array(array) => pending.extend(array.iter().map(|value| (value, depth + 1))),
            _ => {}
        }
    }
    let size = serde_json::to_vec(value)
        .map_err(|_| error(ErrorCode::InvalidDefinition, ""))?
        .len();
    if size > byte_limit {
        return Err(error(ErrorCode::ResourceLimit, ""));
    }
    Ok(size)
}

#[cfg(test)]
mod tests;
