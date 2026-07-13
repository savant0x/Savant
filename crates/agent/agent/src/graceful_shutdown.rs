//! Graceful Shutdown Tracker — RAII-based in-flight request tracking.
//!
//! Ensures no requests are dropped during agent shutdown. Each in-flight
//! request registers a guard; when the guard drops, the count decrements.
//! Shutdown waits for all guards to drop before proceeding.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use tokio::sync::Notify;

/// Tracks in-flight requests and waits for them to complete before shutdown.
///
/// Use `register()` to create a `ShutdownGuard`. The guard increments the
/// counter on creation and decrements on drop. Call `wait_for_all()` to
/// block until all guards are dropped.
pub struct GracefulShutdownTracker {
    count: Arc<AtomicU32>,
    notify: Arc<Notify>,
}

impl GracefulShutdownTracker {
    /// Creates a new tracker.
    pub fn new() -> Self {
        Self {
            count: Arc::new(AtomicU32::new(0)),
            notify: Arc::new(Notify::new()),
        }
    }

    /// Registers a new in-flight request. Returns a guard that must be
    /// dropped when the request completes.
    pub fn register(&self) -> ShutdownGuard {
        self.count.fetch_add(1, Ordering::SeqCst);
        ShutdownGuard {
            count: Arc::clone(&self.count),
            notify: Arc::clone(&self.notify),
        }
    }

    /// Returns the current number of in-flight requests.
    pub fn in_flight(&self) -> u32 {
        self.count.load(Ordering::SeqCst)
    }

    /// Waits for all in-flight requests to complete.
    /// Returns immediately if no requests are in flight.
    pub async fn wait_for_all(&self) {
        while self.count.load(Ordering::SeqCst) > 0 {
            self.notify.notified().await;
        }
    }

    /// Waits for all in-flight requests with a timeout.
    /// Returns true if all completed, false if timed out.
    pub async fn wait_for_all_timeout(&self, timeout: std::time::Duration) -> bool {
        match tokio::time::timeout(timeout, self.wait_for_all()).await {
            Ok(()) => true,
            Err(_) => false,
        }
    }
}

impl Default for GracefulShutdownTracker {
    fn default() -> Self {
        Self::new()
    }
}

/// RAII guard for in-flight request tracking.
/// Increments counter on creation, decrements on drop.
#[must_use = "ShutdownGuard must be held for the duration of the request. Dropping it signals completion."]
pub struct ShutdownGuard {
    count: Arc<AtomicU32>,
    notify: Arc<Notify>,
}

impl Drop for ShutdownGuard {
    fn drop(&mut self) {
        self.count.fetch_sub(1, Ordering::SeqCst);
        self.notify.notify_waiters();
    }
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_empty_tracker_waits_immediately() {
        let tracker = GracefulShutdownTracker::new();
        // Should return immediately since no requests in flight
        tracker.wait_for_all().await;
        assert_eq!(tracker.in_flight(), 0);
    }

    #[tokio::test]
    async fn test_guard_increments_on_create() {
        let tracker = GracefulShutdownTracker::new();
        let _guard = tracker.register();
        assert_eq!(tracker.in_flight(), 1);
    }

    #[tokio::test]
    async fn test_guard_decrements_on_drop() {
        let tracker = GracefulShutdownTracker::new();
        {
            let _guard = tracker.register();
            assert_eq!(tracker.in_flight(), 1);
        }
        assert_eq!(tracker.in_flight(), 0);
    }

    #[tokio::test]
    async fn test_multiple_guards() {
        let tracker = GracefulShutdownTracker::new();
        let g1 = tracker.register();
        let g2 = tracker.register();
        let g3 = tracker.register();
        assert_eq!(tracker.in_flight(), 3);
        drop(g1);
        assert_eq!(tracker.in_flight(), 2);
        drop(g2);
        assert_eq!(tracker.in_flight(), 1);
        drop(g3);
        assert_eq!(tracker.in_flight(), 0);
    }

    #[tokio::test]
    async fn test_wait_for_all_with_guards() {
        let tracker = Arc::new(GracefulShutdownTracker::new());
        let tracker_clone = Arc::clone(&tracker);

        let guard = tracker.register();
        assert_eq!(tracker.in_flight(), 1);

        // Spawn a task that drops the guard after 50ms
        let handle = tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            drop(guard);
            // Verify count is 0 after drop
            assert_eq!(tracker_clone.in_flight(), 0);
        });

        // wait_for_all should complete after the guard is dropped
        tracker.wait_for_all().await;
        assert_eq!(tracker.in_flight(), 0);
        handle.await.unwrap();
    }

    #[tokio::test]
    async fn test_wait_for_all_timeout_succeeds() {
        let tracker = GracefulShutdownTracker::new();
        let guard = tracker.register();

        // Drop guard after 50ms
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            drop(guard);
        });

        let completed = tracker
            .wait_for_all_timeout(std::time::Duration::from_secs(1))
            .await;
        assert!(completed);
    }

    #[tokio::test]
    async fn test_wait_for_all_timeout_fails() {
        let tracker = GracefulShutdownTracker::new();
        let _guard = tracker.register();

        // Guard is never dropped, so timeout should fire
        let completed = tracker
            .wait_for_all_timeout(std::time::Duration::from_millis(50))
            .await;
        assert!(!completed);
    }
}
