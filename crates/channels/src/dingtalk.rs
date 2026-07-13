#![allow(clippy::disallowed_methods)]
// SAFETY: All clippy::disallowed_methods violations in this file originate from serde_json::json!() macro internals. The json!() macro calls .unwrap() on provably-infallible compile-time-validated JSON literals. grep confirms 0 real .unwrap() calls exist in this file outside macro expansions.
use async_trait::async_trait;
use savant_core::error::SavantError;
use savant_core::traits::ChannelAdapter;
use savant_core::types::EventFrame;
use std::sync::Arc;
use tracing::{info, warn};

/// DingTalk channel configuration.
#[derive(Debug, Clone)]
pub struct DingTalkConfig {
    pub app_key: String,
    pub app_secret: String,
    pub robot_code: String,
}

/// DingTalk enterprise messaging adapter.
pub struct DingTalkAdapter {
    config: DingTalkConfig,
    http: reqwest::Client,
    nexus: Arc<savant_core::bus::NexusBridge>,
    _access_token: Arc<tokio::sync::Mutex<Option<String>>>,
}

impl DingTalkAdapter {
    pub fn new(config: DingTalkConfig, nexus: Arc<savant_core::bus::NexusBridge>) -> Self {
        Self {
            config,
            http: savant_core::net::secure_client(),
            nexus,
            _access_token: Arc::new(tokio::sync::Mutex::new(None)),
        }
    }

    /// Gets or refreshes the access_token.
    async fn get_token(&self) -> Result<String, SavantError> {
        {
            let lock = self._access_token.lock().await;
            if let Some(ref token) = *lock {
                return Ok(token.clone());
            }
        }

        let resp: serde_json::Value = self
            .http
            .post("https://api.dingtalk.com/v1.0/oauth2/accessToken")
            .json(&serde_json::json!({
                "appKey": self.config.app_key,
                "appSecret": self.config.app_secret,
            }))
            .send()
            .await
            .map_err(|e| SavantError::Unknown(format!("DingTalk token request failed: {}", e)))?
            .json()
            .await
            .map_err(|e| SavantError::Unknown(format!("DingTalk token parse failed: {}", e)))?;

        let token = resp["accessToken"]
            .as_str()
            .ok_or_else(|| SavantError::Unknown("No accessToken in DingTalk response".into()))?
            .to_string();

        let mut lock = self._access_token.lock().await;
        *lock = Some(token.clone());
        Ok(token)
    }

    /// Sends a text message to a group chat.
    async fn send_text(&self, chat_id: &str, text: &str) -> Result<(), SavantError> {
        let token = self.get_token().await?;
        let resp: serde_json::Value = self
            .http
            .post("https://api.dingtalk.com/v1.0/robot/groupHeaders/send")
            .bearer_auth(token)
            .json(&serde_json::json!({
                "robotCode": self.config.robot_code,
                "openConversationId": chat_id,
                "msgKey": "sampleText",
                "msgParam": serde_json::json!({ "content": text }).to_string(),
            }))
            .send()
            .await
            .map_err(|e| SavantError::Unknown(format!("DingTalk send failed: {}", e)))?
            .json()
            .await
            .map_err(|e| SavantError::Unknown(format!("DingTalk response parse failed: {}", e)))?;

        if resp["code"].as_i64() != Some(0) {
            warn!("[DINGTALK] Send error: {}", resp);
        }
        Ok(())
    }

    /// Spawns background inbound listener + outbound handler.
    pub fn spawn(self) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            info!("[DINGTALK] Starting DingTalk adapter");

            let (mut event_rx, _) = self.nexus.subscribe().await;
            let send_self = Arc::new(self);

            // Outbound listener
            let send_adapter = send_self.clone();
            tokio::spawn(async move {
                while let Ok(event) = event_rx.recv().await {
                    if event.event_type == "chat.message" {
                        if let Ok(payload) =
                            serde_json::from_str::<serde_json::Value>(&event.payload)
                        {
                            let is_assistant = payload["role"].as_str() == Some("Assistant");
                            let is_for = payload["recipient"]
                                .as_str()
                                .is_some_and(|r| r.starts_with("dingtalk:"));
                            if is_assistant || is_for {
                                let session_id = payload["session_id"].as_str().unwrap_or("");
                                if let Some(chat_id) = session_id.strip_prefix("dingtalk:") {
                                    let text = payload["content"].as_str().unwrap_or("");
                                    if let Err(e) = send_adapter.send_text(chat_id, text).await {
                                        warn!("[DINGTALK] Failed to send: {}", e);
                                    }
                                }
                            }
                        }
                    }
                }
            });

            // DingTalk uses webhook for inbound — no polling loop needed.
            // Messages arrive via HTTP POST to the configured webhook URL.
            // The webhook handler is registered in the gateway server.
            info!(
                "[DINGTALK] Adapter ready — inbound via webhook at /api/channels/dingtalk/webhook"
            );
            futures::future::pending::<()>().await;
        })
    }
}

#[async_trait]
impl ChannelAdapter for DingTalkAdapter {
    fn name(&self) -> &str {
        "dingtalk"
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
        self.send_event(event).await
    }
}
