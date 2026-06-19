//! Timer Manager
//!
//! Manages individual timer tasks for each rule, with support for:
//! - Interval-based timers (fires every N seconds/minutes/hours/days)
//! - Cron-based timers (fires based on cron expressions)
//! - DateTime-based timers (fires once at a specific time)
//! - RRULE-based timers (RFC 5545 recurrence rules; biweekly, nth-weekday-of-month, etc.)

use crate::api_client::{ApiClient, CreateEventRequest};
use crate::types::{TimeUnit, TimerConfig};
use anyhow::{Context, Result};
use chrono::Utc;
use rrule::RRuleSet;
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Mutex, RwLock};
use tokio_cron_scheduler::{Job, JobScheduler};
use tracing::{debug, error, info, warn};
use uuid::Uuid;

/// Timer manager for handling per-rule timers
#[derive(Clone)]
pub struct TimerManager {
    inner: Arc<TimerManagerInner>,
}

struct TimerManagerInner {
    /// Serializes scheduler mutations so lifecycle messages and reconciliation
    /// cannot register duplicate jobs for the same rule concurrently.
    timer_mutation: Mutex<()>,
    /// Map of rule_id -> job UUID in the scheduler
    active_jobs: RwLock<HashMap<i64, Uuid>>,
    /// Last applied timer config per rule.
    active_configs: RwLock<HashMap<i64, TimerConfig>>,
    /// Shared cron scheduler for all timer types (wrapped in Mutex for shutdown)
    scheduler: Mutex<JobScheduler>,
    /// API client for creating events
    api_client: ApiClient,
    /// Sensor reference for event payloads
    sensor_ref: String,
}

impl TimerManager {
    /// Create a new timer manager
    pub async fn new(api_client: ApiClient, sensor_ref: String) -> Result<Self> {
        let scheduler = JobScheduler::new().await?;

        // Start the scheduler
        scheduler.start().await?;

        Ok(Self {
            inner: Arc::new(TimerManagerInner {
                timer_mutation: Mutex::new(()),
                active_jobs: RwLock::new(HashMap::new()),
                active_configs: RwLock::new(HashMap::new()),
                scheduler: Mutex::new(scheduler),
                api_client,
                sensor_ref,
            }),
        })
    }

    /// Start a timer for a rule
    pub async fn start_timer(&self, rule_id: i64, config: TimerConfig) -> Result<()> {
        let _mutation = self.inner.timer_mutation.lock().await;

        let existing_config = self
            .inner
            .active_configs
            .read()
            .await
            .get(&rule_id)
            .cloned();
        if existing_config.as_ref() == Some(&config)
            && self.inner.active_jobs.read().await.contains_key(&rule_id)
        {
            debug!(
                "Timer for rule {} already matches desired config; keeping existing schedule",
                rule_id
            );
            return Ok(());
        }

        // Stop existing timer if any. This must be part of the same serialized
        // mutation as adding the replacement job; otherwise lifecycle delivery
        // and reconciliation can race and leave duplicate scheduler jobs alive.
        self.stop_timer_locked(rule_id).await;

        info!("Starting timer for rule {}: {:?}", rule_id, config);

        // Create appropriate job type
        let job = match &config {
            TimerConfig::Interval { interval, unit } => {
                self.create_interval_job(rule_id, *interval, *unit).await?
            }
            TimerConfig::Cron { expression } => {
                self.create_cron_job(rule_id, expression.clone()).await?
            }
            TimerConfig::DateTime { fire_at } => {
                self.create_datetime_job(rule_id, *fire_at).await?
            }
            TimerConfig::RRule {
                rule,
                dtstart,
                timezone,
            } => {
                // RRULE timers reschedule themselves and manage their own job
                // UUIDs in the active_jobs map. We return early to skip the
                // generic add+insert path below.
                self.start_rrule_timer(rule_id, rule.clone(), *dtstart, timezone.clone())
                    .await?;
                self.inner
                    .active_configs
                    .write()
                    .await
                    .insert(rule_id, config);
                return Ok(());
            }
        };

        // Add job to scheduler and store UUID
        let job_uuid = self.inner.scheduler.lock().await.add(job).await?;
        self.inner
            .active_jobs
            .write()
            .await
            .insert(rule_id, job_uuid);
        self.inner
            .active_configs
            .write()
            .await
            .insert(rule_id, config);

        info!(
            "Timer started for rule {} with job UUID {}",
            rule_id, job_uuid
        );

        Ok(())
    }

