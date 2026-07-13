use crate::error::SavantError;
use crate::traits::EmbeddingProvider;
use async_trait::async_trait;
use fastembed::{EmbeddingModel, InitOptions, TextEmbedding};
use lru::LruCache;
use std::num::NonZeroUsize;
use std::sync::{Arc, Mutex, RwLock};
use tracing::{info, warn};

/// Cache capacity — 1000 is always non-zero.
#[allow(clippy::disallowed_methods)]
const CACHE_CAPACITY: NonZeroUsize = NonZeroUsize::new(1000).expect("1000 is non-zero");

/// Circuit breaker states.
#[derive(Debug, Clone, Copy, PartialEq)]
enum CircuitState {
    /// Normal operation — requests pass through.
    Closed,
    /// Too many failures — requests are rejected immediately.
    Open,
    /// Testing recovery — one probe request allowed through.
    HalfOpen,
}

/// Circuit breaker for embedding provider resilience.
///
/// After `threshold` consecutive failures, the circuit opens and all requests
/// fail fast with a cached fallback or error. After `cooldown_ms`, the circuit
/// transitions to half-open and allows one probe request. If the probe succeeds,
/// the circuit closes. If it fails, the circuit re-opens.
struct CircuitBreaker {
    state: CircuitState,
    consecutive_failures: u32,
    threshold: u32,
    cooldown_ms: u64,
    last_failure: Option<std::time::Instant>,
}

impl CircuitBreaker {
    fn new(threshold: u32, cooldown_ms: u64) -> Self {
        Self {
            state: CircuitState::Closed,
            consecutive_failures: 0,
            threshold,
            cooldown_ms,
            last_failure: None,
        }
    }

    /// Returns `Ok(())` if the circuit allows a request, `Err` if it should be rejected.
    fn check(&mut self) -> Result<(), SavantError> {
        match self.state {
            CircuitState::Closed => Ok(()),
            CircuitState::Open => {
                // Check if cooldown has elapsed
                if let Some(last_fail) = self.last_failure {
                    if last_fail.elapsed().as_millis() >= self.cooldown_ms as u128 {
                        self.state = CircuitState::HalfOpen;
                        info!("Circuit breaker: transitioning to half-open (probe)");
                        Ok(())
                    } else {
                        Err(SavantError::Unknown(
                            "Circuit breaker OPEN — embedding service unavailable".to_string(),
                        ))
                    }
                } else {
                    Err(SavantError::Unknown(
                        "Circuit breaker OPEN — embedding service unavailable".to_string(),
                    ))
                }
            }
            CircuitState::HalfOpen => Ok(()),
        }
    }

    fn record_success(&mut self) {
        self.consecutive_failures = 0;
        if self.state != CircuitState::Closed {
            info!("Circuit breaker: probe succeeded, closing circuit");
            self.state = CircuitState::Closed;
        }
    }

    fn record_failure(&mut self) {
        self.consecutive_failures += 1;
        self.last_failure = Some(std::time::Instant::now());

        if self.consecutive_failures >= self.threshold {
            if self.state != CircuitState::Open {
                warn!(
                    failures = self.consecutive_failures,
                    threshold = self.threshold,
                    "Circuit breaker: opening after consecutive failures"
                );
            }
            self.state = CircuitState::Open;
        }
    }

    #[cfg(test)]
    fn state(&self) -> CircuitState {
        self.state
    }
}

/// Service for generating text embeddings using fastembed.
///
/// Uses the AllMiniLML6V2 model (384 dimensions) for sentence embeddings.
/// The model is downloaded on first use and cached locally.
/// An LRU cache stores recent embeddings for fast repeated lookups.
///
/// Includes a circuit breaker (MEM-13) that fails fast after 5 consecutive
/// embedding failures, preventing cascading timeouts.
///
/// Thread safety: `TextEmbedding` implements `Send + Sync` in fastembed 5.12.1,
/// so this service can be wrapped in `Arc` and shared across async tasks.
pub struct EmbeddingService {
    model: Arc<Mutex<TextEmbedding>>,
    cache: Arc<RwLock<LruCache<String, Vec<f32>>>>,
    circuit_breaker: Arc<Mutex<CircuitBreaker>>,
}

