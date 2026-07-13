use crate::types::EventFrame;
use moka::sync::Cache;
use std::sync::OnceLock;
use tokio::sync::{broadcast, RwLock};
use tracing::{debug, warn};

/// Global debug log channel for capturing tracing output.
/// The sender is initialized once at startup; the receiver is used by the gateway
/// to forward log messages to WebSocket clients.
static DEBUG_LOG_TX: OnceLock<broadcast::Sender<String>> = OnceLock::new();

/// Returns a reference to the global debug log sender, initializing it on first call.
pub fn debug_log_sender() -> &'static broadcast::Sender<String> {
    DEBUG_LOG_TX.get_or_init(|| {
        let (tx, _) = broadcast::channel(1024);
        tx
    })
}

/// Subscribe to the global debug log channel.
pub fn subscribe_debug_logs() -> broadcast::Receiver<String> {
    debug_log_sender().subscribe()
}

/// Maximum number of entries in the shared memory before eviction.
const MAX_SHARED_MEMORY_ENTRIES: u64 = 10_000;

/// The Nexus Bridge: A shared data bus for the Savant Swarm.
pub struct NexusBridge {
    pub shared_memory: Cache<String, String>,
    pub event_bus: broadcast::Sender<EventFrame>,
    /// Dedicated command bus for user chat messages.
    /// Isolated from the high-volume event_bus (chunks, telemetry) to ensure
    /// user messages are never delayed behind hundreds of streaming chunk events.
    command_bus: broadcast::Sender<EventFrame>,
    /// SwarmSync: High-speed broadcast for causal-ordered state deltas.
    pub swarm_sync: broadcast::Sender<String>,
    /// 🏰 AAA Optimization: Context string cache to prevent O(N) re-joins.
    context_cache: RwLock<Option<String>>,
}

impl NexusBridge {
    pub fn new() -> Self {
        let (event_bus, _) = broadcast::channel(16384);
        let (command_bus, _) = broadcast::channel(1024);
        let (swarm_sync, _) = broadcast::channel(1024);

        let bridge = Self {
            shared_memory: Cache::builder()
                .max_capacity(MAX_SHARED_MEMORY_ENTRIES)
                .build(),
            event_bus,
            command_bus,
            swarm_sync,
            context_cache: RwLock::new(None),
        };

        // Pre-flight-pinning
        bridge.pre_flight_pinning();

        bridge
    }

    /// Attempts to pin the shared memory pages to RAM to prevent swapping/jitter.
    /// This reduces latency jitter by preventing the OS from swapping critical data to disk.
    fn pre_flight_pinning(&self) {
        #[cfg(unix)]
        {
            unsafe {
                // SAFETY: mlockall is safe to call here because:
                // 1. MCL_CURRENT | MCL_FUTURE are valid flags on all Unix platforms
                // 2. The call may fail if RLIMIT_MEMLOCK is exceeded, which we handle gracefully
                // 3. No pointers are passed - the kernel operates on the process address space
                if libc::mlockall(libc::MCL_CURRENT | libc::MCL_FUTURE) == 0 {
                    tracing::info!("NexusBridge: Memory pinning successful.");
                } else {
                    // Log at debug level - failure is non-critical
                    debug!("NexusBridge: Memory pinning failed (check RLIMIT_MEMLOCK). Continuing without pinning.");
                }
            }
        }
        #[cfg(windows)]
        {
            debug!("NexusBridge: Memory pinning relies on OS working set management.");
        }
    }

    /// Updates a key-value pair in the shared memory bus.
    ///
    /// # Arguments
    /// * `key` - State key (max 256 characters)
    /// * `value` - State value (max 1MB)
    pub async fn update_state(&self, key: String, value: String) {
        // Validate key length to prevent unbounded key storage
        if key.len() > 256 {
            warn!(
                "NexusBridge: Rejected state update - key too long ({} bytes, max 256)",
                key.len()
            );
            return;
        }

        // AAA-Perfection: Bounded Memory Enforcement (HS-003)
        // Prevent individual "Bloat-Bombs"
        if value.len() > 1_000_000 {
            warn!(
                "NexusBridge: Rejected large state update for key {} ({} bytes, max 1MB)",
                key,
                value.len()
            );
            return;
        }

        // Invalidate context cache on write
        let mut cache = self.context_cache.write().await;
        *cache = None;

        // moka handles eviction automatically when max_capacity is reached
        self.shared_memory.insert(key, value);
    }

