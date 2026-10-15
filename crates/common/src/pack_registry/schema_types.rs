//! Type and consumer graph preflight before projection mutation. The candidate
//! view validates authoring; runtime records materialize released contracts.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::Path;

use sqlx::PgConnection;

use crate::models::AbsentMetadataPolicy;
use crate::repositories::pack::PackRepository;
use crate::repositories::schema_type::SchemaTypeRepository;
use crate::repositories::FindById;
use crate::schema_type::SchemaTypeDefinition;
use crate::value_schema::{
    compile, inspect_definition, Contract, Limits, RegistryEntry, RegistryView, SchemaContext,
    SourceIdentity, TypeRevision,
};
use crate::{Error, Result};

pub struct PackTypePlan {
    pub declared: BTreeMap<String, SchemaTypeDefinition>,
    pub contracts: BTreeMap<String, Contract>,
    pub registry: RegistryView,
}

pub fn read_definitions(
    pack_dir: &Path,
    pack_ref: &str,
) -> Result<BTreeMap<String, SchemaTypeDefinition>> {
    let dir = pack_dir.join("types");
    let mut definitions = BTreeMap::new();
    let mut source_files = BTreeMap::new();
    let mut bytes = 0;
    if !dir.exists() {
        return Ok(definitions);
    }
    if !std::fs::symlink_metadata(&dir)
        .map_err(|_| Error::validation("Cannot inspect named type directory"))?
        .is_dir()
    {
        return Err(Error::validation(
            "Named type directory must be a real directory",
        ));
    }
    for entry in std::fs::read_dir(&dir)
        .map_err(|_| Error::validation("Cannot read named type directory"))?
    {
        let entry =
            entry.map_err(|_| Error::validation("Cannot read named type directory entry"))?;
        let path = entry.path();
        let file = format!("types/{}", entry.file_name().to_string_lossy());
        if !matches!(
            path.extension().and_then(|value| value.to_str()),
            Some("yaml" | "yml")
        ) {
            continue;
        }
        let metadata = std::fs::symlink_metadata(&path).map_err(|_| {
            super::diagnostics::located(
                Error::io("Cannot inspect named type file; check storage permissions"),
                &file,
                "",
            )
        })?;
        if !metadata.is_file() {
            return Err(super::diagnostics::metadata_error(
                &file,
                "",
                "Named type definitions must be regular files",
            ));
        }
        bytes += metadata.len() as usize;
        if bytes > Limits::default().max_source_bytes
            || definitions.len() >= Limits::default().max_schema_nodes
        {
            return Err(super::diagnostics::metadata_error(&file, "/schema", format!("Named type definitions exceed the pack source budget. Maximum source bytes: {}; maximum schema nodes: {}", Limits::default().max_source_bytes, Limits::default().max_schema_nodes)));
        }
        let content = std::fs::read_to_string(&path).map_err(|_| {
            super::diagnostics::located(
                Error::io("Cannot read named type file; check storage permissions"),
                &file,
                "",
            )
        })?;
        let value = serde_yaml_ng::from_str(&content)
            .map_err(|error| super::diagnostics::yaml_error(&file, &error))?;
        let definition: SchemaTypeDefinition =
            super::diagnostics::decode_yaml_value(&file, "", value)?;
        definition
            .validate(pack_ref)
            .map_err(|error| super::diagnostics::located(error, &file, ""))?;
        let type_ref = definition.r#ref.clone();
        if let Some(previous) = source_files.insert(type_ref.clone(), file.clone()) {
            return Err(super::diagnostics::metadata_error(&file, "/ref", format!("Duplicate named type declaration; the same ref is already declared in {previous}")));
        }
        definitions.insert(type_ref, definition);
    }
    Ok(definitions)
}