#[async_trait]
impl EmbeddingProvider for EmbeddingService {
    async fn embed(&self, text: &str) -> Result<Vec<f32>, SavantError> {
        let text = text.to_string();
        let model = self.model.clone();
        let cache = self.cache.clone();
        let circuit_breaker = self.circuit_breaker.clone();
        tokio::task::spawn_blocking(move || {
            // Check circuit breaker
            if let Ok(mut cb) = circuit_breaker.lock() {
                cb.check()?;
            }

            // Check cache
            {
                let cache = cache
                    .read()
                    .map_err(|e| SavantError::Unknown(format!("Cache lock poisoned: {}", e)))?;
                if let Some(embedding) = cache.peek(&text) {
                    return Ok(embedding.clone());
                }
            }

            // Run model inference
            let result = {
                let mut model = model
                    .lock()
                    .map_err(|e| SavantError::Unknown(format!("Model lock poisoned: {}", e)))?;
                match model.embed(vec![&text], None) {
                    Ok(embeddings) => embeddings[0].clone(),
                    Err(e) => {
                        if let Ok(mut cb) = circuit_breaker.lock() {
                            cb.record_failure();
                        }
                        return Err(SavantError::Unknown(format!("Embedding error: {}", e)));
                    }
                }
            };

            // Record success
            if let Ok(mut cb) = circuit_breaker.lock() {
                cb.record_success();
            }

            // Cache result
            {
                let mut cache = cache
                    .write()
                    .map_err(|e| SavantError::Unknown(format!("Cache lock poisoned: {}", e)))?;
                cache.put(text, result.clone());
            }

            Ok(result)
        })
        .await
        .map_err(|e| SavantError::Unknown(format!("Embedding task panicked: {}", e)))?
    }

    async fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, SavantError> {
        let texts: Vec<String> = texts.iter().map(|s| s.to_string()).collect();
        let model = self.model.clone();
        let cache = self.cache.clone();
        let circuit_breaker = self.circuit_breaker.clone();
        tokio::task::spawn_blocking(move || {
            if texts.is_empty() {
                return Ok(Vec::new());
            }

            // Check circuit breaker
            if let Ok(mut cb) = circuit_breaker.lock() {
                cb.check()?;
            }

            let mut results = Vec::with_capacity(texts.len());
            let mut uncached_indices = Vec::new();
            let mut uncached_texts = Vec::new();

            // Check cache
            {
                let cache = cache
                    .read()
                    .map_err(|e| SavantError::Unknown(format!("Cache lock poisoned: {}", e)))?;
                for (i, text) in texts.iter().enumerate() {
                    if let Some(embedding) = cache.peek(text.as_str()) {
                        results.push(Some(embedding.clone()));
                    } else {
                        results.push(None);
                        uncached_indices.push(i);
                        uncached_texts.push(text.clone());
                    }
                }
            }

            // Batch embed uncached
            if !uncached_texts.is_empty() {
                let uncached_refs: Vec<&str> = uncached_texts.iter().map(|s| s.as_str()).collect();
                let batch_embeddings = {
                    let mut model = model
                        .lock()
                        .map_err(|e| SavantError::Unknown(format!("Model lock poisoned: {}", e)))?;
                    match model.embed(uncached_refs, None) {
                        Ok(embeddings) => embeddings,
                        Err(e) => {
                            if let Ok(mut cb) = circuit_breaker.lock() {
                                cb.record_failure();
                            }
                            return Err(SavantError::Unknown(format!(
                                "Batch embedding error: {}",
                                e
                            )));
                        }
                    }
                };

                if let Ok(mut cb) = circuit_breaker.lock() {
                    cb.record_success();
                }

                let mut cache = cache
                    .write()
                    .map_err(|e| SavantError::Unknown(format!("Cache lock poisoned: {}", e)))?;
                for (cache_idx, embedding) in batch_embeddings.iter().enumerate() {
                    let orig_idx = uncached_indices[cache_idx];
                    cache.put(uncached_texts[cache_idx].clone(), embedding.clone());
                    results[orig_idx] = Some(embedding.clone());
                }
            }

            // COR-02: Propagate errors instead of unwrap_or_default
            results
                .into_iter()
                .enumerate()
                .map(|(i, r)| {
                    r.ok_or_else(|| {
                        SavantError::Unknown(format!("Embedding failed for text index {}", i))
                    })
                })
                .collect()
        })
        .await
        .map_err(|e| SavantError::Unknown(format!("Batch embedding task panicked: {}", e)))?
    }

