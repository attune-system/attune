//! Explicit external approvals. Human IAM is checked when changing authority,
//! not when an admitted execution consumes it.

use crate::{
    auth::middleware::{AuthenticatedUser, RequireAuth},
    authz::AuthorizationCheck,
    dto::{
        runtime_approval::{IssueRuntimeApprovalRequest, RuntimeApprovalResponse},
        ApiResponse, PaginatedResponse, PaginationParams,
    },
    middleware::{ApiError, ApiResult},
    state::AppState,
};
use attune_common::{
    audit::{event_type, AuditCategory, AuditEventBuilder, AuditOutcome},
    auth::TokenType,
    delegation::DelegationAuthority,
    models::Pack,
    rbac::{Action, AuthorizationContext, Resource},
    repositories::{
        pack::PackRepository,
        runtime_authority::{ExternalApprovalRecord, RuntimeAuthorityRepository},
    },
};
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use std::sync::Arc;

async fn authorize_pack(
    connection: &mut sqlx::PgConnection,
    state: &AppState,
    user: &AuthenticatedUser,
    reference: &str,
) -> ApiResult<Pack> {
    if user.claims.token_type != TokenType::Access {
        return Err(ApiError::Forbidden(
            "External approval controls require an access token".into(),
        ));
    }
    let identity = user
        .identity_id()
        .map_err(|_| ApiError::Unauthorized("Invalid identity".into()))?;
    attune_common::repositories::schema_type::SchemaTypeRepository::lock_selection(
        &mut *connection,
    )
    .await?;
    let pack = PackRepository::find_by_ref_for_share(connection, reference)
        .await?
        .ok_or_else(|| ApiError::NotFound("Pack not found".into()))?;
    let mut context = AuthorizationContext::new(identity);
    context.target_id = Some(pack.id);
    context.target_ref = Some(pack.r#ref.clone());
    context.pack_ref = Some(pack.r#ref.clone());
    state
        .authorization_service()
        .authorize_identity_fresh(
            connection,
            identity,
            AuthorizationCheck {
                resource: Resource::Packs,
                action: Action::Read,
                context,
            },
        )
        .await?;
    Ok(pack)
}

async fn subject_approval(
    connection: &mut sqlx::PgConnection,
    pack_id: i64,
    id: i64,
) -> ApiResult<ExternalApprovalRecord> {
    RuntimeAuthorityRepository::find_approval(connection, id)
        .await?
        .filter(|record| record.subject_pack == pack_id)
        .ok_or_else(|| ApiError::NotFound("External approval not found".into()))
}

fn audit(
    state: &AppState,
    user: &AuthenticatedUser,
    operation: &'static str,
    pack: &Pack,
    id: i64,
    previous: Option<i64>,
) {
    let mut event = AuditEventBuilder::new(AuditCategory::Admin, operation, AuditOutcome::Success)
        .actor_login(user.login().to_string())
        .resource("runtime_external_approval")
        .resource_id(id)
        .with_details(
            serde_json::json!({"subject_pack_id": pack.id, "previous_approval_id": previous}),
        );
    if let Ok(identity) = user.identity_id() {
        event = event.actor_identity(identity);
    }
    state.audit_emitter.emit(event.build());
}

#[utoipa::path(get, path = "/api/v1/packs/{pack_ref}/external-approvals", tag = "permissions",
    params(("pack_ref" = String, Path), PaginationParams),
    responses((status = 200, description = "Fixed external approvals, including revoked and expired records", body = PaginatedResponse<RuntimeApprovalResponse>)),
    security(("bearer_auth" = [])))]
pub async fn list_runtime_approvals(
    State(state): State<Arc<AppState>>,
    RequireAuth(user): RequireAuth,
    Path(reference): Path<String>,
    Query(page): Query<PaginationParams>,
) -> ApiResult<Json<PaginatedResponse<RuntimeApprovalResponse>>> {
    if page.page == 0 || page.page_size == 0 || page.page_size > 100 {
        return Err(ApiError::BadRequest(
            "page must be positive and page_size must be 1 to 100".into(),
        ));
    }
    let mut tx = state.db.begin().await?;
    let pack = authorize_pack(&mut tx, &state, &user, &reference).await?;
    let (records, total) = RuntimeAuthorityRepository::list_approvals(
        &mut tx,
        pack.id,
        i64::from(page.limit()),
        i64::from(page.page.saturating_sub(1)) * i64::from(page.limit()),
    )
    .await?;
    tx.commit().await?;
    Ok(Json(PaginatedResponse::new(
        records.into_iter().map(Into::into).collect(),
        &page,
        total as u64,
    )))
}

#[utoipa::path(get, path = "/api/v1/packs/{pack_ref}/external-approvals/{id}", tag = "permissions",
    params(("pack_ref" = String, Path), ("id" = i64, Path)),
    responses((status = 200, description = "Immutable external approval", body = ApiResponse<RuntimeApprovalResponse>)),
    security(("bearer_auth" = [])))]
pub async fn get_runtime_approval(
    State(state): State<Arc<AppState>>,
    RequireAuth(user): RequireAuth,
    Path((reference, id)): Path<(String, i64)>,
) -> ApiResult<Json<ApiResponse<RuntimeApprovalResponse>>> {
    let mut tx = state.db.begin().await?;
    let pack = authorize_pack(&mut tx, &state, &user, &reference).await?;
    let record = subject_approval(&mut tx, pack.id, id).await?;
    tx.commit().await?;
    Ok(Json(ApiResponse::new(record.into())))
}

