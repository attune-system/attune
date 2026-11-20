//! Purpose-limited credentials for exact worker-spooled log bytes.
use crate::{
    auth::jwt::{Claims, JwtConfig, TokenType},
    repositories::log_delivery::LogDeliveryIntent,
    Error, Result,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Credential {
    pub intent: i64,
    pub version: i64,
    pub worker: i64,
    pub instance: Uuid,
}
pub fn credential(claims: &Claims) -> Result<Option<Credential>> {
    if claims.scope.as_deref() != Some("log_delivery") {
        return Ok(None);
    }
    let c: Credential = serde_json::from_value(claims.metadata.clone().unwrap_or_default())
        .map_err(|_| Error::PermissionDenied("Invalid log delivery credential".into()))?;
    if claims.token_type != TokenType::Worker
        || claims.sub != c.worker.to_string()
        || c.intent <= 0
        || c.version <= 0
        || c.worker <= 0
        || c.instance.is_nil()
    {
        return Err(Error::PermissionDenied(
            "Invalid log delivery binding".into(),
        ));
    }
    Ok(Some(c))
}
pub fn mint(intent: &LogDeliveryIntent, config: &JwtConfig) -> Result<String> {
    if !matches!(intent.state.as_str(), "prepared" | "active")
        || intent.finalize_by <= chrono::Utc::now()
    {
        return Err(Error::PermissionDenied("Log delivery unavailable".into()));
    }
    let claims = Claims {
        sub: intent.worker.to_string(),
        login: "log-delivery".into(),
        iat: chrono::Utc::now().timestamp(),
        exp: intent.finalize_by.timestamp(),
        token_type: TokenType::Worker,
        scope: Some("log_delivery".into()),
        metadata: Some(serde_json::to_value(Credential {
            intent: intent.id,
            version: intent.artifact_version,
            worker: intent.worker,
            instance: intent.delivery_instance,
        })?),
    };
    jsonwebtoken::encode(
        &jsonwebtoken::Header::default(),
        &claims,
        &jsonwebtoken::EncodingKey::from_secret(config.secret.as_bytes()),
    )
    .map_err(|_| Error::PermissionDenied("Unable to sign log delivery credential".into()))
}

pub const WORKFLOW_LOG_SCOPE: &str = "workflow_log_delivery";

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowCredential {
    pub outbox: i64,
    pub version: i64,
    pub claim: Uuid,
    pub attempt: i32,
}

pub fn workflow_credential(claims: &Claims) -> Result<Option<WorkflowCredential>> {
    if claims.scope.as_deref() != Some(WORKFLOW_LOG_SCOPE) {
        return Ok(None);
    }
    let denied = || Error::PermissionDenied("Invalid workflow log delivery credential".into());
    let credential: WorkflowCredential =
        serde_json::from_value(claims.metadata.clone().ok_or_else(denied)?)
            .map_err(|_| denied())?;
    if claims.token_type != TokenType::Worker
        || claims.sub != "0"
        || credential.outbox <= 0
        || credential.version <= 0
        || credential.claim.is_nil()
        || credential.attempt <= 0
    {
        return Err(denied());
    }
    Ok(Some(credential))
}

pub fn mint_workflow(
    record: &crate::models::log_stream::WorkflowLogOutboxRecord,
    version: i64,
    config: &JwtConfig,
) -> Result<String> {
    if config.secret.trim().len() < 32 || config.secret == JwtConfig::default().secret {
        return Err(Error::PermissionDenied(
            "Workflow log signing is unavailable".into(),
        ));
    }
    let now = chrono::Utc::now().timestamp();
    let claims = Claims {
        sub: "0".into(),
        login: "workflow-log-delivery".into(),
        iat: now,
        exp: now + 30,
        token_type: TokenType::Worker,
        scope: Some(WORKFLOW_LOG_SCOPE.into()),
        metadata: Some(serde_json::to_value(WorkflowCredential {
            outbox: record.id,
            version,
            claim: record.claimed_by,
            attempt: record.attempt_count,
        })?),
    };
    workflow_credential(&claims)?;
    jsonwebtoken::encode(
        &jsonwebtoken::Header::default(),
        &claims,
        &jsonwebtoken::EncodingKey::from_secret(config.secret.as_bytes()),
    )
    .map_err(|_| Error::PermissionDenied("Unable to sign workflow log delivery credential".into()))
}
