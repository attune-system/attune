//! Registered operator storage-management transport. This is deliberately NOT
//! an S3 data endpoint, an execution MCP action, or a caller-supplied proof body.
//! Only pinned, fresh, challenge-bound controller observations become native
//! facts. The controller must own the actual process/provider management scope.
use crate::{
    config::{BlobStorageConfig, Config},
    repositories::file_legacy_audit::{
        ExactLegacyAbsence, LegacyAuditItem, LegacyAuditPage, LegacyAuditPlan, LegacyAuditScope,
        LegacyFileAuditRepository, NativeLegacyAuditor, NativeWriterFence,
    },
    Error, Result,
};
use async_trait::async_trait;
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use futures::StreamExt;
use jsonwebtoken::DecodingKey;
use serde::{de::DeserializeOwned, Deserialize};
use serde_json::{json, Value};
use sqlx::PgPool;
use std::{sync::Arc, time::Duration};
use uuid::Uuid;

const MAX_REPLY: usize = 1024 * 1024;

pub struct StorageLegacyController {
    scope: LegacyAuditScope,
    pool: PgPool,
    connection: Option<ManagementConnection>,
}

struct ManagementConnection {
    endpoint: reqwest::Url,
    client: reqwest::Client,
    key: DecodingKey,
    issuer: String,
    audience: String,
}

#[derive(Deserialize)]
struct ControllerObservation {
    iss: String,
    aud: String,
    iat: i64,
    exp: i64,
    nbf: Option<i64>,
    nonce: Uuid,
    audit: Uuid,
    operation: String,
    request_digest: String,
    scope: LegacyAuditScope,
    data: Value,
}

fn validate_binding(
    observed: &ControllerObservation,
    plan: &LegacyAuditPlan,
    operation: &str,
    nonce: Uuid,
    request_digest: &str,
    now: i64,
) -> Result<()> {
    if observed.nonce != nonce
        || observed.audit != plan.id
        || observed.scope != plan.scope
        || observed.operation != operation
        || observed.request_digest != request_digest
        || observed.iat > now
        || observed.iat < now - 60
        || observed.exp > now + 60
        || observed.exp <= now
        || observed
            .exp
            .checked_sub(observed.iat)
            .is_none_or(|lifetime| lifetime > 60)
        || observed.nbf.is_some_and(|at| at > now)
        || observed.data.is_null()
    {
        return Err(unavailable());
    }
    Ok(())
}

fn verify_observation(
    connection: &ManagementConnection,
    signed: &str,
) -> Result<ControllerObservation> {
    let parts: Vec<_> = signed.splitn(4, '.').collect();
    if parts.len() != 3 {
        return Err(unavailable());
    }
    let header: Value = serde_json::from_slice(
        &URL_SAFE_NO_PAD
            .decode(parts[0])
            .map_err(|_| unavailable())?,
    )
    .map_err(|_| unavailable())?;
    if header.get("alg").and_then(Value::as_str) != Some("RS256")
        || header.get("crit").is_some()
        || header.get("b64").is_some()
    {
        return Err(unavailable());
    }
    let signature = URL_SAFE_NO_PAD
        .decode(parts[2])
        .map_err(|_| unavailable())?;
    let message = format!("{}.{}", parts[0], parts[1]);
    // Attune's process-global JWT provider intentionally supports HMAC only.
    // Verify the independently pinned management key without replacing it.
    aws_lc_rs::signature::UnparsedPublicKey::new(
        &aws_lc_rs::signature::RSA_PKCS1_2048_8192_SHA256,
        connection.key.as_bytes(),
    )
    .verify(message.as_bytes(), &signature)
    .map_err(|_| unavailable())?;
    let claims: ControllerObservation = serde_json::from_slice(
        &URL_SAFE_NO_PAD
            .decode(parts[1])
            .map_err(|_| unavailable())?,
    )
    .map_err(|_| unavailable())?;
    if claims.iss != connection.issuer || claims.aud != connection.audience {
        return Err(unavailable());
    }
    Ok(claims)
}

