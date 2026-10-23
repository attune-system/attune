//! Deployment request rejections use the same JSON envelope as handler failures.
use crate::middleware::ApiError;
use axum::{
    extract::{FromRequest, Request},
    http::StatusCode,
    Json,
};
use serde::de::DeserializeOwned;

pub struct PackJson<T>(pub T);

impl<S, T> FromRequest<S> for PackJson<T>
where
    S: Send + Sync,
    T: DeserializeOwned,
{
    type Rejection = ApiError;
    async fn from_request(request: Request, state: &S) -> Result<Self, Self::Rejection> {
        let Json(value) = Json::<T>::from_request(request, state).await.map_err(|error| {
            let message = "Invalid deployment request JSON. Check Content-Type, JSON syntax, and the types of the request fields.".to_string();
            match error.status() {
                StatusCode::PAYLOAD_TOO_LARGE => ApiError::PayloadTooLarge("Deployment request exceeds the API body-size limit. Reduce the request size.".into()),
                StatusCode::UNPROCESSABLE_ENTITY => ApiError::UnprocessableEntity(message),
                _ => ApiError::BadRequest(message),
            }
        })?;
        Ok(Self(value))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::{to_bytes, Body},
        extract::DefaultBodyLimit,
        routing::post,
        Router,
    };
    use serde::Deserialize;
    use tower::ServiceExt;

    #[derive(Deserialize)]
    struct RequestValue {
        source: String,
    }

    fn app(limit: usize) -> Router {
        Router::new()
            .route(
                "/",
                post(|PackJson(value): PackJson<RequestValue>| async move {
                    Json(serde_json::json!({"empty": value.source.is_empty()}))
                }),
            )
            .layer(DefaultBodyLimit::max(limit))
    }

    #[tokio::test]
    async fn invalid_deployment_json_has_a_safe_error_envelope() {
        let request = Request::builder()
            .method("POST")
            .uri("/")
            .header("content-type", "application/json")
            .body(Body::from(r#"{"source":7,"token":"SECRET_CANARY"}"#))
            .unwrap();
        let response = app(1024).oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(value["error"]
            .as_str()
            .unwrap()
            .contains("deployment request JSON"));
        assert!(!String::from_utf8(body.to_vec())
            .unwrap()
            .contains("SECRET_CANARY"));
    }

    #[tokio::test]
    async fn deployment_body_limit_has_a_json_413_response() {
        let request = Request::builder()
            .method("POST")
            .uri("/")
            .header("content-type", "application/json")
            .body(Body::from(r#"{"source":"demo"}"#))
            .unwrap();
        let response = app(4).oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["code"], "PAYLOAD_TOO_LARGE");
    }
}
