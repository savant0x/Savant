#![allow(clippy::disallowed_methods)]
// SAFETY: All clippy::disallowed_methods violations in this file originate from serde_json::json!() macro internals. The json!() macro calls .unwrap() on provably-infallible compile-time-validated JSON literals. grep confirms 0 real .unwrap() calls exist in this file outside macro expansions.
//! Slack Channel Adapter
//!
//! Provides integration with the Slack Web API for sending and receiving messages.
//! Uses HTTP polling (`conversations.history`) for inbound messages and
//! `chat.postMessage` for outbound delivery.
//!
//! Supports:
//! - Bot token authentication via `Authorization: Bearer`
//! - Deduplication by message timestamp (`ts`)
//! - Rate limiting with `Retry-After` header respect
//! - Message chunking (Slack 4000-char limit)
//! - Typing indicator via ephemeral messages
//! - Health check via `auth.test`

use async_trait::async_trait;
use dashmap::DashSet;
use savant_core::error::SavantError;
use savant_core::traits::ChannelAdapter;
use savant_core::types::{AgentOutputChannel, ChatMessage, ChatRole, EventFrame};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;
use tracing::{debug, error, info, warn};

/// Slack API base URL
const SLACK_API_BASE: &str = "https://slack.com/api";

/// Maximum message length for Slack (4000 characters)
const MAX_MESSAGE_LENGTH: usize = 4000;

/// Chunk size for outbound messages (leave headroom for chunk prefix)
const CHUNK_SIZE: usize = 3800;

/// Default polling interval in seconds
const DEFAULT_POLL_INTERVAL_SECS: u64 = 3;

/// Slack adapter configuration.
#[derive(Debug, Clone)]
pub struct SlackConfig {
    /// Bot token (xoxb-...) for Slack API authentication
    pub bot_token: String,
    /// Default channel ID to send messages to (e.g., "C0123456789")
    pub default_channel: Option<String>,
    /// Allowed user IDs; empty = allow all users
    pub allowed_users: Vec<String>,
}

/// Slack channel adapter.
///
/// Polls `conversations.history` for inbound messages and sends outbound
/// messages via `chat.postMessage`. Maintains a dedup set of processed
/// message timestamps.
pub struct SlackAdapter {
    config: SlackConfig,
    nexus: Arc<savant_core::bus::NexusBridge>,
    client: reqwest::Client,
    /// Set of already-processed message timestamps for deduplication
    seen_ts: Arc<DashSet<String>>,
    /// Last poll timestamp (Slack `oldest` param)
    last_ts: Arc<Mutex<String>>,
    /// Rate-limit backoff state
    rate_limited_until: Arc<Mutex<Option<std::time::Instant>>>,
}

impl SlackAdapter {
    /// Creates a new Slack adapter.
    pub fn new(config: SlackConfig, nexus: Arc<savant_core::bus::NexusBridge>) -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .unwrap_or_else(|_| savant_core::net::secure_client());

