//! Shared types for timer sensor
//!
//! Defines timer configurations and common data structures.
//! Updated: 2026-02-05 - Fixed TimerConfig parsing to use trigger_ref

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Timer configuration for different timer types
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum TimerConfig {
    /// Interval-based timer (fires every N seconds/minutes/hours)
    Interval {
        /// Number of units between fires
        interval: u64,
        /// Unit of time (seconds, minutes, hours, days)
        #[serde(default = "default_unit")]
        unit: TimeUnit,
    },
    /// Cron-based timer (fires based on cron expression)
    Cron {
        /// Cron expression (e.g., "0 0 * * *")
        expression: String,
    },
    /// Date/time-based timer (fires at a specific time)
    DateTime {
        /// ISO 8601 timestamp to fire at
        fire_at: DateTime<Utc>,
    },
    /// Recurrence-rule (RFC 5545 RRULE) timer
    ///
    /// Supports schedules that cron and simple intervals can't express, such as
    /// "every other Monday at 9am", "the 2nd Tuesday of each month", or
    /// "the last weekday of each month".
    RRule {
        /// RRULE body without the `RRULE:` prefix
        /// (e.g., `FREQ=WEEKLY;INTERVAL=2;BYDAY=MO`).
        rule: String,
        /// Anchor instant for the recurrence. Required by RFC 5545.
        dtstart: DateTime<Utc>,
        /// Optional IANA timezone name (e.g., `America/New_York`). When set,
        /// `dtstart` is interpreted in this timezone for daylight-saving and
        /// wall-clock semantics. Defaults to UTC.
        timezone: Option<String>,
    },
}

fn default_unit() -> TimeUnit {
    TimeUnit::Seconds
}

/// Allowed FREQ values per RFC 5545 §3.3.10.
const ALLOWED_FREQ: &[&str] = &[
    "SECONDLY", "MINUTELY", "HOURLY", "DAILY", "WEEKLY", "MONTHLY", "YEARLY",
];

/// Allowed week-day codes for BYDAY / WKST.
const ALLOWED_WEEKDAYS: &[&str] = &["MO", "TU", "WE", "TH", "FR", "SA", "SU"];

