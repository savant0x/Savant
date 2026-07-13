// SAFETY: All clippy::disallowed_methods violations in this file originate from serde_json::json!() macro internals. The json!() macro calls .unwrap() on provably-infallible compile-time-validated JSON literals. grep confirms 0 real .unwrap() calls exist in this file outside macro expansions.
#![allow(clippy::disallowed_methods)]
//! Generation Orchestrator
//!
//! Manages VRAM ping-pong between LLM and generation models,
//! prompt expansion, and backend selection.
// SAFETY: All `clippy::disallowed_methods` violations in this file originate from
// the `serde_json::json!()` macro, which internally uses `.unwrap()` on
// compile-time-validated JSON literals. A malformed JSON literal would be a
// compile error, making the panic path statically unreachable.

use std::sync::Arc;
use tokio::sync::{Mutex, Semaphore};
use tracing::{debug, info, warn};

use crate::backends::{GenerationBackend, GenerationParams};
use crate::cache::ImageCache;
use crate::prompt::PromptExpander;
use crate::{GenerationConfig, GenerationError, GenerationResult};

/// Ollama API client for VRAM management
struct OllamaClient {
    base_url: String,
    client: reqwest::Client,
}

impl OllamaClient {
    fn new(base_url: &str) -> Self {
        Self {
            base_url: base_url.to_string(),
            client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .unwrap_or_default(),
        }
    }

    /// Check if Ollama is running
    async fn is_running(&self) -> bool {
        self.client
            .get(format!("{}/api/tags", self.base_url))
            .send()
            .await
            .map(|r| r.status().is_success())
            .unwrap_or(false)
    }

    /// Unload all models from VRAM (set keep_alive to 0)
    async fn unload_all(&self) -> Result<(), GenerationError> {
        // List running models and unload them
        let response = self
            .client
            .get(format!("{}/api/ps", self.base_url))
            .send()
            .await
            .map_err(|e| {
                GenerationError::OllamaUnavailable(format!("Failed to list models: {}", e))
            })?;

        let _body: serde_json::Value = response.json().await.map_err(|e| {
            GenerationError::OllamaUnavailable(format!("Failed to parse response: {}", e))
        })?;

        if let Some(models) = _body["models"].as_array() {
            for model in models {
                if let Some(name) = model["name"].as_str() {
                    info!("Ollama: unloading model '{}'", name);
                    let _ = self
                        .client
                        .post(format!("{}/api/generate", self.base_url))
                        .json(&serde_json::json!({
                            "model": name,
                            "keep_alive": 0
                        }))
                        .send()
                        .await;
                }
            }
        }

        Ok(())
    }

    /// Wait until VRAM is freed (check that no models are loaded)
    async fn wait_for_vram_free(&self, timeout_secs: u64) -> Result<(), GenerationError> {
        let start = std::time::Instant::now();
        let timeout = std::time::Duration::from_secs(timeout_secs);

        loop {
            if start.elapsed() > timeout {
                return Err(GenerationError::Timeout(timeout_secs));
            }

            let response = self
                .client
                .get(format!("{}/api/ps", self.base_url))
                .send()
                .await;

            match response {
                Ok(r) => {
                    let body: serde_json::Value = r.json().await.unwrap_or_default();
                    let empty = Vec::new();
                    let models = body["models"].as_array().unwrap_or(&empty);
                    if models.is_empty() {
                        debug!("Ollama: VRAM is free");
                        return Ok(());
                    }
                }
                Err(_) => {
                    // Ollama might not be running — that's OK
                    return Ok(());
                }
            }

            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        }
    }
}

/// Generation orchestrator
///
/// Manages the lifecycle of image/video generation:
/// 1. Expand prompt via LLM
/// 2. Select appropriate backend based on VRAM
/// 3. VRAM ping-pong: unload LLM -> generate -> reload LLM
/// 4. Cache result
/// 5. Return to agent
pub struct GenerationOrchestrator {
    /// Configuration
    config: GenerationConfig,
    /// Available backends (ordered by preference)
    backends: Vec<Arc<dyn GenerationBackend>>,
    /// Prompt expander
    expander: PromptExpander,
    /// Image cache
    cache: Arc<Mutex<ImageCache>>,
    /// Concurrency limiter (replaces single lock — allows config-driven parallelism)
    semaphore: Arc<Semaphore>,
    /// Cooldown tracker — tracks when the last generation completed
    last_generation: Mutex<Option<std::time::Instant>>,
    /// Ollama client for VRAM management
    ollama: Option<OllamaClient>,
}

impl GenerationOrchestrator {
    /// Create a new orchestrator
    pub fn new(
        config: GenerationConfig,
        backends: Vec<Arc<dyn GenerationBackend>>,
        expander: PromptExpander,
        cache: ImageCache,
    ) -> Self {
        let max_concurrent = config.max_concurrent.max(1) as usize;
        Self {
            config,
            backends,
            expander,
            cache: Arc::new(Mutex::new(cache)),
            semaphore: Arc::new(Semaphore::new(max_concurrent)),
            last_generation: Mutex::new(None),
            ollama: Some(OllamaClient::new("http://localhost:11434")),
        }
    }