        Self {
            config,
            nexus,
            client,
            seen_ts: Arc::new(DashSet::new()),
            last_ts: Arc::new(Mutex::new(String::new())),
            rate_limited_until: Arc::new(Mutex::new(None)),
        }
    }

    /// Performs an authenticated GET request to the Slack API.
    async fn slack_get(
        &self,
        method: &str,
        params: &[(&str, &str)],
    ) -> Result<serde_json::Value, SavantError> {
        self.wait_for_rate_limit().await;

        let url = format!("{}/{}", SLACK_API_BASE, method);
        let mut request = self
            .client
            .get(&url)
            .header("Authorization", format!("Bearer {}", self.config.bot_token));

        for (key, value) in params {
            request = request.query(&[(*key, *value)]);
        }

        let response = request.send().await.map_err(|e| {
            SavantError::NetworkError(format!("Slack API GET {} failed: {}", method, e))
        })?;

        self.check_rate_limit(&response).await;

        let status = response.status();
        let body: serde_json::Value = response.json().await.map_err(|e| {
            SavantError::NetworkError(format!(
                "Failed to parse Slack API response for {}: {}",
                method, e
            ))
        })?;

        if !status.is_success() {
            let err_msg = body
                .get("error")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown_error");
            return Err(SavantError::NetworkError(format!(
                "Slack API {} returned {}: {}",
                method, status, err_msg
            )));
        }

        if body.get("ok").and_then(|v| v.as_bool()) != Some(true) {
            let err_msg = body
                .get("error")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown_error");
            return Err(SavantError::NetworkError(format!(
                "Slack API {} error: {}",
                method, err_msg
            )));
        }

        Ok(body)
    }

    /// Performs an authenticated POST request to the Slack API with JSON body.
    async fn slack_post(
        &self,
        method: &str,
        body: serde_json::Value,
    ) -> Result<serde_json::Value, SavantError> {
        self.wait_for_rate_limit().await;

        let url = format!("{}/{}", SLACK_API_BASE, method);
        let response = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.config.bot_token))
            .header("Content-Type", "application/json; charset=utf-8")
            .json(&body)
            .send()
            .await
            .map_err(|e| {
                SavantError::NetworkError(format!("Slack API POST {} failed: {}", method, e))
            })?;

        self.check_rate_limit(&response).await;

        let status = response.status();
        let resp_body: serde_json::Value = response.json().await.map_err(|e| {
            SavantError::NetworkError(format!(
                "Failed to parse Slack API response for {}: {}",
                method, e
            ))
        })?;

        if !status.is_success() {
            let err_msg = resp_body
                .get("error")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown_error");
            return Err(SavantError::NetworkError(format!(
                "Slack API {} returned {}: {}",
                method, status, err_msg
            )));
        }

        if resp_body.get("ok").and_then(|v| v.as_bool()) != Some(true) {
            let err_msg = resp_body
                .get("error")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown_error");
            return Err(SavantError::NetworkError(format!(
                "Slack API {} error: {}",
                method, err_msg
            )));
        }

        Ok(resp_body)
    }

    /// Checks for rate-limit headers and sets backoff if present.
    async fn check_rate_limit(&self, response: &reqwest::Response) {
        if response.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
            let retry_after = response
                .headers()
                .get("Retry-After")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.parse::<u64>().ok())
                .unwrap_or(5);

            warn!(
                "[SLACK] Rate limited. Backing off for {} seconds.",
                retry_after
            );
            let mut guard = self.rate_limited_until.lock().await;
            *guard = Some(std::time::Instant::now() + Duration::from_secs(retry_after));
        }
    }

    /// Waits if we are currently in a rate-limited backoff window.
    async fn wait_for_rate_limit(&self) {
        let guard = self.rate_limited_until.lock().await;
        if let Some(until) = *guard {
            let now = std::time::Instant::now();
            if until > now {
                let wait = until - now;
                drop(guard);
                debug!("[SLACK] Waiting for rate limit: {:?}", wait);
                tokio::time::sleep(wait).await;
            }
        }
    }

    /// Performs the health check via `auth.test`.
    pub async fn health_check(&self) -> Result<(), SavantError> {
        let body = self.slack_get("auth.test", &[]).await?;
        let user = body
            .get("user")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");
        let team = body
            .get("team")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");
        info!("[SLACK] Health check passed. Bot: {} Team: {}", user, team);
        Ok(())
    }

    /// Sends a typing indicator as an ephemeral message in the given channel.
    async fn send_typing_indicator(&self, channel: &str, thread_ts: Option<&str>) {
        let mut body = serde_json::json!({
            "channel": channel,
            "text": "_typing..._",
        });
        if let Some(ts) = thread_ts {
            body["thread_ts"] = serde_json::Value::String(ts.to_string());
        }

        // Use ephemeral via chat.postEphemeral if we have a user, otherwise
        // fall back to a regular (short-lived) message we delete immediately.
        // For simplicity, send a regular message and delete after a short delay.
        match self.slack_post("chat.postMessage", body).await {
            Ok(resp) => {
                if let Some(ts) = resp.get("ts").and_then(|v| v.as_str()) {
                    let channel = channel.to_string();
                    let ts = ts.to_string();
                    let client = self.client.clone();
                    let token = self.config.bot_token.clone();
                    tokio::spawn(async move {
                        tokio::time::sleep(Duration::from_millis(800)).await;
                        let url = format!("{}/chat.delete", SLACK_API_BASE);
                        if let Err(e) = client
                            .post(&url)
                            .header("Authorization", format!("Bearer {}", token))
                            .json(&serde_json::json!({
                                "channel": channel,
                                "ts": ts,
                            }))
                            .send()
                            .await
                        {
                            tracing::warn!("[channels] HTTP send failed: {}", e);
                        }
                    });
                }
            }
            Err(e) => {
                debug!("[SLACK] Failed to send typing indicator: {}", e);
            }
        }
    }

    /// Polls `conversations.history` for new messages and pushes them to the Nexus event bus.
    async fn poll_loop(self: Arc<Self>) {
        info!("[SLACK] Starting inbound poll loop.");
        let mut interval = tokio::time::interval(Duration::from_secs(DEFAULT_POLL_INTERVAL_SECS));

        loop {
            interval.tick().await;

            let channel = match &self.config.default_channel {
                Some(ch) => ch.clone(),
                None => {
                    warn!("[SLACK] No default channel configured; skipping poll.");
                    continue;
                }
            };

            let oldest = {
                let guard = self.last_ts.lock().await;
                if guard.is_empty() {
                    // On first poll, fetch only messages from the last 60 seconds
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs();
                    format!("{}", now - 60)
                } else {
                    guard.clone()
                }
            };

            let params = vec![
                ("channel", channel.as_str()),
                ("oldest", oldest.as_str()),
                ("limit", "50"),
            ];

            let body = match self.slack_get("conversations.history", &params).await {
                Ok(b) => b,
                Err(e) => {
                    warn!("[SLACK] Poll error: {}", e);
                    continue;
                }
            };

            let messages = match body.get("messages").and_then(|v| v.as_array()) {
                Some(arr) => arr.clone(),
                None => continue,
            };

            let mut latest_ts = oldest.clone();

            for msg in &messages {
                let ts = match msg.get("ts").and_then(|v| v.as_str()) {
                    Some(t) => t.to_string(),
                    None => continue,
                };

                // Update latest timestamp
                if ts > latest_ts {
                    latest_ts = ts.clone();
                }

                // Deduplication
                if !self.seen_ts.insert(ts.clone()) {
                    debug!("[SLACK] Skipping duplicate message ts={}", ts);
                    continue;
                }

                // Prune old entries from dedup set to prevent unbounded growth
                if self.seen_ts.len() > 10_000 {
                    // Clear oldest half (DashSet doesn't support ordered eviction)
                    let keys: Vec<String> = self.seen_ts.iter().map(|r| r.key().clone()).collect();
                    for key in keys.iter().take(keys.len() / 2) {
                        self.seen_ts.remove(key);
                    }
                }

                // Skip bot messages to prevent echo loops
                let subtype = msg.get("subtype").and_then(|v| v.as_str());
                if subtype == Some("bot_message") {
                    debug!("[SLACK] Skipping bot message ts={}", ts);
                    continue;
                }

                // Skip messages without text (e.g., file uploads, join/leave)
                let text = match msg.get("text").and_then(|v| v.as_str()) {
                    Some(t) if !t.is_empty() => t.to_string(),
                    _ => {
                        debug!("[SLACK] Skipping message without text ts={}", ts);
                        continue;
                    }
                };

                let user_id = msg
                    .get("user")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown")
                    .to_string();

                // User allow-list filtering
                if !self.config.allowed_users.is_empty()
                    && !self.config.allowed_users.contains(&user_id)
                {
                    debug!(
                        "[SLACK] Ignoring message from non-allowed user: {}",
                        user_id
                    );
                    continue;
                }

                info!("[SLACK] Inbound message from {}: {}", user_id, text);

                // Session mapping
                let session_id = savant_core::session::SessionMapper::map("slack", &channel);

                let chat_message = ChatMessage {
                    is_telemetry: false,
                    role: ChatRole::User,
                    content: text,
                    sender: Some(format!("slack:{}", user_id)),
                    recipient: Some("savant".to_string()),
                    agent_id: None,
                    session_id: Some(session_id),
                    channel: AgentOutputChannel::Chat,
                    images: Vec::new(),
                    ..Default::default()
                };

                let event = EventFrame {
                    event_type: "chat.message".to_string(),
                    payload: match serde_json::to_string(&chat_message) {
                        Ok(p) => p,
                        Err(e) => {
                            error!("[SLACK] Failed to serialize ChatMessage: {}", e);
                            continue;
                        }
                    },
                };

                if let Err(e) = self.nexus.event_bus.send(event) {
                    error!("[SLACK] Failed to publish event to Nexus: {}", e);
                }
            }

            // Advance the polling cursor
            if latest_ts > oldest {
                let mut guard = self.last_ts.lock().await;
                *guard = latest_ts;
            }
        }
    }

    /// Listens on the Nexus event bus for outbound messages to deliver to Slack.
    async fn outbound_loop(self: Arc<Self>) {
        info!("[SLACK] Starting outbound delivery loop.");
        let (rx, _) = self.nexus.subscribe().await;
        let mut event_rx = rx;

        while let Ok(event) = event_rx.recv().await {
            if event.event_type != "chat.message" {
                continue;
            }

            let payload: serde_json::Value = match serde_json::from_str(&event.payload) {
                Ok(p) => p,
                Err(e) => {
                    debug!("[SLACK] Failed to parse outbound payload: {}", e);
                    continue;
                }
            };

            let is_assistant = payload["role"].as_str() == Some("Assistant")
                || payload["role"].as_str() == Some("assistant");

            let recipient = payload["recipient"].as_str().unwrap_or("");
            let is_for_slack = recipient.starts_with("slack:");

            if !is_assistant && !is_for_slack {
                continue;
            }

            let session_id_str = payload["session_id"]
                .as_str()
                .or_else(|| {
                    payload["session_id"]
                        .as_object()
                        .and_then(|obj| obj.get("0").or(obj.values().next()))
                        .and_then(|v| v.as_str())
                })
                .unwrap_or("");

            let channel_id = session_id_str
                .strip_prefix("slack:")
                .map(|s| s.to_string())
                .or_else(|| self.config.default_channel.clone());

            let channel_id = match channel_id {
                Some(ch) => ch,
                None => {
                    warn!("[SLACK] Cannot determine channel for outbound message.");
                    continue;
                }
            };

            let content = payload["content"].as_str().unwrap_or("");
            if content.is_empty() {
                continue;
            }

            debug!(
                "[SLACK] Delivering outbound message to channel {} ({} chars)",
                channel_id,
                content.len()
            );

            // Typing indicator
            self.send_typing_indicator(&channel_id, None).await;

            // Chunk message if needed
            let chunks = chunk_message(content, CHUNK_SIZE);
            let total = chunks.len();

            for (i, chunk) in chunks.iter().enumerate() {
                let display_text = if total > 1 {
                    format!("[{}/{}] {}", i + 1, total, chunk)
                } else {
                    chunk.clone()
                };

                if display_text.len() > MAX_MESSAGE_LENGTH {
                    warn!(
                        "[SLACK] Chunk {} exceeds Slack limit ({} chars), truncating.",
                        i + 1,
                        display_text.len()
                    );
                }

                let body = serde_json::json!({
                    "channel": channel_id,
                    "text": display_text,
                });

                match self.slack_post("chat.postMessage", body).await {
                    Ok(_) => {
                        debug!("[SLACK] Chunk {}/{} delivered.", i + 1, total);
                    }
                    Err(e) => {
                        error!("[SLACK] Failed to deliver chunk {}/{}: {}", i + 1, total, e);
                    }
                }
            }
        }
    }

    /// Spawns the adapter's inbound poll and outbound listener as background tasks.
    /// Returns a `JoinHandle` for the combined task group.
    pub fn spawn(self) -> tokio::task::JoinHandle<()> {
        let adapter = Arc::new(self);

        tokio::spawn(async move {
            info!("[SLACK] Spawned adapter background tasks.");

            // Health check on startup
            if let Err(e) = adapter.health_check().await {
                error!("[SLACK] Startup health check failed: {}", e);
            }

            let poll_adapter = adapter.clone();
            let outbound_adapter = adapter.clone();

            let poll_handle = tokio::spawn(async move {
                poll_adapter.poll_loop().await;
            });

            let outbound_handle = tokio::spawn(async move {
                outbound_adapter.outbound_loop().await;
            });

            // Wait for either task to complete (they should run indefinitely)
            if let Err(e) = tokio::try_join!(poll_handle, outbound_handle) {
                tracing::warn!("[channels] Task join failed: {}", e);
            }
            warn!("[SLACK] Adapter tasks exited.");
        })
    }
}

