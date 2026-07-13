//! Provider trait and types for external data source integrations.

use crate::error::IntegrationResult;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Kind of external data provider.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum ProviderKind {
    #[serde(rename = "gmail")]
    Gmail,
    #[serde(rename = "notion")]
    Notion,
    #[serde(rename = "github")]
    GitHub,
    #[serde(rename = "slack")]
    Slack,
    #[serde(rename = "custom")]
    Custom(String),
}

impl std::fmt::Display for ProviderKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProviderKind::Gmail => write!(f, "gmail"),
            ProviderKind::Notion => write!(f, "notion"),
            ProviderKind::GitHub => write!(f, "github"),
            ProviderKind::Slack => write!(f, "slack"),
            ProviderKind::Custom(name) => write!(f, "custom:{}", name),
        }
    }
}

/// A single fetched item from an external provider.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FetchItem {
    /// Unique identifier from the provider.
    pub external_id: String,
    /// Item title or subject.
    pub title: String,
    /// Item content (plain text or markdown).
    pub content: String,
    /// Item URL.
    pub url: Option<String>,
    /// Creation timestamp.
    pub created_at: Option<DateTime<Utc>>,
    /// Last modification timestamp.
    pub updated_at: Option<DateTime<Utc>>,
    /// Provider-specific metadata.
    pub metadata: HashMap<String, String>,
    /// Content hash for deduplication.
    pub content_hash: String,
}

/// Result of a provider fetch operation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FetchResult {
    /// Items fetched.
    pub items: Vec<FetchItem>,
    /// Whether there are more items to fetch.
    pub has_more: bool,
    /// Cursor for the next fetch.
    pub next_cursor: Option<String>,
    /// Total count (if available).
    pub total_count: Option<usize>,
}

/// Configuration for a provider instance.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderConfig {
    /// Provider kind.
    pub kind: ProviderKind,
    /// Provider-specific settings.
    pub settings: HashMap<String, String>,
    /// Whether the provider is enabled.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Sync interval in seconds (default: 1200 = 20 minutes).
    #[serde(default = "default_sync_interval")]
    pub sync_interval_secs: u64,
}

fn default_true() -> bool {
    true
}
fn default_sync_interval() -> u64 {
    1200
}

/// Trait for external data source providers.
#[async_trait]
pub trait Provider: Send + Sync {
    /// Returns the provider kind.
    fn kind(&self) -> ProviderKind;

    /// Returns the provider name.
    fn name(&self) -> &str;

    /// Fetches new items since the given cursor.
    async fn fetch(&self, cursor: Option<&str>) -> IntegrationResult<FetchResult>;

    /// Tests the provider connection.
    async fn test_connection(&self) -> IntegrationResult<bool>;

    /// Returns the provider configuration.
    fn config(&self) -> &ProviderConfig;
}
