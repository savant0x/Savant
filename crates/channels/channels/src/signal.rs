#![allow(clippy::disallowed_methods)]
// SAFETY: All clippy::disallowed_methods violations in this file originate from serde_json::json!() macro internals. The json!() macro calls .unwrap() on provably-infallible compile-time-validated JSON literals. grep confirms 0 real .unwrap() calls exist in this file outside macro expansions.
//! Signal Channel Adapter
//!
//! Provides integration with signal-cli daemon via HTTP + SSE.
//! Supports:
//! - SSE event stream from `/api/v1/events`
//! - JSON-RPC message sending via `/api/v1/rpc`
//! - Reconnection with exponential backoff (2s -> 60s)
//! - Typing indicators via `sendTyping` RPC
//! - Group support via `groupId` in message envelope
//! - Sender allow-listing

use async_trait::async_trait;
use reqwest::Client as HttpClient;
use savant_core::error::SavantError;
use savant_core::traits::ChannelAdapter;
use savant_core::types::{ChatMessage, ChatRole, EventFrame};
use serde_json::json;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;
use tracing::{debug, error, info, warn};

/// Signal channel adapter configuration.
#[derive(Debug, Clone)]
pub struct SignalConfig {
    /// signal-cli daemon HTTP base URL (e.g. "http://127.0.0.1:8686")
    pub http_url: String,
    /// E.164 phone number registered with signal-cli
    pub account: String,
    /// Optional group ID to bind this adapter to
    pub group_id: Option<String>,
    /// Allow-list of sender phone numbers (empty = accept all)
    pub allowed_from: Vec<String>,
}

/// Signal channel adapter backed by signal-cli daemon.
pub struct SignalAdapter {
    config: SignalConfig,
    nexus: Arc<savant_core::bus::NexusBridge>,
    http: HttpClient,
    /// Incrementing JSON-RPC request ID
    rpc_id: AtomicU64,
    /// Whether the SSE loop should attempt reconnection
    reconnecting: AtomicBool,
    /// Typing indicator state lock
    typing_lock: Mutex<()>,
}

impl SignalAdapter {
    /// Creates a new Signal adapter.
    pub fn new(config: SignalConfig, nexus: Arc<savant_core::bus::NexusBridge>) -> Self {
        let http = HttpClient::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .unwrap_or_else(|_| savant_core::net::secure_client());

        Self {
            config,
            nexus,
            http,
            rpc_id: AtomicU64::new(1),
            reconnecting: AtomicBool::new(false),
            typing_lock: Mutex::new(()),
        }
    }

    /// Builds the SSE events URL.
    fn events_url(&self) -> String {
        format!(
            "{}/api/v1/events?account={}",
            self.config.http_url.trim_end_matches('/'),
            self.config.account
        )
    }

    /// Builds the RPC endpoint URL.
    fn rpc_url(&self) -> String {
        format!("{}/api/v1/rpc", self.config.http_url.trim_end_matches('/'))
    }