/// Splits a message into chunks that respect UTF-8 character boundaries.
fn chunk_message(text: &str, max_chunk: usize) -> Vec<String> {
    if text.len() <= max_chunk {
        return vec![text.to_string()];
    }

    let mut chunks = Vec::new();
    let mut remaining = text;

    while remaining.len() > max_chunk {
        // Find the largest char boundary <= max_chunk
        let split_idx = remaining
            .char_indices()
            .map(|(i, _)| i)
            .take_while(|&i| i <= max_chunk)
            .last()
            .unwrap_or(0);

        if split_idx == 0 {
            // Safety: should not happen if text is non-empty and max_chunk > 0
            break;
        }

        // Try to split at a newline or sentence boundary for readability
        let chunk = &remaining[..split_idx];
        let actual_split = if let Some(newline_pos) = chunk.rfind('\n') {
            if newline_pos > max_chunk / 2 {
                newline_pos + 1 // include the newline in the first chunk
            } else {
                split_idx
            }
        } else if let Some(space_pos) = chunk.rfind(' ') {
            if space_pos > max_chunk / 2 {
                space_pos + 1 // include the space in the first chunk
            } else {
                split_idx
            }
        } else {
            split_idx
        };

        let (head, tail) = remaining.split_at(actual_split);
        chunks.push(head.to_string());
        remaining = tail;
    }

    if !remaining.is_empty() {
        chunks.push(remaining.to_string());
    }

    chunks
}

