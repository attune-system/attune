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
use std::future::Future;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Mutex, RwLock};
use tokio::task::JoinHandle;
use tokio::time::{Instant, MissedTickBehavior};
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
    timer_mutation: Mutex<TimerLifecycle>,
    /// Per-rule scheduler jobs or owned interval tasks.
    active_jobs: RwLock<HashMap<i64, ActiveTimer>>,
    /// Last applied timer config per rule.
    active_configs: RwLock<HashMap<i64, TimerConfig>>,
    /// Shared scheduler for calendar timers (wrapped in Mutex for shutdown).
    scheduler: Mutex<JobScheduler>,
    /// API client for creating events
    api_client: ApiClient,
    /// Sensor reference for event payloads
    sensor_ref: String,
}

#[derive(Default)]
struct TimerLifecycle {
    shutting_down: bool,
}

enum ActiveTimer {
    Scheduled(Uuid),
    Interval { id: Uuid, task: JoinHandle<()> },
}

impl ActiveTimer {
    fn id(&self) -> Uuid {
        match self {
            Self::Scheduled(id) | Self::Interval { id, .. } => *id,
        }
    }

    fn is_running(&self) -> bool {
        match self {
            Self::Scheduled(_) => true,
            Self::Interval { task, .. } => !task.is_finished(),
        }
    }
}

impl Drop for ActiveTimer {
    fn drop(&mut self) {
        // Dropping a JoinHandle alone detaches it. Also abort if registration or
        // teardown is cancelled before the handle can be explicitly joined.
        if let Self::Interval { task, .. } = self {
            task.abort();
        }
    }
}

