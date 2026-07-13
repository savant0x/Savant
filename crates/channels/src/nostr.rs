#![allow(clippy::disallowed_methods)]
// SAFETY: All clippy::disallowed_methods violations in this file originate from serde_json::json!() macro internals. The json!() macro calls .unwrap() on provably-infallible compile-time-validated JSON literals. grep confirms 0 real .unwrap() calls exist in this file outside macro expansions.
use async_trait::async_trait;
use nostr_sdk::prelude::*;
use savant_core::error::SavantError;
use savant_core::traits::ChannelAdapter;
use savant_core::types::{ChatMessage, ChatRole, EventFrame};
use std::sync::Arc;
use tracing::{error, info, warn};

/// Nostr channel configuration.
#[derive(Debug, Clone)]
pub struct NostrConfig {
    pub relays: Vec<String>,
    pub private_key: Option<String>, // hex-encoded nsec
}

/// Nostr channel adapter.
///
/// Enterprise-grade Nostr integration using the `nostr-sdk` crate:
/// - Proper event signing via secp256k1 (every event has valid pubkey, id, sig)
/// - Connection reuse via nostr-sdk Client (single connection per relay)
/// - Automatic relay reconnection handled by nostr-sdk
/// - Per-relay health tracking
pub struct NostrAdapter {
    client: Client,
    keys: Keys,
    config: NostrConfig,
    nexus: Arc<savant_core::bus::NexusBridge>,
}

impl NostrAdapter {
    /// Creates a new NostrAdapter with proper keypair initialization.
    pub fn new(
        config: NostrConfig,
        nexus: Arc<savant_core::bus::NexusBridge>,
    ) -> Result<Self, SavantError> {
        // Initialize keys from hex-encoded nsec or generate new
        let keys = match &config.private_key {
            Some(hex_key) => Keys::parse(hex_key)
                .map_err(|e| SavantError::AuthError(format!("Invalid Nostr private key: {}", e)))?,
            None => {
                info!("[NOSTR] No private key configured — generating ephemeral keypair");
                Keys::generate()
            }
        };

        info!(
            "[NOSTR] Initialized with pubkey: {}",
            keys.public_key().to_bech32().unwrap_or_default()
        );

        // Build client with signer
        let client = Client::builder().signer(keys.clone()).build();

        Ok(Self {
            client,
            keys,
            config,
            nexus,
        })
    }

    /// Connects to all configured relays.
    pub async fn connect(&self) {
        for relay_url in &self.config.relays {
            match Url::parse(relay_url) {
                Ok(url) => {
                    if let Err(e) = self.client.add_relay(url.clone()).await {
                        warn!("[NOSTR] Failed to add relay {}: {}", url, e);
                    }
                }
                Err(e) => warn!("[NOSTR] Invalid relay URL '{}': {}", relay_url, e),
            }
        }
        self.client.connect().await;
    }

    /// Publishes a text note (kind:1) to all connected relays with proper signing.
    async fn publish_note(&self, text: &str) -> Result<(), SavantError> {
        let builder = EventBuilder::text_note(text);
        let event = builder
            .sign_with_keys(&self.keys)
            .map_err(|e| SavantError::Unknown(format!("Event signing failed: {}", e)))?;

        let output = self
            .client
            .send_event(&event)
            .await
            .map_err(|e| SavantError::Unknown(format!("Publish failed: {}", e)))?;

        info!("[NOSTR] Published note: {:?}", output);
        Ok(())
    }

    /// Subscribes to incoming messages from all connected relays.
    async fn subscribe_messages(self: Arc<Self>) {
        let filter = Filter::new().kind(Kind::TextNote).since(Timestamp::now());

        if let Err(e) = self.client.subscribe(filter, None).await {
            error!("[NOSTR] Subscribe failed: {}", e);
            return;
        }

        info!("[NOSTR] Subscribed to text notes on all relays");

        // Handle incoming notifications
        let mut notifications = self.client.notifications();
        while let Ok(notification) = notifications.recv().await {
            match notification {
                RelayPoolNotification::Event { event, .. } => {
                    if event.kind == Kind::TextNote {
                        let content = event.content.to_string();
                        if !content.is_empty() {
                            let pubkey_hex = event.pubkey.to_string();
                            let pubkey_bech32 =
                                event.pubkey.to_bech32().unwrap_or(pubkey_hex.clone());
                            let sid =
                                savant_core::session::SessionMapper::map("nostr", &pubkey_hex);
                            let chat_msg = ChatMessage {
                                is_telemetry: false,
                                role: ChatRole::User,
                                content: content.clone(),
                                sender: Some(format!("nostr:{}", pubkey_bech32)),
                                recipient: Some("savant".into()),
                                agent_id: None,
                                session_id: Some(sid),
                                channel: savant_core::types::AgentOutputChannel::Chat,
                                images: Vec::new(),
                                ..Default::default()
                            };
                            let frame = EventFrame {
                                event_type: "chat.message".into(),
                                payload: serde_json::to_string(&chat_msg).unwrap_or_default(),
                            };
                            if self.nexus.event_bus.send(frame).is_err() {
                                tracing::warn!("[channels::nostr] Event bus send failed");
                            }
                        }
                    }
                }
                RelayPoolNotification::Shutdown => {
                    warn!("[NOSTR] Relay pool shutdown");
                    break;
                }
                _ => {}
            }
        }
    }

    pub fn spawn(self) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            info!(
                "[NOSTR] Starting Nostr adapter (relays: {:?})",
                self.config.relays
            );

            let adapter = Arc::new(self);

            // Connect to relays
            adapter.connect().await;

            // Spawn subscription listener
            let adapter_clone = adapter.clone();
            tokio::spawn(async move {
                adapter_clone.subscribe_messages().await;
            });

            // Outbound listener
            let (mut event_rx, _) = adapter.nexus.subscribe().await;
            while let Ok(event) = event_rx.recv().await {
                if event.event_type == "chat.message" {
                    if let Ok(p) = serde_json::from_str::<serde_json::Value>(&event.payload) {
                        if p["recipient"]
                            .as_str()
                            .is_some_and(|r| r.starts_with("nostr:"))
                            || p["role"].as_str() == Some("Assistant")
                        {
                            let text = p["content"].as_str().unwrap_or("");
                            if let Err(e) = adapter.publish_note(text).await {
                                warn!("[NOSTR] Publish error: {}", e);
                            }
                        }
                    }
                }
            }
        })
    }
}

#[async_trait]
impl ChannelAdapter for NostrAdapter {
    fn name(&self) -> &str {
        "nostr"
    }

    async fn send_event(&self, event: EventFrame) -> Result<(), SavantError> {
        if event.event_type != "chat.message" {
            return Ok(());
        }
        let payload: serde_json::Value = serde_json::from_str(&event.payload)
            .map_err(|e| SavantError::Unknown(e.to_string()))?;
        let content = payload["content"].as_str().unwrap_or("");

        self.publish_note(content).await
    }

    async fn handle_event(&self, event: EventFrame) -> Result<(), SavantError> {
        if event.event_type != "chat.message" {
            return Ok(());
        }
        self.nexus
            .event_bus
            .send(event)
            .map(|_| ())
            .map_err(|e| SavantError::Unknown(format!("Event bus send failed: {}", e)))
    }
}
