//! Source locations survive validation and service boundaries without exposing values.
use crate::pack_check::PackDiagnostic;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct DeploymentDetails {
    pub stage: String,
    pub outcome: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pack_ref: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub install_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub release_id: Option<i64>,
    #[serde(default)]
    pub diagnostics: Vec<PackDiagnostic>,
}

impl DeploymentDetails {
    pub fn new(stage: &str, diagnostics: Vec<PackDiagnostic>) -> Self {
        Self {
            stage: stage.into(),
            outcome: "active_release_unchanged".into(),
            pack_ref: None,
            version: None,
            install_id: None,
            release_id: None,
            diagnostics,
        }
    }
}

impl std::fmt::Display for DeploymentDetails {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.stage.replace('_', " "))?;
        for diagnostic in &self.diagnostics {
            write!(f, "\n{}", diagnostic.path.as_deref().unwrap_or("pack"))?;
            if let Some(path) = diagnostic
                .metadata_path
                .as_deref()
                .filter(|path| !path.is_empty())
            {
                write!(f, " at {path}")?;
            }
            write!(f, ": {}", diagnostic.message)?;
        }
        Ok(())
    }
}

pub fn metadata_error(file: &str, field: &str, message: impl Into<String>) -> crate::Error {
    let message = message.into();
    crate::Error::Deployment {
        details: Box::new(DeploymentDetails::new(
            "metadata_validation",
            vec![PackDiagnostic::at(file, field, &message)],
        )),
        source: Box::new(crate::Error::Validation(message)),
    }
}

pub fn schema_error(
    file: &str,
    field: &str,
    error: crate::value_schema::SchemaError,
) -> crate::Error {
    let message = error.code.to_string();
    let mut diagnostic = PackDiagnostic::at(file, format!("{field}{}", error.path), &message);
    diagnostic.code = error.code.as_str().into();
    diagnostic.component_ref = error.source_ref;
    diagnostic.reference_chain = error.reference_chain;
    crate::Error::Deployment {
        details: Box::new(DeploymentDetails::new(
            "metadata_validation",
            vec![diagnostic],
        )),
        source: Box::new(crate::Error::Validation(message)),
    }
}

pub fn located(error: crate::Error, file: &str, field: &str) -> crate::Error {
    if matches!(error, crate::Error::Deployment { .. }) {
        return error;
    }
    // Infrastructure errors must not expose SQL, providers, or filesystem roots.
    let message = match &error {
        crate::Error::Validation(m) | crate::Error::SchemaValidation(m) | crate::Error::PermissionDenied(m) => m.clone(),
        _ => "The API could not process this component. Check the service configuration and retry only after checking install status.".into(),
    };
    crate::Error::Deployment {
        details: Box::new(DeploymentDetails::new(
            "component_loading",
            vec![PackDiagnostic::at(file, field, message)],
        )),
        source: Box::new(error),
    }
}

pub fn yaml_error(file: &str, error: &serde_yaml_ng::Error) -> crate::Error {
    // YAML's Display can include submitted scalar values. Only expose its position.
    let mut diagnostic = PackDiagnostic::at(
        file,
        "",
        "Invalid YAML syntax or field type. Check the indicated location.",
    );
    diagnostic.code = "metadata.invalid_yaml".into();
    if let Some(location) = error.location() {
        diagnostic.line = Some(location.line());
        diagnostic.column = Some(location.column());
    }
    crate::Error::Deployment {
        details: Box::new(DeploymentDetails::new(
            "metadata_validation",
            vec![diagnostic],
        )),
        source: Box::new(crate::Error::Validation("Invalid YAML metadata".into())),
    }
}

pub fn pointer(field: &str) -> String {
    format!("/{}", field.replace('~', "~0").replace('/', "~1"))
}

pub fn source_for_ref(root: &std::path::Path, folder: &str, reference: &str) -> Option<String> {
    for (name, content) in super::loader::read_yaml_files(&root.join(folder)).ok()? {
        let value: serde_yaml_ng::Value = serde_yaml_ng::from_str(&content).ok()?;
        if let Some(authored) = value.get("ref").and_then(serde_yaml_ng::Value::as_str) {
            if authored == reference
                || (!authored.contains('.') && reference.rsplit('.').next() == Some(authored))
            {
                return Some(format!("{folder}/{name}"));
            }
        }
    }
    None
}

