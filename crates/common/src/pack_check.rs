//! Read-only validation for an unpacked Attune pack directory.

use crate::action_visibility::collect_workflow_action_refs;
use crate::dashboard_spec::validate_dashboard_spec;
use crate::pack_cache_definition::{CacheDefinitionOwnerType, CacheDefinitionYaml};
use crate::policy_control::parse_policy_controls;
use crate::queue_definition::parse_work_queue_definition_yaml;
use crate::rbac::{validate_cache_grant_constraints, Grant};
use crate::schema::RefValidator;
use crate::workflow::parser::parse_workflow_yaml;
use serde::{Deserialize, Serialize};
use serde_yaml_ng::{Mapping, Value as YamlValue};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Read;
use std::path::{Component, Path, PathBuf};

const COMPONENT_DIRS: &[&str] = &[
    "permission_sets",
    "runtimes",
    "triggers",
    "actions",
    "dashboards",
    "queues",
    "policies",
    "rules",
    "sensors",
    "caches",
];
const MAX_METADATA_FILE_SIZE: u64 = 1024 * 1024;
const MAX_TRAVERSED_ENTRIES: usize = 10_000;
const MAX_METADATA_FILES: usize = 2_000;
const MAX_DIAGNOSTICS: usize = 1_000;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "lowercase")]
pub enum PackDiagnosticSeverity {
    Error,
    Warning,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PackDiagnostic {
    pub severity: PackDiagnosticSeverity,
    pub code: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackCheckReport {
    pub path: PathBuf,
    pub valid: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pack_ref: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    pub files_checked: usize,
    pub components: BTreeMap<String, usize>,
    pub errors: usize,
    pub warnings: usize,
    pub diagnostics: Vec<PackDiagnostic>,
}

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
struct LocalReference {
    source_path: String,
    source_kind: &'static str,
    target_kind: &'static str,
    target_ref: String,
}

struct PackChecker {
    root: PathBuf,
    report: PackCheckReport,
    refs: BTreeMap<&'static str, BTreeMap<String, String>>,
    references: BTreeSet<LocalReference>,
    checked_paths: BTreeSet<PathBuf>,
    traversed_entries: usize,
    traversal_limited: bool,
    metadata_limited: bool,
    diagnostics_limited: bool,
}

/// Validate all metadata that the pack registrar discovers without contacting an Attune server.
pub fn check_pack(path: impl AsRef<Path>) -> PackCheckReport {
    let supplied = path.as_ref();
    let absolute = if supplied.is_absolute() {
        supplied.to_path_buf()
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(supplied)
    };
    let root = absolute.canonicalize().unwrap_or(absolute);
    let mut checker = PackChecker {
        root: root.clone(),
        report: PackCheckReport {
            path: root,
            valid: false,
            pack_ref: None,
            version: None,
            files_checked: 0,
            components: BTreeMap::new(),
            errors: 0,
            warnings: 0,
            diagnostics: Vec::new(),
        },
        refs: BTreeMap::new(),
        references: BTreeSet::new(),
        checked_paths: BTreeSet::new(),
        traversed_entries: 0,
        traversal_limited: false,
        metadata_limited: false,
        diagnostics_limited: false,
    };
    checker.run();
    checker.finish()
}

impl PackChecker {
    fn run(&mut self) {
        if !self.root.exists() {
            self.error(None, "pack.path_missing", "Pack path does not exist");
            return;
        }
        if !self.root.is_dir() {
            self.error(
                None,
                "pack.path_not_directory",
                "Pack path is not a directory",
            );
            return;
        }

        self.check_manifest();
        for component in COMPONENT_DIRS {
            self.check_component_dir(component);
        }
        self.check_workflow_dir("workflows", false);
        self.check_workflow_dir("actions/workflows", true);
        self.check_local_references();
        self.find_ignored_yaml();
    }

    fn finish(mut self) -> PackCheckReport {
        self.report.diagnostics.sort_by(|a, b| {
            a.path
                .cmp(&b.path)
                .then(a.severity.cmp(&b.severity))
                .then(a.code.cmp(&b.code))
                .then(a.message.cmp(&b.message))
        });
        self.report.errors = self
            .report
            .diagnostics
            .iter()
            .filter(|item| item.severity == PackDiagnosticSeverity::Error)
            .count();
        self.report.warnings = self.report.diagnostics.len() - self.report.errors;
        self.report.valid = self.report.errors == 0;
        self.report
    }

    fn check_manifest(&mut self) {
        let path = self.root.join("pack.yaml");
        let Some(value) = self.read_yaml(&path, "manifest") else {
            if !path.exists() {
                self.error(
                    Some("pack.yaml"),
                    "manifest.missing",
                    "Pack directory must contain pack.yaml",
                );
            }
            return;
        };
        let Some(map) = value.as_mapping() else {
            self.error(
                Some("pack.yaml"),
                "manifest.not_object",
                "Pack manifest must be a YAML object",
            );
            return;
        };

        if let Some(pack_ref) = self.required_string(map, "ref", "pack.yaml") {
            if let Err(error) = RefValidator::validate_pack_ref(&pack_ref) {
                self.error(Some("pack.yaml"), "manifest.invalid_ref", error.to_string());
            } else {
                self.report.pack_ref = Some(pack_ref);
            }
        }
        if let Some(version) = self.required_string(map, "version", "pack.yaml") {
            if let Err(error) = semver::Version::parse(&version) {
                self.error(
                    Some("pack.yaml"),
                    "manifest.invalid_version",
                    format!("Pack version must be semantic version syntax: {error}"),
                );
            }
            self.report.version = Some(version);
        }
        self.optional_type(map, "label", "string", "pack.yaml");
        self.optional_type(map, "description", "string", "pack.yaml");
        self.optional_type(map, "author", "string", "pack.yaml");
        self.optional_type(map, "email", "string", "pack.yaml");
        self.optional_type(map, "enabled", "boolean", "pack.yaml");
        self.optional_type(map, "system", "boolean", "pack.yaml");
        self.optional_type(map, "config", "object", "pack.yaml");
        self.optional_type(map, "meta", "object", "pack.yaml");
        self.optional_type(map, "tags", "array", "pack.yaml");
        self.optional_type(map, "runtime_deps", "array", "pack.yaml");
        self.optional_type(map, "dependencies", "array", "pack.yaml");
        self.check_flat_schema(map, "conf_schema", "pack.yaml");
    }

    fn check_component_dir(&mut self, component: &'static str) {
        let dir = self.root.join(component);
        if !dir.exists() {
            return;
        }
        if !dir.is_dir() {
            self.error(
                Some(component),
                "component.not_directory",
                format!("Expected {component} to be a directory"),
            );
            return;
        }
        for path in self.yaml_files(&dir) {
            self.check_component_file(component, &path);
        }
    }

    fn check_component_file(&mut self, component: &'static str, path: &Path) {
        let rel = self.relative(path);
        let Some(value) = self.read_yaml(path, component) else {
            return;
        };
        let Some(map) = value.as_mapping() else {
            self.error(
                Some(&rel),
                "component.not_object",
                format!("{component} metadata must be a YAML object"),
            );
            return;
        };

        match component {
            "queues" => self.check_queue(path, &rel),
            "dashboards" => self.check_dashboard(&value, &rel),
            "permission_sets" => self.check_permission_set(map, &rel),
            "actions" => self.check_action(map, &rel),
            "sensors" => self.check_sensor(map, &rel),
            "rules" => self.check_rule(map, &rel),
            "runtimes" => self.check_runtime(map, &rel),
            "caches" => self.check_cache(&value, &rel),
            "triggers" => {
                self.check_owned_ref(map, component, "trigger", &rel, false);
                self.check_flat_schema(map, "parameters", &rel);
                self.check_flat_schema(map, "output", &rel);
            }
            "policies" => {
                self.check_owned_ref(map, component, "policy", &rel, true);
                self.check_policy(&value, map, &rel);
            }
            _ => {}
        }
    }

    fn check_policy(&mut self, value: &YamlValue, map: &Mapping, rel: &str) {
        if let Err(error) = parse_policy_controls(value) {
            self.error(Some(rel), "policy.invalid_controls", error);
        }
        if let Some(pack_ref) = self.optional_string(map, "pack_ref", rel) {
            if self.report.pack_ref.as_deref() != Some(pack_ref.as_str()) {
                self.error(
                    Some(rel),
                    "policy.pack_ref_mismatch",
                    format!("Policy pack_ref '{pack_ref}' must equal the manifest ref"),
                );
            }
        }
        if let Some(action_ref) = self.optional_string(map, "action_ref", rel) {
            let qualified = self.qualify(&action_ref);
            if let Err(error) = RefValidator::validate_component_ref(&qualified) {
                self.error(Some(rel), "policy.invalid_action_ref", error.to_string());
            } else {
                self.add_reference(rel, "policy", "action", &action_ref);
            }
        }
    }

    fn check_permission_set(&mut self, map: &Mapping, rel: &str) {
        self.check_owned_ref(map, "permission_sets", "permission_set", rel, false);
        let Some(grants) = yaml_get(map, "grants") else {
            self.error(
                Some(rel),
                "permission_set.missing_grants",
                "Missing required field 'grants'",
            );
            return;
        };
        match serde_yaml_ng::from_value::<Vec<Grant>>(grants.clone()) {
            Ok(grants) => {
                for grant in grants {
                    if grant.actions.is_empty() {
                        self.error(
                            Some(rel),
                            "permission_set.empty_actions",
                            "Grant actions cannot be empty",
                        );
                    }
                    if let Some(constraints) = grant.constraints {
                        if let Err(error) = validate_cache_grant_constraints(&constraints) {
                            if matches!(grant.resource, crate::rbac::Resource::Caches) {
                                self.error(Some(rel), "permission_set.invalid_cache_grant", error);
                            }
                        }
                    }
                }
            }
            Err(error) => self.error(
                Some(rel),
                "permission_set.invalid_grants",
                format!("Invalid grants: {error}"),
            ),
        }
    }

    fn check_action(&mut self, map: &Mapping, rel: &str) {
        self.check_owned_ref(map, "actions", "action", rel, false);
        self.check_flat_schema(map, "parameters", rel);
        self.check_flat_schema_alias(map, &["output", "output_schema"], rel);
        self.optional_type(map, "enabled", "boolean", rel);
        self.optional_type(map, "runner_type", "string", rel);
        self.optional_type(map, "default_execution_permission_set_refs", "array", rel);

        if let Some(workflow_file) = self.optional_string(map, "workflow_file", rel) {
            self.check_referenced_workflow(&workflow_file, rel);
            return;
        }
        let Some(entrypoint) = self.required_string(map, "entry_point", rel) else {
            return;
        };
        let native = self
            .optional_string(map, "runner_type", rel)
            .is_some_and(|runner| {
                matches!(
                    runner.to_ascii_lowercase().as_str(),
                    "native" | "builtin" | "standalone"
                )
            });
        if !native {
            self.check_referenced_file(
                &self.root.join("actions"),
                &entrypoint,
                rel,
                "action.entrypoint",
            );
        }
    }

    fn check_sensor(&mut self, map: &Mapping, rel: &str) {
        self.check_owned_ref(map, "sensors", "sensor", rel, false);
        self.check_flat_schema(map, "parameters", rel);
        let entrypoint = self.required_string(map, "entry_point", rel);
        let runner = self
            .optional_string(map, "runner_type", rel)
            .unwrap_or_else(|| "native".to_string());
        if let Some(entrypoint) = entrypoint {
            if !matches!(
                runner.to_ascii_lowercase().as_str(),
                "native" | "builtin" | "standalone"
            ) {
                self.check_referenced_file(
                    &self.root.join("sensors"),
                    &entrypoint,
                    rel,
                    "sensor.entrypoint",
                );
            }
        }
        let triggers = yaml_get(map, "trigger_types").or_else(|| yaml_get(map, "trigger_type"));
        match triggers {
            Some(YamlValue::String(value)) => self.add_reference(rel, "sensor", "trigger", value),
            Some(YamlValue::Sequence(values)) => {
                for value in values {
                    if let Some(value) = value.as_str() {
                        self.add_reference(rel, "sensor", "trigger", value);
                    } else {
                        self.error(
                            Some(rel),
                            "sensor.invalid_trigger_types",
                            "trigger_types must contain only strings",
                        );
                    }
                }
            }
            Some(_) => self.error(
                Some(rel),
                "sensor.invalid_trigger_types",
                "trigger_type(s) must be a string or array of strings",
            ),
            None => self.error(
                Some(rel),
                "sensor.missing_trigger_types",
                "Sensor requires trigger_type or trigger_types",
            ),
        }
    }

    fn check_rule(&mut self, map: &Mapping, rel: &str) {
        self.check_owned_ref(map, "rules", "rule", rel, true);
        for (field, kind) in [("trigger_ref", "trigger"), ("action_ref", "action")] {
            if let Some(target) = self.required_string(map, field, rel) {
                if let Err(error) = RefValidator::validate_component_ref(&self.qualify(&target)) {
                    self.error(
                        Some(rel),
                        "rule.invalid_reference",
                        format!("Invalid {field}: {error}"),
                    );
                } else {
                    self.add_reference(rel, "rule", kind, &target);
                }
            }
        }
        self.optional_type(map, "conditions", "object", rel);
        self.optional_type(map, "action_params", "object", rel);
        self.optional_type(map, "trigger_params", "object", rel);
    }

    fn check_runtime(&mut self, map: &Mapping, rel: &str) {
        self.check_owned_ref(map, "runtimes", "runtime", rel, false);
        self.required_string(map, "name", rel);
        self.optional_type(map, "execution_config", "object", rel);
        self.optional_type(map, "versions", "array", rel);
        if let Some(versions) = yaml_get(map, "versions").and_then(YamlValue::as_sequence) {
            let mut seen = BTreeSet::new();
            let mut defaults = 0;
            for version in versions {
                let Some(version_map) = version.as_mapping() else {
                    self.error(
                        Some(rel),
                        "runtime.invalid_version",
                        "Runtime versions must be objects",
                    );
                    continue;
                };
                if let Some(number) = self.required_string(version_map, "version", rel) {
                    if !seen.insert(number.clone()) {
                        self.error(
                            Some(rel),
                            "runtime.duplicate_version",
                            format!("Duplicate runtime version '{number}'"),
                        );
                    }
                }
                if yaml_get(version_map, "is_default").and_then(YamlValue::as_bool) == Some(true) {
                    defaults += 1;
                }
            }
            if defaults > 1 {
                self.error(
                    Some(rel),
                    "runtime.multiple_defaults",
                    "Only one runtime version may be the default",
                );
            }
        }
    }

    fn check_cache(&mut self, value: &YamlValue, rel: &str) {
        match serde_yaml_ng::from_value::<CacheDefinitionYaml>(value.clone()) {
            Ok(cache) => {
                self.record_ref("cache", &cache.r#ref, rel, false);
                if cache.owner_ref.trim().is_empty() {
                    self.error(
                        Some(rel),
                        "cache.invalid_owner",
                        "Cache owner_ref cannot be empty",
                    );
                }
                if let Err(error) = cache.validate() {
                    self.error(Some(rel), "cache.invalid_definition", error.to_string());
                }
                match cache.owner_type {
                    CacheDefinitionOwnerType::Pack => {
                        if self.report.pack_ref.as_deref() != Some(cache.owner_ref.as_str()) {
                            self.error(
                                Some(rel),
                                "cache.owner_mismatch",
                                "Pack-owned cache owner_ref must equal the manifest ref",
                            );
                        }
                    }
                    CacheDefinitionOwnerType::Action => {
                        self.add_reference(rel, "cache", "action", &cache.owner_ref)
                    }
                    CacheDefinitionOwnerType::Sensor => {
                        self.add_reference(rel, "cache", "sensor", &cache.owner_ref)
                    }
                }
            }
            Err(error) => self.error(
                Some(rel),
                "cache.invalid_definition",
                format!("Invalid cache definition: {error}"),
            ),
        }
    }

    fn check_queue(&mut self, path: &Path, rel: &str) {
        let Some(content) = self.read_text(path, rel) else {
            return;
        };
        match parse_work_queue_definition_yaml(&content) {
            Ok(queue) => {
                self.record_ref("queue", &queue.r#ref, rel, true);
                self.add_reference(rel, "queue", "action", &queue.dispatch_action);
            }
            Err(error) => self.error(Some(rel), "queue.invalid_definition", error.to_string()),
        }
    }

    fn check_dashboard(&mut self, value: &YamlValue, rel: &str) {
        if let Some(map) = value.as_mapping() {
            self.check_owned_ref(map, "dashboards", "dashboard", rel, true);
        }
        match serde_json::to_value(value) {
            Ok(value) => {
                if let Err(error) = validate_dashboard_spec(&value) {
                    self.error(Some(rel), "dashboard.invalid_spec", error);
                }
            }
            Err(error) => self.error(Some(rel), "dashboard.invalid_yaml_value", error.to_string()),
        }
    }

    fn check_workflow_dir(&mut self, relative_dir: &str, strict: bool) {
        let dir = self.root.join(relative_dir);
        if !dir.is_dir() {
            return;
        }
        for path in self.yaml_files(&dir) {
            let rel = self.relative(&path);
            if !self.track_metadata_file(&path, "workflows") {
                break;
            }
            let Some(content) = self.read_text(&path, &rel) else {
                continue;
            };
            match parse_workflow_yaml(&content) {
                Ok(workflow) => {
                    if !workflow.r#ref.is_empty() {
                        self.record_ref("workflow", &workflow.r#ref, &rel, false);
                    }
                    for action in collect_workflow_action_refs(&workflow) {
                        self.add_reference(&rel, "workflow", "action", &action);
                    }
                }
                Err(error) if strict => {
                    self.error(Some(&rel), "workflow.invalid_definition", error.to_string())
                }
                Err(error) => self.warning(
                    Some(&rel),
                    "workflow.invalid_legacy_definition",
                    format!("Legacy top-level workflow will be skipped: {error}"),
                ),
            }
        }
    }

    fn check_referenced_workflow(&mut self, workflow_file: &str, source: &str) {
        let Some(path) = self.safe_join(
            &self.root.join("actions"),
            workflow_file,
            source,
            "action.workflow_path",
        ) else {
            return;
        };
        if !path.is_file() {
            self.error(
                Some(source),
                "action.workflow_missing",
                format!("Workflow file '{workflow_file}' does not exist"),
            );
            return;
        }
        let Some(content) = self.read_text(&path, source) else {
            return;
        };
        match parse_workflow_yaml(&content) {
            Ok(workflow) => {
                if !workflow.r#ref.is_empty() {
                    self.validate_owned_component_ref("workflow", &workflow.r#ref, source);
                }
                for action in collect_workflow_action_refs(&workflow) {
                    self.add_reference(source, "workflow", "action", &action);
                }
            }
            Err(error) => self.error(
                Some(source),
                "action.invalid_workflow",
                format!("Invalid workflow file '{workflow_file}': {error}"),
            ),
        }
    }

    fn check_local_references(&mut self) {
        for reference in std::mem::take(&mut self.references) {
            let qualified = self.qualify(&reference.target_ref);
            let Some(pack_ref) = self.report.pack_ref.as_deref() else {
                continue;
            };
            if qualified
                .strip_prefix(pack_ref)
                .and_then(|rest| rest.strip_prefix('.'))
                .is_none()
            {
                continue;
            }
            if !self
                .refs
                .get(reference.target_kind)
                .is_some_and(|refs| refs.contains_key(&qualified))
            {
                self.error(
                    Some(&reference.source_path),
                    "reference.missing_local_target",
                    format!(
                        "{} references missing local {} '{}'",
                        reference.source_kind, reference.target_kind, qualified
                    ),
                );
            }
        }
    }

    fn find_ignored_yaml(&mut self) {
        let mut ignored = Vec::new();
        for entry in walkdir::WalkDir::new(&self.root).follow_links(false) {
            let Ok(entry) = entry else { continue };
            let path = entry.path();
            if !self.track_traversed_entry() {
                return;
            }
            if !entry.file_type().is_file() || !is_yaml(path) || self.checked_paths.contains(path) {
                continue;
            }
            ignored.push(path.to_path_buf());
        }
        ignored.sort();
        for path in ignored {
            self.warning(
                Some(&self.relative(&path)),
                "metadata.ignored_location",
                "YAML file is outside a metadata location read by pack registration",
            );
        }
    }

    fn yaml_files(&mut self, dir: &Path) -> Vec<PathBuf> {
        let entries = match fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(error) => {
                self.error(
                    Some(&self.relative(dir)),
                    "metadata.directory_unreadable",
                    error.to_string(),
                );
                return Vec::new();
            }
        };
        let mut paths = Vec::new();
        for entry in entries {
            if !self.track_traversed_entry() {
                return Vec::new();
            }
            let Ok(entry) = entry else { continue };
            let path = entry.path();
            if path.is_file() && is_yaml(&path) {
                paths.push(path);
            }
        }
        paths.sort();
        paths
    }

    fn read_yaml(&mut self, path: &Path, component: &str) -> Option<YamlValue> {
        let rel = self.relative(path);
        if !self.track_metadata_file(path, component) {
            return None;
        }
        let content = self.read_text(path, &rel)?;
        match serde_yaml_ng::from_str(&content) {
            Ok(value) => Some(value),
            Err(error) => {
                self.error(Some(&rel), "metadata.invalid_yaml", error.to_string());
                None
            }
        }
    }

    fn track_metadata_file(&mut self, path: &Path, component: &str) -> bool {
        if self.checked_paths.contains(path) {
            return true;
        }
        if self.report.files_checked >= MAX_METADATA_FILES {
            if !self.metadata_limited {
                self.metadata_limited = true;
                self.error(
                    None,
                    "limits.metadata_files_exceeded",
                    format!("Pack contains more than {MAX_METADATA_FILES} metadata files"),
                );
            }
            return false;
        }
        self.checked_paths.insert(path.to_path_buf());
        self.report.files_checked += 1;
        *self
            .report
            .components
            .entry(component.to_string())
            .or_default() += 1;
        true
    }

    fn track_traversed_entry(&mut self) -> bool {
        if self.traversed_entries >= MAX_TRAVERSED_ENTRIES {
            if !self.traversal_limited {
                self.traversal_limited = true;
                self.error(
                    None,
                    "limits.traversed_entries_exceeded",
                    format!("Pack traversal exceeds {MAX_TRAVERSED_ENTRIES} entries"),
                );
            }
            return false;
        }
        self.traversed_entries += 1;
        true
    }

    fn read_text(&mut self, path: &Path, rel: &str) -> Option<String> {
        let metadata = match fs::metadata(path) {
            Ok(metadata) => metadata,
            Err(error) => {
                if path.exists() {
                    self.error(Some(rel), "metadata.unreadable", error.to_string());
                }
                return None;
            }
        };
        if metadata.len() > MAX_METADATA_FILE_SIZE {
            self.error(
                Some(rel),
                "limits.metadata_file_too_large",
                format!("Metadata file exceeds {MAX_METADATA_FILE_SIZE} bytes"),
            );
            return None;
        }
        let mut file = match fs::File::open(path) {
            Ok(file) => file,
            Err(error) => {
                self.error(Some(rel), "metadata.unreadable", error.to_string());
                return None;
            }
        };
        let mut bytes = Vec::new();
        if let Err(error) = file
            .by_ref()
            .take(MAX_METADATA_FILE_SIZE + 1)
            .read_to_end(&mut bytes)
        {
            self.error(Some(rel), "metadata.unreadable", error.to_string());
            return None;
        }
        if bytes.len() as u64 > MAX_METADATA_FILE_SIZE {
            self.error(
                Some(rel),
                "limits.metadata_file_too_large",
                format!("Metadata file exceeds {MAX_METADATA_FILE_SIZE} bytes"),
            );
            return None;
        }
        match String::from_utf8(bytes) {
            Ok(content) => Some(content),
            Err(error) => {
                self.error(Some(rel), "metadata.unreadable", error.to_string());
                None
            }
        }
    }

    fn check_owned_ref(
        &mut self,
        map: &Mapping,
        component: &'static str,
        kind: &'static str,
        rel: &str,
        allow_short: bool,
    ) -> Option<String> {
        let value = self.required_string(map, "ref", rel)?;
        self.record_ref(kind, &value, rel, allow_short);
        if component != "queues" {
            self.optional_type(map, "label", "string", rel);
            self.optional_type(map, "description", "string", rel);
        }
        Some(self.qualify(&value))
    }

    fn record_ref(&mut self, kind: &'static str, value: &str, rel: &str, allow_short: bool) {
        let qualified = if allow_short {
            self.qualify(value)
        } else {
            value.to_string()
        };
        let validation = if kind == "queue" {
            RefValidator::validate_work_queue_ref(&qualified)
        } else {
            RefValidator::validate_component_ref(&qualified)
        };
        if let Err(error) = validation {
            self.error(Some(rel), "component.invalid_ref", error.to_string());
            return;
        }
        if let Some(pack_ref) = self.report.pack_ref.as_deref() {
            if !qualified.starts_with(&format!("{pack_ref}.")) {
                self.error(
                    Some(rel),
                    "component.ref_pack_mismatch",
                    format!("Ref '{qualified}' must belong to pack '{pack_ref}'"),
                );
            }
        }
        let refs = self.refs.entry(kind).or_default();
        if let Some(first) = refs.insert(qualified.clone(), rel.to_string()) {
            self.error(
                Some(rel),
                "component.duplicate_ref",
                format!("Duplicate {kind} ref '{qualified}' (first defined in {first})"),
            );
        }
    }

    fn validate_owned_component_ref(&mut self, kind: &str, value: &str, rel: &str) {
        if let Err(error) = RefValidator::validate_component_ref(value) {
            self.error(Some(rel), "component.invalid_ref", error.to_string());
            return;
        }
        if let Some(pack_ref) = self.report.pack_ref.as_deref() {
            if !value.starts_with(&format!("{pack_ref}.")) {
                self.error(
                    Some(rel),
                    "component.ref_pack_mismatch",
                    format!("{kind} ref '{value}' must belong to pack '{pack_ref}'"),
                );
            }
        }
    }

    fn required_string(&mut self, map: &Mapping, field: &str, rel: &str) -> Option<String> {
        match yaml_get(map, field) {
            Some(YamlValue::String(value)) if !value.trim().is_empty() => {
                Some(value.trim().to_string())
            }
            Some(_) => {
                self.error(
                    Some(rel),
                    "metadata.invalid_field",
                    format!("Field '{field}' must be a non-empty string"),
                );
                None
            }
            None => {
                self.error(
                    Some(rel),
                    "metadata.missing_field",
                    format!("Missing required field '{field}'"),
                );
                None
            }
        }
    }

    fn optional_string(&mut self, map: &Mapping, field: &str, rel: &str) -> Option<String> {
        let value = yaml_get(map, field)?;
        match value {
            YamlValue::String(value) if !value.trim().is_empty() => Some(value.trim().to_string()),
            _ => {
                self.error(
                    Some(rel),
                    "metadata.invalid_field",
                    format!("Field '{field}' must be a non-empty string"),
                );
                None
            }
        }
    }

    fn optional_type(&mut self, map: &Mapping, field: &str, expected: &str, rel: &str) {
        let Some(value) = yaml_get(map, field) else {
            return;
        };
        let valid = match expected {
            "string" => value.is_string(),
            "boolean" => value.is_bool(),
            "array" => value.is_sequence(),
            "object" => value.is_mapping(),
            _ => true,
        };
        if !valid {
            self.error(
                Some(rel),
                "metadata.invalid_field_type",
                format!("Field '{field}' must be an {expected}"),
            );
        }
    }

    fn check_flat_schema(&mut self, map: &Mapping, field: &str, rel: &str) {
        let Some(schema) = yaml_get(map, field) else {
            return;
        };
        let Some(fields) = schema.as_mapping() else {
            self.error(
                Some(rel),
                "schema.not_object",
                format!("Field '{field}' must use Attune's flat per-field object format"),
            );
            return;
        };
        for (name, definition) in fields {
            if !name.is_string() || !definition.is_mapping() {
                self.error(
                    Some(rel),
                    "schema.invalid_field",
                    format!("Each '{field}' entry must have a string name and object definition"),
                );
                continue;
            }
            if definition.get("type").and_then(YamlValue::as_str).is_none() {
                self.error(
                    Some(rel),
                    "schema.missing_type",
                    format!(
                        "Schema field '{}.{}' requires string 'type'",
                        field,
                        name.as_str().unwrap_or("?")
                    ),
                );
            }
        }
    }

    fn check_flat_schema_alias(&mut self, map: &Mapping, fields: &[&str], rel: &str) {
        for field in fields {
            self.check_flat_schema(map, field, rel);
        }
    }

    fn check_referenced_file(&mut self, base: &Path, referenced: &str, source: &str, code: &str) {
        let Some(path) = self.safe_join(base, referenced, source, code) else {
            return;
        };
        if !path.is_file() {
            self.error(
                Some(source),
                format!("{code}_missing"),
                format!("Referenced file '{referenced}' does not exist"),
            );
        }
    }

    fn safe_join(
        &mut self,
        base: &Path,
        referenced: &str,
        source: &str,
        code: &str,
    ) -> Option<PathBuf> {
        let relative = Path::new(referenced);
        if relative.is_absolute()
            || relative.components().any(|item| {
                matches!(
                    item,
                    Component::ParentDir | Component::RootDir | Component::Prefix(_)
                )
            })
        {
            self.error(
                Some(source),
                code,
                format!("Referenced path '{referenced}' must remain inside the pack"),
            );
            return None;
        }
        let candidate = base.join(relative);
        if let Ok(canonical) = candidate.canonicalize() {
            if !canonical.starts_with(&self.root) {
                self.error(
                    Some(source),
                    code,
                    format!("Referenced path '{referenced}' escapes the pack directory"),
                );
                return None;
            }
            return Some(canonical);
        }
        Some(candidate)
    }

    fn add_reference(
        &mut self,
        source_path: &str,
        source_kind: &'static str,
        target_kind: &'static str,
        target_ref: &str,
    ) {
        self.references.insert(LocalReference {
            source_path: source_path.to_string(),
            source_kind,
            target_kind,
            target_ref: target_ref.to_string(),
        });
    }

    fn qualify(&self, value: &str) -> String {
        if value.contains('.') {
            value.to_string()
        } else if let Some(pack_ref) = &self.report.pack_ref {
            format!("{pack_ref}.{value}")
        } else {
            value.to_string()
        }
    }

    fn relative(&self, path: &Path) -> String {
        path.strip_prefix(&self.root)
            .unwrap_or(path)
            .components()
            .map(|part| part.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/")
    }

    fn error(&mut self, path: Option<&str>, code: impl Into<String>, message: impl Into<String>) {
        self.diagnostic(PackDiagnosticSeverity::Error, path, code, message);
    }

    fn warning(&mut self, path: Option<&str>, code: impl Into<String>, message: impl Into<String>) {
        self.diagnostic(PackDiagnosticSeverity::Warning, path, code, message);
    }

    fn diagnostic(
        &mut self,
        severity: PackDiagnosticSeverity,
        path: Option<&str>,
        code: impl Into<String>,
        message: impl Into<String>,
    ) {
        if self.diagnostics_limited {
            return;
        }
        if self.report.diagnostics.len() >= MAX_DIAGNOSTICS - 1 {
            self.diagnostics_limited = true;
            self.report.diagnostics.push(PackDiagnostic {
                severity: PackDiagnosticSeverity::Error,
                code: "limits.diagnostics_exceeded".to_string(),
                path: None,
                message: format!("Validation produced at least {MAX_DIAGNOSTICS} diagnostics"),
            });
            return;
        }
        self.report.diagnostics.push(PackDiagnostic {
            severity,
            code: code.into(),
            path: path.map(str::to_string),
            message: message.into(),
        });
    }
}

fn yaml_get<'a>(map: &'a Mapping, field: &str) -> Option<&'a YamlValue> {
    map.get(YamlValue::String(field.to_string()))
}

fn is_yaml(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension == "yaml" || extension == "yml")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn write(root: &Path, path: &str, content: &str) {
        let path = root.join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }

    #[test]
    fn valid_pack_checks_all_local_metadata() {
        let dir = TempDir::new().unwrap();
        write(
            dir.path(),
            "pack.yaml",
            "ref: demo\nlabel: Demo\nversion: 1.0.0\n",
        );
        write(
            dir.path(),
            "triggers/new.yaml",
            "ref: demo.new\nparameters:\n  value:\n    type: string\n",
        );
        write(dir.path(), "actions/run.yaml", "ref: demo.run\nrunner_type: shell\nentry_point: run.sh\nparameters:\n  value:\n    type: string\n");
        write(dir.path(), "actions/run.sh", "#!/bin/sh\n");
        write(dir.path(), "sensors/watch.yaml", "ref: demo.watch\nrunner_type: native\nentry_point: demo-watch\ntrigger_types: [demo.new]\n");
        write(
            dir.path(),
            "rules/run.yaml",
            "ref: demo.rule\ntrigger_ref: demo.new\naction_ref: demo.run\n",
        );

        let report = check_pack(dir.path());

        assert!(report.valid, "{:?}", report.diagnostics);
        assert_eq!(report.files_checked, 5);
        assert_eq!(report.pack_ref.as_deref(), Some("demo"));
    }

    #[test]
    fn reports_syntax_refs_entrypoints_and_cross_references() {
        let dir = TempDir::new().unwrap();
        write(dir.path(), "pack.yaml", "ref: demo\nversion: nope\n");
        write(
            dir.path(),
            "actions/run.yaml",
            "ref: other.run\nentry_point: missing.sh\n",
        );
        write(
            dir.path(),
            "rules/run.yaml",
            "ref: run\ntrigger_ref: missing\naction_ref: run\n",
        );
        write(dir.path(), "sensors/broken.yaml", "ref: [broken\n");

        let report = check_pack(dir.path());
        let codes = report
            .diagnostics
            .iter()
            .map(|item| item.code.as_str())
            .collect::<BTreeSet<_>>();

        assert!(!report.valid);
        assert!(codes.contains("manifest.invalid_version"));
        assert!(codes.contains("component.ref_pack_mismatch"));
        assert!(codes.contains("action.entrypoint_missing"));
        assert!(codes.contains("metadata.invalid_yaml"));
    }

    #[test]
    fn missing_directory_is_a_structured_failure() {
        let dir = TempDir::new().unwrap();
        let report = check_pack(dir.path().join("missing"));
        assert!(!report.valid);
        assert_eq!(report.diagnostics[0].code, "pack.path_missing");
    }

    #[test]
    fn warns_for_yaml_registration_will_ignore() {
        let dir = TempDir::new().unwrap();
        write(dir.path(), "pack.yaml", "ref: demo\nversion: 1.0.0\n");
        write(
            dir.path(),
            "actions/nested/ignored.yaml",
            "ref: demo.ignored\n",
        );
        let report = check_pack(dir.path());
        assert!(report.valid);
        assert_eq!(report.warnings, 1);
        assert_eq!(report.diagnostics[0].code, "metadata.ignored_location");
    }

    #[test]
    fn rejects_oversized_metadata_without_reading_it() {
        let dir = TempDir::new().unwrap();
        write(dir.path(), "pack.yaml", "ref: demo\nversion: 1.0.0\n");
        let path = dir.path().join("actions/large.yaml");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::File::create(path)
            .unwrap()
            .set_len(MAX_METADATA_FILE_SIZE + 1)
            .unwrap();

        let report = check_pack(dir.path());

        assert!(report
            .diagnostics
            .iter()
            .any(|item| item.code == "limits.metadata_file_too_large"));
    }

    #[test]
    fn accepts_global_policy_and_checks_same_pack_action_target() {
        let dir = TempDir::new().unwrap();
        write(dir.path(), "pack.yaml", "ref: demo\nversion: 1.0.0\n");
        write(
            dir.path(),
            "policies/global.yaml",
            "ref: global\nquotas:\n  - quota_type: daily\n    limit: 10\n",
        );
        write(
            dir.path(),
            "policies/missing.yaml",
            "ref: missing\naction_ref: absent\nconcurrency: { limit: 1, method: cancel }\n",
        );

        let report = check_pack(dir.path());

        assert!(!report
            .diagnostics
            .iter()
            .any(|item| item.code == "policy.missing_target"));
        assert!(report.diagnostics.iter().any(|item| {
            item.code == "reference.missing_local_target" && item.message.contains("demo.absent")
        }));
    }

    #[test]
    fn shared_workflow_ref_differs_from_actions_and_nested_refs_are_checked() {
        let dir = TempDir::new().unwrap();
        write(dir.path(), "pack.yaml", "ref: demo\nversion: 1.0.0\n");
        write(
            dir.path(),
            "actions/first.yaml",
            "ref: demo.first\nworkflow_file: workflows/shared.workflow.yaml\n",
        );
        write(
            dir.path(),
            "actions/second.yaml",
            "ref: demo.second\nworkflow_file: workflows/shared.workflow.yaml\n",
        );
        write(
            dir.path(),
            "actions/workflows/shared.workflow.yaml",
            "ref: demo.shared\nlabel: Shared\nversion: 1.0.0\ntasks:\n  - name: parallel\n    type: parallel\n    tasks:\n      - name: nested\n        action: demo.missing\n",
        );

        let report = check_pack(dir.path());

        assert!(!report
            .diagnostics
            .iter()
            .any(|item| item.code == "action.workflow_ref_mismatch"));
        assert!(!report
            .diagnostics
            .iter()
            .any(|item| item.code == "component.duplicate_ref"));
        assert!(report.diagnostics.iter().any(|item| {
            item.code == "reference.missing_local_target" && item.message.contains("demo.missing")
        }));
    }

    #[test]
    fn cache_checks_use_registration_namespace_and_policy_rules() {
        let dir = TempDir::new().unwrap();
        write(dir.path(), "pack.yaml", "ref: demo\nversion: 1.0.0\n");
        write(
            dir.path(),
            "caches/bad_namespace.yaml",
            "ref: demo.bad_namespace\nnamespace: Bad/Name\nowner_type: pack\nowner_ref: demo\n",
        );
        write(
            dir.path(),
            "caches/bad_retention.yaml",
            "ref: demo.bad_retention\nnamespace: retained\nowner_type: pack\nowner_ref: demo\nmax_retained_generations: 1\n",
        );
        write(
            dir.path(),
            "caches/zero_freshness.yaml",
            "ref: demo.zero_freshness\nnamespace: fresh\nowner_type: pack\nowner_ref: demo\nfreshness_target_seconds: 0\n",
        );

        let report = check_pack(dir.path());
        let cache_errors = report
            .diagnostics
            .iter()
            .filter(|item| item.code == "cache.invalid_definition")
            .collect::<Vec<_>>();

        assert_eq!(cache_errors.len(), 2);
        assert!(cache_errors.iter().any(|item| item
            .message
            .contains("cache namespace must be lowercase ASCII")));
        assert!(cache_errors
            .iter()
            .any(|item| item.message.contains("max_retained_generations")));
        assert!(!cache_errors
            .iter()
            .any(|item| item.path.as_deref() == Some("caches/zero_freshness.yaml")));
    }

    #[test]
    fn malformed_legacy_workflow_is_a_warning() {
        let dir = TempDir::new().unwrap();
        write(dir.path(), "pack.yaml", "ref: demo\nversion: 1.0.0\n");
        write(
            dir.path(),
            "workflows/legacy.yaml",
            "ref: demo.legacy\nversion: 1.0.0\nvars:\n  - old: format\ntasks: []\n",
        );

        let report = check_pack(dir.path());

        assert!(report.valid);
        assert_eq!(report.errors, 0);
        assert!(report.diagnostics.iter().any(|item| {
            item.code == "workflow.invalid_legacy_definition"
                && item.severity == PackDiagnosticSeverity::Warning
        }));
    }
}