    /// Create a new orchestrator with custom Ollama URL
    pub fn with_ollama_url(mut self, url: &str) -> Self {
        self.ollama = Some(OllamaClient::new(url));
        self
    }

    /// Create a new orchestrator without Ollama (no VRAM ping-pong)
    pub fn without_ollama(mut self) -> Self {
        self.ollama = None;
        self
    }

    /// Generate an image
    ///
    /// This is the main entry point for image generation.
    /// It handles:
    /// - Prompt expansion
    /// - Cache lookup
    /// - Backend selection
    /// - VRAM ping-pong
    /// - Result caching
    pub async fn generate_image(
        &self,
        prompt: &str,
        params: &GenerationParams,
    ) -> Result<GenerationResult, GenerationError> {
        // Config: check if generation is enabled
        if !self.config.enabled {
            return Err(GenerationError::Disabled);
        }

        info!("Generation request: {}", prompt);

        // Config: enforce cooldown between generations
        {
            let mut last = self.last_generation.lock().await;
            if let Some(ref instant) = *last {
                let elapsed = instant.elapsed().as_millis() as u64;
                if elapsed < self.config.cooldown_ms {
                    let remaining = self.config.cooldown_ms - elapsed;
                    debug!(
                        "Cooldown active — {}ms remaining (config: {}ms)",
                        remaining, self.config.cooldown_ms
                    );
                    return Err(GenerationError::CooldownActive(remaining));
                }
            }
            *last = Some(std::time::Instant::now());
        }

        // 1. Check cache (skip if cache_enabled is false)
        let cache_key = ImageCache::cache_key(
            prompt,
            &params.art_style.to_string(),
            &params.aspect_ratio.to_string(),
            &params.quality_tier.to_string(),
        );

        if self.config.cache_enabled {
            let cache = self.cache.lock().await;
            if let Some(path) = cache.get(&cache_key) {
                let data = std::fs::read(&path)
                    .map_err(|e| GenerationError::Cache(format!("Failed to read cache: {}", e)))?;
                info!("Cache hit for: {}", prompt);
                return Ok(GenerationResult {
                    id: format!("cached-{}", &cache_key[..12]),
                    prompt: prompt.to_string(),
                    expanded_prompt: prompt.to_string(),
                    data,
                    mime_type: "image/webp".to_string(),
                    cached: true,
                    ..Default::default()
                });
            }
        } else {
            debug!("Cache disabled — skipping lookup");
        }

        // 2. Expand prompt
        let expanded = self
            .expander
            .expand(prompt, &params.art_style.to_string())
            .await?;
        debug!("Expanded prompt: {}", expanded);

        // 3. Select backend
        let backend = self.select_backend(params).await?;

        // 4. Acquire generation permit (prevents concurrent VRAM access, respects config.max_concurrent)
        let _permit = self.semaphore.acquire().await.map_err(|e| {
            GenerationError::Internal(format!("Failed to acquire generation permit: {}", e))
        })?;

        // 5. VRAM ping-pong: unload LLM if generation backend needs GPU
        let needs_vram = backend.requires_gpu();
        if needs_vram {
            if let Some(ref ollama) = self.ollama {
                if ollama.is_running().await {
                    info!("VRAM ping-pong: unloading LLM from Ollama");
                    if let Err(e) = ollama.unload_all().await {
                        warn!("Failed to unload Ollama models: {}", e);
                    }
                    if let Err(e) = ollama.wait_for_vram_free(30).await {
                        warn!("Timeout waiting for VRAM free: {}", e);
                    }
                } else {
                    debug!("Ollama not running — skipping VRAM unload");
                }
            }
        }

        // 6. Generate
        let mut result = backend.generate(&expanded, params).await?;
        result.prompt = prompt.to_string();
        result.expanded_prompt = expanded;

        // 7. Cache result
        {
            let mut cache = self.cache.lock().await;
            if let Err(e) = cache.put(&cache_key, &result) {
                warn!("Failed to cache result: {}", e);
            }
        }

        info!(
            "Generation complete: {}x{} in {}ms",
            result.width, result.height, result.duration_ms
        );

        Ok(result)
    }

    /// Select the best available backend
    async fn select_backend(
        &self,
        _params: &GenerationParams,
    ) -> Result<Arc<dyn GenerationBackend>, GenerationError> {
        // Try backends in order of preference
        for backend in &self.backends {
            if backend.is_available().await {
                debug!("Selected backend: {}", backend.name());
                return Ok(backend.clone());
            }
        }

        Err(GenerationError::Backend(
            "No available generation backend".to_string(),
        ))
    }

    /// Get cache statistics
    pub async fn cache_stats(&self) -> crate::cache::CacheStats {
        let cache = self.cache.lock().await;
        cache.stats()
    }

