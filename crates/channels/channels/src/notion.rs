#![allow(clippy::disallowed_methods)]
// SAFETY: All clippy::disallowed_methods violations in this file originate from serde_json::json!() macro internals. The json!() macro calls .unwrap() on provably-infallible compile-time-validated JSON literals. grep confirms 0 real .unwrap() calls exist in this file outside macro expansions.
use async_trait::async_trait;
use savant_core::error::SavantError;
use savant_core::traits::ChannelAdapter;
use savant_core::types::{ChatMessage, ChatRole, EventFrame};
use std::sync::Arc;
use std::time::Duration;
use tracing::{info, warn};

/// Notion channel configuration.
#[derive(Debug, Clone)]
pub struct NotionConfig {
    pub api_key: String,
    pub database_id: Option<String>,
}

/// Notion channel adapter.
/// Polls a Notion database for new pages and can append content to pages.
pub struct NotionAdapter {
    config: NotionConfig,
    http: reqwest::Client,
    nexus: Arc<savant_core::bus::NexusBridge>,
}

impl NotionAdapter {
    pub fn new(config: NotionConfig, nexus: Arc<savant_core::bus::NexusBridge>) -> Self {
        Self {
            config,
            http: savant_core::net::secure_client(),
            nexus,
        }
    }

    /// Queries the configured database for recently updated pages.
    async fn query_database(&self, db_id: &str) -> Result<Vec<serde_json::Value>, SavantError> {
        let resp: serde_json::Value = self
            .http
            .post(format!(
                "https://api.notion.com/v1/databases/{}/query",
                db_id
            ))
            .bearer_auth(&self.config.api_key)
            .header("Notion-Version", "2022-06-28")
            .json(&serde_json::json!({
                "sorts": [{ "timestamp": "last_edited_time", "direction": "descending" }],
                "page_size": 10
            }))
            .send()
            .await
            .map_err(|e| SavantError::Unknown(e.to_string()))?
            .json()
            .await
            .map_err(|e| SavantError::Unknown(e.to_string()))?;

        Ok(resp["results"].as_array().cloned().unwrap_or_default())
    }

    /// Appends a text block to a page.
    async fn append_to_page(&self, page_id: &str, text: &str) -> Result<(), SavantError> {
        let resp = self
            .http
            .patch(format!(
                "https://api.notion.com/v1/blocks/{}/children",
                page_id
            ))
            .bearer_auth(&self.config.api_key)
            .header("Notion-Version", "2022-06-28")
            .json(&serde_json::json!({
                "children": [{
                    "object": "block",
                    "type": "paragraph",
                    "paragraph": {
                        "rich_text": [{ "type": "text", "text": { "content": text } }]
                    }
                }]
            }))
            .send()
            .await
            .map_err(|e| SavantError::Unknown(e.to_string()))?;

        if !resp.status().is_success() {
            warn!("[NOTION] Append failed: {}", resp.status());
        }
        Ok(())
    }

    /// Extracts title from a Notion page.
    fn extract_title(page: &serde_json::Value) -> String {
        if let Some(properties) = page["properties"].as_object() {
            for (_, prop) in properties {
                if prop["type"].as_str() == Some("title") {
                    if let Some(title_arr) = prop["title"].as_array() {
                        return title_arr
                            .iter()
                            .filter_map(|t| t["plain_text"].as_str())
                            .collect::<Vec<_>>()
                            .join("");
                    }
                }
            }
        }
        "Untitled".to_string()
    }

    pub fn spawn(self) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            info!("[NOTION] Starting Notion adapter");
            let nexus = self.nexus.clone();
            let (mut event_rx, _) = nexus.subscribe().await;

            if let Some(ref db_id) = self.config.database_id.clone() {
                // Outbound: append agent responses to Notion pages
                let outbound = Arc::new(self);
                let out = outbound.clone();
                tokio::spawn(async move {
                    while let Ok(event) = event_rx.recv().await {
                        if event.event_type == "chat.message" {
                            if let Ok(p) = serde_json::from_str::<serde_json::Value>(&event.payload)
                            {
                                if p["recipient"]
                                    .as_str()
                                    .is_some_and(|r| r.starts_with("notion:"))
                                    || p["role"].as_str() == Some("Assistant")
                                {
                                    let sid = p["session_id"].as_str().unwrap_or("");
                                    if let Some(page_id) = sid.strip_prefix("notion:") {
                                        let text = p["content"].as_str().unwrap_or("");
                                        if let Err(e) = out.append_to_page(page_id, text).await {
                                            warn!("[NOTION] Append error: {}", e);
                                        }
                                    }
                                }
                            }
                        }
                    }
                });

                // Inbound: poll database for new pages
                loop {
                    match outbound.query_database(db_id).await {
                        Ok(pages) => {
                            for page in &pages {
                                let page_id = page["id"].as_str().unwrap_or("");
                                let title = Self::extract_title(page);
                                let last_edited = page["last_edited_time"].as_str().unwrap_or("");
                                if !page_id.is_empty() {
                                    let sid =
                                        savant_core::session::SessionMapper::map("notion", page_id);
                                    let content = format!(
                                        "Notion page updated: {}\nEdited: {}",
                                        title, last_edited
                                    );
                                    let msg = ChatMessage {
                                        is_telemetry: false,
                                        role: ChatRole::User,
                                        content,
                                        sender: Some("notion:system".into()),
                                        recipient: Some("savant".into()),
                                        agent_id: None,
                                        session_id: Some(sid),
                                        channel: savant_core::types::AgentOutputChannel::Chat,
                                        images: Vec::new(),
                                        ..Default::default()
                                    };
                                    let frame = EventFrame {
                                        event_type: "chat.message".into(),
                                        payload: serde_json::to_string(&msg).unwrap_or_default(),
                                    };
                                    if let Err(e) = outbound.nexus.event_bus.send(frame) {
                                        tracing::warn!("[channels] Event publish failed: {}", e);
                                    }
                                }
                            }
                        }
                        Err(e) => warn!("[NOTION] Query error: {}", e),
                    }
                    tokio::time::sleep(Duration::from_secs(60)).await;
                }
            } else {
                futures::future::pending::<()>().await;
            }
        })
    }
}

#[async_trait]
impl ChannelAdapter for NotionAdapter {
    fn name(&self) -> &str {
        "notion"
    }

    async fn send_event(&self, event: EventFrame) -> Result<(), SavantError> {
        if event.event_type != "chat.message" {
            return Ok(());
        }
        let payload: serde_json::Value = serde_json::from_str(&event.payload)
            .map_err(|e| SavantError::Unknown(e.to_string()))?;
        let content = payload["content"].as_str().unwrap_or("");
        let session_id = payload["session_id"].as_str().unwrap_or("");

        let page_id = session_id
            .strip_prefix("notion:")
            .or(self.config.database_id.as_deref())
            .ok_or_else(|| SavantError::Unknown("No Notion page_id available".to_string()))?;

        self.append_to_page(page_id, content).await
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
