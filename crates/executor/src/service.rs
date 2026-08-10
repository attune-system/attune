//! Executor Service - Core orchestration and execution management
//!
//! The ExecutorService is the central component that:
//! - Processes enforcement messages from triggered rules
//! - Schedules executions to workers
//! - Manages execution lifecycle and state transitions
//! - Enforces execution policies (rate limiting, concurrency)
//! - Orchestrates workflows (parent-child executions)
//! - Handles human-in-the-loop inquiries

use anyhow::Result;
use attune_common::{
    config::Config,
    db::Database,
    mq::{
        ActionChangedPayload, Connection, Consumer, MessageEnvelope, MessageQueueConfig, Publisher,
    },
    system_alert::{emit_core_alert, SystemAlert},
};
use sqlx::PgPool;
use std::sync::Arc;
use tokio::task::JoinHandle;
use tracing::{error, info, warn};

use crate::completion_listener::CompletionListener;
use crate::dead_letter_handler::{create_dlq_consumer_config, DeadLetterHandler};
use crate::enforcement_processor::EnforcementProcessor;
use crate::event_processor::EventProcessor;
use crate::execution_manager::ExecutionManager;
use crate::inquiry_handler::InquiryHandler;
use crate::policy_enforcer::PolicyEnforcer;
use crate::queue_dispatcher::WorkQueueDispatcher;
use crate::queue_manager::{ExecutionQueueManager, QueueConfig};
use crate::scheduler::{ExecutionScheduler, SchedulerMetadataCaches};
use crate::timeout_monitor::{ExecutionTimeoutMonitor, TimeoutMonitorConfig};

/// Main executor service that orchestrates execution processing
#[derive(Clone)]
pub struct ExecutorService {
    /// Shared internal state
    inner: Arc<ExecutorServiceInner>,
}

/// Internal state for the executor service
struct ExecutorServiceInner {
    /// Database connection pool
    pool: PgPool,

    /// Configuration
    config: Arc<Config>,

    /// Message queue connection
    mq_connection: Arc<Connection>,

    /// Message queue publisher
    /// Publisher for sending messages
    publisher: Arc<Publisher>,

    /// Queue name for consumers
    #[allow(dead_code)]
    queue_name: String,

    /// Message queue configuration
    mq_config: Arc<MessageQueueConfig>,

    /// Policy enforcer for execution policies
    policy_enforcer: Arc<PolicyEnforcer>,

    /// Queue manager for FIFO execution ordering
    queue_manager: Arc<ExecutionQueueManager>,

    /// Metadata cached within this database-backed service instance.
    scheduler_metadata_caches: Arc<SchedulerMetadataCaches>,

    /// Service shutdown signal
    shutdown_tx: tokio::sync::broadcast::Sender<()>,
}

impl ExecutorService {
    async fn run_metadata_invalidation_consumer(
        consumer: Arc<Consumer>,
        metadata_caches: Arc<SchedulerMetadataCaches>,
    ) -> Result<()> {
        info!("Starting metadata invalidation consumer");
        consumer
            .consume_with_handler(move |envelope: MessageEnvelope<ActionChangedPayload>| {
                let metadata_caches = metadata_caches.clone();
                async move {
                    let payload = envelope.payload;
                    metadata_caches
                        .invalidate_action(
                            Some(payload.action_id),
                            Some(payload.action_ref.as_str()),
                        )
                        .await;
                    Ok(())
                }
            })
            .await?;
        Ok(())
    }

