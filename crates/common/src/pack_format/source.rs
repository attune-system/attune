use super::*;
use anyhow::{Context, Result};
use serde_yaml_ng::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::{Read, Write};

pub(super) fn collect(options: &BuildOptions, root: &Path, limits: Limits) -> Result<Manifest> {
    ensure!(
        fs::symlink_metadata(&options.source)?.is_dir(),
        "pack source must be a real directory"
    );
    let source = options.source.canonicalize()?;
    let mut files = BTreeMap::new();
    let mut names = paths::Paths::default();
    let mut total = 0;
    let walker = ignore::WalkBuilder::new(&source)
        .hidden(false)
        .parents(false)
        .git_global(false)
        .git_exclude(false)
        .require_git(false)
        .follow_links(false)
        .filter_entry(|e| !matches!(e.file_name().to_str(), Some(".git" | ".hg" | ".svn")))
        .build();
    for entry in walker {
        let entry = entry?;
        if entry.path() == source {
            continue;
        }
        let kind = entry.file_type().context("missing source file type")?;
        ensure!(
            kind.is_dir() || kind.is_file(),
            "source contains link or special file: {}",
            entry.path().display()
        );
        if kind.is_dir() {
            continue;
        }
        let path = entry
            .path()
            .strip_prefix(&source)?
            .to_str()
            .context("non-UTF-8 source path")?;
        let path = icu_normalizer_v1::ComposingNormalizer::new_nfc().normalize(path);
        ensure!(
            path != MANIFEST_PATH && !path.starts_with(".attune/artifacts/"),
            "reserved generated release path: {path}"
        );
        names.insert(&path)?;
        limit("entries", files.len() as u64 + 2, limits.entries)?;
        let file = snapshot(
            entry.path(),
            root,
            &path,
            FileMode::Data,
            &mut total,
            limits,
        )?;
        files.insert(path, file);
    }
    let mut artifacts = BTreeMap::<String, Artifact>::new();
    for input in &options.artifacts {
        let (id, variant, source, mode) = match input {
            ArtifactInput::Native { id, target, source } => {
                (id, target.variant_id(), source, FileMode::Executable)
            }
            ArtifactInput::Jar { id, source } => (id, "portable".into(), source, FileMode::Data),
        };
        paths::id(id)?;
        let path = format!(".attune/artifacts/{id}/{variant}");
        names.insert(&path)?;
        limit("entries", files.len() as u64 + 2, limits.entries)?;
        files.insert(
            path.clone(),
            snapshot(source, root, &path, mode, &mut total, limits)?,
        );
        match input {
            ArtifactInput::Native { target, .. } => {
                let artifact = artifacts
                    .entry(id.clone())
                    .or_insert_with(|| Artifact::Native {
                        id: id.clone(),
                        component_refs: vec![],
                        variants: vec![],
                    });
                let Artifact::Native { variants, .. } = artifact else {
                    anyhow::bail!("conflicting artifact kinds: {id}");
                };
                variants.push(NativeVariant {
                    id: variant,
                    path,
                    target: target.clone(),
                });
                variants.sort_by(|a, b| a.id.cmp(&b.id));
                limit(
                    "variants_per_artifact",
                    variants.len() as u64,
                    limits.variants_per_artifact,
                )?;
            }
            ArtifactInput::Jar { .. } => {
                ensure!(
                    artifacts
                        .insert(
                            id.clone(),
                            Artifact::Jar {
                                id: id.clone(),
                                component_refs: vec![],
                                variants: vec![JarVariant { id: variant, path }]
                            }
                        )
                        .is_none(),
                    "duplicate artifact: {id}"
                );
                limit("variants_per_artifact", 1, limits.variants_per_artifact)?;
            }
        }
        limit("artifacts", artifacts.len() as u64, limits.artifacts)?;
    }
    let metadata = metadata(root)?;
    let mut executables: BTreeSet<_> = options.executables.iter().cloned().collect();
    executables.extend(launch::legacy_executables(root, files.values())?);
    for path in executables {
        paths::validate(&path)?;
        let entry = files
            .get_mut(&path)
            .with_context(|| format!("executable path is missing: {path}"))?;
        entry.mode = FileMode::Executable;
    }
    let mut manifest = Manifest {
        schema: Schema::V1,
        pack: metadata.pack,
        requires: metadata.requires,
        dependencies: metadata.dependencies,
        source: metadata.source,
        files: files.into_values().collect(),
        artifacts: artifacts.into_values().collect(),
        evidence: metadata.evidence,
    };
    let consumers = launch::validate(root, &manifest, limits)?;
    for artifact in &mut manifest.artifacts {
        let refs = consumers
            .get(artifact.id())
            .context("unused artifact")?
            .iter()
            .cloned()
            .collect();
        match artifact {
            Artifact::Native { component_refs, .. } | Artifact::Jar { component_refs, .. } => {
                *component_refs = refs
            }
        }
    }
    validate(root, &manifest, limits)?;
    Ok(manifest)
}

