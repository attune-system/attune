//! Fresh authenticated prospective enrollment from the actual owning controller.
//! This proves original generation identity, NOT writer/provider settlement.
use crate::{config::BlobStorageConfig, value_schema::digest, Error, Result};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
#[cfg(unix)]
use std::{io::Read, os::unix::fs::MetadataExt};
use std::{path::PathBuf, time::Duration};
use uuid::Uuid;

#[derive(Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum NativeMode {
    Blob,
    Shared,
    BlobAndShared,
}
impl NativeMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Blob => "blob",
            Self::Shared => "shared",
            Self::BlobAndShared => "blob_and_shared",
        }
    }
    pub fn blob(self) -> bool {
        self != Self::Shared
    }
    pub fn shared(self) -> bool {
        self != Self::Blob
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeControllerAccess {
    pub endpoint: String,
    pub issuer: String,
    pub audience: String,
    pub verification_key_file: PathBuf,
    pub client_identity_file: PathBuf,
    pub ca_certificate_file: PathBuf,
    pub storage: BlobStorageConfig,
    pub artifacts_dir: PathBuf,
}
impl NativeControllerAccess {
    pub fn scope(&self) -> Result<Value> {
        Ok(
            json!({"backend":digest(&json!({"storage":self.storage,"artifacts_dir":self.artifacts_dir})).map_err(|_| denied())?,"staging_prefix":".attune-upload/"}),
        )
    }
}

pub struct VerifiedNativeOrigin {
    pub(crate) generation: Uuid,
    pub(crate) mode: NativeMode,
    pub(crate) operation: Uuid,
    pub(crate) native_instance: Uuid,
    pub(crate) worker_instance: Uuid,
    pub(crate) attempt: Uuid,
    pub(crate) scope: Value,
    pub(crate) evidence: Value,
    pub(crate) identity_digest: String,
    deadline: i64,
}

fn denied() -> Error {
    Error::PermissionDenied("Original native generation proof unavailable".into())
}

#[cfg(unix)]
fn management_file(path: &PathBuf, private: bool) -> Result<Vec<u8>> {
    use std::os::unix::fs::OpenOptionsExt;
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
        .map_err(|_| denied())?;
    let info = file.metadata().map_err(|_| denied())?;
    if !info.is_file()
        || info.uid() != 0
        || info.mode() & 0o022 != 0
        || (private && info.mode() & 0o007 != 0)
        || info.len() > 65536
    {
        return Err(denied());
    }
    let mut bytes = Vec::new();
    file.take(65537)
        .read_to_end(&mut bytes)
        .map_err(|_| denied())?;
    if bytes.len() > 65536 {
        return Err(denied());
    }
    Ok(bytes)
}

#[cfg(not(unix))]
fn management_file(_path: &PathBuf, _private: bool) -> Result<Vec<u8>> {
    // Controller credentials require Unix ownership/mode and no-follow checks.
    // Do not accept an unchecked file on platforms without that contract.
    Err(denied())
}

impl VerifiedNativeOrigin {
    pub fn ensure_fresh(&self) -> Result<()> {
        if chrono::Utc::now().timestamp() >= self.deadline {
            Err(denied())
        } else {
            Ok(())
        }
    }

    pub async fn acquire(
        access: &NativeControllerAccess,
        run: &str,
        project: &str,
        generation: Uuid,
        mode: NativeMode,
    ) -> Result<Self> {
        if !matches!(access.storage, BlobStorageConfig::Filesystem { .. }) || generation.is_nil() {
            return Err(denied());
        }
        let mut url = reqwest::Url::parse(&access.endpoint).map_err(|_| denied())?;
        if url.scheme() != "https"
            || url.host_str() != Some("127.0.0.1")
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
            || !matches!(url.path(), "" | "/")
        {
            return Err(denied());
        }
        url.set_path(&format!("/native-generations/{generation}/prepare-origin"));
        let nonce = Uuid::new_v4();
        let request = json!({"nonce":nonce,"generation":generation,"run_id":run,"project":project,"mode":mode});
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(3))
            .timeout(Duration::from_secs(10))
            .identity(
                reqwest::Identity::from_pem(&management_file(&access.client_identity_file, true)?)
                    .map_err(|_| denied())?,
            )
            .add_root_certificate(
                reqwest::Certificate::from_pem(&management_file(
                    &access.ca_certificate_file,
                    false,
                )?)
                .map_err(|_| denied())?,
            )
            .build()
            .map_err(|_| denied())?;
        let response = client
            .post(url)
            .header("content-type", "application/json")
            .body(serde_json::to_vec(&request)?)
            .send()
            .await
            .map_err(|_| denied())?;
        if !response.status().is_success() || response.content_length().is_some_and(|n| n > 65536) {
            return Err(denied());
        }
        let mut stream = response.bytes_stream();
        let mut body = Vec::new();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|_| denied())?;
            if body.len() + chunk.len() > 65536 {
                return Err(denied());
            }
            body.extend_from_slice(&chunk);
        }
        let text = std::str::from_utf8(&body).map_err(|_| denied())?;
        let parts: Vec<_> = text.split('.').collect();
        if parts.len() != 3 {
            return Err(denied());
        }
        let header: Value =
            serde_json::from_slice(&URL_SAFE_NO_PAD.decode(parts[0]).map_err(|_| denied())?)?;
        if header.get("alg").and_then(Value::as_str) != Some("RS256")
            || header.get("crit").is_some()
            || header.get("b64").is_some()
        {
            return Err(denied());
        }
        let key = jsonwebtoken::DecodingKey::from_rsa_pem(&management_file(
            &access.verification_key_file,
            false,
        )?)
        .map_err(|_| denied())?;
        aws_lc_rs::signature::UnparsedPublicKey::new(
            &aws_lc_rs::signature::RSA_PKCS1_2048_8192_SHA256,
            key.as_bytes(),
        )
        .verify(
            format!("{}.{}", parts[0], parts[1]).as_bytes(),
            &URL_SAFE_NO_PAD.decode(parts[2]).map_err(|_| denied())?,
        )
        .map_err(|_| denied())?;
        let claims: Value =
            serde_json::from_slice(&URL_SAFE_NO_PAD.decode(parts[1]).map_err(|_| denied())?)?;
        let now = chrono::Utc::now().timestamp();
        let issued = claims["iat"].as_i64().ok_or_else(denied)?;
        let deadline = claims["exp"].as_i64().ok_or_else(denied)?;
        let scope = access.scope()?;
        if claims["purpose"] != "controlled-native-origin"
            || claims["iss"] != access.issuer
            || claims["aud"] != access.audience
            || claims["nonce"] != json!(nonce)
            || claims["generation"] != json!(generation)
            || claims["scope"] != scope
            || claims["request_digest"] != digest(&request).map_err(|_| denied())?
            || issued > now
            || issued < now - 60
            || deadline <= now
            || deadline - issued > 60
        {
            return Err(denied());
        }
        let data = &claims["data"];
        let original = &data["enrollment"];
        if data["binding"]["generation"] != json!(generation)
            || data["binding"]["scope"] != scope
            || original["run_id"] != run
            || original["project"] != project
            || original["mode"] != json!(mode)
            || data["binding"]["service_uid"] != 61124
            || data["binding"]["service_gid"] != 61124
        {
            return Err(denied());
        }
        let uuid = |value: &Value| -> Result<Uuid> {
            let id = Uuid::parse_str(value.as_str().ok_or_else(denied)?).map_err(|_| denied())?;
            if id.is_nil() {
                return Err(denied());
            }
            Ok(id)
        };
        Ok(Self {
            generation,
            mode,
            operation: uuid(&original["operation"])?,
            native_instance: uuid(&original["writer"]["native_instance"])?,
            worker_instance: uuid(&original["writer"]["worker_instance"])?,
            attempt: uuid(&original["writer"]["execution_attempt"])?,
            scope,
            identity_digest: digest(data).map_err(|_| denied())?,
            evidence: json!({"original":data,"attestation":text,"verification_key_sha256":URL_SAFE_NO_PAD.encode(crate::blob_store::sha256(key.as_bytes()))}),
            deadline,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_management_file_is_denied() {
        let root = tempfile::tempdir().unwrap();
        for private in [false, true] {
            assert!(matches!(
                management_file(&root.path().join("missing.pem"), private),
                Err(Error::PermissionDenied(_))
            ));
        }
        root.close().unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn management_files_retain_unix_security_checks() {
        use std::os::unix::fs::{symlink, PermissionsExt};

        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("identity.pem");
        std::fs::write(&path, b"controller identity").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        // The test works for root and unprivileged runners without chown/sudo.
        if path.metadata().unwrap().uid() == 0 {
            assert_eq!(
                management_file(&path, true).unwrap(),
                b"controller identity"
            );
        } else {
            assert!(matches!(
                management_file(&path, true),
                Err(Error::PermissionDenied(_))
            ));
        }

        let link = root.path().join("link.pem");
        symlink(&path, &link).unwrap();
        assert!(matches!(
            management_file(&link, true),
            Err(Error::PermissionDenied(_))
        ));
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o622)).unwrap();
        assert!(matches!(
            management_file(&path, false),
            Err(Error::PermissionDenied(_))
        ));
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o604)).unwrap();
        assert!(matches!(
            management_file(&path, true),
            Err(Error::PermissionDenied(_))
        ));
        root.close().unwrap();
    }

    #[cfg(not(unix))]
    #[test]
    fn readable_management_files_are_denied_without_unix_security_checks() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("identity.pem");
        std::fs::write(&path, b"controller identity").unwrap();
        for private in [false, true] {
            assert!(matches!(
                management_file(&path, private),
                Err(Error::PermissionDenied(_))
            ));
        }
        root.close().unwrap();
    }
}
