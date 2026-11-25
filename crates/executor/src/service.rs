//! Executor Service - Core orchestration and execution management
//!
//! The ExecutorService is the central component that:
//! - Processes enforcement messages from triggered rules
//! - Schedules executions to workers
//! - Manages execution lifecycle and state transitions
//! - Enforces execution policies (rate limiting, concurrency)
//! - Orchestrates workflows (parent-child executions)
//! - Handles human-in-the-loop inquiries

use anyhow::{anyhow, Context, Result};
use attune_common::{
    artifact_transport::{ApiTransport, ArtifactFileTransport},
    auth::{jwt::JwtConfig, WorkerTokenProvider},
    config::Config,
    db::Database,
    mq::{
        ActionChangedPayload, Connection, Consumer, MessageEnvelope, MessageQueueConfig, Publisher,
    },
    system_alert::{emit_core_alert, SystemAlert},
};
use sqlx::PgPool;
use std::{num::NonZeroUsize, sync::Arc};
use tokio::task::JoinSet;
use tracing::{error, info, warn};

use crate::completion_listener::CompletionListener;
use crate::dead_letter_handler::{create_dlq_consumer_config, DeadLetterHandler};
use crate::enforcement_processor::EnforcementProcessor;
use crate::event_processor::EventProcessor;
use crate::execution_manager::ExecutionManager;
use crate::inquiry_handler::InquiryHandler;
use crate::pack_test_processor::PackTestProcessor;
use crate::policy_enforcer::PolicyEnforcer;
use crate::queue_dispatcher::WorkQueueDispatcher;
use crate::queue_manager::{ExecutionQueueManager, QueueConfig};
use crate::scheduler::{ExecutionScheduler, SchedulerMetadataCaches};
use crate::timeout_monitor::{ExecutionTimeoutMonitor, TimeoutMonitorConfig};
use crate::workflow::log::WorkflowLogDispatcher;

use attune_common::{
    execution_authority::consumer_contract::ConsumerRole,
    mq::consumer::SubscriptionObservation,
    repositories::recorded_consumer_observations::{
        RecordedConsumerIdentity, RecordedConsumerObservations,
    },
};

const COMPLETION_SHUTDOWN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);
const WORKFLOW_LOG_SHUTDOWN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);
const RESOURCE_SHUTDOWN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

struct PreparedProcessors {
    subscriptions: Option<Vec<SubscriptionObservation>>,
    completion_consumer: Arc<Consumer>,
    completion_listener: CompletionListener,
    workflow_log_dispatcher: WorkflowLogDispatcher,
}

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

    /// Prefix used to distinguish this service's RabbitMQ consumers
    consumer_tag_prefix: String,

    /// Policy enforcer for execution policies
    policy_enforcer: Arc<PolicyEnforcer>,

    /// Queue manager for FIFO execution ordering
    queue_manager: Arc<ExecutionQueueManager>,

    /// Metadata cached within this database-backed service instance.
    scheduler_metadata_caches: Arc<SchedulerMetadataCaches>,

    /// Service shutdown signal
    shutdown_requested: tokio::sync::watch::Sender<bool>,
}

impl ExecutorService {
    async fn run_metadata_invalidation_consumer(
        mq_url: String,
        exchange: String,
        consumer_tag: String,
        metadata_caches: Arc<SchedulerMetadataCaches>,
    ) -> Result<()> {
        info!("Starting metadata invalidation consumer");
        loop {
            match Connection::connect(&mq_url).await {
                Ok(connection) => match connection
                    .create_ephemeral_topic_consumer(
                        &exchange,
                        &["metadata.action.changed"],
                        &consumer_tag,
                        32,
                    )
                    .await
                {
                    Ok(consumer) => {
                        let metadata_caches = metadata_caches.clone();
                        if let Err(error) = consumer
                            .consume_once_with_handler(
                                move |envelope: MessageEnvelope<ActionChangedPayload>| {
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
                                },
                            )
                            .await
                        {
                            warn!("Metadata invalidation consumer ended with error: {}", error);
                        }
                    }
                    Err(error) => {
                        warn!("Failed to create metadata invalidation consumer: {}", error)
                    }
                },
                Err(error) => warn!(
                    "Failed to connect MQ for metadata invalidation consumer: {}",
                    error
                ),
            }

            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        }
    }

