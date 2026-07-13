//! Executive Monitor — Selection-Broadcast Cycle.
//!
//! Continuous background thread that evaluates competing signals,
//! selects the most salient one, and broadcasts it to all listeners.
//! Uses adaptive tick rate: 100ms when active, exponential backoff during stillness.

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Duration;

use dashmap::DashMap;
use tokio::sync::{broadcast, watch, RwLock};
use tracing::{debug, info};

use super::state::{SignalSource, SignalType, WorkspaceSlot};

/// A broadcast event from the Executive Monitor.
#[derive(Debug, Clone)]
pub struct BroadcastEvent {
    /// The selected signal's ID.
    pub slot_id: String,
    /// The signal type.
    pub signal_type: SignalType,
    /// The signal content.
    pub content: String,
    /// The signal source.
    pub source: SignalSource,
    /// Salience score at time of broadcast.
    pub salience: f32,
}

/// Callback type for broadcast listeners.
pub type BroadcastListener = Arc<dyn Fn(&BroadcastEvent) + Send + Sync>;

/// The Executive Monitor — implements GWT selection-broadcast cycle.
pub struct ExecutiveMonitor {
    /// Competing signals.
    slots: Arc<RwLock<Vec<WorkspaceSlot>>>,
    /// Broadcast history (last 100 entries for novelty computation).
    history: Arc<RwLock<VecDeque<String>>>,
    /// Registered listeners.
    listeners: Arc<DashMap<String, BroadcastListener>>,
    /// Broadcast channel for subscribers.
    broadcast_tx: broadcast::Sender<BroadcastEvent>,
    /// Current delta score (from heartbeat).
    delta_rx: watch::Receiver<f32>,
}

impl ExecutiveMonitor {
    /// Creates a new Executive Monitor.
    pub fn new(delta_rx: watch::Receiver<f32>) -> Self {
        let (broadcast_tx, _) = broadcast::channel(100);
        Self {
            slots: Arc::new(RwLock::new(Vec::new())),
            history: Arc::new(RwLock::new(VecDeque::with_capacity(100))),
            listeners: Arc::new(DashMap::new()),
            broadcast_tx,
            delta_rx,
        }
    }

    /// Subscribes to broadcast events.
    pub fn subscribe(&self) -> broadcast::Receiver<BroadcastEvent> {
        self.broadcast_tx.subscribe()
    }

    /// Registers a named listener.
    pub fn register_listener(&self, name: &str, listener: BroadcastListener) {
        self.listeners.insert(name.to_string(), listener);
    }

    /// Submits a signal to compete for workspace attention.
    pub async fn submit_signal(&self, slot: WorkspaceSlot) {
        let mut slots = self.slots.write().await;
        slots.push(slot);
    }

    /// Returns the current number of competing signals.
    pub async fn signal_count(&self) -> usize {
        self.slots.read().await.len()
    }

    /// Returns the current delta score from the heartbeat.
    /// Higher values indicate more active work.
    pub fn current_delta(&self) -> f32 {
        *self.delta_rx.borrow()
    }

    /// Runs the selection-broadcast loop.
    ///
    /// Adaptive tick rate:
    /// - 100ms when signals are present
    /// - Exponential backoff (200ms → 500ms → 1s → 2s → 5s) during stillness
    /// - Resets to 100ms immediately when new signal arrives
    pub async fn run(self: Arc<Self>) {
        info!("[ExecutiveMonitor] Online (adaptive tick rate)");

        let mut tick_ms: u64 = 100;
        let mut stillness_count: u32 = 0;
        const BACKOFF_STEPS: [u64; 5] = [200, 500, 1000, 2000, 5000];

        loop {
            tokio::time::sleep(Duration::from_millis(tick_ms)).await;

            let mut slots = self.slots.write().await;

            if slots.is_empty() {
                // No signals — increase stillness, backoff tick rate
                stillness_count += 1;
                if stillness_count >= 5 {
                    let backoff_idx = ((stillness_count - 5) as usize).min(BACKOFF_STEPS.len() - 1);
                    tick_ms = BACKOFF_STEPS[backoff_idx];
                }
                continue;
            }

            // Reset tick rate — signals present
            tick_ms = 100;
            stillness_count = 0;

            // Recompute salience for all slots
            let history = self.history.read().await;
            let history_vec: Vec<String> = history.iter().cloned().collect();
            drop(history);

            for slot in slots.iter_mut() {
                slot.recompute_salience(&history_vec, &[]);
            }

            // Select highest salience signal
            slots.sort_by(|a, b| {
                b.salience
                    .partial_cmp(&a.salience)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            let selected = slots.remove(0);
            drop(slots);

            let event = BroadcastEvent {
                slot_id: selected.id.clone(),
                signal_type: selected.signal_type,
                content: selected.content.clone(),
                source: selected.source,
                salience: selected.salience,
            };

            debug!(
                "[ExecutiveMonitor] Broadcasting: {} (salience={:.2}, source={:?})",
                &event.content[..event.content.len().min(60)],
                event.salience,
                event.source,
            );

            // Add to history
            {
                let mut history = self.history.write().await;
                if history.len() >= 100 {
                    history.pop_front();
                }
                history.push_back(selected.content.clone());
            }

            // Notify all listeners
            for listener in self.listeners.iter() {
                (listener.value())(&event);
            }

            // Broadcast via channel
            if let Err(e) = self.broadcast_tx.send(event) {
                tracing::warn!("[broadcast] Failed to send event: {}", e);
            }
        }
    }
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_submit_and_count() {
        let (_, rx) = watch::channel(0.0);
        let monitor = ExecutiveMonitor::new(rx);

        let slot = WorkspaceSlot::new(
            "test-1".to_string(),
            SignalType::External,
            "Test signal".to_string(),
            SignalSource::NexusBus,
            &[],
            &[],
        );

        monitor.submit_signal(slot).await;
        assert_eq!(monitor.signal_count().await, 1);
    }

    #[tokio::test]
    async fn test_listener_registration() {
        let (_, rx) = watch::channel(0.0);
        let monitor = ExecutiveMonitor::new(rx);

        let counter = Arc::new(std::sync::atomic::AtomicU32::new(0));
        let counter_clone = counter.clone();

        monitor.register_listener(
            "test",
            Arc::new(move |_| {
                counter_clone.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            }),
        );

        assert!(monitor.listeners.contains_key("test"));
    }

    #[tokio::test]
    async fn test_subscribe() {
        let (_, rx) = watch::channel(0.0);
        let monitor = ExecutiveMonitor::new(rx);
        let _receiver = monitor.subscribe();
    }
}