    /// SwarmSync: Broadcast a state delta to all agents.
    pub async fn sync_delta(&self, delta: String) {
        // 🏰 Invalidate cache on sync (since it affects state)
        let mut cache = self.context_cache.write().await;
        *cache = None;
        if let Err(e) = self.swarm_sync.send(delta) {
            warn!("[core::bus] Failed to broadcast swarm sync delta: {:?}", e);
        }
    }

    pub async fn get_global_context(&self) -> String {
        // 🏰 AAA: Cache-First context retrieval
        {
            let cache = self.context_cache.read().await;
            if let Some(ref context) = *cache {
                return context.clone();
            }
        }

        let context = self
            .shared_memory
            .iter()
            .map(|(k, v)| format!("{}: {}", k, v))
            .collect::<Vec<_>>()
            .join("\n");

        // Update cache
        let mut cache = self.context_cache.write().await;
        *cache = Some(context.clone());

        context
    }

    pub async fn publish(
        &self,
        channel: &str,
        message: &str,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let event = EventFrame {
            event_type: channel.to_string(),
            payload: message.to_string(),
        };

        let receiver_count = self.event_bus.receiver_count();
        tracing::trace!(
            "[nexus] PUBLISH topic={} size={} receivers={}",
            channel,
            message.len(),
            receiver_count
        );

        if let Err(e) = self.event_bus.send(event) {
            tracing::error!(
                "[nexus] PUBLISH FAILED topic={} size={} error={:?} receivers={}",
                channel,
                message.len(),
                e,
                receiver_count
            );
            return Err("Failed to publish to event bus".into());
        }

        Ok(())
    }

    /// Publishes a user chat message to the dedicated command bus.
    ///
    /// The command bus is isolated from the high-volume event_bus, ensuring
    /// user messages are never delayed behind hundreds of streaming chunks.
    pub async fn publish_command(
        &self,
        message: &str,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let event = EventFrame {
            event_type: "chat.message".to_string(),
            payload: message.to_string(),
        };

        let receiver_count = self.command_bus.receiver_count();
        tracing::trace!(
            "[nexus] PUBLISH_COMMAND topic=chat.message size={} receivers={}",
            message.len(),
            receiver_count
        );

        if let Err(e) = self.command_bus.send(event) {
            tracing::error!(
                "[nexus] PUBLISH_COMMAND FAILED size={} error={:?} receivers={}",
                message.len(),
                e,
                receiver_count
            );
            return Err("Failed to publish to command bus".into());
        }

        Ok(())
    }

    /// Subscribe to the main event bus and swarm sync.
    pub async fn subscribe(
        &self,
    ) -> (broadcast::Receiver<EventFrame>, broadcast::Receiver<String>) {
        (self.event_bus.subscribe(), self.swarm_sync.subscribe())
    }

    /// Subscribe to the dedicated command bus for user chat messages.
    pub async fn subscribe_commands(&self) -> broadcast::Receiver<EventFrame> {
        self.command_bus.subscribe()
    }

    /// Number of active command bus receivers (heartbeat subscribers).
    /// When this is 0, user messages published to the command bus will fail.
    pub fn command_bus_receiver_count(&self) -> usize {
        self.command_bus.receiver_count()
    }

    /// Number of active event bus receivers.
    pub fn event_bus_receiver_count(&self) -> usize {
        self.event_bus.receiver_count()
    }
}

impl Default for NexusBridge {
    fn default() -> Self {
        Self::new()
    }
}
#[cfg(test)]
#[allow(clippy::disallowed_methods)]
mod tests {
    use super::*;
    use std::time::Instant;

    #[tokio::test]
    async fn benchmark_global_context_cache() {
        let bridge = NexusBridge::new();

        // Fill shared memory with 1000 keys
        for i in 0..1000 {
            bridge
                .update_state(format!("key_{}", i), "value".to_string())
                .await;
        }

        // First call (cache miss)
        let start = Instant::now();
        let ctx = bridge.get_global_context().await;
        let duration_miss = start.elapsed();
        drop(ctx);

        // Second call (cache hit)
        let start = Instant::now();
        let ctx = bridge.get_global_context().await;
        drop(ctx);
        let duration_hit = start.elapsed();

        tracing::info!(
            "Context Cache: Miss={:?}, Hit={:?}",
            duration_miss,
            duration_hit
        );
        assert!(
            duration_hit < duration_miss,
            "Cache hit ({:?}) must be faster than miss ({:?})",
            duration_hit,
            duration_miss
        );
    }
}
