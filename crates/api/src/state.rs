//! Application state shared across request handlers

use sqlx::PgPool;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{broadcast, Mutex, OwnedMutexGuard, RwLock};

use crate::execution_log_streams::ExecutionLogStreams;
use crate::log_stream_wakeups::LogStreamWakeups;
use crate::{auth::jwt::JwtConfig, authz::AuthorizationService};
use attune_common::{
    audit::AuditEmitter,
    blob_store::{BlobStore, FilesystemBlobStore, GcsBlobStore, S3BlobStore},
    config::{BlobStorageConfig, Config},
    mq::Publisher,
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
    /// Local wakeups for readers interested in a specific execution log stream.
    pub log_stream_wakeups: LogStreamWakeups,
    /// Admission control, shutdown signaling, and metrics for execution log SSE streams.
    pub execution_log_streams: ExecutionLogStreams,
    /// Audit event emitter (non-blocking; no-op if not configured)
    pub audit_emitter: AuditEmitter,
    /// Durable immutable storage. Only the API receives provider credentials.
    pub blob_store: Arc<dyn BlobStore>,
    pack_projection_locks: Arc<Mutex<HashMap<String, Arc<Mutex<()>>>>>,
}

impl AppState {
    /// Create new application state
    pub fn new(db: PgPool, config: Config) -> Self {
        Self::new_with_audit(db, config, AuditEmitter::noop())
    }

    /// Create new application state with a configured audit emitter.
    pub fn new_with_audit(db: PgPool, config: Config, audit_emitter: AuditEmitter) -> Self {
        let blob_store: Arc<dyn BlobStore> = match &config.storage {
            BlobStorageConfig::Filesystem { root } => {
                Arc::new(FilesystemBlobStore::new(root).unwrap_or_else(|error| {
                    panic!("failed to initialize filesystem blob storage: {error}")
                }))
            }
            BlobStorageConfig::S3 {
                bucket,
                region,
                prefix,
                endpoint,
                kms_key,
            } => Arc::new(
                S3BlobStore::new(
                    bucket,
                    region,
                    prefix,
                    endpoint.as_deref(),
                    kms_key.as_deref(),
                )
                .unwrap_or_else(|error| panic!("failed to initialize S3 blob storage: {error}")),
            ),
            BlobStorageConfig::Gcs {
                bucket,
                prefix,
                endpoint,
            } => Arc::new(
                GcsBlobStore::new(bucket, prefix, endpoint.as_deref()).unwrap_or_else(|error| {
                    panic!("failed to initialize GCS blob storage: {error}")
                }),
            ),
        };
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

        let execution_log_streams = ExecutionLogStreams::new(
            config.server.execution_log_stream_global_limit,
            config.server.execution_log_stream_per_identity_limit,
        );

        Self {
            db,
            jwt_config: Arc::new(jwt_config),
            cors_origins,
            config: Arc::new(config),
            publisher: Arc::new(RwLock::new(None)),
            broadcast_tx,
            log_stream_wakeups: LogStreamWakeups::default(),
            execution_log_streams,
            audit_emitter,
            blob_store,
            pack_projection_locks: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Create an authorization service connected to the application's audit writer.
    pub fn authorization_service(&self) -> AuthorizationService {
        AuthorizationService::new_with_audit(self.db.clone(), self.audit_emitter.clone())
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

    /// Serialize database activation and local projection publication per pack.
    pub async fn lock_pack_projection(&self, pack_ref: &str) -> OwnedMutexGuard<()> {
        let lock = self
            .pack_projection_locks
            .lock()
            .await
            .entry(pack_ref.to_string())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone();
        lock.lock_owned().await
    }
}

/// Type alias for Arc-wrapped application state
/// Used by Axum handlers
pub type SharedState = Arc<AppState>;
