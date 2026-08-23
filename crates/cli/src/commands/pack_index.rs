//! Pack registry index management utilities

use crate::output::{self, OutputFormat};
use anyhow::{Context, Result};
use attune_common::pack_registry::{
    calculate_directory_checksum, validate_remote_pack_url, ComponentSummary, InstallSource,
    PackContents, PackDependencies, PackIndex, PackIndexEntry, PackMeta,
};
use std::collections::{BTreeMap, HashSet};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

/// Update a registry index file with a new pack entry
#[allow(
    clippy::too_many_arguments,
    reason = "the arguments map directly to the index-update CLI options"
)]
pub async fn handle_index_update(
    index_path: String,
    pack_path: String,
    git_url: Option<String>,
    git_ref: Option<String>,
    archive_url: Option<String>,
    archive_checksum: Option<String>,
    update: bool,
    output_format: OutputFormat,
) -> Result<()> {
    // Load existing index
    // nosemgrep: rust.actix.path-traversal.tainted-path.tainted-path -- Registry index maintenance is a local CLI/admin operation over operator-supplied files.
    let index_file_path = Path::new(&index_path);
    if !index_file_path.exists() {
        return Err(anyhow::anyhow!("Index file not found: {}", index_path));
    }

    // nosemgrep: rust.actix.path-traversal.tainted-path.tainted-path -- The CLI intentionally reads the local index file selected by the operator.
    let index_content = fs::read_to_string(index_file_path)?;
    let mut index: PackIndex =
        serde_json::from_str(&index_content).context("Invalid index format")?;
    let previous_packs = serde_json::to_value(&index.packs)?;

    // Load pack.yaml from the pack directory
    // nosemgrep: rust.actix.path-traversal.tainted-path.tainted-path -- Local pack directories are explicit CLI inputs, not remote taint.
    let pack_dir = Path::new(&pack_path);
    if !pack_dir.exists() || !pack_dir.is_dir() {
        return Err(anyhow::anyhow!("Pack directory not found: {}", pack_path));
    }

    let (index_entry, directory_checksum) = build_index_entry(
        pack_dir,
        git_url.as_deref(),
        git_ref.as_deref(),
        archive_url.as_deref(),
        archive_checksum.as_deref(),
    )?;
    let pack_ref = index_entry.pack_ref.clone();
    let version = index_entry.version.clone();

    // Check if pack already exists in index
    let existing_index = index
        .packs
        .iter()
        .position(|pack| pack.pack_ref == pack_ref);

    if let Some(_idx) = existing_index {
        if !update {
            return Err(anyhow::anyhow!(
                "Pack '{}' already exists in index. Use --update to replace it.",
                pack_ref
            ));
        }
        if output_format == OutputFormat::Table {
            output::print_info(&format!("Updating existing entry for '{}'", pack_ref));
        }
    } else if output_format == OutputFormat::Table {
        output::print_info(&format!("Adding new entry for '{}'", pack_ref));
    }

    // Update or add entry
    if let Some(idx) = existing_index {
        index.packs[idx] = index_entry;
    } else {
        index.packs.push(index_entry);
    }

    index
        .packs
        .sort_by(|left, right| left.pack_ref.cmp(&right.pack_ref));
    if serde_json::to_value(&index.packs)? != previous_packs {
        index.last_updated = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    }
    validate_pack_index(&index)?;

    let updated_content = serde_json::to_string_pretty(&index)? + "\n";
    atomic_write(index_file_path, updated_content.as_bytes())?;

    match output_format {
        OutputFormat::Table => {
            output::print_success(&format!("✓ Index updated successfully: {}", index_path));
            output::print_info(&format!("  Pack: {} v{}", pack_ref, version));
            output::print_info(&format!(
                "  Directory checksum: sha256:{}",
                directory_checksum
            ));
        }
        OutputFormat::Json => {
            let response = serde_json::json!({
                "success": true,
                "index_file": index_path,
                "pack_ref": pack_ref,
                "version": version,
                "directory_checksum": format!("sha256:{}", directory_checksum),
                "action": if existing_index.is_some() { "updated" } else { "added" }
            });
            output::print_output(&response, OutputFormat::Json)?;
        }
        OutputFormat::Yaml => {
            let response = serde_json::json!({
                "success": true,
                "index_file": index_path,
                "pack_ref": pack_ref,
                "version": version,
                "directory_checksum": format!("sha256:{}", directory_checksum),
                "action": if existing_index.is_some() { "updated" } else { "added" }
            });
            output::print_output(&response, OutputFormat::Yaml)?;
        }
    }

    Ok(())
}