pub fn decode_yaml_value<T: serde::de::DeserializeOwned>(
    file: &str,
    base: &str,
    value: serde_yaml_ng::Value,
) -> crate::Result<T> {
    serde_path_to_error::deserialize(value).map_err(|error| {
        let mut path = base.to_string();
        for segment in error.path().iter() {
            match segment {
                serde_path_to_error::Segment::Seq { index } => path.push_str(&format!("/{index}")),
                serde_path_to_error::Segment::Map { key } => path.push_str(&pointer(key)),
                serde_path_to_error::Segment::Enum { variant } => path.push_str(&pointer(variant)),
                serde_path_to_error::Segment::Unknown => {}
            }
        }
        metadata_error(
            file,
            &path,
            "Invalid field type or unsupported value. Check the metadata schema for this field.",
        )
    })
}

pub fn read_manifest(root: &std::path::Path) -> crate::Result<serde_yaml_ng::Value> {
    let content = std::fs::read_to_string(root.join("pack.yaml")).map_err(|_| {
        located(
            crate::Error::io("Cannot read pack manifest"),
            "pack.yaml",
            "",
        )
    })?;
    let mut value: serde_yaml_ng::Value =
        serde_yaml_ng::from_str(&content).map_err(|error| yaml_error("pack.yaml", &error))?;
    let map = value
        .as_mapping_mut()
        .ok_or_else(|| metadata_error("pack.yaml", "", "Pack manifest must be an object"))?;
    let tags_field = if map.get("tags").is_some() {
        "tags"
    } else {
        "keywords"
    };
    for (canonical, alias) in [
        ("label", "name"),
        ("conf_schema", "config_schema"),
        ("meta", "metadata"),
        ("tags", "keywords"),
    ] {
        if let Some(legacy) = map.get(alias).cloned() {
            if let Some(current) = map.get(canonical) {
                if current != &legacy {
                    return Err(metadata_error("pack.yaml", &pointer(canonical), format!("Conflicting '{canonical}' and '{alias}' declarations; keep only '{canonical}'")));
                }
            } else {
                map.insert(canonical.into(), legacy);
            }
        }
    }
    for field in ["ref", "version"] {
        if !map
            .get(field)
            .and_then(serde_yaml_ng::Value::as_str)
            .is_some_and(|v| !v.trim().is_empty())
        {
            return Err(metadata_error(
                "pack.yaml",
                &pointer(field),
                "Required field must be a non-empty string",
            ));
        }
    }
    for field in ["tags", "runtime_deps", "dependencies"] {
        let authored_field = if field == "tags" { tags_field } else { field };
        if let Some(value) = map.get(field) {
            let list = value.as_sequence().ok_or_else(|| {
                metadata_error(
                    "pack.yaml",
                    &pointer(authored_field),
                    "Expected an array of strings",
                )
            })?;
            for (index, item) in list.iter().enumerate() {
                if !item.as_str().is_some_and(|v| !v.trim().is_empty()) {
                    return Err(metadata_error(
                        "pack.yaml",
                        &format!("{}/{index}", pointer(authored_field)),
                        "Expected a non-empty string",
                    ));
                }
            }
        }
    }
    Ok(value)
}

