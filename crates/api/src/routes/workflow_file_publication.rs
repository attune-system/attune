//! Owned staging for newly created workflow files. Existing files are never replaced.
use crate::middleware::{ApiError, ApiResult};
use std::path::{Path, PathBuf};

pub(super) struct PreparedWorkflowFiles {
    stage: Option<tempfile::TempDir>,
    files: Vec<(PathBuf, PathBuf)>,
    published: Vec<PathBuf>,
    directories: Vec<PathBuf>,
    retained: bool,
}

impl PreparedWorkflowFiles {
    pub(super) fn stage(
        base: &Path,
        pack: &str,
        name: &str,
        graph: &str,
        action: &str,
    ) -> ApiResult<Self> {
        let mut owned = Self {
            stage: None,
            files: Vec::new(),
            published: Vec::new(),
            directories: Vec::new(),
            retained: false,
        };
        let pack_dir = base.join(pack);
        let actions = pack_dir.join("actions");
        let workflows = actions.join("workflows");
        for path in [&pack_dir, &actions, &workflows] {
            match std::fs::create_dir(path) {
                Ok(()) => owned.directories.push(path.clone()),
                Err(error)
                    if error.kind() == std::io::ErrorKind::AlreadyExists && path.is_dir() => {}
                Err(error) => {
                    return Err(ApiError::InternalServerError(format!(
                        "Cannot prepare workflow directory: {error}"
                    )))
                }
            }
        }
        let stage = tempfile::Builder::new()
            .prefix(".workflow-publication-")
            .tempdir_in(&actions)
            .map_err(|error| {
                ApiError::InternalServerError(format!("Cannot stage workflow files: {error}"))
            })?;
        for (filename, destination, content) in [
            (
                "graph",
                workflows.join(format!("{name}.workflow.yaml")),
                graph,
            ),
            ("action", actions.join(format!("{name}.yaml")), action),
        ] {
            let source = stage.path().join(filename);
            std::fs::write(&source, content).map_err(|error| {
                ApiError::InternalServerError(format!("Cannot stage workflow content: {error}"))
            })?;
            owned.files.push((source, destination));
        }
        owned.stage = Some(stage);
        Ok(owned)
    }

    pub(super) fn publish(&mut self) -> ApiResult<()> {
        // No await between linking a file and recording its cleanup ownership.
        for (source, destination) in &self.files {
            std::fs::hard_link(source, destination).map_err(|error| {
                if error.kind() == std::io::ErrorKind::AlreadyExists {
                    ApiError::Conflict("Workflow file already exists".into())
                } else {
                    ApiError::InternalServerError(format!("Cannot publish workflow files: {error}"))
                }
            })?;
            self.published.push(destination.clone());
        }
        Ok(())
    }

    pub(super) fn committed(mut self) {
        self.retained = true;
    }

    pub(super) fn uncertain(mut self, workflow: i64) {
        // Transport failure during COMMIT cannot prove rollback. Keep the files
        // and a private owned manifest for reconciliation instead of deleting a
        // possibly committed workflow's files.
        if let Some(stage) = self.stage.take() {
            let manifest =
                serde_json::json!({"workflow_id":workflow,"files":self.published.clone()});
            if let Err(error) = std::fs::write(
                stage.path().join("uncertain-commit.json"),
                manifest.to_string(),
            ) {
                tracing::error!(%error, workflow, "Could not record uncertain workflow commit manifest");
            }
            let path = stage.keep();
            tracing::error!(workflow, path=%path.display(), "Workflow commit outcome is uncertain; retained owned publication for reconciliation");
        }
        self.retained = true;
    }
}

impl Drop for PreparedWorkflowFiles {
    fn drop(&mut self) {
        if !self.retained {
            for path in self.published.iter().rev() {
                if let Err(error) = std::fs::remove_file(path) {
                    tracing::error!(%error,path=%path.display(),"Failed to clean owned workflow file");
                }
            }
        }
        // Remove the owned stage before trying to remove newly created parents.
        if let Some(stage) = self.stage.take() {
            if let Err(error) = stage.close() {
                tracing::error!(%error,"Failed to clean workflow staging directory");
            }
        }
        if !self.retained {
            for path in self.directories.iter().rev() {
                if let Err(error) = std::fs::remove_dir(path) {
                    // Another authorized component may now use an originally
                    // created parent; never recursively remove it.
                    if error.kind() != std::io::ErrorKind::DirectoryNotEmpty {
                        tracing::error!(%error,path=%path.display(),"Failed to clean owned workflow directory");
                    }
                }
            }
        }
    }
}