/// Build an RRULE body string from structured trigger parameters.
///
/// The trigger schema exposes RRULE parts as discrete, type-checked fields
/// (e.g. `freq`, `interval`, `by_day`) instead of one opaque string. This
/// helper assembles them into the canonical `KEY=VAL;KEY=VAL` form expected
/// by the `rrule` crate.
///
/// As an escape hatch for edge cases not covered by the structured fields,
/// callers may still pass a raw `rule` string (with or without a leading
/// `RRULE:` prefix); when present it short-circuits the assembly.
fn build_rrule_from_params(params: &serde_json::Value) -> anyhow::Result<String> {
    if let Some(raw) = params.get("rule").and_then(|v| v.as_str()) {
        let trimmed = raw.trim().trim_start_matches("RRULE:").trim().to_string();
        if !trimmed.is_empty() {
            return Ok(trimmed);
        }
    }

    let freq = params
        .get("freq")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("Missing required 'freq' field for RRULE timer"))?
        .to_uppercase();

    if !ALLOWED_FREQ.contains(&freq.as_str()) {
        anyhow::bail!(
            "Invalid 'freq' value '{}'; must be one of {}",
            freq,
            ALLOWED_FREQ.join(", ")
        );
    }

    let mut parts: Vec<String> = vec![format!("FREQ={}", freq)];

    if let Some(v) = params.get("interval") {
        let n = v
            .as_u64()
            .ok_or_else(|| anyhow::anyhow!("'interval' must be a positive integer"))?;
        if n < 1 {
            anyhow::bail!("'interval' must be >= 1");
        }
        parts.push(format!("INTERVAL={}", n));
    }

    if let Some(v) = params.get("count") {
        let n = v
            .as_u64()
            .ok_or_else(|| anyhow::anyhow!("'count' must be a positive integer"))?;
        if n < 1 {
            anyhow::bail!("'count' must be >= 1");
        }
        if params.get("until").is_some() {
            anyhow::bail!("'count' and 'until' are mutually exclusive (RFC 5545 §3.3.10)");
        }
        parts.push(format!("COUNT={}", n));
    }

    if let Some(v) = params.get("until") {
        let until: DateTime<Utc> = serde_json::from_value(v.clone())
            .map_err(|e| anyhow::anyhow!("Failed to parse 'until' as datetime: {}", e))?;
        parts.push(format!("UNTIL={}", until.format("%Y%m%dT%H%M%SZ")));
    }

    if let Some(values) = params.get("by_day") {
        let days = values
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("'by_day' must be an array of strings"))?;
        let formatted: Vec<String> = days
            .iter()
            .map(|d| {
                let s = d
                    .as_str()
                    .ok_or_else(|| anyhow::anyhow!("'by_day' entries must be strings"))?
                    .trim()
                    .to_uppercase();
                validate_byday_token(&s)?;
                Ok::<String, anyhow::Error>(s)
            })
            .collect::<Result<_, _>>()?;
        if !formatted.is_empty() {
            parts.push(format!("BYDAY={}", formatted.join(",")));
        }
    }

    push_by_int_array(&mut parts, params, "by_month_day", "BYMONTHDAY", -31, 31)?;
    push_by_int_array(&mut parts, params, "by_month", "BYMONTH", 1, 12)?;
    push_by_int_array(&mut parts, params, "by_year_day", "BYYEARDAY", -366, 366)?;
    push_by_int_array(&mut parts, params, "by_week_no", "BYWEEKNO", -53, 53)?;
    push_by_int_array(&mut parts, params, "by_hour", "BYHOUR", 0, 23)?;
    push_by_int_array(&mut parts, params, "by_minute", "BYMINUTE", 0, 59)?;
    push_by_int_array(&mut parts, params, "by_second", "BYSECOND", 0, 60)?;
    push_by_int_array(&mut parts, params, "by_set_pos", "BYSETPOS", -366, 366)?;

    if let Some(v) = params.get("week_start").and_then(|v| v.as_str()) {
        let s = v.trim().to_uppercase();
        if !ALLOWED_WEEKDAYS.contains(&s.as_str()) {
            anyhow::bail!(
                "'week_start' must be one of {}",
                ALLOWED_WEEKDAYS.join(", ")
            );
        }
        parts.push(format!("WKST={}", s));
    }

    Ok(parts.join(";"))
}

fn validate_byday_token(token: &str) -> anyhow::Result<()> {
    let len = token.len();
    if len < 2 {
        anyhow::bail!("Invalid BYDAY token '{}'", token);
    }
    let weekday = &token[len - 2..];
    if !ALLOWED_WEEKDAYS.contains(&weekday) {
        anyhow::bail!(
            "Invalid BYDAY weekday '{}'; must end in one of {}",
            token,
            ALLOWED_WEEKDAYS.join(", ")
        );
    }
    let prefix = &token[..len - 2];
    if !prefix.is_empty() {
        prefix.parse::<i32>().map_err(|_| {
            anyhow::anyhow!(
                "Invalid BYDAY ordinal prefix in '{}' (expected integer like '2' or '-1')",
                token
            )
        })?;
    }
    Ok(())
}

fn push_by_int_array(
    parts: &mut Vec<String>,
    params: &serde_json::Value,
    field: &str,
    rrule_key: &str,
    min: i64,
    max: i64,
) -> anyhow::Result<()> {
    let Some(values) = params.get(field) else {
        return Ok(());
    };
    let arr = values
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("'{}' must be an array of integers", field))?;
    let formatted: Vec<String> = arr
        .iter()
        .map(|n| {
            let v = n
                .as_i64()
                .ok_or_else(|| anyhow::anyhow!("'{}' entries must be integers", field))?;
            if v < min || v > max {
                anyhow::bail!("'{}' value {} is out of range [{}, {}]", field, v, min, max);
            }
            Ok(v.to_string())
        })
        .collect::<Result<_, _>>()?;
    if !formatted.is_empty() {
        parts.push(format!("{}={}", rrule_key, formatted.join(",")));
    }
    Ok(())
}

