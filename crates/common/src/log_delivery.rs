//! Purpose-limited credentials for exact worker-spooled log bytes.
use crate::{
    auth::jwt::{Claims, JwtConfig, TokenType},
    repositories::log_delivery::LogDeliveryIntent,
    Error, Result,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
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

#[derive(Clone, Serialize)]
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
    if claims.token_type != TokenType::Worker || claims.sub != "0" {
        return Err(denied());
    }
    let metadata = claims
        .metadata
        .as_ref()
        .and_then(Value::as_object)
        .filter(|fields| fields.len() == 4)
        .ok_or_else(denied)?;
    let positive_integer = |field| {
        metadata
            .get(field)
            .and_then(Value::as_i64)
            .filter(|value| *value > 0)
    };
    let outbox = positive_integer("outbox").ok_or_else(denied)?;
    let version = positive_integer("version").ok_or_else(denied)?;
    let attempt = positive_integer("attempt")
        .and_then(|value| i32::try_from(value).ok())
        .ok_or_else(denied)?;
    let claim = metadata
        .get("claim")
        .and_then(Value::as_str)
        .filter(|value| value.len() <= uuid::fmt::Urn::LENGTH)
        .and_then(|value| Uuid::try_parse(value).ok())
        .filter(|uuid| !uuid.is_nil())
        .ok_or_else(denied)?;
    Ok(Some(WorkflowCredential {
        outbox,
        version,
        claim,
        attempt,
    }))
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

#[cfg(test)]
mod workflow_credential_tests {
    use super::*;
    use crate::{auth::jwt::validate_token, models::log_stream::WorkflowLogOutboxRecord};
    use serde_json::json;

    #[test]
    fn workflow_log_credential_round_trips_through_signed_claims() {
        crate::auth::crypto_provider::install();
        let record = WorkflowLogOutboxRecord {
            id: i64::MAX,
            workflow_execution: 1,
            sequence: 0,
            kind: "seal".into(),
            payload: None,
            parent_execution: 2,
            action_ref: "fixture.workflow".into(),
            claimed_by: Uuid::new_v4(),
            attempt_count: i32::MAX,
        };
        let config = JwtConfig {
            secret: Uuid::new_v4().simple().to_string(),
            ..JwtConfig::default()
        };
        let token = mint_workflow(&record, 5, &config).unwrap();
        let claims = validate_token(&token, &config).unwrap();
        let parsed = workflow_credential(&claims).unwrap().unwrap();
        assert_eq!(parsed.outbox, record.id);
        assert_eq!(parsed.version, 5);
        assert_eq!(parsed.claim, record.claimed_by);
        assert_eq!(parsed.attempt, record.attempt_count);
    }

    #[test]
    fn workflow_log_credential_requires_exact_metadata_and_worker_subject() {
        let metadata = json!({"outbox": 1, "version": 2, "claim": Uuid::new_v4(), "attempt": 3});
        let mut claims = Claims {
            sub: "0".into(),
            login: "workflow-log-delivery".into(),
            iat: 1,
            exp: 2,
            token_type: TokenType::Worker,
            scope: Some(WORKFLOW_LOG_SCOPE.into()),
            metadata: Some(metadata.clone()),
        };
        assert!(workflow_credential(&claims).unwrap().is_some());

        for (field, value) in [
            ("outbox", json!(0)),
            ("version", json!(-1)),
            ("version", json!(1.0)),
            ("attempt", json!(i64::from(i32::MAX) + 1)),
            ("attempt", json!("3")),
            ("claim", json!(Uuid::nil())),
            ("claim", json!("invalid")),
            ("extra", json!(true)),
        ] {
            let mut invalid = metadata.clone();
            invalid[field] = value;
            claims.metadata = Some(invalid);
            assert!(workflow_credential(&claims).is_err(), "{field}");
        }

        let mut unknown = metadata.clone();
        let fields = unknown.as_object_mut().unwrap();
        fields.remove("attempt");
        claims.metadata = Some(unknown.clone());
        assert!(workflow_credential(&claims).is_err());
        unknown["extra"] = json!(3);
        for invalid in [None, Some(json!([])), Some(unknown)] {
            claims.metadata = invalid;
            assert!(workflow_credential(&claims).is_err());
        }

        claims.metadata = Some(metadata);
        claims.token_type = TokenType::Access;
        assert!(workflow_credential(&claims).is_err());
        claims.token_type = TokenType::Worker;
        claims.sub = "1".into();
        assert!(workflow_credential(&claims).is_err());
        claims.scope = Some("log_delivery".into());
        assert!(workflow_credential(&claims).unwrap().is_none());
    }
}
