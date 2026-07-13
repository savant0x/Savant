#![allow(clippy::disallowed_methods)]
// SAFETY: All clippy::disallowed_methods violations in this file originate from serde_json::json!() macro internals. The json!() macro calls .unwrap() on provably-infallible compile-time-validated JSON literals. grep confirms 0 real .unwrap() calls exist in this file outside macro expansions.
use async_trait::async_trait;
use savant_core::error::SavantError;
use savant_core::traits::ChannelAdapter;
use savant_core::types::EventFrame;
use std::sync::Arc;
use tracing::{info, warn};

#[derive(Debug, Clone)]
pub struct TeamsConfig {
    pub app_id: String,
    pub app_password: String,
}

pub struct TeamsAdapter {
    config: TeamsConfig,
    http: reqwest::Client,
    nexus: Arc<savant_core::bus::NexusBridge>,
    token: Arc<tokio::sync::Mutex<Option<String>>>,
}

impl TeamsAdapter {
    pub fn new(config: TeamsConfig, nexus: Arc<savant_core::bus::NexusBridge>) -> Self {
        Self {
            config,
            http: savant_core::net::secure_client(),
            nexus,
            token: Arc::new(tokio::sync::Mutex::new(None)),
        }
    }

    async fn get_token(&self) -> Result<String, SavantError> {
        {
            let lock = self.token.lock().await;
            if let Some(ref t) = *lock {
                return Ok(t.clone());
            }
        }
        let resp: serde_json::Value = self
            .http
            .post("https://login.microsoftonline.com/botframework.com/oauth2/v2.0/token")
            .form(&[
                ("grant_type", "client_credentials"),
                ("client_id", &self.config.app_id),
                ("client_secret", &self.config.app_password),
                ("scope", "https://api.botframework.com/.default"),
            ])
            .send()
            .await
            .map_err(|e| SavantError::Unknown(e.to_string()))?
            .json()
            .await
            .map_err(|e| SavantError::Unknown(e.to_string()))?;
        let token = resp["access_token"]
            .as_str()
            .ok_or_else(|| SavantError::Unknown("No token".into()))?
            .to_string();
        *self.token.lock().await = Some(token.clone());
        Ok(token)
    }

    async fn send_text(
        &self,
        service_url: &str,
        conversation_id: &str,
        text: &str,
    ) -> Result<(), SavantError> {
        let token = self.get_token().await?;
        let resp = self
            .http
            .post(format!(
                "{}/v3/conversations/{}/activities",
                service_url, conversation_id
            ))
            .bearer_auth(&token)
            .json(&serde_json::json!({"type": "message", "text": text}))
            .send()
            .await
            .map_err(|e| SavantError::Unknown(e.to_string()))?;
        if !resp.status().is_success() {
            warn!("[TEAMS] Send failed: {}", resp.status());
        }
        Ok(())
    }

    pub fn spawn(self) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            info!("[TEAMS] Starting Teams adapter");
            let (mut rx, _) = self.nexus.subscribe().await;
            while let Ok(event) = rx.recv().await {
                if event.event_type == "chat.message" {
                    if let Ok(p) = serde_json::from_str::<serde_json::Value>(&event.payload) {
                        if p["recipient"]
                            .as_str()
                            .is_some_and(|r| r.starts_with("teams:"))
                            || p["role"].as_str() == Some("Assistant")
                        {
                            let sid = p["session_id"].as_str().unwrap_or("");
                            if let Some(rest) = sid.strip_prefix("teams:") {
                                if let Some((svc, conv)) = rest.split_once('/') {
                                    let text = p["content"].as_str().unwrap_or("");
                                    if let Err(e) = self.send_text(svc, conv, text).await {
                                        warn!("[TEAMS] {}", e);
                                    }
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
impl ChannelAdapter for TeamsAdapter {
    fn name(&self) -> &str {
        "teams"
    }

    async fn send_event(&self, event: EventFrame) -> Result<(), SavantError> {
        if event.event_type != "chat.message" {
            return Ok(());
        }
        let payload: serde_json::Value = serde_json::from_str(&event.payload)
            .map_err(|e| SavantError::Unknown(e.to_string()))?;
        let content = payload["content"].as_str().unwrap_or("");
        let session_id = payload["session_id"].as_str().unwrap_or("");

        let rest = session_id
            .strip_prefix("teams:")
            .ok_or_else(|| SavantError::Unknown("No Teams session_id available".to_string()))?;

        let (svc, conv) = rest.split_once('/').ok_or_else(|| {
            SavantError::Unknown(
                "Teams session_id must be format 'teams:service_url/conversation_id'".to_string(),
            )
        })?;

        self.send_text(svc, conv, content).await
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
