#![allow(clippy::disallowed_methods)]
// SAFETY: All clippy::disallowed_methods violations in this file originate from serde_json::json!() macro internals. The json!() macro calls .unwrap() on provably-infallible compile-time-validated JSON literals. grep confirms 0 real .unwrap() calls exist in this file outside macro expansions.
use async_trait::async_trait;
use savant_core::error::SavantError;
use savant_core::traits::ChannelAdapter;
use savant_core::types::{ChatMessage, ChatRole, EventFrame};
use std::sync::Arc;
use std::time::Duration;

use tracing::{debug, info, warn};

/// Feishu/Lark channel configuration.
#[derive(Debug, Clone)]
pub struct FeishuConfig {
    pub app_id: String,
    pub app_secret: String,
    pub verification_token: String,
    pub chat_id: String, // Required: the chat/group ID to poll messages from
}

/// Feishu/Lark channel adapter.
/// Communicates via Feishu Open Platform webhook + REST API.
pub struct FeishuAdapter {
    config: FeishuConfig,
    http: reqwest::Client,
    nexus: Arc<savant_core::bus::NexusBridge>,
    tenant_token: Arc<tokio::sync::Mutex<Option<(String, i64)>>>, // (token, expires_at_epoch)
}

impl FeishuAdapter {
    pub fn new(config: FeishuConfig, nexus: Arc<savant_core::bus::NexusBridge>) -> Self {
        Self {
            config,
            http: savant_core::net::secure_client(),
            nexus,
            tenant_token: Arc::new(tokio::sync::Mutex::new(None)),
        }
    }

    /// Gets or refreshes the tenant_access_token with proactive refresh.
    /// Feishu tokens expire after 2 hours — refresh 5 minutes before expiry.
    async fn get_token(&self) -> Result<String, SavantError> {
        {
            let lock = self.tenant_token.lock().await;
            if let Some((ref token, expires_at)) = *lock {
                let now = chrono::Utc::now().timestamp();
                // Proactive refresh: 5 minutes before expiry
                if now < expires_at - 300 {
                    return Ok(token.clone());
                }
            }
        }

        let resp: serde_json::Value = self
            .http
            .post("https://open.feishu.cn/open-apis/auth/v3/tenant_access_token/internal")
            .json(&serde_json::json!({
                "app_id": self.config.app_id,
                "app_secret": self.config.app_secret,
            }))
            .send()
            .await
            .map_err(|e| SavantError::Unknown(format!("Feishu token request failed: {}", e)))?
            .json()
            .await
            .map_err(|e| SavantError::Unknown(format!("Feishu token parse failed: {}", e)))?;

        let token = resp["tenant_access_token"]
            .as_str()
            .ok_or_else(|| {
                SavantError::Unknown("No tenant_access_token in Feishu response".into())
            })?
            .to_string();

        // Feishu tokens expire after 7200 seconds (2 hours)
        let expire_seconds = resp["expire"].as_i64().unwrap_or(7200);
        let expires_at = chrono::Utc::now().timestamp() + expire_seconds;

        let mut lock = self.tenant_token.lock().await;
        *lock = Some((token.clone(), expires_at));

        info!("[FEISHU] Token refreshed, expires in {}s", expire_seconds);
        Ok(token)
    }

    /// Sends a text message to a chat.
    async fn send_text(&self, chat_id: &str, text: &str) -> Result<(), SavantError> {
        let token = self.get_token().await?;

        let resp: serde_json::Value = self
            .http
            .post("https://open.feishu.cn/open-apis/im/v1/messages")
            .query(&[("receive_id_type", "chat_id")])
            .bearer_auth(&token)
            .json(&serde_json::json!({
                "receive_id": chat_id,
                "msg_type": "text",
                "content": serde_json::json!({ "text": text }).to_string(),
            }))
            .send()
            .await
            .map_err(|e| SavantError::Unknown(format!("Feishu send failed: {}", e)))?
            .json()
            .await
            .map_err(|e| SavantError::Unknown(format!("Feishu response parse failed: {}", e)))?;

        if resp["code"].as_i64() != Some(0) {
            let error_msg = resp["msg"].as_str().unwrap_or("Unknown error");
            return Err(SavantError::Unknown(format!(
                "Feishu send error: {} (code: {})",
                error_msg,
                resp["code"].as_i64().unwrap_or(-1)
            )));
        }
        Ok(())
    }

    /// Polls for messages via long-polling endpoint.
    async fn poll_messages(&self) -> Result<Vec<serde_json::Value>, SavantError> {
        let token = self.get_token().await?;

        let resp: serde_json::Value = self
            .http
            .get("https://open.feishu.cn/open-apis/im/v1/messages")
            .bearer_auth(&token)
            .query(&[
                ("container_id_type", "chat"),
                ("container_id", &self.config.chat_id),
                ("page_size", "20"),
            ])
            .send()
            .await
            .map_err(|e| SavantError::Unknown(format!("Feishu poll failed: {}", e)))?
            .json()
            .await
            .map_err(|e| SavantError::Unknown(format!("Feishu poll parse failed: {}", e)))?;

        Ok(resp["data"]["items"]
            .as_array()
            .cloned()
            .unwrap_or_default())
    }