    /// Create a new executor service
    pub async fn new(config: Config) -> Result<Self> {
        info!("Initializing Executor Service");

        // Initialize database
        let db = Database::new(&config.database).await?;
        let pool = db.pool().clone();
        info!("Database connection established");

        // Get message queue URL
        let mq_url = config
            .message_queue
            .as_ref()
            .map(|mq| mq.url.as_str())
            .ok_or_else(|| anyhow::anyhow!("Message queue configuration is required"))?;

        // Initialize message queue connection
        let mq_connection = Connection::connect(mq_url).await?;
        info!("Message queue connection established");

        // Setup common message queue infrastructure (exchanges and DLX)
        let mq_config = MessageQueueConfig::default();
        match mq_connection.setup_common_infrastructure(&mq_config).await {
            Ok(_) => info!("Common message queue infrastructure setup completed"),
            Err(e) => {
                warn!(
                    "Failed to setup common MQ infrastructure (may already exist): {}",
                    e
                );
            }
        }

        // Setup executor-specific queues and bindings
        match mq_connection
            .setup_executor_infrastructure(&mq_config)
            .await
        {
            Ok(_) => info!("Executor message queue infrastructure setup completed"),
            Err(e) => {
                warn!(
                    "Failed to setup executor MQ infrastructure (may already exist): {}",
                    e
                );
            }
        }

        // Get queue names from MqConfig
        let enforcements_queue = mq_config.rabbitmq.queues.enforcements.name.clone();
        let execution_requests_queue = mq_config.rabbitmq.queues.execution_requests.name.clone();
        let execution_status_queue = mq_config.rabbitmq.queues.execution_status.name.clone();
        let exchange_name = mq_config.rabbitmq.exchanges.executions.name.clone();

        // Initialize message queue publisher
        let publisher = Publisher::new(
            &mq_connection,
            attune_common::mq::PublisherConfig {
                confirm_publish: true,
                timeout_secs: 30,
                exchange: exchange_name,
            },
        )
        .await?;
        info!("Message queue publisher initialized");

        info!(
            "Queue names - Enforcements: {}, Execution Requests: {}, Execution Status: {}",
            enforcements_queue, execution_requests_queue, execution_status_queue
        );

        // Create shutdown channel
        let (shutdown_tx, _) = tokio::sync::broadcast::channel(1);

        // Initialize queue manager with default configuration and database pool
        let queue_config = QueueConfig::default();
        let queue_manager = Arc::new(ExecutionQueueManager::with_db_pool(
            queue_config,
            pool.clone(),
        ));
        info!("Queue manager initialized with database persistence");

        // Initialize policy enforcer with queue manager
        let policy_enforcer = Arc::new(PolicyEnforcer::with_queue_manager(
            pool.clone(),
            queue_manager.clone(),
        ));
        info!("Policy enforcer initialized with queue manager");

        let inner = ExecutorServiceInner {
            pool,
            config: Arc::new(config),
            mq_connection: Arc::new(mq_connection),
            publisher: Arc::new(publisher),
            queue_name: execution_requests_queue.clone(), // Keep for backward compatibility
            policy_enforcer,
            queue_manager,
            scheduler_metadata_caches: Arc::new(SchedulerMetadataCaches::new()),
            shutdown_tx,
            mq_config: Arc::new(mq_config),
        };

        Ok(Self {
            inner: Arc::new(inner),
        })
    }