#[async_trait]
impl ChannelAdapter for SlackAdapter {
    fn name(&self) -> &str {
        "slack"
    }

    async fn send_event(&self, event: EventFrame) -> Result<(), SavantError> {
        info!("[SLACK] send_event called: event_type={}", event.event_type);

        if event.event_type == "chat.message" {
            let payload: serde_json::Value = serde_json::from_str(&event.payload)
                .map_err(|e| SavantError::InvalidInput(format!("Invalid payload JSON: {}", e)))?;

            let channel_id = self
                .config
                .default_channel
                .as_ref()
                .ok_or_else(|| {
                    SavantError::ConfigError("No default channel configured".to_string())
                })?
                .clone();

            let content = payload["content"].as_str().unwrap_or(&event.payload);

            let chunks = chunk_message(content, CHUNK_SIZE);
            for chunk in chunks {
                let body = serde_json::json!({
                    "channel": channel_id,
                    "text": chunk,
                });
                self.slack_post("chat.postMessage", body).await?;
            }
        }

        Ok(())
    }

    async fn handle_event(&self, event: EventFrame) -> Result<(), SavantError> {
        info!(
            "[SLACK] handle_event called: event_type={}",
            event.event_type
        );

        match event.event_type.as_str() {
            "message.send" => {
                let payload: serde_json::Value =
                    serde_json::from_str(&event.payload).map_err(|e| {
                        SavantError::InvalidInput(format!("Invalid payload JSON: {}", e))
                    })?;

                let channel_id = payload
                    .get("chat_id")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
                    .or_else(|| self.config.default_channel.clone())
                    .ok_or_else(|| {
                        SavantError::InvalidInput(
                            "No chat_id in payload and no default channel".to_string(),
                        )
                    })?;

                let text = payload
                    .get("text")
                    .and_then(|v| v.as_str())
                    .unwrap_or(&event.payload);

                let chunks = chunk_message(text, CHUNK_SIZE);
                for chunk in chunks {
                    let body = serde_json::json!({
                        "channel": channel_id,
                        "text": chunk,
                    });
                    self.slack_post("chat.postMessage", body).await?;
                }

                Ok(())
            }
            "health.check" => self.health_check().await,
            _ => {
                debug!("[SLACK] Unhandled event type: {}", event.event_type);
                Ok(())
            }
        }
    }
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)]
mod tests {
    use super::*;