pub(super) fn build_index_entry(
    pack_dir: &Path,
    git_url: Option<&str>,
    git_ref: Option<&str>,
    archive_url: Option<&str>,
    archive_checksum: Option<&str>,
) -> Result<(PackIndexEntry, String)> {
    let pack_yaml_path = pack_dir.join("pack.yaml");
    if !pack_yaml_path.exists() {
        return Err(anyhow::anyhow!(
            "pack.yaml not found in directory: {}",
            pack_dir.display()
        ));
    }

    // nosemgrep: rust.actix.path-traversal.tainted-path.tainted-path -- Reading pack.yaml from a local operator-selected pack directory is expected CLI behavior.
    let pack_yaml_content = fs::read_to_string(&pack_yaml_path)?;
    let pack_yaml: serde_yaml_ng::Value = serde_yaml_ng::from_str(&pack_yaml_content)?;
    let pack_ref = required_string(&pack_yaml, "ref")?;
    let version = required_string(&pack_yaml, "version")?;
    let directory_checksum = calculate_directory_checksum(pack_dir)?;
    let git_checksum = format!("sha256:{directory_checksum}");

    let mut install_sources = Vec::new();
    if let Some(url) = git_url {
        let url = validate_index_source_url(url, "--git-url")?;
        let git_ref = git_ref
            .filter(|git_ref| !git_ref.is_empty())
            .ok_or_else(|| anyhow::anyhow!("--git-ref is required with --git-url"))?;
        install_sources.push(InstallSource::Git {
            url,
            git_ref: Some(git_ref.to_owned()),
            checksum: git_checksum.clone(),
        });
    }

    match (archive_url, archive_checksum) {
        (Some(url), Some(checksum)) => {
            let url = validate_index_source_url(url, "--archive-url")?;
            validate_sha256_checksum(checksum)?;
            install_sources.push(InstallSource::Archive {
                url,
                checksum: checksum.to_owned(),
            });
        }
        (Some(_), None) => anyhow::bail!(
            "--archive-checksum is required with --archive-url and must hash the exact archive bytes"
        ),
        (None, Some(_)) => anyhow::bail!("--archive-checksum requires --archive-url"),
        (None, None) => {}
    }

    if install_sources.is_empty() {
        anyhow::bail!(
            "at least one install source is required; provide --git-url or --archive-url"
        );
    }

    let metadata = match pack_yaml.get("meta") {
        Some(value) if value.is_mapping() => Some(value),
        Some(_) => anyhow::bail!("pack.yaml meta must be an object"),
        None => None,
    };
    let contents = filesystem_contents(pack_dir, &pack_ref, &pack_yaml)?;
    let entry = PackIndexEntry {
        pack_ref: pack_ref.clone(),
        label: first_present_string(&pack_yaml, &["label", "name"])?
            .unwrap_or_else(|| pack_ref.clone()),
        description: present_optional_string(&pack_yaml, "description")?.unwrap_or_default(),
        use_case: preferred_string(&pack_yaml, "use_case", metadata, "use_case")?,
        version,
        author: present_optional_string(&pack_yaml, "author")?
            .unwrap_or_else(|| "Unknown".to_owned()),
        email: present_optional_string(&pack_yaml, "email")?,
        homepage: preferred_string(&pack_yaml, "homepage", metadata, "documentation_url")?,
        repository: preferred_string(&pack_yaml, "repository", metadata, "repository_url")?,
        license: preferred_string(&pack_yaml, "license", metadata, "license")?
            .unwrap_or_else(|| "NOASSERTION".to_owned()),
        keywords: first_string_sequence(&pack_yaml, metadata, &["tags", "keywords"])?,
        runtime_deps: string_sequence(&pack_yaml, "runtime_deps")?,
        install_sources,
        contents,
        dependencies: normalize_dependencies(pack_yaml.get("dependencies"))?,
        meta: normalize_meta(metadata)?,
    };

    Ok((entry, directory_checksum))
}

