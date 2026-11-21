//! Release preparation uses a manager-owned storage scope and a DB-published
//! reference. No file written inside a build is authoritative for readiness.

use super::*;
use crate::models::{Action, ParameterDelivery, ParameterFormat, Runtime, Sensor};
use std::io::Write;
use uuid::Uuid;

pub const PREPARATION_TRIGGER: &str = "runtime_preparation";
const STORAGE: &str = ".installed-runtime-preparations";
pub const MAX_TARGETS: usize = 16;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct PreparationScope {
    storage: Uuid,
    root: PathBuf,
    revision: String,
    platform: String,
}

impl PreparationScope {
    pub fn open(root: &Path, environment: &ChildProcessEnvironment) -> Result<Option<Self>> {
        let Some(revision) = environment.preparation_revision() else {
            return Ok(None);
        };
        let root = root.join(STORAGE);
        std::fs::create_dir_all(&root)?;
        let root = root.canonicalize()?;
        let marker = root.join("storage-id");
        if !marker.try_exists()? {
            let mut file = tempfile::NamedTempFile::new_in(&root)?;
            write!(file, "{}", Uuid::new_v4())?;
            file.as_file().sync_all()?;
            if let Err(error) = file.persist_noclobber(&marker) {
                if error.error.kind() != std::io::ErrorKind::AlreadyExists {
                    return Err(error.error.into());
                }
            }
            sync_dir(&root)?;
        }
        let storage = std::fs::read_to_string(&marker)?
            .parse()
            .map_err(|_| invalid("Invalid runtime preparation storage identity"))?;
        Ok(Some(Self {
            storage,
            root,
            revision: revision.into(),
            platform: format!("{}/{}", std::env::consts::OS, std::env::consts::ARCH),
        }))
    }

    pub fn for_worker(worker: &crate::models::Worker) -> Option<Self> {
        serde_json::from_value(
            worker
                .capabilities
                .as_ref()?
                .get("runtime_preparation_scope")?
                .clone(),
        )
        .ok()
    }

