//! Heartbeat Module
//!
//! Manages periodic heartbeat updates to keep the worker's status fresh in the database.

use attune_common::error::Result;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Mutex, RwLock};
use tokio::task::JoinHandle;
use tokio::time;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

use crate::registration::WorkerRegistration;

/// Heartbeat manager for worker status updates
pub struct HeartbeatManager {
    registration: Arc<RwLock<WorkerRegistration>>,
    interval: Duration,
    task: Mutex<Option<HeartbeatTask>>,
}

struct HeartbeatTask {
    cancellation: CancellationToken,
    handle: JoinHandle<()>,
}

impl HeartbeatManager {
    /// Create a new heartbeat manager
    ///
    /// # Arguments
    /// * `registration` - Worker registration instance
    /// * `interval_secs` - Heartbeat interval in seconds
    pub fn new(registration: Arc<RwLock<WorkerRegistration>>, interval_secs: u64) -> Self {
        Self {
            registration,
            interval: Duration::from_secs(interval_secs),
            task: Mutex::new(None),
        }
    }

    /// Start the heartbeat loop
    ///
    /// This spawns a background task that periodically updates the worker's heartbeat
    /// in the database. The task will continue running until `stop()` is called.
    pub async fn start(&self) -> Result<()> {
        let mut task = self.task.lock().await;
        if task.is_some() {
            warn!("Heartbeat manager is already running");
            return Ok(());
        }

        info!(
            "Starting heartbeat manager with interval: {:?}",
            self.interval
        );

        let registration = self.registration.clone();
        let interval = self.interval;
        let cancellation = CancellationToken::new();
        let task_cancellation = cancellation.clone();

        let handle = tokio::spawn(async move {
            let mut ticker = time::interval(interval);

            loop {
                tokio::select! {
                    _ = task_cancellation.cancelled() => break,
                    _ = ticker.tick() => {}
                }

                let update_result = tokio::select! {
                    _ = task_cancellation.cancelled() => break,
                    result = async {
                        let reg = registration.read().await;
                        reg.update_heartbeat().await
                    } => result,
                };

                match update_result {
                    Ok(_) => {
                        debug!("Heartbeat sent successfully");
                    }
                    Err(e) => {
                        error!("Failed to send heartbeat: {}", e);
                        // Continue trying - don't break the loop on transient errors
                    }
                }
            }

            info!("Heartbeat manager stopped");
        });
        *task = Some(HeartbeatTask {
            cancellation,
            handle,
        });

        Ok(())
    }

    /// Stop the heartbeat loop
    pub async fn stop(&self) {
        info!("Stopping heartbeat manager");
        let task = self.task.lock().await.take();
        if let Some(task) = task {
            task.cancellation.cancel();
            if let Err(error) = task.handle.await {
                error!("Heartbeat manager task failed during shutdown: {}", error);
            }
        }
    }

    /// Check if the heartbeat manager is running
    pub async fn is_running(&self) -> bool {
        self.task
            .lock()
            .await
            .as_ref()
            .is_some_and(|task| !task.handle.is_finished())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registration::WorkerRegistration;
    use attune_common::config::Config;
    use attune_common::repositories::{runtime::WorkerRepository, FindById};
    use attune_common::test_database::TestDatabase;

    fn test_config() -> Config {
        Config::load_from_file(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../config.test.yaml"
        ))
        .unwrap()
    }

    #[tokio::test]
    #[ignore] // Requires database
    async fn test_heartbeat_manager() {
        let mut config = test_config();
        if let Some(worker) = config.worker.as_mut() {
            worker.name = Some(format!("heartbeat-test-{}", uuid::Uuid::new_v4().simple()));
        }
        let database = TestDatabase::create(&config.database)
            .await
            .expect("Failed to create isolated heartbeat test database")
            .with_cleanup_on_drop();
        let pool = database.pool().clone();
        let mut registration = WorkerRegistration::new(pool.clone(), &config);
        let worker_id = registration.register().await.unwrap();
        let initial_heartbeat = WorkerRepository::get_by_id(&pool, worker_id)
            .await
            .unwrap()
            .last_heartbeat;

        let registration = Arc::new(RwLock::new(registration));
        let manager = HeartbeatManager::new(registration.clone(), 1);

        // Start heartbeat
        manager.start().await.unwrap();
        assert!(manager.is_running().await);

        let heartbeat_advanced = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let worker = WorkerRepository::get_by_id(&pool, worker_id).await.unwrap();
                if worker.last_heartbeat > initial_heartbeat {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
        })
        .await
        .is_ok();

        // Stop and join the background task before asserting or cleaning up.
        manager.stop().await;
        assert!(!manager.is_running().await);
        assert!(
            heartbeat_advanced,
            "worker heartbeat did not advance within the timeout"
        );

        // Deregister worker
        let reg = registration.read().await;
        reg.deregister().await.unwrap();
    }
}