fn unavailable() -> Error {
    Error::invalid_state("Native legacy storage management proof is unavailable; retain quarantine. Configure the owning mTLS storage controller with a pinned independent signing key, or use a separately authorized export/isolation migration. Standard S3/RustFS data APIs and local HEAD do not prove old-writer/request settlement")
}

fn digest(value: &Value) -> Result<String> {
    crate::value_schema::digest(value).map_err(|_| unavailable())
}

fn config_file(path: &std::path::Path) -> Result<Vec<u8>> {
    use std::io::Read;
    let file = std::fs::File::open(path).map_err(|_| unavailable())?;
    let mut bytes = Vec::new();
    file.take((MAX_REPLY + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| unavailable())?;
    if bytes.len() > MAX_REPLY {
        return Err(unavailable());
    }
    Ok(bytes)
}

impl StorageLegacyController {
    /// Constructed once from the API's validated storage configuration. The
    /// backend fingerprint includes both blob storage and SharedFile transport
    /// roots, so a controller cannot settle one deployment using another root.
    pub fn configured(config: &Config, pool: PgPool) -> Result<Arc<Self>> {
        let scope = LegacyAuditScope {
            backend: digest(
                &json!({"storage":config.storage,"artifacts_dir":config.artifacts_dir}),
            )?,
            staging_prefix: ".attune-upload/".into(),
        };
        // This management integration is registered for the application's
        // SharedFile/filesystem and versioned S3/RustFS storage topologies.
        // GCS and injected/custom backends stay explicitly unsupported.
        let settings = match &config.storage {
            BlobStorageConfig::Filesystem { .. } | BlobStorageConfig::S3 { .. } => {
                config.artifacts.legacy_audit_controller.as_ref()
            }
            BlobStorageConfig::Gcs { .. } => None,
        };
        let connection = settings
            .map(|settings| {
                let mut endpoint =
                    reqwest::Url::parse(&settings.endpoint).map_err(|_| unavailable())?;
                if endpoint.scheme() != "https"
                    || endpoint.host_str().is_none()
                    || !endpoint.username().is_empty()
                    || endpoint.password().is_some()
                    || endpoint.query().is_some()
                    || endpoint.fragment().is_some()
                    || settings.issuer.is_empty()
                    || settings.audience.is_empty()
                {
                    return Err(unavailable());
                }
                if !endpoint.path().ends_with('/') {
                    endpoint.set_path(&format!("{}/", endpoint.path()));
                }
                let key = DecodingKey::from_rsa_pem(&config_file(&settings.verification_key_file)?)
                    .map_err(|_| unavailable())?;
                let identity =
                    reqwest::Identity::from_pem(&config_file(&settings.client_identity_file)?)
                        .map_err(|_| unavailable())?;
                let certificate =
                    reqwest::Certificate::from_pem(&config_file(&settings.ca_certificate_file)?)
                        .map_err(|_| unavailable())?;
                let client = reqwest::Client::builder()
                    .redirect(reqwest::redirect::Policy::none())
                    .connect_timeout(Duration::from_secs(3))
                    .timeout(Duration::from_secs(10))
                    .identity(identity)
                    .add_root_certificate(certificate)
                    .build()
                    .map_err(|_| unavailable())?;
                Ok(ManagementConnection {
                    endpoint,
                    client,
                    key,
                    issuer: settings.issuer.clone(),
                    audience: settings.audience.clone(),
                })
            })
            .transpose()?;
        Ok(Arc::new(Self {
            scope,
            pool,
            connection,
        }))
    }

    pub fn available(&self) -> bool {
        self.connection.is_some()
    }

    /// Hosts injecting arbitrary BlobStore implementations cannot inherit the
    /// native configured backend's proof authority by merely copying its config.
    pub fn unsupported(config: &Config, pool: PgPool) -> Result<Arc<Self>> {
        Ok(Arc::new(Self {
            scope: LegacyAuditScope {
                backend: format!(
                    "custom:{}",
                    digest(
                        &json!({"storage":config.storage,"artifacts_dir":config.artifacts_dir})
                    )?
                ),
                staging_prefix: ".attune-upload/".into(),
            },
            pool,
            connection: None,
        }))
    }

    pub fn scope(&self) -> &LegacyAuditScope {
        &self.scope
    }

    async fn observe<T: DeserializeOwned>(
        &self,
        plan: &LegacyAuditPlan,
        operation: &str,
        arguments: Value,
    ) -> Result<T> {
        let connection = self.connection.as_ref().ok_or_else(unavailable)?;
        if plan.scope != self.scope {
            return Err(unavailable());
        }
        let nonce = Uuid::new_v4();
        let request =
            json!({"nonce":nonce,"plan":plan,"operation":operation,"arguments":arguments});
        let bytes = serde_json::to_vec(&request)?;
        if bytes.len() > MAX_REPLY {
            return Err(unavailable());
        }
        let request_digest = digest(&request)?;
        let url = connection
            .endpoint
            .join(&format!("legacy-file-audits/{}/{operation}", plan.id))
            .map_err(|_| unavailable())?;
        let response = connection
            .client
            .post(url)
            .header("accept", "application/jose")
            .header("content-type", "application/json")
            .body(bytes)
            .send()
            .await
            .map_err(|_| unavailable())?;
        if !response.status().is_success()
            || response
                .content_length()
                .is_some_and(|n| n > MAX_REPLY as u64)
        {
            return Err(unavailable());
        }
        let mut stream = response.bytes_stream();
        let mut bytes = Vec::new();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|_| unavailable())?;
            if bytes.len().saturating_add(chunk.len()) > MAX_REPLY {
                return Err(unavailable());
            }
            bytes.extend_from_slice(&chunk);
        }
        let signed = std::str::from_utf8(&bytes).map_err(|_| unavailable())?;
        let observed = verify_observation(connection, signed)?;
        let now = chrono::Utc::now().timestamp();
        validate_binding(&observed, plan, operation, nonce, &request_digest, now)?;
        let data = serde_json::from_value(observed.data).map_err(|_| unavailable())?;
        // A management attestation is audit-scoped evidence, NOT a login or
        // provider credential. Never store mTLS private keys, URLs with auth,
        // raw response errors, or arbitrary controller logs here.
        LegacyFileAuditRepository::controller_observation(
            &self.pool,
            plan.id,
            &json!({"operation":operation,"request_digest":request_digest,"verification_key_sha256":URL_SAFE_NO_PAD.encode(crate::blob_store::sha256(connection.key.as_bytes())),"attestation":signed}),
        )
        .await?;
        Ok(data)
    }
}