    /// Stop a timer for a rule
    pub async fn stop_timer(&self, rule_id: i64) {
        let _mutation = self.inner.timer_mutation.lock().await;
        self.stop_timer_locked(rule_id).await;
    }

    async fn stop_timer_locked(&self, rule_id: i64) {
        self.inner.active_configs.write().await.remove(&rule_id);
        let mut active_jobs = self.inner.active_jobs.write().await;

        if let Some(job_uuid) = active_jobs.remove(&rule_id) {
            if let Err(e) = self.inner.scheduler.lock().await.remove(&job_uuid).await {
                warn!(
                    "Failed to remove job {} for rule {}: {}",
                    job_uuid, rule_id, e
                );
            } else {
                info!("Stopped timer for rule {}", rule_id);
            }
        } else {
            debug!("No timer found for rule {}", rule_id);
        }
    }

    /// Stop all timers
    pub async fn stop_all(&self) {
        self.inner.active_configs.write().await.clear();
        let mut active_jobs = self.inner.active_jobs.write().await;

        let count = active_jobs.len();
        for (rule_id, job_uuid) in active_jobs.drain() {
            if let Err(e) = self.inner.scheduler.lock().await.remove(&job_uuid).await {
                warn!(
                    "Failed to remove job {} for rule {}: {}",
                    job_uuid, rule_id, e
                );
            } else {
                debug!("Stopped timer for rule {}", rule_id);
            }
        }

        info!("Stopped {} timers", count);
    }

    /// Get count of active timers
    #[allow(dead_code)]
    pub async fn timer_count(&self) -> usize {
        self.inner.active_jobs.read().await.len()
    }

    /// Get currently active rule IDs.
    pub async fn active_rule_ids(&self) -> Vec<i64> {
        self.inner
            .active_jobs
            .read()
            .await
            .keys()
            .copied()
            .collect()
    }

    #[cfg(test)]
    pub async fn job_uuid_for_rule(&self, rule_id: i64) -> Option<Uuid> {
        self.inner.active_jobs.read().await.get(&rule_id).copied()
    }

    /// Shutdown the scheduler
    pub async fn shutdown(&self) -> Result<()> {
        info!("Shutting down timer manager");
        self.stop_all().await;
        self.inner.scheduler.lock().await.shutdown().await?;
        Ok(())
    }

    /// Create an interval-based job
    async fn create_interval_job(
        &self,
        rule_id: i64,
        interval: u64,
        unit: TimeUnit,
    ) -> Result<Job> {
        let interval_seconds = match unit {
            TimeUnit::Seconds => interval,
            TimeUnit::Minutes => interval * 60,
            TimeUnit::Hours => interval * 3600,
            TimeUnit::Days => interval * 86400,
        };

        if interval_seconds == 0 {
            return Err(anyhow::anyhow!("Interval must be greater than 0"));
        }

        let api_client = self.inner.api_client.clone();
        let sensor_ref = self.inner.sensor_ref.clone();
        let duration = Duration::from_secs(interval_seconds);

        info!(
            "Creating interval job for rule {} (interval: {}s)",
            rule_id, interval_seconds
        );

        let mut execution_count = 0u64;

        let job = Job::new_repeated_async(duration, move |_uuid, _lock| {
            let api_client = api_client.clone();
            let sensor_ref = sensor_ref.clone();
            let rule_id = rule_id;
            execution_count += 1;
            let count = execution_count;
            let interval_secs = interval_seconds;

            Box::pin(async move {
                let now = Utc::now();

                // Create event payload matching intervaltimer output schema
                let payload = serde_json::json!({
                    "type": "interval",
                    "interval_seconds": interval_secs,
                    "fired_at": now.to_rfc3339(),
                    "execution_count": count,
                    "sensor_ref": sensor_ref,
                });

                // Create event via API
                let request = CreateEventRequest::new("core.intervaltimer".to_string(), payload)
                    .with_trigger_instance_id(format!("rule_{}", rule_id));

                match api_client.create_event_with_retry(request).await {
                    Ok(event_id) => {
                        info!(
                            "Interval timer fired for rule {} (count: {}), created event {}",
                            rule_id, count, event_id
                        );
                    }
                    Err(e) => {
                        error!(
                            "Failed to create event for rule {} interval timer: {}",
                            rule_id, e
                        );
                    }
                }
            })
        })?;

        Ok(job)
    }