    /// Sends a JSON-RPC request to the signal-cli daemon.
    async fn send_rpc(&self, method: &str, params: serde_json::Value) -> Result<(), SavantError> {
        let id = self.rpc_id.fetch_add(1, Ordering::Relaxed);
        let body = json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params,
            "id": id,
        });

        debug!("[SIGNAL] RPC -> {} (id={})", method, id);

        let resp = self
            .http
            .post(self.rpc_url())
            .json(&body)
            .send()
            .await
            .map_err(|e| SavantError::NetworkError(format!("Signal RPC request failed: {}", e)))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            error!("[SIGNAL] RPC error {}: {}", status, text);
            return Err(SavantError::NetworkError(format!(
                "Signal RPC returned {}: {}",
                status, text
            )));
        }

        debug!("[SIGNAL] RPC OK (id={})", id);
        Ok(())
    }

    /// Sends a text message via signal-cli RPC.
    async fn send_message_rpc(
        &self,
        recipient: &str,
        text: &str,
        group_id: Option<&str>,
    ) -> Result<(), SavantError> {
        let mut params = json!({
            "account": self.config.account,
            "message": text,
        });

        let params_obj = match params.as_object_mut() {
            Some(obj) => obj,
            None => {
                warn!("[SIGNAL] params is not an object");
                return Ok(());
            }
        };

        if let Some(gid) = group_id {
            params_obj.insert("groupId".to_string(), json!(gid));
        } else {
            params_obj.insert("recipient".to_string(), json!(recipient));
        }

        self.send_rpc("send", params).await
    }

    /// Sends a typing indicator (started or stopped).
    async fn send_typing_indicator(
        &self,
        group_id: Option<&str>,
        typing: bool,
    ) -> Result<(), SavantError> {
        let _guard = self.typing_lock.lock().await;

        let mut params = json!({
            "account": self.config.account,
            "typing": typing,
        });

        let params_obj = match params.as_object_mut() {
            Some(obj) => obj,
            None => {
                warn!("[SIGNAL] params is not an object");
                return Ok(());
            }
        };

        if let Some(gid) = group_id {
            params_obj.insert("groupId".to_string(), json!(gid));
        }

        self.send_rpc("sendTyping", params).await
    }

    /// Processes a single SSE event (already deserialized from accumulated `data:` lines).
    async fn process_sse_event(&self, data: &str) {
        let envelope: serde_json::Value = match serde_json::from_str(data) {
            Ok(v) => v,
            Err(e) => {
                debug!(
                    "[SIGNAL] Non-JSON SSE data: {} (error: {})",
                    &data[..data.len().min(100)],
                    e
                );
                return;
            }
        };

        // signal-cli envelope structure: { "envelope": { "source": ..., "dataMessage": { ... } } }
        let envelope = match envelope.get("envelope") {
            Some(e) => e,
            None => {
                // Some events (like "version", "exception") don't have an envelope
                if let Some(ev_type) = envelope.get("type").and_then(|v| v.as_str()) {
                    debug!("[SIGNAL] System event: {}", ev_type);
                }
                return;
            }
        };

        let source = envelope
            .get("source")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");

        // Check allow-list
        if !self.config.allowed_from.is_empty()
            && !self.config.allowed_from.contains(&source.to_string())
        {
            debug!(
                "[SIGNAL] Ignoring message from non-allowed sender: {}",
                source
            );
            return;
        }

        // Extract message content from dataMessage or syncMessage
        let (content, msg_group_id) = if let Some(data_msg) = envelope.get("dataMessage") {
            let text = data_msg
                .get("message")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let gid = data_msg
                .get("groupInfo")
                .and_then(|g| g.get("groupId"))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            (text.to_string(), gid)
        } else if let Some(sync_msg) = envelope.get("syncMessage") {
            let text = sync_msg
                .get("sentMessage")
                .and_then(|sm| sm.get("message"))
                .and_then(|v| v.as_str())
                .unwrap_or("");
            (text.to_string(), None)
        } else {
            // Typing indicators, receipts, etc. — skip
            return;
        };

        if content.is_empty() {
            return;
        }

        // Group filter: if adapter is bound to a group, only process messages from that group
        if let Some(ref bound_group) = self.config.group_id {
            match &msg_group_id {
                Some(gid) if gid == bound_group => {}
                _ => {
                    debug!("[SIGNAL] Ignoring message not from bound group");
                    return;
                }
            }
        }

        info!("[SIGNAL] Inbound message from {}: {}", source, content);

        let sender_id = format!("signal:{}", source);

        // Session anchoring: use group ID or source number
        let session_key = msg_group_id.as_deref().unwrap_or(source);
        let session_id = savant_core::session::SessionMapper::map("signal", session_key);

        let chat_message = ChatMessage {
            is_telemetry: false,
            role: ChatRole::User,
            content: content.clone(),
            sender: Some(sender_id),
            recipient: Some("savant".to_string()),
            agent_id: None,
            session_id: Some(session_id),
            channel: savant_core::types::AgentOutputChannel::Chat,
            images: Vec::new(),
            ..Default::default()
        };

        let event = EventFrame {
            event_type: "chat.message".to_string(),
            payload: match serde_json::to_string(&chat_message) {
                Ok(p) => p,
                Err(e) => {
                    error!("[SIGNAL] Failed to serialize ChatMessage: {}", e);
                    return;
                }
            },
        };

        if let Err(e) = self.nexus.event_bus.send(event) {
            error!("[SIGNAL] Failed to publish event to Nexus: {}", e);
        }
    }

    /// Runs the SSE event listener loop with exponential backoff reconnection.
    async fn run_sse_loop(&self) {
        let mut backoff_secs = 2u64;

        loop {
            let url = self.events_url();
            info!("[SIGNAL] Connecting SSE stream: {}", url);

            let resp = match self.http.get(&url).send().await {
                Ok(r) => r,
                Err(e) => {
                    error!("[SIGNAL] SSE connection failed: {}", e);
                    self.reconnecting.store(true, Ordering::Relaxed);
                    info!(
                        "[SIGNAL] Reconnecting in {}s (exponential backoff)...",
                        backoff_secs
                    );
                    tokio::time::sleep(Duration::from_secs(backoff_secs)).await;
                    backoff_secs = (backoff_secs * 2).min(60);
                    continue;
                }
            };

            if !resp.status().is_success() {
                error!("[SIGNAL] SSE returned status {}", resp.status());
                self.reconnecting.store(true, Ordering::Relaxed);
                info!("[SIGNAL] Reconnecting in {}s...", backoff_secs);
                tokio::time::sleep(Duration::from_secs(backoff_secs)).await;
                backoff_secs = (backoff_secs * 2).min(60);
                continue;
            }

            // Successful connection — reset backoff
            self.reconnecting.store(false, Ordering::Relaxed);
            backoff_secs = 2;
            info!("[SIGNAL] SSE stream connected.");

            let mut stream = resp.bytes_stream();
            let mut buffer = String::new();
            let mut data_lines: Vec<String> = Vec::new();

            use futures::StreamExt;

            while let Some(chunk_result) = stream.next().await {
                let chunk = match chunk_result {
                    Ok(c) => c,
                    Err(e) => {
                        error!("[SIGNAL] SSE stream error: {}", e);
                        break;
                    }
                };

                let text = match std::str::from_utf8(&chunk) {
                    Ok(s) => s,
                    Err(e) => {
                        error!("[SIGNAL] SSE non-UTF-8 chunk: {}", e);
                        continue;
                    }
                };

                buffer.push_str(text);

                // Process complete lines from the buffer
                while let Some(newline_pos) = buffer.find('\n') {
                    let line = buffer[..newline_pos].trim_end_matches('\r').to_string();
                    buffer = buffer[newline_pos + 1..].to_string();

                    if line.is_empty() {
                        // Empty line = dispatch accumulated data
                        if !data_lines.is_empty() {
                            let combined = data_lines.join("\n");
                            data_lines.clear();
                            self.process_sse_event(&combined).await;
                        }
                    } else if let Some(stripped) = line.strip_prefix("data:") {
                        let value = stripped.trim_start();
                        data_lines.push(value.to_string());
                    }
                    // Ignore "event:", "id:", "retry:" lines
                }
            }

            warn!("[SIGNAL] SSE stream ended. Reconnecting...");
            self.reconnecting.store(true, Ordering::Relaxed);
            tokio::time::sleep(Duration::from_secs(backoff_secs)).await;
            backoff_secs = (backoff_secs * 2).min(60);
        }
    }

    /// Runs the outbound event loop, listening on the Nexus event bus.
    async fn run_outbound_loop(&self) {
        let mut event_rx = self.nexus.subscribe().await.0;

        while let Ok(event) = event_rx.recv().await {
            if event.event_type != "chat.message" {
                continue;
            }

            if let Ok(payload) = serde_json::from_str::<serde_json::Value>(&event.payload) {
                let is_assistant = payload["role"].as_str() == Some("Assistant");
                if let Some(recipient) = payload["recipient"].as_str() {
                    let is_for_signal = recipient.starts_with("signal:");

                    if is_assistant || is_for_signal {
                        let content = payload["content"].as_str().unwrap_or("");
                        if content.is_empty() {
                            continue;
                        }

                        // Determine recipient and group
                        let (target, group_id) = if is_for_signal {
                            let number = recipient.strip_prefix("signal:").unwrap_or("");
                            (number.to_string(), self.config.group_id.clone())
                        } else {
                            // Assistant reply — use session_id to find the target
                            let session_id = payload["session_id"].as_str().unwrap_or("");
                            if let Some(session_key) = session_id.strip_prefix("signal:") {
                                (session_key.to_string(), self.config.group_id.clone())
                            } else {
                                continue;
                            }
                        };

                        // Show typing indicator before sending
                        if let Err(e) = self.send_typing_indicator(group_id.as_deref(), true).await
                        {
                            tracing::warn!("[channels] Typing indicator failed: {}", e);
                        }

                        debug!(
                            "[SIGNAL] Delivering message to {}: {}",
                            target,
                            &content[..content.len().min(80)]
                        );

                        match self
                            .send_message_rpc(&target, content, group_id.as_deref())
                            .await
                        {
                            Ok(_) => {
                                debug!("[SIGNAL] Message delivered to {}", target);
                                if let Err(e) =
                                    self.send_typing_indicator(group_id.as_deref(), false).await
                                {
                                    tracing::warn!("[channels] Typing indicator failed: {}", e);
                                }
                            }
                            Err(e) => {
                                error!("[SIGNAL] Failed to deliver message to {}: {}", target, e);
                            }
                        }
                    }
                }
            }
        }
    }
}

