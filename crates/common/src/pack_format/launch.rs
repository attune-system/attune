use super::*;
use anyhow::{Context, Result};
use serde_yaml_ng::Value;
use std::collections::{BTreeMap, BTreeSet};

fn component_kind(path: &str) -> Option<&str> {
    let (directory, name) = path.split_once('/')?;
    (matches!(directory, "actions" | "sensors")
        && !name.contains('/')
        && (name.ends_with(".yaml") || name.ends_with(".yml")))
    .then_some(directory)
}

fn runtime(value: &Value, sensor: bool) -> Result<String> {
    let name = value
        .get("runner_type")
        .map(|v| v.as_str().context("runner_type must be a string"))
        .transpose()?
        .unwrap_or(if sensor { "native" } else { "" });
    let name = name.to_ascii_lowercase();
    Ok(crate::runtime_detection::normalize_runtime_name(
        name.strip_prefix("core.").unwrap_or(&name),
    ))
}

pub(super) fn legacy_executables<'a>(
    root: &Path,
    files: impl Iterator<Item = &'a FileEntry>,
) -> Result<Vec<String>> {
    let mut executables = vec![];
    for file in files {
        let Some(kind) = component_kind(&file.path) else {
            continue;
        };
        let value = source::yaml(root, &file.path)?;
        if runtime(&value, kind == "sensors")? == "native" {
            if let Some(entry) = value.get("entry_point") {
                let entry = entry.as_str().context("entry_point must be a string")?;
                paths::validate(entry)?;
                executables.push(format!("{kind}/{entry}"));
            }
        }
    }
    Ok(executables)
}

