//! Message Type Definitions
//!
//! This module defines the core message types and traits for inter-service
//! communication in Attune. All messages follow a standard envelope format
//! with headers and payload.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value as JsonValue;
use uuid::Uuid;

use crate::models::Id;

/// Message trait that all messages must implement
pub trait Message: Serialize + for<'de> Deserialize<'de> + Send + Sync {
    /// Get the message type identifier
    fn message_type(&self) -> MessageType;

    /// Get the routing key for this message
    fn routing_key(&self) -> String {
        self.message_type().routing_key()
    }

    /// Get the exchange name for this message
    fn exchange(&self) -> String {
        self.message_type().exchange()
    }

    /// Serialize message to JSON
    fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }

    /// Deserialize message from JSON
    fn from_json(json: &str) -> Result<Self, serde_json::Error>
    where
        Self: Sized,
    {
        serde_json::from_str(json)
    }
}

/// Message type identifier
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MessageType {
    /// Event created by sensor
    EventCreated,
    /// Enforcement created (rule triggered)
    EnforcementCreated,
    /// Execution requested
    ExecutionRequested,
    /// Execution status changed
    ExecutionStatusChanged,
    /// Execution completed
    ExecutionCompleted,
    /// Inquiry created (human input needed)
    InquiryCreated,
    /// Inquiry responded
    InquiryResponded,
    /// Notification created
    NotificationCreated,
    /// Rule created
    RuleCreated,
    /// Rule enabled
    RuleEnabled,
    /// Rule disabled
    RuleDisabled,
    /// Rule deleted
    RuleDeleted,
    /// Pack registered or installed (triggers runtime environment setup in workers)
    PackRegistered,
    /// Pack deleted (triggers pack file cleanup in workers/sensors)
    PackDeleted,
    /// Action metadata changed (create/update/delete/enable/disable)
    ActionChanged,
    /// Trigger metadata changed (create/update/delete/enable/disable)
    TriggerChanged,
    /// Runtime metadata changed (create/update/delete)
    RuntimeChanged,
    /// Pack metadata changed (create/update/delete/register)
    PackChanged,
    /// Permission set metadata changed (update/role changes)
    PermissionSetChanged,
    /// Identity authorization changed (role/permission assignment changes)
    IdentityAuthorizationChanged,
    /// Execution cancel requested (sent to worker to gracefully stop a running execution)
    ExecutionCancelRequested,
    /// Pack test requested (dispatched to a worker to run a pack's test suite)
    PackTestRequested,
}

impl MessageType {
    /// Get the routing key for this message type
    pub fn routing_key(&self) -> String {
        match self {
            Self::EventCreated => "event.created".to_string(),
            Self::EnforcementCreated => "enforcement.created".to_string(),
            Self::ExecutionRequested => "execution.requested".to_string(),
            Self::ExecutionStatusChanged => "execution.status.changed".to_string(),
            Self::ExecutionCompleted => "execution.completed".to_string(),
            Self::InquiryCreated => "inquiry.created".to_string(),
            Self::InquiryResponded => "inquiry.responded".to_string(),
            Self::NotificationCreated => "notification.created".to_string(),
            Self::RuleCreated => "rule.created".to_string(),
            Self::RuleEnabled => "rule.enabled".to_string(),
            Self::RuleDisabled => "rule.disabled".to_string(),
            Self::RuleDeleted => "rule.deleted".to_string(),
            Self::PackRegistered => "pack.registered".to_string(),
            Self::PackDeleted => "pack.deleted".to_string(),
            Self::ActionChanged => "metadata.action.changed".to_string(),
            Self::TriggerChanged => "metadata.trigger.changed".to_string(),
            Self::RuntimeChanged => "metadata.runtime.changed".to_string(),
            Self::PackChanged => "metadata.pack.changed".to_string(),
            Self::PermissionSetChanged => "metadata.permission_set.changed".to_string(),
            Self::IdentityAuthorizationChanged => {
                "metadata.identity_authorization.changed".to_string()
            }
            Self::ExecutionCancelRequested => "execution.cancel".to_string(),
            Self::PackTestRequested => "pack.test.requested".to_string(),
        }
    }

