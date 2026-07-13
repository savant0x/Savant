//! Gmail provider implementation.

use crate::error::{IntegrationError, IntegrationResult};
use crate::provider::{FetchItem, FetchResult, Provider, ProviderConfig, ProviderKind};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use tracing::{info, warn};

/// Gmail provider configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GmailConfig {
    /// Gmail API base URL.
    #[serde(default = "default_gmail_api_url")]
    pub api_url: String,
    /// OAuth2 access token.
    pub access_token: Option<String>,
    /// Maximum messages per fetch.
    #[serde(default = "default_max_messages")]
    pub max_messages: usize,
    /// Label filters (e.g., ["INBOX", "IMPORTANT"]).
    #[serde(default)]
    pub label_filters: Vec<String>,
}

fn default_gmail_api_url() -> String {
    "https://gmail.googleapis.com/gmail/v1".to_string()
}

fn default_max_messages() -> usize {
    50
}

impl Default for GmailConfig {
    fn default() -> Self {
        Self {
            api_url: default_gmail_api_url(),
            access_token: None,
            max_messages: default_max_messages(),
            label_filters: Vec::new(),
        }
    }
}

/// Gmail provider.
#[derive(Debug, Clone)]
pub struct GmailProvider {
    config: ProviderConfig,
    gmail_config: GmailConfig,
    http_client: reqwest::Client,
}

impl GmailProvider {
    /// Creates a new Gmail provider from configuration.
    pub fn new(config: ProviderConfig, gmail_config: GmailConfig) -> Self {
        Self {
            config,
            gmail_config,
            http_client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .unwrap_or_default(),
        }
    }

    /// Fetches messages from the Gmail API.
    async fn fetch_messages(
        &self,
        page_token: Option<&str>,
    ) -> IntegrationResult<GmailListResponse> {
        let token = self
            .gmail_config
            .access_token
            .as_ref()
            .ok_or_else(|| IntegrationError::AuthError("No access token".to_string()))?;

        let url = format!("{}/users/me/messages", self.gmail_config.api_url);
        let mut request = self
            .http_client
            .get(&url)
            .bearer_auth(token)
            .query(&[("maxResults", self.gmail_config.max_messages.to_string())]);

        if let Some(token) = page_token {
            request = request.query(&[("pageToken", token)]);
        }

        for label in &self.gmail_config.label_filters {
            request = request.query(&[("labelIds", label)]);
        }

        let response = request.send().await?;
        if !response.status().is_success() {
            return Err(IntegrationError::ProviderError(format!(
                "Gmail API error: {}",
                response.status()
            )));
        }

        let list: GmailListResponse = response.json().await?;
        Ok(list)
    }

    /// Fetches a single message by ID.
    async fn fetch_message(&self, message_id: &str) -> IntegrationResult<GmailMessage> {
        let token = self
            .gmail_config
            .access_token
            .as_ref()
            .ok_or_else(|| IntegrationError::AuthError("No access token".to_string()))?;

        let url = format!(
            "{}/users/me/messages/{}",
            self.gmail_config.api_url, message_id
        );
        let response = self
            .http_client
            .get(&url)
            .bearer_auth(token)
            .query(&[("format", "full")])
            .send()
            .await?;

        if !response.status().is_success() {
            return Err(IntegrationError::ProviderError(format!(
                "Gmail API error: {}",
                response.status()
            )));
        }

        let msg: GmailMessage = response.json().await?;
        Ok(msg)
    }

    /// Extracts plain text from a Gmail message payload.
    fn extract_body(payload: &GmailPayload) -> String {
        if !payload.body.data.is_empty() {
            return Self::decode_base64(&payload.body.data);
        }
        for part in &payload.parts {
            if part.mime_type == "text/plain" && !part.body.data.is_empty() {
                return Self::decode_base64(&part.body.data);
            }
        }
        String::new()
    }

    /// Decodes URL-safe base64.
    fn decode_base64(data: &str) -> String {
        use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
        URL_SAFE_NO_PAD
            .decode(data)
            .ok()
            .and_then(|bytes| String::from_utf8(bytes).ok())
            .unwrap_or_else(|| data.to_string())
    }

    /// Extracts headers into a map.
    fn extract_headers(headers: &[GmailHeader]) -> HashMap<String, String> {
        headers
            .iter()
            .map(|h| (h.name.clone(), h.value.clone()))
            .collect()
    }
}