    /// Start the executor service
    pub async fn start(&self) -> Result<()> {
        info!("Starting Executor Service");

        // Spawn message consumers
        let mut handles: Vec<JoinHandle<Result<()>>> = Vec::new();

        // Start event processor with its own consumer
        info!("Starting event processor...");
        let events_queue = self
            .inner
            .mq_config
            .rabbitmq
            .queues
            .executor_events
            .name
            .clone();
        let event_consumer = Consumer::new(
            &self.inner.mq_connection,
            attune_common::mq::ConsumerConfig {
                queue: events_queue,
                tag: "executor.event".to_string(),
                prefetch_count: 10,
                auto_ack: false,
                exclusive: false,
            },
        )
        .await?;
        let event_processor = EventProcessor::new(
            self.inner.pool.clone(),
            self.inner.publisher.clone(),
            Arc::new(event_consumer),
            self.inner.config.security.encryption_key.clone(),
        );
        handles.push(tokio::spawn(async move { event_processor.start().await }));

        // Start completion listener with its own consumer
        info!("Starting completion listener...");
        let execution_completed_queue = self
            .inner
            .mq_config
            .rabbitmq
            .queues
            .execution_completed
            .name
            .clone();
        let completion_consumer = Consumer::new(
            &self.inner.mq_connection,
            attune_common::mq::ConsumerConfig {
                queue: execution_completed_queue,
                tag: "executor.completion".to_string(),
                prefetch_count: 10,
                auto_ack: false,
                exclusive: false,
            },
        )
        .await?;
        let completion_listener = CompletionListener::new(
            self.inner.pool.clone(),
            Arc::new(completion_consumer),
            self.inner.publisher.clone(),
            self.inner.queue_manager.clone(),
            self.inner.config.artifacts_dir.clone(),
            self.inner.config.security.encryption_key.clone(),
            self.inner.scheduler_metadata_caches.clone(),
        );
        handles.push(tokio::spawn(
            async move { completion_listener.start().await },
        ));

        // Start enforcement processor with its own consumer
        info!("Starting enforcement processor...");
        let enforcements_queue = self
            .inner
            .mq_config
            .rabbitmq
            .queues
            .enforcements
            .name
            .clone();
        let enforcement_consumer = Consumer::new(
            &self.inner.mq_connection,
            attune_common::mq::ConsumerConfig {
                queue: enforcements_queue,
                tag: "executor.enforcement".to_string(),
                prefetch_count: 10,
                auto_ack: false,
                exclusive: false,
            },
        )
        .await?;
        let enforcement_processor = EnforcementProcessor::new(
            self.inner.pool.clone(),
            self.inner.publisher.clone(),
            Arc::new(enforcement_consumer),
            self.inner.policy_enforcer.clone(),
            self.inner.queue_manager.clone(),
        );
        handles.push(tokio::spawn(
            async move { enforcement_processor.start().await },
        ));

        // Start execution scheduler with its own consumer
        info!("Starting execution scheduler...");
        let execution_requests_queue = self
            .inner
            .mq_config
            .rabbitmq
            .queues
            .execution_requests
            .name
            .clone();
        let scheduler_consumer = Consumer::new(
            &self.inner.mq_connection,
            attune_common::mq::ConsumerConfig {
                queue: execution_requests_queue,
                tag: "executor.scheduler".to_string(),
                prefetch_count: 10,
                auto_ack: false,
                exclusive: false,
            },
        )
        .await?;
        let scheduler = ExecutionScheduler::new(
            self.inner.pool.clone(),
            self.inner.publisher.clone(),
            Arc::new(scheduler_consumer),
            self.inner.policy_enforcer.clone(),
            self.inner.config.artifacts_dir.clone(),
            self.inner.config.security.encryption_key.clone(),
            self.inner.scheduler_metadata_caches.clone(),
        );
        handles.push(tokio::spawn(async move { scheduler.start().await }));

        // Start execution manager with its own consumer
        info!("Starting execution manager...");
        let execution_status_queue = self
            .inner
            .mq_config
            .rabbitmq
            .queues
            .execution_status
            .name
            .clone();
        let manager_consumer = Consumer::new(
            &self.inner.mq_connection,
            attune_common::mq::ConsumerConfig {
                queue: execution_status_queue,
                tag: "executor.manager".to_string(),
                prefetch_count: 10,
                auto_ack: false,
                exclusive: false,
            },
        )
        .await?;
        let execution_manager = ExecutionManager::new(
            self.inner.pool.clone(),
            self.inner.publisher.clone(),
            Arc::new(manager_consumer),
        );
        handles.push(tokio::spawn(async move { execution_manager.start().await }));

        // Start inquiry handler with its own consumer
        info!("Starting inquiry handler...");
        let inquiry_response_queue = self
            .inner
            .mq_config
            .rabbitmq
            .queues
            .inquiry_responses
            .name
            .clone();
        let inquiry_consumer = Consumer::new(
            &self.inner.mq_connection,
            attune_common::mq::ConsumerConfig {
                queue: inquiry_response_queue,
                tag: "executor.inquiry".to_string(),
                prefetch_count: 10,
                auto_ack: false,
                exclusive: false,
            },
        )
        .await?;
        let inquiry_handler = InquiryHandler::new(
            self.inner.pool.clone(),
            self.inner.publisher.clone(),
            Arc::new(inquiry_consumer),
        );
        handles.push(tokio::spawn(async move { inquiry_handler.start().await }));

        // Start inquiry timeout checker
        info!("Starting inquiry timeout checker...");
        let timeout_pool = self.inner.pool.clone();
        let timeout_publisher = self.inner.publisher.clone();
        handles.push(tokio::spawn(async move {
            InquiryHandler::timeout_check_loop(timeout_pool, timeout_publisher, 1).await;
            Ok(())
        }));

        // Start metadata invalidation consumer (per-instance ephemeral queue)
        info!("Starting metadata invalidation consumer...");
        let metadata_consumer = self
            .inner
            .mq_connection
            .create_ephemeral_topic_consumer(
                "attune.metadata",
                &["metadata.action.changed"],
                "executor.metadata.invalidation",
                32,
            )
            .await?;
        handles.push(tokio::spawn(Self::run_metadata_invalidation_consumer(
            Arc::new(metadata_consumer),
            self.inner.scheduler_metadata_caches.clone(),
        )));

        // Start worker heartbeat monitor
        info!("Starting worker heartbeat monitor...");
        let worker_pool = self.inner.pool.clone();
        let worker_publisher = self.inner.publisher.clone();
        handles.push(tokio::spawn(async move {
            Self::worker_heartbeat_monitor_loop(worker_pool, worker_publisher, 60).await;
            Ok(())
        }));

        // Start execution timeout monitor
        info!("Starting execution timeout monitor...");
        let timeout_config = TimeoutMonitorConfig {
            scheduled_timeout: std::time::Duration::from_secs(
                self.inner
                    .config
                    .executor
                    .as_ref()
                    .and_then(|e| e.scheduled_timeout)
                    .unwrap_or(300), // Default: 5 minutes
            ),
            check_interval: std::time::Duration::from_secs(
                self.inner
                    .config
                    .executor
                    .as_ref()
                    .and_then(|e| e.timeout_check_interval)
                    .unwrap_or(60), // Default: 1 minute
            ),
            enabled: self
                .inner
                .config
                .executor
                .as_ref()
                .and_then(|e| e.enable_timeout_monitor)
                .unwrap_or(true), // Default: enabled
        };
        let timeout_monitor = Arc::new(ExecutionTimeoutMonitor::new(
            self.inner.pool.clone(),
            self.inner.publisher.clone(),
            timeout_config,
        ));
        handles.push(tokio::spawn(async move { timeout_monitor.start().await }));

        // Start work queue dispatcher
        info!("Starting work queue dispatcher...");
        let queue_dispatcher = WorkQueueDispatcher::new(
            self.inner.pool.clone(),
            self.inner.publisher.clone(),
            self.inner.config.security.encryption_key.clone(),
        );
        handles.push(tokio::spawn(async move { queue_dispatcher.start().await }));

        // Start dead letter handler (if DLQ is enabled)
        if self.inner.mq_config.rabbitmq.dead_letter.enabled {
            info!("Starting dead letter handler...");
            let dlq_name = format!(
                "{}.queue",
                self.inner.mq_config.rabbitmq.dead_letter.exchange
            );
            let dlq_consumer = Consumer::new(
                &self.inner.mq_connection,
                create_dlq_consumer_config(&dlq_name, "executor.dlq"),
            )
            .await?;
            let dlq_handler = Arc::new(
                DeadLetterHandler::new(Arc::new(self.inner.pool.clone()), dlq_consumer)
                    .await
                    .map_err(|e| anyhow::anyhow!("Failed to create DLQ handler: {}", e))?,
            );
            handles.push(tokio::spawn(async move {
                dlq_handler
                    .start()
                    .await
                    .map_err(|e| anyhow::anyhow!("DLQ handler error: {}", e))
            }));
        } else {
            info!("Dead letter queue is disabled, skipping DLQ handler");
        }

        info!("Executor Service started successfully");
        info!("All processors are listening for messages...");

        // Wait for shutdown signal
        let mut shutdown_rx = self.inner.shutdown_tx.subscribe();
        tokio::select! {
            _ = shutdown_rx.recv() => {
                info!("Shutdown signal received");
            }
            result = Self::wait_for_tasks(handles) => {
                match result {
                    Ok(_) => info!("All tasks completed"),
                    Err(e) => error!("Task error: {}", e),
                }
            }
        }

        Ok(())
    }