    /// Get the exchange name for this message type
    pub fn exchange(&self) -> String {
        match self {
            Self::EventCreated => "attune.events".to_string(),
            Self::EnforcementCreated => "attune.executions".to_string(),
            Self::ExecutionRequested | Self::ExecutionStatusChanged | Self::ExecutionCompleted => {
                "attune.executions".to_string()
            }
            Self::InquiryCreated | Self::InquiryResponded => "attune.executions".to_string(),
            Self::NotificationCreated => "attune.notifications".to_string(),
            Self::RuleCreated | Self::RuleEnabled | Self::RuleDisabled | Self::RuleDeleted => {
                "attune.events".to_string()
            }
            Self::PackRegistered | Self::PackDeleted => "attune.events".to_string(),
            Self::ActionChanged => "attune.metadata".to_string(),
            Self::TriggerChanged => "attune.metadata".to_string(),
            Self::RuntimeChanged => "attune.metadata".to_string(),
            Self::PackChanged => "attune.metadata".to_string(),
            Self::PermissionSetChanged => "attune.metadata".to_string(),
            Self::IdentityAuthorizationChanged => "attune.metadata".to_string(),
            Self::ExecutionCancelRequested => "attune.executions".to_string(),
            Self::PackTestRequested => "attune.executions".to_string(),
        }
    }

    /// Get the message type as a string
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::EventCreated => "EventCreated",
            Self::EnforcementCreated => "EnforcementCreated",
            Self::ExecutionRequested => "ExecutionRequested",
            Self::ExecutionStatusChanged => "ExecutionStatusChanged",
            Self::ExecutionCompleted => "ExecutionCompleted",
            Self::InquiryCreated => "InquiryCreated",
            Self::InquiryResponded => "InquiryResponded",
            Self::NotificationCreated => "NotificationCreated",
            Self::RuleCreated => "RuleCreated",
            Self::RuleEnabled => "RuleEnabled",
            Self::RuleDisabled => "RuleDisabled",
            Self::RuleDeleted => "RuleDeleted",
            Self::PackRegistered => "PackRegistered",
            Self::PackDeleted => "PackDeleted",
            Self::ActionChanged => "ActionChanged",
            Self::TriggerChanged => "TriggerChanged",
            Self::RuntimeChanged => "RuntimeChanged",
            Self::PackChanged => "PackChanged",
            Self::PermissionSetChanged => "PermissionSetChanged",
            Self::IdentityAuthorizationChanged => "IdentityAuthorizationChanged",
            Self::ExecutionCancelRequested => "ExecutionCancelRequested",
            Self::PackTestRequested => "PackTestRequested",
        }
    }
}

/// Deserialize a UUID, substituting a freshly-generated one when the value is
/// null or absent. This keeps envelope parsing tolerant of messages that were
/// hand-crafted or produced by older tooling.
fn deserialize_uuid_default<'de, D>(deserializer: D) -> Result<Uuid, D::Error>
where
    D: Deserializer<'de>,
{
    let opt: Option<Uuid> = Option::deserialize(deserializer)?;
    Ok(opt.unwrap_or_else(Uuid::new_v4))
}

/// Message envelope that wraps all messages with metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageEnvelope<T>
where
    T: Clone,
{
    /// Unique message identifier
    #[serde(
        default = "Uuid::new_v4",
        deserialize_with = "deserialize_uuid_default"
    )]
    pub message_id: Uuid,

    /// Correlation ID for tracing related messages
    #[serde(
        default = "Uuid::new_v4",
        deserialize_with = "deserialize_uuid_default"
    )]
    pub correlation_id: Uuid,

    /// Message type
    pub message_type: MessageType,

    /// Message version (for backwards compatibility)
    #[serde(default = "default_version")]
    pub version: String,

    /// Timestamp when message was created
    pub timestamp: DateTime<Utc>,

    /// Message headers
    #[serde(default)]
    pub headers: MessageHeaders,

    /// Message payload
    pub payload: T,
}