#[utoipa::path(post, path = "/api/v1/packs/{pack_ref}/external-approvals", tag = "permissions",
    params(("pack_ref" = String, Path)), request_body = IssueRuntimeApprovalRequest,
    responses((status = 201, description = "External approval issued after current authority validation", body = ApiResponse<RuntimeApprovalResponse>)),
    security(("bearer_auth" = [])))]
pub async fn issue_runtime_approval(
    State(state): State<Arc<AppState>>,
    RequireAuth(user): RequireAuth,
    Path(reference): Path<String>,
    Json(request): Json<IssueRuntimeApprovalRequest>,
) -> ApiResult<(StatusCode, Json<ApiResponse<RuntimeApprovalResponse>>)> {
    let mut tx = state.db.begin().await?;
    let pack = authorize_pack(&mut tx, &state, &user, &reference).await?;
    let identity = user
        .identity_id()
        .map_err(|_| ApiError::Unauthorized("Invalid identity".into()))?;
    let record = RuntimeAuthorityRepository::issue_approval(
        &mut tx,
        pack.id,
        identity,
        request
            .grants
            .into_iter()
            .map(|grant| grant.into_external_approval_grant())
            .collect::<attune_common::Result<Vec<_>>>()?,
        request.expires_at,
    )
    .await?;
    tx.commit().await?;
    audit(
        &state,
        &user,
        event_type::admin::RUNTIME_APPROVAL_ISSUED,
        &pack,
        record.id,
        None,
    );
    Ok((StatusCode::CREATED, Json(ApiResponse::new(record.into()))))
}

#[utoipa::path(post, path = "/api/v1/packs/{pack_ref}/external-approvals/{id}/replace", tag = "permissions",
    params(("pack_ref" = String, Path), ("id" = i64, Path)), request_body = IssueRuntimeApprovalRequest,
    responses((status = 201, description = "New approval issued and predecessor explicitly revoked atomically", body = ApiResponse<RuntimeApprovalResponse>)),
    security(("bearer_auth" = [])))]
pub async fn replace_runtime_approval(
    State(state): State<Arc<AppState>>,
    RequireAuth(user): RequireAuth,
    Path((reference, id)): Path<(String, i64)>,
    Json(request): Json<IssueRuntimeApprovalRequest>,
) -> ApiResult<(StatusCode, Json<ApiResponse<RuntimeApprovalResponse>>)> {
    let mut tx = state.db.begin().await?;
    let pack = authorize_pack(&mut tx, &state, &user, &reference).await?;
    let identity = user
        .identity_id()
        .map_err(|_| ApiError::Unauthorized("Invalid identity".into()))?;
    subject_approval(&mut tx, pack.id, id).await?;
    let record = RuntimeAuthorityRepository::replace_approval(
        &mut tx,
        id,
        identity,
        request
            .grants
            .into_iter()
            .map(|grant| grant.into_external_approval_grant())
            .collect::<attune_common::Result<Vec<_>>>()?,
        request.expires_at,
    )
    .await?;
    tx.commit().await?;
    audit(
        &state,
        &user,
        event_type::admin::RUNTIME_APPROVAL_REPLACED,
        &pack,
        record.id,
        Some(id),
    );
    Ok((StatusCode::CREATED, Json(ApiResponse::new(record.into()))))
}

#[utoipa::path(post, path = "/api/v1/packs/{pack_ref}/external-approvals/{id}/revoke", tag = "permissions",
    params(("pack_ref" = String, Path), ("id" = i64, Path)),
    responses((status = 200, description = "Approval explicitly revoked", body = ApiResponse<RuntimeApprovalResponse>)),
    security(("bearer_auth" = [])))]
pub async fn revoke_runtime_approval(
    State(state): State<Arc<AppState>>,
    RequireAuth(user): RequireAuth,
    Path((reference, id)): Path<(String, i64)>,
) -> ApiResult<Json<ApiResponse<RuntimeApprovalResponse>>> {
    let mut tx = state.db.begin().await?;
    let pack = authorize_pack(&mut tx, &state, &user, &reference).await?;
    let identity = user
        .identity_id()
        .map_err(|_| ApiError::Unauthorized("Invalid identity".into()))?;
    let record = subject_approval(&mut tx, pack.id, id).await?;
    let authority = DelegationAuthority::load_for_share(&mut tx, identity).await?;
    if !authority.manages_permissions() {
        authority.require_grants(&record.grants)?;
    }
    RuntimeAuthorityRepository::revoke_approval(&mut tx, id).await?;
    let record = subject_approval(&mut tx, pack.id, id).await?;
    tx.commit().await?;
    audit(
        &state,
        &user,
        event_type::admin::RUNTIME_APPROVAL_REVOKED,
        &pack,
        id,
        None,
    );
    Ok(Json(ApiResponse::new(record.into())))
}

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route(
            "/packs/{pack_ref}/external-approvals",
            get(list_runtime_approvals).post(issue_runtime_approval),
        )
        .route(
            "/packs/{pack_ref}/external-approvals/{id}",
            get(get_runtime_approval),
        )
        .route(
            "/packs/{pack_ref}/external-approvals/{id}/replace",
            post(replace_runtime_approval),
        )
        .route(
            "/packs/{pack_ref}/external-approvals/{id}/revoke",
            post(revoke_runtime_approval),
        )
}