fn required_string(value: &serde_yaml_ng::Value, field: &str) -> Result<String> {
    match value.get(field) {
        Some(value) => value
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| anyhow::anyhow!("pack.yaml {field} must be a string")),
        None => Err(anyhow::anyhow!("Missing '{}' field in pack.yaml", field)),
    }
}

fn first_present_string(value: &serde_yaml_ng::Value, fields: &[&str]) -> Result<Option<String>> {
    for field in fields {
        if value.get(*field).is_some() {
            return required_string(value, field).map(Some);
        }
    }
    Ok(None)
}

fn present_optional_string(value: &serde_yaml_ng::Value, field: &str) -> Result<Option<String>> {
    match value.get(field) {
        Some(_) => required_string(value, field).map(Some),
        None => Ok(None),
    }
}

fn preferred_string(
    manifest: &serde_yaml_ng::Value,
    field: &str,
    metadata: Option<&serde_yaml_ng::Value>,
    metadata_field: &str,
) -> Result<Option<String>> {
    if manifest.get(field).is_some() {
        return required_string(manifest, field).map(Some);
    }
    match metadata {
        Some(metadata) if metadata.get(metadata_field).is_some() => {
            required_string(metadata, metadata_field).map(Some)
        }
        _ => Ok(None),
    }
}

fn string_sequence(value: &serde_yaml_ng::Value, field: &str) -> Result<Vec<String>> {
    match value.get(field) {
        Some(value) => strings(value, field),
        None => Ok(Vec::new()),
    }
}

fn first_string_sequence(
    manifest: &serde_yaml_ng::Value,
    metadata: Option<&serde_yaml_ng::Value>,
    fields: &[&str],
) -> Result<Vec<String>> {
    for field in fields {
        if let Some(value) = manifest.get(*field) {
            return strings(value, field);
        }
    }
    match metadata.and_then(|value| value.get("keywords")) {
        Some(value) => strings(value, "meta.keywords"),
        None => Ok(Vec::new()),
    }
}

fn strings(value: &serde_yaml_ng::Value, field: &str) -> Result<Vec<String>> {
    let sequence = value
        .as_sequence()
        .ok_or_else(|| anyhow::anyhow!("pack.yaml {field} must be an array"))?;
    let mut values = sequence
        .iter()
        .map(|value| scalar_string(value, field))
        .collect::<Result<Vec<_>>>()?;
    values.sort();
    values.dedup();
    Ok(values)
}

fn scalar_string(value: &serde_yaml_ng::Value, field: &str) -> Result<String> {
    value
        .as_str()
        .map(str::to_owned)
        .or_else(|| value.as_i64().map(|value| value.to_string()))
        .or_else(|| value.as_u64().map(|value| value.to_string()))
        .or_else(|| {
            value.as_f64().and_then(|value| {
                serde_json::Number::from_f64(value).map(|value| value.to_string())
            })
        })
        .ok_or_else(|| {
            anyhow::anyhow!("pack.yaml {field} values must be strings or finite numbers")
        })
}

fn normalize_dependencies(
    value: Option<&serde_yaml_ng::Value>,
) -> Result<Option<PackDependencies>> {
    let Some(value) = value else {
        return Ok(None);
    };
    if value.is_sequence() {
        return Ok(Some(PackDependencies {
            packs: strings(value, "dependencies")?,
            ..PackDependencies::default()
        }));
    }
    if !value.is_mapping() {
        anyhow::bail!("pack.yaml dependencies must be an array or object");
    }

    Ok(Some(PackDependencies {
        attune_version: optional_scalar_string(value, "attune_version")?,
        python_version: optional_scalar_string(value, "python_version")?,
        nodejs_version: optional_scalar_string(value, "nodejs_version")?,
        packs: match value.get("packs") {
            Some(value) => strings(value, "dependencies.packs")?,
            None => Vec::new(),
        },
    }))
}