impl<T> MessageEnvelope<T>
where
    T: Clone + Serialize + for<'de> Deserialize<'de>,
{
    /// Create a new message envelope
    pub fn new(message_type: MessageType, payload: T) -> Self {
        let message_id = Uuid::new_v4();
        Self {
            message_id,
            correlation_id: message_id, // Default to message_id, can be overridden
            message_type,
            version: "1.0".to_string(),
            timestamp: Utc::now(),
            headers: MessageHeaders::default(),
            payload,
        }
    }

    /// Set correlation ID for message tracing
    pub fn with_correlation_id(mut self, correlation_id: Uuid) -> Self {
        self.correlation_id = correlation_id;
        self
    }

    /// Set source service
    pub fn with_source(mut self, source: impl Into<String>) -> Self {
        self.headers.source_service = Some(source.into());
        self
    }

    /// Set trace ID
    pub fn with_trace_id(mut self, trace_id: Uuid) -> Self {
        self.headers.trace_id = Some(trace_id);
        self
    }

    /// Increment retry count
    pub fn increment_retry(&mut self) {
        self.headers.retry_count += 1;
    }

    /// Serialize to JSON string
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }

    /// Deserialize from JSON string
    pub fn from_json(json: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(json)
    }

    /// Serialize to JSON bytes
    pub fn to_bytes(&self) -> Result<Vec<u8>, serde_json::Error> {
        serde_json::to_vec(self)
    }

    /// Deserialize from JSON bytes
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, serde_json::Error> {
        serde_json::from_slice(bytes)
    }
}

/// Message headers for metadata and tracing
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MessageHeaders {
    /// Number of times this message has been retried
    #[serde(default)]
    pub retry_count: u32,

    /// Source service that generated this message
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_service: Option<String>,

    /// Trace ID for distributed tracing
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trace_id: Option<Uuid>,

    /// Additional custom headers
    #[serde(flatten)]
    pub custom: JsonValue,
}

impl MessageHeaders {
    /// Create new headers
    pub fn new() -> Self {
        Self::default()
    }

    /// Create headers with source service
    pub fn with_source(source: impl Into<String>) -> Self {
        Self {
            source_service: Some(source.into()),
            ..Default::default()
        }
    }
}

fn default_version() -> String {
    "1.0".to_string()
}

// ============================================================================
// Message Payload Definitions
// ============================================================================

/// Payload for EventCreated message
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventCreatedPayload {
    /// Event ID
    pub event_id: Id,
    /// Trigger ID (may be None if trigger was deleted)
    pub trigger_id: Option<Id>,
    /// Trigger reference
    pub trigger_ref: String,
    /// Sensor ID that generated the event (None for system events)
    pub sensor_id: Option<Id>,
    /// Sensor reference (None for system events)
    pub sensor_ref: Option<String>,
    /// Event payload data
    pub payload: JsonValue,
    /// Configuration snapshot
    pub config: Option<JsonValue>,
}

/// Payload for EnforcementCreated message
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EnforcementCreatedPayload {
    /// Enforcement ID
    pub enforcement_id: Id,
    /// Rule ID (may be None if rule was deleted)
    pub rule_id: Option<Id>,
    /// Rule reference
    pub rule_ref: String,
    /// Event ID that triggered this enforcement
    pub event_id: Option<Id>,
    /// Trigger reference
    pub trigger_ref: String,
    /// Event payload for rule evaluation
    pub payload: JsonValue,
}

/// Payload for ExecutionRequested message
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionRequestedPayload {
    /// Execution ID
    pub execution_id: Id,
    /// Action ID (may be None if action was deleted)
    pub action_id: Option<Id>,
    /// Action reference
    pub action_ref: String,
    /// Parent execution ID (for workflows)
    pub parent_id: Option<Id>,
    /// Enforcement ID that created this execution
    pub enforcement_id: Option<Id>,
    /// Execution configuration/parameters
    pub config: Option<JsonValue>,
}

/// Payload for ExecutionStatusChanged message
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionStatusChangedPayload {
    /// Execution ID
    pub execution_id: Id,
    /// Action reference
    pub action_ref: String,
    /// Previous status
    pub previous_status: String,
    /// New status
    pub new_status: String,
    /// Status change timestamp
    pub changed_at: DateTime<Utc>,
}

