//! Dedicated target-aware controller credentials, minted only by the trusted CP.
//! These are not IAM user grants and cannot be consumed outside launch routes.
use crate::{
    auth::jwt::{Claims, JwtConfig, TokenType},
    repositories::workload_launch::ControllerActor,
    Error, Result,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const SCOPE: &str = "workload_controller";
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Binding {
    controller: i64,
    engine: i64,
    incarnation: Uuid,
}
fn denied() -> Error {
    Error::PermissionDenied("Controller credential unavailable".into())
}
pub fn actor(claims: &Claims) -> Result<ControllerActor> {
    if claims.token_type != TokenType::Worker || claims.scope.as_deref() != Some(SCOPE) {
        return Err(denied());
    }
    let binding: Binding = serde_json::from_value(claims.metadata.clone().ok_or_else(denied)?)
        .map_err(|_| denied())?;
    let identity = claims.sub.parse::<i64>().map_err(|_| denied())?;
    if identity <= 0
        || claims.sub != identity.to_string()
        || binding.controller <= 0
        || binding.engine <= 0
        || binding.incarnation.is_nil()
        || claims.exp <= chrono::Utc::now().timestamp()
    {
        return Err(denied());
    }
    Ok(ControllerActor {
        controller: binding.controller,
        identity,
        engine: binding.engine,
        incarnation: binding.incarnation,
        expires_at: claims.exp,
    })
}
pub fn mint(actor: ControllerActor, config: &JwtConfig) -> Result<String> {
    let now = chrono::Utc::now().timestamp();
    if actor.identity <= 0
        || actor.controller <= 0
        || actor.engine <= 0
        || actor.incarnation.is_nil()
        || actor.expires_at <= now
        || actor.expires_at > now + 300
    {
        return Err(denied());
    }
    let claims = Claims {
        sub: actor.identity.to_string(),
        login: "workload-controller".into(),
        iat: now,
        exp: actor.expires_at,
        token_type: TokenType::Worker,
        scope: Some(SCOPE.into()),
        metadata: Some(serde_json::to_value(Binding {
            controller: actor.controller,
            engine: actor.engine,
            incarnation: actor.incarnation,
        })?),
    };
    jsonwebtoken::encode(
        &jsonwebtoken::Header::default(),
        &claims,
        &jsonwebtoken::EncodingKey::from_secret(config.secret.as_bytes()),
    )
    .map_err(|_| denied())
}
