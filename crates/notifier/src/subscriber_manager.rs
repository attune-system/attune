//! Subscriber management for WebSocket clients

use dashmap::DashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{debug, info};

use crate::service::Notification;
use crate::websocket_server::WebSocketAuthContext;

const RULE_LIFECYCLE_NOTIFICATION_TYPE: &str = "rule_lifecycle_changed";

/// Unique identifier for a WebSocket client connection
pub type ClientId = String;

/// Subscription filter for notifications
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum SubscriptionFilter {
    /// Subscribe to all notifications
    All,

    /// Subscribe to notifications for a specific entity type
    EntityType(String),

    /// Subscribe to notifications for a specific entity
    Entity { entity_type: String, entity_id: i64 },

    /// Subscribe to notifications for a specific user
    User(i64),

    /// Subscribe to a specific notification type
    NotificationType(String),

    /// Subscribe to rule-lifecycle notifications for a specific trigger ref
    TriggerRef(String),
}

impl SubscriptionFilter {
    /// Check if this filter matches a notification
    pub fn matches(&self, notification: &Notification) -> bool {
        match self {
            SubscriptionFilter::All => true,
            SubscriptionFilter::EntityType(entity_type) => &notification.entity_type == entity_type,
            SubscriptionFilter::Entity {
                entity_type,
                entity_id,
            } => &notification.entity_type == entity_type && notification.entity_id == *entity_id,
            SubscriptionFilter::User(user_id) => notification.user_id == Some(*user_id),
            SubscriptionFilter::NotificationType(notification_type) => {
                &notification.notification_type == notification_type
            }
            SubscriptionFilter::TriggerRef(trigger_ref) => {
                notification.notification_type == RULE_LIFECYCLE_NOTIFICATION_TYPE
                    && notification
                        .payload
                        .get("trigger_ref")
                        .and_then(|value| value.as_str())
                        .is_some_and(|value| value == trigger_ref)
            }
        }
    }
}

/// A WebSocket client subscriber
pub struct Subscriber {
    /// Unique client identifier
    #[allow(dead_code)]
    pub client_id: ClientId,

    /// Immutable authorization snapshot captured at connect time, shared with
    /// the connection's receive loop. The central broadcast path uses this to
    /// authorize deliveries once per identity (see `auth_fingerprint`).
    pub auth: Arc<WebSocketAuthContext>,

    /// Stable fingerprint of `auth`. Connections whose fingerprints match
    /// always yield the same visibility decision, so the broadcast path
    /// evaluates authorization at most once per distinct fingerprint per
    /// notification.
    pub auth_fingerprint: u64,

    /// Channel to send notifications to this client
    pub tx: mpsc::UnboundedSender<Notification>,

    /// Filters that determine which notifications this client receives
    pub filters: Vec<SubscriptionFilter>,
}

/// A connection selected for delivery of a specific notification.
///
/// Produced by [`SubscriberManager::collect_delivery_candidates`] after the
/// per-subscriber filter precheck, before the (memoized) authorization step.
pub struct DeliveryCandidate {
    pub client_id: ClientId,
    pub tx: mpsc::UnboundedSender<Notification>,
    pub auth: Arc<WebSocketAuthContext>,
    pub auth_fingerprint: u64,
}

impl Subscriber {
    /// Check if this subscriber should receive a notification
    pub fn should_receive(&self, notification: &Notification) -> bool {
        // If no filters, don't receive anything (must explicitly subscribe)
        if self.filters.is_empty() {
            return false;
        }

        // Check if any filter matches
        self.filters
            .iter()
            .any(|filter| filter.matches(notification))
    }
}

/// Manages all WebSocket subscribers
pub struct SubscriberManager {
    /// Map of client ID to subscriber
    subscribers: Arc<DashMap<ClientId, Subscriber>>,

    /// Counter for generating unique client IDs
    next_id: AtomicUsize,
}

impl SubscriberManager {
    /// Create a new subscriber manager
    pub fn new() -> Self {
        Self {
            subscribers: Arc::new(DashMap::new()),
            next_id: AtomicUsize::new(1),
        }
    }

    /// Generate a unique client ID
    pub fn generate_client_id(&self) -> ClientId {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        format!("client_{}", id)
    }

