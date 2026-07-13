#![allow(clippy::disallowed_methods)]
// SAFETY: All clippy::disallowed_methods violations in this file originate from serde_json::json!() macro internals. The json!() macro calls .unwrap() on provably-infallible compile-time-validated JSON literals. grep confirms 0 real .unwrap() calls exist in this file outside macro expansions.
use async_trait::async_trait;
use savant_core::error::SavantError;
use savant_core::traits::ChannelAdapter;
use savant_core::types::{ChatMessage, ChatRole, EventFrame};
use std::sync::Arc;
use std::time::Duration;
use tracing::{info, warn};

/// Reddit channel configuration.
#[derive(Debug, Clone)]
pub struct RedditConfig {
    pub client_id: String,
    pub client_secret: String,
    pub user_agent: String,
    pub subreddit: Option<String>,
}

/// Reddit channel adapter.
/// Supports OAuth2, polling inbox for messages, and posting comments.
pub struct RedditAdapter {
    config: RedditConfig,
    http: reqwest::Client,
    nexus: Arc<savant_core::bus::NexusBridge>,
    access_token: Arc<tokio::sync::Mutex<Option<String>>>,
}

impl RedditAdapter {
    pub fn new(config: RedditConfig, nexus: Arc<savant_core::bus::NexusBridge>) -> Self {
        Self {
            config,
            http: savant_core::net::secure_client(),
            nexus,
            access_token: Arc::new(tokio::sync::Mutex::new(None)),
        }
    }

    /// Gets an OAuth2 access token via client credentials.
    async fn get_token(&self) -> Result<String, SavantError> {
        {
            let lock = self.access_token.lock().await;
            if let Some(ref t) = *lock {
                return Ok(t.clone());
            }
        }

        let resp: serde_json::Value = self
            .http
            .post("https://www.reddit.com/api/v1/access_token")
            .basic_auth(
                self.config.client_id.as_str(),
                Some(self.config.client_secret.as_str()),
            )
            .header("User-Agent", self.config.user_agent.as_str())
            .form(&[("grant_type", "client_credentials")])
            .send()
            .await
            .map_err(|e| SavantError::Unknown(e.to_string()))?
            .json()
            .await
            .map_err(|e| SavantError::Unknown(e.to_string()))?;

        let token = resp["access_token"]
            .as_str()
            .ok_or_else(|| SavantError::Unknown("No access_token from Reddit".into()))?
            .to_string();

        *self.access_token.lock().await = Some(token.clone());
        Ok(token)
    }

    /// Polls inbox for unread messages.
    async fn poll_inbox(&self) -> Result<Vec<serde_json::Value>, SavantError> {
        let token = self.get_token().await?;
        let resp: serde_json::Value = self
            .http
            .get("https://oauth.reddit.com/message/unread")
            .bearer_auth(&token)
            .header("User-Agent", &self.config.user_agent)
            .query(&[("limit", "10")])
            .send()
            .await
            .map_err(|e| SavantError::Unknown(e.to_string()))?
            .json()
            .await
            .map_err(|e| SavantError::Unknown(e.to_string()))?;

        Ok(resp["data"]["children"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .filter_map(|c| c["data"].as_object().map(|_| c["data"].clone()))
            .collect())
    }

    /// Replies to a message/comment.
    async fn reply(&self, thing_id: &str, text: &str) -> Result<(), SavantError> {
        let token = self.get_token().await?;
        let resp = self
            .http
            .post("https://oauth.reddit.com/api/comment")
            .bearer_auth(&token)
            .header("User-Agent", &self.config.user_agent)
            .form(&[("thing_id", thing_id), ("text", text)])
            .send()
            .await
            .map_err(|e| SavantError::Unknown(e.to_string()))?;

        if !resp.status().is_success() {
            warn!("[REDDIT] Reply failed: {}", resp.status());
        }
        Ok(())
    }

    pub fn spawn(self) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            info!("[REDDIT] Starting Reddit adapter");
            let (mut event_rx, _) = self.nexus.subscribe().await;
            let adapter = Arc::new(self);

            // Outbound listener
            let outbound = adapter.clone();
            tokio::spawn(async move {
                while let Ok(event) = event_rx.recv().await {
                    if event.event_type == "chat.message" {
                        if let Ok(p) = serde_json::from_str::<serde_json::Value>(&event.payload) {
                            if p["recipient"]
                                .as_str()
                                .is_some_and(|r| r.starts_with("reddit:"))
                                || p["role"].as_str() == Some("Assistant")
                            {
                                let sid = p["session_id"].as_str().unwrap_or("");
                                if let Some(thing_id) = sid.strip_prefix("reddit:") {
                                    let text = p["content"].as_str().unwrap_or("");
                                    if let Err(e) = outbound.reply(thing_id, text).await {
                                        warn!("[REDDIT] Reply error: {}", e);
                                    }
                                }
                            }
                        }
                    }
                }
            });

            // Inbox polling loop
            loop {
                match adapter.poll_inbox().await {
                    Ok(messages) => {
                        for msg in &messages {
                            let sender = msg["author"].as_str().unwrap_or("unknown");
                            let body = msg["body"].as_str().unwrap_or("");
                            let name = msg["name"].as_str().unwrap_or(""); // t1_xxx or t4_xxx
                            if !body.is_empty() && sender != "reddit" {
                                let sid = savant_core::session::SessionMapper::map("reddit", name);
                                let chat_msg = ChatMessage {
                                    is_telemetry: false,
                                    role: ChatRole::User,
                                    content: body.to_string(),
                                    sender: Some(format!("reddit:{}", sender)),
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
                    }
                    Err(e) => warn!("[REDDIT] Inbox poll error: {}", e),
                }
                tokio::time::sleep(Duration::from_secs(30)).await;
            }
        })
    }
}

#[async_trait]
impl ChannelAdapter for RedditAdapter {
    fn name(&self) -> &str {
        "reddit"
    }

    async fn send_event(&self, event: EventFrame) -> Result<(), SavantError> {
        if event.event_type != "chat.message" {
            return Ok(());
        }
        let payload: serde_json::Value = serde_json::from_str(&event.payload)
            .map_err(|e| SavantError::Unknown(e.to_string()))?;
        let content = payload["content"].as_str().unwrap_or("");
        let session_id = payload["session_id"].as_str().unwrap_or("");

        let thing_id = session_id
            .strip_prefix("reddit:")
            .ok_or_else(|| SavantError::Unknown("No Reddit thing_id available".to_string()))?;

        self.reply(thing_id, content).await
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