    pub fn attach_capability(
        capabilities: &mut std::collections::HashMap<String, serde_json::Value>,
        root: &Path,
        environment: &ChildProcessEnvironment,
        runtime_filter: Option<&[String]>,
    ) -> Result<()> {
        capabilities.remove("runtime_preparation_scope");
        capabilities.remove("runtime_preparation_filter");
        if let Some(scope) = Self::open(root, environment)? {
            capabilities.insert("runtime_preparation_scope".into(), serde_json::json!(scope));
            if let Some(filter) = runtime_filter {
                capabilities.insert(
                    "runtime_preparation_filter".into(),
                    serde_json::json!(filter),
                );
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PreparationTarget {
    pub kind: PreparationWorkload,
    pub reference: String,
    pub source: PathBuf,
    pub runtime_ref: String,
    pub runtime_names: Vec<String>,
    pub recipe_digest: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PreparationWorkload {
    Action,
    Sensor,
}

impl PreparationTarget {
    pub fn for_action(
        action: &Action,
        runtime: &Runtime,
        recipe: &RuntimeRecipe,
    ) -> Result<Option<Self>> {
        if !recipe.prepare_at_install || action.is_adhoc || action.workflow_def.is_some() {
            return Ok(None);
        }
        if action.launch.is_some()
            || action.runtime_version_constraint.is_some()
            || action.parameter_delivery != ParameterDelivery::Stdin
            || action.parameter_format != ParameterFormat::Json
        {
            return Err(invalid(
                "Install-time preparation requires an unconstrained file-backed recipe action using stdin JSON",
            ));
        }
        let entrypoint = action
            .entrypoint
            .as_deref()
            .ok_or_else(|| invalid("Missing recipe action entrypoint"))?;
        Self::new(
            PreparationWorkload::Action,
            &action.r#ref,
            entrypoint,
            runtime,
            recipe,
        )
        .map(Some)
    }

    pub fn for_sensor(
        sensor: &Sensor,
        runtime: &Runtime,
        recipe: &RuntimeRecipe,
    ) -> Result<Option<Self>> {
        if !recipe.prepare_at_install {
            return Ok(None);
        }
        if sensor.runtime_version_constraint.is_some() {
            return Err(invalid(
                "Install-time preparation requires an unconstrained file-backed managed sensor",
            ));
        }
        Self::new(
            PreparationWorkload::Sensor,
            &sensor.r#ref,
            &sensor.entrypoint,
            runtime,
            recipe,
        )
        .map(Some)
    }

    fn new(
        kind: PreparationWorkload,
        reference: &str,
        entrypoint: &str,
        runtime: &Runtime,
        recipe: &RuntimeRecipe,
    ) -> Result<Self> {
        if entrypoint.contains("://") {
            return Err(invalid(
                "Install-time preparation requires a pack file entrypoint",
            ));
        }
        let directory = match kind {
            PreparationWorkload::Action => "actions",
            PreparationWorkload::Sensor => "sensors",
        };
        let mut runtime_names = runtime.aliases.clone();
        runtime_names.push(runtime.name.clone());
        runtime_names.sort();
        runtime_names.dedup();
        Ok(Self {
            kind,
            reference: reference.into(),
            source: Path::new(directory).join(relative_path(entrypoint)?),
            runtime_ref: runtime.r#ref.clone(),
            runtime_names,
            recipe_digest: recipe.digest()?,
        })
    }
}

/// This reference is persisted only by the worker finishing a fenced install
/// attempt and is bound to a release by the API's activation transaction.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstalledPreparation {
    pub target: PreparationTarget,
    pub release_digest: String,
    pub scope: PreparationScope,
    directory: Uuid,
    generation: String,
    seal: Uuid,
    retained: (String, String),
    tools: BTreeMap<String, VerifiedTool>,
    tool_hashes: BTreeMap<String, String>,
}

impl RuntimeRecipe {
    pub async fn prepare_for_install(
        &self,
        candidate: &Path,
        target: PreparationTarget,
        release_digest: &str,
        scope: &PreparationScope,
        environment: &ChildProcessEnvironment,
    ) -> Result<InstalledPreparation> {
        if !self.prepare_at_install
            || target.recipe_digest != self.require_admission(environment)?
        {
            return Err(invalid("Install build does not match the admitted recipe"));
        }
        if environment.preparation_revision() != Some(&scope.revision) {
            return Err(invalid(
                "Install build requires the admitted Bubblewrap preparation scope",
            ));
        }
        let execution_environment = environment.for_install_preparation();
        let directory = Uuid::new_v4();
        let root = scope.root.join(directory.to_string());
        let pack = root.join("pack");
        let mut directory_builder = std::fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            directory_builder.mode(0o700);
        }
        directory_builder.create(&root)?;
        copy_workspace(candidate, &pack)?;
        let build = self
            .prepare_build(
                &pack,
                RuntimeSource::File(&pack.join(&target.source)),
                &root,
                &execution_environment,
            )
            .await?;
        // Validate the launch while the complete tree is still private, using
        // the same expansion and path checks as the consumer.
        let invocation = self.bind_private_invocation(&build, &root, &execution_environment)?;
        std::fs::remove_dir_all(invocation.cleanup_root)?;
        for disposable in ["home", "scratch"] {
            std::fs::remove_dir_all(build.generation.join(disposable))?;
        }
        let generation = build
            .generation
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| invalid("Invalid runtime generation name"))?
            .to_owned();
        let tool_hashes = hash_tools(&build.tools)?;
        let seal = Uuid::new_v4();
        let retained = fingerprint_preparation(&pack, &build.generation, true).await?;
        let mut marker = std::fs::File::create_new(root.join("ready"))?;
        write!(marker, "{seal}")?;
        marker.sync_all()?;
        sync_dir(&root)?;
        sync_dir(&scope.root)?;
        Ok(InstalledPreparation {
            target,
            release_digest: release_digest.into(),
            scope: scope.clone(),
            directory,
            generation,
            seal,
            retained,
            tools: build.tools,
            tool_hashes,
        })
    }

    pub async fn open_installed(
        &self,
        prepared: &InstalledPreparation,
        runtime_root: &Path,
        environment: &ChildProcessEnvironment,
    ) -> Result<PreparedTarget> {
        if !self.prepare_at_install
            || prepared.target.recipe_digest != self.require_admission(environment)?
            || PreparationScope::open(runtime_root, environment)?.as_ref() != Some(&prepared.scope)
        {
            return Err(invalid(
                "Installed runtime preparation is outside this manager's admitted scope",
            ));
        }
        let root = prepared.scope.root.join(prepared.directory.to_string());
        if std::fs::read_to_string(root.join("ready"))? != prepared.seal.to_string() {
            return Err(invalid("Installed runtime generation is incomplete"));
        }
        let generation = root.join(relative_path(&prepared.generation)?);
        let pack_dir = root.join("pack");
        if root.canonicalize()? != root
            || pack_dir.canonicalize()? != pack_dir
            || generation.canonicalize()? != generation
            || !generation.is_dir()
        {
            return Err(invalid("Invalid installed runtime generation paths"));
        }
        if fingerprint_preparation(&pack_dir, &generation, false).await? != prepared.retained {
            return Err(invalid(
                "Installed runtime files differ from the prepared generation",
            ));
        }
        let source_file = if self.workspace {
            generation.join("workspace").join(&prepared.target.source)
        } else {
            pack_dir.join(&prepared.target.source)
        };
        if !source_file.is_file() || !source_file.canonicalize()?.starts_with(&root) {
            return Err(invalid("Installed runtime source is unavailable"));
        }
        let execution_environment = environment.for_install_preparation();
        let tools = self.discover(&execution_environment).await?;
        if tools != prepared.tools || hash_tools(&tools)? != prepared.tool_hashes {
            return Err(invalid(
                "Installed runtime toolchain changed since preparation",
            ));
        }
        let primary_version = self
            .tools
            .iter()
            .find(|tool| tool.primary)
            .and_then(|tool| tools.get(&tool.name))
            .ok_or_else(|| invalid("Missing verified primary runtime tool"))?
            .version
            .clone();
        let mut variables = BTreeMap::from([
            ("pack_dir".into(), pack_dir.display().to_string()),
            (
                "workspace_dir".into(),
                generation.join("workspace").display().to_string(),
            ),
            ("env_dir".into(), generation.display().to_string()),
            (
                "output_dir".into(),
                generation.join("output").display().to_string(),
            ),
            ("source_file".into(), source_file.display().to_string()),
        ]);
        for (name, tool) in &tools {
            variables.insert(
                format!("tool_{name}"),
                tool.executable.display().to_string(),
            );
        }
        let build = PreparedBuild {
            generation,
            pack_dir,
            variables,
            tools,
            recipe_digest: prepared.target.recipe_digest.clone(),
            primary_version,
        };
        let invocation_root = runtime_root.join("lifecycle-generations");
        std::fs::create_dir_all(&invocation_root)?;
        let mut target =
            self.bind_private_invocation(&build, &invocation_root, &execution_environment)?;
        target.owns_generation = false;
        Ok(target)
    }
}

fn hash_tools(tools: &BTreeMap<String, VerifiedTool>) -> Result<BTreeMap<String, String>> {
    tools
        .iter()
        .map(|(name, tool)| {
            let hash = crate::pack_registry::calculate_file_checksum(&tool.identity)
                .map_err(|_| invalid("Cannot hash the reviewed runtime tool"))?;
            Ok((name.clone(), hash))
        })
        .collect()
}

fn sync_dir(path: &Path) -> Result<()> {
    std::fs::File::open(path)?.sync_all()?;
    Ok(())
}

async fn fingerprint_preparation(
    pack: &Path,
    build: &Path,
    sync: bool,
) -> Result<(String, String)> {
    let pack = pack.to_path_buf();
    let build = build.to_path_buf();
    tokio::task::spawn_blocking(move || {
        Ok((
            fingerprint_tree(&pack, &build, sync)?,
            fingerprint_tree(&build, &pack, sync)?,
        ))
    })
    .await
    .map_err(|_| invalid("Runtime preparation verification did not complete"))?
}

// Bind complete file contents, permissions, relative paths and link identities.
// Retained links may target this preparation or its revisioned system image.
fn fingerprint_tree(root: &Path, other_root: &Path, sync: bool) -> Result<String> {
    if root.canonicalize()? != root {
        return Err(invalid("Retained runtime root is an alias"));
    }
    let mut directories = vec![root.to_path_buf()];
    let mut bytes = 0u64;
    let mut fingerprint = Sha256::new();
    for (index, entry) in walkdir::WalkDir::new(root)
        .min_depth(1)
        .sort_by_file_name()
        .into_iter()
        .enumerate()
    {
        if index >= MAX_WORKSPACE_FILES {
            return Err(invalid("Retained runtime tree exceeds the file limit"));
        }
        let entry = entry.map_err(|_| invalid("Cannot enumerate retained runtime files"))?;
        let path = entry.path();
        let relative = path
            .strip_prefix(root)
            .map_err(|_| invalid("Invalid retained runtime path"))?;
        let kind = entry.file_type();
        let metadata = std::fs::symlink_metadata(path)?;
        #[cfg(unix)]
        let mode = std::os::unix::fs::PermissionsExt::mode(&metadata.permissions());
        #[cfg(not(unix))]
        let mode = 0;
        let (tag, value) = if kind.is_dir() {
            if sync {
                directories.push(path.to_path_buf());
            }
            ("directory", String::new())
        } else if kind.is_symlink() {
            let target = path.canonicalize()?;
            let in_image = ["/usr", "/bin", "/lib", "/lib64"]
                .iter()
                .any(|prefix| target.starts_with(prefix));
            if !target.starts_with(root) && !target.starts_with(other_root) && !in_image {
                return Err(invalid(
                    "Retained runtime symlink escapes the prepared inputs",
                ));
            }
            let link = std::fs::read_link(path)?;
            (
                "link",
                link.to_str()
                    .ok_or_else(|| invalid("Invalid retained runtime link"))?
                    .to_owned(),
            )
        } else if kind.is_file() {
            bytes = bytes.saturating_add(metadata.len());
            if bytes > MAX_WORKSPACE_BYTES {
                return Err(invalid("Retained runtime tree exceeds the size limit"));
            }
            let hash = crate::pack_registry::calculate_file_checksum(path)
                .map_err(|_| invalid("Cannot hash a retained runtime file"))?;
            if sync {
                std::fs::File::open(path)?.sync_all()?;
            }
            ("file", hash)
        } else {
            return Err(invalid("Retained runtime tree contains a special file"));
        };
        let record = serde_json::to_vec(&(relative, tag, mode, value))
            .map_err(|_| invalid("Cannot encode a retained runtime file"))?;
        fingerprint.update((record.len() as u64).to_be_bytes());
        fingerprint.update(record);
    }
    if sync {
        for directory in directories.into_iter().rev() {
            sync_dir(&directory)?;
        }
    }
    Ok(fingerprint
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}