    /// Starts the background polling + outbound loop.
    pub fn spawn(self) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            info!("[FEISHU] Starting Feishu adapter");

            // Subscribe to Nexus for outbound messages
            let (mut event_rx, _) = self.nexus.subscribe().await;
            let nexus_out = self.nexus.clone();
            let http_out = self.http.clone();
            let _config_out = self.config.clone();
            let token_out = self.tenant_token.clone();

            // Outbound listener
            tokio::spawn(async move {
                while let Ok(event) = event_rx.recv().await {
                    if event.event_type == "chat.message" {
                        if let Ok(payload) =
                            serde_json::from_str::<serde_json::Value>(&event.payload)
                        {
                            let is_assistant = payload["role"].as_str() == Some("Assistant");
                            let is_for_feishu = payload["recipient"]
                                .as_str()
                                .is_some_and(|r| r.starts_with("feishu:"));
                            if is_assistant || is_for_feishu {
                                let session_id = payload["session_id"].as_str().unwrap_or("");
                                if let Some(chat_id) = session_id.strip_prefix("feishu:") {
                                    let text = payload["content"].as_str().unwrap_or("");
                                    // Inline send to avoid borrowing issues
                                    let resp = http_out
                                        .post("https://open.feishu.cn/open-apis/im/v1/messages")
                                        .query(&[("receive_id_type", "chat_id")])
                                        .bearer_auth(token_out.lock().await.as_ref().map(|(t, _)| t.as_str()).unwrap_or(""))
                                        .json(&serde_json::json!({
                                            "receive_id": chat_id,
                                            "msg_type": "text",
                                            "content": serde_json::json!({ "text": text }).to_string(),
                                        }))
                                        .send()
                                        .await;
                                    if let Err(e) = resp {
                                        warn!("[FEISHU] Failed to send: {}", e);
                                    }
                                }
                            }
                        }
                    }
                }
            });

            // Inbound polling loop with exponential backoff
            let mut poll_interval = Duration::from_secs(5);
            let max_interval = Duration::from_secs(60);
            loop {
                match self.poll_messages().await {
                    Ok(messages) => {
                        // Reset interval on success
                        poll_interval = Duration::from_secs(5);
                        for msg in messages {
                            let sender = msg["sender"]["sender_id"]["open_id"]
                                .as_str()
                                .unwrap_or("unknown");
                            let chat_id = msg["chat_id"].as_str().unwrap_or("unknown");
                            let content = msg["body"]["content"].as_str().unwrap_or("");

                            if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(content) {
                                let text = parsed["text"].as_str().unwrap_or(content);
                                if !text.is_empty() {
                                    let session_id =
                                        savant_core::session::SessionMapper::map("feishu", chat_id);
                                    let chat_msg = ChatMessage {
                                        is_telemetry: false,
                                        role: ChatRole::User,
                                        content: text.to_string(),
                                        sender: Some(format!("feishu:{}", sender)),
                                        recipient: Some("savant".to_string()),
                                        agent_id: None,
                                        session_id: Some(session_id),
                                        channel: savant_core::types::AgentOutputChannel::Chat,
                                        images: Vec::new(),
                                        ..Default::default()
                                    };
                                    let frame = EventFrame {
                                        event_type: "chat.message".to_string(),
                                        payload: serde_json::to_string(&chat_msg)
                                            .unwrap_or_default(),
                                    };
                                    if let Err(e) = nexus_out.event_bus.send(frame) {
                                        tracing::warn!("[channels] Event publish failed: {}", e);
                                    }
                                }
                            }
                        }
                    }
                    Err(e) => {
                        warn!("[FEISHU] Poll error: {}", e);
                        // Exponential backoff on error: 5s → 10s → 20s → 40s → 60s max
                        poll_interval = (poll_interval * 2).min(max_interval);
                    }
                }
                tokio::time::sleep(poll_interval).await;
            }
        })
    }
}

#[async_trait]
impl ChannelAdapter for FeishuAdapter {
    fn name(&self) -> &str {
        "feishu"
    }

    async fn send_event(&self, event: EventFrame) -> Result<(), SavantError> {
        if event.event_type == "message.send" {
            if let Ok(payload) = serde_json::from_str::<serde_json::Value>(&event.payload) {
                let chat_id = payload["chat_id"].as_str().unwrap_or("");
                let text = payload["text"].as_str().unwrap_or("");
                if !chat_id.is_empty() && !text.is_empty() {
                    return self.send_text(chat_id, text).await;
                }
            }
        }
        Ok(())
    }

    async fn handle_event(&self, event: EventFrame) -> Result<(), SavantError> {
        debug!("[FEISHU] Handling event: {}", event.event_type);
        self.send_event(event).await
    }
}
