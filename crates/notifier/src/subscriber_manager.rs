//! Subscriber management for WebSocket clients

use dashmap::DashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tokio::sync::mpsc;
use tracing::{debug, info};

use crate::service::Notification;

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

    /// Optional user ID associated with this client
    #[allow(dead_code)]
    pub user_id: Option<i64>,

    /// Role names assigned to the connecting identity, captured at connect
    /// time. Used by the filter ACL to grant admin bypass.
    ///
    /// TODO: Roles are not refreshed mid-connection. This is a UX/perf
    /// tradeoff (avoids per-message DB lookups) — clients that gain or lose
    /// admin must reconnect to pick up the change. Mid-connection JWT
    /// expiration enforcement (see `websocket_server.rs`) bounds the staleness
    /// window to at most one access-token lifetime.
    #[allow(dead_code)]
    pub roles: Vec<String>,

    /// JWT `exp` claim (Unix seconds) for the token used at connect. The
    /// per-connection task tears down the connection once `now > token_exp`.
    #[allow(dead_code)]
    pub token_exp: i64,

    /// Channel to send notifications to this client
    pub tx: mpsc::UnboundedSender<Notification>,

    /// Filters that determine which notifications this client receives
    pub filters: Vec<SubscriptionFilter>,
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
        user_id: Option<i64>,
        roles: Vec<String>,
        token_exp: i64,
        tx: mpsc::UnboundedSender<Notification>,
    ) {
        let subscriber = Subscriber {
            client_id: client_id.clone(),
            user_id,
            roles,
            token_exp,
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

    /// Broadcast a notification to all matching subscribers
    pub fn broadcast(&self, notification: Notification) {
        let mut sent_count = 0;
        let mut failed_count = 0;

        // Collect client IDs to remove (if send fails)
        let mut to_remove = Vec::new();

        for entry in self.subscribers.iter() {
            let client_id = entry.key();
            let subscriber = entry.value();

            // Check if this subscriber should receive the notification
            if !subscriber.should_receive(&notification) {
                continue;
            }

            // Try to send the notification
            match subscriber.tx.send(notification.clone()) {
                Ok(_) => {
                    sent_count += 1;
                    debug!("Sent notification to client: {}", client_id);
                }
                Err(_) => {
                    // Channel closed, client disconnected
                    failed_count += 1;
                    to_remove.push(client_id.clone());
                    debug!("Client {} disconnected — removing", client_id);
                }
            }
        }

        // Remove disconnected clients
        for client_id in to_remove {
            self.unregister(&client_id);
        }

        if sent_count > 0 {
            debug!(
                "Broadcast notification: sent={}, failed={}, type={}, entity_type={}, entity_id={}",
                sent_count,
                failed_count,
                notification.notification_type,
                notification.entity_type,
                notification.entity_id,
            );
        }
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
                user_id: subscriber.user_id,
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
        manager.register(client_id.clone(), Some(123), vec![], 0, tx);

        assert_eq!(manager.client_count(), 1);

        manager.unregister(&client_id);

        assert_eq!(manager.client_count(), 0);
    }

    #[test]
    fn test_subscriber_manager_subscribe() {
        let manager = SubscriberManager::new();
        let client_id = manager.generate_client_id();

        let (tx, _rx) = mpsc::unbounded_channel();
        manager.register(client_id.clone(), None, vec![], 0, tx);

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
        let subscriber = Subscriber {
            client_id: "test".to_string(),
            user_id: Some(456),
            roles: vec![],
            token_exp: 0,
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
    fn test_broadcast_to_matching_subscribers() {
        let manager = SubscriberManager::new();

        let client1_id = manager.generate_client_id();
        let (tx1, mut rx1) = mpsc::unbounded_channel();
        manager.register(client1_id.clone(), None, vec![], 0, tx1);
        manager.subscribe(
            &client1_id,
            SubscriptionFilter::EntityType("execution".to_string()),
        );

        let client2_id = manager.generate_client_id();
        let (tx2, mut rx2) = mpsc::unbounded_channel();
        manager.register(client2_id.clone(), None, vec![], 0, tx2);
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

        manager.broadcast(notification.clone());

        // Client 1 should receive the notification
        let received1 = rx1.try_recv();
        assert!(received1.is_ok());
        assert_eq!(received1.unwrap().entity_id, 123);

        // Client 2 should not receive the notification
        let received2 = rx2.try_recv();
        assert!(received2.is_err());
    }
}