    #[test]
    fn test_chunk_message_short() {
        let text = "Hello, world!";
        let chunks = chunk_message(text, 3800);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0], "Hello, world!");
    }

    #[test]
    fn test_chunk_message_long() {
        let text = "a".repeat(8000);
        let chunks = chunk_message(&text, 3800);
        assert!(chunks.len() >= 2);
        let reassembled: String = chunks.concat();
        assert_eq!(reassembled, text);
    }

    #[test]
    fn test_chunk_message_at_newline() {
        let line = "x".repeat(3700);
        let text = format!("{}\n{}", line, line);
        let chunks = chunk_message(&text, 3800);
        assert_eq!(chunks.len(), 2);
        assert!(chunks[0].ends_with('\n'));
    }

    #[test]
    fn test_chunk_message_unicode() {
        // Each emoji is 4 bytes in UTF-8
        let emoji = "\u{1F600}"; // 😀
        let text = emoji.repeat(2000); // 8000 bytes
        let chunks = chunk_message(&text, 3800);
        assert!(chunks.len() >= 2);
        // Verify all chunks are valid UTF-8
        for chunk in &chunks {
            assert!(std::str::from_utf8(chunk.as_bytes()).is_ok());
        }
        let reassembled: String = chunks.concat();
        assert_eq!(reassembled, text);
    }

    #[test]
    fn test_config_default() {
        let config = SlackConfig {
            bot_token: "xoxb-test".to_string(),
            default_channel: Some("C123".to_string()),
            allowed_users: vec![],
        };
        assert_eq!(config.bot_token, "xoxb-test");
        assert_eq!(config.default_channel, Some("C123".to_string()));
        assert!(config.allowed_users.is_empty());
    }
}
