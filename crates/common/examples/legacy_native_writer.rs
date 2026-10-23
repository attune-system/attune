//! Pinned native writer for deployment-owned legacy filesystem generations.
//! No credentials, logs, admission synthesis, or cleanup authority. The owning
//! controller admits its exact operation BEFORE exec and owns its cgroup join.
use attune_common::{
    artifact_transport::{resolve_checked_path, ValidatedRelativePath},
    blob_store::{sha256, BlobBody, BlobStore, FilesystemBlobStore, ObjectKey},
};
use bytes::Bytes;
use futures::stream;
use serde::Deserialize;
use std::{
    error::Error,
    io::{BufRead, Read, Write},
    path::PathBuf,
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    root: PathBuf,
    key: String,
    mode: String,
    shared_root: PathBuf,
    staging_path: Option<String>,
    length: usize,
    pause_after_first_chunk: bool,
}

async fn run() -> Result<(), Box<dyn Error>> {
    let mut input = std::io::stdin().lock();
    let mut header = Vec::new();
    (&mut input).take(8193).read_until(b'\n', &mut header)?;
    if header.len() > 8192 || header.last() != Some(&b'\n') {
        return Err("InvalidHeader".into());
    }
    let request: Request = serde_json::from_slice(&header)?;
    if request.length > 512 * 1024
        || !request.root.is_absolute()
        || !request.shared_root.is_absolute()
    {
        return Err("InvalidRequestBound".into());
    }
    let relative = ValidatedRelativePath::new(&request.key)?;
    let mut data = Vec::new();
    (&mut input)
        .take(request.length as u64 + 1)
        .read_to_end(&mut data)?;
    if data.len() != request.length {
        return Err("InvalidBodyLength".into());
    }
    drop(input);
    // A real original member can own BOTH a shared staging path and a provider
    // object. Keep one operation/member and both physical obligations intact.
    let shared_path = if request.mode == "shared" {
        Some(resolve_checked_path(&request.root, &relative).await?)
    } else if request.mode == "blob_and_shared" {
        let stage = ValidatedRelativePath::new(
            request
                .staging_path
                .as_deref()
                .ok_or("MissingStagingPath")?,
        )?;
        Some(resolve_checked_path(&request.shared_root, &stage).await?)
    } else if request.mode == "blob" {
        None
    } else {
        return Err("UnsupportedMode".into());
    };
    let shared = if let Some(path) = shared_path {
        // Do not chmod the private root through shared-dir utilities.
        let parent = path.parent().ok_or("InvalidParent")?.to_path_buf();
        std::fs::create_dir_all(&parent)?;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)?;
        file.write_all(&data)?;
        Some((file, parent))
    } else {
        None
    };
    if request.mode == "blob" || request.mode == "blob_and_shared" {
        let expected = sha256(&data);
        // Exercise the actual old random staging + multipart + copy path, NOT
        // protected put_file_atomic. Pausing holds a real allocated upload.
        let body: BlobBody = Box::pin(stream::unfold(
            (Some(Bytes::from(data)), request.pause_after_first_chunk),
            |(data, pause)| async move {
                if let Some(data) = data {
                    Some((Ok(data), (None, pause)))
                } else if pause {
                    std::future::pending().await
                } else {
                    None
                }
            },
        ));
        FilesystemBlobStore::new(request.root)?
            .put(&ObjectKey::new(request.key)?, body, expected)
            .await?;
    } else if request.pause_after_first_chunk {
        std::future::pending::<()>().await;
    }
    if let Some((file, parent)) = shared {
        file.sync_all()?;
        std::fs::File::open(parent)?.sync_all()?;
    }
    Ok(())
}

#[tokio::main]
async fn main() {
    if run().await.is_err() {
        // No payload, backend error, path, caller-supplied text or secrets.
        std::process::exit(2);
    }
}
