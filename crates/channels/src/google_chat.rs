#![allow(clippy::disallowed_methods)]
// SAFETY: All clippy::disallowed_methods violations in this file originate from serde_json::json!() macro internals. The json!() macro calls .unwrap() on provably-infallible compile-time-validated JSON literals. grep confirms 0 real .unwrap() calls exist in this file outside macro expansions.
use async_trait::async_trait;
use savant_core::error::SavantError;
use savant_core::traits::ChannelAdapter;
use savant_core::types::EventFrame;
use std::sync::Arc;
use tracing::{info, warn};

#[derive(Debug, Clone)]
pub struct GoogleChatConfig {
    pub auth_token: String,
}

pub struct GoogleChatAdapter {
    config: GoogleChatConfig,
    http: reqwest::Client,
    nexus: Arc<savant_core::bus::NexusBridge>,
}

impl GoogleChatAdapter {
    pub fn new(config: GoogleChatConfig, nexus: Arc<savant_core::bus::NexusBridge>) -> Self {
        Self {
            config,
            http: savant_core::net::secure_client(),
            nexus,
        }
    }

    async fn send_text(&self, space_id: &str, text: &str) -> Result<(), SavantError> {
        let resp = self
            .http
            .post(format!(
                "https://chat.googleapis.com/v1/{}/messages",
                space_id
            ))
            .bearer_auth(&self.config.auth_token)
            .json(&serde_json::json!({ "text": text }))
            .send()
            .await
            .map_err(|e| SavantError::Unknown(e.to_string()))?;
        if !resp.status().is_success() {
            warn!("[GOOGLE_CHAT] Send failed: {}", resp.status());
        }
        Ok(())
    }

    pub fn spawn(self) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            info!("[GOOGLE_CHAT] Starting Google Chat adapter");
            let (mut rx, _) = self.nexus.subscribe().await;
            while let Ok(event) = rx.recv().await {
                if event.event_type == "chat.message" {
                    if let Ok(p) = serde_json::from_str::<serde_json::Value>(&event.payload) {
                        if p["recipient"]
                            .as_str()
                            .is_some_and(|r| r.starts_with("googlechat:"))
                            || p["role"].as_str() == Some("Assistant")
                        {
                            let sid = p["session_id"].as_str().unwrap_or("");
                            if let Some(space) = sid.strip_prefix("googlechat:") {
                                let text = p["content"].as_str().unwrap_or("");
                                if let Err(e) = self.send_text(space, text).await {
                                    warn!("[GOOGLE_CHAT] {}", e);
                                }
                            }
                        }
                    }
                }
            }
        })
    }
}

#[async_trait]
impl ChannelAdapter for GoogleChatAdapter {
    fn name(&self) -> &str {
        "google_chat"
    }

    async fn send_event(&self, event: EventFrame) -> Result<(), SavantError> {
        if event.event_type != "chat.message" {
            return Ok(());
        }
        let payload: serde_json::Value = serde_json::from_str(&event.payload)
            .map_err(|e| SavantError::Unknown(e.to_string()))?;
        let content = payload["content"].as_str().unwrap_or("");
        let session_id = payload["session_id"].as_str().unwrap_or("");

        let space_id = session_id
            .strip_prefix("googlechat:")
            .ok_or_else(|| SavantError::Unknown("No Google Chat space_id available".to_string()))?;

        self.send_text(space_id, content).await
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
