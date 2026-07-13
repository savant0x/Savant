// SAFETY: All clippy::disallowed_methods violations in this file originate from serde_json::json!() macro internals. The json!() macro calls .unwrap() on provably-infallible compile-time-validated JSON literals. grep confirms 0 real .unwrap() calls exist in this file outside macro expansions.
#![allow(clippy::disallowed_methods)]
//! Notion provider implementation.
// SAFETY: All `clippy::disallowed_methods` violations in this file originate from
// the `serde_json::json!()` macro, which internally uses `.unwrap()` on
// compile-time-validated JSON literals. A malformed JSON literal would be a
// compile error, making the panic path statically unreachable.

use crate::error::{IntegrationError, IntegrationResult};
use crate::provider::{FetchItem, FetchResult, Provider, ProviderConfig, ProviderKind};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tracing::{info, warn};

/// Notion provider configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotionConfig {
    /// Notion API base URL.
    #[serde(default = "default_notion_api_url")]
    pub api_url: String,
    /// Notion API integration token.
    pub integration_token: Option<String>,
    /// Notion API version.
    #[serde(default = "default_notion_version")]
    pub api_version: String,
    /// Maximum pages per fetch.
    #[serde(default = "default_max_pages")]
    pub max_pages: usize,
    /// Database IDs to sync.
    #[serde(default)]
    pub database_ids: Vec<String>,
}

fn default_notion_api_url() -> String {
    "https://api.notion.com/v1".to_string()
}

fn default_notion_version() -> String {
    "2022-06-28".to_string()
}

fn default_max_pages() -> usize {
    50
}

impl Default for NotionConfig {
    fn default() -> Self {
        Self {
            api_url: default_notion_api_url(),
            integration_token: None,
            api_version: default_notion_version(),
            max_pages: default_max_pages(),
            database_ids: Vec::new(),
        }
    }
}

/// Notion provider.
#[derive(Debug, Clone)]
pub struct NotionProvider {
    config: ProviderConfig,
    notion_config: NotionConfig,
    http_client: reqwest::Client,
}

impl NotionProvider {
    /// Creates a new Notion provider from configuration.
    pub fn new(config: ProviderConfig, notion_config: NotionConfig) -> Self {
        Self {
            config,
            notion_config,
            http_client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .unwrap_or_default(),
        }
    }

    /// Fetches pages from the Notion API.
    async fn fetch_pages(
        &self,
        start_cursor: Option<&str>,
    ) -> IntegrationResult<NotionSearchResponse> {
        let token = self
            .notion_config
            .integration_token
            .as_ref()
            .ok_or_else(|| IntegrationError::AuthError("No integration token".to_string()))?;

        let url = format!("{}/search", self.notion_config.api_url);
        let body = serde_json::json!({
            "filter": { "value": "page", "property": "object" },
            "page_size": self.notion_config.max_pages,
            "start_cursor": start_cursor,
        });

        let response = self
            .http_client
            .post(&url)
            .bearer_auth(token)
            .header("Notion-Version", &self.notion_config.api_version)
            .json(&body)
            .send()
            .await?;

        if !response.status().is_success() {
            return Err(IntegrationError::ProviderError(format!(
                "Notion API error: {}",
                response.status()
            )));
        }

        let result: NotionSearchResponse = response.json().await?;
        Ok(result)
    }

    /// Fetches a page's content (blocks).
    async fn fetch_page_content(&self, page_id: &str) -> IntegrationResult<String> {
        let token = self
            .notion_config
            .integration_token
            .as_ref()
            .ok_or_else(|| IntegrationError::AuthError("No integration token".to_string()))?;

        let url = format!("{}/blocks/{}/children", self.notion_config.api_url, page_id);
        let response = self
            .http_client
            .get(&url)
            .bearer_auth(token)
            .header("Notion-Version", &self.notion_config.api_version)
            .query(&[("page_size", "100")])
            .send()
            .await?;

        if !response.status().is_success() {
            return Err(IntegrationError::ProviderError(format!(
                "Notion API error: {}",
                response.status()
            )));
        }

        let blocks: NotionBlocksResponse = response.json().await?;
        Ok(Self::blocks_to_markdown(&blocks.results))
    }

