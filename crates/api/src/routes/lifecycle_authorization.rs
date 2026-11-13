//! Fresh access-token authorization for definition lifecycle controls.
use crate::{
    auth::middleware::AuthenticatedUser,
    authz::AuthorizationCheck,
    middleware::{ApiError, ApiResult},
    state::AppState,
};
use attune_common::{
    rbac::{Action, AuthorizationContext, Resource},
    repositories::runtime_target::{RuntimeTarget, RuntimeTargetRepository},
};

pub(super) async fn existing(
    state: &AppState,
    connection: &mut sqlx::PgConnection,
    user: &AuthenticatedUser,
    target: RuntimeTarget,
    action: Action,
) -> ApiResult<()> {
    let identity = user
        .identity_id()
        .map_err(|_| ApiError::Unauthorized("Invalid identity".into()))?;
    let target = RuntimeTargetRepository::resolve(connection, target, identity)
        .await?
        .ok_or_else(|| ApiError::NotFound("Lifecycle target is unavailable".into()))?;
    state
        .authorization_service()
        .authorize_access_fresh(
            connection,
            user,
            AuthorizationCheck {
                resource: target.resource(),
                action,
                context: target.context().clone(),
            },
        )
        .await
}

pub(super) async fn create(
    state: &AppState,
    user: &AuthenticatedUser,
    resource: Resource,
    reference: &str,
    pack_ref: Option<&str>,
) -> ApiResult<()> {
    let identity = user
        .identity_id()
        .map_err(|_| ApiError::Unauthorized("Invalid identity".into()))?;
    let mut context = AuthorizationContext::new(identity);
    context.target_ref = Some(reference.to_owned());
    context.pack_ref = pack_ref.map(str::to_owned);
    // API-created definitions are ad hoc: association is not managed ownership.
    let mut tx = state.db.begin().await?;
    state
        .authorization_service()
        .authorize_access_fresh(
            &mut tx,
            user,
            AuthorizationCheck {
                resource,
                action: Action::Create,
                context,
            },
        )
        .await?;
    tx.commit().await?;
    Ok(())
}