    fn dimensions(&self) -> usize {
        EmbeddingService::dimensions(self)
    }
}

impl EmbeddingService {
    /// Initializes the embedding service with the default AllMiniLML6V2 model.
    ///
    /// This downloads the model on first call (~80MB) and caches it locally.
    /// Subsequent calls are fast.
    pub fn new() -> Result<Self, SavantError> {
        info!("Initializing EmbeddingService (AllMiniLML6V2, 384 dims)");
        let model = TextEmbedding::try_new(
            InitOptions::new(EmbeddingModel::AllMiniLML6V2).with_show_download_progress(true),
        )
        .map_err(|e| SavantError::ModelError(format!("Embedding init error: {}", e)))?;

        Ok(Self {
            model: Arc::new(Mutex::new(model)),
            cache: Arc::new(RwLock::new(LruCache::new(CACHE_CAPACITY))),
            circuit_breaker: Arc::new(Mutex::new(CircuitBreaker::new(5, 30_000))),
        })
    }

    /// Returns the embedding dimensionality (384 for AllMiniLML6V2).
    pub fn dimensions(&self) -> usize {
        384
    }

    /// Generates an embedding for a single text, using cache if available.
    ///
    /// This is a synchronous method. When calling from async code, use
    /// `EmbeddingProvider::embed` or wrap this in `tokio::task::spawn_blocking`.
    pub fn embed_sync(&self, text: &str) -> Result<Vec<f32>, SavantError> {
        // Check circuit breaker
        if let Ok(mut cb) = self.circuit_breaker.lock() {
            cb.check()?;
        }

        // Check cache first (read lock — concurrent reads allowed)
        {
            let cache = self
                .cache
                .read()
                .map_err(|e| SavantError::Unknown(format!("Cache lock poisoned: {}", e)))?;
            if let Some(embedding) = cache.peek(text) {
                return Ok(embedding.clone());
            }
        }

        // Run model inference
        let text_owned = text.to_string();
        let result = {
            let mut model = self
                .model
                .lock()
                .map_err(|e| SavantError::Unknown(format!("Model lock poisoned: {}", e)))?;
            match model.embed(vec![&text_owned], None) {
                Ok(embeddings) => embeddings[0].clone(),
                Err(e) => {
                    if let Ok(mut cb) = self.circuit_breaker.lock() {
                        cb.record_failure();
                    }
                    return Err(SavantError::Unknown(format!("Embedding error: {}", e)));
                }
            }
        };

        // Record success in circuit breaker
        if let Ok(mut cb) = self.circuit_breaker.lock() {
            cb.record_success();
        }

        // Cache the result (write lock)
        {
            let mut cache = self
                .cache
                .write()
                .map_err(|e| SavantError::Unknown(format!("Cache lock poisoned: {}", e)))?;
            cache.put(text_owned, result.clone());
        }

        Ok(result)
    }

    /// Generates embeddings for multiple texts in a single batch.
    ///
    /// Batch processing is significantly faster than individual calls for
    /// large numbers of texts due to optimized matrix operations.
    /// Results are returned in the same order as the input texts.
    pub fn embed_batch_sync(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, SavantError> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }

        // Check circuit breaker
        if let Ok(mut cb) = self.circuit_breaker.lock() {
            cb.check()?;
        }

        let mut results = Vec::with_capacity(texts.len());
        let mut uncached_indices = Vec::new();
        let mut uncached_texts = Vec::new();