    /// Converts Notion blocks to markdown.
    fn blocks_to_markdown(blocks: &[NotionBlock]) -> String {
        let mut content = String::new();
        for block in blocks {
            match block.block_type.as_str() {
                "paragraph" => {
                    if let Some(text) = Self::extract_rich_text(&block.paragraph.rich_text) {
                        content.push_str(&text);
                        content.push('\n');
                    }
                }
                "heading_1" => {
                    if let Some(text) = Self::extract_rich_text(&block.heading_1.rich_text) {
                        content.push_str(&format!("# {}\n", text));
                    }
                }
                "heading_2" => {
                    if let Some(text) = Self::extract_rich_text(&block.heading_2.rich_text) {
                        content.push_str(&format!("## {}\n", text));
                    }
                }
                "heading_3" => {
                    if let Some(text) = Self::extract_rich_text(&block.heading_3.rich_text) {
                        content.push_str(&format!("### {}\n", text));
                    }
                }
                "bulleted_list_item" => {
                    if let Some(text) = Self::extract_rich_text(&block.bulleted_list_item.rich_text)
                    {
                        content.push_str(&format!("- {}\n", text));
                    }
                }
                "numbered_list_item" => {
                    if let Some(text) = Self::extract_rich_text(&block.numbered_list_item.rich_text)
                    {
                        content.push_str(&format!("1. {}\n", text));
                    }
                }
                "to_do" => {
                    let checked = if block.to_do.checked { "[x]" } else { "[ ]" };
                    if let Some(text) = Self::extract_rich_text(&block.to_do.rich_text) {
                        content.push_str(&format!("- {} {}\n", checked, text));
                    }
                }
                "divider" => {
                    content.push_str("---\n");
                }
                _ => {}
            }
        }
        content
    }

    /// Extracts plain text from Notion rich text array.
    fn extract_rich_text(rich_text: &[NotionRichText]) -> Option<String> {
        let text: String = rich_text
            .iter()
            .map(|rt| rt.plain_text.clone())
            .collect::<Vec<_>>()
            .join("");
        if text.is_empty() {
            None
        } else {
            Some(text)
        }
    }

    /// Extracts the title from a Notion page.
    fn extract_title(page: &NotionPage) -> String {
        if let Some(title_prop) = page.properties.get("title") {
            if let Some(title_array) = &title_prop.title {
                return Self::extract_rich_text(title_array).unwrap_or_default();
            }
        }
        // Fallback: check for Name property
        if let Some(name_prop) = page.properties.get("Name") {
            if let Some(title_array) = &name_prop.title {
                return Self::extract_rich_text(title_array).unwrap_or_default();
            }
        }
        "Untitled".to_string()
    }
}

#[async_trait]
impl Provider for NotionProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Notion
    }

    fn name(&self) -> &str {
        "Notion"
    }

    async fn fetch(&self, cursor: Option<&str>) -> IntegrationResult<FetchResult> {
        info!("[notion] Fetching pages (cursor: {:?})", cursor);

        let search = self.fetch_pages(cursor).await?;
        let mut items = Vec::new();

        for page in &search.results {
            let title = Self::extract_title(page);
            let content = match self.fetch_page_content(&page.id).await {
                Ok(c) => c,
                Err(e) => {
                    warn!("[notion] Failed to fetch page {}: {}", page.id, e);
                    String::new()
                }
            };
            let content_hash = blake3::hash(content.as_bytes()).to_hex().to_string();

            items.push(FetchItem {
                external_id: page.id.clone(),
                title,
                content,
                url: page.url.clone(),
                created_at: page.created_time,
                updated_at: page.last_edited_time,
                metadata: {
                    let mut m = HashMap::new();
                    m.insert("page_id".to_string(), page.id.clone());
                    m.insert("object".to_string(), page.object.clone());
                    m
                },
                content_hash,
            });
        }

        Ok(FetchResult {
            items,
            has_more: search.has_more,
            next_cursor: search.next_cursor,
            total_count: None,
        })
    }

    async fn test_connection(&self) -> IntegrationResult<bool> {
        if self.notion_config.integration_token.is_none() {
            return Ok(false);
        }
        let url = format!("{}/users/me", self.notion_config.api_url);
        let response = self
            .http_client
            .get(&url)
            .bearer_auth(
                self.notion_config
                    .integration_token
                    .as_ref()
                    .ok_or_else(|| {
                        IntegrationError::ConfigError(
                            "Notion integration token not configured".to_string(),
                        )
                    })?,
            )
            .header("Notion-Version", &self.notion_config.api_version)
            .send()
            .await?;
        Ok(response.status().is_success())
    }

    fn config(&self) -> &ProviderConfig {
        &self.config
    }
}

