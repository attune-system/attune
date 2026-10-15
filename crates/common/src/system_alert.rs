use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value as JsonValue};
use sqlx::PgPool;
use tracing::warn;

use crate::{
    models::{event::Event, Id, JsonDict},
    mq::{EventCreatedPayload, MessageEnvelope, MessageType, Publisher},
    repositories::{event::CreateEventInput, event_schema::EventSchemaRepository},
    Result,
};

pub const CORE_ALERT_TRIGGER_REF: &str = "core.alert";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct SystemAlert {
    pub severity: String,
    pub category: String,
    pub failure_type: String,
    pub component_type: String,
    pub component_id: Option<Id>,
    pub component_ref: Option<String>,
    pub worker_role: Option<String>,
    pub observed_at: DateTime<Utc>,
    pub summary: String,
    #[serde(default)]
    pub details: JsonValue,
    pub correlation_id: Option<String>,
}

impl SystemAlert {
    pub fn payload(&self) -> JsonDict {
        json!(self)
    }
}

pub async fn emit_core_alert(
    pool: &PgPool,
    publisher: Option<&Publisher>,
    alert: SystemAlert,
) -> Result<Option<Event>> {
    let event = EventSchemaRepository::create_internal(
        pool,
        CreateEventInput {
            trigger: None,
            trigger_ref: CORE_ALERT_TRIGGER_REF.to_string(),
            config: None,
            payload: Some(alert.payload()),
            trace_tag: None,
            source: None,
            source_ref: Some("attune.system".to_string()),
            rule: None,
            rule_ref: None,
        },
        None,
    )
    .await?;
    let Some(event) = event else {
        warn!("Skipping system alert because its trigger is not registered");
        return Ok(None);
    };

    if let Some(publisher) = publisher {
        let payload = EventCreatedPayload {
            event_id: event.id,
            trigger_id: event.trigger,
            trigger_ref: event.trigger_ref.clone(),
            sensor_id: event.source,
            sensor_ref: event.source_ref.clone(),
            payload: event.payload.clone().unwrap_or_else(|| json!({})),
            config: event.config.clone(),
        };
        let envelope =
            MessageEnvelope::new(MessageType::EventCreated, payload).with_source("system-alert");
        publisher.publish_envelope(&envelope).await?;
    }

    Ok(Some(event))
}