        // Check cache for each text (read lock)
        {
            let cache = self
                .cache
                .read()
                .map_err(|e| SavantError::Unknown(format!("Cache lock poisoned: {}", e)))?;

            for (i, text) in texts.iter().enumerate() {
                if let Some(embedding) = cache.peek(*text) {
                    results.push(Some(embedding.clone()));
                } else {
                    results.push(None);
                    uncached_indices.push(i);
                    uncached_texts.push(text.to_string());
                }
            }
        }

        // Batch embed uncached texts
        if !uncached_texts.is_empty() {
            let uncached_refs: Vec<&str> = uncached_texts.iter().map(|s| s.as_str()).collect();
            let batch_embeddings = {
                let mut model = self
                    .model
                    .lock()
                    .map_err(|e| SavantError::Unknown(format!("Model lock poisoned: {}", e)))?;
                match model.embed(uncached_refs, None) {
                    Ok(embeddings) => embeddings,
                    Err(e) => {
                        if let Ok(mut cb) = self.circuit_breaker.lock() {
                            cb.record_failure();
                        }
                        return Err(SavantError::Unknown(format!(
                            "Batch embedding error: {}",
                            e
                        )));
                    }
                }
            };

            // Record success in circuit breaker
            if let Ok(mut cb) = self.circuit_breaker.lock() {
                cb.record_success();
            }

            // Populate results and cache (write lock)
            let mut cache = self
                .cache
                .write()
                .map_err(|e| SavantError::Unknown(format!("Cache lock poisoned: {}", e)))?;

            for (cache_idx, embedding) in batch_embeddings.iter().enumerate() {
                let orig_idx = uncached_indices[cache_idx];
                cache.put(uncached_texts[cache_idx].clone(), embedding.clone());
                results[orig_idx] = Some(embedding.clone());
            }
        }

        // Convert Option<Vec<f32>> to Vec<f32> (all should be Some now)
        Ok(results.into_iter().map(|r| r.unwrap_or_default()).collect())
    }

    /// Clears the embedding cache.
    pub fn clear_cache(&self) {
        if let Ok(mut cache) = self.cache.write() {
            cache.clear();
        }
    }

    pub fn cache_size(&self) -> usize {
        self.cache.read().map(|c| c.len()).unwrap_or(0)
    }
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)]
mod tests {
    use super::*;

    #[test]
    fn test_circuit_breaker_starts_closed() {
        let mut cb = CircuitBreaker::new(5, 30_000);
        assert_eq!(cb.state(), CircuitState::Closed);
        assert!(cb.check().is_ok());
    }

    #[test]
    fn test_circuit_breaker_opens_after_threshold() {
        let mut cb = CircuitBreaker::new(3, 30_000);
        cb.record_failure();
        cb.record_failure();
        assert_eq!(cb.state(), CircuitState::Closed);
        cb.record_failure();
        assert_eq!(cb.state(), CircuitState::Open);
        assert!(cb.check().is_err());
    }

    #[test]
    fn test_circuit_breaker_half_open_after_cooldown() {
        let mut cb = CircuitBreaker::new(1, 0); // 0ms cooldown
        cb.record_failure();
        assert_eq!(cb.state(), CircuitState::Open);
        // With 0ms cooldown, should transition to half-open immediately
        assert!(cb.check().is_ok());
        assert_eq!(cb.state(), CircuitState::HalfOpen);
    }

    #[test]
    fn test_circuit_breaker_closes_on_success() {
        let mut cb = CircuitBreaker::new(1, 0);
        cb.record_failure();
        assert_eq!(cb.state(), CircuitState::Open);
        cb.check().ok(); // transition to half-open
        cb.record_success();
        assert_eq!(cb.state(), CircuitState::Closed);
        assert_eq!(cb.consecutive_failures, 0);
    }

    #[test]
    fn test_circuit_breaker_reopens_on_failure_in_half_open() {
        let mut cb = CircuitBreaker::new(1, 0);
        cb.record_failure(); // open
        cb.check().ok(); // half-open
        cb.record_failure(); // should reopen
        assert_eq!(cb.state(), CircuitState::Open);
    }
}
