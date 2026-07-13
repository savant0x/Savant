#![allow(clippy::disallowed_methods)]
// SAFETY: All clippy::disallowed_methods violations in this file originate from serde_json::json!() macro internals. The json!() macro calls .unwrap() on provably-infallible compile-time-validated JSON literals. grep confirms 0 real .unwrap() calls exist in this file outside macro expansions.
use async_trait::async_trait;
use savant_core::error::SavantError;
use savant_core::traits::ChannelAdapter;
use savant_core::types::{ChatMessage, ChatRole, EventFrame};
use std::sync::Arc;
use std::time::Duration;
use tracing::{info, warn};

#[derive(Debug, Clone)]
pub struct MattermostConfig {
    pub server_url: String,
    pub token: String,
    pub channel_id: Option<String>,
}

pub struct MattermostAdapter {
    config: MattermostConfig,
    http: reqwest::Client,
    nexus: Arc<savant_core::bus::NexusBridge>,
}

impl MattermostAdapter {
    pub fn new(config: MattermostConfig, nexus: Arc<savant_core::bus::NexusBridge>) -> Self {
        Self {
            config,
            http: savant_core::net::secure_client(),
            nexus,
        }
    }

    async fn send_text(&self, channel_id: &str, text: &str) -> Result<(), SavantError> {
        let resp = self
            .http
            .post(format!("{}/api/v4/posts", self.config.server_url))
            .bearer_auth(&self.config.token)
            .json(&serde_json::json!({"channel_id": channel_id, "message": text}))
            .send()
            .await
            .map_err(|e| SavantError::Unknown(e.to_string()))?;
        if !resp.status().is_success() {
            return Err(SavantError::Unknown(format!(
                "Mattermost send failed: HTTP {}",
                resp.status()
            )));
        }
        Ok(())
    }

    async fn poll_posts(&self, channel_id: &str) -> Result<Vec<serde_json::Value>, SavantError> {
        let resp: serde_json::Value = self
            .http
            .get(format!(
                "{}/api/v4/channels/{}/posts",
                self.config.server_url, channel_id
            ))
            .bearer_auth(&self.config.token)
            .query(&[("per_page", "20")])
            .send()
            .await
            .map_err(|e| SavantError::Unknown(e.to_string()))?
            .json()
            .await
            .map_err(|e| SavantError::Unknown(e.to_string()))?;
        let posts = resp["order"].as_array().cloned().unwrap_or_default();
        let mut result = Vec::new();
        for id in posts {
            if let Some(id_str) = id.as_str() {
                if let Some(post) = resp["posts"].get(id_str) {
                    result.push(post.clone());
                }
            }
        }
        Ok(result)
    }

    pub fn spawn(self) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            info!("[MATTERMOST] Starting Mattermost adapter");
            let (mut event_rx, _) = self.nexus.subscribe().await;
            let adapter = Arc::new(self);

            let send_adapter = adapter.clone();
            tokio::spawn(async move {
                while let Ok(event) = event_rx.recv().await {
                    if event.event_type == "chat.message" {
                        if let Ok(p) = serde_json::from_str::<serde_json::Value>(&event.payload) {
                            if p["recipient"]
                                .as_str()
                                .is_some_and(|r| r.starts_with("mattermost:"))
                                || p["role"].as_str() == Some("Assistant")
                            {
                                let sid = p["session_id"].as_str().unwrap_or("");
                                if let Some(ch) = sid.strip_prefix("mattermost:") {
                                    let text = p["content"].as_str().unwrap_or("");
                                    if let Err(e) = send_adapter.send_text(ch, text).await {
                                        warn!("[MATTERMOST] {}", e);
                                    }
                                }
                            }
                        }
                    }
                }
            });

            if let Some(ref ch) = adapter.config.channel_id {
                loop {
                    match adapter.poll_posts(ch).await {
                        Ok(posts) => {
                            for post in &posts {
                                let sender = post["user_id"].as_str().unwrap_or("unknown");
                                let message = post["message"].as_str().unwrap_or("");
                                if !message.is_empty() {
                                    let sid =
                                        savant_core::session::SessionMapper::map("mattermost", ch);
                                    let msg = ChatMessage {
                                        is_telemetry: false,
                                        role: ChatRole::User,
                                        content: message.to_string(),
                                        sender: Some(format!("mattermost:{}", sender)),
                                        recipient: Some("savant".into()),
                                        agent_id: None,
                                        session_id: Some(sid),
                                        channel: savant_core::types::AgentOutputChannel::Chat,
                                        images: Vec::new(),
                                        ..Default::default()
                                    };
                                    let frame = EventFrame {
                                        event_type: "chat.message".into(),
                                        payload: serde_json::to_string(&msg)
                                            .unwrap_or_else(|_| "{}".to_string()),
                                    };
                                    if let Err(e) = adapter.nexus.event_bus.send(frame) {
                                        tracing::warn!("[channels] Event publish failed: {}", e);
                                    }
                                }
                            }
                        }
                        Err(e) => warn!("[MATTERMOST] Poll error: {}", e),
                    }
                    tokio::time::sleep(Duration::from_secs(3)).await;
                }
            } else {
                futures::future::pending::<()>().await;
            }
        })
    }
}

#[async_trait]
impl ChannelAdapter for MattermostAdapter {
    fn name(&self) -> &str {
        "mattermost"
    }

    async fn send_event(&self, event: EventFrame) -> Result<(), SavantError> {
        if event.event_type != "chat.message" {
            return Ok(());
        }
        let payload: serde_json::Value = serde_json::from_str(&event.payload)
            .map_err(|e| SavantError::Unknown(e.to_string()))?;
        let content = payload["content"].as_str().unwrap_or("");
        let session_id = payload["session_id"].as_str().unwrap_or("");

        let channel_id = session_id
            .strip_prefix("mattermost:")
            .or(self.config.channel_id.as_deref())
            .ok_or_else(|| {
                SavantError::Unknown("No Mattermost channel_id available".to_string())
            })?;

        self.send_text(channel_id, content).await
    }

    async fn handle_event(&self, event: EventFrame) -> Result<(), SavantError> {
        if event.event_type != "chat.message" {
            return Ok(());
        }
        // Route inbound event through the nexus event bus
        self.nexus
            .event_bus
            .send(event)
            .map(|_| ())
            .map_err(|e| SavantError::Unknown(format!("Event bus send failed: {}", e)))
    }
}
