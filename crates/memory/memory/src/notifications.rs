//! Hive-Mind Notification Channel
//!
//! When any agent stores a high-importance memory, ALL agents get notified.
//! Memory is already global (hive-mind model). This is about proactive notification.
//!
//! # Architecture
//! ```text
//! index_memory() detects importance >= 7
//!     ↓
//! broadcast notification to all subscribers
//!     ↓
//! agent context assembly injects notification
//! ```

use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;
use tracing::debug;

/// A notification about a high-importance memory discovery.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryNotification {
    /// Unique notification ID
    pub notification_id: String,
    /// Session that generated this memory
    pub source_session: String,
    /// Reference to the MemoryEntry
    pub memory_id: u64,
    /// Domain tags for filtering
    pub domain_tags: Vec<String>,
    /// Importance score (always >= 7 for notifications)
    pub importance: u8,
    /// Timestamp
    pub timestamp: i64,
    /// Memory content preview (first 200 chars)
    pub content_preview: String,
}

/// Hive-mind notification channel for cross-agent knowledge awareness.
pub struct NotificationChannel {
    sender: broadcast::Sender<MemoryNotification>,
}

impl NotificationChannel {
    /// Creates a new notification channel with the given capacity.
    pub fn new(capacity: usize) -> Self {
        let (sender, _) = broadcast::channel(capacity);
        Self { sender }
    }

    /// Sends a notification to all subscribers.
    pub fn notify(&self, notification: MemoryNotification) {
        match self.sender.send(notification.clone()) {
            Ok(receivers) => {
                debug!(
                    "Notification sent to {} subscribers: {}",
                    receivers, notification.notification_id
                );
            }
            Err(_) => {
                debug!("No subscribers for notification (expected if no agents active)");
            }
        }
    }

    /// Creates a new subscription for receiving notifications.
    pub fn subscribe(&self) -> broadcast::Receiver<MemoryNotification> {
        self.sender.subscribe()
    }

    /// Returns the number of active subscribers.
    pub fn subscriber_count(&self) -> usize {
        self.sender.receiver_count()
    }
}

impl Default for NotificationChannel {
    fn default() -> Self {
        Self::new(64)
    }
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)]
mod tests {
    use super::*;

    #[test]
    fn test_notification_channel_creation() {
        let channel = NotificationChannel::new(10);
        assert_eq!(channel.subscriber_count(), 0);
    }

    #[test]
    fn test_notification_subscribe() {
        let channel = NotificationChannel::new(10);
        let _rx = channel.subscribe();
        assert_eq!(channel.subscriber_count(), 1);
    }

    #[test]
    fn test_notification_send_receive() {
        let channel = NotificationChannel::new(10);
        let mut rx = channel.subscribe();

        let notification = MemoryNotification {
            notification_id: "test-1".to_string(),
            source_session: "session-1".to_string(),
            memory_id: 42,
            domain_tags: vec!["docker".to_string()],
            importance: 8,
            timestamp: 1234567890,
            content_preview: "Discovered a bug".to_string(),
        };

        channel.notify(notification.clone());
        let received = rx.try_recv().unwrap();
        assert_eq!(received.notification_id, "test-1");
        assert_eq!(received.importance, 8);
    }

    #[test]
    fn test_notification_multiple_subscribers() {
        let channel = NotificationChannel::new(10);
        let mut rx1 = channel.subscribe();
        let mut rx2 = channel.subscribe();
        assert_eq!(channel.subscriber_count(), 2);

        let notification = MemoryNotification {
            notification_id: "test-2".to_string(),
            source_session: "session-1".to_string(),
            memory_id: 100,
            domain_tags: vec!["security".to_string()],
            importance: 9,
            timestamp: 1234567890,
            content_preview: "Critical finding".to_string(),
        };

        channel.notify(notification);
        assert!(rx1.try_recv().is_ok());
        assert!(rx2.try_recv().is_ok());
    }

    #[test]
    fn test_notification_no_subscribers() {
        let channel = NotificationChannel::new(10);
        let notification = MemoryNotification {
            notification_id: "test-3".to_string(),
            source_session: "session-1".to_string(),
            memory_id: 200,
            domain_tags: vec![],
            importance: 7,
            timestamp: 1234567890,
            content_preview: "Info".to_string(),
        };

        // Should not panic even with no subscribers
        channel.notify(notification);
    }
}
