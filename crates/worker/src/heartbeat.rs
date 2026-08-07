//! Heartbeat Module
//!
//! Manages periodic heartbeat updates to keep the worker's status fresh in the database.

use attune_common::error::Result;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use tokio::time;
use tracing::{debug, error, info, warn};

use crate::registration::WorkerRegistration;

/// Heartbeat manager for worker status updates
pub struct HeartbeatManager {
    registration: Arc<RwLock<WorkerRegistration>>,
    interval: Duration,
    running: Arc<RwLock<bool>>,
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
            running: Arc::new(RwLock::new(false)),
        }
    }

    /// Start the heartbeat loop
    ///
    /// This spawns a background task that periodically updates the worker's heartbeat
    /// in the database. The task will continue running until `stop()` is called.
    pub async fn start(&self) -> Result<()> {
        let mut running = self.running.write().await;
        if *running {
            warn!("Heartbeat manager is already running");
            return Ok(());
        }
        *running = true;
        drop(running);

        info!(
            "Starting heartbeat manager with interval: {:?}",
            self.interval
        );

        let registration = self.registration.clone();
        let interval = self.interval;
        let running = self.running.clone();

        tokio::spawn(async move {
            let mut ticker = time::interval(interval);

            loop {
                ticker.tick().await;

                // Check if we should stop
                {
                    let is_running = running.read().await;
                    if !*is_running {
                        info!("Heartbeat manager stopping");
                        break;
                    }
                }

                // Send heartbeat
                let reg = registration.read().await;
                match reg.update_heartbeat().await {
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

        Ok(())
    }

    /// Stop the heartbeat loop
    pub async fn stop(&self) {
        info!("Stopping heartbeat manager");
        let mut running = self.running.write().await;
        *running = false;
    }

    /// Check if the heartbeat manager is running
    pub async fn is_running(&self) -> bool {
        let running = self.running.read().await;
        *running
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registration::WorkerRegistration;
    use attune_common::config::Config;
    use attune_common::db::Database;

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
        let config = test_config();
        let db = Database::new(&config.database).await.unwrap();
        let pool = db.pool().clone();
        let mut registration = WorkerRegistration::new(pool, &config);
        registration.register().await.unwrap();

        let registration = Arc::new(RwLock::new(registration));
        let manager = HeartbeatManager::new(registration.clone(), 1);

        // Start heartbeat
        manager.start().await.unwrap();
        assert!(manager.is_running().await);

        // Wait for a few heartbeats
        tokio::time::sleep(Duration::from_secs(3)).await;

        // Stop heartbeat
        manager.stop().await;
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!(!manager.is_running().await);

        // Deregister worker
        let reg = registration.read().await;
        reg.deregister().await.unwrap();
    }
}