/// Time unit for interval timers
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TimeUnit {
    Seconds,
    Minutes,
    Hours,
    Days,
}

impl TimerConfig {
    /// Deserialize TimerConfig from JSON value based on trigger_ref
    ///
    /// Maps trigger_ref to the appropriate TimerConfig variant:
    /// - "core.intervaltimer" -> TimerConfig::Interval
    /// - "core.crontimer" -> TimerConfig::Cron
    /// - "core.datetimetimer" -> TimerConfig::DateTime
    pub fn from_trigger_params(
        trigger_ref: &str,
        params: serde_json::Value,
    ) -> anyhow::Result<Self> {
        match trigger_ref {
            "core.intervaltimer" => {
                // Parse interval and unit from params
                let interval =
                    params
                        .get("interval")
                        .and_then(|v| v.as_u64())
                        .ok_or_else(|| {
                            anyhow::anyhow!(
                                "Missing or invalid 'interval' field in params: {}",
                                serde_json::to_string(&params)
                                    .unwrap_or_else(|_| "<invalid>".to_string())
                            )
                        })?;

                let unit = if let Some(unit_val) = params.get("unit") {
                    serde_json::from_value(unit_val.clone())
                        .map_err(|e| anyhow::anyhow!("Failed to parse 'unit' field: {}", e))?
                } else {
                    TimeUnit::Seconds
                };

                Ok(TimerConfig::Interval { interval, unit })
            }
            "core.crontimer" => {
                let expression = params
                    .get("expression")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| {
                        anyhow::anyhow!(
                            "Missing or invalid 'expression' field in params: {}",
                            serde_json::to_string(&params)
                                .unwrap_or_else(|_| "<invalid>".to_string())
                        )
                    })?
                    .to_string();

                Ok(TimerConfig::Cron { expression })
            }
            "core.datetimetimer" => {
                let fire_at = params.get("fire_at").ok_or_else(|| {
                    anyhow::anyhow!(
                        "Missing 'fire_at' field in params: {}",
                        serde_json::to_string(&params).unwrap_or_else(|_| "<invalid>".to_string())
                    )
                })?;

                let fire_at: DateTime<Utc> = serde_json::from_value(fire_at.clone())
                    .map_err(|e| anyhow::anyhow!("Failed to parse 'fire_at' as DateTime: {}", e))?;

                Ok(TimerConfig::DateTime { fire_at })
            }
            "core.rruletimer" => {
                let rule = build_rrule_from_params(&params)?;

                let dtstart_value = params.get("dtstart").ok_or_else(|| {
                    anyhow::anyhow!(
                        "Missing 'dtstart' field in params: {}",
                        serde_json::to_string(&params).unwrap_or_else(|_| "<invalid>".to_string())
                    )
                })?;

                let dtstart: DateTime<Utc> = serde_json::from_value(dtstart_value.clone())
                    .map_err(|e| anyhow::anyhow!("Failed to parse 'dtstart' as DateTime: {}", e))?;

                let timezone = params
                    .get("timezone")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());

                Ok(TimerConfig::RRule {
                    rule,
                    dtstart,
                    timezone,
                })
            }
            _ => Err(anyhow::anyhow!(
                "Unknown timer trigger type: {}",
                trigger_ref
            )),
        }
    }

    /// Calculate total interval in seconds
    #[allow(dead_code)]
    pub fn interval_seconds(&self) -> Option<u64> {
        match self {
            TimerConfig::Interval { interval, unit } => Some(match unit {
                TimeUnit::Seconds => *interval,
                TimeUnit::Minutes => interval * 60,
                TimeUnit::Hours => interval * 3600,
                TimeUnit::Days => interval * 86400,
            }),
            _ => None,
        }
    }

    /// Get the cron expression if this is a cron timer
    #[allow(dead_code)]
    pub fn cron_expression(&self) -> Option<&str> {
        match self {
            TimerConfig::Cron { expression } => Some(expression),
            _ => None,
        }
    }

    /// Get the fire time if this is a datetime timer
    #[allow(dead_code)]
    pub fn fire_time(&self) -> Option<DateTime<Utc>> {
        match self {
            TimerConfig::DateTime { fire_at } => Some(*fire_at),
            _ => None,
        }
    }

    /// Get the RRULE body if this is an RRULE timer
    #[allow(dead_code)]
    pub fn rrule(&self) -> Option<&str> {
        match self {
            TimerConfig::RRule { rule, .. } => Some(rule),
            _ => None,
        }
    }
}