    /// Create a cron-based job
    async fn create_cron_job(&self, rule_id: i64, expression: String) -> Result<Job> {
        info!(
            "Creating cron job for rule {} with expression: {}",
            rule_id, expression
        );

        let api_client = self.inner.api_client.clone();
        let sensor_ref = self.inner.sensor_ref.clone();
        let expr_clone = expression.clone();

        let mut execution_count = 0u64;

        let job = Job::new_async(&expression, move |uuid, mut lock| {
            let api_client = api_client.clone();
            let sensor_ref = sensor_ref.clone();
            let rule_id = rule_id;
            let expression = expr_clone.clone();
            execution_count += 1;
            let count = execution_count;

            Box::pin(async move {
                let now = Utc::now();

                // Get next scheduled time
                let next_fire = match lock.next_tick_for_job(uuid).await {
                    Ok(Some(ts)) => ts.to_rfc3339(),
                    Ok(None) => "unknown".to_string(),
                    Err(e) => {
                        warn!("Failed to get next tick for cron job {}: {}", uuid, e);
                        "unknown".to_string()
                    }
                };

                // Create event payload matching crontimer output schema
                let payload = serde_json::json!({
                    "type": "cron",
                    "fired_at": now.to_rfc3339(),
                    "scheduled_at": now.to_rfc3339(),
                    "expression": expression,
                    "timezone": "UTC",
                    "next_fire_at": next_fire,
                    "execution_count": count,
                    "sensor_ref": sensor_ref,
                });

                // Create event via API
                let request = CreateEventRequest::new("core.crontimer".to_string(), payload)
                    .with_trigger_instance_id(format!("rule_{}", rule_id));

                match api_client.create_event_with_retry(request).await {
                    Ok(event_id) => {
                        info!(
                            "Cron timer fired for rule {} (count: {}), created event {}",
                            rule_id, count, event_id
                        );
                    }
                    Err(e) => {
                        error!(
                            "Failed to create event for rule {} cron timer: {}",
                            rule_id, e
                        );
                    }
                }
            })
        })?;

        Ok(job)
    }

    /// Create a datetime-based (one-shot) job
    async fn create_datetime_job(
        &self,
        rule_id: i64,
        fire_at: chrono::DateTime<Utc>,
    ) -> Result<Job> {
        let now = Utc::now();

        if fire_at <= now {
            return Err(anyhow::anyhow!(
                "DateTime timer fire_at must be in the future"
            ));
        }

        let duration = (fire_at - now)
            .to_std()
            .map_err(|e| anyhow::anyhow!("Invalid duration: {}", e))?;

        info!(
            "Creating one-shot job for rule {} scheduled at {}",
            rule_id,
            fire_at.to_rfc3339()
        );

        let api_client = self.inner.api_client.clone();
        let sensor_ref = self.inner.sensor_ref.clone();
        let scheduled_time = fire_at.to_rfc3339();

        let job = Job::new_one_shot_async(duration, move |_uuid, _lock| {
            let api_client = api_client.clone();
            let sensor_ref = sensor_ref.clone();
            let rule_id = rule_id;
            let scheduled_time = scheduled_time.clone();

            Box::pin(async move {
                let now = Utc::now();

                // Calculate delay between scheduled and actual fire time
                let delay_ms = (now.timestamp_millis() - fire_at.timestamp_millis()).max(0);

                // Create event payload matching datetimetimer output schema
                let payload = serde_json::json!({
                    "type": "one_shot",
                    "fire_at": scheduled_time,
                    "fired_at": now.to_rfc3339(),
                    "timezone": "UTC",
                    "delay_ms": delay_ms,
                    "sensor_ref": sensor_ref,
                });

                // Create event via API
                let request = CreateEventRequest::new("core.datetimetimer".to_string(), payload)
                    .with_trigger_instance_id(format!("rule_{}", rule_id));

                match api_client.create_event_with_retry(request).await {
                    Ok(event_id) => {
                        info!(
                            "DateTime timer fired for rule {}, created event {}",
                            rule_id, event_id
                        );
                    }
                    Err(e) => {
                        error!(
                            "Failed to create event for rule {} datetime timer: {}",
                            rule_id, e
                        );
                    }
                }

                info!("One-shot timer completed for rule {}", rule_id);
            })
        })?;

        Ok(job)
    }

