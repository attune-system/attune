//! Request/Response logging middleware

use axum::{extract::MatchedPath, extract::Request, middleware::Next, response::Response};
use std::time::Instant;
use tracing::{info, warn};

/// Middleware for logging HTTP requests and responses
pub async fn log_request(req: Request, next: Next) -> Response {
    let method = req.method().clone();
    let path = request_log_path(&req);
    let version = req.version();

    let start = Instant::now();

    info!(
        method = %method,
        path = %path,
        version = ?version,
        "request started"
    );

    let response = next.run(req).await;

    let duration = start.elapsed();
    let status = response.status();

    if status.is_success() {
        info!(
            method = %method,
            path = %path,
            status = %status.as_u16(),
            duration_ms = %duration.as_millis(),
            "request completed"
        );
    } else if status.is_client_error() {
        warn!(
            method = %method,
            path = %path,
            status = %status.as_u16(),
            duration_ms = %duration.as_millis(),
            "request failed (client error)"
        );
    } else if status.is_server_error() {
        warn!(
            method = %method,
            path = %path,
            status = %status.as_u16(),
            duration_ms = %duration.as_millis(),
            "request failed (server error)"
        );
    }

    response
}

pub(crate) fn request_log_path(req: &Request) -> String {
    if let Some(matched_path) = req.extensions().get::<MatchedPath>() {
        return matched_path.as_str().to_string();
    }

    "<unmatched>".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;

    #[test]
    fn request_log_path_redacts_unmatched_paths_and_query_strings() {
        let request = Request::builder()
            .uri("/api/v1/rules?page=1&token=query-secret")
            .body(Body::empty())
            .unwrap();

        assert_eq!(request_log_path(&request), "<unmatched>");
    }

    #[test]
    fn request_log_path_redacts_every_unmatched_concrete_path() {
        let request = Request::builder()
            .uri("/not/a/route/concrete-secret?signature=query-secret")
            .body(Body::empty())
            .unwrap();

        let path = request_log_path(&request);
        assert_eq!(path, "<unmatched>");
        assert!(!path.contains("concrete-secret"));
        assert!(!path.contains("query-secret"));
    }

    #[tokio::test]
    async fn request_log_path_preserves_recognized_route_templates() {
        use axum::{middleware, response::IntoResponse, routing::get, Router};
        use tower::ServiceExt;

        async fn path_from_middleware(request: Request, _next: Next) -> Response {
            request_log_path(&request).into_response()
        }

        let response = Router::new()
            .route("/things/{thing_id}", get(|| async {}))
            .layer(middleware::from_fn(path_from_middleware))
            .oneshot(
                Request::builder()
                    .uri("/things/concrete-secret")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();

        assert_eq!(body, "/things/{thing_id}");
    }
}
