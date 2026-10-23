//! Authenticated, read-only schema discovery grants no access to Keys,
//! decrypted values, or action execution.
use std::sync::Arc;

use crate::{
    auth::middleware::RequireAuth,
    dto::{
        common::{PaginatedResponse, PaginationSearchParams},
        schema_types::{SchemaContractResponse, SchemaPreviewRequest, SchemaTypeResponse},
        ApiResponse,
    },
    middleware::error::{ApiError, ApiResult},
    state::AppState,
};
use attune_common::{
    repositories::{schema_contract::SchemaContractRepository, schema_type::SchemaTypeRepository},
    value_schema::{SchemaContext, SourceIdentity},
};
use axum::{
    extract::{Path, Query, State},
    routing::{get, post},
    Json, Router,
};

#[utoipa::path(get, path = "/api/v1/schema-types", tag = "schema-types", params(PaginationSearchParams),
    responses((status = 200, description = "Installed, non-sensitive type metadata", body = PaginatedResponse<SchemaTypeResponse>)), security(("bearer_auth" = [])))]
pub async fn list_schema_types(
    State(state): State<Arc<AppState>>,
    RequireAuth(_user): RequireAuth,
    Query(query): Query<PaginationSearchParams>,
) -> ApiResult<Json<PaginatedResponse<SchemaTypeResponse>>> {
    list(&state, None, query).await
}

#[utoipa::path(get, path = "/api/v1/packs/{pack_ref}/schema-types", tag = "schema-types", params(("pack_ref" = String, Path), PaginationSearchParams),
    responses((status = 200, description = "Installed type metadata for one pack", body = PaginatedResponse<SchemaTypeResponse>)), security(("bearer_auth" = [])))]
pub async fn list_pack_schema_types(
    State(state): State<Arc<AppState>>,
    RequireAuth(_user): RequireAuth,
    Path(pack_ref): Path<String>,
    Query(query): Query<PaginationSearchParams>,
) -> ApiResult<Json<PaginatedResponse<SchemaTypeResponse>>> {
    list(&state, Some(&pack_ref), query).await
}

async fn list(
    state: &Arc<AppState>,
    pack_ref: Option<&str>,
    query: PaginationSearchParams,
) -> ApiResult<Json<PaginatedResponse<SchemaTypeResponse>>> {
    let mut pagination = query.pagination();
    if pagination.page == 0 || pagination.page_size == 0 {
        return Err(ApiError::BadRequest(
            "Pagination page and page_size must be positive".into(),
        ));
    }
    pagination.page_size = pagination.limit();
    let offset = u64::from(pagination.page - 1) * u64::from(pagination.page_size);
    let (rows, total) = SchemaTypeRepository::list_installed(
        &mut *state.db.acquire().await?,
        pack_ref,
        query.q.as_deref(),
        pagination.limit() as i64,
        offset as i64,
    )
    .await?;
    Ok(Json(PaginatedResponse::new(
        rows.into_iter().map(Into::into).collect(),
        &pagination,
        total,
    )))
}

#[utoipa::path(get, path = "/api/v1/schema-types/{ref}", tag = "schema-types", params(("ref" = String, Path)),
    responses((status = 200, description = "Authored named type definition", body = ApiResponse<SchemaTypeResponse>), (status = 404, description = "Type not installed")), security(("bearer_auth" = [])))]
pub async fn get_schema_type(
    State(state): State<Arc<AppState>>,
    RequireAuth(_user): RequireAuth,
    Path(type_ref): Path<String>,
) -> ApiResult<Json<ApiResponse<SchemaTypeResponse>>> {
    let definition = SchemaTypeRepository::find_installed(&state.db, &type_ref)
        .await?
        .ok_or_else(|| ApiError::NotFound("Named schema is not installed".into()))?;
    Ok(Json(ApiResponse::new(definition.into())))
}

#[utoipa::path(get, path = "/api/v1/schema-types/{ref}/inspect", tag = "schema-types", params(("ref" = String, Path)),
    responses((status = 200, description = "Resolved named-type rendering contract", body = ApiResponse<SchemaContractResponse>)), security(("bearer_auth" = [])))]
pub async fn inspect_schema_type(
    State(state): State<Arc<AppState>>,
    RequireAuth(_user): RequireAuth,
    Path(type_ref): Path<String>,
) -> ApiResult<Json<ApiResponse<SchemaContractResponse>>> {
    let mut transaction = state.db.begin().await?;
    SchemaTypeRepository::lock_selection(&mut transaction).await?;
    let revision = SchemaTypeRepository::active_revision(&mut transaction, &type_ref)
        .await?
        .filter(|revision| revision.active)
        .ok_or_else(|| ApiError::NotFound("Named schema is not installed".into()))?;
    let definition: attune_common::schema_type::SchemaTypeDefinition =
        serde_json::from_value(revision.definition).map_err(|_| {
            ApiError::InternalServerError("Invalid immutable type definition".into())
        })?;
    let contract = SchemaContractRepository::materialize(
        &mut transaction,
        SourceIdentity::Released {
            component_ref: type_ref,
            release_id: revision.release_id,
            release_digest: revision.release_digest,
        },
        SchemaContext::SingleValue,
        &definition.schema,
    )
    .await?;
    let response = SchemaContractResponse::from_contract(contract)?;
    transaction.commit().await?;
    Ok(Json(ApiResponse::new(response)))
}

#[utoipa::path(post, path = "/api/v1/schema-types/preview", tag = "schema-types", request_body = SchemaPreviewRequest,
    responses((status = 200, description = "Authoring rendering preview; not an authoritative assignment", body = ApiResponse<SchemaContractResponse>), (status = 400, description = "Invalid schema")), security(("bearer_auth" = [])))]
pub async fn preview_schema(
    State(state): State<Arc<AppState>>,
    RequireAuth(_user): RequireAuth,
    Json(request): Json<SchemaPreviewRequest>,
) -> ApiResult<Json<ApiResponse<SchemaContractResponse>>> {
    let mut transaction = state.db.begin().await?;
    let contract = SchemaContractRepository::materialize_assignment(
        &mut transaction,
        SourceIdentity::Authoring {
            component_ref: "schema-preview".into(),
        },
        request.context.into(),
        &request.schema,
    )
    .await?;
    let response = SchemaContractResponse::from_contract(contract)?;
    transaction.commit().await?;
    Ok(Json(ApiResponse::new(response)))
}

pub fn routes() -> Router<Arc<AppState>> {
    Router::new()
        .route("/schema-types", get(list_schema_types))
        .route(
            "/packs/{pack_ref}/schema-types",
            get(list_pack_schema_types),
        )
        .route("/schema-types/preview", post(preview_schema))
        .route("/schema-types/{ref}", get(get_schema_type))
        .route("/schema-types/{ref}/inspect", get(inspect_schema_type))
        .layer(axum::Extension(
            crate::auth::middleware::RecordedRuntimeConsumer::SchemaMetadata,
        ))
}