    /// Create a new executor service
    pub async fn new(config: Config) -> Result<Self> {
        Self::new_with_mq_config(
            config,
            MessageQueueConfig::default(),
            "executor".to_string(),
        )
        .await
    }

    async fn new_with_mq_config(
        config: Config,
        mq_config: MessageQueueConfig,
        consumer_tag_prefix: String,
    ) -> Result<Self> {
        info!("Initializing Executor Service");

        // Initialize database
        let db = Database::new_named(
            &config.database,
            attune_common::db::ConnectionIdentity::for_process("executor", "main"),
        )
        .await?;
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

        // Retain the stop request even if startup is still in progress.
        let (shutdown_requested, _) = tokio::sync::watch::channel(false);

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
            shutdown_requested,
            mq_config: Arc::new(mq_config),
            consumer_tag_prefix,
        };

        Ok(Self {
            inner: Arc::new(inner),
        })
    }

    fn consumer_tag(&self, name: &str) -> String {
        format!("{}.{}", self.inner.consumer_tag_prefix, name)
    }

    async fn prepare_processors(
        &self,
        recorded_identity: Option<Arc<RecordedConsumerIdentity>>,
        tasks: &mut JoinSet<Result<()>>,
    ) -> Result<PreparedProcessors> {
        let mut recorded_subscriptions = recorded_identity.as_ref().map(|_| Vec::new());
        let api_url = std::env::var("ATTUNE_API_URL").unwrap_or_else(|_| {
            format!(
                "http://{}:{}",
                self.inner.config.server.host, self.inner.config.server.port
            )
        });
        let jwt_config = JwtConfig {
            secret: self
                .inner
                .config
                .security
                .jwt_secret
                .clone()
                .unwrap_or_else(|| "insecure_default_secret_change_in_production".to_string()),
            access_token_expiration: self.inner.config.security.jwt_access_expiration as i64,
            refresh_token_expiration: self.inner.config.security.jwt_refresh_expiration as i64,
        };
        let workflow_log_api = ApiTransport::new_with_worker_token_provider(
            &api_url,
            Arc::new(WorkerTokenProvider::new(0, "executor", jwt_config.clone())),
            &self.inner.config.artifacts_dir,
        )?;
        let workflow_log_transport: Arc<dyn ArtifactFileTransport> =
            Arc::new(workflow_log_api.clone());

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
                tag: self.consumer_tag("event"),
                prefetch_count: 10,
                auto_ack: false,
                exclusive: false,
            },
        )
        .await?;
        let event_processor = EventProcessor::new(
            self.inner.pool.clone(),
            self.inner.publisher.clone(),
            Self::observe_subscription(event_consumer, &mut recorded_subscriptions),
            self.inner.config.security.encryption_key.clone(),
        );
        tasks.spawn(async move { event_processor.start().await });

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
                tag: self.consumer_tag("completion"),
                prefetch_count: 10,
                auto_ack: false,
                exclusive: false,
            },
        )
        .await?;
        let completion_consumer =
            Self::observe_subscription(completion_consumer, &mut recorded_subscriptions);
        let completion_listener = CompletionListener::new(
            self.inner.pool.clone(),
            completion_consumer.clone(),
            self.inner.publisher.clone(),
            self.inner.queue_manager.clone(),
            self.inner.config.artifacts_dir.clone(),
            self.inner.config.security.encryption_key.clone(),
            self.inner.scheduler_metadata_caches.clone(),
            self.inner.config.artifacts.log_segment_max_bytes,
            self.inner.config.artifacts.flush_interval_ms,
        )
        .with_workflow_log_transport(workflow_log_transport.clone());
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
                tag: self.consumer_tag("enforcement"),
                prefetch_count: 10,
                auto_ack: false,
                exclusive: false,
            },
        )
        .await?;
        let enforcement_processor = EnforcementProcessor::new(
            self.inner.pool.clone(),
            self.inner.publisher.clone(),
            Self::observe_subscription(enforcement_consumer, &mut recorded_subscriptions),
            self.inner.policy_enforcer.clone(),
            self.inner.queue_manager.clone(),
            self.inner.config.security.encryption_key.clone(),
        );
        tasks.spawn(async move { enforcement_processor.start().await });

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
                tag: self.consumer_tag("scheduler"),
                prefetch_count: 10,
                auto_ack: false,
                exclusive: false,
            },
        )
        .await?;
        let scheduler = ExecutionScheduler::new(
            self.inner.pool.clone(),
            self.inner.publisher.clone(),
            Self::observe_subscription(scheduler_consumer, &mut recorded_subscriptions),
            self.inner.policy_enforcer.clone(),
            self.inner.config.artifacts_dir.clone(),
            self.inner.config.security.encryption_key.clone(),
            self.inner.scheduler_metadata_caches.clone(),
            self.inner.config.artifacts.log_segment_max_bytes,
            self.inner.config.artifacts.flush_interval_ms,
        )
        .with_workflow_log_transport(workflow_log_transport.clone())
        .with_recorded_identity(recorded_identity.clone());
        tasks.spawn(async move { scheduler.start().await });

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
                tag: self.consumer_tag("manager"),
                prefetch_count: 10,
                auto_ack: false,
                exclusive: false,
            },
        )
        .await?;
        let execution_manager = ExecutionManager::new(
            self.inner.pool.clone(),
            self.inner.publisher.clone(),
            Self::observe_subscription(manager_consumer, &mut recorded_subscriptions),
            self.inner.config.security.encryption_key.clone(),
        );
        tasks.spawn(async move { execution_manager.start().await });

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
                tag: self.consumer_tag("inquiry"),
                prefetch_count: 10,
                auto_ack: false,
                exclusive: false,
            },
        )
        .await?;
        let inquiry_handler = InquiryHandler::new(
            self.inner.pool.clone(),
            self.inner.publisher.clone(),
            Self::observe_subscription(inquiry_consumer, &mut recorded_subscriptions),
            self.inner.config.security.encryption_key.clone(),
        );
        tasks.spawn(async move { inquiry_handler.start().await });

        // Start pack test processor with its own consumer
        info!("Starting pack test processor...");
        let pack_tests_queue = self.inner.mq_config.rabbitmq.queues.pack_tests.name.clone();
        let pack_tests_consumer = Consumer::new(
            &self.inner.mq_connection,
            attune_common::mq::ConsumerConfig {
                queue: pack_tests_queue,
                tag: self.consumer_tag("packtest"),
                prefetch_count: 5,
                auto_ack: false,
                exclusive: false,
            },
        )
        .await?;
        let pack_test_processor = PackTestProcessor::new(
            self.inner.pool.clone(),
            self.inner.publisher.clone(),
            Arc::new(pack_tests_consumer),
        );
        tasks.spawn(async move { pack_test_processor.start().await });

        // Start inquiry timeout checker
        info!("Starting inquiry timeout checker...");
        let timeout_pool = self.inner.pool.clone();
        let timeout_publisher = self.inner.publisher.clone();
        let timeout_encryption_key = self.inner.config.security.encryption_key.clone();
        tasks.spawn(async move {
            InquiryHandler::timeout_check_loop(
                timeout_pool,
                timeout_publisher,
                timeout_encryption_key,
                1,
            )
            .await;
            Ok(())
        });

        // Start metadata invalidation consumer (per-instance ephemeral queue)
        info!("Starting metadata invalidation consumer...");
        let metadata_mq_url = self
            .inner
            .config
            .message_queue
            .as_ref()
            .expect("message queue configuration was validated during initialization")
            .url
            .clone();
        tasks.spawn(Self::run_metadata_invalidation_consumer(
            metadata_mq_url,
            self.inner
                .mq_config
                .rabbitmq
                .exchanges
                .metadata
                .name
                .clone(),
            self.consumer_tag("metadata.invalidation"),
            self.inner.scheduler_metadata_caches.clone(),
        ));

        // Start worker heartbeat monitor
        info!("Starting worker heartbeat monitor...");
        let worker_pool = self.inner.pool.clone();
        let worker_publisher = self.inner.publisher.clone();
        tasks.spawn(async move {
            Self::worker_heartbeat_monitor_loop(worker_pool, worker_publisher, 60).await;
            Ok(())
        });

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
        tasks.spawn(async move { timeout_monitor.start().await });

        // Start work queue dispatcher
        info!("Starting work queue dispatcher...");
        let queue_dispatcher = WorkQueueDispatcher::new(
            self.inner.pool.clone(),
            self.inner.publisher.clone(),
            self.inner.config.security.encryption_key.clone(),
            recorded_identity.clone(),
        );
        tasks.spawn(async move { queue_dispatcher.start().await });

        // Start dead letter handler (if DLQ is enabled)
        if self.inner.mq_config.rabbitmq.dead_letter.enabled {
            info!("Starting dead letter handler...");
            let dlq_name = format!(
                "{}.queue",
                self.inner.mq_config.rabbitmq.dead_letter.exchange
            );
            let dlq_consumer = Consumer::new(
                &self.inner.mq_connection,
                create_dlq_consumer_config(&dlq_name, &self.consumer_tag("dlq")),
            )
            .await?;
            let dlq_handler = Arc::new(
                DeadLetterHandler::new(Arc::new(self.inner.pool.clone()), dlq_consumer)
                    .await
                    .map_err(|e| anyhow::anyhow!("Failed to create DLQ handler: {}", e))?,
            );
            tasks.spawn(async move {
                dlq_handler
                    .start()
                    .await
                    .map_err(|e| anyhow::anyhow!("DLQ handler error: {}", e))
            });
        } else {
            info!("Dead letter queue is disabled, skipping DLQ handler");
        }

        info!("Starting workflow log outbox dispatcher...");
        let workflow_log_dispatcher = WorkflowLogDispatcher::new(
            self.inner.pool.clone(),
            workflow_log_transport,
            self.inner.config.artifacts.log_segment_max_bytes,
            self.inner.config.artifacts.flush_interval_ms,
        )
        .with_recorded_transport(workflow_log_api, jwt_config);
        Ok(PreparedProcessors {
            subscriptions: recorded_subscriptions,
            completion_consumer,
            completion_listener,
            workflow_log_dispatcher,
        })
    }

    /// Start the executor service
    pub async fn start(&self) -> Result<()> {
        info!("Starting Executor Service");
        let recorded_identity = RecordedConsumerIdentity::from_environment()?.map(Arc::new);
        let mut tasks = JoinSet::new();
        let mut shutdown = self.inner.shutdown_requested.subscribe();
        let prepared = tokio::select! {
            biased;
            _ = shutdown.wait_for(|requested| *requested) => Ok(None),
            result = self.prepare_processors(recorded_identity.clone(), &mut tasks) => result.map(Some),
        };
        let PreparedProcessors {
            subscriptions: recorded_subscriptions,
            completion_consumer,
            completion_listener,
            workflow_log_dispatcher,
        } = match prepared {
            Ok(Some(prepared)) => prepared,
            result => {
                tasks.shutdown().await;
                let cleanup = self.close_resources().await;
                if let Err(error) = result {
                    if let Err(cleanup_error) = cleanup {
                        warn!(%cleanup_error, "Failed to close executor resources after startup error");
                    }
                    return Err(error);
                }
                return cleanup;
            }
        };

        let completion_concurrency = self
            .inner
            .config
            .executor
            .as_ref()
            .and_then(|config| config.completion_concurrency)
            .unwrap_or(NonZeroUsize::MIN);

        let (workflow_log_stop, workflow_log_shutdown) = tokio::sync::broadcast::channel(1);
        let mut workflow_log_handle =
            tokio::spawn(async move { workflow_log_dispatcher.start(workflow_log_shutdown).await });
        let mut completion_task = JoinSet::new();
        if *self.inner.shutdown_requested.borrow() {
            completion_consumer.request_stop();
        }
        completion_task
            .spawn(async move { completion_listener.start(completion_concurrency).await });

        info!("Executor Service started successfully");
        info!("All processors are listening for messages...");

        // Wait for shutdown signal
        let mut observations = Box::pin(self.observe_recorded_executor(
            recorded_identity.as_deref(),
            recorded_subscriptions.as_deref().unwrap_or_default(),
        ));
        let mut service_failure = None;
        let dispatcher_finished = tokio::select! {
            _ = shutdown.wait_for(|requested| *requested) => {
                info!("Shutdown signal received");
                false
            }
            () = observations.as_mut() => unreachable!("recorded observation loop returned"),
            result = Self::wait_for_tasks(&mut tasks) => {
                match result {
                    Ok(_) => info!("All tasks completed"),
                    Err(e) => error!("Task error: {}", e),
                }
                false
            }
            result = completion_task.join_next() => {
                service_failure = Some(match result {
                    Some(Ok(Err(error))) => error,
                    Some(Err(error)) => anyhow!("Completion listener task failed: {error}"),
                    _ => anyhow!("Completion listener stopped unexpectedly"),
                });
                false
            }
            result = &mut workflow_log_handle => {
                match result {
                    Ok(Ok(())) => {
                        warn!("Workflow log dispatcher stopped unexpectedly");
                        service_failure = Some(anyhow!("workflow log dispatcher stopped unexpectedly"));
                    }
                    Ok(Err(error)) => {
                        error!("Workflow log dispatcher stopped with error: {error}");
                        service_failure = Some(error);
                    }
                    Err(error) => {
                        error!("Workflow log dispatcher task panicked: {error}");
                        service_failure = Some(anyhow!("workflow log dispatcher task panicked: {error}"));
                    }
                }
                true
            }
        };
        drop(observations);

        if *shutdown.borrow() {
            service_failure = None;
        }

        completion_consumer.request_stop();
        let ((), completion_drain) = tokio::join!(
            async {
                if let Some(identity) = &recorded_identity {
                    self.publish_recorded_executor(identity, false).await;
                }
            },
            tokio::time::timeout(COMPLETION_SHUTDOWN_TIMEOUT, completion_task.join_next()),
        );
        match completion_drain {
            Ok(Some(Ok(Err(error)))) => {
                error!(%error, "Completion listener failed during shutdown")
            }
            Ok(Some(Err(error))) => {
                error!(%error, "Completion listener task failed during shutdown")
            }
            Ok(_) => {}
            Err(_) => {
                warn!("Completion listener did not drain before the shutdown deadline");
                completion_task.shutdown().await;
            }
        }
        tasks.shutdown().await;
        let _ = workflow_log_stop.send(());
        if !dispatcher_finished {
            match tokio::time::timeout(WORKFLOW_LOG_SHUTDOWN_TIMEOUT, &mut workflow_log_handle)
                .await
            {
                Ok(Ok(Ok(()))) => info!("Workflow log dispatcher stopped"),
                Ok(Ok(Err(error))) => error!("Workflow log dispatcher stopped with error: {error}"),
                Ok(Err(error)) => error!("Workflow log dispatcher task panicked: {error}"),
                Err(_) => {
                    warn!("Workflow log dispatcher did not drain before the shutdown deadline");
                    workflow_log_handle.abort();
                    let _ = workflow_log_handle.await;
                }
            }
        }
        let cleanup = self.close_resources().await;

        if let Some(error) = service_failure {
            return Err(error);
        }
        cleanup
    }

    async fn close_resources(&self) -> Result<()> {
        let (broker, ()) = tokio::time::timeout(RESOURCE_SHUTDOWN_TIMEOUT, async {
            tokio::join!(self.inner.mq_connection.close(), self.inner.pool.close())
        })
        .await
        .context("Executor resource shutdown timed out")?;
        broker.map_err(Into::into)
    }

    fn observe_subscription(
        consumer: Consumer,
        subscriptions: &mut Option<Vec<SubscriptionObservation>>,
    ) -> Arc<Consumer> {
        if let Some(subscriptions) = subscriptions {
            let (consumer, observation) = consumer.with_subscription_observation();
            subscriptions.push(observation);
            Arc::new(consumer)
        } else {
            Arc::new(consumer)
        }
    }

    async fn observe_recorded_executor(
        &self,
        identity: Option<&RecordedConsumerIdentity>,
        subscriptions: &[SubscriptionObservation],
    ) {
        let Some(identity) = identity else {
            return std::future::pending().await;
        };
        let configured = self
            .inner
            .config
            .security
            .encryption_key
            .as_ref()
            .is_some_and(|key| key.trim().len() >= 32)
            && self
                .inner
                .config
                .security
                .jwt_secret
                .as_ref()
                .is_some_and(|key| key.trim().len() >= 32 && key != &JwtConfig::default().secret);
        let mut ticks = tokio::time::interval(std::time::Duration::from_secs(5));
        ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            ticks.tick().await;
            let ready = configured
                && subscriptions.len() == 6
                && subscriptions
                    .iter()
                    .all(SubscriptionObservation::is_current);
            self.publish_recorded_executor(identity, ready).await;
        }
    }

    async fn publish_recorded_executor(
        &self,
        identity: &RecordedConsumerIdentity,
        installed: bool,
    ) {
        let result = tokio::time::timeout(std::time::Duration::from_secs(3), async {
            let mut connection = self.inner.pool.acquire().await?;
            let ready = installed
                && self.inner.publisher.channel().await.status().connected()
                && RecordedConsumerObservations::executor_access_available(&mut connection).await?;
            identity
                .publish(&mut connection, ready.then_some(ConsumerRole::Executor))
                .await
        })
        .await;
        if !matches!(result, Ok(Ok(()))) {
            warn!("Unable to update the recorded executor observation");
        }
    }

    /// Request shutdown. The running service drains and closes its resources
    /// before `start` returns.
    pub async fn stop(&self) -> Result<()> {
        info!("Stopping Executor Service");
        self.inner.shutdown_requested.send_replace(true);
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
    async fn wait_for_tasks(tasks: &mut JoinSet<Result<()>>) -> Result<()> {
        while let Some(result) = tasks.join_next().await {
            if let Err(e) = result {
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
    use attune_common::test_database::TestDatabase;
    use lapin::options::{ExchangeDeleteOptions, QueueDeleteOptions};
    use std::{collections::HashSet, time::Duration};
    use tokio::time::timeout;
    use uuid::Uuid;

    const TEST_OPERATION_TIMEOUT: Duration = Duration::from_secs(60);

    include!("service/completion_tests.rs");

    struct TestMqTopology {
        config: MessageQueueConfig,
        consumer_tag_prefix: String,
        owned_queues: Vec<String>,
        owned_exchanges: Vec<String>,
    }

    impl TestMqTopology {
        fn new(test_id: &str) -> Self {
            let prefix = format!("attune.test.executor.{test_id}");
            let mut config = MessageQueueConfig::default();
            config.rabbitmq.dead_letter.enabled = false;

            let mut owned_queues = Vec::new();
            for (queue, suffix) in [
                &mut config.rabbitmq.queues.executor_events,
                &mut config.rabbitmq.queues.enforcements,
                &mut config.rabbitmq.queues.execution_requests,
                &mut config.rabbitmq.queues.execution_status,
                &mut config.rabbitmq.queues.execution_completed,
                &mut config.rabbitmq.queues.inquiry_responses,
                &mut config.rabbitmq.queues.pack_tests,
            ]
            .into_iter()
            .zip([
                "executor.events",
                "enforcements",
                "execution.requests",
                "execution.status",
                "execution.completed",
                "inquiry.responses",
                "pack.tests",
            ]) {
                queue.name = format!("{prefix}.{suffix}.queue");
                owned_queues.push(queue.name.clone());
            }

            let mut owned_exchanges = Vec::new();
            for (exchange, suffix) in [
                &mut config.rabbitmq.exchanges.events,
                &mut config.rabbitmq.exchanges.executions,
                &mut config.rabbitmq.exchanges.notifications,
                &mut config.rabbitmq.exchanges.metadata,
            ]
            .into_iter()
            .zip(["events", "executions", "notifications", "metadata"])
            {
                exchange.name = format!("{prefix}.{suffix}");
                owned_exchanges.push(exchange.name.clone());
            }

            Self {
                config,
                consumer_tag_prefix: format!("{prefix}.consumer"),
                owned_queues,
                owned_exchanges,
            }
        }

        async fn delete_owned(&self, connection: &Connection) -> Result<()> {
            let channel = connection.create_channel().await?;
            let mut first_error = None;
            for queue in &self.owned_queues {
                if let Err(error) = channel
                    .queue_delete(queue.as_str().into(), QueueDeleteOptions::default())
                    .await
                    .with_context(|| format!("delete owned queue {queue}"))
                {
                    first_error.get_or_insert(error);
                }
            }
            for exchange in &self.owned_exchanges {
                if let Err(error) = channel
                    .exchange_delete(exchange.as_str().into(), ExchangeDeleteOptions::default())
                    .await
                    .with_context(|| format!("delete owned exchange {exchange}"))
                {
                    first_error.get_or_insert(error);
                }
            }
            if let Some(error) = first_error {
                return Err(error);
            }
            Ok(())
        }
    }

    #[test]
    fn test_service_topology_is_unique() {
        let topology = TestMqTopology::new("config-test");
        let consumer_tags = [
            "event",
            "completion",
            "enforcement",
            "scheduler",
            "manager",
            "inquiry",
            "packtest",
            "metadata.invalidation",
            "dlq",
        ]
        .map(|suffix| format!("{}.{}", topology.consumer_tag_prefix, suffix));

        assert_eq!(
            topology.owned_queues.iter().collect::<HashSet<_>>().len(),
            topology.owned_queues.len()
        );
        assert_eq!(
            topology
                .owned_exchanges
                .iter()
                .collect::<HashSet<_>>()
                .len(),
            topology.owned_exchanges.len()
        );
        assert!(topology
            .owned_queues
            .iter()
            .all(|name| name.starts_with("attune.test.executor.config-test.")));
        assert!(topology
            .owned_exchanges
            .iter()
            .all(|name| name.starts_with("attune.test.executor.config-test.")));
        assert_eq!(
            consumer_tags.iter().collect::<HashSet<_>>().len(),
            consumer_tags.len()
        );
        assert!(consumer_tags
            .iter()
            .all(|tag| tag.starts_with("attune.test.executor.config-test.consumer.")));
    }

    #[tokio::test]
    #[ignore = "requires a disposable database and RabbitMQ vhost"]
    async fn test_service_drains_completion_before_closing_resources() -> Result<()> {
        let config_path = format!("{}/../../config.test.yaml", env!("CARGO_MANIFEST_DIR"));
        let mut config = Config::load_from_file(&config_path).expect("Failed to load test config");
        let mq_url =
            std::env::var("ATTUNE_TEST_RABBITMQ_URL").context("owned RabbitMQ fixture URL")?;
        config
            .message_queue
            .as_mut()
            .context("test message queue config")?
            .url = mq_url.clone();
        let work = tempfile::tempdir()?;
        config.artifacts_dir = work.path().join("artifacts").display().to_string();
        config.packs_base_dir = work.path().join("packs").display().to_string();
        config.runtime_envs_dir = work.path().join("runtimes").display().to_string();
        let database = timeout(
            TEST_OPERATION_TIMEOUT,
            TestDatabase::create(&config.database),
        )
        .await
        .context("create test database before timeout")??
        .with_cleanup_on_drop();
        config.database.url = database.database_url().to_string();
        config.database.schema = Some(database.schema().to_string());
        let topology = TestMqTopology::new(&Uuid::new_v4().simple().to_string());
        let cleanup_connection = timeout(TEST_OPERATION_TIMEOUT, Connection::connect(&mq_url))
            .await
            .context("connect cleanup client before timeout")??;

        let mut tasks = JoinSet::new();
        let mut service_connection = None;
        let service_result: Result<()> = async {
            let service = timeout(
                TEST_OPERATION_TIMEOUT,
                ExecutorService::new_with_mq_config(
                    config,
                    topology.config.clone(),
                    topology.consumer_tag_prefix.clone(),
                ),
            )
            .await
            .context("create executor service before timeout")??;
            service_connection = Some(service.inner.mq_connection.clone());
            let queue = &topology.config.rabbitmq.queues.execution_completed.name;
            let channel = cleanup_connection.create_channel().await?;
            tasks.spawn({
                let service = service.clone();
                async move { service.start().await }
            });
            timeout(TEST_OPERATION_TIMEOUT, async {
                loop {
                    let declared = channel.queue_declare(queue.as_str().into(),
                        lapin::options::QueueDeclareOptions {
                            passive: true,
                            durable: true,
                            ..Default::default()
                        },
                        lapin::types::FieldTable::default()).await?;
                    if declared.consumer_count() == 1 {
                        return Ok::<_, lapin::Error>(());
                    }
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
            }).await.context("wait for the completion subscription")??;

            let mut blocker = database.pool().begin().await?;
            sqlx::query("LOCK TABLE execution_resource_admission IN ACCESS EXCLUSIVE MODE")
                .execute(&mut *blocker).await?;
            let blocker_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
                .fetch_one(&mut *blocker).await?;
            let envelope = MessageEnvelope::new(
                attune_common::mq::MessageType::ExecutionCompleted,
                attune_common::mq::ExecutionCompletedPayload {
                    execution_id: i64::MAX,
                    action_id: i64::MAX,
                    action_ref: "test.shutdown".into(),
                    status: "Completed".into(),
                    result: None,
                    completed_at: chrono::Utc::now(),
                },
            );
            channel
                .basic_publish(
                    "".into(),
                    queue.as_str().into(),
                    Default::default(),
                    &envelope.to_bytes()?,
                    Default::default(),
                )
                .await?
                .await?;
            timeout(TEST_OPERATION_TIMEOUT, async {
                loop {
                    let blocked: bool = sqlx::query_scalar(
                        "SELECT EXISTS (SELECT 1 FROM pg_stat_activity
                         WHERE datname = current_database() AND $1 = ANY(pg_blocking_pids(pid))
                         AND query LIKE '%SELECT EXISTS (SELECT 1 FROM execution_resource_admission%')",
                    ).bind(blocker_pid).fetch_one(database.pool()).await?;
                    if blocked {
                        return Ok::<_, sqlx::Error>(());
                    }
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
            }).await.context("wait for a completion handler to reach the database gate")??;
            service.stop().await?;
            anyhow::ensure!(
                timeout(Duration::from_millis(150), tasks.join_next()).await.is_err(),
                "the service must wait for its blocked completion"
            );
            anyhow::ensure!(
                service.inner.mq_connection.is_healthy().await && !service.pool().is_closed(),
                "the active completion needs both connections during shutdown"
            );
            blocker.rollback().await?;
            timeout(TEST_OPERATION_TIMEOUT, tasks.join_next())
                .await
                .context("drain executor service before timeout")?
                .context("missing executor task")???;
            anyhow::ensure!(
                channel.basic_get(queue.as_str().into(), Default::default()).await?.is_none(),
                "the active completion was requeued instead of acknowledged"
            );
            anyhow::ensure!(service.pool().is_closed(), "executor pool was left open");
            Ok(())
        }
        .await;
        tasks.shutdown().await;
        let service_close_result = if let Some(connection) = service_connection {
            connection.close().await
        } else {
            Ok(())
        };

        let topology_cleanup_result = timeout(
            TEST_OPERATION_TIMEOUT,
            topology.delete_owned(&cleanup_connection),
        )
        .await
        .context("delete owned RabbitMQ topology before timeout")
        .and_then(|result| result);
        let connection_close_result = timeout(TEST_OPERATION_TIMEOUT, cleanup_connection.close())
            .await
            .context("close cleanup connection before timeout")
            .and_then(|result| result.map_err(Into::into));
        let database_cleanup_result = timeout(TEST_OPERATION_TIMEOUT, database.cleanup())
            .await
            .context("clean up test database before timeout")
            .and_then(|result| result.map_err(Into::into));

        service_result?;
        service_close_result?;
        topology_cleanup_result?;
        connection_close_result?;
        database_cleanup_result?;
        Ok(())
    }
}