#[async_trait]
impl ChannelAdapter for SignalAdapter {
    fn name(&self) -> &str {
        "signal"
    }

    async fn send_event(&self, event: EventFrame) -> Result<(), SavantError> {
        debug!("[SIGNAL] Manual send_event: {:?}", event.event_type);

        if event.event_type == "chat.message" {
            if let Ok(payload) = serde_json::from_str::<serde_json::Value>(&event.payload) {
                let content = payload["content"].as_str().unwrap_or("");
                let recipient = payload["recipient"].as_str().unwrap_or("");
                let target = recipient.strip_prefix("signal:").unwrap_or(recipient);

                let group_id = self.config.group_id.as_deref();
                return self.send_message_rpc(target, content, group_id).await;
            }
        }

        Ok(())
    }

    async fn handle_event(&self, event: EventFrame) -> Result<(), SavantError> {
        debug!("[SIGNAL] Incoming internal event: {:?}", event.event_type);

        match event.event_type.as_str() {
            "typing.start" => {
                self.send_typing_indicator(self.config.group_id.as_deref(), true)
                    .await
            }
            "typing.stop" => {
                self.send_typing_indicator(self.config.group_id.as_deref(), false)
                    .await
            }
            _ => Ok(()),
        }
    }
}

impl SignalAdapter {
    /// Spawns the autonomous Signal adapter background task.
    /// Runs SSE listener and outbound event loop concurrently.
    pub fn spawn(self) -> tokio::task::JoinHandle<()> {
        let adapter = Arc::new(self);

        tokio::spawn(async move {
            info!(
                "[SIGNAL] Spawned autonomous background task for account={}",
                adapter.config.account
            );

            let sse_adapter = adapter.clone();
            let sse_handle = tokio::spawn(async move {
                sse_adapter.run_sse_loop().await;
            });

            let outbound_adapter = adapter.clone();
            let outbound_handle = tokio::spawn(async move {
                outbound_adapter.run_outbound_loop().await;
            });

            // Run both loops; if either exits, the other is dropped
            let (sse_result, outbound_result) = tokio::join!(sse_handle, outbound_handle);
            if let Err(e) = sse_result {
                tracing::warn!("[channels] SSE task failed: {}", e);
            }
            if let Err(e) = outbound_result {
                tracing::warn!("[channels] Outbound task failed: {}", e);
            }
            error!("[SIGNAL] Background task terminated.");
        })
    }
}