pub fn validate_metadata(root: &std::path::Path) -> crate::Result<Vec<PackDiagnostic>> {
    let manifest = read_manifest(root)?;
    if let Some(testing) = manifest.get("testing") {
        let config: crate::test_executor::TestConfig =
            decode_yaml_value("pack.yaml", "/testing", testing.clone())?;
        config
            .validate_policy()
            .map_err(|error| located(error, "pack.yaml", "/testing"))?;
        for (name, runner) in &config.runners {
            let base = format!("/testing/runners{}", pointer(name));
            if !matches!(runner.r#type.as_str(), "script" | "unittest" | "pytest") {
                return Err(metadata_error(
                    "pack.yaml",
                    &format!("{base}/type"),
                    "Supported test runners are script, unittest, and pytest",
                ));
            }
            if runner
                .result_format
                .as_deref()
                .is_some_and(|format| format != "simple")
            {
                return Err(metadata_error(
                    "pack.yaml",
                    &format!("{base}/result_format"),
                    "Only the simple test result format is supported",
                ));
            }
            let entry = std::path::Path::new(&runner.entry_point);
            if entry.is_absolute()
                || entry
                    .components()
                    .any(|c| matches!(c, std::path::Component::ParentDir))
                || !root.join(entry).is_file()
            {
                return Err(metadata_error(
                    "pack.yaml",
                    &format!("{base}/entry_point"),
                    "Test entrypoint must name an existing file within the pack",
                ));
            }
        }
    }
    let report = crate::pack_check::check_pack(root);
    let errors: Vec<_> = report
        .diagnostics
        .iter()
        .filter(|d| d.severity == crate::pack_check::PackDiagnosticSeverity::Error)
        .cloned()
        .collect();
    if !errors.is_empty() {
        let mut details = DeploymentDetails::new("metadata_validation", errors);
        details.pack_ref = report.pack_ref;
        details.version = report.version;
        return Err(crate::Error::Deployment {
            details: Box::new(details),
            source: Box::new(crate::Error::validation(
                "Pack metadata is invalid. Correct the listed source locations before deploying.",
            )),
        });
    }
    Ok(report.diagnostics)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn typed_field_errors_keep_nested_paths_without_values() {
        let value = serde_yaml_ng::from_str("enabled: true\nrunners:\n  python:\n    type: script\n    entry_point: run.sh\n    timeout: SECRET_CANARY\n").unwrap();
        let error =
            decode_yaml_value::<crate::test_executor::TestConfig>("pack.yaml", "/testing", value)
                .unwrap_err();
        let crate::Error::Deployment { details, .. } = &error else {
            panic!("missing details")
        };
        assert_eq!(
            details.diagnostics[0].metadata_path.as_deref(),
            Some("/testing/runners/python/timeout")
        );
        assert!(!error.to_string().contains("SECRET_CANARY"));
    }

    #[test]
    fn located_errors_preserve_authority_category() {
        let error = located(
            crate::Error::PermissionDenied(
                "Registering identity lacks the requested authority".into(),
            ),
            "permission_sets/write.yaml",
            "/grants/0",
        );
        assert!(error.is_auth_error());
        assert!(error
            .to_string()
            .contains("permission_sets/write.yaml at /grants/0"));
    }

    #[test]
    fn metadata_gate_rejects_bad_actions_with_exact_field() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("pack.yaml"), "ref: demo\nversion: 1.0.0\n").unwrap();
        std::fs::create_dir(root.path().join("actions")).unwrap();
        std::fs::write(
            root.path().join("actions/run.yaml"),
            "ref: demo.run\nrunner_type: native\nentry_point: run\ntimeout_seconds: 0\n",
        )
        .unwrap();
        let crate::Error::Deployment { details, .. } = validate_metadata(root.path()).unwrap_err()
        else {
            panic!("missing details")
        };
        assert!(details
            .diagnostics
            .iter()
            .any(
                |diagnostic| diagnostic.path.as_deref() == Some("actions/run.yaml")
                    && diagnostic.metadata_path.as_deref() == Some("/timeout_seconds")
            ));
    }

    #[test]
    fn schema_errors_prefix_authored_fields_and_use_plain_messages() {
        let error = crate::value_schema::compile_inline(
            "demo.run",
            crate::value_schema::SchemaContext::FlatDocument,
            &serde_json::json!({"value": {"type": "not-a-type"}}),
        )
        .unwrap_err();
        let located = schema_error("actions/run.yaml", "/parameters", error);
        let crate::Error::Deployment { details, .. } = &located else {
            panic!()
        };
        assert!(details.diagnostics[0]
            .metadata_path
            .as_deref()
            .unwrap()
            .starts_with("/parameters/"));
        assert!(!located.to_string().contains("InvalidDefinition"));
        assert!(!located.to_string().contains("MissingReference"));
    }
    #[test]
    fn manifest_preserves_canonical_fields_and_rejects_invalid_entries() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(
            root.path().join("pack.yaml"),
            "ref: demo\nversion: 1.0.0\nlabel: Demo\nconf_schema: {}\n",
        )
        .unwrap();
        assert_eq!(
            read_manifest(root.path()).unwrap()["label"].as_str(),
            Some("Demo")
        );
        std::fs::write(
            root.path().join("pack.yaml"),
            "ref: demo\nversion: 1.0.0\ndependencies: [core, 7]\n",
        )
        .unwrap();
        let crate::Error::Deployment { details, .. } = read_manifest(root.path()).unwrap_err()
        else {
            panic!("missing diagnostic")
        };
        assert_eq!(
            details.diagnostics[0].metadata_path.as_deref(),
            Some("/dependencies/1")
        );
    }
    #[test]
    fn yaml_errors_do_not_display_submitted_values() {
        let error =
            serde_yaml_ng::from_str::<serde_yaml_ng::Value>("a: [SECRET_CANARY").unwrap_err();
        let crate::Error::Deployment { details, source } = yaml_error("actions/run.yaml", &error)
        else {
            panic!()
        };
        assert!(details.diagnostics[0].line.is_some());
        assert!(!source.to_string().contains("SECRET_CANARY"));
    }
}
