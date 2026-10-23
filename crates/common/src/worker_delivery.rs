//! Purpose-specific worker delivery credentials. Never pass these to action code.
use crate::repositories::worker_delivery::{DeliveryContext, DeliveryPermit};
use crate::{
    auth::jwt::{Claims, JwtConfig, TokenType},
    Error, Result,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Stable roots for recorded logs, shared by worker allocation and API discovery.
/// This only names a stream; it does not authorize disclosure.
pub fn log_reference(execution: &crate::models::Execution, stream: &str) -> Result<String> {
    if !matches!(stream, "stdout" | "stderr") {
        return Err(Error::validation("Unknown execution log stream"));
    }
    let snapshot = execution
        .executable_snapshot
        .as_ref()
        .ok_or_else(|| Error::PermissionDenied("Pinned log owner unavailable".into()))?;
    Ok(format!(
        "{}.owner_{}.{}.log",
        execution.action_ref, snapshot.executable.action.id, stream
    ))
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkerDeliveryCredential {
    pub permit: Uuid,
    pub worker: i64,
    pub instance: Uuid,
    pub version: i64,
}

pub fn credential(claims: &Claims) -> Result<Option<WorkerDeliveryCredential>> {
    if claims.scope.as_deref() != Some("worker_delivery") {
        return Ok(None);
    }
    if claims.token_type != TokenType::Worker {
        return Err(Error::PermissionDenied(
            "Invalid worker delivery credential".into(),
        ));
    }
    let credential: WorkerDeliveryCredential =
        serde_json::from_value(claims.metadata.clone().unwrap_or_default())
            .map_err(|_| Error::PermissionDenied("Invalid worker delivery credential".into()))?;
    if credential.worker <= 0
        || credential.version <= 0
        || credential.permit.is_nil()
        || credential.instance.is_nil()
        || claims.sub != credential.worker.to_string()
    {
        return Err(Error::PermissionDenied(
            "Invalid worker delivery binding".into(),
        ));
    }
    Ok(Some(credential))
}

pub fn mint(
    context: &DeliveryContext,
    permit: &DeliveryPermit,
    config: &JwtConfig,
) -> Result<String> {
    if permit.context != context.id || context.deadline <= chrono::Utc::now() {
        return Err(Error::PermissionDenied(
            "Invalid worker delivery binding".into(),
        ));
    }
    let claims = Claims {
        sub: context.worker.to_string(),
        login: "worker-delivery".into(),
        iat: chrono::Utc::now().timestamp(),
        exp: context.deadline.timestamp(),
        token_type: TokenType::Worker,
        scope: Some("worker_delivery".into()),
        metadata: Some(
            serde_json::to_value(WorkerDeliveryCredential {
                permit: permit.id,
                worker: context.worker,
                instance: context.worker_instance,
                version: permit.artifact_version,
            })
            .map_err(|_| Error::PermissionDenied("Invalid worker delivery binding".into()))?,
        ),
    };
    jsonwebtoken::encode(
        &jsonwebtoken::Header::default(),
        &claims,
        &jsonwebtoken::EncodingKey::from_secret(config.secret.as_bytes()),
    )
    .map_err(|_| Error::PermissionDenied("Unable to sign worker delivery credential".into()))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn delivery_credentials_are_not_execution_credentials_and_bind_exact_worker_instance() {
        crate::auth::crypto_provider::install();
        let context = DeliveryContext {
            id: Uuid::new_v4(),
            execution: 11,
            admission: 12,
            attempt: Uuid::new_v4(),
            worker: 13,
            worker_instance: Uuid::new_v4(),
            deadline: chrono::Utc::now() + chrono::Duration::minutes(1),
        };
        let permit = DeliveryPermit {
            id: Uuid::new_v4(),
            context: context.id,
            artifact_version: 14,
            log_stream: Some(15),
            max_bytes: 10,
            max_sequence: 10,
            publication_action: "update".into(),
            max_plaintext_bytes: None,
        };
        let config = JwtConfig::default();
        let token = mint(&context, &permit, &config).unwrap();
        let mut claims = crate::auth::jwt::validate_token(&token, &config).unwrap();
        assert!(claims.admitted_execution_credential().unwrap().is_none());
        let parsed = credential(&claims).unwrap().unwrap();
        assert_eq!(parsed.worker, context.worker);
        assert_eq!(parsed.instance, context.worker_instance);
        assert_eq!(parsed.version, permit.artifact_version);
        claims.sub = "99".into();
        assert!(credential(&claims).is_err());
        claims.sub = context.worker.to_string();
        claims.token_type = TokenType::Execution;
        assert!(credential(&claims).is_err());
    }

    #[test]
    fn delivery_mint_rejects_other_context_and_expired_deadline() {
        let mut context = DeliveryContext {
            id: Uuid::new_v4(),
            execution: 11,
            admission: 12,
            attempt: Uuid::new_v4(),
            worker: 13,
            worker_instance: Uuid::new_v4(),
            deadline: chrono::Utc::now() - chrono::Duration::seconds(1),
        };
        let permit = DeliveryPermit {
            id: Uuid::new_v4(),
            context: context.id,
            artifact_version: 14,
            log_stream: None,
            max_bytes: 10,
            max_sequence: 0,
            publication_action: "update".into(),
            max_plaintext_bytes: None,
        };
        assert!(mint(&context, &permit, &JwtConfig::default()).is_err());
        context.deadline = chrono::Utc::now() + chrono::Duration::minutes(1);
        context.id = Uuid::new_v4();
        assert!(mint(&context, &permit, &JwtConfig::default()).is_err());
    }
}