/// Payload for ExecutionCompleted message
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionCompletedPayload {
    /// Execution ID
    pub execution_id: Id,
    /// Action ID (needed for queue notification)
    pub action_id: Id,
    /// Action reference
    pub action_ref: String,
    /// Execution status (completed, failed, timeout, etc.)
    pub status: String,
    /// Execution result data
    pub result: Option<JsonValue>,
    /// Completion timestamp
    pub completed_at: DateTime<Utc>,
}

/// Payload for InquiryCreated message
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InquiryCreatedPayload {
    /// Inquiry ID
    pub inquiry_id: Id,
    /// Execution ID that created this inquiry
    pub execution_id: Id,
    /// Prompt text for the user
    pub prompt: String,
    /// Response schema (optional)
    pub response_schema: Option<JsonValue>,
    /// User/identity assigned to respond (optional)
    pub assigned_to: Option<Id>,
    /// Timeout timestamp (optional)
    pub timeout_at: Option<DateTime<Utc>>,
}

/// Payload for InquiryResponded message
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InquiryRespondedPayload {
    /// Inquiry ID
    pub inquiry_id: Id,
    /// Execution ID
    pub execution_id: Id,
    /// Response data
    pub response: JsonValue,
    /// User/identity that responded
    pub responded_by: Option<Id>,
    /// Response timestamp
    pub responded_at: DateTime<Utc>,
}

/// Payload for NotificationCreated message
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotificationCreatedPayload {
    /// Notification ID
    pub notification_id: Id,
    /// Notification channel
    pub channel: String,
    /// Entity type (execution, inquiry, etc.)
    pub entity_type: String,
    /// Entity identifier
    pub entity: String,
    /// Activity description
    pub activity: String,
    /// Notification content
    pub content: Option<JsonValue>,
}

/// Payload for RuleCreated message
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuleCreatedPayload {
    /// Rule ID
    pub rule_id: Id,
    /// Rule reference
    pub rule_ref: String,
    /// Trigger ID
    pub trigger_id: Option<Id>,
    /// Trigger reference
    pub trigger_ref: String,
    /// Action ID
    pub action_id: Option<Id>,
    /// Action reference
    pub action_ref: String,
    /// Trigger parameters (from rule.trigger_params)
    pub trigger_params: Option<JsonValue>,
    /// Whether rule is enabled
    pub enabled: bool,
}

/// Payload for RuleEnabled message
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuleEnabledPayload {
    /// Rule ID
    pub rule_id: Id,
    /// Rule reference
    pub rule_ref: String,
    /// Trigger reference
    pub trigger_ref: String,
    /// Trigger parameters (from rule.trigger_params)
    pub trigger_params: Option<JsonValue>,
}

/// Payload for RuleDisabled message
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuleDisabledPayload {
    /// Rule ID
    pub rule_id: Id,
    /// Rule reference
    pub rule_ref: String,
    /// Trigger reference
    pub trigger_ref: String,
}

/// Payload for RuleDeleted message
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuleDeletedPayload {
    /// Rule ID
    pub rule_id: Id,
    /// Rule reference
    pub rule_ref: String,
    /// Trigger ID
    pub trigger_id: Option<Id>,
    /// Trigger reference
    pub trigger_ref: String,
}

/// Payload for PackRegistered message
///
/// Published when a pack is registered or installed so that workers can
/// proactively create runtime environments (virtualenvs, node_modules, etc.)
/// instead of waiting until the first execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackRegisteredPayload {
    /// Pack ID
    pub pack_id: Id,
    /// Pack reference (e.g., "python_example")
    pub pack_ref: String,
    /// Pack version
    pub version: String,
    /// Runtime names that require environment setup (lowercase, e.g., ["python"])
    pub runtime_names: Vec<String>,
}

/// Payload for PackDeleted message
///
/// Published when a pack is deleted so that workers and sensors can
/// remove local pack files and clean up runtime environments.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackDeletedPayload {
    /// Pack ID that was deleted
    pub pack_id: Id,
    /// Pack reference (e.g., "python_example")
    pub pack_ref: String,
}