    /// Register a new subscriber
    pub fn register(
        &self,
        client_id: ClientId,
        auth: Arc<WebSocketAuthContext>,
        tx: mpsc::UnboundedSender<Notification>,
    ) {
        let auth_fingerprint = auth.fingerprint();
        let subscriber = Subscriber {
            client_id: client_id.clone(),
            auth,
            auth_fingerprint,
            tx,
            filters: vec![],
        };

        self.subscribers.insert(client_id.clone(), subscriber);
        info!("Registered new subscriber: {}", client_id);
    }

    /// Unregister a subscriber
    pub fn unregister(&self, client_id: &ClientId) {
        if self.subscribers.remove(client_id).is_some() {
            info!("Unregistered subscriber: {}", client_id);
        }
    }

    /// Add a subscription filter for a client
    pub fn subscribe(&self, client_id: &ClientId, filter: SubscriptionFilter) -> bool {
        if let Some(mut subscriber) = self.subscribers.get_mut(client_id) {
            if !subscriber.filters.contains(&filter) {
                subscriber.filters.push(filter.clone());
                debug!("Client {} subscribed to {:?}", client_id, filter);
                return true;
            }
        }
        false
    }

    /// Remove a subscription filter for a client
    pub fn unsubscribe(&self, client_id: &ClientId, filter: &SubscriptionFilter) -> bool {
        if let Some(mut subscriber) = self.subscribers.get_mut(client_id) {
            if let Some(pos) = subscriber.filters.iter().position(|f| f == filter) {
                subscriber.filters.remove(pos);
                debug!("Client {} unsubscribed from {:?}", client_id, filter);
                return true;
            }
        }
        false
    }

    /// Collect the connections that pass the per-subscriber filter precheck for
    /// `notification`, along with the data the broadcast path needs to
    /// authorize and deliver.
    ///
    /// This intentionally does **not** authorize: authorization is memoized per
    /// distinct `auth_fingerprint` by `dispatch_notification`, so that an
    /// identity with many open sockets is evaluated only once. Candidates are
    /// snapshotted (senders/auth cloned) so the DashMap shard locks are
    /// released before any `await`.
    pub fn collect_delivery_candidates(
        &self,
        notification: &Notification,
    ) -> Vec<DeliveryCandidate> {
        let mut candidates = Vec::new();
        for entry in self.subscribers.iter() {
            let subscriber = entry.value();
            if !subscriber.should_receive(notification) {
                continue;
            }
            candidates.push(DeliveryCandidate {
                client_id: entry.key().clone(),
                tx: subscriber.tx.clone(),
                auth: subscriber.auth.clone(),
                auth_fingerprint: subscriber.auth_fingerprint,
            });
        }
        candidates
    }

    /// Get the number of connected clients
    pub fn client_count(&self) -> usize {
        self.subscribers.len()
    }

    /// Get the total number of subscriptions across all clients
    pub fn subscription_count(&self) -> usize {
        self.subscribers
            .iter()
            .map(|entry| entry.value().filters.len())
            .sum()
    }

    /// Disconnect all subscribers
    pub async fn disconnect_all(&self) {
        let client_ids: Vec<ClientId> = self
            .subscribers
            .iter()
            .map(|entry| entry.key().clone())
            .collect();

        for client_id in client_ids {
            self.unregister(&client_id);
        }

        info!("Disconnected all subscribers");
    }

    /// Get subscriber information for a client
    #[allow(dead_code)]
    pub fn get_subscriber_info(&self, client_id: &ClientId) -> Option<SubscriberInfo> {
        self.subscribers
            .get(client_id)
            .map(|subscriber| SubscriberInfo {
                client_id: subscriber.client_id.clone(),
                user_id: Some(subscriber.auth.identity_id),
                filter_count: subscriber.filters.len(),
            })
    }
}

impl Default for SubscriberManager {
    fn default() -> Self {
        Self::new()
    }
}

/// Information about a subscriber (for status/debugging)
#[derive(Debug, Clone, serde::Serialize)]
#[allow(dead_code)]
pub struct SubscriberInfo {
    pub client_id: ClientId,
    pub user_id: Option<i64>,
    pub filter_count: usize,
}