fn optional_scalar_string(value: &serde_yaml_ng::Value, field: &str) -> Result<Option<String>> {
    match value.get(field) {
        Some(value) => scalar_string(value, &format!("dependencies.{field}")).map(Some),
        None => Ok(None),
    }
}

fn normalize_meta(value: Option<&serde_yaml_ng::Value>) -> Result<Option<PackMeta>> {
    let Some(value) = value else {
        return Ok(None);
    };
    validate_json_compatible_yaml(value, "meta")?;
    let value = serde_json::to_value(value).context("pack.yaml meta must be JSON-compatible")?;
    let metadata: PackMeta =
        serde_json::from_value(value).context("pack.yaml meta is incompatible with PackMeta")?;
    ensure_unique_strings(
        "pack.yaml meta.tested_attune_versions",
        &metadata.tested_attune_versions,
    )?;
    Ok(Some(metadata))
}

fn validate_json_compatible_yaml(value: &serde_yaml_ng::Value, field: &str) -> Result<()> {
    if value.as_f64().is_some_and(|value| !value.is_finite()) {
        anyhow::bail!("pack.yaml {field} must contain only finite numbers");
    }
    if let Some(sequence) = value.as_sequence() {
        for item in sequence {
            validate_json_compatible_yaml(item, field)?;
        }
    }
    if let Some(mapping) = value.as_mapping() {
        for (key, item) in mapping {
            if key.as_str().is_none() {
                anyhow::bail!("pack.yaml {field} keys must be strings");
            }
            validate_json_compatible_yaml(item, field)?;
        }
    }
    Ok(())
}

fn component_summaries(value: &serde_yaml_ng::Value, field: &str) -> Result<Vec<ComponentSummary>> {
    let Some(components) = value.get(field) else {
        return Ok(Vec::new());
    };
    let components = components
        .as_mapping()
        .ok_or_else(|| anyhow::anyhow!("pack.yaml {field} must be an object"))?;
    let mut summaries = components
        .iter()
        .map(|(name, definition)| {
            let name = name.as_str().ok_or_else(|| {
                anyhow::anyhow!("pack.yaml {field} component names must be strings")
            })?;
            if definition.as_mapping().is_none() {
                anyhow::bail!("pack.yaml {field}.{name} must be an object");
            }
            strict_non_empty_string(definition, "workflow_file", &format!("{field}.{name}"))?;
            Ok(ComponentSummary {
                name: name.to_owned(),
                description: first_non_empty_string(
                    definition,
                    &["description", "label"],
                    &format!("{field}.{name}"),
                )?
                .unwrap_or_default(),
            })
        })
        .collect::<Result<Vec<_>>>()?;
    summaries.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(summaries)
}

fn filesystem_contents(
    pack_dir: &Path,
    pack_ref: &str,
    pack_yaml: &serde_yaml_ng::Value,
) -> Result<PackContents> {
    let action_files = component_files(pack_dir, "actions", pack_ref, true)?;
    let mut workflow_actions = Vec::new();
    let actions = match action_files {
        Some(files) => files
            .into_iter()
            .filter_map(|(summary, is_workflow)| {
                if is_workflow {
                    workflow_actions.push(summary);
                    None
                } else {
                    Some(summary)
                }
            })
            .collect(),
        None => component_summaries(pack_yaml, "actions")?,
    };

    let mut workflows = workflow_actions;
    workflows.extend(
        component_files(pack_dir, "workflows", pack_ref, false)?
            .map(|files| files.into_iter().map(|(summary, _)| summary).collect())
            .map_or_else(|| component_summaries(pack_yaml, "workflows"), Ok)?,
    );

    Ok(PackContents {
        actions: sorted_summaries(actions),
        sensors: filesystem_or_inline(pack_dir, pack_ref, pack_yaml, "sensors")?,
        triggers: filesystem_or_inline(pack_dir, pack_ref, pack_yaml, "triggers")?,
        rules: filesystem_or_inline(pack_dir, pack_ref, pack_yaml, "rules")?,
        workflows: sorted_summaries(workflows),
    })
}

