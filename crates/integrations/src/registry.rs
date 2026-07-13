//! Provider registry — manages all configured providers.

use crate::error::IntegrationResult;
use crate::provider::{Provider, ProviderConfig, ProviderKind};
use dashmap::DashMap;
use std::fmt;
use std::sync::Arc;
use tracing::{info, warn};

/// Registry of all configured providers.
#[derive(Clone)]
pub struct ProviderRegistry {
    /// Map from provider kind to provider instance.
    providers: DashMap<ProviderKind, Arc<dyn Provider>>,
    /// Provider configurations.
    configs: DashMap<ProviderKind, ProviderConfig>,
}

impl fmt::Debug for ProviderRegistry {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let kinds: Vec<String> = self.providers.iter().map(|e| e.key().to_string()).collect();
        f.debug_struct("ProviderRegistry")
            .field("providers", &kinds)
            .field("count", &self.providers.len())
            .finish()
    }
}

impl ProviderRegistry {
    /// Creates a new empty registry.
    pub fn new() -> Self {
        Self {
            providers: DashMap::new(),
            configs: DashMap::new(),
        }
    }

    /// Registers a provider.
    pub fn register(&self, config: ProviderConfig, provider: Arc<dyn Provider>) {
        let kind = config.kind.clone();
        info!("[integrations] Registering provider: {}", kind);
        self.configs.insert(kind.clone(), config);
        self.providers.insert(kind, provider);
    }

    /// Returns a provider by kind.
    pub fn get(&self, kind: &ProviderKind) -> Option<Arc<dyn Provider>> {
        self.providers.get(kind).map(|p| p.value().clone())
    }

    /// Returns all registered provider kinds.
    pub fn kinds(&self) -> Vec<ProviderKind> {
        self.providers.iter().map(|e| e.key().clone()).collect()
    }

    /// Returns the number of registered providers.
    pub fn len(&self) -> usize {
        self.providers.len()
    }

    pub fn is_empty(&self) -> bool {
        self.providers.is_empty()
    }

    /// Returns a provider configuration.
    pub fn get_config(&self, kind: &ProviderKind) -> Option<ProviderConfig> {
        self.configs.get(kind).map(|c| c.value().clone())
    }

    /// Tests all provider connections.
    pub async fn test_all(&self) -> IntegrationResult<Vec<(ProviderKind, bool)>> {
        let mut results = Vec::new();
        for entry in self.providers.iter() {
            let kind = entry.key().clone();
            let provider = entry.value();
            match provider.test_connection().await {
                Ok(ok) => results.push((kind, ok)),
                Err(e) => {
                    warn!(
                        "[integrations] Provider {} connection test failed: {}",
                        kind, e
                    );
                    results.push((kind, false));
                }
            }
        }
        Ok(results)
    }
}

impl Default for ProviderRegistry {
    fn default() -> Self {
        Self::new()
    }
}