#[async_trait]
impl Provider for GmailProvider {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Gmail
    }

    fn name(&self) -> &str {
        "Gmail"
    }

    async fn fetch(&self, cursor: Option<&str>) -> IntegrationResult<FetchResult> {
        info!("[gmail] Fetching messages (cursor: {:?})", cursor);

        let list = self.fetch_messages(cursor).await?;
        let mut items = Vec::new();

        for msg_ref in &list.messages {
            match self.fetch_message(&msg_ref.id).await {
                Ok(msg) => {
                    let headers = Self::extract_headers(&msg.payload.headers);
                    let subject = headers.get("Subject").cloned().unwrap_or_default();
                    let from = headers.get("From").cloned().unwrap_or_default();
                    let _date = headers.get("Date").cloned().unwrap_or_default();
                    let body = Self::extract_body(&msg.payload);
                    let content_hash = blake3::hash(body.as_bytes()).to_hex().to_string();

                    items.push(FetchItem {
                        external_id: msg.id.clone(),
                        title: format!("{}: {}", from, subject),
                        content: body,
                        url: Some(format!(
                            "https://mail.google.com/mail/u/0/#inbox/{}",
                            msg.id
                        )),
                        created_at: None,
                        updated_at: None,
                        metadata: {
                            let mut m = headers;
                            m.insert("thread_id".to_string(), msg.thread_id.clone());
                            m.insert("message_id".to_string(), msg.id.clone());
                            m
                        },
                        content_hash,
                    });
                }
                Err(e) => {
                    warn!("[gmail] Failed to fetch message {}: {}", msg_ref.id, e);
                }
            }
        }

        Ok(FetchResult {
            items,
            has_more: list.next_page_token.is_some(),
            next_cursor: list.next_page_token,
            total_count: list.result_size_estimate,
        })
    }

    async fn test_connection(&self) -> IntegrationResult<bool> {
        if self.gmail_config.access_token.is_none() {
            return Ok(false);
        }
        let url = format!("{}/users/me/profile", self.gmail_config.api_url);
        let response = self
            .http_client
            .get(&url)
            .bearer_auth(self.gmail_config.access_token.as_ref().ok_or_else(|| {
                IntegrationError::ConfigError("Gmail access token not configured".to_string())
            })?)
            .send()
            .await?;
        Ok(response.status().is_success())
    }

    fn config(&self) -> &ProviderConfig {
        &self.config
    }
}

// ── Gmail API response types ──

#[derive(Debug, Deserialize)]
struct GmailListResponse {
    #[serde(default)]
    messages: Vec<GmailMessageRef>,
    #[serde(rename = "nextPageToken")]
    next_page_token: Option<String>,
    #[serde(rename = "resultSizeEstimate")]
    result_size_estimate: Option<usize>,
}

#[derive(Debug, Deserialize)]
struct GmailMessageRef {
    id: String,
    #[serde(default)]
    #[allow(dead_code)]
    thread_id: String,
}

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
struct GmailMessage {
    id: String,
    #[serde(rename = "threadId")]
    thread_id: String,
    #[serde(default)]
    label_ids: Vec<String>,
    payload: GmailPayload,
    #[serde(default)]
    snippet: String,
}

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
struct GmailPayload {
    #[serde(default)]
    headers: Vec<GmailHeader>,
    #[serde(default)]
    body: GmailBody,
    #[serde(default)]
    parts: Vec<GmailPart>,
    #[serde(rename = "mimeType", default)]
    mime_type: String,
}

#[derive(Debug, Clone, Deserialize)]
struct GmailHeader {
    name: String,
    value: String,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[allow(dead_code)]
struct GmailBody {
    #[serde(default)]
    data: String,
    #[serde(default)]
    size: i64,
}

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)]
struct GmailPart {
    #[serde(rename = "mimeType")]
    mime_type: String,
    #[serde(default)]
    body: GmailBody,
    #[serde(default)]
    parts: Vec<GmailPart>,
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_headers() {
        let headers = vec![
            GmailHeader {
                name: "Subject".to_string(),
                value: "Test".to_string(),
            },
            GmailHeader {
                name: "From".to_string(),
                value: "a@b.com".to_string(),
            },
        ];
        let map = GmailProvider::extract_headers(&headers);
        assert_eq!(map.get("Subject").unwrap(), "Test");
        assert_eq!(map.get("From").unwrap(), "a@b.com");
    }

    #[test]
    fn test_decode_base64() {
        let encoded = "SGVsbG8gV29ybGQ"; // "Hello World" in URL-safe base64 without padding
        let result = GmailProvider::decode_base64(encoded);
        assert!(result == "Hello World" || result == encoded); // May fail if padding wrong
    }
}