    /// Start an RRULE-based timer.
    ///
    /// Validates the recurrence rule up front, then schedules a one-shot job
    /// for the next future occurrence. Each fire reschedules itself for the
    /// occurrence after that, so a long-running RRULE timer is implemented as
    /// a chain of one-shots rather than a single repeating job.
    async fn start_rrule_timer(
        &self,
        rule_id: i64,
        rule: String,
        dtstart: chrono::DateTime<Utc>,
        timezone: Option<String>,
    ) -> Result<()> {
        // Validate timezone if provided.
        if let Some(tz_name) = timezone.as_deref() {
            chrono_tz::Tz::from_str(tz_name)
                .map_err(|e| anyhow::anyhow!("Invalid timezone '{}': {}", tz_name, e))?;
        }

        let ical = build_ical_input(&rule, dtstart, timezone.as_deref());

        // Parse once to surface syntax errors immediately rather than on first fire.
        RRuleSet::from_str(&ical).with_context(|| {
            format!(
                "Failed to parse RRULE for rule {}: {} (input: {})",
                rule_id, rule, ical
            )
        })?;

        info!(
            "Starting RRULE timer for rule {}: rule='{}', dtstart={}, tz={:?}",
            rule_id,
            rule,
            dtstart.to_rfc3339(),
            timezone
        );

        let expected_config = TimerConfig::RRule {
            rule,
            dtstart,
            timezone,
        };

        self.schedule_next_rrule(rule_id, expected_config, ical, 0, None)
            .await
    }

    /// Compute the next RRULE occurrence after now and schedule a one-shot for it.
    ///
    /// On fire, the closure publishes an event and recursively reschedules
    /// itself for the subsequent occurrence. `execution_count` is threaded
    /// through so the emitted event payload can report a meaningful count.
    ///
    /// Returns a boxed future to break the recursive `async fn` type so the
    /// resulting future remains `Send` (required by tokio's scheduler tasks).
    fn schedule_next_rrule(
        &self,
        rule_id: i64,
        expected_config: TimerConfig,
        ical: String,
        execution_count: u64,
        fired_job_uuid: Option<Uuid>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<()>> + Send + 'static>> {
        let this = self.clone();
        Box::pin(async move {
            let set = RRuleSet::from_str(&ical)
                .with_context(|| format!("Failed to parse RRULE for rule {}", rule_id))?;

            // RRULE callbacks may race with stop/restart operations. When this
            // path is entered from an in-flight callback, hold the same mutation
            // lock used by start/stop and verify that this callback still owns
            // the active RRULE chain before scheduling another one-shot job.
            let _mutation = if let Some(fired_uuid) = fired_job_uuid {
                let mutation = this.inner.timer_mutation.lock().await;

                let active_config = this
                    .inner
                    .active_configs
                    .read()
                    .await
                    .get(&rule_id)
                    .cloned();
                if active_config.as_ref() != Some(&expected_config) {
                    debug!(
                        "Skipping RRULE reschedule for rule {}: active config changed or timer stopped",
                        rule_id
                    );
                    return Ok(());
                }

                let active_job_uuid = this.inner.active_jobs.read().await.get(&rule_id).copied();
                if active_job_uuid != Some(fired_uuid) {
                    debug!(
                        "Skipping RRULE reschedule for rule {}: callback job {} is no longer current",
                        rule_id, fired_uuid
                    );
                    return Ok(());
                }

                Some(mutation)
            } else {
                None
            };

            let now = Utc::now();
            let next = set
                .into_iter()
                .map(|d| d.with_timezone(&Utc))
                .find(|d| *d > now);

            let Some(next) = next else {
                info!(
                    "RRULE for rule {} has no future occurrences; timer is complete",
                    rule_id
                );
                this.inner.active_jobs.write().await.remove(&rule_id);
                this.inner.active_configs.write().await.remove(&rule_id);
                return Ok(());
            };

            let duration = (next - now)
                .to_std()
                .map_err(|e| anyhow::anyhow!("Invalid duration to next RRULE occurrence: {}", e))?;

            info!(
                "Next RRULE fire for rule {} at {} (in {}s)",
                rule_id,
                next.to_rfc3339(),
                duration.as_secs()
            );

            let manager = this.clone();
            let api_client = this.inner.api_client.clone();
            let sensor_ref = this.inner.sensor_ref.clone();
            let scheduled_time = next.to_rfc3339();
            let ical_for_reschedule = ical.clone();
            let expected_config_for_reschedule = expected_config.clone();

            let job = Job::new_one_shot_async(duration, move |fired_uuid, _lock| {
                let api_client = api_client.clone();
                let sensor_ref = sensor_ref.clone();
                let manager = manager.clone();
                let scheduled_time = scheduled_time.clone();
                let ical_for_reschedule = ical_for_reschedule.clone();
                let expected_config_for_reschedule = expected_config_for_reschedule.clone();
                let count = execution_count + 1;

                Box::pin(async move {
                    let now = Utc::now();
                    let delay_ms = (now.timestamp_millis() - next.timestamp_millis()).max(0);

                    let payload = serde_json::json!({
                        "type": "rrule",
                        "fired_at": now.to_rfc3339(),
                        "scheduled_at": scheduled_time,
                        "delay_ms": delay_ms,
                        "execution_count": count,
                        "sensor_ref": sensor_ref,
                    });

                    let request = CreateEventRequest::new("core.rruletimer".to_string(), payload)
                        .with_trigger_instance_id(format!("rule_{}", rule_id));

                    match api_client.create_event_with_retry(request).await {
                        Ok(event_id) => {
                            info!(
                                "RRULE timer fired for rule {} (count: {}), created event {}",
                                rule_id, count, event_id
                            );
                        }
                        Err(e) => {
                            error!(
                                "Failed to create event for rule {} RRULE timer: {}",
                                rule_id, e
                            );
                        }
                    }

                    if let Err(e) = manager
                        .schedule_next_rrule(
                            rule_id,
                            expected_config_for_reschedule,
                            ical_for_reschedule,
                            count,
                            Some(fired_uuid),
                        )
                        .await
                    {
                        error!(
                            "Failed to reschedule next RRULE occurrence for rule {}: {}",
                            rule_id, e
                        );
                    }
                })
            })?;

            let job_uuid = this.inner.scheduler.lock().await.add(job).await?;
            this.inner
                .active_jobs
                .write()
                .await
                .insert(rule_id, job_uuid);

            Ok(())
        })
    }
}