pub async fn preflight(
    connection: &mut PgConnection,
    pack: i64,
    pack_ref: &str,
    pack_dir: &Path,
    omission: AbsentMetadataPolicy,
) -> Result<PackTypePlan> {
    SchemaTypeRepository::lock_graph(&mut *connection, pack).await?;
    let declared = read_definitions(pack_dir, pack_ref)?;
    let consumers = read_consumer_schemas(pack_dir, pack_ref)?;
    let owner = PackRepository::find_by_id(&mut *connection, pack)
        .await?
        .ok_or_else(|| Error::not_found("pack", "id", pack.to_string()))?;
    let mut effective = if omission == AbsentMetadataPolicy::Remove {
        BTreeMap::new()
    } else {
        SchemaTypeRepository::list_by_pack(&mut *connection, pack)
            .await?
            .into_iter()
            .map(|row| (row.r#ref.clone(), row.definition()))
            .collect()
    };
    effective.extend(declared.clone());
    let mut registry: RegistryView = effective
        .iter()
        .map(|(type_ref, definition)| {
            (
                type_ref.clone(),
                RegistryEntry::Candidate(definition.schema.clone()),
            )
        })
        .collect();
    let mut metadata = effective.clone();
    let mut dependencies = BTreeMap::from([(
        pack_ref.to_owned(),
        parse_dependencies(&owner.dependencies)?,
    )]);
    let mut versions = BTreeMap::new();
    let mut pending: VecDeque<_> = effective
        .values()
        .map(|definition| {
            (
                definition.r#ref.clone(),
                SchemaContext::SingleValue,
                definition.schema.clone(),
            )
        })
        .collect();
    pending.extend(consumers.iter().map(|consumer| {
        (
            consumer.source_ref.clone(),
            SchemaContext::FlatDocument,
            consumer.schema.clone(),
        )
    }));
    let mut checked = BTreeSet::new();
    let mut source_bytes = 0usize;
    while let Some((source_ref, context, schema)) = pending.pop_front() {
        if !checked.insert((
            source_ref.clone(),
            matches!(context, SchemaContext::FlatDocument),
            crate::value_schema::digest(&schema)
                .map_err(|_| Error::validation("Invalid schema source"))?,
        )) {
            continue;
        }
        source_bytes = source_bytes.saturating_add(
            serde_json::to_vec(&schema)
                .map_err(|_| Error::validation("Invalid named type source"))?
                .len(),
        );
        if source_bytes > Limits::default().max_source_bytes
            || checked.len() > Limits::default().max_schema_nodes
        {
            return Err(Error::validation(
                "Named type graph exceeds its source budget",
            ));
        }
        let source_pack = source_ref
            .split_once('.')
            .map(|(pack, _)| pack)
            .unwrap_or(&source_ref);
        let consumer = consumers
            .iter()
            .find(|consumer| consumer.source_ref == source_ref && consumer.schema == schema);
        let file = consumer
            .map(|c| c.source_file.clone())
            .unwrap_or_else(|| definition_file(pack_dir, &source_ref));
        let field = consumer
            .map(|c| super::diagnostics::pointer(&c.schema_field))
            .unwrap_or_else(|| "/schema".into());
        let reference_result: Result<()> = async {
            for reference in inspect_definition(context, &schema, Limits::default())
                .map_err(|error| {
                    Error::validation(format!("Invalid named type '{source_ref}': {error}"))
                })?
                .into_iter()
            {
                let use_field = format!("{field}{}", reference.path);
                let use_result: Result<()> = async {
                    let target_pack = reference
                        .type_ref
                        .split_once('.')
                        .map(|(pack, _)| pack)
                        .unwrap_or("");
                    if !metadata.contains_key(&reference.type_ref) {
                        if target_pack == pack_ref {
                            return Err(Error::validation(format!(
                                "Missing local named type '{}'",
                                reference.type_ref
                            )));
                        }
                        let revision = SchemaTypeRepository::active_revision(
                            &mut *connection,
                            &reference.type_ref,
                        )
                        .await?
                        .ok_or_else(|| {
                            Error::validation(format!(
                                "Missing active named type '{}'",
                                reference.type_ref
                            ))
                        })?;
                        if !revision.active {
                            return Err(Error::validation("Named type reference is retired"));
                        }
                        let definition: SchemaTypeDefinition =
                            serde_json::from_value(revision.definition).map_err(|_| {
                                Error::validation("Invalid immutable named type definition")
                            })?;
                        definition.validate(&revision.pack_ref)?;
                        if crate::value_schema::digest(&definition.schema)
                            .map_err(|_| Error::validation("Invalid named type digest"))?
                            != revision.content_digest
                        {
                            return Err(Error::validation("Named type revision digest mismatch"));
                        }
                        let declared_dependencies: BTreeMap<String, String> =
                            serde_json::from_value(revision.dependencies).map_err(|_| {
                                Error::validation("Invalid immutable schema dependency contract")
                            })?;
                        dependencies.insert(revision.pack_ref.clone(), declared_dependencies);
                        versions.insert(revision.pack_ref.clone(), revision.version);
                        registry.insert(
                            reference.type_ref.clone(),
                            RegistryEntry::Available(TypeRevision {
                                source: SourceIdentity::Released {
                                    component_ref: reference.type_ref.clone(),
                                    release_id: revision.release_id,
                                    release_digest: revision.release_digest,
                                },
                                revision_id: revision.revision_id,
                                schema: definition.schema.clone(),
                            }),
                        );
                        pending.push_back((
                            reference.type_ref.clone(),
                            SchemaContext::SingleValue,
                            definition.schema.clone(),
                        ));
                        metadata.insert(reference.type_ref.clone(), definition);
                    }
                    let target = &metadata[&reference.type_ref];
                    if !target.reference_allowed(source_pack) {
                        return Err(Error::validation(
                            "Named type reference policy denies this dependency",
                        ));
                    }
                    if target_pack != pack_ref {
                        let current = SchemaTypeRepository::find_by_ref(
                            &mut *connection,
                            &reference.type_ref,
                        )
                        .await?
                        .ok_or_else(|| Error::validation("Missing named type owner"))?;
                        if current.retired_at.is_some()
                            || !current.definition().reference_allowed(source_pack)
                        {
                            return Err(Error::validation(
                                "Current named type reference policy denies this dependency",
                            ));
                        }
                    }
                    if source_pack != target_pack {
                        let constraint = dependencies
                            .get(source_pack)
                            .and_then(|declared| declared.get(target_pack))
                            .ok_or_else(|| {
                                Error::validation(
                                    "Cross-pack named types require a declared pack dependency",
                                )
                            })?;
                        let requirement = semver::VersionReq::parse(constraint).map_err(|_| {
                            Error::validation("Invalid schema dependency version constraint")
                        })?;
                        let version = if target_pack == pack_ref {
                            &owner.version
                        } else {
                            versions.get(target_pack).ok_or_else(|| {
                                Error::validation("Missing schema dependency revision version")
                            })?
                        };
                        let version = semver::Version::parse(version).map_err(|_| {
                            Error::validation("Invalid schema source release version")
                        })?;
                        if !requirement.matches(&version) {
                            return Err(Error::validation(
                        "Latest schema dependency release does not satisfy the declared constraint",
                    ));
                        }
                    }
                    Ok(())
                }
                .await;
                use_result
                    .map_err(|error| super::diagnostics::located(error, &file, &use_field))?;
            }
            Ok(())
        }
        .await;
        reference_result.map_err(|error| super::diagnostics::located(error, &file, &field))?;
    }
    let mut contracts = BTreeMap::new();
    for definition in effective.values() {
        let contract = compile(
            SourceIdentity::Authoring {
                component_ref: definition.r#ref.clone(),
            },
            SchemaContext::SingleValue,
            &definition.schema,
            &registry,
            Limits::default(),
        )
        .map_err(|error| {
            super::diagnostics::schema_error(
                &definition_file(pack_dir, &definition.r#ref),
                "/schema",
                error,
            )
        })?;
        contracts.insert(definition.r#ref.clone(), contract);
    }
    for consumer in &consumers {
        let contract = compile(
            SourceIdentity::Authoring {
                component_ref: consumer.source_ref.clone(),
            },
            SchemaContext::FlatDocument,
            &consumer.schema,
            &registry,
            Limits::default(),
        )
        .map_err(|error| {
            super::diagnostics::schema_error(
                &consumer.source_file,
                &super::diagnostics::pointer(&consumer.schema_field),
                error,
            )
        })?;
        contracts.insert(
            format!("{}#{}", consumer.source_ref, consumer.schema_field),
            contract,
        );
    }
    Ok(PackTypePlan {
        declared,
        contracts,
        registry,
    })
}

pub fn parse_dependencies(declarations: &[String]) -> Result<BTreeMap<String, String>> {
    let mut dependencies = BTreeMap::new();
    for declaration in declarations {
        let (pack, constraint) = declaration.split_once('@').unwrap_or((declaration, "*"));
        crate::schema::RefValidator::validate_pack_ref(pack)?;
        semver::VersionReq::parse(constraint)
            .map_err(|_| Error::validation("Invalid named type dependency constraint"))?;
        if dependencies
            .insert(pack.to_owned(), constraint.to_owned())
            .is_some()
        {
            return Err(Error::validation("Duplicate named type pack dependency"));
        }
    }
    Ok(dependencies)
}

struct ConsumerSchemaSource {
    source_file: String,
    source_ref: String,
    schema_field: String,
    schema: serde_json::Value,
}

fn read_consumer_schemas(pack_dir: &Path, pack_ref: &str) -> Result<Vec<ConsumerSchemaSource>> {
    let mut consumers = Vec::new();
    let mut entries = 0;
    for entry in walkdir::WalkDir::new(pack_dir).follow_links(false) {
        entries += 1;
        if entries > 10_000 {
            return Err(Error::validation(
                "Pack schema traversal exceeds its entry budget",
            ));
        }
        let entry = entry.map_err(|_| Error::validation("Cannot inspect pack schema sources"))?;
        if !entry.file_type().is_file()
            || !matches!(
                entry.path().extension().and_then(|value| value.to_str()),
                Some("yaml" | "yml")
            )
        {
            continue;
        }
        let relative = entry
            .path()
            .strip_prefix(pack_dir)
            .map_err(|_| Error::validation("Invalid schema source path"))?;
        if relative.starts_with("types") {
            continue;
        }
        let parent = relative
            .parent()
            .and_then(|value| value.to_str())
            .unwrap_or("");
        let groups: &[&[&str]] = match parent {
            "" if relative.file_name().and_then(|value| value.to_str()) == Some("pack.yaml") => {
                &[&["conf_schema", "config_schema"]]
            }
            "actions" | "triggers" | "sensors" | "workflows" | "actions/workflows" => &[
                crate::schema_type::PARAM_SCHEMA_FIELDS,
                crate::schema_type::OUTPUT_SCHEMA_FIELDS,
                &["event_config_schema"],
            ],
            "queues" => &[&["item_schema"]],
            _ => continue,
        };
        if entry
            .metadata()
            .map_err(|_| Error::validation("Cannot inspect pack schema source"))?
            .len()
            > Limits::default().max_source_bytes as u64
        {
            return Err(Error::validation(
                "Pack schema source exceeds its byte budget",
            ));
        }
        let content = std::fs::read_to_string(entry.path())
            .map_err(|_| Error::validation("Cannot read pack schema source"))?;
        let data: serde_yaml_ng::Value = serde_yaml_ng::from_str(&content)
            .map_err(|error| super::diagnostics::yaml_error(&relative.to_string_lossy(), &error))?;
        let reference = data
            .get("ref")
            .and_then(|value| value.as_str())
            .unwrap_or("")
            .trim();
        let source_ref = if parent.is_empty() {
            pack_ref.to_string()
        } else if reference.contains('.') {
            reference.to_string()
        } else if !reference.is_empty() {
            format!("{pack_ref}.{reference}")
        } else {
            format!("{pack_ref}.{}", relative.display())
        };
        for fields in groups {
            if let Some(schema) =
                crate::schema_type::read_schema_field(&data, fields).map_err(|error| {
                    super::diagnostics::located(
                        error,
                        &relative.to_string_lossy(),
                        &super::diagnostics::pointer(fields[0]),
                    )
                })?
            {
                consumers.push(ConsumerSchemaSource {
                    source_file: relative.to_string_lossy().into_owned(),
                    source_ref: source_ref.clone(),
                    schema_field: fields
                        .iter()
                        .find(|field| data.get(**field).is_some())
                        .unwrap_or(&fields[0])
                        .to_string(),
                    schema,
                });
            }
        }
    }
    Ok(consumers)
}

fn definition_file(root: &Path, reference: &str) -> String {
    if let Ok(files) = super::loader::read_yaml_files(&root.join("types")) {
        for (name, content) in files {
            if let Ok(value) = serde_yaml_ng::from_str::<serde_yaml_ng::Value>(&content) {
                if value.get("ref").and_then(serde_yaml_ng::Value::as_str) == Some(reference) {
                    return format!("types/{name}");
                }
            }
        }
    }
    "types".into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn definitions_and_dependencies_fail_closed() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("types")).unwrap();
        let body = "ref: demo.value\nlabel: Value\nschema: {type: string}\n";
        std::fs::write(root.path().join("types/value.yaml"), body).unwrap();
        assert_eq!(read_definitions(root.path(), "demo").unwrap().len(), 1);
        std::fs::write(root.path().join("types/duplicate.yml"), body).unwrap();
        assert!(read_definitions(root.path(), "demo").is_err());
        assert!(parse_dependencies(&["external@^1".into()]).is_ok());
        assert!(parse_dependencies(&["external".into(), "external@^1".into()]).is_err());
        assert!(parse_dependencies(&["external@not-a-version".into()]).is_err());
    }

    #[test]
    fn local_graph_checks_unused_cycles_without_external_bodies() {
        let definitions = BTreeMap::from([(
            "demo.value".into(),
            serde_json::json!({
                "type": "object", "properties": {"value": {"type": "external.value"}},
                "$defs": {"unused": {"$ref": "#/$defs/unused"}}
            }),
        )]);
        assert!(crate::value_schema::check_local_graph(&definitions, Limits::default()).is_err());
        let definitions = BTreeMap::from([(
            "demo.value".into(),
            serde_json::json!({"type": "external.value"}),
        )]);
        assert!(crate::value_schema::check_local_graph(&definitions, Limits::default()).is_ok());
    }
}
