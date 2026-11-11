//! Explicit execution disclosure and private, file-only MCP export.

use crate::client::{ApiClient, ApiRequestError};
use anyhow::{anyhow, Result};
use serde::Serialize;
use serde_json::Value;
use std::{
    fs::{File, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
};

pub fn execution_detail_path(id: i64, include_secret_values: bool) -> String {
    if include_secret_values {
        format!("/executions/{id}?include_secret_values=true")
    } else {
        format!("/executions/{id}")
    }
}

/// Drop remote bodies, parser text and source chains before disclosure errors
/// can enter tool results. Keep only the HTTP status for normal reauthentication.
pub fn safe_disclosure_error(error: anyhow::Error) -> anyhow::Error {
    if let Some(request) = error.downcast_ref::<ApiRequestError>() {
        return ApiRequestError::from_response(
            request.status,
            None,
            r#"{"error":"Execution disclosure request failed"}"#,
        )
        .into();
    }
    anyhow!("Execution disclosure or private file export failed")
}

#[derive(Debug, Serialize)]
pub struct ExecutionExportReceipt {
    pub output_file: PathBuf,
    pub bytes_written: u64,
    pub status: &'static str,
}

fn require_private_export_support() -> Result<()> {
    if cfg!(unix) {
        Ok(())
    } else {
        Err(anyhow!(
            "Private execution file export is unsupported on this platform"
        ))
    }
}

fn output_path(requested: &Path) -> Result<PathBuf> {
    let name = requested
        .file_name()
        .filter(|name| !name.is_empty())
        .ok_or_else(|| anyhow!("An explicit output file path is required"))?;
    let parent = requested
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let parent = parent
        .canonicalize()
        .map_err(|_| anyhow!("Output directory must already exist"))?;
    if !parent.is_dir() {
        return Err(anyhow!("Output parent must be a directory"));
    }
    let output = parent.join(name);
    match output.symlink_metadata() {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(output),
        _ => Err(anyhow!("Output file already exists or is unavailable")),
    }
}

fn remove_owned_file(path: &Path, opened: &std::fs::Metadata) -> io::Result<()> {
    let current = path.symlink_metadata()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if current.dev() != opened.dev() || current.ino() != opened.ino() || !current.is_file() {
            return Err(io::Error::other("Output file ownership changed"));
        }
    }
    #[cfg(not(unix))]
    {
        // Without a portable file identity, retain the file rather than risk
        // removing a replacement supplied by another process.
        let _ = (current, opened);
        return Err(io::Error::other(
            "Cannot prove output file ownership for cleanup",
        ));
    }
    #[cfg(unix)]
    std::fs::remove_file(path)
}

fn write_owned_export(
    path: &Path,
    bytes: &[u8],
    write: impl FnOnce(&mut File, &[u8]) -> io::Result<()>,
) -> Result<()> {
    require_private_export_support()?;
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path).map_err(|_| {
        anyhow!("Cannot create private output file; existing files are never overwritten")
    })?;
    let identity = file
        .metadata()
        .map_err(|_| anyhow!("Cannot inspect new output file"))?;
    if write(&mut file, bytes)
        .and_then(|()| file.sync_all())
        .is_err()
    {
        drop(file);
        if remove_owned_file(path, &identity).is_err() {
            return Err(anyhow!(
                "Export write failed; partial output could not be removed"
            ));
        }
        return Err(anyhow!("Export write failed; partial output removed"));
    }
    Ok(())
}