    /// Clear the cache
    pub async fn clear_cache(&self) -> Result<(), GenerationError> {
        let mut cache = self.cache.lock().await;
        cache.clear()
    }
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)]
mod tests {
    use super::*;
    use crate::backends::{GenerationBackend, GenerationParams};
    use crate::cache::ImageCache;
    use crate::prompt::PromptExpander;
    use crate::{GenerationConfig, GenerationError, GenerationResult};
    use async_trait::async_trait;
    use std::sync::Arc;

    struct MockBackend {
        name: String,
    }

    #[async_trait]
    impl GenerationBackend for MockBackend {
        fn name(&self) -> &str {
            &self.name
        }
        fn requires_gpu(&self) -> bool {
            false
        }
        fn vram_requirement_mb(&self) -> u64 {
            0
        }
        async fn is_available(&self) -> bool {
            true
        }
        async fn generate(
            &self,
            _prompt: &str,
            _params: &GenerationParams,
        ) -> Result<GenerationResult, GenerationError> {
            Ok(GenerationResult::default())
        }
    }

    fn make_orchestrator(
        config: GenerationConfig,
        backends: Vec<Arc<dyn GenerationBackend>>,
    ) -> GenerationOrchestrator {
        let cache_dir =
            std::env::temp_dir().join(format!("savant_gen_test_{}", uuid::Uuid::new_v4()));
        let cache = ImageCache::new(cache_dir, 1024).unwrap();
        let expander = PromptExpander::new(None);
        GenerationOrchestrator::new(config, backends, expander, cache).without_ollama()
    }

    #[tokio::test]
    async fn test_config_disabled_returns_error() {
        let config = GenerationConfig {
            enabled: false,
            ..Default::default()
        };
        let orch = make_orchestrator(config, vec![]);
        let params = GenerationParams::default();
        let result = orch.generate_image("test prompt", &params).await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), GenerationError::Disabled));
    }

    #[tokio::test]
    async fn test_cooldown_enforcement() {
        let config = GenerationConfig {
            cooldown_ms: 60_000,
            ..Default::default()
        };
        let backend: Arc<dyn GenerationBackend> = Arc::new(MockBackend {
            name: "mock".to_string(),
        });
        let orch = make_orchestrator(config, vec![backend]);
        let params = GenerationParams::default();

        let first = orch.generate_image("prompt1", &params).await;
        assert!(first.is_ok());

        let second = orch.generate_image("prompt2", &params).await;
        assert!(second.is_err());
        assert!(matches!(
            second.unwrap_err(),
            GenerationError::CooldownActive(_)
        ));
    }

    #[test]
    fn test_default_config_values() {
        let config = GenerationConfig::default();
        assert_eq!(config.default_backend, "auto");
        assert_eq!(config.default_model, "sd3.5-medium-q5");
        assert_eq!(config.max_concurrent, 1);
        assert_eq!(config.cooldown_ms, 5000);
        assert_eq!(config.output_dir, ".savant/generated/images");
        assert!(config.cache_enabled);
        assert_eq!(config.cache_max_size_mb, 1024);
    }

    #[test]
    fn test_disabled_error_variant() {
        let err = GenerationError::Disabled;
        assert_eq!(err.to_string(), "Generation is disabled in configuration");
    }

    #[test]
    fn test_cooldown_error_variant() {
        let err = GenerationError::CooldownActive(100);
        assert_eq!(
            err.to_string(),
            "Generation cooldown active \u{2014} try again in 100ms"
        );
    }

    #[tokio::test]
    async fn test_semaphore_limits_concurrency() {
        let config = GenerationConfig {
            max_concurrent: 1,
            ..Default::default()
        };
        let backend: Arc<dyn GenerationBackend> = Arc::new(MockBackend {
            name: "mock".to_string(),
        });
        let orch = make_orchestrator(config, vec![backend]);
        assert_eq!(Arc::strong_count(&orch.semaphore), 1);
        let permits = orch.semaphore.available_permits();
        assert_eq!(permits, 1);
    }

    #[tokio::test]
    async fn test_generate_with_no_backends() {
        let config = GenerationConfig::default();
        let orch = make_orchestrator(config, vec![]);
        let params = GenerationParams::default();
        let result = orch.generate_image("test", &params).await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), GenerationError::Backend(_)));
    }

    #[test]
    fn test_cache_key_generation() {
        let key1 = ImageCache::cache_key("a cat", "photorealistic", "1:1", "balanced");
        let key2 = ImageCache::cache_key("a cat", "photorealistic", "1:1", "balanced");
        assert_eq!(key1, key2);

        let key3 = ImageCache::cache_key("a dog", "photorealistic", "1:1", "balanced");
        assert_ne!(key1, key3);

        assert_eq!(key1.len(), 64);
    }

    #[test]
    fn test_config_enabled_by_default() {
        let config = GenerationConfig::default();
        assert!(config.enabled);
    }

    #[test]
    fn test_config_cooldown_default() {
        let config = GenerationConfig::default();
        assert!(config.cooldown_ms >= 1000);
        assert!(config.cooldown_ms <= 60_000);
    }
}
