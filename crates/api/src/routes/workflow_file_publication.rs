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

    pub(super) fn failed_commit(self, error: &sqlx::Error, workflow: i64) {
        if !commit_was_rejected(error) {
            self.uncertain(workflow);
        }
        // Only an established rejection drops the owner and removes its files.
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

/// Only explicitly recognized, ordinary PostgreSQL transaction rejections
/// justify destructive cleanup. A Database ErrorResponse can also be FATAL or
/// report an unknown transaction outcome, so its Rust variant is not proof.
pub(super) fn commit_was_rejected(error: &sqlx::Error) -> bool {
    error
        .as_database_error()
        .and_then(|error| error.try_downcast_ref::<sqlx::postgres::PgDatabaseError>())
        .is_some_and(|error| known_rejection_fields(error.severity(), error.code()))
}

fn known_rejection_fields(severity: sqlx::postgres::PgSeverity, code: &str) -> bool {
    matches!(severity, sqlx::postgres::PgSeverity::Error)
        && matches!(
            code,
            // Deferred integrity constraints.
            "23502" | "23503" | "23505" | "23514" | "23P01"
            // Recognized transaction rollback causes. In particular, do NOT
            // include the whole class 40: 40003 means completion is unknown.
            | "40001" | "40P01"
            // Ordinary RAISE EXCEPTION from a deferred constraint trigger.
            | "P0001"
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::postgres::PgSeverity;

    #[test]
    fn only_known_ordinary_transaction_rejections_allow_cleanup() {
        for code in [
            "23502", "23503", "23505", "23514", "23P01", "40001", "40P01", "P0001",
        ] {
            assert!(known_rejection_fields(PgSeverity::Error, code), "{code}");
            assert!(
                !known_rejection_fields(PgSeverity::Fatal, code),
                "fatal {code}"
            );
            assert!(
                !known_rejection_fields(PgSeverity::Panic, code),
                "panic {code}"
            );
        }
        for code in [
            "08000", "08003", "08006", "08007", "08P01", "40003", "57P01", "57P02", "XX000",
            "ZZZZZ",
        ] {
            assert!(!known_rejection_fields(PgSeverity::Error, code), "{code}");
            assert!(
                !known_rejection_fields(PgSeverity::Fatal, code),
                "fatal {code}"
            );
        }
        assert!(!known_rejection_fields(PgSeverity::Warning, "23505"));
    }

    #[test]
    fn transport_uncertainty_preserves_published_files_and_private_manifest() {
        let root = tempfile::tempdir().unwrap();
        let mut files = PreparedWorkflowFiles::stage(
            root.path(),
            "pack",
            "flow",
            "tasks: []",
            "workflow_file: workflows/flow.workflow.yaml",
        )
        .unwrap();
        let stage = files.stage.as_ref().unwrap().path().to_owned();
        files.publish().unwrap();
        let error = sqlx::Error::Io(std::io::Error::new(
            std::io::ErrorKind::ConnectionReset,
            "owned synthetic acknowledgement loss",
        ));
        assert!(!commit_was_rejected(&error));
        files.failed_commit(&error, 42);
        assert!(root.path().join("pack/actions/flow.yaml").exists());
        assert!(root
            .path()
            .join("pack/actions/workflows/flow.workflow.yaml")
            .exists());
        let manifest: serde_json::Value =
            serde_json::from_slice(&std::fs::read(stage.join("uncertain-commit.json")).unwrap())
                .unwrap();
        assert_eq!(manifest["workflow_id"], 42);
        assert_eq!(manifest["files"].as_array().unwrap().len(), 2);
        root.close().unwrap();
    }

    #[test]
    fn protocol_or_pool_errors_do_not_establish_transaction_rejection() {
        for error in [
            sqlx::Error::Protocol("owned uncertain completion".into()),
            sqlx::Error::PoolTimedOut,
            sqlx::Error::PoolClosed,
        ] {
            assert!(!commit_was_rejected(&error));
        }
    }
}
