//! Application state shared across request handlers

use sqlx::PgPool;
use std::sync::Arc;
use tokio::sync::{broadcast, RwLock};

use crate::auth::jwt::JwtConfig;
use attune_common::{
    audit::AuditEmitter, config::Config, metadata_cache::MetadataCache, mq::Publisher,
};

/// Shared application state
#[derive(Clone)]
pub struct AppState {
    /// Database connection pool
    pub db: PgPool,
    /// JWT configuration
    pub jwt_config: Arc<JwtConfig>,
    /// CORS allowed origins
    pub cors_origins: Vec<String>,
    /// Application configuration
    pub config: Arc<Config>,
    /// Optional message queue publisher (shared, swappable after reconnection)
    pub publisher: Arc<RwLock<Option<Arc<Publisher>>>>,
    /// Broadcast channel for SSE notifications
    pub broadcast_tx: broadcast::Sender<String>,
    /// Audit event emitter (non-blocking; no-op if not configured)
    pub audit_emitter: AuditEmitter,
    /// Optional Valkey-backed metadata cache facade.
    pub metadata_cache: MetadataCache,
}

impl AppState {
    /// Create new application state
    pub fn new(db: PgPool, config: Config) -> Self {
        Self::new_with_audit(db, config, AuditEmitter::noop())
    }

    /// Create new application state with a configured audit emitter.
    pub fn new_with_audit(db: PgPool, config: Config, audit_emitter: AuditEmitter) -> Self {
        Self::new_with_audit_and_metadata_cache(
            db,
            config,
            audit_emitter,
            MetadataCache::disabled(),
        )
    }

    /// Create new application state with audit and metadata cache support.
    pub fn new_with_audit_and_metadata_cache(
        db: PgPool,
        config: Config,
        audit_emitter: AuditEmitter,
        metadata_cache: MetadataCache,
    ) -> Self {
        let jwt_secret = config.security.jwt_secret.clone().unwrap_or_else(|| {
            tracing::warn!(
                "JWT_SECRET not set in config, using default (INSECURE for production!)"
            );
            "insecure_default_secret_change_in_production".to_string()
        });

        let jwt_config = JwtConfig {
            secret: jwt_secret,
            access_token_expiration: config.security.jwt_access_expiration as i64,
            refresh_token_expiration: config.security.jwt_refresh_expiration as i64,
        };

        let cors_origins = config.server.cors_origins.clone();

        // Create broadcast channel for SSE notifications (capacity 1000)
        let (broadcast_tx, _) = broadcast::channel(1000);

        Self {
            db,
            jwt_config: Arc::new(jwt_config),
            cors_origins,
            config: Arc::new(config),
            publisher: Arc::new(RwLock::new(None)),
            broadcast_tx,
            audit_emitter,
            metadata_cache,
        }
    }

    /// Set the message queue publisher (called once at startup or after reconnection)
    pub async fn set_publisher(&self, publisher: Arc<Publisher>) {
        let mut guard = self.publisher.write().await;
        *guard = Some(publisher);
    }

    /// Get a clone of the current publisher, if available
    pub async fn get_publisher(&self) -> Option<Arc<Publisher>> {
        self.publisher.read().await.clone()
    }
}

/// Type alias for Arc-wrapped application state
/// Used by Axum handlers
pub type SharedState = Arc<AppState>;
