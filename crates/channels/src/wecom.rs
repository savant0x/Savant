#![allow(clippy::disallowed_methods)]
// SAFETY: All clippy::disallowed_methods violations in this file originate from serde_json::json!() macro internals. The json!() macro calls .unwrap() on provably-infallible compile-time-validated JSON literals. grep confirms 0 real .unwrap() calls exist in this file outside macro expansions.
use async_trait::async_trait;
use savant_core::error::SavantError;
use savant_core::traits::ChannelAdapter;
use savant_core::types::EventFrame;
use std::sync::Arc;
use tracing::{info, warn};

/// WeCom (WeChat Work) channel configuration.
#[derive(Debug, Clone)]
pub struct WeComConfig {
    pub corp_id: String,
    pub corp_secret: String,
    pub agent_id: String,
}

/// WeCom (WeChat Work) enterprise messaging adapter.
pub struct WeComAdapter {
    config: WeComConfig,
    http: reqwest::Client,
    nexus: Arc<savant_core::bus::NexusBridge>,
    access_token: Arc<tokio::sync::Mutex<Option<String>>>,
}

impl WeComAdapter {
    pub fn new(config: WeComConfig, nexus: Arc<savant_core::bus::NexusBridge>) -> Self {
        Self {
            config,
            http: savant_core::net::secure_client(),
            nexus,
            access_token: Arc::new(tokio::sync::Mutex::new(None)),
        }
    }

    /// Gets or refreshes the access_token.
    async fn get_token(&self) -> Result<String, SavantError> {
        {
            let lock = self.access_token.lock().await;
            if let Some(ref token) = *lock {
                return Ok(token.clone());
            }
        }

        let resp: serde_json::Value = self
            .http
            .get(format!(
                "https://qyapi.weixin.qq.com/cgi-bin/gettoken?corpid={}&corpsecret={}",
                self.config.corp_id, self.config.corp_secret
            ))
            .send()
            .await
            .map_err(|e| SavantError::Unknown(format!("WeCom token request failed: {}", e)))?
            .json()
            .await
            .map_err(|e| SavantError::Unknown(format!("WeCom token parse failed: {}", e)))?;

        let token = resp["access_token"]
            .as_str()
            .ok_or_else(|| SavantError::Unknown("No access_token in WeCom response".into()))?
            .to_string();

        let mut lock = self.access_token.lock().await;
        *lock = Some(token.clone());
        Ok(token)
    }

    /// Sends a text message.
    async fn send_text(&self, to_user: &str, text: &str) -> Result<(), SavantError> {
        let token = self.get_token().await?;

        let resp: serde_json::Value = self
            .http
            .post(format!(
                "https://qyapi.weixin.qq.com/cgi-bin/message/send?access_token={}",
                token
            ))
            .json(&serde_json::json!({
                "touser": to_user,
                "msgtype": "text",
                "agentid": self.config.agent_id,
                "text": { "content": text },
            }))
            .send()
            .await
            .map_err(|e| SavantError::Unknown(format!("WeCom send failed: {}", e)))?
            .json()
            .await
            .map_err(|e| SavantError::Unknown(format!("WeCom response parse failed: {}", e)))?;

        if resp["errcode"].as_i64() != Some(0) {
            warn!("[WECOM] Send error: {}", resp);
        }
        Ok(())
    }

    /// Spawns background outbound handler.
    pub fn spawn(self) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            info!("[WECOM] Starting WeCom adapter");

            let (mut event_rx, _) = self.nexus.subscribe().await;
            let adapter = Arc::new(self);

            // Outbound listener
            while let Ok(event) = event_rx.recv().await {
                if event.event_type == "chat.message" {
                    if let Ok(payload) = serde_json::from_str::<serde_json::Value>(&event.payload) {
                        let is_assistant = payload["role"].as_str() == Some("Assistant");
                        let is_for = payload["recipient"]
                            .as_str()
                            .is_some_and(|r| r.starts_with("wecom:"));
                        if is_assistant || is_for {
                            let session_id = payload["session_id"].as_str().unwrap_or("");
                            if let Some(user_id) = session_id.strip_prefix("wecom:") {
                                let text = payload["content"].as_str().unwrap_or("");
                                if let Err(e) = adapter.send_text(user_id, text).await {
                                    warn!("[WECOM] Failed to send: {}", e);
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
impl ChannelAdapter for WeComAdapter {
    fn name(&self) -> &str {
        "wecom"
    }

    async fn send_event(&self, event: EventFrame) -> Result<(), SavantError> {
        if event.event_type == "message.send" {
            if let Ok(payload) = serde_json::from_str::<serde_json::Value>(&event.payload) {
                let user_id = payload["user_id"].as_str().unwrap_or("");
                let text = payload["text"].as_str().unwrap_or("");
                if !user_id.is_empty() && !text.is_empty() {
                    return self.send_text(user_id, text).await;
                }
            }
        }
        Ok(())
    }

    async fn handle_event(&self, event: EventFrame) -> Result<(), SavantError> {
        self.send_event(event).await
    }
}