/// Payload for ActionChanged message
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionChangedPayload {
    /// Action ID
    pub action_id: Id,
    /// Action reference
    pub action_ref: String,
    /// Owning pack reference
    pub pack_ref: String,
    /// Operation type (created, updated, deleted, enabled, disabled)
    pub operation: String,
    /// Last-updated timestamp (or event emission time for delete)
    pub updated_at: DateTime<Utc>,
}

/// Payload for TriggerChanged message
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TriggerChangedPayload {
    /// Trigger ID
    pub trigger_id: Id,
    /// Trigger reference
    pub trigger_ref: String,
    /// Owning pack reference (when present)
    pub pack_ref: Option<String>,
    /// Operation type (created, updated, deleted, enabled, disabled)
    pub operation: String,
    /// Last-updated timestamp (or event emission time for delete)
    pub updated_at: DateTime<Utc>,
}

/// Payload for RuntimeChanged message
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeChangedPayload {
    /// Runtime ID
    pub runtime_id: Id,
    /// Runtime reference
    pub runtime_ref: String,
    /// Owning pack reference (when present)
    pub pack_ref: Option<String>,
    /// Operation type (created, updated, deleted)
    pub operation: String,
    /// Last-updated timestamp (or event emission time for delete)
    pub updated_at: DateTime<Utc>,
}

/// Payload for PackChanged message
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackChangedPayload {
    /// Pack ID
    pub pack_id: Id,
    /// Pack reference
    pub pack_ref: String,
    /// Operation type (created, updated, deleted, registered)
    pub operation: String,
    /// Last-updated timestamp (or event emission time for delete)
    pub updated_at: DateTime<Utc>,
}

/// Payload for PermissionSetChanged message
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionSetChangedPayload {
    /// Permission set ID
    pub permission_set_id: Id,
    /// Permission set reference
    pub permission_set_ref: String,
    /// Operation type (updated, role_assigned, role_unassigned)
    pub operation: String,
    /// Event timestamp
    pub updated_at: DateTime<Utc>,
}

/// Payload for IdentityAuthorizationChanged message
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IdentityAuthorizationChangedPayload {
    /// Identity ID whose authz graph changed
    pub identity_id: Id,
    /// Operation type (role_assigned, role_unassigned, permission_assigned, permission_unassigned)
    pub operation: String,
    /// Event timestamp
    pub updated_at: DateTime<Utc>,
}

/// Payload for ExecutionCancelRequested message
///
/// Sent by the API or executor to the worker that is running a specific
/// execution, instructing it to terminate the process promptly.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionCancelRequestedPayload {
    /// Execution ID to cancel
    pub execution_id: Id,
    /// Worker ID that should handle this cancel (used for routing)
    pub worker_id: Id,
}

/// Payload for PackTestRequested message
///
/// Sent from the API to the executor (`pack.test.requested`) and then from the
/// executor to a specific worker (`pack.test.dispatch.worker.{worker_id}`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackTestRequestedPayload {
    /// Pack install tracking record that owns this test run
    pub pack_install_id: Id,
    /// Pack reference being tested
    pub pack_ref: String,
    /// Pack version being tested
    pub pack_version: String,
    /// What triggered the test ('install', 'update', 'manual', 'validation')
    pub trigger_reason: String,
    /// Runtime names/aliases the worker must support (derived from the pack's
    /// test runner config, e.g. python for unittest/pytest runners)
    #[serde(default)]
    pub required_runtimes: Vec<String>,
    /// Mandatory pack-level placement constraints selected by the executor.
    #[serde(default = "default_json_object")]
    pub worker_selector: serde_json::Value,
    #[serde(default = "default_json_array")]
    pub worker_tolerations: serde_json::Value,
    #[serde(default = "default_json_object")]
    pub worker_affinity: serde_json::Value,
}

fn default_json_object() -> serde_json::Value {
    serde_json::json!({})
}

