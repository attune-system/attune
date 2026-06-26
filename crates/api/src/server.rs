//! Server setup and lifecycle management

use anyhow::Result;
use axum::{middleware, Router};
use std::sync::Arc;
use tokio::net::TcpListener;
use tower::ServiceBuilder;
use tower_http::trace::TraceLayer;
use tracing::info;
use utoipa::OpenApi;
use utoipa_swagger_ui::SwaggerUi;

use crate::{
    middleware::{audit_request, create_cors_layer, log_request},
    openapi::ApiDoc,
    routes,
    state::AppState,
};

/// Server configuration and lifecycle manager
pub struct Server {
    /// Application state
    state: Arc<AppState>,
    /// Server host address
    host: String,
    /// Server port
    port: u16,
}

impl Server {
    /// Create a new server instance
    pub fn new(state: Arc<AppState>) -> Self {
        let host = state.config.server.host.clone();
        let port = state.config.server.port;

        Self { state, host, port }
    }

    /// Get the router for testing purposes
    pub fn router(&self) -> Router {
        self.build_router()
    }

    /// Build the application router with all routes and middleware
    fn build_router(&self) -> Router {
        // API v1 routes (versioned endpoints)
        let api_v1 = Router::new()
            .merge(routes::pack_routes())
            .merge(routes::action_routes())
            .merge(routes::policy_routes())
            .merge(routes::runtime_routes())
            .merge(routes::rule_routes())
            .merge(routes::execution_routes())
            .merge(routes::trigger_routes())
            .merge(routes::inquiry_routes())
            .merge(routes::event_routes())
            .merge(routes::key_routes())
            .merge(routes::permission_routes())
            .merge(routes::worker_routes())
            .merge(routes::retention_routes())
            .merge(routes::work_queue_routes())
            .merge(routes::workflow_routes())
            .merge(routes::webhook_routes())
            .merge(routes::history_routes())
            .merge(routes::analytics_routes())
            .merge(routes::dashboard_routes())
            .merge(routes::artifact_routes())
            .merge(routes::agent_routes())
            .merge(routes::audit_routes())
            .merge(routes::internal_file_routes())
            .merge(routes::sensor_log_routes())
            .merge(routes::trace_routes())
            .with_state(self.state.clone());

        // Auth routes at root level (not versioned for frontend compatibility)
        let auth_routes = routes::auth_routes().with_state(self.state.clone());

        // Health endpoint at root level (operational endpoint, not versioned)
        let health_routes = routes::health_routes().with_state(self.state.clone());

        // Root router with versioning and documentation
        Router::new()
            .merge(SwaggerUi::new("/docs").url("/api-spec/openapi.json", ApiDoc::openapi()))
            .merge(health_routes)
            .nest("/auth", auth_routes)
            .nest("/api/v1", api_v1)
            .layer(
                ServiceBuilder::new()
                    // Add tracing for all requests
                    .layer(TraceLayer::new_for_http())
                    // Add CORS support with configured origins
                    .layer(create_cors_layer(self.state.cors_origins.clone()))
                    // Add custom request logging
                    .layer(middleware::from_fn(log_request)),
            )
            // Record an audit event for every request (skip-list applies).
            // Layered separately because `from_fn_with_state` doesn't
            // compose cleanly inside a ServiceBuilder chain.
            .layer(middleware::from_fn_with_state(
                self.state.clone(),
                audit_request,
            ))
    }

    /// Start the server and listen for requests
    pub async fn run(self) -> Result<()> {
        let router = self.build_router();
        let addr = format!("{}:{}", self.host, self.port);

        info!("Starting server on {}", addr);
        info!("API documentation available at http://{}/docs", addr);

        let listener = TcpListener::bind(&addr).await?;
        info!("Server listening on {}", addr);

        axum::serve(
            listener,
            router.into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .await?;

        Ok(())
    }

    /// Graceful shutdown handler
    pub async fn shutdown(&self) {
        info!("Shutting down server...");
        // Perform any cleanup here
        // - Close database connections
        // - Flush logs
        // - Wait for in-flight requests
        info!("Server shutdown complete");
    }
}

#[cfg(test)]
mod tests {
    #[tokio::test]
    #[ignore] // Ignore until we have test database setup
    async fn test_server_creation() {
        // This test is ignored because it requires a test database pool
        // When implemented, create a test pool and verify server creation
        // let pool = PgPool::connect(&test_db_url).await.unwrap();
        // let state = AppState::new(pool);
        // let server = Server::new(state, "127.0.0.1".to_string(), 8080);
        // assert_eq!(server.host, "127.0.0.1");
        // assert_eq!(server.port, 8080);
    }
}