#[cfg(test)]
mod tests {
    use super::*;
    use attune_common::auth::TokenType;

    /// Build a shared access-token auth snapshot for the given identity.
    fn test_auth(identity_id: i64) -> Arc<WebSocketAuthContext> {
        Arc::new(WebSocketAuthContext::test_context(
            identity_id,
            TokenType::Access,
            vec![],
        ))
    }

    #[test]
    fn test_subscription_filter_all_matches_everything() {
        let filter = SubscriptionFilter::All;
        let notification = Notification {
            notification_type: "test".to_string(),
            entity_type: "execution".to_string(),
            entity_id: 123,
            user_id: Some(456),
            payload: serde_json::json!({}),
            timestamp: chrono::Utc::now(),
        };

        assert!(filter.matches(&notification));
    }

    #[test]
    fn test_subscription_filter_entity_type() {
        let filter = SubscriptionFilter::EntityType("execution".to_string());

        let notification1 = Notification {
            notification_type: "test".to_string(),
            entity_type: "execution".to_string(),
            entity_id: 123,
            user_id: None,
            payload: serde_json::json!({}),
            timestamp: chrono::Utc::now(),
        };

        let notification2 = Notification {
            notification_type: "test".to_string(),
            entity_type: "inquiry".to_string(),
            entity_id: 456,
            user_id: None,
            payload: serde_json::json!({}),
            timestamp: chrono::Utc::now(),
        };

        assert!(filter.matches(&notification1));
        assert!(!filter.matches(&notification2));
    }

    #[test]
    fn test_subscription_filter_specific_entity() {
        let filter = SubscriptionFilter::Entity {
            entity_type: "execution".to_string(),
            entity_id: 123,
        };

        let notification1 = Notification {
            notification_type: "test".to_string(),
            entity_type: "execution".to_string(),
            entity_id: 123,
            user_id: None,
            payload: serde_json::json!({}),
            timestamp: chrono::Utc::now(),
        };

        let notification2 = Notification {
            notification_type: "test".to_string(),
            entity_type: "execution".to_string(),
            entity_id: 456,
            user_id: None,
            payload: serde_json::json!({}),
            timestamp: chrono::Utc::now(),
        };

        assert!(filter.matches(&notification1));
        assert!(!filter.matches(&notification2));
    }

    #[test]
    fn test_subscription_filter_user() {
        let filter = SubscriptionFilter::User(456);

        let notification1 = Notification {
            notification_type: "test".to_string(),
            entity_type: "execution".to_string(),
            entity_id: 123,
            user_id: Some(456),
            payload: serde_json::json!({}),
            timestamp: chrono::Utc::now(),
        };

        let notification2 = Notification {
            notification_type: "test".to_string(),
            entity_type: "execution".to_string(),
            entity_id: 123,
            user_id: Some(789),
            payload: serde_json::json!({}),
            timestamp: chrono::Utc::now(),
        };

        assert!(filter.matches(&notification1));
        assert!(!filter.matches(&notification2));
    }

    #[test]
    fn test_subscription_filter_trigger_ref_requires_rule_lifecycle_notification_type() {
        let filter = SubscriptionFilter::TriggerRef("core.intervaltimer".to_string());

        let lifecycle_notification = Notification {
            notification_type: "rule_lifecycle_changed".to_string(),
            entity_type: "rule_lifecycle".to_string(),
            entity_id: 1,
            user_id: None,
            payload: serde_json::json!({
                "trigger_ref": "core.intervaltimer",
            }),
            timestamp: chrono::Utc::now(),
        };

        let unrelated_notification = Notification {
            notification_type: "event_created".to_string(),
            entity_type: "event".to_string(),
            entity_id: 2,
            user_id: None,
            payload: serde_json::json!({
                "trigger_ref": "core.intervaltimer",
            }),
            timestamp: chrono::Utc::now(),
        };

        assert!(filter.matches(&lifecycle_notification));
        assert!(!filter.matches(&unrelated_notification));
    }

    #[test]
    fn test_subscriber_manager_register_unregister() {
        let manager = SubscriberManager::new();
        let client_id = manager.generate_client_id();

        assert_eq!(manager.client_count(), 0);

        let (tx, _rx) = mpsc::unbounded_channel();
        manager.register(client_id.clone(), test_auth(123), tx);

        assert_eq!(manager.client_count(), 1);

        manager.unregister(&client_id);

        assert_eq!(manager.client_count(), 0);
    }

