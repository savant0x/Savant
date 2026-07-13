//! Sync state persistence for provider cursors and dedup.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use tokio::fs;
use tracing::{info, warn};

/// Cursor state for a single provider.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SyncCursor {
    /// Provider kind.
    pub provider_kind: String,
    /// Last sync cursor (provider-specific).
    pub cursor: Option<String>,
    /// Last sync timestamp.
    pub last_sync_at: Option<DateTime<Utc>>,
    /// Number of items fetched in last sync.
    pub last_fetch_count: usize,
    /// Content hashes for deduplication.
    pub dedup_set: Vec<String>,
}

/// Persistent sync state across all providers.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SyncState {
    /// Per-provider cursors.
    pub cursors: HashMap<String, SyncCursor>,
    /// Daily budget tracking (date -> item count).
    pub daily_budget: HashMap<String, u32>,
    /// Last budget reset date.
    pub last_budget_reset: Option<String>,
    /// Global dedup set (content hashes).
    pub global_dedup: HashSet<String>,
}

impl SyncState {
    /// Loads sync state from disk.
    pub async fn load(path: &PathBuf) -> Self {
        if !path.exists() {
            return Self::default();
        }
        match fs::read_to_string(path).await {
            Ok(content) => serde_json::from_str(&content).unwrap_or_else(|e| {
                warn!(
                    "[integrations] Sync state file corrupted, resetting to default: {}",
                    e
                );
                SyncState::default()
            }),
            Err(e) => {
                warn!("[integrations] Failed to load sync state: {}", e);
                Self::default()
            }
        }
    }

    /// Saves sync state to disk.
    pub async fn save(&self, path: &PathBuf) -> crate::error::IntegrationResult<()> {
        let content = serde_json::to_string_pretty(self)
            .map_err(crate::error::IntegrationError::SerializationError)?;
        fs::write(path, content)
            .await
            .map_err(crate::error::IntegrationError::IoError)?;
        Ok(())
    }

    /// Gets the cursor for a provider.
    pub fn get_cursor(&self, provider_kind: &str) -> Option<&SyncCursor> {
        self.cursors.get(provider_kind)
    }

    /// Updates the cursor for a provider.
    pub fn update_cursor(&mut self, kind: &str, cursor: Option<String>, fetch_count: usize) {
        let entry = self.cursors.entry(kind.to_string()).or_default();
        entry.cursor = cursor;
        entry.last_sync_at = Some(Utc::now());
        entry.last_fetch_count = fetch_count;
    }

    /// Adds a content hash to the dedup set.
    pub fn add_dedup(&mut self, hash: String) {
        self.global_dedup.insert(hash);
    }

    /// Checks if a content hash is in the dedup set.
    pub fn is_duplicate(&self, hash: &str) -> bool {
        self.global_dedup.contains(hash)
    }

    /// Checks and resets daily budget if date changed.
    pub fn check_budget_reset(&mut self) {
        let today = Utc::now().format("%Y-%m-%d").to_string();
        if self.last_budget_reset.as_ref() != Some(&today) {
            info!("[integrations] Resetting daily budget for {}", today);
            self.daily_budget.clear();
            self.last_budget_reset = Some(today);
        }
    }

    /// Increments the daily item count.
    pub fn increment_daily_count(&mut self, count: u32) {
        self.check_budget_reset();
        let today = Utc::now().format("%Y-%m-%d").to_string();
        let entry = self.daily_budget.entry(today).or_insert(0);
        *entry += count;
    }

    /// Returns today's item count.
    pub fn today_count(&self) -> u32 {
        let today = Utc::now().format("%Y-%m-%d").to_string();
        self.daily_budget.get(&today).copied().unwrap_or(0)
    }
}