/// Rule lifecycle event types
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "event_type", rename_all = "PascalCase")]
#[allow(clippy::enum_variant_names)]
pub enum RuleLifecycleEvent {
    RuleCreated {
        rule_id: i64,
        rule_ref: String,
        trigger_type: String,
        trigger_params: Option<serde_json::Value>,
        enabled: bool,
        timestamp: DateTime<Utc>,
    },
    RuleEnabled {
        rule_id: i64,
        rule_ref: String,
        trigger_type: String,
        trigger_params: Option<serde_json::Value>,
        timestamp: DateTime<Utc>,
    },
    RuleDisabled {
        rule_id: i64,
        rule_ref: String,
        trigger_type: String,
        timestamp: DateTime<Utc>,
    },
    RuleDeleted {
        rule_id: i64,
        rule_ref: String,
        trigger_type: String,
        timestamp: DateTime<Utc>,
    },
}

impl RuleLifecycleEvent {
    /// Get the rule ID from any event type
    #[allow(dead_code)]
    pub fn rule_id(&self) -> i64 {
        match self {
            RuleLifecycleEvent::RuleCreated { rule_id, .. }
            | RuleLifecycleEvent::RuleEnabled { rule_id, .. }
            | RuleLifecycleEvent::RuleDisabled { rule_id, .. }
            | RuleLifecycleEvent::RuleDeleted { rule_id, .. } => *rule_id,
        }
    }

    /// Get trigger params if available
    #[allow(dead_code)]
    pub fn trigger_params(&self) -> Option<&serde_json::Value> {
        match self {
            RuleLifecycleEvent::RuleCreated { trigger_params, .. }
            | RuleLifecycleEvent::RuleEnabled { trigger_params, .. } => trigger_params.as_ref(),
            _ => None,
        }
    }

