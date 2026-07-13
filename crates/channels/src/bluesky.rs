#![allow(clippy::disallowed_methods)]
// SAFETY: All clippy::disallowed_methods violations in this file originate from serde_json::json!() macro internals. The json!() macro calls .unwrap() on provably-infallible compile-time-validated JSON literals. grep confirms 0 real .unwrap() calls exist in this file outside macro expansions.
use async_trait::async_trait;
use savant_core::error::SavantError;
use savant_core::traits::ChannelAdapter;
use savant_core::types::EventFrame;
use std::sync::Arc;
use tracing::{info, warn};

/// Bluesky channel configuration.
#[derive(Debug, Clone)]
pub struct BlueskyConfig {
    pub handle: String,
    pub app_password: String,
}

/// Bluesky channel adapter using AT Protocol.
pub struct BlueskyAdapter {
    config: BlueskyConfig,
    http: reqwest::Client,
    nexus: Arc<savant_core::bus::NexusBridge>,
    session: Arc<tokio::sync::Mutex<Option<BlueskySession>>>,
}

struct BlueskySession {
    access_jwt: String,
    _refresh_jwt: String,
    did: String,
}

impl BlueskyAdapter {
    pub fn new(config: BlueskyConfig, nexus: Arc<savant_core::bus::NexusBridge>) -> Self {
        Self {
            config,
            http: savant_core::net::secure_client(),
            nexus,
            session: Arc::new(tokio::sync::Mutex::new(None)),
        }
    }

    /// Creates a session via AT Protocol.
    async fn login(&self) -> Result<BlueskySession, SavantError> {
        let resp: serde_json::Value = self
            .http
            .post("https://bsky.social/xrpc/com.atproto.server.createSession")
            .json(&serde_json::json!({
                "identifier": self.config.handle,
                "password": self.config.app_password,
            }))
            .send()
            .await
            .map_err(|e| SavantError::Unknown(e.to_string()))?
            .json()
            .await
            .map_err(|e| SavantError::Unknown(e.to_string()))?;

        Ok(BlueskySession {
            access_jwt: resp["accessJwt"]
                .as_str()
                .filter(|s| !s.is_empty())
                .ok_or_else(|| {
                    SavantError::AuthError("Bluesky login: missing or empty accessJwt".into())
                })?
                .to_string(),
            _refresh_jwt: resp["refreshJwt"].as_str().unwrap_or("").to_string(),
            did: resp["did"]
                .as_str()
                .filter(|s| !s.is_empty())
                .ok_or_else(|| {
                    SavantError::AuthError("Bluesky login: missing or empty did".into())
                })?
                .to_string(),
        })
    }

    /// Posts a skeet (Bluesky post).
    async fn post_skeet(&self, text: &str) -> Result<(), SavantError> {
        let session = self.session.lock().await;
        let session = session
            .as_ref()
            .ok_or_else(|| SavantError::Unknown("Not logged in".into()))?;

        let now = chrono::Utc::now().to_rfc3339();
        let resp = self
            .http
            .post("https://bsky.social/xrpc/com.atproto.repo.createRecord")
            .bearer_auth(&session.access_jwt)
            .json(&serde_json::json!({
                "repo": session.did,
                "collection": "app.bsky.feed.post",
                "record": {
                    "text": text,
                    "createdAt": now,
                    "$type": "app.bsky.feed.post",
                }
            }))
            .send()
            .await
            .map_err(|e| SavantError::Unknown(e.to_string()))?;

        if !resp.status().is_success() {
            warn!("[BLUESKY] Post failed: {}", resp.status());
        }
        Ok(())
    }

    pub fn spawn(self) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            info!("[BLUESKY] Starting Bluesky adapter");

            // Login
            match self.login().await {
                Ok(s) => {
                    info!("[BLUESKY] Logged in as {}", s.did);
                    *self.session.lock().await = Some(s);
                }
                Err(e) => {
                    warn!("[BLUESKY] Login failed: {}", e);
                    return;
                }
            }

            let (mut event_rx, _) = self.nexus.subscribe().await;
            while let Ok(event) = event_rx.recv().await {
                if event.event_type == "chat.message" {
                    if let Ok(p) = serde_json::from_str::<serde_json::Value>(&event.payload) {
                        if p["recipient"]
                            .as_str()
                            .is_some_and(|r| r.starts_with("bluesky:"))
                            && p["role"].as_str() == Some("Assistant")
                        {
                            let text = p["content"].as_str().unwrap_or("");
                            if let Err(e) = self.post_skeet(text).await {
                                warn!("[BLUESKY] Post error: {}", e);
                            }
                        }
                    }
                }
            }
        })
    }
}

#[async_trait]
impl ChannelAdapter for BlueskyAdapter {
    fn name(&self) -> &str {
        "bluesky"
    }
    async fn send_event(&self, _e: EventFrame) -> Result<(), SavantError> {
        Ok(())
    }
    async fn handle_event(&self, _e: EventFrame) -> Result<(), SavantError> {
        Ok(())
    }
}