#[async_trait]
impl NativeLegacyAuditor for StorageLegacyController {
    fn scope(&self) -> LegacyAuditScope {
        self.scope.clone()
    }

    async fn stop_and_settle_writers(&self, plan: &LegacyAuditPlan) -> Result<NativeWriterFence> {
        self.observe(plan, "stop-and-settle", json!({})).await
    }

    async fn resume_fence(
        &self,
        plan: &LegacyAuditPlan,
        receipt: &Value,
    ) -> Result<NativeWriterFence> {
        // A fresh challenge-bound management response must revalidate the native
        // restart barrier and provider drain record. A local JSON echo cannot.
        self.observe(plan, "verify-fence", json!({"receipt":receipt}))
            .await
    }

    async fn catalogue(
        &self,
        plan: &LegacyAuditPlan,
        fence: &NativeWriterFence,
        cursor: Option<&str>,
        limit: usize,
    ) -> Result<LegacyAuditPage> {
        self.observe(
            plan,
            "catalogue",
            json!({"fence":fence,"cursor":cursor,"limit":limit.min(128),"max_bytes":512*1024}),
        )
        .await
    }

    async fn delete_and_confirm_exact(
        &self,
        plan: &LegacyAuditPlan,
        fence: &NativeWriterFence,
        item: &LegacyAuditItem,
    ) -> Result<ExactLegacyAbsence> {
        self.observe(plan, "delete-exact", json!({"fence":fence,"item":item}))
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aws_lc_rs::signature::KeyPair;

    fn signer() -> aws_lc_rs::signature::RsaKeyPair {
        let key = jsonwebtoken::EncodingKey::from_rsa_pem(include_bytes!(
            "../tests/fixtures/jwt_signing_test_private.pem"
        ))
        .unwrap();
        aws_lc_rs::signature::RsaKeyPair::from_der(key.inner()).unwrap()
    }

    fn sign(pair: &aws_lc_rs::signature::RsaKeyPair, payload: &Value, algorithm: &str) -> String {
        let header = URL_SAFE_NO_PAD
            .encode(serde_json::to_vec(&json!({"alg":algorithm,"typ":"JWT"})).unwrap());
        let claims = URL_SAFE_NO_PAD.encode(serde_json::to_vec(payload).unwrap());
        let message = format!("{header}.{claims}");
        let mut signature = vec![0; pair.public_modulus_len()];
        pair.sign(
            &aws_lc_rs::signature::RSA_PKCS1_SHA256,
            &aws_lc_rs::rand::SystemRandom::new(),
            message.as_bytes(),
            &mut signature,
        )
        .unwrap();
        format!("{message}.{}", URL_SAFE_NO_PAD.encode(signature))
    }

    #[tokio::test]
    async fn management_observations_require_independent_signature_and_exact_challenge_binding() {
        let pair = signer();
        let connection = ManagementConnection {
            endpoint: reqwest::Url::parse("https://controller.example.test/").unwrap(),
            client: reqwest::Client::new(),
            key: DecodingKey::from_rsa_der(pair.public_key().as_ref()),
            issuer: "native-management".into(),
            audience: "legacy-file-audit".into(),
        };
        let plan = LegacyAuditPlan {
            id: Uuid::new_v4(),
            scope: LegacyAuditScope {
                backend: "owned-storage".into(),
                staging_prefix: ".attune-upload/".into(),
            },
            members: vec![],
        };
        let nonce = Uuid::new_v4();
        let claims = json!({"iss":connection.issuer,"aud":connection.audience,"iat":1000,"exp":1030,"nonce":nonce,"audit":plan.id,"operation":"stop-and-settle","request_digest":"exact-request","scope":plan.scope,"data":{"native_stop_receipt":"owned-controller-record"}});
        let signed = sign(&pair, &claims, "RS256");
        let observed = verify_observation(&connection, &signed).unwrap();
        validate_binding(
            &observed,
            &plan,
            "stop-and-settle",
            nonce,
            "exact-request",
            1000,
        )
        .unwrap();
        let mut overlong = claims.clone();
        overlong["iat"] = json!(960);
        overlong["exp"] = json!(1040);
        let overlong = verify_observation(&connection, &sign(&pair, &overlong, "RS256")).unwrap();
        assert!(validate_binding(
            &overlong,
            &plan,
            "stop-and-settle",
            nonce,
            "exact-request",
            1000
        )
        .is_err());
        assert!(validate_binding(
            &observed,
            &plan,
            "stop-and-settle",
            Uuid::new_v4(),
            "exact-request",
            1000
        )
        .is_err());
        assert!(validate_binding(
            &observed,
            &plan,
            "delete-exact",
            nonce,
            "exact-request",
            1000
        )
        .is_err());
        assert!(validate_binding(
            &observed,
            &plan,
            "stop-and-settle",
            nonce,
            "different-plan-or-body",
            1000
        )
        .is_err());
        assert!(validate_binding(
            &observed,
            &plan,
            "stop-and-settle",
            nonce,
            "exact-request",
            1031
        )
        .is_err());
        assert!(verify_observation(&connection, &sign(&pair, &claims, "HS256")).is_err());
        let mut tampered = claims.clone();
        tampered["data"] = json!({"writer_stopped":true});
        let payload = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&tampered).unwrap());
        let parts: Vec<_> = signed.split('.').collect();
        assert!(
            verify_observation(&connection, &format!("{}.{payload}.{}", parts[0], parts[2]))
                .is_err()
        );
        let mut wrong_audience = claims.clone();
        wrong_audience["aud"] = json!("execution-token");
        assert!(verify_observation(&connection, &sign(&pair, &wrong_audience, "RS256")).is_err());
    }
}
