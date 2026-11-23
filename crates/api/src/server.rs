//! Server setup and lifecycle management

use anyhow::Result;
use attune_common::{
    execution_authority::consumer_contract::ConsumerRole,
    repositories::recorded_consumer_observations::{
        RecordedConsumerIdentity, RecordedConsumerObservations,
    },
};
use axum::{extract::Request, middleware, Router};
use std::future::IntoFuture;
use std::sync::Arc;
use std::time::Duration;
use tokio::{net::TcpListener, time::Instant};
use tokio_util::task::AbortOnDropHandle;
use tower::ServiceBuilder;
use tower_http::trace::TraceLayer;
use tracing::info;
use utoipa::OpenApi;
use utoipa_swagger_ui::SwaggerUi;

use crate::{
    middleware::logging::request_log_path,
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
            .merge(routes::candidate_effects::routes())
            .merge(routes::action_routes())
            .merge(routes::execution_options::routes())
            .merge(routes::schema_type_routes())
            .merge(routes::policy_routes())
            .merge(routes::runtime_routes())
            .merge(routes::rule_routes())
            .merge(routes::execution_routes())
            .merge(routes::trigger_routes())
            .merge(routes::inquiry_routes())
            .merge(routes::inquiry_callback_routes())
            .merge(routes::event_routes())
            .merge(routes::key_routes())
            .merge(routes::cache_routes())
            .merge(routes::permission_routes())
            .merge(routes::runtime_approvals::routes())
            .merge(routes::external_identity_mapping_routes())
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
            .merge(routes::managers::routes())
            .merge(routes::sensor_log_routes())
            .merge(routes::trace_routes())
            .merge(routes::info::routes())
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
                    .layer(
                        TraceLayer::new_for_http().make_span_with(|request: &Request| {
                            tracing::info_span!(
                                "http_request",
                                method = %request.method(),
                                path = %request_log_path(request),
                                version = ?request.version(),
                            )
                        }),
                    )
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
        let addr = format!("{}:{}", self.host, self.port);

        info!("Starting server on {}", addr);
        info!("API documentation available at http://{}/docs", addr);

        let listener = TcpListener::bind(&addr).await?;
        info!("Server listening on {}", addr);

        self.run_with_listener(listener).await
    }

    /// Serve on an already-bound listener and complete the shutdown lifecycle.
    pub async fn run_with_listener(self, listener: TcpListener) -> Result<()> {
        let router = self.build_router();

        let shutdown = self.state.execution_log_streams.shutdown_token();
        let recorded_observation = self
            .state
            .recorded_consumer_identity
            .as_ref()
            .map(|identity| {
                let state = Arc::clone(&self.state);
                let identity = Arc::clone(identity);
                AbortOnDropHandle::new(tokio::spawn(async move {
                    Self::observe_recorded_api(&state, &identity).await;
                }))
            });
        let shutdown_signal = shutdown.clone();
        let mut server = Box::pin(
            axum::serve(
                listener,
                router.into_make_service_with_connect_info::<std::net::SocketAddr>(),
            )
            .with_graceful_shutdown(async move { shutdown_signal.cancelled().await })
            .into_future(),
        );

        let mut server_result = None;
        let shutdown_started = tokio::select! {
            result = server.as_mut() => {
                server_result = Some(result);
                None
            }
            _ = shutdown.cancelled() => Some(Instant::now()),
        };

        if let Some(observer) = recorded_observation {
            observer.abort();
            let _ = observer.await;
            if let Some(identity) = &self.state.recorded_consumer_identity {
                Self::publish_recorded_api(&self.state, identity, false).await;
            }
        }

        let release_deadline = if let Some(shutdown_started) = shutdown_started {
            let deadline = shutdown_started
                + Duration::from_secs(self.state.config.server.shutdown_grace_period);
            match tokio::time::timeout_at(deadline, server.as_mut()).await {
                Ok(result) => server_result = Some(result),
                Err(_) => tracing::warn!(
                    grace_period_seconds = self.state.config.server.shutdown_grace_period,
                    "Forcing server shutdown after drain deadline"
                ),
            }
            deadline
        } else {
            Instant::now() + Duration::from_secs(self.state.config.server.shutdown_grace_period)
        };

        self.state.reclamation_cancellation.begin_shutdown();
        drop(server);
        if tokio::time::timeout_at(
            release_deadline,
            self.state.execution_log_streams.drain_releases(),
        )
        .await
        .is_err()
        {
            tracing::warn!("Execution log stream lease release drain timed out");
        }
        // Request drain can consume its entire grace. Cancellation owns a
        // separate bounded teardown window, not an already-expired deadline.
        let cancellation_shutdown = self
            .state
            .reclamation_cancellation
            .shutdown(Instant::now() + Duration::from_secs(10))
            .await;
        if cancellation_shutdown.is_err() {
            tracing::error!("Reclamation cancellation shutdown did not acknowledge cleanup");
        }
        if let Some(result) = server_result {
            result?;
        }
        cancellation_shutdown?;

        Ok(())
    }

    async fn observe_recorded_api(state: &AppState, identity: &RecordedConsumerIdentity) {
        use attune_common::auth::jwt::JwtConfig;

        if !state
            .config
            .security
            .encryption_key
            .as_ref()
            .is_some_and(|key| key.trim().len() >= 32)
            || state.jwt_config.secret.trim().len() < 32
            || state.jwt_config.secret == JwtConfig::default().secret
        {
            tracing::warn!("Recorded API consumers require configured cryptography");
            return;
        }
        // Check real exact-version blob operations through the same backend as
        // the routes. Router construction and an open socket alone are insufficient.
        if state.verify_blob_store().await.is_err() {
            tracing::warn!("Recorded API consumers could not verify their storage backend");
            return;
        }
        let mut ticks = tokio::time::interval(Duration::from_secs(5));
        ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            ticks.tick().await;
            Self::publish_recorded_api(state, identity, true).await;
        }
    }

    async fn publish_recorded_api(
        state: &AppState,
        identity: &RecordedConsumerIdentity,
        installed: bool,
    ) {
        let result = tokio::time::timeout(Duration::from_secs(3), async {
            let mut connection = state.db.acquire().await?;
            let mut ready = installed
                && RecordedConsumerObservations::api_access_available(&mut connection).await?;
            if ready {
                let publisher = state.publisher.read().await.clone();
                ready = if let Some(publisher) = publisher {
                    publisher.channel().await.status().connected()
                } else {
                    false
                };
            }
            identity
                .publish(&mut connection, ready.then_some(ConsumerRole::Api))
                .await
        })
        .await;
        if !matches!(result, Ok(Ok(()))) {
            tracing::warn!("Unable to update the recorded API observation");
        }
    }
}
