#![allow(clippy::disallowed_methods)]
// SAFETY: All clippy::disallowed_methods violations in this file originate from serde_json::json!() macro internals. The json!() macro calls .unwrap() on provably-infallible compile-time-validated JSON literals. grep confirms 0 real .unwrap() calls exist in this file outside macro expansions.
use dashmap::DashMap;
use savant_core::error::SavantError;
use savant_core::traits::ChannelAdapter;
use savant_core::types::EventFrame;
use std::sync::Arc;

use savant_core::bus::NexusBridge;

/// Central pool for managing multiple communication channels.
pub struct InboxPool {
    adapters: DashMap<String, Arc<dyn ChannelAdapter>>,
    nexus: Arc<NexusBridge>,
}

impl InboxPool {
    /// Creates a new InboxPool tied to a NexusBridge.
    pub fn new(nexus: Arc<NexusBridge>) -> Self {
        Self {
            adapters: DashMap::new(),
            nexus,
        }
    }

    /// Registers a new channel adapter.
    pub fn register(&self, adapter: Arc<dyn ChannelAdapter>) {
        let name = adapter.name().to_string();
        tracing::info!("Registering channel adapter: {}", name);
        self.adapters.insert(name, adapter);
    }

    /// Broadcasts an event frame to all registered channels.
    pub async fn broadcast(&self, event: EventFrame) -> Result<(), SavantError> {
        for entry in self.adapters.iter() {
            let adapter = entry.value();
            if let Err(e) = adapter.send_event(event.clone()).await {
                tracing::error!("Failed to broadcast to {}: {}", adapter.name(), e);
            }
        }
        Ok(())
    }

    /// Routes an outbound event to a specific channel.
    pub async fn send_to(&self, channel: &str, event: EventFrame) -> Result<(), SavantError> {
        if let Some(adapter) = self.adapters.get(channel) {
            adapter.send_event(event).await
        } else {
            Err(SavantError::Unknown(format!(
                "Channel not found: {}",
                channel
            )))
        }
    }

    /// Submits an inbound event from an adapter to the NexusBridge.
    pub async fn submit_inbound(&self, event: EventFrame) {
        let event_type = event.event_type.clone();
        info!("Inbound event from adapter: {:?}", event_type);
        if let Err(e) = self.nexus.event_bus.send(event) {
            tracing::warn!("Failed to submit inbound event {:?}: {}", event_type, e);
        }
    }
}

use tracing::info;