fn filesystem_or_inline(
    pack_dir: &Path,
    pack_ref: &str,
    pack_yaml: &serde_yaml_ng::Value,
    component_type: &str,
) -> Result<Vec<ComponentSummary>> {
    Ok(
        match component_files(pack_dir, component_type, pack_ref, false)? {
            Some(files) => {
                sorted_summaries(files.into_iter().map(|(summary, _)| summary).collect())
            }
            None => component_summaries(pack_yaml, component_type)?,
        },
    )
}

fn component_files(
    pack_dir: &Path,
    component_type: &str,
    pack_ref: &str,
    classify_workflows: bool,
) -> Result<Option<Vec<(ComponentSummary, bool)>>> {
    let directory = pack_dir.join(component_type);
    if !directory.exists() {
        return Ok(None);
    }
    if !directory.is_dir() {
        anyhow::bail!("{} is not a directory", directory.display());
    }

    let mut paths = Vec::<PathBuf>::new();
    for entry in fs::read_dir(&directory)
        .with_context(|| format!("Failed to read {}", directory.display()))?
    {
        let path = entry
            .with_context(|| format!("Failed to read an entry in {}", directory.display()))?
            .path();
        if path.is_file()
            && matches!(
                path.extension().and_then(|extension| extension.to_str()),
                Some("yaml" | "yml")
            )
        {
            paths.push(path);
        }
    }
    paths.sort();
    if paths.is_empty() {
        return Ok(None);
    }

    paths
        .into_iter()
        .map(|path| {
            let content = fs::read_to_string(&path)
                .with_context(|| format!("Failed to read {}", path.display()))?;
            let definition: serde_yaml_ng::Value = serde_yaml_ng::from_str(&content)
                .with_context(|| format!("Failed to parse {}", path.display()))?;
            if definition.as_mapping().is_none() {
                anyhow::bail!("{} must contain a YAML object", path.display());
            }
            let fallback_name = path
                .file_stem()
                .and_then(|name| name.to_str())
                .ok_or_else(|| anyhow::anyhow!("Invalid component filename: {}", path.display()))?;
            let name = first_non_empty_string(
                &definition,
                &["ref", "name"],
                &format!("component {}", path.display()),
            )?
            .unwrap_or_else(|| fallback_name.to_owned());
            let name = name
                .strip_prefix(&format!("{pack_ref}."))
                .unwrap_or(&name)
                .to_owned();
            let description = first_non_empty_string(
                &definition,
                &["description", "label"],
                &format!("component {}", path.display()),
            )?
            .unwrap_or_default();
            let is_workflow = classify_workflows
                && strict_non_empty_string(
                    &definition,
                    "workflow_file",
                    &format!("component {}", path.display()),
                )?
                .is_some();
            Ok((ComponentSummary { name, description }, is_workflow))
        })
        .collect::<Result<Vec<_>>>()
        .map(Some)
}

fn sorted_summaries(mut summaries: Vec<ComponentSummary>) -> Vec<ComponentSummary> {
    summaries.sort_by(|left, right| left.name.cmp(&right.name));
    summaries
}

fn first_non_empty_string(
    value: &serde_yaml_ng::Value,
    fields: &[&str],
    context: &str,
) -> Result<Option<String>> {
    for field in fields {
        if let Some(value) = strict_non_empty_string(value, field, context)? {
            return Ok(Some(value));
        }
    }
    Ok(None)
}

fn strict_non_empty_string(
    value: &serde_yaml_ng::Value,
    field: &str,
    context: &str,
) -> Result<Option<String>> {
    match value.get(field) {
        Some(value) => value
            .as_str()
            .map(|value| (!value.is_empty()).then(|| value.to_owned()))
            .ok_or_else(|| anyhow::anyhow!("{context} {field} must be a string")),
        None => Ok(None),
    }
}