fn snapshot(
    source: &Path,
    root: &Path,
    path: &str,
    mode: FileMode,
    total: &mut u64,
    limits: Limits,
) -> Result<FileEntry> {
    let metadata = fs::symlink_metadata(source)?;
    ensure!(
        metadata.is_file(),
        "input is not a regular file: {}",
        source.display()
    );
    limit("file_bytes", metadata.len(), limits.file_bytes)?;
    let destination = root.join(path);
    fs::create_dir_all(destination.parent().context("missing parent")?)?;
    let mut output = File::create_new(destination)?;
    let mut open = fs::OpenOptions::new();
    open.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        open.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let mut input = open.open(source)?;
    ensure!(
        input.metadata()?.is_file(),
        "input changed to a special file"
    );
    let mut hash = Sha256::new();
    let mut size = 0;
    let mut buffer = [0; 64 * 1024];
    loop {
        let n = input.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        size += n as u64;
        *total += n as u64;
        limit("file_bytes", size, limits.file_bytes)?;
        limit("extracted_bytes", *total, limits.extracted_bytes)?;
        hash.update(&buffer[..n]);
        output.write_all(&buffer[..n])?;
    }
    Ok(FileEntry {
        path: path.into(),
        size,
        sha256: hex(&hash.finalize()),
        mode,
    })
}

pub(super) fn yaml(root: &Path, path: &str) -> Result<Value> {
    let file = File::open(root.join(path))?;
    // Metadata parsing has a tighter memory budget than opaque payload files.
    limit("metadata_bytes", file.metadata()?.len(), 1 << 20)?;
    serde_yaml_ng::from_reader(file).with_context(|| format!("invalid YAML: {path}"))
}

struct Metadata {
    pack: PackIdentity,
    requires: Requirements,
    dependencies: BTreeMap<String, String>,
    source: Option<Source>,
    evidence: Evidence,
}

