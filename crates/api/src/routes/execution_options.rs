//! Server-owned execution permission choices. This preview never admits a run;
//! execution creation rechecks the selected policy and current authority.

use std::{collections::BTreeSet, sync::Arc};

use axum::{
    extract::{Path, State},
    routing::get,
    Json, Router,
};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use attune_common::{
    auth::jwt::{TokenType, STANDARD_EXECUTION_ACCESS_REF},
    rbac::{Action, Resource},
    repositories::{
        action::ActionRepository,
        executable_snapshot::ExecutableSnapshotRepository,
        identity::PermissionSetRepository,
        managed_executable::ManagedExecutableRepository,
        runtime_authority::RuntimeAuthorityRepository,
        runtime_target::{ComponentTarget, RuntimeTarget, RuntimeTargetRepository},
        schema_type::SchemaTypeRepository,
        FindByRef, List,
    },
    runtime_authority::InvocationBinding,
};

use crate::{
    auth::RequireAuth,
    authz::AuthorizationCheck,
    dto::ApiResponse,
    middleware::{ApiError, ApiResult},
    state::AppState,
};

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ExecutionOptionsAuthoritySource {
    Pack,
    HumanDelegation,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ActionExecutionOptions {
    pub action_ref: String,
    pub default_permission_set_refs: Vec<String>,
    pub selectable_permission_set_refs: Vec<String>,
    pub authority_source: ExecutionOptionsAuthoritySource,
    pub policy_revision_id: Option<i64>,
    /// Informational only. Actual creation rechecks admission and revocation.
    pub default_selection_available: bool,
}

#[utoipa::path(
    get,
    path = "/api/v1/actions/{ref}/execution-options",
    tag = "actions",
    params(("ref" = String, Path, description = "Action reference")),
    responses(
        (status = 200, description = "Server-approved permission selection choices", body = inline(ApiResponse<ActionExecutionOptions>)),
        (status = 401, description = "Authentication required"),
        (status = 403, description = "Action execution is not authorized"),
        (status = 404, description = "Action not found"),
    ),
    security(("bearer_auth" = []))
)]
pub async fn get_execution_options(
    State(state): State<Arc<AppState>>,
    RequireAuth(user): RequireAuth,
    Path(action_ref): Path<String>,
) -> ApiResult<Json<ApiResponse<ActionExecutionOptions>>> {
    if user.claims.token_type != TokenType::Access {
        return Err(ApiError::Forbidden(
            "Execution selection inspection requires an access token".into(),
        ));
    }
    // Human IAM can query the pool. Load it before reserving a connection or
    // holding graph admission, then keep graph -> target -> policy lock order.
    let authz = state.authorization_service();
    let snapshot = authz.load_snapshot(&user).await?;
    let mut transaction = state.db.begin().await?;
    SchemaTypeRepository::lock_selection(&mut transaction).await?;
    let action = ActionRepository::find_by_ref(&mut *transaction, &action_ref)
        .await?
        .ok_or_else(|| ApiError::NotFound("Action not found".into()))?;
    let source = RuntimeTargetRepository::resolve(
        &mut transaction,
        RuntimeTarget::Component(ComponentTarget::Action, action.id),
        user.identity_id()
            .map_err(|_| ApiError::Unauthorized("Invalid authentication subject".into()))?,
    )
    .await?
    .ok_or_else(|| ApiError::Forbidden("Action structural context is unavailable".into()))?;
    if source.context().target_ref.as_deref() != Some(action.r#ref.as_str()) {
        return Err(ApiError::Forbidden(
            "Action structural context changed".into(),
        ));
    }
    let mut context = source.context().clone();
    // Association is useful for ordinary human selectors, but never constitutes
    // the managing-pack proof used for local capability scope.
    context.pack_ref = Some(action.pack_ref.clone());
    authz.authorize_with_snapshot(
        &user,
        snapshot.as_ref(),
        AuthorizationCheck {
            resource: Resource::Actions,
            action: Action::Execute,
            context,
        },
    )?;
    if !action.enabled || action.retired_at.is_some() || !source.accepts_new_effects() {
        return Err(ApiError::BadRequest("Action is disabled".into()));
    }
    if source.pack_id().is_some() {
        if source.pack_id() != Some(action.pack) {
            return Err(ApiError::Forbidden(
                "Managed action ownership requires readmission".into(),
            ));
        }
        let policy = RuntimeAuthorityRepository::current_policy(&mut transaction, action.pack)
            .await?
            .ok_or_else(|| ApiError::Forbidden("Managed action has no current admission".into()))?;
        let policy =
            RuntimeAuthorityRepository::require_policy(&mut transaction, policy.id).await?;
        let binding = InvocationBinding::Action {
            action_ref: action.r#ref.clone(),
        };
        let invocation = policy.policy.invocation_for_source(&binding, &source)?;
        if invocation.metadata_revision.is_none() {
            return Err(ApiError::Forbidden(
                "Managed action metadata requires readmission".into(),
            ));
        }
        let executable = ManagedExecutableRepository::resolve_invocation(
            &mut transaction,
            &policy.policy,
            &binding,
        )
        .await?;
        // Current code pins prove projection identity, not editable defaults.
        // The choices below still come only from admitted immutable metadata.
        let projected_code =
            ExecutableSnapshotRepository::resolve_for_action(&mut *transaction, action.id).await?;
        if executable.executable.action.id != action.id
            || executable.executable.action.pack != action.pack
            || projected_code.release.id != executable.release.id
            || projected_code.release.digest != executable.release.digest
        {
            return Err(ApiError::Forbidden(
                "Admitted action identity changed".into(),
            ));
        }
        let defaults: Vec<_> = executable
            .executable
            .action
            .default_execution_permission_set_refs
            .iter()
            .cloned()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        let mut active_approvals = std::collections::BTreeMap::new();
        for approval_id in invocation
            .permission_sets
            .values()
            .flat_map(|set| set.grants.iter().filter_map(|grant| grant.approval_id()))
            .collect::<BTreeSet<_>>()
        {
            let active =
                match RuntimeAuthorityRepository::find_approval(&mut transaction, approval_id)
                    .await?
                {
                    Some(approval) => {
                        approval.approval()?;
                        approval.subject_pack == action.pack
                            && approval.is_active(chrono::Utc::now())
                    }
                    None => false,
                };
            active_approvals.insert(approval_id, active);
        }
        let mut selectable = invocation
            .permission_sets
            .iter()
            .filter(|(_, set)| {
                set.grants.iter().all(|grant| {
                    grant
                        .approval_id()
                        .is_none_or(|id| active_approvals.get(&id) == Some(&true))
                })
            })
            .map(|(reference, _)| reference.clone())
            .collect::<BTreeSet<_>>();
        if invocation.standard_access {
            selectable.insert(STANDARD_EXECUTION_ACCESS_REF.into());
        }
        let default_selection_available = defaults
            .iter()
            .all(|reference| selectable.contains(reference));
        transaction.commit().await?;
        return Ok(Json(ApiResponse::new(ActionExecutionOptions {
            action_ref: action.r#ref,
            default_permission_set_refs: defaults,
            selectable_permission_set_refs: selectable.into_iter().collect(),
            authority_source: ExecutionOptionsAuthoritySource::Pack,
            policy_revision_id: Some(policy.id),
            default_selection_available,
        })));
    }
    let executable =
        ExecutableSnapshotRepository::resolve_for_action(&mut *transaction, action.id).await?;
    let defaults: Vec<_> = executable
        .executable
        .action
        .default_execution_permission_set_refs
        .iter()
        .cloned()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    transaction.commit().await?;
    // Delegation is checked by effective coverage, not membership of an assigned
    // permission-set name. No caller-side core.admin special case is needed.
    let mut selectable = BTreeSet::from([STANDARD_EXECUTION_ACCESS_REF.to_owned()]);
    for set in PermissionSetRepository::list(&state.db).await? {
        if authz
            .can_delegate_permission_sets_with_snapshot(
                &user,
                snapshot.as_ref(),
                std::slice::from_ref(&set.r#ref),
            )
            .await?
        {
            selectable.insert(set.r#ref);
        }
    }
    let default_selection_available = defaults
        .iter()
        .all(|reference| selectable.contains(reference));
    Ok(Json(ApiResponse::new(ActionExecutionOptions {
        action_ref: action.r#ref,
        default_permission_set_refs: defaults,
        selectable_permission_set_refs: selectable.into_iter().collect(),
        authority_source: ExecutionOptionsAuthoritySource::HumanDelegation,
        policy_revision_id: None,
        default_selection_available,
    })))
}

pub fn routes() -> Router<Arc<AppState>> {
    Router::new().route(
        "/actions/{ref}/execution-options",
        get(get_execution_options),
    )
}