fn validate_index_source_url(url: &str, option: &str) -> Result<String> {
    let url = validate_remote_pack_url(url)?;
    if url.scheme() != "https" {
        anyhow::bail!("{option} must use HTTPS");
    }
    Ok(url.to_string())
}

fn validate_sha256_checksum(checksum: &str) -> Result<()> {
    let hash = checksum.strip_prefix("sha256:").unwrap_or_default();
    if hash.len() != 64
        || !hash
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        anyhow::bail!("checksum must use sha256:<64 lowercase hex characters>");
    }
    Ok(())
}

fn validate_pack_index(index: &PackIndex) -> Result<()> {
    if index.registry_name.trim().is_empty() {
        anyhow::bail!("Invalid index: registry_name must not be empty");
    }
    if index.registry_url.trim() != index.registry_url {
        anyhow::bail!("Invalid index: registry_url must not contain surrounding whitespace");
    }
    let registry_url = validate_remote_pack_url(&index.registry_url)
        .context("Invalid index: registry_url must be a valid remote URL")?;
    if registry_url.scheme() != "https" {
        anyhow::bail!("Invalid index: registry_url must use HTTPS");
    }
    if index.version != "1.0" {
        anyhow::bail!("Invalid index: version must be 1.0");
    }
    chrono::DateTime::parse_from_rfc3339(&index.last_updated)
        .context("Invalid index: last_updated must be an RFC 3339 timestamp")?;

    let mut previous_ref: Option<&str> = None;
    for pack in &index.packs {
        if !valid_pack_ref(&pack.pack_ref) {
            anyhow::bail!(
                "Invalid index entry '{}': ref must match ^[a-z][a-z0-9_-]*$",
                pack.pack_ref
            );
        }
        if previous_ref.is_some_and(|previous| previous >= pack.pack_ref.as_str()) {
            anyhow::bail!("Invalid index: pack refs must be unique and sorted");
        }
        previous_ref = Some(&pack.pack_ref);
        if pack.label.trim().is_empty()
            || pack.author.trim().is_empty()
            || pack.license.trim().is_empty()
        {
            anyhow::bail!(
                "Invalid index entry '{}': label, author, and license must not be empty",
                pack.pack_ref
            );
        }
        if let Some(homepage) = &pack.homepage {
            url::Url::parse(homepage).with_context(|| {
                format!(
                    "Invalid index entry '{}': homepage must be a valid URL",
                    pack.pack_ref
                )
            })?;
        }
        if let Some(repository) = &pack.repository {
            let repository = url::Url::parse(repository).with_context(|| {
                format!(
                    "Invalid index entry '{}': repository must be a valid URL",
                    pack.pack_ref
                )
            })?;
            if repository.scheme() != "https" {
                anyhow::bail!(
                    "Invalid index entry '{}': repository must use HTTPS",
                    pack.pack_ref
                );
            }
        }
        semver::Version::parse(&pack.version).map_err(|_| {
            anyhow::anyhow!(
                "Invalid index entry '{}': version must be semantic versioning",
                pack.pack_ref
            )
        })?;
        validate_unique_strings(&pack.pack_ref, "keywords", &pack.keywords)?;
        validate_unique_strings(&pack.pack_ref, "runtime_deps", &pack.runtime_deps)?;
        if let Some(meta) = &pack.meta {
            validate_unique_strings(
                &pack.pack_ref,
                "meta.tested_attune_versions",
                &meta.tested_attune_versions,
            )?;
        }
        if pack.install_sources.is_empty() {
            anyhow::bail!(
                "Invalid index entry '{}': at least one install source is required",
                pack.pack_ref
            );
        }
        for source in &pack.install_sources {
            if source.url().trim() != source.url() {
                anyhow::bail!(
                    "Invalid index entry '{}': install source URL must not contain surrounding whitespace",
                    pack.pack_ref
                );
            }
            validate_index_source_url(source.url(), "install source URL")?;
            validate_sha256_checksum(source.checksum()).with_context(|| {
                format!(
                    "Invalid index entry '{}': invalid source checksum",
                    pack.pack_ref
                )
            })?;
            if let InstallSource::Git { git_ref, .. } = source {
                if git_ref.as_deref().is_none_or(|git_ref| {
                    git_ref.trim().is_empty()
                        || git_ref.trim() != git_ref
                        || git_ref.starts_with('-')
                        || git_ref.chars().any(char::is_control)
                }) {
                    anyhow::bail!(
                        "Invalid index entry '{}': Git sources require a ref",
                        pack.pack_ref
                    );
                }
            }
        }
        for (component_type, components) in [
            ("actions", &pack.contents.actions),
            ("sensors", &pack.contents.sensors),
            ("triggers", &pack.contents.triggers),
            ("rules", &pack.contents.rules),
            ("workflows", &pack.contents.workflows),
        ] {
            let mut names = HashSet::new();
            for component in components {
                if component.name.trim().is_empty() || !names.insert(&component.name) {
                    anyhow::bail!(
                        "Invalid index entry '{}': {} component names must be non-empty and unique",
                        pack.pack_ref,
                        component_type
                    );
                }
            }
        }
    }
    Ok(())
}