    #[test]
    fn test_subscriber_manager_subscribe() {
        let manager = SubscriberManager::new();
        let client_id = manager.generate_client_id();

        let (tx, _rx) = mpsc::unbounded_channel();
        manager.register(client_id.clone(), test_auth(1), tx);

        // Subscribe to all notifications
        let result = manager.subscribe(&client_id, SubscriptionFilter::All);
        assert!(result);

        assert_eq!(manager.subscription_count(), 1);

        // Subscribing to the same filter again should not increase count
        let result = manager.subscribe(&client_id, SubscriptionFilter::All);
        assert!(!result);

        assert_eq!(manager.subscription_count(), 1);
    }

    #[test]
    fn test_subscriber_should_receive() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let auth = test_auth(456);
        let auth_fingerprint = auth.fingerprint();
        let subscriber = Subscriber {
            client_id: "test".to_string(),
            auth,
            auth_fingerprint,
            tx,
            filters: vec![SubscriptionFilter::EntityType("execution".to_string())],
        };

        let notification1 = Notification {
            notification_type: "test".to_string(),
            entity_type: "execution".to_string(),
            entity_id: 123,
            user_id: None,
            payload: serde_json::json!({}),
            timestamp: chrono::Utc::now(),
        };

        let notification2 = Notification {
            notification_type: "test".to_string(),
            entity_type: "inquiry".to_string(),
            entity_id: 456,
            user_id: None,
            payload: serde_json::json!({}),
            timestamp: chrono::Utc::now(),
        };

        assert!(subscriber.should_receive(&notification1));
        assert!(!subscriber.should_receive(&notification2));
    }

    #[test]
    fn test_collect_delivery_candidates_respects_filters() {
        let manager = SubscriberManager::new();

        let client1_id = manager.generate_client_id();
        let (tx1, _rx1) = mpsc::unbounded_channel();
        manager.register(client1_id.clone(), test_auth(1), tx1);
        manager.subscribe(
            &client1_id,
            SubscriptionFilter::EntityType("execution".to_string()),
        );

        let client2_id = manager.generate_client_id();
        let (tx2, _rx2) = mpsc::unbounded_channel();
        manager.register(client2_id.clone(), test_auth(2), tx2);
        manager.subscribe(
            &client2_id,
            SubscriptionFilter::EntityType("inquiry".to_string()),
        );

        let notification = Notification {
            notification_type: "test".to_string(),
            entity_type: "execution".to_string(),
            entity_id: 123,
            user_id: None,
            payload: serde_json::json!({}),
            timestamp: chrono::Utc::now(),
        };

        let candidates = manager.collect_delivery_candidates(&notification);

        // Only client 1 (execution filter) is a candidate.
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].client_id, client1_id);
    }

    #[test]
    fn test_collect_delivery_candidates_shares_fingerprint_per_identity() {
        let manager = SubscriberManager::new();
        let auth = test_auth(42);

        // Two connections (tabs) for the same identity share one auth snapshot.
        let client1_id = manager.generate_client_id();
        let (tx1, _rx1) = mpsc::unbounded_channel();
        manager.register(client1_id.clone(), auth.clone(), tx1);
        manager.subscribe(&client1_id, SubscriptionFilter::All);

        let client2_id = manager.generate_client_id();
        let (tx2, _rx2) = mpsc::unbounded_channel();
        manager.register(client2_id.clone(), auth.clone(), tx2);
        manager.subscribe(&client2_id, SubscriptionFilter::All);

        let notification = Notification {
            notification_type: "execution_status_changed".to_string(),
            entity_type: "execution".to_string(),
            entity_id: 7,
            user_id: None,
            payload: serde_json::json!({}),
            timestamp: chrono::Utc::now(),
        };

        let candidates = manager.collect_delivery_candidates(&notification);
        assert_eq!(candidates.len(), 2);
        // Both connections share a fingerprint, so the broadcast path evaluates
        // authorization only once for this identity.
        assert_eq!(
            candidates[0].auth_fingerprint,
            candidates[1].auth_fingerprint
        );
    }
}
