//! Notifier publication helpers for managed sensor rule-lifecycle updates.
//!
//! This path is separate from RabbitMQ rule lifecycle envelopes used by the
//! internal sensor service. Managed sensor processes consume this stream over
//! the notifier WebSocket.

use chrono::Utc;
use serde_json::{json, Value as JsonValue};
use sqlx::PgPool;

const RULE_LIFECYCLE_CHANNEL: &str = "rule_lifecycle_changed";

/// Emit a PostgreSQL NOTIFY payload for managed-sensor rule lifecycle updates.
///
/// The notifier listens on [`RULE_LIFECYCLE_CHANNEL`] and forwards this payload
/// to WebSocket subscribers.
pub async fn notify_rule_lifecycle_changed(
    db: &PgPool,
    event_type: &str,
    rule_id: i64,
    rule_ref: &str,
    trigger_ref: &str,
    trigger_params: Option<&JsonValue>,
    active: bool,
) -> Result<(), sqlx::Error> {
    let payload = json!({
        "entity_type": "rule_lifecycle",
        "entity_id": rule_id,
        "event_type": event_type,
        "active": active,
        "rule_id": rule_id,
        "rule_ref": rule_ref,
        "trigger_ref": trigger_ref,
        "trigger_params": trigger_params.cloned(),
        "timestamp": Utc::now(),
    })
    .to_string();

    sqlx::query("SELECT pg_notify($1, $2)")
        .bind(RULE_LIFECYCLE_CHANNEL)
        .bind(payload)
        .execute(db)
        .await?;

    Ok(())
}
