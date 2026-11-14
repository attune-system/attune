//! Persisted interactive session issuance. Integration tokens never enter this path.
use crate::{dto::TokenResponse, middleware::error::ApiError, state::SharedState};
use attune_common::{
    models::Identity, repositories::interactive_session::InteractiveSessionRepository,
};

pub async fn issue(state: &SharedState, identity: &Identity) -> Result<TokenResponse, ApiError> {
    let access =
        super::jwt::generate_access_token(identity.id, &identity.login, &state.jwt_config)?;
    let refresh =
        super::jwt::generate_refresh_token(identity.id, &identity.login, &state.jwt_config)?;
    if !InteractiveSessionRepository::create(
        &state.db,
        identity,
        &refresh,
        state.jwt_config.refresh_token_expiration,
    )
    .await?
    {
        return Err(ApiError::Unauthorized(
            "Identity credentials changed; sign in again".into(),
        ));
    }
    Ok(TokenResponse::new(
        access,
        refresh,
        state.jwt_config.access_token_expiration.min(300),
    ))
}
