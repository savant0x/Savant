#![allow(clippy::disallowed_methods)]
// SAFETY: All clippy::disallowed_methods violations in this file originate from serde_json::json!() macro internals. The json!() macro calls .unwrap() on provably-infallible compile-time-validated JSON literals. grep confirms 0 real .unwrap() calls exist in this file outside macro expansions.
use async_trait::async_trait;
use savant_core::error::SavantError;
use savant_core::traits::ChannelAdapter;
use savant_core::types::{ChatMessage, ChatRole, EventFrame};
use std::sync::Arc;
use std::time::Duration;
use tracing::{info, warn};

/// X (formerly Twitter) channel configuration.
#[derive(Debug, Clone)]
pub struct XConfig {
    pub bearer_token: String,
}

/// X (formerly Twitter) channel adapter.
/// Supports posting tweets and polling DMs.
pub struct XAdapter {
    config: XConfig,
    http: reqwest::Client,
    nexus: Arc<savant_core::bus::NexusBridge>,
}

impl XAdapter {
    pub fn new(config: XConfig, nexus: Arc<savant_core::bus::NexusBridge>) -> Self {
        Self {
            config,
            http: savant_core::net::secure_client(),
            nexus,
        }
    }

    /// Posts a tweet.
    async fn post_tweet(&self, text: &str) -> Result<(), SavantError> {
        let resp = self
            .http
            .post("https://api.x.com/2/tweets")
            .bearer_auth(&self.config.bearer_token)
            .json(&serde_json::json!({"text": text}))
            .send()
            .await
            .map_err(|e| SavantError::Unknown(format!("X post failed: {}", e)))?;

        if !resp.status().is_success() {
            warn!("[X] Post failed: {}", resp.status());
        }
        Ok(())
    }

    /// Fetches recent DM events via Twitter API v2.
    /// GET /2/dm_events — returns all DM events for the authenticated user.
    async fn fetch_dms(&self) -> Result<Vec<serde_json::Value>, SavantError> {
        let resp = self
            .http
            .get("https://api.x.com/2/dm_events")
            .bearer_auth(&self.config.bearer_token)
            .query(&[("dm_event_type", "MessageCreate"), ("max_results", "25")])
            .send()
            .await
            .map_err(|e| SavantError::Unknown(format!("X DM fetch failed: {}", e)))?;

        // Handle rate limiting
        if resp.status() == 429 {
            let reset = resp
                .headers()
                .get("x-rate-limit-reset")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.parse::<u64>().ok())
                .unwrap_or(60);
            let sleep_secs = reset.min(60);
            warn!(
                "[X] Rate limited. Reset in {}s (capped to {}s)",
                reset, sleep_secs
            );
            tokio::time::sleep(Duration::from_secs(sleep_secs)).await;
            return Ok(vec![]);
        }

        let data: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| SavantError::Unknown(format!("X DM parse failed: {}", e)))?;

        Ok(data["data"].as_array().cloned().unwrap_or_default())
    }

    /// Sends a DM to a participant via Twitter API v2.
    /// POST /2/dm_conversations/with/{participant_id}/messages
    async fn send_dm(&self, recipient_id: &str, text: &str) -> Result<(), SavantError> {
        let url = format!(
            "https://api.x.com/2/dm_conversations/with/{}/messages",
            recipient_id
        );
        let resp = self
            .http
            .post(&url)
            .bearer_auth(&self.config.bearer_token)
            .json(&serde_json::json!({ "text": text }))
            .send()
            .await
            .map_err(|e| SavantError::Unknown(format!("X DM send failed: {}", e)))?;

        if resp.status() == 429 {
            let reset = resp
                .headers()
                .get("x-rate-limit-reset")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.parse::<u64>().ok())
                .unwrap_or(60);
            let sleep_secs = reset.min(60);
            warn!(
                "[X] Rate limited on DM send. Reset in {}s (capped to {}s)",
                reset, sleep_secs
            );
            tokio::time::sleep(Duration::from_secs(sleep_secs)).await;
            return Err(SavantError::Unknown("X DM send rate limited".to_string()));
        }

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            warn!("[X] DM send failed: {} — {}", status, body);
            return Err(SavantError::Unknown(format!(
                "X DM send failed: {}",
                status
            )));
        }
        Ok(())
    }

    pub fn spawn(self) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            info!("[X] Starting X adapter");
            let (mut event_rx, _) = self.nexus.subscribe().await;
            let adapter = Arc::new(self);

            // Outbound listener
            let outbound = adapter.clone();
            tokio::spawn(async move {
                while let Ok(event) = event_rx.recv().await {
                    if event.event_type == "chat.message" {
                        if let Ok(p) = serde_json::from_str::<serde_json::Value>(&event.payload) {
                            let is_for =
                                p["recipient"].as_str().is_some_and(|r| r.starts_with("x:"));
                            let is_assistant = p["role"].as_str() == Some("Assistant");
                            if is_assistant || is_for {
                                let content = p["content"].as_str().unwrap_or("");
                                let sid = p["session_id"].as_str().unwrap_or("");
                                if let Some(target) = sid.strip_prefix("x:") {
                                    if target == "post" {
                                        if let Err(e) = outbound.post_tweet(content).await {
                                            tracing::warn!("[channels] HTTP send failed: {}", e);
                                        }
                                    } else {
                                        if let Err(e) = outbound.send_dm(target, content).await {
                                            tracing::warn!("[channels] HTTP send failed: {}", e);
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            });

            // DM polling loop
            loop {
                match adapter.fetch_dms().await {
                    Ok(dms) => {
                        for dm in dms {
                            let sender = dm["sender_id"].as_str().unwrap_or("unknown");
                            let text = dm["text"].as_str().unwrap_or("");
                            if !text.is_empty() {
                                let sid = savant_core::session::SessionMapper::map("x", sender);
                                let msg = ChatMessage {
                                    is_telemetry: false,
                                    role: ChatRole::User,
                                    content: text.to_string(),
                                    sender: Some(format!("x:{}", sender)),
                                    recipient: Some("savant".into()),
                                    agent_id: None,
                                    session_id: Some(sid),
                                    channel: savant_core::types::AgentOutputChannel::Chat,
                                    images: Vec::new(),
                                    ..Default::default()
                                };
                                let frame = EventFrame {
                                    event_type: "chat.message".into(),
                                    payload: serde_json::to_string(&msg).unwrap_or_default(),
                                };
                                if let Err(e) = adapter.nexus.event_bus.send(frame) {
                                    tracing::warn!("[channels] Event publish failed: {}", e);
                                }
                            }
                        }
                    }
                    Err(e) => warn!("[X] DM poll error: {}", e),
                }
                tokio::time::sleep(Duration::from_secs(10)).await;
            }
        })
    }
}

#[async_trait]
impl ChannelAdapter for XAdapter {
    fn name(&self) -> &str {
        "x"
    }
    async fn send_event(&self, event: EventFrame) -> Result<(), SavantError> {
        if event.event_type == "message.send" {
            if let Ok(p) = serde_json::from_str::<serde_json::Value>(&event.payload) {
                let text = p["text"].as_str().unwrap_or("");
                let target = p["target"].as_str().unwrap_or("post");
                if target == "post" {
                    return self.post_tweet(text).await;
                } else {
                    return self.send_dm(target, text).await;
                }
            }
        }
        Ok(())
    }
    async fn handle_event(&self, event: EventFrame) -> Result<(), SavantError> {
        self.send_event(event).await
    }
}
