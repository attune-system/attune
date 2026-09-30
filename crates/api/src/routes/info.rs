//! Public build identity. This endpoint does not expose configuration or query the database.

use crate::{
    dto::{common::ApiResponse, info::InfoResponse},
    state::AppState,
};
use axum::{http::header::CACHE_CONTROL, response::IntoResponse, routing::get, Json, Router};
use std::sync::Arc;

#[utoipa::path(
    get,
    path = "/api/v1/info",
    tag = "info",
    responses((status = 200, description = "Build identity of the responding API process", body = ApiResponse<InfoResponse>))
)]
pub async fn get_info() -> impl IntoResponse {
    (
        [(CACHE_CONTROL, "no-store")],
        Json(ApiResponse::new(InfoResponse::current())),
    )
}

pub fn routes() -> Router<Arc<AppState>> {
    Router::new().route("/info", get(get_info))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::{to_bytes, Body},
        http::Request,
    };
    use tower::ServiceExt;

    #[tokio::test]
    async fn info_reports_the_compiled_build_without_authentication_or_database_state() {
        let response = Router::new()
            .route("/api/v1/info", get(get_info))
            .oneshot(
                Request::builder()
                    .uri("/api/v1/info")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), axum::http::StatusCode::OK);
        assert_eq!(response.headers()[CACHE_CONTROL], "no-store");
        let body: serde_json::Value =
            serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
        assert_eq!(
            body["data"],
            serde_json::to_value(InfoResponse::current()).unwrap()
        );
    }
}