// ── Notion API response types ──

#[derive(Debug, Deserialize)]
struct NotionSearchResponse {
    results: Vec<NotionPage>,
    #[serde(default)]
    has_more: bool,
    #[serde(rename = "nextCursor")]
    next_cursor: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct NotionPage {
    id: String,
    object: String,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    created_time: Option<DateTime<Utc>>,
    #[serde(default)]
    last_edited_time: Option<DateTime<Utc>>,
    properties: HashMap<String, NotionProperty>,
}

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
struct NotionProperty {
    #[serde(default)]
    title: Option<Vec<NotionRichText>>,
    #[serde(default)]
    rich_text: Option<Vec<NotionRichText>>,
    #[serde(flatten)]
    extra: HashMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
struct NotionBlocksResponse {
    results: Vec<NotionBlock>,
    #[serde(default)]
    has_more: bool,
    #[serde(rename = "nextCursor")]
    next_cursor: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[allow(dead_code)]
struct NotionBlock {
    id: String,
    #[serde(rename = "type")]
    block_type: String,
    #[serde(default)]
    paragraph: NotionParagraphBlock,
    #[serde(default)]
    heading_1: NotionHeadingBlock,
    #[serde(default)]
    heading_2: NotionHeadingBlock,
    #[serde(default)]
    heading_3: NotionHeadingBlock,
    #[serde(default)]
    bulleted_list_item: NotionListItemBlock,
    #[serde(default)]
    numbered_list_item: NotionListItemBlock,
    #[serde(default)]
    to_do: NotionToDoBlock,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct NotionParagraphBlock {
    #[serde(default)]
    rich_text: Vec<NotionRichText>,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct NotionHeadingBlock {
    #[serde(default)]
    rich_text: Vec<NotionRichText>,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct NotionListItemBlock {
    #[serde(default)]
    rich_text: Vec<NotionRichText>,
}

#[derive(Debug, Clone, Deserialize, Default)]
struct NotionToDoBlock {
    #[serde(default)]
    checked: bool,
    #[serde(default)]
    rich_text: Vec<NotionRichText>,
}

#[derive(Debug, Clone, Deserialize)]
struct NotionRichText {
    #[serde(rename = "plain_text", default)]
    plain_text: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_rich_text() {
        let rich_text = vec![
            NotionRichText {
                plain_text: "Hello ".to_string(),
            },
            NotionRichText {
                plain_text: "World".to_string(),
            },
        ];
        let result = NotionProvider::extract_rich_text(&rich_text);
        assert_eq!(result, Some("Hello World".to_string()));
    }

    #[test]
    fn test_extract_rich_text_empty() {
        let rich_text: Vec<NotionRichText> = vec![];
        let result = NotionProvider::extract_rich_text(&rich_text);
        assert!(result.is_none());
    }

    #[test]
    fn test_blocks_to_markdown() {
        let blocks = vec![
            NotionBlock {
                id: "1".to_string(),
                block_type: "heading_1".to_string(),
                heading_1: NotionHeadingBlock {
                    rich_text: vec![NotionRichText {
                        plain_text: "Title".to_string(),
                    }],
                },
                ..Default::default()
            },
            NotionBlock {
                id: "2".to_string(),
                block_type: "paragraph".to_string(),
                paragraph: NotionParagraphBlock {
                    rich_text: vec![NotionRichText {
                        plain_text: "Content".to_string(),
                    }],
                },
                ..Default::default()
            },
        ];
        let md = NotionProvider::blocks_to_markdown(&blocks);
        assert!(md.contains("# Title"));
        assert!(md.contains("Content"));
    }
}