impl TimerManager {
    /// Create a new timer manager
    pub async fn new(api_client: ApiClient, sensor_ref: String) -> Result<Self> {
        let scheduler = JobScheduler::new().await?;

        // Start the scheduler
        scheduler.start().await?;

        Ok(Self {
            inner: Arc::new(TimerManagerInner {
                timer_mutation: Mutex::new(TimerLifecycle::default()),
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
        let mutation = self.inner.timer_mutation.lock().await;
        if mutation.shutting_down {
            anyhow::bail!("Timer manager is shutting down");
        }

        let existing_config = self
            .inner
            .active_configs
            .read()
            .await
            .get(&rule_id)
            .cloned();
        if existing_config.as_ref() == Some(&config)
            && self
                .inner
                .active_jobs
                .read()
                .await
                .get(&rule_id)
                .is_some_and(ActiveTimer::is_running)
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

        let timer = match &config {
            TimerConfig::Interval { interval, unit } => {
                self.create_interval_timer(rule_id, *interval, *unit)?
            }
            TimerConfig::Cron { expression } => {
                let job = self.create_cron_job(rule_id, expression.clone()).await?;
                ActiveTimer::Scheduled(self.inner.scheduler.lock().await.add(job).await?)
            }
            TimerConfig::DateTime { fire_at } => {
                let job = self.create_datetime_job(rule_id, *fire_at).await?;
                ActiveTimer::Scheduled(self.inner.scheduler.lock().await.add(job).await?)
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

        let timer_id = timer.id();
        self.inner.active_jobs.write().await.insert(rule_id, timer);
        self.inner
            .active_configs
            .write()
            .await
            .insert(rule_id, config);

        info!(
            "Timer started for rule {} with timer UUID {}",
            rule_id, timer_id
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
        let timer = self.inner.active_jobs.write().await.remove(&rule_id);
        if let Some(timer) = timer {
            self.stop_owned_timer(rule_id, timer).await;
        } else {
            debug!("No timer found for rule {}", rule_id);
        }
    }

    /// Stop all timers
    pub async fn stop_all(&self) {
        let _mutation = self.inner.timer_mutation.lock().await;
        self.inner.active_configs.write().await.clear();
        let timers: Vec<_> = self.inner.active_jobs.write().await.drain().collect();
        let count = timers.len();
        for (rule_id, timer) in timers {
            self.stop_owned_timer(rule_id, timer).await;
        }

        info!("Stopped {} timers", count);
    }

    async fn stop_owned_timer(&self, rule_id: i64, mut timer: ActiveTimer) {
        match &mut timer {
            ActiveTimer::Scheduled(id) => {
                if let Err(e) = self.inner.scheduler.lock().await.remove(id).await {
                    warn!("Failed to remove job {} for rule {}: {}", id, rule_id, e);
                }
            }
            ActiveTimer::Interval { task, .. } => {
                // Abort pending ticks and HTTP retries, then wait until the task
                // has actually stopped before a replacement can start.
                task.abort();
                if let Err(e) = task.await {
                    if !e.is_cancelled() {
                        warn!("Interval task for rule {} failed: {}", rule_id, e);
                    }
                }
            }
        }
        info!("Stopped timer for rule {}", rule_id);
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
        self.inner
            .active_jobs
            .read()
            .await
            .get(&rule_id)
            .map(ActiveTimer::id)
    }

    /// Shutdown the scheduler
    pub async fn shutdown(&self) -> Result<()> {
        info!("Shutting down timer manager");
        self.inner.timer_mutation.lock().await.shutting_down = true;
        self.stop_all().await;
        self.inner.scheduler.lock().await.shutdown().await?;
        Ok(())
    }

    /// Create an interval task independent of the wall-clock cron scheduler.
    /// Repeated cron-scheduler jobs can strand next_tick behind last_tick after
    /// a late tick. Tokio's Skip policy advances directly to a future deadline.
    fn create_interval_timer(
        &self,
        rule_id: i64,
        interval: u64,
        unit: TimeUnit,
    ) -> Result<ActiveTimer> {
        let seconds_per_unit = match unit {
            TimeUnit::Seconds => 1,
            TimeUnit::Minutes => 60,
            TimeUnit::Hours => 3600,
            TimeUnit::Days => 86400,
        };
        let interval_seconds = interval
            .checked_mul(seconds_per_unit)
            .context("Interval is too large to represent in seconds")?;

        if interval_seconds == 0 {
            return Err(anyhow::anyhow!("Interval must be greater than 0"));
        }

        let api_client = self.inner.api_client.clone();
        let sensor_ref = self.inner.sensor_ref.clone();
        let duration = Duration::from_secs(interval_seconds);
        let first_tick = Instant::now()
            .checked_add(duration)
            .context("Interval is too large for the monotonic clock")?;

        info!(
            "Creating interval task for rule {} (interval: {}s)",
            rule_id, interval_seconds
        );

        let task = tokio::spawn(run_interval_timer(
            rule_id,
            first_tick,
            duration,
            move |count| {
                let api_client = api_client.clone();
                let sensor_ref = sensor_ref.clone();
                let interval_secs = interval_seconds;

                async move {
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
                    let request =
                        CreateEventRequest::new("core.intervaltimer".to_string(), payload)
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
                }
            },
        ));

        Ok(ActiveTimer::Interval {
            id: Uuid::new_v4(),
            task,
        })
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
                if mutation.shutting_down {
                    return Ok(());
                }

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

                let active_job_uuid = this
                    .inner
                    .active_jobs
                    .read()
                    .await
                    .get(&rule_id)
                    .map(ActiveTimer::id);
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
                .insert(rule_id, ActiveTimer::Scheduled(job_uuid));

            Ok(())
        })
    }
}

/// Publish once on recovery, skip the other overdue ticks, and keep the cadence.
/// Awaiting publication bounds each rule to one in-flight emission, including retries.
async fn run_interval_timer<F, Fut>(
    rule_id: i64,
    first_tick: Instant,
    period: Duration,
    mut publish: F,
) where
    F: FnMut(u64) -> Fut,
    Fut: Future<Output = ()>,
{
    // Unlike interval(), interval_at() does not fire immediately on startup.
    let mut ticks = tokio::time::interval_at(first_tick, period);
    ticks.set_missed_tick_behavior(MissedTickBehavior::Skip);
    let mut execution_count = 0u64;
    loop {
        let scheduled_tick = ticks.tick().await;
        let delay = Instant::now().saturating_duration_since(scheduled_tick);
        // A whole additional interval must have elapsed to distinguish missed
        // ticks from ordinary executor jitter. Skip retains the original phase.
        let skipped_ticks = delay.as_secs() / period.as_secs();
        if skipped_ticks > 0 {
            let recovered_at = Utc::now();
            let expected_at = chrono::Duration::from_std(delay)
                .ok()
                .and_then(|delay| recovered_at.checked_sub_signed(delay))
                .map(|time| time.to_rfc3339());
            warn!(
                rule_id,
                interval_seconds = period.as_secs(),
                missed_ticks = skipped_ticks + 1,
                skipped_ticks,
                delay_ms = delay.as_millis() as u64,
                expected_at,
                recovered_at = %recovered_at.to_rfc3339(),
                "Interval timer missed ticks; publishing one recovery event and skipping the backlog"
            );
        }
        execution_count += 1;
        publish(execution_count).await;
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
    use std::io::Write;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;
    use tokio::sync::{mpsc, oneshot, Semaphore};
    use tracing::instrument::WithSubscriber;

    #[derive(Clone, Default)]
    struct CapturedLogs(Arc<std::sync::Mutex<Vec<u8>>>);

    impl Write for CapturedLogs {
        fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buffer);
            Ok(buffer.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl CapturedLogs {
        fn warnings(&self) -> Vec<serde_json::Value> {
            String::from_utf8(self.0.lock().unwrap().clone())
                .unwrap()
                .lines()
                .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
                .filter(|entry| entry["level"] == "WARN")
                .collect()
        }
    }

    fn spawn_test_interval<F, Fut>(
        period_seconds: u64,
        publish: F,
    ) -> (JoinHandle<()>, CapturedLogs)
    where
        F: FnMut(u64) -> Fut + Send + 'static,
        Fut: Future<Output = ()> + Send + 'static,
    {
        let logs = CapturedLogs::default();
        let writer = logs.clone();
        let subscriber = tracing_subscriber::fmt()
            .json()
            .without_time()
            .with_writer(move || writer.clone())
            .finish();
        let period = Duration::from_secs(period_seconds);
        let task = tokio::spawn(
            run_interval_timer(42, Instant::now() + period, period, publish)
                .with_subscriber(subscriber),
        );
        (task, logs)
    }

    async fn stop_test_task(task: JoinHandle<()>) {
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
    }

    async fn next_interval_event(
        events: &mut mpsc::UnboundedReceiver<(u64, Instant)>,
    ) -> (u64, Instant) {
        tokio::time::timeout(Duration::from_secs(1), events.recv())
            .await
            .expect("timer did not publish promptly")
            .expect("interval task exited")
    }

    struct IntervalHarness {
        task: JoinHandle<()>,
        events: mpsc::UnboundedReceiver<(u64, Instant)>,
        logs: CapturedLogs,
    }

    impl IntervalHarness {
        fn new(period_seconds: u64) -> Self {
            let (sender, events) = mpsc::unbounded_channel();
            let (task, logs) = spawn_test_interval(period_seconds, move |count| {
                sender.send((count, Instant::now())).unwrap();
                std::future::ready(())
            });
            Self { task, events, logs }
        }

        async fn next(&mut self) -> (u64, Instant) {
            next_interval_event(&mut self.events).await
        }

        async fn assert_no_event(&mut self) {
            tokio::task::yield_now().await;
            assert!(self.events.try_recv().is_err(), "unexpected catch-up event");
        }

        async fn stop(self) {
            stop_test_task(self.task).await;
        }
    }

    struct EventRequest {
        body: serde_json::Value,
        respond: oneshot::Sender<u16>,
    }

    struct EventApi {
        client: ApiClient,
        requests: mpsc::UnboundedReceiver<EventRequest>,
        task: JoinHandle<()>,
    }

    impl EventApi {
        async fn start() -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let (sender, requests) = mpsc::unbounded_channel();
            let task = tokio::spawn(async move {
                loop {
                    let (mut socket, _) = listener.accept().await.unwrap();
                    let mut bytes = Vec::new();
                    let header_end = loop {
                        let mut buffer = [0; 1024];
                        let size = socket.read(&mut buffer).await.unwrap();
                        assert_ne!(size, 0, "client closed before sending headers");
                        bytes.extend_from_slice(&buffer[..size]);
                        if let Some(index) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
                            break index + 4;
                        }
                    };
                    let headers = std::str::from_utf8(&bytes[..header_end]).unwrap();
                    assert!(headers.starts_with("POST /api/v1/events HTTP/1.1\r\n"));
                    let content_length = headers
                        .lines()
                        .find_map(|line| {
                            let (name, value) = line.split_once(':')?;
                            name.eq_ignore_ascii_case("content-length")
                                .then(|| value.trim().parse::<usize>().unwrap())
                        })
                        .unwrap();
                    let mut body = bytes[header_end..].to_vec();
                    body.resize(content_length, 0);
                    socket
                        .read_exact(&mut body[bytes.len() - header_end..])
                        .await
                        .unwrap();
                    let (respond, response) = oneshot::channel();
                    sender
                        .send(EventRequest {
                            body: serde_json::from_slice(&body).unwrap(),
                            respond,
                        })
                        .unwrap();
                    let status = response.await.unwrap();
                    let body = if status == 200 {
                        r#"{"data":{"id":123}}"#
                    } else {
                        r#"{"error":"test failure"}"#
                    };
                    let response = format!(
                        "HTTP/1.1 {status} Test\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    // A cancellation test intentionally closes the client first.
                    let _ = socket.write_all(response.as_bytes()).await;
                }
            });
            Self {
                client: ApiClient::new(format!("http://{address}"), "test-token".to_string()),
                requests,
                task,
            }
        }

        async fn next(&mut self) -> EventRequest {
            tokio::time::timeout(Duration::from_secs(5), self.requests.recv())
                .await
                .expect("timer did not call the event API")
                .expect("event API exited")
        }

        async fn stop(self) {
            stop_test_task(self.task).await;
        }
    }

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

    #[tokio::test(start_paused = true)]
    async fn test_interval_job_advances_overdue_deadline() {
        let start = Instant::now();
        let mut timer = IntervalHarness::new(10);
        timer.assert_no_event().await;
        tokio::time::advance(Duration::from_secs(10)).await;
        assert_eq!(timer.next().await, (1, start + Duration::from_secs(10)));

        // Do not poll the scheduler for several intervals. One tick at 45s
        // represents the missed 20s, 30s and 40s ticks, without a replay burst.
        tokio::time::advance(Duration::from_secs(35)).await;
        assert_eq!(timer.next().await, (2, start + Duration::from_secs(45)));
        timer.assert_no_event().await;
        let warnings = timer.logs.warnings();
        assert_eq!(warnings.len(), 1);
        let fields = &warnings[0]["fields"];
        assert_eq!(fields["rule_id"], 42);
        assert_eq!(fields["interval_seconds"], 10);
        assert_eq!(fields["missed_ticks"], 3);
        assert_eq!(fields["skipped_ticks"], 2);
        assert_eq!(fields["delay_ms"], 25000);
        assert!(fields["expected_at"].as_str().is_some());
        assert!(fields["recovered_at"].as_str().is_some());

        // Keep the original phase, rather than waiting until 55s after recovery.
        tokio::time::advance(Duration::from_secs(4)).await;
        timer.assert_no_event().await;
        tokio::time::advance(Duration::from_secs(1)).await;
        assert_eq!(timer.next().await, (3, start + Duration::from_secs(50)));
        for count in 4..=6 {
            tokio::time::advance(Duration::from_secs(10)).await;
            assert_eq!(timer.next().await.0, count);
            timer.assert_no_event().await;
        }
        assert_eq!(timer.logs.warnings().len(), 1);
        timer.stop().await;
    }

    #[tokio::test(start_paused = true)]
    async fn test_interval_recovers_from_repeated_stalls_and_exact_boundaries() {
        let start = Instant::now();
        let mut timer = IntervalHarness::new(10);
        for (count, time) in [(1, 40), (2, 80), (3, 120)] {
            tokio::time::advance(Duration::from_secs(40)).await;
            assert_eq!(
                timer.next().await,
                (count, start + Duration::from_secs(time))
            );
            timer.assert_no_event().await;
        }
        assert_eq!(timer.logs.warnings().len(), 3);
        tokio::time::advance(Duration::from_secs(10)).await;
        assert_eq!(timer.next().await.0, 4);
        timer.assert_no_event().await;
        timer.stop().await;
    }

    #[tokio::test(start_paused = true)]
    async fn test_interval_jitter_does_not_warn_or_shift_cadence() {
        let start = Instant::now();
        let mut timer = IntervalHarness::new(10);
        tokio::time::advance(Duration::from_millis(10020)).await;
        assert_eq!(timer.next().await.0, 1);
        assert!(timer.logs.warnings().is_empty());
        tokio::time::advance(Duration::from_millis(9980)).await;
        assert_eq!(timer.next().await, (2, start + Duration::from_secs(20)));
        timer.stop().await;
    }

    #[tokio::test(start_paused = true)]
    async fn test_interval_slow_publication_does_not_overlap_or_replay_backlog() {
        let start = Instant::now();
        let completion = Arc::new(Semaphore::new(0));
        let permits = completion.clone();
        let (sender, mut events) = mpsc::unbounded_channel();
        let (task, logs) = spawn_test_interval(10, move |count| {
            let completion = completion.clone();
            sender.send((count, Instant::now())).unwrap();
            async move {
                // Model a slow API request or its retry sequence.
                completion.acquire().await.unwrap().forget();
            }
        });

        tokio::time::advance(Duration::from_secs(10)).await;
        assert_eq!(next_interval_event(&mut events).await.0, 1);
        tokio::time::advance(Duration::from_secs(35)).await;
        tokio::task::yield_now().await;
        assert!(events.try_recv().is_err(), "emissions must not overlap");
        permits.add_permits(1);
        assert_eq!(
            next_interval_event(&mut events).await,
            (2, start + Duration::from_secs(45))
        );
        permits.add_permits(1);
        tokio::task::yield_now().await;
        assert!(events.try_recv().is_err(), "backlog must not be replayed");
        assert_eq!(logs.warnings().len(), 1);
        tokio::time::advance(Duration::from_secs(5)).await;
        assert_eq!(
            next_interval_event(&mut events).await,
            (3, start + Duration::from_secs(50))
        );
        // Cancellation must also work while publication is pending.
        stop_test_task(task).await;
    }

    #[tokio::test(start_paused = true)]
    async fn test_interval_rules_keep_independent_cadences() {
        let mut fast = IntervalHarness::new(10);
        let mut slow = IntervalHarness::new(15);
        tokio::time::advance(Duration::from_secs(35)).await;
        assert_eq!(fast.next().await.0, 1);
        assert_eq!(slow.next().await.0, 1);
        fast.assert_no_event().await;
        slow.assert_no_event().await;
        tokio::time::advance(Duration::from_secs(5)).await;
        assert_eq!(fast.next().await.0, 2);
        slow.assert_no_event().await;
        tokio::time::advance(Duration::from_secs(5)).await;
        assert_eq!(slow.next().await.0, 2);
        fast.assert_no_event().await;
        fast.stop().await;
        slow.stop().await;
    }

    #[tokio::test(flavor = "current_thread")]
    async fn test_interval_publishes_and_resumes_after_real_scheduler_stall() {
        let mut api = EventApi::start().await;
        let manager = TimerManager::new(api.client.clone(), "core.timer_sensor".to_string())
            .await
            .unwrap();
        manager
            .start_timer(
                42,
                TimerConfig::Interval {
                    interval: 1,
                    unit: TimeUnit::Seconds,
                },
            )
            .await
            .unwrap();
        let first = api.next().await;
        assert_eq!(first.body["payload"]["execution_count"], 1);
        first.respond.send(200).unwrap();
        // Deliberately block this test-owned single-thread runtime. Unlike
        // paused Tokio time, this also delays the old UTC-based scheduler and
        // reproduces the deployed failure through real HTTP event publication.
        std::thread::sleep(Duration::from_millis(3500));
        for count in 2..=4 {
            let request = api.next().await;
            assert_eq!(request.body["trigger_ref"], "core.intervaltimer");
            assert_eq!(request.body["trigger_instance_id"], "rule_42");
            let payload = &request.body["payload"];
            assert_eq!(payload["type"], "interval");
            assert_eq!(payload["execution_count"], count);
            assert_eq!(payload["interval_seconds"], 1);
            assert_eq!(payload["sensor_ref"], "core.timer_sensor");
            chrono::DateTime::parse_from_rfc3339(payload["fired_at"].as_str().unwrap()).unwrap();
            request.respond.send(200).unwrap();
        }
        manager.shutdown().await.unwrap();
        api.stop().await;
    }

    #[tokio::test]
    async fn test_interval_survives_failed_publication_and_cancels_pending_request() {
        let mut api = EventApi::start().await;
        let manager = TimerManager::new(api.client.clone(), "core.timer_sensor".to_string())
            .await
            .unwrap();
        manager
            .start_timer(
                42,
                TimerConfig::Interval {
                    interval: 1,
                    unit: TimeUnit::Seconds,
                },
            )
            .await
            .unwrap();
        let mut first_body = None;
        for _ in 0..3 {
            let request = api.next().await;
            assert_eq!(request.body["payload"]["execution_count"], 1);
            if let Some(body) = &first_body {
                assert_eq!(&request.body, body, "retries must reuse the same event");
            } else {
                first_body = Some(request.body);
            }
            request.respond.send(503).unwrap();
        }
        let next = api.next().await;
        assert_eq!(next.body["payload"]["execution_count"], 2);
        // Hold this HTTP response while stopping the timer. Stop must not wait
        // for the request timeout or leave a detached retry task behind.
        let task = interval_abort_handle(&manager, 42).await;
        tokio::time::timeout(Duration::from_secs(1), manager.stop_timer(42))
            .await
            .unwrap();
        assert!(task.is_finished());
        next.respond.send(200).unwrap();
        manager.shutdown().await.unwrap();
        api.stop().await;
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
    async fn test_interval_timer_rejects_overflow_without_spawning_tasks() {
        let manager = test_manager().await;
        for unit in [
            TimeUnit::Seconds,
            TimeUnit::Minutes,
            TimeUnit::Hours,
            TimeUnit::Days,
        ] {
            let result = manager
                .start_timer(
                    42,
                    TimerConfig::Interval {
                        interval: u64::MAX,
                        unit,
                    },
                )
                .await;
            assert!(result.is_err(), "oversized {unit:?} interval must fail");
            assert_eq!(manager.timer_count().await, 0);
        }
        manager.shutdown().await.unwrap();
    }

    async fn interval_abort_handle(
        manager: &TimerManager,
        rule_id: i64,
    ) -> tokio::task::AbortHandle {
        match manager
            .inner
            .active_jobs
            .read()
            .await
            .get(&rule_id)
            .unwrap()
        {
            ActiveTimer::Interval { task, .. } => task.abort_handle(),
            ActiveTimer::Scheduled(_) => panic!("expected interval task"),
        }
    }

    #[tokio::test]
    async fn test_interval_task_is_not_detached_when_registration_is_dropped() {
        let manager = test_manager().await;
        let timer = manager
            .create_interval_timer(42, 60, TimeUnit::Seconds)
            .unwrap();
        let abort = match &timer {
            ActiveTimer::Interval { task, .. } => task.abort_handle(),
            ActiveTimer::Scheduled(_) => panic!("expected interval task"),
        };
        drop(timer);
        tokio::time::timeout(Duration::from_secs(1), async {
            while !abort.is_finished() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("dropped interval task was detached instead of aborted");
        manager.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn test_interval_replacement_stop_all_and_shutdown_join_tasks() {
        let manager = test_manager().await;
        let config = TimerConfig::Interval {
            interval: 60,
            unit: TimeUnit::Seconds,
        };
        manager.start_timer(42, config.clone()).await.unwrap();
        let original = interval_abort_handle(&manager, 42).await;
        manager.start_timer(42, config.clone()).await.unwrap();
        assert!(
            !original.is_finished(),
            "unchanged config must keep the task"
        );
        manager
            .start_timer(
                42,
                TimerConfig::Cron {
                    expression: "0 * * * * *".to_string(),
                },
            )
            .await
            .unwrap();
        assert!(original.is_finished(), "replacement must join the old task");
        manager.start_timer(42, config.clone()).await.unwrap();
        let replacement = interval_abort_handle(&manager, 42).await;
        manager.stop_timer(42).await;
        assert!(replacement.is_finished());

        manager.start_timer(42, config.clone()).await.unwrap();
        let stopped = interval_abort_handle(&manager, 42).await;
        manager.stop_all().await;
        assert!(stopped.is_finished());
        assert!(manager.active_rule_ids().await.is_empty());

        manager.start_timer(42, config).await.unwrap();
        let shutdown = interval_abort_handle(&manager, 42).await;
        manager.shutdown().await.unwrap();
        assert!(shutdown.is_finished());
        assert_eq!(manager.timer_count().await, 0);
        assert!(manager
            .start_timer(
                42,
                TimerConfig::Interval {
                    interval: 60,
                    unit: TimeUnit::Seconds
                }
            )
            .await
            .is_err());
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