fn valid_pack_ref(pack_ref: &str) -> bool {
    let mut bytes = pack_ref.bytes();
    bytes.next().is_some_and(|byte| byte.is_ascii_lowercase())
        && bytes
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"_-".contains(&byte))
}

fn validate_unique_strings(pack_ref: &str, field: &str, values: &[String]) -> Result<()> {
    let mut unique = HashSet::new();
    if values.iter().any(|value| !unique.insert(value)) {
        anyhow::bail!("Invalid index entry '{pack_ref}': {field} values must be unique");
    }
    Ok(())
}

fn ensure_unique_strings(field: &str, values: &[String]) -> Result<()> {
    let mut unique = HashSet::new();
    if values.iter().any(|value| !unique.insert(value)) {
        anyhow::bail!("{field} values must be unique");
    }
    Ok(())
}

fn atomic_write(path: &Path, content: &[u8]) -> Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| anyhow::anyhow!("Invalid index filename: {}", path.display()))?;

    for attempt in 0..100 {
        let temporary = parent.join(format!(
            ".{file_name}.{}.{}.tmp",
            std::process::id(),
            attempt
        ));
        let mut file = match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
        {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error).context("Failed to create temporary index file"),
        };

        let result = (|| -> Result<()> {
            if path.exists() {
                fs::set_permissions(&temporary, fs::metadata(path)?.permissions())?;
            }
            file.write_all(content)?;
            file.sync_all()?;
            drop(file);
            fs::rename(&temporary, path)?;
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        return result.context("Failed to atomically replace index file");
    }

    anyhow::bail!("Unable to allocate a temporary index file")
}

