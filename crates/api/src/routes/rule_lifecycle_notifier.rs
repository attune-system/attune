//! Notifier publication helpers for managed sensor rule-lifecycle updates.
//!
//! This path is separate from RabbitMQ rule lifecycle envelopes used by the
//! internal sensor service. Managed sensor processes consume this stream over
//! the notifier WebSocket.

use chrono::Utc;
use serde_json::json;
use sqlx::PgPool;

const RULE_LIFECYCLE_CHANNEL: &str = "rule_lifecycle_changed";

/// Signal a rule change. Keep the trigger reference for subscription scoping
/// and let consumers fetch rule configuration through the authenticated API.
pub async fn notify_rule_lifecycle_changed(
    db: &PgPool,
    event_type: &str,
    rule_id: i64,
    rule_ref: &str,
    trigger_ref: &str,
    active: bool,
) -> Result<(), sqlx::Error> {
    let payload = build_rule_lifecycle_payload(
        event_type,
        rule_id,
        rule_ref,
        trigger_ref,
        active,
        Utc::now(),
    );

    sqlx::query("SELECT pg_notify($1, $2)")
        .bind(RULE_LIFECYCLE_CHANNEL)
        .bind(payload)
        .execute(db)
        .await?;

    Ok(())
}

fn build_rule_lifecycle_payload(
    event_type: &str,
    rule_id: i64,
    rule_ref: &str,
    trigger_ref: &str,
    active: bool,
    timestamp: chrono::DateTime<Utc>,
) -> String {
    json!({
        "entity_type": "rule_lifecycle",
        "entity_id": rule_id,
        "event_type": event_type,
        "active": active,
        "rule_id": rule_id,
        "rule_ref": rule_ref,
        "trigger_ref": trigger_ref,
        "timestamp": timestamp,
        "auth_mode": "deferred",
    })
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rule_lifecycle_payload_contains_only_the_deferred_envelope() {
        let timestamp = Utc::now();
        let payload = build_rule_lifecycle_payload(
            "rule.enabled",
            42,
            "core.timer",
            "core.intervaltimer",
            true,
            timestamp,
        );
        let parsed: serde_json::Value = serde_json::from_str(&payload).unwrap();
        assert_eq!(
            parsed,
            json!({
                "entity_type": "rule_lifecycle",
                "entity_id": 42,
                "event_type": "rule.enabled",
                "active": true,
                "rule_id": 42,
                "rule_ref": "core.timer",
                "trigger_ref": "core.intervaltimer",
                "timestamp": timestamp,
                "auth_mode": "deferred",
            })
        );
    }
}
