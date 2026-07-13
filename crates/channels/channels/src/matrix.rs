#![allow(clippy::disallowed_methods)]
// SAFETY: All clippy::disallowed_methods violations in this file originate from serde_json::json!() macro internals. The json!() macro calls .unwrap() on provably-infallible compile-time-validated JSON literals. grep confirms 0 real .unwrap() calls exist in this file outside macro expansions.
use async_trait::async_trait;
use savant_core::error::SavantError;
use savant_core::traits::ChannelAdapter;
use savant_core::types::{ChatMessage, ChatRole, EventFrame};
use std::sync::Arc;
use std::time::Duration;
use tracing::{info, warn};

/// Matrix channel configuration.
#[derive(Debug, Clone)]
pub struct MatrixConfig {
    pub homeserver: String, // e.g. "https://matrix.org"
    pub access_token: String,
    pub user_id: String, // e.g. "@bot:matrix.org"
    pub room_id: Option<String>,
}

/// Matrix channel adapter using Client-Server API via reqwest.
pub struct MatrixAdapter {
    config: MatrixConfig,
    http: reqwest::Client,
    nexus: Arc<savant_core::bus::NexusBridge>,
}

impl MatrixAdapter {
    pub fn new(config: MatrixConfig, nexus: Arc<savant_core::bus::NexusBridge>) -> Self {
        Self {
            config,
            http: savant_core::net::secure_client(),
            nexus,
        }
    }

    /// Sends a text message to a room.
    async fn send_text(&self, room_id: &str, text: &str) -> Result<(), SavantError> {
        let txn_id = chrono::Utc::now().timestamp_millis();
        let resp = self
            .http
            .put(format!(
                "{}/_matrix/client/v3/rooms/{}/send/m.room.message/{}",
                self.config.homeserver, room_id, txn_id
            ))
            .bearer_auth(&self.config.access_token)
            .json(&serde_json::json!({
                "msgtype": "m.text",
                "body": text,
            }))
            .send()
            .await
            .map_err(|e| SavantError::Unknown(format!("Matrix send failed: {}", e)))?;

        if !resp.status().is_success() {
            warn!("[MATRIX] Send failed: {}", resp.status());
        }
        Ok(())
    }

    /// Long-polls for new messages via sync API.
    async fn sync_once(&self, since: &str) -> Result<(serde_json::Value, String), SavantError> {
        let mut url = format!("{}/_matrix/client/v3/sync", self.config.homeserver);
        if !since.is_empty() {
            url.push_str(&format!("?since={}&timeout=30000", since));
        } else {
            url.push_str("?timeout=30000");
        }

        let resp: serde_json::Value = self
            .http
            .get(&url)
            .bearer_auth(&self.config.access_token)
            .send()
            .await
            .map_err(|e| SavantError::Unknown(format!("Matrix sync failed: {}", e)))?
            .json()
            .await
            .map_err(|e| SavantError::Unknown(format!("Matrix sync parse failed: {}", e)))?;

        let next_batch = resp["next_batch"].as_str().unwrap_or("").to_string();

        Ok((resp, next_batch))
    }

    /// Extracts text messages from a sync response.
    fn extract_messages(sync_resp: &serde_json::Value) -> Vec<(String, String, String)> {
        let mut messages = Vec::new();

        if let Some(rooms) = sync_resp["rooms"]["join"].as_object() {
            for (room_id, room_data) in rooms {
                if let Some(events) = room_data["timeline"]["events"].as_array() {
                    for event in events {
                        let sender = event["sender"].as_str().unwrap_or("");
                        let msgtype = event["content"]["msgtype"].as_str().unwrap_or("");
                        let body = event["content"]["body"].as_str().unwrap_or("");

                        // Skip our own messages
                        if msgtype == "m.text" && !body.is_empty() {
                            messages.push((room_id.clone(), sender.to_string(), body.to_string()));
                        }
                    }
                }
            }
        }

        messages
    }

    /// Spawns background sync + outbound handler.
    pub fn spawn(self) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            info!("[MATRIX] Starting Matrix adapter");

            let (mut event_rx, _) = self.nexus.subscribe().await;
            let adapter = Arc::new(self);

            // Outbound listener
            let outbound = adapter.clone();
            tokio::spawn(async move {
                while let Ok(event) = event_rx.recv().await {
                    if event.event_type == "chat.message" {
                        if let Ok(p) = serde_json::from_str::<serde_json::Value>(&event.payload) {
                            let is_for = p["recipient"]
                                .as_str()
                                .is_some_and(|r| r.starts_with("matrix:"));
                            let is_assistant = p["role"].as_str() == Some("Assistant");
                            if is_assistant || is_for {
                                let sid = p["session_id"].as_str().unwrap_or("");
                                if let Some(room) = sid.strip_prefix("matrix:") {
                                    let text = p["content"].as_str().unwrap_or("");
                                    if let Err(e) = outbound.send_text(room, text).await {
                                        warn!("[MATRIX] Send error: {}", e);
                                    }
                                }
                            }
                        }
                    }
                }
            });

            // Sync loop
            let mut since = String::new();
            loop {
                match adapter.sync_once(&since).await {
                    Ok((resp, next_batch)) => {
                        since = next_batch;

                        // Skip our own messages using user_id
                        let messages = Self::extract_messages(&resp);
                        for (room_id, sender, body) in messages {
                            if sender == adapter.config.user_id {
                                continue; // Skip own messages
                            }

                            let sid = savant_core::session::SessionMapper::map("matrix", &room_id);
                            let chat_msg = ChatMessage {
                                is_telemetry: false,
                                role: ChatRole::User,
                                content: body,
                                sender: Some(format!("matrix:{}", sender)),
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
                            if let Err(e) = adapter.nexus.event_bus.send(frame) {
                                tracing::warn!("[channels] Event publish failed: {}", e);
                            }
                        }
                    }
                    Err(e) => {
                        warn!("[MATRIX] Sync error: {}", e);
                        tokio::time::sleep(Duration::from_secs(5)).await;
                    }
                }
            }
        })
    }
}

#[async_trait]
impl ChannelAdapter for MatrixAdapter {
    fn name(&self) -> &str {
        "matrix"
    }

    async fn send_event(&self, event: EventFrame) -> Result<(), SavantError> {
        if event.event_type != "chat.message" {
            return Ok(());
        }
        let payload: serde_json::Value = serde_json::from_str(&event.payload)
            .map_err(|e| SavantError::Unknown(e.to_string()))?;
        let content = payload["content"].as_str().unwrap_or("");
        let session_id = payload["session_id"].as_str().unwrap_or("");

        let room_id = session_id
            .strip_prefix("matrix:")
            .or(self.config.room_id.as_deref())
            .ok_or_else(|| SavantError::Unknown("No Matrix room_id available".to_string()))?;

        self.send_text(room_id, content).await
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