pub(super) fn validate(
    root: &Path,
    manifest: &Manifest,
    limits: Limits,
) -> Result<BTreeMap<String, BTreeSet<String>>> {
    limit(
        "artifacts",
        manifest.artifacts.len() as u64,
        limits.artifacts,
    )?;
    source::sorted_unique(
        &manifest
            .artifacts
            .iter()
            .map(Artifact::id)
            .collect::<Vec<_>>(),
        "artifacts",
    )?;
    let files: BTreeMap<_, _> = manifest
        .files
        .iter()
        .map(|f| (f.path.as_str(), f))
        .collect();
    let mut variant_paths = BTreeSet::new();
    for artifact in &manifest.artifacts {
        paths::id(artifact.id())?;
        let (variants, count): (Vec<(&str, &str, FileMode)>, _) = match artifact {
            Artifact::Native { variants, .. } => {
                ensure!(!variants.is_empty(), "native artifact has no variants");
                let mut targets = BTreeSet::new();
                for variant in variants {
                    ensure!(targets.insert(&variant.target), "duplicate native target");
                }
                (
                    variants
                        .iter()
                        .map(|v| (v.id.as_str(), v.path.as_str(), FileMode::Executable))
                        .collect(),
                    variants.len(),
                )
            }
            Artifact::Jar { variants, .. } => {
                ensure!(
                    variants.len() == 1 && variants[0].id == "portable",
                    "JAR requires exactly one portable variant"
                );
                (
                    variants
                        .iter()
                        .map(|v| (v.id.as_str(), v.path.as_str(), FileMode::Data))
                        .collect(),
                    variants.len(),
                )
            }
        };
        limit(
            "variants_per_artifact",
            count as u64,
            limits.variants_per_artifact,
        )?;
        source::sorted_unique(
            &variants.iter().map(|v| v.0).collect::<Vec<_>>(),
            "variants",
        )?;
        for (id, path, mode) in variants {
            paths::id(id)?;
            let file = files
                .get(path)
                .context("artifact variant file is missing")?;
            ensure!(
                file.mode == mode,
                "artifact file has wrong executable intent: {path}"
            );
            variant_paths.insert(path);
        }
    }
    for path in files.keys() {
        if path.starts_with(".attune/artifacts/") {
            ensure!(
                variant_paths.contains(path),
                "undeclared generated artifact: {path}"
            );
        }
    }
    let artifacts: BTreeMap<_, _> = manifest.artifacts.iter().map(|a| (a.id(), a)).collect();
    let mut consumers = BTreeMap::<String, BTreeSet<String>>::new();
    let mut jar_main_required = BTreeSet::new();
    let mut refs = BTreeSet::new();
    for file in &manifest.files {
        let Some(kind) = component_kind(&file.path) else {
            continue;
        };
        let value = source::yaml(root, &file.path)?;
        let component = value
            .get("ref")
            .and_then(Value::as_str)
            .context("component requires ref")?;
        crate::schema::RefValidator::validate_component_ref(component)?;
        ensure!(
            component.starts_with(&format!("{}.", manifest.pack.r#ref)),
            "component outside pack namespace: {component}"
        );
        ensure!(
            refs.insert((kind, component.to_owned())),
            "duplicate component ref: {component}"
        );
        let runner = runtime(&value, kind == "sensors")?;
        let launch_value = value.get("launch");
        let entry = value.get("entry_point");
        let workflow = value.get("workflow_file");
        ensure!(
            [launch_value.is_some(), entry.is_some(), workflow.is_some()]
                .into_iter()
                .filter(|v| *v)
                .count()
                == 1,
            "exactly one of launch, entry_point, workflow_file is required: {component}"
        );
        if let Some(workflow) = workflow {
            ensure!(kind == "actions", "sensors cannot launch workflows");
            let path = workflow
                .as_str()
                .context("workflow_file must be a string")?;
            paths::validate(path)?;
            require_data(&files, &format!("actions/{path}"))?;
            continue;
        }
        if let Some(entry) = entry {
            let path = entry.as_str().context("entry_point must be a string")?;
            paths::validate(path)?;
            let path = format!("{kind}/{path}");
            let file = files
                .get(path.as_str())
                .context("legacy entry_point file is missing")?;
            let mode = if runner == "native" {
                FileMode::Executable
            } else {
                FileMode::Data
            };
            ensure!(file.mode == mode, "entry_point has wrong executable intent");
            continue;
        }
        let launch: LaunchSpec =
            serde_yaml_ng::from_value(launch_value.context("missing launch")?.clone())
                .with_context(|| format!("invalid launch: {component}"))?;
        let mut consume = |id: &str, native: bool| -> Result<()> {
            paths::id(id)?;
            let artifact = artifacts
                .get(id)
                .with_context(|| format!("unresolved artifact {id} for {component}"))?;
            ensure!(
                matches!(artifact, Artifact::Native { .. }) == native,
                "artifact kind mismatch: {id}"
            );
            consumers
                .entry(id.into())
                .or_default()
                .insert(component.into());
            Ok(())
        };
        match launch {
            LaunchSpec::File { path } => {
                ensure!(
                    matches!(
                        runner.as_str(),
                        "shell" | "python" | "node" | "java" | "ruby" | "perl" | "go" | "r"
                    ),
                    "file launch requires a known interpreter runtime"
                );
                require_data(&files, &path)?;
            }
            LaunchSpec::Native { artifact } => {
                ensure!(runner == "native", "native launch requires native runtime");
                consume(&artifact, true)?;
            }
            LaunchSpec::JavaJar { artifact, jvm_args } => {
                ensure!(runner == "java", "java_jar requires java runtime");
                jvm_arguments(&jvm_args)?;
                consume(&artifact, false)?;
                jar_main_required.insert(artifact);
            }
            LaunchSpec::JavaClass {
                main_class,
                classpath,
                jvm_args,
            } => {
                ensure!(runner == "java", "java_class requires java runtime");
                java_class(&main_class)?;
                jvm_arguments(&jvm_args)?;
                ensure!(!classpath.is_empty(), "classpath must be nonempty");
                let mut seen = BTreeSet::new();
                for entry in classpath {
                    match entry {
                        ClasspathEntry::Artifact { artifact } => {
                            consume(&artifact, false)?;
                            if let Artifact::Jar { variants, .. } = artifacts[artifact.as_str()] {
                                ensure!(
                                    seen.insert(variants[0].path.clone()),
                                    "duplicate resolved classpath entry"
                                );
                            }
                        }
                        ClasspathEntry::Path { path } => {
                            paths::validate(&path)?;
                            ensure!(seen.insert(path.clone()), "duplicate classpath entry");
                            ensure!(
                                !files.contains_key(path.as_str()),
                                "classpath path must name a directory"
                            );
                            let prefix = format!("{path}/");
                            let subtree: Vec<_> = files
                                .values()
                                .filter(|f| f.path.starts_with(&prefix))
                                .collect();
                            ensure!(
                                !subtree.is_empty(),
                                "classpath directory is empty or missing"
                            );
                            ensure!(
                                subtree.iter().all(|f| f.mode == FileMode::Data),
                                "classpath inputs must be non-executable"
                            );
                        }
                    }
                }
            }
            LaunchSpec::Intrinsic {
                handler: IntrinsicHandler::InquiryV1,
            } => {
                anyhow::bail!(
                    "attune.inquiry/v1 was removed; use an action-owned inquiry and wait_for.inquiry"
                );
            }
        }
    }
    for artifact in &manifest.artifacts {
        ensure!(
            consumers.contains_key(artifact.id()),
            "unused artifact: {}",
            artifact.id()
        );
        if let Artifact::Jar { id, variants, .. } = artifact {
            jar::validate(
                &root.join(&variants[0].path),
                jar_main_required.contains(id),
                limits,
            )?;
        }
    }
    for file in &manifest.files {
        if file.path == "pack.yaml"
            || matches!(
                Path::new(&file.path).extension().and_then(|s| s.to_str()),
                Some("yaml" | "yml" | "java" | "jar" | "class")
            )
        {
            ensure!(
                file.mode == FileMode::Data,
                "definitions and Java inputs must be non-executable: {}",
                file.path
            );
        }
    }
    Ok(consumers)
}

fn require_data(files: &BTreeMap<&str, &FileEntry>, path: &str) -> Result<()> {
    paths::validate(path)?;
    ensure!(
        files.get(path).is_some_and(|f| f.mode == FileMode::Data),
        "missing or executable interpreter input: {path}"
    );
    Ok(())
}

pub(super) fn java_class(name: &str) -> Result<()> {
    // JVM binary names, including nested classes, but not paths or launcher options.
    ensure!(
        !name.is_empty()
            && name.split('.').all(|part| {
                let mut chars = part.chars();
                chars
                    .next()
                    .is_some_and(|c| c == '_' || c == '$' || c.is_alphabetic())
                    && chars.all(|c| c == '_' || c == '$' || c.is_alphanumeric())
            }),
        "invalid Java main_class: {name:?}"
    );
    Ok(())
}

fn jvm_arguments(args: &[String]) -> Result<()> {
    for arg in args {
        ensure!(
            !arg.contains(['\0', '\r', '\n', '@']) && !arg.contains("{{") && !arg.contains("${"),
            "JVM arguments must be literal and cannot use argument files"
        );
        // Accept self-contained VM options only. No positional operand can become
        // a main class or consume the launcher's classpath/-jar arguments.
        let allowed = arg.starts_with("-Xmx")
            || arg.starts_with("-Xms")
            || arg.starts_with("-XX:")
            || arg.starts_with("-D")
            || matches!(
                arg.as_str(),
                "-ea" | "-da" | "-esa" | "-dsa" | "-server" | "-client"
            );
        ensure!(
            allowed
                && ![
                    "-Djava.class.path",
                    "-Djava.module.path",
                    "-Djdk.module.",
                    "-Djava.system.class.loader",
                    "-Dsun.boot.class.path",
                    "-XX:SharedArchiveFile",
                    "-XX:ArchiveClassesAtExit",
                    "-XX:Flags",
                    "-XX:VMOptionsFile"
                ]
                .iter()
                .any(|prefix| arg.starts_with(prefix)),
            "JVM argument can override launch or is unsupported: {arg}"
        );
    }
    Ok(())
}