/// Build an iCalendar input suitable for `RRuleSet::from_str`.
///
/// When `tz` is provided, `dtstart` is rendered in that timezone using the
/// `DTSTART;TZID=...` form so wall-clock semantics (e.g., "9am New York time")
/// survive daylight-saving transitions. Otherwise it is emitted as a UTC
/// instant via the `DTSTART:...Z` form.
fn build_ical_input(rule: &str, dtstart: chrono::DateTime<Utc>, tz: Option<&str>) -> String {
    let rule_body = rule.trim().trim_start_matches("RRULE:");
    match tz {
        Some(tz_name) => match chrono_tz::Tz::from_str(tz_name) {
            Ok(tz_parsed) => {
                let local = dtstart.with_timezone(&tz_parsed);
                format!(
                    "DTSTART;TZID={}:{}\nRRULE:{}",
                    tz_name,
                    local.format("%Y%m%dT%H%M%S"),
                    rule_body
                )
            }
            Err(_) => format!(
                "DTSTART:{}\nRRULE:{}",
                dtstart.format("%Y%m%dT%H%M%SZ"),
                rule_body
            ),
        },
        None => format!(
            "DTSTART:{}\nRRULE:{}",
            dtstart.format("%Y%m%dT%H%M%SZ"),
            rule_body
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn test_manager() -> TimerManager {
        let api_client = ApiClient::new("http://localhost:8080".to_string(), "token".to_string());
        TimerManager::new(api_client, "core.timer_sensor".to_string())
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn test_timer_manager_creation() {
        let manager = test_manager().await;
        assert_eq!(manager.timer_count().await, 0);
        manager.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn test_timer_manager_start_stop() {
        let manager = test_manager().await;

        let config = TimerConfig::Interval {
            interval: 60,
            unit: TimeUnit::Seconds,
        };

        // Start timer
        manager.start_timer(1, config).await.unwrap();
        assert_eq!(manager.timer_count().await, 1);

        // Stop timer
        manager.stop_timer(1).await;
        assert_eq!(manager.timer_count().await, 0);

        manager.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn test_timer_manager_stop_all() {
        let manager = test_manager().await;

        let config = TimerConfig::Interval {
            interval: 60,
            unit: TimeUnit::Seconds,
        };

        // Start multiple timers
        manager.start_timer(1, config.clone()).await.unwrap();
        manager.start_timer(2, config.clone()).await.unwrap();
        manager.start_timer(3, config).await.unwrap();

        assert_eq!(manager.timer_count().await, 3);

        // Stop all
        manager.stop_all().await;
        assert_eq!(manager.timer_count().await, 0);

        manager.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn test_interval_timer_validation() {
        let manager = test_manager().await;

        let config = TimerConfig::Interval {
            interval: 0,
            unit: TimeUnit::Seconds,
        };

        // Should fail with zero interval
        let result = manager.start_timer(1, config).await;
        assert!(result.is_err());

        manager.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn test_datetime_timer_validation() {
        let manager = test_manager().await;

        // Create a datetime in the past
        let past = Utc::now() - chrono::Duration::seconds(60);
        let config = TimerConfig::DateTime { fire_at: past };

        // Should fail with past datetime
        let result = manager.start_timer(1, config).await;
        assert!(result.is_err());

        manager.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn test_rrule_timer_creation() {
        let manager = test_manager().await;

        // Anchor in the past so the iterator must skip ahead, exercising the
        // catch-up branch of schedule_next_rrule.
        let dtstart = Utc::now() - chrono::Duration::days(30);
        let config = TimerConfig::RRule {
            rule: "FREQ=WEEKLY;INTERVAL=2;BYDAY=MO".to_string(),
            dtstart,
            timezone: Some("UTC".to_string()),
        };

        manager.start_timer(700, config).await.unwrap();
        assert_eq!(manager.timer_count().await, 1);

        manager.stop_timer(700).await;
        assert_eq!(manager.timer_count().await, 0);

        manager.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn test_rrule_timer_invalid_rule() {
        let manager = test_manager().await;

        let config = TimerConfig::RRule {
            rule: "this is not a valid rrule".to_string(),
            dtstart: Utc::now(),
            timezone: None,
        };

        let result = manager.start_timer(701, config).await;
        assert!(result.is_err());

        manager.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn test_rrule_timer_invalid_timezone() {
        let manager = test_manager().await;

        let config = TimerConfig::RRule {
            rule: "FREQ=DAILY".to_string(),
            dtstart: Utc::now(),
            timezone: Some("Mars/Olympus_Mons".to_string()),
        };

        let result = manager.start_timer(702, config).await;
        assert!(result.is_err());

        manager.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn test_rrule_stale_callback_does_not_reschedule_after_stop() {
        let manager = test_manager().await;
        let rule_id = 703;
        let dtstart = Utc::now() + chrono::Duration::days(1);
        let config = TimerConfig::RRule {
            rule: "FREQ=DAILY".to_string(),
            dtstart,
            timezone: Some("UTC".to_string()),
        };

        manager.start_timer(rule_id, config.clone()).await.unwrap();
        let stale_job_uuid = manager
            .job_uuid_for_rule(rule_id)
            .await
            .expect("rrule job should exist");

        manager.stop_timer(rule_id).await;
        assert_eq!(manager.timer_count().await, 0);

        let ical = build_ical_input("FREQ=DAILY", dtstart, Some("UTC"));
        manager
            .schedule_next_rrule(rule_id, config, ical, 1, Some(stale_job_uuid))
            .await
            .unwrap();

        assert_eq!(manager.timer_count().await, 0);
        manager.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn test_rrule_stale_callback_does_not_reschedule_after_restart_same_config() {
        let manager = test_manager().await;
        let rule_id = 704;
        let dtstart = Utc::now() + chrono::Duration::days(1);
        let config = TimerConfig::RRule {
            rule: "FREQ=DAILY".to_string(),
            dtstart,
            timezone: Some("UTC".to_string()),
        };

        manager.start_timer(rule_id, config.clone()).await.unwrap();
        let stale_job_uuid = manager
            .job_uuid_for_rule(rule_id)
            .await
            .expect("initial rrule job should exist");

        manager.stop_timer(rule_id).await;
        manager.start_timer(rule_id, config.clone()).await.unwrap();
        let current_job_uuid = manager
            .job_uuid_for_rule(rule_id)
            .await
            .expect("replacement rrule job should exist");

        let ical = build_ical_input("FREQ=DAILY", dtstart, Some("UTC"));
        manager
            .schedule_next_rrule(rule_id, config, ical, 1, Some(stale_job_uuid))
            .await
            .unwrap();

        assert_eq!(
            manager.job_uuid_for_rule(rule_id).await,
            Some(current_job_uuid)
        );
        assert_eq!(manager.timer_count().await, 1);
        manager.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn test_rrule_stale_callback_does_not_reschedule_after_replacement() {
        let manager = test_manager().await;
        let rule_id = 705;
        let dtstart = Utc::now() + chrono::Duration::days(1);
        let old_config = TimerConfig::RRule {
            rule: "FREQ=DAILY".to_string(),
            dtstart,
            timezone: Some("UTC".to_string()),
        };

        manager
            .start_timer(rule_id, old_config.clone())
            .await
            .unwrap();
        let stale_job_uuid = manager
            .job_uuid_for_rule(rule_id)
            .await
            .expect("initial rrule job should exist");

        let replacement_config = TimerConfig::Interval {
            interval: 60,
            unit: TimeUnit::Seconds,
        };
        manager
            .start_timer(rule_id, replacement_config)
            .await
            .unwrap();
        let current_job_uuid = manager
            .job_uuid_for_rule(rule_id)
            .await
            .expect("replacement interval job should exist");

        let ical = build_ical_input("FREQ=DAILY", dtstart, Some("UTC"));
        manager
            .schedule_next_rrule(rule_id, old_config, ical, 1, Some(stale_job_uuid))
            .await
            .unwrap();

        assert_eq!(
            manager.job_uuid_for_rule(rule_id).await,
            Some(current_job_uuid)
        );
        assert_eq!(manager.timer_count().await, 1);
        manager.shutdown().await.unwrap();
    }

    #[test]
    fn test_build_ical_input_utc() {
        let dt = chrono::DateTime::parse_from_rfc3339("2026-05-04T13:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let ical = build_ical_input("FREQ=WEEKLY;INTERVAL=2;BYDAY=MO", dt, None);
        assert_eq!(
            ical,
            "DTSTART:20260504T130000Z\nRRULE:FREQ=WEEKLY;INTERVAL=2;BYDAY=MO"
        );
    }

    #[test]
    fn test_build_ical_input_tz() {
        let dt = chrono::DateTime::parse_from_rfc3339("2026-05-04T13:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let ical = build_ical_input("FREQ=WEEKLY", dt, Some("America/New_York"));
        // 13:00Z = 09:00 EDT on 2026-05-04
        assert_eq!(
            ical,
            "DTSTART;TZID=America/New_York:20260504T090000\nRRULE:FREQ=WEEKLY"
        );
    }

    #[test]
    fn test_build_ical_input_strips_prefix() {
        let dt = chrono::DateTime::parse_from_rfc3339("2026-05-04T13:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let ical = build_ical_input("RRULE:FREQ=DAILY", dt, None);
        assert!(ical.ends_with("\nRRULE:FREQ=DAILY"));
    }

    #[tokio::test]
    async fn test_cron_timer_creation() {
        let manager = test_manager().await;

        // Valid cron expression: every minute
        let config = TimerConfig::Cron {
            expression: "0 * * * * *".to_string(),
        };

        // Should succeed
        let result = manager.start_timer(1, config).await;
        assert!(result.is_ok());
        assert_eq!(manager.timer_count().await, 1);

        manager.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn test_cron_timer_invalid_expression() {
        let manager = test_manager().await;

        // Invalid cron expression
        let config = TimerConfig::Cron {
            expression: "invalid cron".to_string(),
        };

        // Should fail with invalid expression
        let result = manager.start_timer(1, config).await;
        assert!(result.is_err());

        manager.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn test_timer_restart() {
        let manager = test_manager().await;

        let config1 = TimerConfig::Interval {
            interval: 60,
            unit: TimeUnit::Seconds,
        };

        let config2 = TimerConfig::Interval {
            interval: 30,
            unit: TimeUnit::Seconds,
        };

        // Start first timer
        manager.start_timer(1, config1).await.unwrap();
        assert_eq!(manager.timer_count().await, 1);

        // Start second timer for same rule (should replace)
        manager.start_timer(1, config2).await.unwrap();
        assert_eq!(manager.timer_count().await, 1);

        manager.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn test_start_timer_with_same_config_keeps_existing_job() {
        let manager = test_manager().await;

        let config = TimerConfig::Interval {
            interval: 60,
            unit: TimeUnit::Seconds,
        };

        manager.start_timer(1, config.clone()).await.unwrap();
        let first_job = manager
            .job_uuid_for_rule(1)
            .await
            .expect("timer should be active");

        manager.start_timer(1, config).await.unwrap();
        let second_job = manager
            .job_uuid_for_rule(1)
            .await
            .expect("timer should remain active");

        assert_eq!(first_job, second_job);
        assert_eq!(manager.timer_count().await, 1);

        manager.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn test_all_timer_types_comprehensive() {
        let manager = test_manager().await;

        // Test 1: Interval timer
        let interval_config = TimerConfig::Interval {
            interval: 5,
            unit: TimeUnit::Seconds,
        };
        manager.start_timer(100, interval_config).await.unwrap();

        // Test 2: Cron timer - every minute
        let cron_config = TimerConfig::Cron {
            expression: "0 * * * * *".to_string(),
        };
        manager.start_timer(200, cron_config).await.unwrap();

        // Test 3: DateTime timer - 2 seconds in the future
        let fire_time = Utc::now() + chrono::Duration::seconds(2);
        let datetime_config = TimerConfig::DateTime { fire_at: fire_time };
        manager.start_timer(300, datetime_config).await.unwrap();

        // Verify all three timers are active
        assert_eq!(manager.timer_count().await, 3);

        // Stop specific timers
        manager.stop_timer(100).await;
        assert_eq!(manager.timer_count().await, 2);

        manager.stop_timer(200).await;
        assert_eq!(manager.timer_count().await, 1);

        manager.stop_timer(300).await;
        assert_eq!(manager.timer_count().await, 0);

        manager.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn test_cron_various_expressions() {
        let manager = test_manager().await;

        // Test various valid cron expressions
        let expressions = [
            "0 0 * * * *",    // Every hour
            "0 */15 * * * *", // Every 15 minutes
            "0 0 0 * * *",    // Daily at midnight
            "0 0 9 * * 1-5",  // Weekdays at 9 AM
            "0 30 8 * * *",   // Every day at 8:30 AM
        ];

        for (i, expr) in expressions.iter().enumerate() {
            let config = TimerConfig::Cron {
                expression: expr.to_string(),
            };
            let result = manager.start_timer(i as i64 + 1, config).await;
            assert!(
                result.is_ok(),
                "Failed to create cron job with expression: {}",
                expr
            );
        }

        assert_eq!(manager.timer_count().await, expressions.len());

        manager.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn test_datetime_timer_future_validation() {
        let manager = test_manager().await;

        // Test various future times
        let one_second = Utc::now() + chrono::Duration::seconds(1);
        let one_minute = Utc::now() + chrono::Duration::minutes(1);
        let one_hour = Utc::now() + chrono::Duration::hours(1);

        let config1 = TimerConfig::DateTime {
            fire_at: one_second,
        };
        assert!(manager.start_timer(1, config1).await.is_ok());

        let config2 = TimerConfig::DateTime {
            fire_at: one_minute,
        };
        assert!(manager.start_timer(2, config2).await.is_ok());

        let config3 = TimerConfig::DateTime { fire_at: one_hour };
        assert!(manager.start_timer(3, config3).await.is_ok());

        assert_eq!(manager.timer_count().await, 3);

        manager.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn test_mixed_timer_replacement() {
        let manager = test_manager().await;

        let rule_id = 42;

        // Start with interval timer
        let interval_config = TimerConfig::Interval {
            interval: 60,
            unit: TimeUnit::Seconds,
        };
        manager.start_timer(rule_id, interval_config).await.unwrap();
        assert_eq!(manager.timer_count().await, 1);

        // Replace with cron timer
        let cron_config = TimerConfig::Cron {
            expression: "0 0 * * * *".to_string(),
        };
        manager.start_timer(rule_id, cron_config).await.unwrap();
        assert_eq!(manager.timer_count().await, 1);

        // Replace with datetime timer
        let datetime_config = TimerConfig::DateTime {
            fire_at: Utc::now() + chrono::Duration::hours(1),
        };
        manager.start_timer(rule_id, datetime_config).await.unwrap();
        assert_eq!(manager.timer_count().await, 1);

        manager.shutdown().await.unwrap();
    }
}
