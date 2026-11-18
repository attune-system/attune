//! Compile action input annotations into a bounded, secret-aware argv plan.
//!
//! This module is opt-in. Production admission and workers must implement the
//! complete launch contract before advertising `action_cli_arguments_v1`.
//! Ordinary value-schema compilation continues rejecting CLI annotations.

mod materialize;
mod types;

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

use crate::value_schema::{self, CompiledContract, Contract, Limits, RegistryView, SourceIdentity};
pub use materialize::PreparedArguments;
use types::failure;
pub use types::*;

const MAX_GROUPS: usize = 1024;

struct ParameterArgument {
    segments: Vec<String>,
    schema_path: String,
    display_path: String,
    binding: CliBinding,
}

enum Group {
    Literal { value: String, path: String },
    Parameter(ParameterArgument),
}

/// A derived, immutable plan. Construction validates the complete authored
/// schema and retains its digest, including CLI metadata, in the contract.
/// It is deliberately not deserializable as a substitute for admitted metadata.
pub struct ArgumentPlan {
    contract: Contract,
    groups: BTreeMap<i64, Group>,
}

impl ArgumentPlan {
    /// Compile against the same fixed, authorized type view used at admission.
    /// Authoring sources remain previews and do not establish runtime authority.
    pub fn compile(
        source: SourceIdentity,
        parameters: &Value,
        literals: &[LiteralArgument],
        registry: &RegistryView,
        limits: Limits,
    ) -> Result<Self> {
        // Bound recursion and byte size before walking any authored occurrences.
        value_schema::check_json(parameters, limits, limits.max_source_bytes)?;
        let mut groups = BTreeMap::new();
        for (index, literal) in literals.iter().enumerate() {
            let path = format!("launch.arguments[{index}]");
            check_token(&literal.literal, &path, true)?;
            insert_group(
                &mut groups,
                literal.order,
                Group::Literal {
                    value: literal.literal.clone(),
                    path: path.clone(),
                },
                &path,
            )?;
        }
        let fields = parameters
            .as_object()
            .ok_or_else(|| failure(ErrorCode::InvalidBinding, "parameters"))?;
        let mut cli_paths = BTreeSet::new();
        for (name, schema) in fields {
            collect(
                schema,
                vec![name.clone()],
                value_schema::pointer("", name),
                &mut groups,
                &mut cli_paths,
            )?;
        }
        let contract =
            value_schema::compile_action_input(source, parameters, registry, limits, cli_paths)?;
        let compiled = CompiledContract::new(&contract)?;
        for group in groups.values() {
            if let Group::Parameter(parameter) = group {
                let secret = compiled
                    .check_argument_binding(
                        &parameter.schema_path,
                        &parameter.segments,
                        &parameter.binding,
                    )
                    .map_err(|error| {
                        if error.code == value_schema::ErrorCode::InvalidDefinition {
                            failure(ErrorCode::InvalidBinding, &parameter.display_path)
                        } else {
                            Error::Schema(error)
                        }
                    })?;
                if secret {
                    return Err(failure(ErrorCode::SecretValue, &parameter.display_path));
                }
            }
        }
        Ok(Self { contract, groups })
    }

    pub fn contract(&self) -> &Contract {
        &self.contract
    }
}

fn collect(
    schema: &Value,
    segments: Vec<String>,
    schema_path: String,
    groups: &mut BTreeMap<i64, Group>,
    cli_paths: &mut BTreeSet<String>,
) -> Result<()> {
    if let Some(annotation) = schema.get("cli") {
        let display_path = display_path(&segments);
        let binding: CliBinding = serde_json::from_value(annotation.clone())
            .map_err(|_| failure(ErrorCode::InvalidBinding, &display_path))?;
        match &binding {
            CliBinding::Option { name, .. } => check_name(name, &display_path)?,
            CliBinding::Flag {
                name, false_name, ..
            } => {
                check_name(name, &display_path)?;
                if let Some(name) = false_name {
                    check_name(name, &display_path)?;
                }
            }
            CliBinding::Positional { .. } => {}
        }
        cli_paths.insert(schema_path.clone());
        insert_group(
            groups,
            binding.order(),
            Group::Parameter(ParameterArgument {
                segments: segments.clone(),
                schema_path: schema_path.clone(),
                display_path: display_path.clone(),
                binding,
            }),
            &display_path,
        )?;
    }
    if let Some(properties) = schema.get("properties").and_then(Value::as_object) {
        for (name, child) in properties {
            let mut child_segments = segments.clone();
            child_segments.push(name.clone());
            collect(
                child,
                child_segments,
                value_schema::pointer(&value_schema::pointer(&schema_path, "properties"), name),
                groups,
                cli_paths,
            )?;
        }
    }
    Ok(())
}

fn insert_group(
    groups: &mut BTreeMap<i64, Group>,
    order: i64,
    group: Group,
    path: &str,
) -> Result<()> {
    if !(0..=i64::from(i32::MAX)).contains(&order) {
        return Err(failure(ErrorCode::InvalidBinding, path));
    }
    if groups.contains_key(&order) {
        return Err(failure(ErrorCode::DuplicateOrder, path));
    }
    if groups.len() >= MAX_GROUPS {
        return Err(failure(ErrorCode::LimitExceeded, path));
    }
    groups.insert(order, group);
    Ok(())
}

fn check_token(value: &str, path: &str, allow_empty: bool) -> Result<()> {
    if value.contains('\0') || (!allow_empty && value.is_empty()) {
        return Err(failure(ErrorCode::InvalidBinding, path));
    }
    if value.len() > ArgumentLimits::default().max_argument_bytes {
        return Err(failure(ErrorCode::LimitExceeded, path));
    }
    Ok(())
}

fn check_name(value: &str, path: &str) -> Result<()> {
    check_token(value, path, false)?;
    if matches!(value, "-" | "--") {
        return Err(failure(ErrorCode::InvalidBinding, path));
    }
    Ok(())
}

fn display_path(segments: &[String]) -> String {
    let mut result = String::new();
    for segment in segments {
        let identifier = !segment.is_empty()
            && segment.bytes().enumerate().all(|(index, c)| {
                c == b'_' || c.is_ascii_alphabetic() || (index > 0 && c.is_ascii_digit())
            });
        if identifier {
            if !result.is_empty() {
                result.push('.');
            }
            result.push_str(segment);
        } else {
            result.push('[');
            // Serializing a UTF-8 string cannot fail.
            result.push_str(&serde_json::to_string(segment).expect("string serialization"));
            result.push(']');
        }
    }
    result
}