    /// Stop the executor service
    pub async fn stop(&self) -> Result<()> {
        info!("Stopping Executor Service");

        // Send shutdown signal
        let _ = self.inner.shutdown_tx.send(());

        // Close message queue connection (will close publisher and consumer)
        self.inner.mq_connection.close().await?;

        // Close database connections
        self.inner.pool.close().await;

        info!("Executor Service stopped");

        Ok(())
    }

    /// Worker heartbeat monitor loop
    ///
    /// Periodically checks for stale workers and marks them as inactive
    async fn worker_heartbeat_monitor_loop(
        pool: PgPool,
        publisher: Arc<Publisher>,
        interval_secs: u64,
    ) {
        use attune_common::models::enums::WorkerStatus;
        use attune_common::repositories::{
            runtime::{UpdateWorkerInput, WorkerRepository},
            Update,
        };
        use chrono::Utc;
        use std::time::Duration;

        let check_interval = Duration::from_secs(interval_secs);

        // Heartbeat staleness threshold: 3x the expected interval (90 seconds)
        // NOTE: These constants MUST match DEFAULT_HEARTBEAT_INTERVAL and
        // HEARTBEAT_STALENESS_MULTIPLIER in scheduler.rs to ensure consistency
        const HEARTBEAT_INTERVAL: u64 = 30;
        const STALENESS_MULTIPLIER: u64 = 3;
        let max_age_secs = HEARTBEAT_INTERVAL * STALENESS_MULTIPLIER;

        info!(
            "Worker heartbeat monitor started (check interval: {}s, staleness threshold: {}s)",
            interval_secs, max_age_secs
        );

        loop {
            tokio::time::sleep(check_interval).await;

            // Get all active workers
            match WorkerRepository::find_by_status(&pool, WorkerStatus::Active).await {
                Ok(workers) => {
                    let now = Utc::now();
                    let mut deactivated_count = 0;

                    for worker in workers {
                        // Check if worker has a heartbeat
                        let Some(last_heartbeat) = worker.last_heartbeat else {
                            warn!(
                                "Worker {} (ID: {}) has no heartbeat, marking as inactive",
                                worker.name, worker.id
                            );

                            if let Err(e) = WorkerRepository::update(
                                &pool,
                                worker.id,
                                UpdateWorkerInput {
                                    status: Some(WorkerStatus::Inactive),
                                    ..Default::default()
                                },
                            )
                            .await
                            {
                                error!(
                                    "Failed to deactivate worker {} (no heartbeat): {}",
                                    worker.name, e
                                );
                            } else {
                                deactivated_count += 1;
                                if !worker.cordoned {
                                    let alert = SystemAlert {
                                        severity: "error".to_string(),
                                        category: "worker_health".to_string(),
                                        failure_type: "worker_missing_heartbeat".to_string(),
                                        component_type: match worker.worker_role {
                                            attune_common::models::WorkerRole::Action => {
                                                "worker".to_string()
                                            }
                                            attune_common::models::WorkerRole::Sensor => {
                                                "sensor_worker".to_string()
                                            }
                                        },
                                        component_id: Some(worker.id),
                                        component_ref: Some(worker.name.clone()),
                                        worker_role: Some(
                                            format!("{:?}", worker.worker_role).to_lowercase(),
                                        ),
                                        observed_at: Utc::now(),
                                        summary: format!(
                                            "Worker '{}' has no heartbeat and was marked inactive",
                                            worker.name
                                        ),
                                        details: serde_json::json!({
                                            "reason": "missing_heartbeat",
                                            "worker_id": worker.id,
                                            "worker_name": worker.name,
                                            "worker_role": worker.worker_role,
                                        }),
                                        correlation_id: Some(format!(
                                            "worker:{}:missing_heartbeat",
                                            worker.id
                                        )),
                                    };
                                    if let Err(e) =
                                        emit_core_alert(&pool, Some(&publisher), alert).await
                                    {
                                        warn!(
                                            "Failed to emit missing-heartbeat alert for worker {}: {}",
                                            worker.id, e
                                        );
                                    }
                                }
                            }
                            continue;
                        };

                        // Check if heartbeat is stale
                        let age = now.signed_duration_since(last_heartbeat);
                        let age_secs = age.num_seconds();

                        if age_secs > max_age_secs as i64 {
                            warn!(
                                "Worker {} (ID: {}) heartbeat is stale ({}s old), marking as inactive",
                                worker.name, worker.id, age_secs
                            );

                            if let Err(e) = WorkerRepository::update(
                                &pool,
                                worker.id,
                                UpdateWorkerInput {
                                    status: Some(WorkerStatus::Inactive),
                                    ..Default::default()
                                },
                            )
                            .await
                            {
                                error!(
                                    "Failed to deactivate worker {} (stale heartbeat): {}",
                                    worker.name, e
                                );
                            } else {
                                deactivated_count += 1;
                                if !worker.cordoned {
                                    let alert = SystemAlert {
                                        severity: "error".to_string(),
                                        category: "worker_health".to_string(),
                                        failure_type: "worker_stale_heartbeat".to_string(),
                                        component_type: match worker.worker_role {
                                            attune_common::models::WorkerRole::Action => {
                                                "worker".to_string()
                                            }
                                            attune_common::models::WorkerRole::Sensor => {
                                                "sensor_worker".to_string()
                                            }
                                        },
                                        component_id: Some(worker.id),
                                        component_ref: Some(worker.name.clone()),
                                        worker_role: Some(
                                            format!("{:?}", worker.worker_role).to_lowercase(),
                                        ),
                                        observed_at: Utc::now(),
                                        summary: format!(
                                            "Worker '{}' heartbeat is stale and was marked inactive",
                                            worker.name
                                        ),
                                        details: serde_json::json!({
                                            "reason": "stale_heartbeat",
                                            "worker_id": worker.id,
                                            "worker_name": worker.name,
                                            "worker_role": worker.worker_role,
                                            "heartbeat_age_seconds": age_secs,
                                            "staleness_threshold_seconds": max_age_secs,
                                        }),
                                        correlation_id: Some(format!(
                                            "worker:{}:stale_heartbeat",
                                            worker.id
                                        )),
                                    };
                                    if let Err(e) =
                                        emit_core_alert(&pool, Some(&publisher), alert).await
                                    {
                                        warn!(
                                            "Failed to emit stale-heartbeat alert for worker {}: {}",
                                            worker.id, e
                                        );
                                    }
                                }
                            }
                        }
                    }

                    if deactivated_count > 0 {
                        info!(
                            "Deactivated {} worker(s) with stale heartbeats",
                            deactivated_count
                        );
                    }
                }
                Err(e) => {
                    error!("Failed to query active workers for heartbeat check: {}", e);
                }
            }
        }
    }

    /// Wait for all tasks to complete
    async fn wait_for_tasks(handles: Vec<JoinHandle<Result<()>>>) -> Result<()> {
        for handle in handles {
            if let Err(e) = handle.await {
                error!("Task panicked: {}", e);
            }
        }
        Ok(())
    }

    /// Get database pool reference
    #[allow(dead_code)]
    pub fn pool(&self) -> &PgPool {
        &self.inner.pool
    }

    /// Get config reference
    #[allow(dead_code)]
    pub fn config(&self) -> &Config {
        &self.inner.config
    }

    /// Get publisher reference
    #[allow(dead_code)]
    pub fn publisher(&self) -> &Publisher {
        &self.inner.publisher
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    #[ignore] // Requires database and RabbitMQ
    async fn test_service_creation() {
        let config = Config::load().expect("Failed to load config");
        let service = ExecutorService::new(config).await;
        assert!(service.is_ok());
    }
}
