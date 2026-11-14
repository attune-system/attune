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
    let associated_pack_ref = match target {
        RuntimeTarget::Component(kind, id) => {
            RuntimeTargetRepository::component_associated_pack_ref(connection, kind, id).await?
        }
        _ => None,
    };
    let target = RuntimeTargetRepository::resolve(connection, target, identity)
        .await?
        .ok_or_else(|| ApiError::NotFound("Lifecycle target is unavailable".into()))?;
    let mut context = target.context().clone();
    if associated_pack_ref.is_some() {
        context.pack_ref = associated_pack_ref;
    }
    state
        .authorization_service()
        .authorize_access_fresh(
            connection,
            user,
            AuthorizationCheck {
                resource: target.resource(),
                action,
                context,
            },
        )
        .await
}

pub(super) async fn create(
    state: &AppState,
    transaction: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    user: &AuthenticatedUser,
    resource: Resource,
    reference: &str,
    pack: Option<(&str, i64)>,
) -> ApiResult<()> {
    let identity = user
        .identity_id()
        .map_err(|_| ApiError::Unauthorized("Invalid identity".into()))?;
    let mut context = AuthorizationContext::new(identity);
    context.target_ref = Some(reference.to_owned());
    context.pack_ref = pack.map(|(reference, _)| reference.to_owned());
    // API-created definitions are ad hoc: association is not managed ownership.
    attune_common::repositories::schema_type::SchemaTypeRepository::lock_selection(transaction)
        .await?;
    if let Some((reference, expected_id)) = pack {
        attune_common::repositories::pack::PackRepository::acquire_mutation_lock(
            transaction,
            reference,
        )
        .await?;
        let pack = attune_common::repositories::pack::PackRepository::find_by_ref_locked(
            transaction,
            reference,
        )
        .await?
        .ok_or_else(|| ApiError::NotFound("Creation pack is unavailable".into()))?;
        if pack.id != expected_id {
            return Err(ApiError::Conflict(
                "Creation pack changed during authorization".into(),
            ));
        }
        context.pack_ref = Some(pack.r#ref);
    }
    state
        .authorization_service()
        .authorize_access_fresh(
            transaction,
            user,
            AuthorizationCheck {
                resource,
                action: Action::Create,
                context,
            },
        )
        .await?;
    Ok(())
}