/// Fetch plaintext only after validating an explicit target; never return the
/// fetched execution or a content preview to an MCP caller.
pub async fn export_execution_to_file(
    client: &mut ApiClient,
    id: i64,
    requested: &Path,
) -> Result<ExecutionExportReceipt> {
    require_private_export_support()?;
    if id <= 0 {
        return Err(anyhow!("Execution ID must be positive"));
    }
    if requested.as_os_str().is_empty() {
        return Err(anyhow!("An explicit output file path is required"));
    }
    let path = output_path(requested)?;
    let execution: Value = client
        .get(&execution_detail_path(id, true))
        .await
        .map_err(safe_disclosure_error)?;
    let mut bytes = serde_json::to_vec_pretty(&execution)
        .map_err(|_| anyhow!("Cannot encode execution export"))?;
    bytes.push(b'\n');
    let bytes_written = bytes.len() as u64;
    let write_path = path.clone();
    tokio::task::spawn_blocking(move || {
        write_owned_export(&write_path, &bytes, |file, body| file.write_all(body))
    })
    .await
    .map_err(|_| anyhow!("Execution export writer failed"))??;
    Ok(ExecutionExportReceipt {
        output_file: path,
        bytes_written,
        status: "written",
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    use serde_json::json;
    use wiremock::MockServer;
    #[cfg(unix)]
    use wiremock::{
        matchers::{method, path, query_param},
        Mock, ResponseTemplate,
    };

    #[test]
    fn query_requires_explicit_opt_in() {
        assert_eq!(execution_detail_path(42, false), "/executions/42");
        assert_eq!(
            execution_detail_path(42, true),
            "/executions/42?include_secret_values=true"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn authorized_plaintext_is_written_privately_and_only_receipt_is_returned() {
        let server = MockServer::start().await;
        let execution = json!({"id":42,"config":{"password":"fixture-password"},"result":{"stdout":"sanitized","data":{"restricted":{"$attune_secret":true,"redacted":true}}},"revealed_output_channels":{"stdout":"fixture-private-original"}});
        Mock::given(method("GET"))
            .and(path("/api/v1/executions/42"))
            .and(query_param("include_secret_values", "true"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"data":execution})))
            .expect(1)
            .mount(&server)
            .await;
        let root = tempfile::tempdir().unwrap();
        let target = root.path().join("execution.json");
        let receipt =
            export_execution_to_file(&mut ApiClient::new(server.uri(), None), 42, &target)
                .await
                .unwrap();
        let encoded = serde_json::to_string(&receipt).unwrap();
        assert!(
            !encoded.contains("fixture-password") && !encoded.contains("fixture-private-original")
        );
        assert_eq!(
            serde_json::from_slice::<Value>(&std::fs::read(&target).unwrap()).unwrap(),
            execution
        );
        assert!(receipt.output_file.is_absolute());
        assert_eq!(
            receipt.bytes_written,
            std::fs::metadata(&target).unwrap().len()
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&target).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn denied_request_creates_no_file_and_does_not_reflect_remote_error() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(403).set_body_json(
                json!({"error":"fixture-private-error","details":{"password":"fixture-password"}}),
            ))
            .expect(1)
            .mount(&server)
            .await;
        let root = tempfile::tempdir().unwrap();
        let target = root.path().join("denied.json");
        let error = export_execution_to_file(&mut ApiClient::new(server.uri(), None), 42, &target)
            .await
            .unwrap_err();
        assert!(!target.exists());
        assert!(!format!("{error:#}").contains("fixture-private"));
        assert!(!error.to_string().contains("fixture-password"));
    }

    #[tokio::test]
    async fn invalid_or_occupied_targets_do_not_fetch_plaintext() {
        let server = MockServer::start().await;
        let root = tempfile::tempdir().unwrap();
        let target = root.path().join("occupied.json");
        std::fs::write(&target, b"owned elsewhere").unwrap();
        for path in [
            PathBuf::new(),
            target.clone(),
            root.path().join("missing").join("file.json"),
        ] {
            assert!(
                export_execution_to_file(&mut ApiClient::new(server.uri(), None), 42, &path)
                    .await
                    .is_err()
            );
        }
        assert!(server.received_requests().await.unwrap().is_empty());
        assert_eq!(std::fs::read(target).unwrap(), b"owned elsewhere");
    }

    #[cfg(unix)]
    #[test]
    fn partial_write_removes_only_the_new_owned_file() {
        let root = tempfile::tempdir().unwrap();
        let target = root.path().join("partial.json");
        assert!(
            write_owned_export(&target, b"private-content", |file, body| {
                file.write_all(&body[..3])?;
                Err(io::Error::other("fixture failure"))
            })
            .is_err()
        );
        assert!(!target.exists());
        let target = root.path().join("existing.json");
        std::fs::write(&target, b"keep").unwrap();
        assert!(write_owned_export(&target, b"private-content", |_, _| Ok(())).is_err());
        assert_eq!(std::fs::read(target).unwrap(), b"keep");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn final_symlink_is_rejected_before_disclosure() {
        let server = MockServer::start().await;
        let root = tempfile::tempdir().unwrap();
        let target = root.path().join("symlink.json");
        std::os::unix::fs::symlink(root.path().join("missing"), &target).unwrap();
        assert!(
            export_execution_to_file(&mut ApiClient::new(server.uri(), None), 42, &target)
                .await
                .is_err()
        );
        assert!(server.received_requests().await.unwrap().is_empty());
    }
    #[cfg(not(unix))]
    #[tokio::test]
    async fn unsupported_platform_rejects_before_fetch_or_file_creation() {
        let server = MockServer::start().await;
        let root = tempfile::tempdir().unwrap();
        let target = root.path().join("execution.json");
        let error = export_execution_to_file(&mut ApiClient::new(server.uri(), None), 42, &target)
            .await
            .unwrap_err();
        assert_eq!(
            error.to_string(),
            "Private execution file export is unsupported on this platform"
        );
        assert!(!target.exists());
        assert!(server.received_requests().await.unwrap().is_empty());
        assert!(
            write_owned_export(&target, b"private", |file, body| file.write_all(body)).is_err()
        );
        assert!(!target.exists());
    }
}