/// Merge multiple registry index files into one
pub async fn handle_index_merge(
    output_path: String,
    input_paths: Vec<String>,
    force: bool,
    output_format: OutputFormat,
) -> Result<()> {
    // Check if output file exists
    // nosemgrep: rust.actix.path-traversal.tainted-path.tainted-path -- Index merge output is a local CLI path controlled by the operator.
    let output_file_path = Path::new(&output_path);
    if output_file_path.exists() && !force {
        return Err(anyhow::anyhow!(
            "Output file already exists: {}. Use --force to overwrite.",
            output_path
        ));
    }

    let mut packs_map: BTreeMap<String, (semver::Version, PackIndexEntry)> = BTreeMap::new();
    let mut total_loaded = 0;
    let mut duplicates_resolved = 0;
    let mut registry_metadata: Option<(String, String, chrono::DateTime<chrono::FixedOffset>)> =
        None;

    // Load and merge all input files
    for input_path in &input_paths {
        // nosemgrep: rust.actix.path-traversal.tainted-path.tainted-path -- Index merge inputs are local operator-selected files.
        let input_file_path = Path::new(input_path);
        if !input_file_path.exists() {
            anyhow::bail!("Input index file not found: {input_path}");
        }

        if output_format == OutputFormat::Table {
            output::print_info(&format!("Loading: {}", input_path));
        }

        // nosemgrep: rust.actix.path-traversal.tainted-path.tainted-path -- The CLI intentionally reads each local input index file during merge.
        let index_content = fs::read_to_string(input_file_path)?;
        let index: PackIndex = serde_json::from_str(&index_content)
            .with_context(|| format!("Invalid index format in {input_path}"))?;
        validate_pack_index(&index)
            .map_err(|error| anyhow::anyhow!("Invalid index in {input_path}: {error:#}"))?;
        let updated = chrono::DateTime::parse_from_rfc3339(&index.last_updated)?;
        match &mut registry_metadata {
            Some((_, _, latest)) if updated > *latest => *latest = updated,
            None => {
                registry_metadata = Some((
                    index.registry_name.clone(),
                    index.registry_url.clone(),
                    updated,
                ))
            }
            _ => {}
        }

        for pack in index.packs {
            let pack_ref = pack.pack_ref.clone();
            let new_version = semver::Version::parse(&pack.version)?;
            if let Some((existing_version, existing_pack)) = packs_map.get_mut(&pack_ref) {
                if new_version.cmp_precedence(existing_version).is_gt() {
                    if output_format == OutputFormat::Table {
                        output::print_info(&format!(
                            "  Updating '{}' from {} to {}",
                            pack_ref, existing_pack.version, pack.version
                        ));
                    }
                    *existing_version = new_version;
                    *existing_pack = pack;
                } else if output_format == OutputFormat::Table {
                    output::print_info(&format!(
                        "  Keeping '{}' at {} (newer than {})",
                        pack_ref, existing_pack.version, pack.version
                    ));
                }
                duplicates_resolved += 1;
            } else {
                packs_map.insert(pack_ref, (new_version, pack));
            }
            total_loaded += 1;
        }
    }

    let (registry_name, registry_url, last_updated) =
        registry_metadata.ok_or_else(|| anyhow::anyhow!("At least one input index is required"))?;
    let packs: Vec<PackIndexEntry> = packs_map.into_values().map(|(_, pack)| pack).collect();
    let merged_index = PackIndex {
        registry_name,
        registry_url,
        version: "1.0".to_owned(),
        last_updated: last_updated.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        packs,
    };
    validate_pack_index(&merged_index).context("Merged index is invalid")?;

    let unique_packs = merged_index.packs.len();
    let merged_content = serde_json::to_string_pretty(&merged_index)? + "\n";
    atomic_write(output_file_path, merged_content.as_bytes())?;

    match output_format {
        OutputFormat::Table => {
            output::print_success(&format!(
                "✓ Merged {} index files into {}",
                input_paths.len(),
                output_path
            ));
            output::print_info(&format!("  Total packs loaded: {}", total_loaded));
            output::print_info(&format!("  Unique packs: {}", unique_packs));
            if duplicates_resolved > 0 {
                output::print_info(&format!("  Duplicates resolved: {}", duplicates_resolved));
            }
        }
        OutputFormat::Json => {
            let response = serde_json::json!({
                "success": true,
                "output_file": output_path,
                "sources_count": input_paths.len(),
                "total_loaded": total_loaded,
                "unique_packs": unique_packs,
                "duplicates_resolved": duplicates_resolved
            });
            output::print_output(&response, OutputFormat::Json)?;
        }
        OutputFormat::Yaml => {
            let response = serde_json::json!({
                "success": true,
                "output_file": output_path,
                "sources_count": input_paths.len(),
                "total_loaded": total_loaded,
                "unique_packs": unique_packs,
                "duplicates_resolved": duplicates_resolved
            });
            output::print_output(&response, OutputFormat::Yaml)?;
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_index_entry_requires_an_install_source() {
        let pack_dir = tempfile::TempDir::new().unwrap();
        fs::write(
            pack_dir.path().join("pack.yaml"),
            "ref: source-required\nversion: 1.0.0\n",
        )
        .unwrap();

        let error = build_index_entry(pack_dir.path(), None, None, None, None).unwrap_err();
        assert!(error.to_string().contains("at least one install source"));
        assert!(!error.to_string().contains("your-org"));
    }
}