fn default_json_array() -> serde_json::Value {
    serde_json::json!([])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
    struct TestPayload {
        data: String,
    }

    #[test]
    fn test_message_envelope_creation() {
        let payload = TestPayload {
            data: "test".to_string(),
        };
        let envelope = MessageEnvelope::new(MessageType::EventCreated, payload.clone());

        assert_eq!(envelope.message_type, MessageType::EventCreated);
        assert_eq!(envelope.payload.data, "test");
        assert_eq!(envelope.version, "1.0");
        assert_eq!(envelope.message_id, envelope.correlation_id);
    }

    #[test]
    fn test_message_envelope_with_correlation_id() {
        let payload = TestPayload {
            data: "test".to_string(),
        };
        let correlation_id = Uuid::new_v4();
        let envelope = MessageEnvelope::new(MessageType::EventCreated, payload)
            .with_correlation_id(correlation_id);

        assert_eq!(envelope.correlation_id, correlation_id);
        assert_ne!(envelope.message_id, envelope.correlation_id);
    }

    #[test]
    fn test_message_envelope_serialization() {
        let payload = TestPayload {
            data: "test".to_string(),
        };
        let envelope = MessageEnvelope::new(MessageType::EventCreated, payload);

        let json = envelope.to_json().unwrap();
        assert!(json.contains("EventCreated"));
        assert!(json.contains("test"));

        let deserialized: MessageEnvelope<TestPayload> = MessageEnvelope::from_json(&json).unwrap();
        assert_eq!(deserialized.message_id, envelope.message_id);
        assert_eq!(deserialized.payload.data, "test");
    }

    #[test]
    fn test_message_type_routing_key() {
        assert_eq!(MessageType::EventCreated.routing_key(), "event.created");
        assert_eq!(
            MessageType::ExecutionRequested.routing_key(),
            "execution.requested"
        );
        assert_eq!(
            MessageType::ActionChanged.routing_key(),
            "metadata.action.changed"
        );
        assert_eq!(
            MessageType::TriggerChanged.routing_key(),
            "metadata.trigger.changed"
        );
        assert_eq!(
            MessageType::RuntimeChanged.routing_key(),
            "metadata.runtime.changed"
        );
        assert_eq!(
            MessageType::PackChanged.routing_key(),
            "metadata.pack.changed"
        );
        assert_eq!(
            MessageType::PermissionSetChanged.routing_key(),
            "metadata.permission_set.changed"
        );
        assert_eq!(
            MessageType::IdentityAuthorizationChanged.routing_key(),
            "metadata.identity_authorization.changed"
        );
    }

    #[test]
    fn test_message_type_exchange() {
        assert_eq!(MessageType::EventCreated.exchange(), "attune.events");
        assert_eq!(
            MessageType::ExecutionRequested.exchange(),
            "attune.executions"
        );
        assert_eq!(
            MessageType::NotificationCreated.exchange(),
            "attune.notifications"
        );
        assert_eq!(MessageType::ActionChanged.exchange(), "attune.metadata");
        assert_eq!(MessageType::TriggerChanged.exchange(), "attune.metadata");
        assert_eq!(MessageType::RuntimeChanged.exchange(), "attune.metadata");
        assert_eq!(MessageType::PackChanged.exchange(), "attune.metadata");
        assert_eq!(
            MessageType::PermissionSetChanged.exchange(),
            "attune.metadata"
        );
        assert_eq!(
            MessageType::IdentityAuthorizationChanged.exchange(),
            "attune.metadata"
        );
    }

    #[test]
    fn test_retry_increment() {
        let payload = TestPayload {
            data: "test".to_string(),
        };
        let mut envelope = MessageEnvelope::new(MessageType::EventCreated, payload);

        assert_eq!(envelope.headers.retry_count, 0);
        envelope.increment_retry();
        assert_eq!(envelope.headers.retry_count, 1);
        envelope.increment_retry();
        assert_eq!(envelope.headers.retry_count, 2);
    }

    #[test]
    fn test_message_headers_with_source() {
        let headers = MessageHeaders::with_source("api-service");
        assert_eq!(headers.source_service, Some("api-service".to_string()));
    }

    #[test]
    fn test_envelope_with_source_and_trace() {
        let payload = TestPayload {
            data: "test".to_string(),
        };
        let trace_id = Uuid::new_v4();
        let envelope = MessageEnvelope::new(MessageType::EventCreated, payload)
            .with_source("api-service")
            .with_trace_id(trace_id);

        assert_eq!(
            envelope.headers.source_service,
            Some("api-service".to_string())
        );
        assert_eq!(envelope.headers.trace_id, Some(trace_id));
    }
}
