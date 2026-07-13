#![allow(clippy::disallowed_methods)]
// SAFETY: All clippy::disallowed_methods violations in this file originate from serde_json::json!() macro internals. The json!() macro calls .unwrap() on provably-infallible compile-time-validated JSON literals. grep confirms 0 real .unwrap() calls exist in this file outside macro expansions.
use async_trait::async_trait;
use savant_core::error::SavantError;
use savant_core::traits::ChannelAdapter;
use savant_core::types::EventFrame;
use std::sync::Arc;
use tracing::{info, warn};

#[derive(Debug, Clone)]
pub struct LineConfig {
    pub channel_access_token: String,
}

pub struct LineAdapter {
    config: LineConfig,
    http: reqwest::Client,
    nexus: Arc<savant_core::bus::NexusBridge>,
}

impl LineAdapter {
    pub fn new(config: LineConfig, nexus: Arc<savant_core::bus::NexusBridge>) -> Self {
        Self {
            config,
            http: savant_core::net::secure_client(),
            nexus,
        }
    }

    async fn push_text(&self, to: &str, text: &str) -> Result<(), SavantError> {
        let resp = self
            .http
            .post("https://api.line.me/v2/bot/message/push")
            .bearer_auth(&self.config.channel_access_token)
            .json(&serde_json::json!({
                "to": to,
                "messages": [{ "type": "text", "text": text }]
            }))
            .send()
            .await
            .map_err(|e| SavantError::Unknown(e.to_string()))?;
        if !resp.status().is_success() {
            warn!("[LINE] Push failed: {}", resp.status());
        }
        Ok(())
    }

    pub fn spawn(self) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            info!("[LINE] Starting LINE adapter");
            let (mut rx, _) = self.nexus.subscribe().await;
            while let Ok(event) = rx.recv().await {
                if event.event_type == "chat.message" {
                    if let Ok(p) = serde_json::from_str::<serde_json::Value>(&event.payload) {
                        if p["recipient"]
                            .as_str()
                            .is_some_and(|r| r.starts_with("line:"))
                            || p["role"].as_str() == Some("Assistant")
                        {
                            let sid = p["session_id"].as_str().unwrap_or("");
                            if let Some(to) = sid.strip_prefix("line:") {
                                let text = p["content"].as_str().unwrap_or("");
                                if let Err(e) = self.push_text(to, text).await {
                                    warn!("[LINE] {}", e);
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
impl ChannelAdapter for LineAdapter {
    fn name(&self) -> &str {
        "line"
    }
    async fn send_event(&self, _e: EventFrame) -> Result<(), SavantError> {
        Ok(())
    }
    async fn handle_event(&self, _e: EventFrame) -> Result<(), SavantError> {
        Ok(())
    }
}
