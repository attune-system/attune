//! Notifier publication helpers for managed sensor rule-lifecycle updates.
//!
//! This path is separate from RabbitMQ rule lifecycle envelopes used by the
//! internal sensor service. Managed sensor processes consume this stream over
//! the notifier WebSocket.

use chrono::Utc;
use serde_json::{json, Value as JsonValue};
use sqlx::PgPool;

const RULE_LIFECYCLE_CHANNEL: &str = "rule_lifecycle_changed";

/// Safe upper bound for a PostgreSQL NOTIFY payload. The hard limit is 8000
/// bytes; 7000 leaves headroom for channel/frame overhead and multibyte
/// encodings. Payloads above this fall back to a compact envelope.
const NOTIFY_PAYLOAD_SAFE_BYTES: usize = 7000;

/// Emit a PostgreSQL NOTIFY payload for managed-sensor rule lifecycle updates.
///
/// The notifier listens on [`RULE_LIFECYCLE_CHANNEL`] and forwards this payload
/// to WebSocket subscribers.
///
/// `trigger_params` is caller-supplied and unbounded, so the full payload can
/// exceed the NOTIFY size limit. When it does, a compact fallback is emitted
/// with `auth_mode: "deferred"` and `trigger_params` omitted; the notifier
/// retains enough to route (entity + `trigger_ref` for sensor-token scoping),
/// and consumers can refetch the rule for the dropped detail.
pub async fn notify_rule_lifecycle_changed(
    db: &PgPool,
    event_type: &str,
    rule_id: i64,
    rule_ref: &str,
    trigger_ref: &str,
    trigger_params: Option<&JsonValue>,
    active: bool,
) -> Result<(), sqlx::Error> {
    let payload = build_rule_lifecycle_payload(
        event_type,
        rule_id,
        rule_ref,
        trigger_ref,
        trigger_params,
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

/// Build the NOTIFY payload string, applying the size guard. Extracted as a
/// pure function so both the full and compact/deferred branches are testable
/// without a database.
fn build_rule_lifecycle_payload(
    event_type: &str,
    rule_id: i64,
    rule_ref: &str,
    trigger_ref: &str,
    trigger_params: Option<&JsonValue>,
    active: bool,
    timestamp: chrono::DateTime<Utc>,
) -> String {
    let full = json!({
        "entity_type": "rule_lifecycle",
        "entity_id": rule_id,
        "event_type": event_type,
        "active": active,
        "rule_id": rule_id,
        "rule_ref": rule_ref,
        "trigger_ref": trigger_ref,
        "trigger_params": trigger_params.cloned(),
        "timestamp": timestamp,
        "auth_mode": "full",
    })
    .to_string();

    if full.len() <= NOTIFY_PAYLOAD_SAFE_BYTES {
        return full;
    }

    // Compact fallback: keep routing/auth-critical keys, drop trigger_params.
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
    fn small_payload_uses_full_envelope() {
        let params = json!({ "interval": 30 });
        let payload = build_rule_lifecycle_payload(
            "activated",
            1,
            "core.timer",
            "core.intervaltimer",
            Some(&params),
            true,
            Utc::now(),
        );
        let parsed: JsonValue = serde_json::from_str(&payload).unwrap();
        assert_eq!(parsed["auth_mode"], "full");
        assert!(parsed.get("trigger_params").is_some());
        assert_eq!(parsed["trigger_ref"], "core.intervaltimer");
        assert!(payload.len() <= NOTIFY_PAYLOAD_SAFE_BYTES);
    }

    #[test]
    fn oversized_payload_falls_back_to_compact_deferred_envelope() {
        // trigger_params large enough to blow past the safe NOTIFY budget.
        let params = json!({ "blob": "x".repeat(NOTIFY_PAYLOAD_SAFE_BYTES + 500) });
        let payload = build_rule_lifecycle_payload(
            "activated",
            42,
            "core.timer",
            "core.intervaltimer",
            Some(&params),
            true,
            Utc::now(),
        );
        assert!(payload.len() <= NOTIFY_PAYLOAD_SAFE_BYTES);
        let parsed: JsonValue = serde_json::from_str(&payload).unwrap();
        assert_eq!(parsed["auth_mode"], "deferred");
        // Routing/auth-critical keys are retained; the large field is dropped.
        assert_eq!(parsed["entity_type"], "rule_lifecycle");
        assert_eq!(parsed["entity_id"], 42);
        assert_eq!(parsed["trigger_ref"], "core.intervaltimer");
        assert!(parsed.get("trigger_params").is_none());
    }
}