    /// Check if rule should be active (created and enabled, or explicitly enabled)
    #[allow(dead_code)]
    pub fn is_active(&self) -> bool {
        match self {
            RuleLifecycleEvent::RuleCreated { enabled, .. } => *enabled,
            RuleLifecycleEvent::RuleEnabled { .. } => true,
            RuleLifecycleEvent::RuleDisabled { .. } | RuleLifecycleEvent::RuleDeleted { .. } => {
                false
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_timer_config_interval_seconds() {
        let config = TimerConfig::Interval {
            interval: 5,
            unit: TimeUnit::Seconds,
        };
        assert_eq!(config.interval_seconds(), Some(5));

        let config = TimerConfig::Interval {
            interval: 2,
            unit: TimeUnit::Minutes,
        };
        assert_eq!(config.interval_seconds(), Some(120));

        let config = TimerConfig::Interval {
            interval: 1,
            unit: TimeUnit::Hours,
        };
        assert_eq!(config.interval_seconds(), Some(3600));

        let config = TimerConfig::Interval {
            interval: 1,
            unit: TimeUnit::Days,
        };
        assert_eq!(config.interval_seconds(), Some(86400));
    }

    #[test]
    fn test_timer_config_cron() {
        let config = TimerConfig::Cron {
            expression: "0 0 * * *".to_string(),
        };
        assert_eq!(config.cron_expression(), Some("0 0 * * *"));
        assert_eq!(config.interval_seconds(), None);
    }

    #[test]
    fn test_timer_config_datetime() {
        let fire_at = Utc::now();
        let config = TimerConfig::DateTime { fire_at };
        assert_eq!(config.fire_time(), Some(fire_at));
        assert_eq!(config.interval_seconds(), None);
    }

    #[test]
    fn test_timer_config_from_trigger_params_interval() {
        let params = serde_json::json!({
            "interval": 30,
            "unit": "seconds"
        });

        let config = TimerConfig::from_trigger_params("core.intervaltimer", params).unwrap();
        assert_eq!(config.interval_seconds(), Some(30));
    }

    #[test]
    fn test_timer_config_from_trigger_params_interval_default_unit() {
        let params = serde_json::json!({
            "interval": 60
        });

        let config = TimerConfig::from_trigger_params("core.intervaltimer", params).unwrap();
        assert_eq!(config.interval_seconds(), Some(60));
    }

    #[test]
    fn test_timer_config_from_trigger_params_cron() {
        let params = serde_json::json!({
            "expression": "0 0 * * *"
        });

        let config = TimerConfig::from_trigger_params("core.crontimer", params).unwrap();
        assert_eq!(config.cron_expression(), Some("0 0 * * *"));
    }

    #[test]
    fn test_timer_config_from_trigger_params_datetime() {
        let fire_at = chrono::Utc::now();
        let params = serde_json::json!({
            "fire_at": fire_at
        });

        let config = TimerConfig::from_trigger_params("core.datetimetimer", params).unwrap();
        assert_eq!(config.fire_time(), Some(fire_at));
    }

    #[test]
    fn test_timer_config_from_trigger_params_rrule() {
        let dtstart = "2026-05-04T13:00:00Z";
        let params = serde_json::json!({
            "freq": "WEEKLY",
            "interval": 2,
            "by_day": ["MO"],
            "dtstart": dtstart,
            "timezone": "America/New_York",
        });

        let config = TimerConfig::from_trigger_params("core.rruletimer", params).unwrap();
        assert_eq!(config.rrule(), Some("FREQ=WEEKLY;INTERVAL=2;BYDAY=MO"));
        match config {
            TimerConfig::RRule {
                dtstart: ds,
                timezone,
                ..
            } => {
                assert_eq!(ds.to_rfc3339(), "2026-05-04T13:00:00+00:00");
                assert_eq!(timezone.as_deref(), Some("America/New_York"));
            }
            _ => panic!("expected RRule variant"),
        }
    }

    #[test]
    fn test_timer_config_from_trigger_params_rrule_strips_prefix() {
        let params = serde_json::json!({
            "rule": "RRULE:FREQ=DAILY",
            "dtstart": "2026-05-04T13:00:00Z",
        });

        let config = TimerConfig::from_trigger_params("core.rruletimer", params).unwrap();
        assert_eq!(config.rrule(), Some("FREQ=DAILY"));
    }

    #[test]
    fn test_timer_config_from_trigger_params_rrule_missing_dtstart() {
        let params = serde_json::json!({
            "freq": "WEEKLY",
        });

        let result = TimerConfig::from_trigger_params("core.rruletimer", params);
        assert!(result.is_err());
    }

    #[test]
    fn test_timer_config_rrule_structured_complex() {
        let params = serde_json::json!({
            "freq": "monthly",
            "by_day": ["MO", "TU", "WE", "TH", "FR"],
            "by_set_pos": [-1],
            "by_hour": [17],
            "by_minute": [0],
            "dtstart": "2026-05-29T17:00:00Z",
        });

        let config = TimerConfig::from_trigger_params("core.rruletimer", params).unwrap();
        assert_eq!(
            config.rrule(),
            Some("FREQ=MONTHLY;BYDAY=MO,TU,WE,TH,FR;BYHOUR=17;BYMINUTE=0;BYSETPOS=-1")
        );
    }

    #[test]
    fn test_timer_config_rrule_structured_byday_with_ordinal() {
        let params = serde_json::json!({
            "freq": "MONTHLY",
            "by_day": ["2TU"],
            "dtstart": "2026-05-12T15:00:00Z",
        });

        let config = TimerConfig::from_trigger_params("core.rruletimer", params).unwrap();
        assert_eq!(config.rrule(), Some("FREQ=MONTHLY;BYDAY=2TU"));
    }

    #[test]
    fn test_timer_config_rrule_structured_count_until_conflict() {
        let params = serde_json::json!({
            "freq": "DAILY",
            "count": 5,
            "until": "2026-12-31T23:59:59Z",
            "dtstart": "2026-05-01T00:00:00Z",
        });

        let result = TimerConfig::from_trigger_params("core.rruletimer", params);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("mutually exclusive"));
    }

    #[test]
    fn test_timer_config_rrule_structured_invalid_freq() {
        let params = serde_json::json!({
            "freq": "BIWEEKLY",
            "dtstart": "2026-05-01T00:00:00Z",
        });

        let result = TimerConfig::from_trigger_params("core.rruletimer", params);
        assert!(result.is_err());
    }

    #[test]
    fn test_timer_config_rrule_structured_invalid_byday() {
        let params = serde_json::json!({
            "freq": "WEEKLY",
            "by_day": ["XX"],
            "dtstart": "2026-05-01T00:00:00Z",
        });

        let result = TimerConfig::from_trigger_params("core.rruletimer", params);
        assert!(result.is_err());
    }

    #[test]
    fn test_timer_config_rrule_structured_until_formatted() {
        let params = serde_json::json!({
            "freq": "DAILY",
            "until": "2026-12-31T23:59:59Z",
            "dtstart": "2026-05-01T00:00:00Z",
        });

        let config = TimerConfig::from_trigger_params("core.rruletimer", params).unwrap();
        assert_eq!(config.rrule(), Some("FREQ=DAILY;UNTIL=20261231T235959Z"));
    }

    #[test]
    fn test_timer_config_rrule_raw_rule_overrides_structured() {
        let params = serde_json::json!({
            "rule": "FREQ=YEARLY",
            "freq": "DAILY",
            "dtstart": "2026-05-01T00:00:00Z",
        });

        let config = TimerConfig::from_trigger_params("core.rruletimer", params).unwrap();
        assert_eq!(config.rrule(), Some("FREQ=YEARLY"));
    }

    #[test]
    fn test_timer_config_rrule_bymonthday_and_byhour() {
        let params = serde_json::json!({
            "freq": "MONTHLY",
            "by_month_day": [1, 15],
            "by_hour": [9],
            "by_minute": [0],
            "dtstart": "2026-05-01T09:00:00Z",
        });

        let config = TimerConfig::from_trigger_params("core.rruletimer", params).unwrap();
        assert_eq!(
            config.rrule(),
            Some("FREQ=MONTHLY;BYMONTHDAY=1,15;BYHOUR=9;BYMINUTE=0")
        );
    }

    #[test]
    fn test_timer_config_from_trigger_params_unknown_trigger() {
        let params = serde_json::json!({
            "interval": 30
        });

        let result = TimerConfig::from_trigger_params("unknown.trigger", params);
        assert!(result.is_err());
    }

    #[test]
    fn test_rule_lifecycle_event_rule_id() {
        let event = RuleLifecycleEvent::RuleCreated {
            rule_id: 123,
            rule_ref: "test".to_string(),
            trigger_type: "core.timer".to_string(),
            trigger_params: None,
            enabled: true,
            timestamp: Utc::now(),
        };
        assert_eq!(event.rule_id(), 123);
    }

    #[test]
    fn test_rule_lifecycle_event_trigger_type() {
        let event = RuleLifecycleEvent::RuleEnabled {
            rule_id: 123,
            rule_ref: "test".to_string(),
            trigger_type: "core.timer".to_string(),
            trigger_params: None,
            timestamp: Utc::now(),
        };
        match event {
            RuleLifecycleEvent::RuleEnabled { trigger_type, .. } => {
                assert_eq!(trigger_type, "core.timer")
            }
            _ => panic!("expected RuleEnabled"),
        }
    }

    #[test]
    fn test_rule_lifecycle_event_is_active() {
        let event = RuleLifecycleEvent::RuleCreated {
            rule_id: 123,
            rule_ref: "test".to_string(),
            trigger_type: "core.timer".to_string(),
            trigger_params: None,
            enabled: true,
            timestamp: Utc::now(),
        };
        assert!(event.is_active());

        let event = RuleLifecycleEvent::RuleDisabled {
            rule_id: 123,
            rule_ref: "test".to_string(),
            trigger_type: "core.timer".to_string(),
            timestamp: Utc::now(),
        };
        assert!(!event.is_active());
    }
}
