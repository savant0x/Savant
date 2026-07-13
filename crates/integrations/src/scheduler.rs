//! Sync scheduler — periodic provider synchronization.

use crate::error::IntegrationResult;
use crate::provider::{FetchResult, ProviderKind};
use crate::registry::ProviderRegistry;
use crate::state::SyncState;
use chrono::Utc;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::watch;
use tokio::sync::RwLock;
use tokio::time::interval;
use tracing::{error, info, warn};

/// Schedules periodic sync operations across all registered providers.
#[derive(Clone)]
pub struct SyncScheduler {
    /// Provider registry.
    registry: Arc<ProviderRegistry>,
    /// Persistent sync state.
    state: Arc<RwLock<SyncState>>,
    /// Path to sync state file.
    state_path: PathBuf,
    /// Default sync interval in seconds.
    default_interval_secs: u64,
    /// Shutdown signal receiver — `true` means shutdown requested.
    shutdown_rx: watch::Receiver<bool>,
}

impl SyncScheduler {
    /// Creates a new sync scheduler.
    pub async fn new(
        registry: Arc<ProviderRegistry>,
        state_path: PathBuf,
        default_interval_secs: u64,
        shutdown_rx: watch::Receiver<bool>,
    ) -> IntegrationResult<Self> {
        let state = SyncState::load(&state_path).await;
        info!(
            "[integrations] Loaded sync state: {} providers, {} dedup entries",
            state.cursors.len(),
            state.global_dedup.len()
        );
        Ok(Self {
            registry,
            state: Arc::new(RwLock::new(state)),
            state_path,
            default_interval_secs,
            shutdown_rx,
        })
    }

    /// Runs the sync scheduler loop.
    ///
    /// Ticks at the configured interval, syncing all registered providers.
    /// Exits gracefully when the shutdown signal is set to `true`.
    pub async fn run(&self) {
        info!(
            "[integrations] Starting sync scheduler (interval: {}s)",
            self.default_interval_secs
        );
        let mut ticker = interval(Duration::from_secs(self.default_interval_secs));
        let mut shutdown_rx = self.shutdown_rx.clone();

        loop {
            tokio::select! {
                result = shutdown_rx.changed() => {
                    // Check if shutdown was requested (avoids holding Ref across await)
                    let should_shutdown = result.is_ok() && *shutdown_rx.borrow();
                    if should_shutdown {
                        info!("[integrations] Sync scheduler shutting down gracefully");
                        // Persist state before exit
                        let state = self.state.read().await;
                        if let Err(e) = state.save(&self.state_path).await {
                            warn!("[integrations] Failed to persist sync state on shutdown: {}", e);
                        }
                        break;
                    }
                }
                _ = ticker.tick() => {
                    if let Err(e) = self.sync_all().await {
                        error!("[integrations] Sync all failed: {}", e);
                    }
                }
            }
        }
    }

    /// Syncs all registered providers.
    pub async fn sync_all(&self) -> IntegrationResult<()> {
        let kinds = self.registry.kinds();
        if kinds.is_empty() {
            info!("[integrations] No providers registered, skipping sync");
            return Ok(());
        }

        info!("[integrations] Syncing {} providers", kinds.len());
        let mut total_fetched = 0;

        for kind in &kinds {
            match self.sync_provider(kind).await {
                Ok(count) => total_fetched += count,
                Err(e) => {
                    warn!("[integrations] Provider {} sync failed: {}", kind, e);
                }
            }
        }

        info!(
            "[integrations] Sync complete: {} items fetched",
            total_fetched
        );
        Ok(())
    }

    /// Syncs a single provider.
    pub async fn sync_provider(&self, kind: &ProviderKind) -> IntegrationResult<usize> {
        let provider = match self.registry.get(kind) {
            Some(p) => p,
            None => {
                return Ok(0);
            }
        };

        let config = provider.config();
        if !config.enabled {
            info!("[integrations] Provider {} is disabled, skipping", kind);
            return Ok(0);
        }

        // Check cooldown
        {
            let state = self.state.read().await;
            if let Some(cursor) = state.get_cursor(&kind.to_string()) {
                if let Some(last_sync) = cursor.last_sync_at {
                    let elapsed = Utc::now().timestamp() - last_sync.timestamp();
                    if elapsed < config.sync_interval_secs as i64 {
                        info!(
                            "[integrations] Provider {} in cooldown ({}s < {}s), skipping",
                            kind, elapsed, config.sync_interval_secs
                        );
                        return Ok(0);
                    }
                }
            }
        }

        // Get cursor
        let cursor = {
            let state = self.state.read().await;
            state
                .get_cursor(&kind.to_string())
                .and_then(|c| c.cursor.clone())
        };

        // Fetch
        info!("[integrations] Fetching from {}", kind);
        let result: FetchResult = provider.fetch(cursor.as_deref()).await?;

        // Dedup and count
        let mut new_count = 0;
        {
            let mut state = self.state.write().await;
            for item in &result.items {
                if !state.is_duplicate(&item.content_hash) {
                    state.add_dedup(item.content_hash.clone());
                    new_count += 1;
                }
            }
            state.update_cursor(&kind.to_string(), result.next_cursor.clone(), new_count);
            state.increment_daily_count(new_count as u32);
        }

        // Persist state
        {
            let state = self.state.read().await;
            state.save(&self.state_path).await?;
        }

        info!(
            "[integrations] Provider {}: {} new items ({} total, {} deduped)",
            kind,
            new_count,
            result.total_count.unwrap_or(result.items.len()),
            result.items.len() - new_count
        );

        Ok(new_count)
    }
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)]
mod tests {
    use super::*;

    #[test]
    fn test_sync_state_dedup() {
        let mut state = SyncState::default();
        assert!(!state.is_duplicate("abc123"));
        state.add_dedup("abc123".to_string());
        assert!(state.is_duplicate("abc123"));
    }

    #[test]
    fn test_sync_state_budget_reset() {
        let mut state = SyncState::default();
        state.increment_daily_count(5);
        assert_eq!(state.today_count(), 5);
    }

    #[test]
    fn test_sync_cursor_update() {
        let mut state = SyncState::default();
        state.update_cursor("gmail", Some("cursor123".to_string()), 10);
        let cursor = state
            .get_cursor("gmail")
            .expect("cursor should exist after update");
        assert_eq!(cursor.cursor, Some("cursor123".to_string()));
        assert_eq!(cursor.last_fetch_count, 10);
    }
}