fn metadata(root: &Path) -> Result<Metadata> {
    let value = yaml(root, "pack.yaml")?;
    let text = |field| {
        value
            .get(field)
            .and_then(Value::as_str)
            .with_context(|| format!("pack.yaml requires string {field}"))
    };
    let pack = PackIdentity {
        r#ref: text("ref")?.into(),
        version: text("version")?.into(),
    };
    crate::schema::RefValidator::validate_pack_ref(&pack.r#ref)?;
    semver::Version::parse(&pack.version)?;
    let mut dependencies = BTreeMap::new();
    if let Some(values) = value.get("dependencies") {
        let values: Vec<String> = serde_yaml_ng::from_value(values.clone())
            .context("dependencies must be an array of ref@constraint strings")?;
        for dependency in values {
            let (name, version) = dependency.split_once('@').unwrap_or((&dependency, "*"));
            crate::schema::RefValidator::validate_pack_ref(name)?;
            let requirement = semver::VersionReq::parse(version)?.to_string();
            ensure!(
                dependencies.insert(name.to_owned(), requirement).is_none(),
                "duplicate pack dependency: {name}"
            );
        }
    }
    let mut requires = if let Some(requires) = value.get("requires") {
        serde_yaml_ng::from_value(requires.clone())?
    } else {
        let runtimes: Vec<String> = value
            .get("runtime_deps")
            .map(|v| serde_yaml_ng::from_value(v.clone()))
            .transpose()?
            .unwrap_or_default();
        let runtimes = runtimes
            .into_iter()
            .map(|r| {
                let r = crate::runtime_detection::normalize_runtime_name(&r);
                if r.contains('.') {
                    r
                } else {
                    format!("core.{}", if r == "node" { "nodejs" } else { &r })
                }
            })
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        Requirements {
            attune: "*".into(),
            platform_catalog: ">=1".into(),
            runtimes,
        }
    };
    semver::VersionReq::parse(&requires.attune)?;
    semver::VersionReq::parse(&requires.platform_catalog)?;
    requires.runtimes.sort();
    requires.runtimes.dedup();
    for runtime in &requires.runtimes {
        crate::schema::RefValidator::validate_component_ref(runtime)?;
    }
    let source = value
        .get("source")
        .map(|v| serde_yaml_ng::from_value(v.clone()))
        .transpose()?;
    let mut evidence: Evidence = value
        .get("evidence")
        .map(|v| serde_yaml_ng::from_value(v.clone()))
        .transpose()?
        .unwrap_or_default();
    for group in [&mut evidence.sboms, &mut evidence.provenance] {
        let mut entries = group
            .drain(..)
            .map(|entry| Ok((serde_jcs::to_vec(&entry)?, entry)))
            .collect::<Result<Vec<_>>>()?;
        entries.sort_by(|a, b| a.0.cmp(&b.0));
        entries.dedup_by(|a, b| a.0 == b.0);
        group.extend(entries.into_iter().map(|(_, entry)| entry));
    }
    Ok(Metadata {
        pack,
        requires,
        dependencies,
        source,
        evidence,
    })
}

pub(super) fn sorted_unique<T: Ord>(values: &[T], label: &str) -> Result<()> {
    ensure!(
        values.windows(2).all(|w| w[0] < w[1]),
        "{label} must be sorted and unique"
    );
    Ok(())
}

pub(super) fn validate(root: &Path, manifest: &Manifest, limits: Limits) -> Result<()> {
    ensure!(
        manifest.evidence.provenance.is_empty(),
        "embedded provenance is unsupported until file-subject validation is implemented"
    );
    let metadata = metadata(root)?;
    ensure!(
        manifest.pack == metadata.pack,
        "pack identity differs from pack.yaml"
    );
    ensure!(
        manifest.dependencies == metadata.dependencies,
        "dependencies differ from pack.yaml"
    );
    ensure!(
        manifest.requires == metadata.requires,
        "requirements differ from pack.yaml"
    );
    ensure!(
        manifest.source == metadata.source && manifest.evidence == metadata.evidence,
        "source/evidence differ from pack.yaml"
    );
    let mut names = paths::Paths::default();
    names.insert(&format!("{}/{MANIFEST_PATH}", manifest.pack.r#ref))?;
    sorted_unique(
        &manifest.files.iter().map(|f| &f.path).collect::<Vec<_>>(),
        "files",
    )?;
    for file in &manifest.files {
        ensure!(
            file.path != MANIFEST_PATH,
            "manifest cannot inventory itself"
        );
        names.insert(&format!("{}/{}", manifest.pack.r#ref, file.path))?;
        ensure!(
            file.sha256.len() == 64
                && file
                    .sha256
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
            "invalid SHA-256"
        );
    }
    let consumers = launch::validate(root, manifest, limits)?;
    for artifact in &manifest.artifacts {
        let expected: Vec<_> = consumers
            .get(artifact.id())
            .context("unused artifact")?
            .iter()
            .cloned()
            .collect();
        ensure!(
            artifact.component_refs() == expected,
            "artifact consumers do not match launches: {}",
            artifact.id()
        );
    }
    let mut evidence_paths = BTreeSet::new();
    for group in [&manifest.evidence.sboms, &manifest.evidence.provenance] {
        let canonical = group
            .iter()
            .map(serde_jcs::to_vec)
            .collect::<std::result::Result<Vec<_>, _>>()?;
        sorted_unique(&canonical, "evidence")?;
        for evidence in group {
            let file = manifest
                .files
                .iter()
                .find(|f| f.path == evidence.path)
                .context("evidence file missing")?;
            ensure!(
                evidence_paths.insert(&evidence.path)
                    && file.sha256 == evidence.sha256
                    && file.mode == FileMode::Data
                    && !evidence.media_type.is_empty(),
                "invalid evidence descriptor"
            );
        }
    }
    check_source(root, manifest)?;
    Ok(())
}

fn check_source(root: &Path, manifest: &Manifest) -> Result<()> {
    let mut typed_launches = BTreeSet::new();
    for file in &manifest.files {
        let path = Path::new(&file.path);
        if !matches!(
            path.extension().and_then(|s| s.to_str()),
            Some("yaml" | "yml")
        ) {
            continue;
        }
        let parent = path.parent().and_then(|p| p.to_str()).unwrap_or("");
        if !matches!(
            parent,
            "" | "actions"
                | "sensors"
                | "triggers"
                | "runtimes"
                | "rules"
                | "queues"
                | "permission_sets"
                | "caches"
                | "policies"
                | "dashboards"
        ) {
            continue;
        }
        let value = yaml(root, &file.path)?;
        for field in ["param_schema", "out_schema", "conf_schema"] {
            if let Some(schema) = value.get(field) {
                let fields = schema
                    .as_mapping()
                    .context("schema must be a flat per-field mapping")?;
                for (name, definition) in fields {
                    ensure!(
                        name.is_string()
                            && definition.is_mapping()
                            && definition.get("type").and_then(Value::as_str).is_some(),
                        "{field} must use Attune flat schemas: {}",
                        file.path
                    );
                }
            }
        }
        if matches!(parent, "actions" | "sensors") && value.get("launch").is_some() {
            typed_launches.insert(file.path.clone());
        }
    }
    let report = crate::pack_check::check_pack(root);
    for diagnostic in report.diagnostics {
        if diagnostic.severity != crate::pack_check::PackDiagnosticSeverity::Error {
            continue;
        }
        // The legacy checker predates launch. All other source diagnostics remain fatal.
        if diagnostic.code == "metadata.missing_field"
            && diagnostic.message == "Missing required field 'entry_point'"
            && diagnostic
                .path
                .as_ref()
                .is_some_and(|p| typed_launches.contains(p))
        {
            continue;
        }
        anyhow::bail!(
            "source check {} at {:?}: {}",
            diagnostic.code,
            diagnostic.path,
            diagnostic.message
        );
    }
    Ok(())
}
