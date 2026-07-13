//! Provider Chain — Error classification, cooldown, circuit breaker, response cache.
//!
//! Wraps any `LlmProvider` with 4 layers of resilience:
//! 1. **Error Classifier** — categorizes errors for intelligent retry decisions
//! 2. **Cooldown Tracker** — exponential backoff per provider (prevents thundering herd)
//! 3. **Circuit Breaker** — stops hitting dead providers after N consecutive failures
//! 4. **Response Cache** — deduplicates identical queries (saves money and latency)

use crate::providers::privacy_router::{PrivacyConfig, PrivacyRouter, RoutingDecision};
use savant_core::error::SavantError;
use savant_core::traits::LlmProvider;
use savant_core::types::{ChatChunk, ChatMessage};
use std::collections::HashMap;
use std::pin::Pin;
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures::stream::Stream;
use sha2::{Digest, Sha256};

/// Extracts retry-after seconds from a RateLimit error message.
/// Providers embed "retry after Ns" in the error message when a 429 response
/// includes a Retry-After header. Returns None if not found.
fn extract_retry_after_from_error(error: &SavantError) -> Option<u64> {
    match error {
        SavantError::RateLimit(msg) => {
            // Parse "retry after Ns" pattern embedded by check_response_retry_after
            let marker = "retry after ";
            if let Some(pos) = msg.find(marker) {
                let rest = &msg[pos + marker.len()..];
                if let Some(end) = rest.find('s') {
                    return rest[..end].parse::<u64>().ok();
                }
            }
            None
        }
        _ => None,
    }
}

// ============================================================================
// 1. Error Classifier
// ============================================================================

/// Categorized error types for intelligent retry decisions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ErrorCategory {
    /// 401, 403 — credentials invalid or expired
    Auth,
    /// 429 — rate limit exceeded
    RateLimit,
    /// Payment/billing issues
    Billing,
    /// Request timeout or connection timeout
    Timeout,
    /// 400 — malformed request
    Format,
    /// 500, 502, 503, 504 — server overloaded
    Overloaded,
    /// Network errors, transient failures
    Transient,
}

/// Classifies a SavantError into an ErrorCategory.
pub fn classify_error(error: &SavantError) -> ErrorCategory {
    match error {
        // PB-07: Explicit match arms for structured error variants
        SavantError::Timeout(_) => ErrorCategory::Timeout,
        SavantError::RateLimit(_) => ErrorCategory::RateLimit,
        SavantError::NetworkError(_) => ErrorCategory::Transient,
        SavantError::CircuitBreakerTripped(_) => ErrorCategory::Overloaded,
        SavantError::AuthError(msg) => {
            let lower = msg.to_lowercase();
            if lower.contains("429") || lower.contains("rate limit") || lower.contains("ratelimit")
            {
                ErrorCategory::RateLimit
            } else if lower.contains("401")
                || lower.contains("403")
                || lower.contains("unauthorized")
                || lower.contains("forbidden")
            {
                ErrorCategory::Auth
            } else if lower.contains("billing")
                || lower.contains("payment")
                || lower.contains("quota")
                || lower.contains("credit")
            {
                ErrorCategory::Billing
            } else if lower.contains("500")
                || lower.contains("502")
                || lower.contains("503")
                || lower.contains("504")
                || lower.contains("server error")
                || lower.contains("overloaded")
            {
                ErrorCategory::Overloaded
            } else if lower.contains("400")
                || lower.contains("bad request")
                || lower.contains("invalid")
            {
                ErrorCategory::Format
            } else if lower.contains("timeout") {
                ErrorCategory::Timeout
            } else {
                ErrorCategory::Transient
            }
        }
        SavantError::IoError(_) => ErrorCategory::Transient,
        SavantError::Unknown(msg) => {
            let lower = msg.to_lowercase();
            if lower.contains("timeout") {
                ErrorCategory::Timeout
            } else if lower.contains("429") {
                ErrorCategory::RateLimit
            } else if lower.contains("500") || lower.contains("502") || lower.contains("503") {
                ErrorCategory::Overloaded
            } else {
                ErrorCategory::Transient
            }
        }
        _ => ErrorCategory::Transient,
    }
}

// ============================================================================
// 2. Cooldown Tracker
// ============================================================================

/// Tracks per-key cooldown with exponential backoff.
#[derive(Default)]
struct CooldownState {
    failure_count: u32,
    cooldown_start: Option<Instant>,
    resume_at: Option<Instant>,
}

/// Per-key cooldown tracker with exponential backoff.
pub struct CooldownTracker {
    states: tokio::sync::RwLock<HashMap<String, CooldownState>>,
}

impl CooldownTracker {
    pub fn new() -> Self {
        Self {
            states: tokio::sync::RwLock::new(HashMap::new()),
        }
    }

    /// Check if a key is currently on cooldown. Returns false if not.
    pub async fn is_on_cooldown(&self, key: &str) -> bool {
        let states = self.states.read().await;
        if let Some(state) = states.get(key) {
            if let Some(resume_at) = state.resume_at {
                return Instant::now() < resume_at;
            }
        }
        false
    }

    /// Record a failure and compute the cooldown duration.
    pub async fn record_failure(&self, key: &str, category: ErrorCategory) {
        self.record_failure_with_retry_after(key, category, None)
            .await;
    }

    /// Record a failure with an optional Retry-After hint from the server.
    /// PB-15: When the server sends a Retry-After header on 429, use that value
    /// instead of computing exponential backoff.
    pub async fn record_failure_with_retry_after(
        &self,
        key: &str,
        category: ErrorCategory,
        retry_after_secs: Option<u64>,
    ) {
        let mut states = self.states.write().await;
        let state = states.entry(key.to_string()).or_default();
        state.failure_count += 1;
        state.cooldown_start = Some(Instant::now());

        let duration = if let Some(seconds) = retry_after_secs {
            // PB-15: Use server-provided Retry-After value
            Duration::from_secs(seconds.min(3600)) // cap at 1 hour
        } else {
            match category {
                ErrorCategory::Billing => Self::billing_cooldown(state.failure_count),
                ErrorCategory::RateLimit => Self::standard_cooldown(state.failure_count),
                ErrorCategory::Overloaded => Self::standard_cooldown(state.failure_count),
                ErrorCategory::Auth => Duration::from_secs(300),
                _ => Duration::from_secs(30),
            }
        };

        state.resume_at = Some(Instant::now() + duration);

        tracing::warn!(
            "Cooldown: {} for {:?} (failures={}, duration={}s, retry_after={:?})",
            key,
            category,
            state.failure_count,
            duration.as_secs(),
            retry_after_secs
        );
    }

    /// Record a success — resets the failure counter.
    pub async fn record_success(&self, key: &str) {
        let mut states = self.states.write().await;
        if let Some(state) = states.get_mut(key) {
            state.failure_count = 0;
            state.cooldown_start = None;
            state.resume_at = None;
        }
    }

    /// Standard exponential backoff: min(1h, 1min * 5^min(n-1, 3))
    fn standard_cooldown(n: u32) -> Duration {
        let exponent = n.saturating_sub(1).min(3);
        let multiplier = 5u64.pow(exponent);
        let seconds = 60 * multiplier;
        Duration::from_secs(seconds.min(3600))
    }

    /// Billing exponential backoff: min(24h, 5h * 2^min(n-1, 10))
    fn billing_cooldown(n: u32) -> Duration {
        let exponent = n.saturating_sub(1).min(10);
        let multiplier = 2u64.pow(exponent);
        let seconds = 5 * 3600 * multiplier;
        Duration::from_secs(seconds.min(86400))
    }
}

impl Default for CooldownTracker {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================================
// 3. Circuit Breaker
// ============================================================================

/// Circuit breaker state machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BreakerState {
    Closed,
    Open,
    HalfOpen,
}

/// Circuit breaker that stops hitting dead providers.
/// All state is under a single lock to prevent race conditions.
pub struct CircuitBreaker {
    inner: tokio::sync::RwLock<CircuitBreakerInner>,
    failure_threshold: u32,
    open_duration: Duration,
}

struct CircuitBreakerInner {
    state: BreakerState,
    failure_count: u32,
    last_opened: Option<Instant>,
}

/// D7: Serializable circuit breaker state for persistence.
#[derive(serde::Serialize, serde::Deserialize)]
struct CircuitBreakerPersisted {
    state: String,
    failure_count: u32,
    last_opened_epoch: Option<u64>,
}

impl CircuitBreaker {
    pub fn new(failure_threshold: u32, open_duration: Duration) -> Self {
        Self {
            inner: tokio::sync::RwLock::new(CircuitBreakerInner {
                state: BreakerState::Closed,
                failure_count: 0,
                last_opened: None,
            }),
            failure_threshold,
            open_duration,
        }
    }

    /// D7: Save circuit breaker state to a JSON file for crash recovery.
    pub async fn save_to_file(&self, path: &std::path::Path) -> Result<(), String> {
        let inner = self.inner.read().await;
        let state = CircuitBreakerPersisted {
            state: match inner.state {
                BreakerState::Closed => "closed",
                BreakerState::Open => "open",
                BreakerState::HalfOpen => "half_open",
            }
            .to_string(),
            failure_count: inner.failure_count,
            last_opened_epoch: inner.last_opened.map(|t| t.elapsed().as_secs()), // seconds since opened
        };
        let json = serde_json::to_string_pretty(&state).map_err(|e| e.to_string())?;
        std::fs::write(path, json).map_err(|e| e.to_string())?;
        Ok(())
    }

    /// D7: Load circuit breaker state from a JSON file.
    pub async fn load_from_file(&self, path: &std::path::Path) -> Result<(), String> {
        if !path.exists() {
            return Ok(());
        }
        let json = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
        let persisted: CircuitBreakerPersisted =
            serde_json::from_str(&json).map_err(|e| e.to_string())?;
        let mut inner = self.inner.write().await;
        inner.state = match persisted.state.as_str() {
            "open" => BreakerState::Open,
            "half_open" => BreakerState::HalfOpen,
            _ => BreakerState::Closed,
        };
        inner.failure_count = persisted.failure_count;
        if persisted.state == "open" {
            inner.last_opened = Some(Instant::now()); // approximate
        }
        Ok(())
    }

    /// Check if a request is allowed through the breaker.
    pub async fn is_allowed(&self) -> bool {
        let inner = self.inner.read().await;
        match inner.state {
            BreakerState::Closed | BreakerState::HalfOpen => true,
            BreakerState::Open => {
                if let Some(opened) = inner.last_opened {
                    opened.elapsed() >= self.open_duration
                } else {
                    false
                }
            }
        }
    }

    /// Transition Open → HalfOpen if cooldown elapsed. Call before attempting a request.
    pub async fn maybe_transition_to_half_open(&self) {
        let mut inner = self.inner.write().await;
        if inner.state == BreakerState::Open {
            if let Some(opened) = inner.last_opened {
                if opened.elapsed() >= self.open_duration {
                    inner.state = BreakerState::HalfOpen;
                    tracing::info!("Circuit breaker: Open → HalfOpen (cooldown elapsed)");
                }
            }
        }
    }

    /// Record a successful call.
    pub async fn record_success(&self) {
        let mut inner = self.inner.write().await;
        if inner.state == BreakerState::HalfOpen {
            tracing::info!("Circuit breaker: recovered (HalfOpen → Closed)");
        }
        inner.failure_count = 0;
        inner.state = BreakerState::Closed;
    }

    /// Record a failed call.
    pub async fn record_failure(&self) {
        let mut inner = self.inner.write().await;
        inner.failure_count += 1;
        match inner.state {
            BreakerState::Closed => {
                if inner.failure_count >= self.failure_threshold {
                    inner.state = BreakerState::Open;
                    inner.last_opened = Some(Instant::now());
                    tracing::warn!(
                        "Circuit breaker: OPEN after {} consecutive failures",
                        inner.failure_count
                    );
                }
            }
            BreakerState::HalfOpen => {
                inner.state = BreakerState::Open;
                inner.last_opened = Some(Instant::now());
                tracing::warn!("Circuit breaker: probe failed, reopening");
            }
            BreakerState::Open => {}
        }
    }

    /// Get current state.
    pub async fn current_state(&self) -> BreakerState {
        self.inner.read().await.state
    }
}

// ============================================================================
// 4. Response Cache
// ============================================================================

struct CacheEntry {
    chunks: Vec<ChatChunk>,
    inserted_at: Instant,
}

/// SHA-256 keyed LRU response cache.
pub struct ResponseCache {
    entries: tokio::sync::RwLock<HashMap<String, CacheEntry>>,
    ttl: Duration,
    max_size: usize,
}

impl ResponseCache {
    pub fn new(ttl: Duration, max_size: usize) -> Self {
        Self {
            entries: tokio::sync::RwLock::new(HashMap::with_capacity(max_size)),
            ttl,
            max_size,
        }
    }

    /// Generate a cache key from messages (SHA-256 of content).
    fn cache_key(messages: &[ChatMessage]) -> String {
        let mut hasher = Sha256::new();
        for msg in messages {
            hasher.update(format!("{:?}:{}", msg.role, msg.content).as_bytes());
        }
        format!("{:x}", hasher.finalize())
    }

    /// Try to get a cached response. Returns None on miss or expiry.
    pub async fn get(&self, messages: &[ChatMessage]) -> Option<Vec<ChatChunk>> {
        let key = Self::cache_key(messages);
        let entries = self.entries.read().await;
        if let Some(entry) = entries.get(&key) {
            if entry.inserted_at.elapsed() < self.ttl {
                tracing::debug!("Cache hit for key {}", &key[..8.min(key.len())]);
                return Some(entry.chunks.clone());
            }
        }
        None
    }

    /// Store a response in the cache. Skips if response contains tool calls.
    pub async fn put(&self, messages: &[ChatMessage], chunks: &[ChatChunk]) {
        let has_tool_calls = chunks.iter().any(|c| c.tool_calls.is_some());
        if has_tool_calls {
            return;
        }

        let key = Self::cache_key(messages);
        let mut entries = self.entries.write().await;

        // Evict oldest if at capacity
        if entries.len() >= self.max_size {
            if let Some(oldest_key) = entries
                .iter()
                .min_by_key(|(_, e)| e.inserted_at)
                .map(|(k, _)| k.clone())
            {
                entries.remove(&oldest_key);
            }
        }

        entries.insert(
            key,
            CacheEntry {
                chunks: chunks.to_vec(),
                inserted_at: Instant::now(),
            },
        );
    }
}

// ============================================================================
// 5. Provider Chain — combines all 4 layers
// ============================================================================

/// Configuration for the provider chain.
pub struct ChainConfig {
    pub max_retries: u32,
    pub failure_threshold: u32,
    pub open_duration: Duration,
    pub cache_ttl: Duration,
    pub cache_max_size: usize,
    /// Timeout for individual provider calls. Prevents unbounded waits.
    pub call_timeout: Duration,
}

impl Default for ChainConfig {
    fn default() -> Self {
        Self {
            max_retries: 3,
            failure_threshold: 5,
            open_duration: Duration::from_secs(60),
            cache_ttl: Duration::from_secs(300),
            cache_max_size: 256,
            call_timeout: Duration::from_secs(120),
        }
    }
}

impl ChainConfig {
    /// Returns a configuration calibrated for free-tier API limits (e.g., OpenRouter free tier: 20 RPM, 200 RPD).
    ///
    /// Differences from standard:
    /// - `max_retries`: 2 (one less retry to conserve rate-limit budget)
    /// - `open_duration`: 30s (faster circuit breaker recovery — free limits reset quickly)
    pub fn free_tier() -> Self {
        Self {
            max_retries: 2,
            failure_threshold: 5,
            open_duration: Duration::from_secs(30),
            cache_ttl: Duration::from_secs(300),
            cache_max_size: 256,
            call_timeout: Duration::from_secs(120),
        }
    }
}

/// Provider chain combining error classification, cooldown, circuit breaker, and response cache.
pub struct ProviderChain {
    inner: Arc<dyn LlmProvider>,
    /// Optional fallback provider — tried when primary exhausts retries.
    fallback: Option<Arc<dyn LlmProvider>>,
    cooldown: CooldownTracker,
    breaker: CircuitBreaker,
    cache: ResponseCache,
    max_retries: u32,
    call_timeout: Duration,
    chain_key: String,
    privacy_router: Option<PrivacyRouter>,
    /// Local provider for privacy-routed requests.
    local_provider: Option<Box<dyn LlmProvider>>,
    /// Rate limiter to prevent runaway token usage.
    rate_limiter: Option<crate::rate_limiter::RateLimiter>,
}

impl ProviderChain {
    pub fn new(inner: Arc<dyn LlmProvider>, chain_key: String, config: ChainConfig) -> Self {
        // NA-03: Log available free models at chain initialization
        let free_models = crate::free_model_router::FreeModelRouter::dashboard_model_list();
        tracing::info!(
            chain_key = %chain_key,
            free_model_count = free_models.len(),
            free_models = ?free_models.iter().map(|m| m.name.as_str()).collect::<Vec<_>>(),
            "Provider chain initialized with free model fallbacks"
        );
        Self {
            inner,
            fallback: None,
            cooldown: CooldownTracker::new(),
            breaker: CircuitBreaker::new(config.failure_threshold, config.open_duration),
            cache: ResponseCache::new(config.cache_ttl, config.cache_max_size),
            max_retries: config.max_retries,
            call_timeout: config.call_timeout,
            chain_key,
            privacy_router: None,
            local_provider: None,
            rate_limiter: None,
        }
    }

    /// Sets a fallback provider to use when the primary exhausts retries.
    pub fn with_fallback(mut self, fallback: Arc<dyn LlmProvider>) -> Self {
        self.fallback = Some(fallback);
        self
    }

    /// Sets a rate limiter for the provider chain.
    pub fn with_rate_limiter(mut self, limiter: crate::rate_limiter::RateLimiter) -> Self {
        self.rate_limiter = Some(limiter);
        self
    }

    /// Attach a privacy router and optional local provider for sensitive content routing.
    pub fn with_privacy(
        mut self,
        privacy_config: PrivacyConfig,
        local_provider: Option<Box<dyn LlmProvider>>,
    ) -> Self {
        let router = PrivacyRouter::new(privacy_config);
        // NA-03: Log privacy router configuration on attachment
        let cfg = router.config();
        tracing::info!(
            enabled = cfg.enabled,
            threshold = cfg.sensitivity_threshold,
            local_models = ?cfg.local_models,
            cloud_models = ?cfg.cloud_models,
            "Privacy router attached"
        );
        self.privacy_router = Some(router);
        self.local_provider = local_provider;
        self
    }

    fn is_retryable(category: ErrorCategory) -> bool {
        matches!(
            category,
            ErrorCategory::RateLimit
                | ErrorCategory::Overloaded
                | ErrorCategory::Timeout
                | ErrorCategory::Transient
        )
    }
}

#[async_trait::async_trait]
impl LlmProvider for ProviderChain {
    async fn stream_completion(
        &self,
        messages: Vec<ChatMessage>,
        tools: Vec<serde_json::Value>,
    ) -> Result<
        Pin<Box<dyn Stream<Item = Result<ChatChunk, savant_core::error::SavantError>> + Send>>,
        savant_core::error::SavantError,
    > {
        // 0. Privacy routing — scan messages for PII before any cloud call
        if let Some(ref privacy) = self.privacy_router {
            let decision = privacy.route(&messages);
            match decision {
                RoutingDecision::Local {
                    model,
                    reason,
                    score,
                } => {
                    tracing::info!(
                        "[{}] Privacy routing: LOCAL (model={}, score={:.2}, reason={})",
                        self.chain_key,
                        model,
                        score,
                        reason
                    );
                    // Use local provider if available, otherwise fall through to cloud
                    if let Some(ref local) = self.local_provider {
                        return local.stream_completion(messages, tools).await;
                    }
                    tracing::warn!("[{}] Privacy router chose local but no local provider configured — falling through to cloud", self.chain_key);
                }
                RoutingDecision::UserChoice {
                    local: _,
                    cloud: _,
                    reason,
                    score,
                } => {
                    // Default safe choice: use local if available
                    tracing::info!(
                        "[{}] Privacy routing: USER_CHOICE (score={:.2}, reason={}) — defaulting to local",
                        self.chain_key, score, reason
                    );
                    if let Some(ref local) = self.local_provider {
                        return local.stream_completion(messages, tools).await;
                    }
                }
                RoutingDecision::Cloud { reason, score, .. } => {
                    tracing::debug!(
                        "[{}] Privacy routing: CLOUD (score={:.2}, reason={})",
                        self.chain_key,
                        score,
                        reason
                    );
                }
            }
        }

        // 1. Check cache (only for non-tool requests)
        if tools.is_empty() {
            if let Some(cached) = self.cache.get(&messages).await {
                tracing::debug!("[{}] Returning cached response", self.chain_key);
                return Ok(Box::pin(futures::stream::iter(cached.into_iter().map(Ok))));
            }
        }

        // 2. Check circuit breaker — attempt Open→HalfOpen transition first
        self.breaker.maybe_transition_to_half_open().await;
        if !self.breaker.is_allowed().await {
            return Err(SavantError::Unknown(format!(
                "[{}] Circuit breaker is OPEN — provider temporarily unavailable",
                self.chain_key
            )));
        }

        // 2b. Check rate limiter
        if let Some(ref limiter) = self.rate_limiter {
            let estimated_tokens: u32 = messages
                .iter()
                .map(|m| (m.content.chars().count() / 4) as u32)
                .sum();
            if let Err(wait_ms) = limiter.check(estimated_tokens).await {
                return Err(SavantError::RateLimit(format!(
                    "[{}] Rate limit exceeded — wait {}ms",
                    self.chain_key, wait_ms
                )));
            }
        }

        // 3. Check cooldown — use separate key to prevent cross-agent cooldown sharing
        let cooldown_key = format!("{}:cooldown", self.chain_key);
        if self.cooldown.is_on_cooldown(&cooldown_key).await {
            return Err(SavantError::Unknown(format!(
                "[{}] Provider is on cooldown — try again later",
                self.chain_key
            )));
        }

        // 4. Attempt with retry
        let mut attempts = 0u32;
        let mut last_error = SavantError::Unknown("Chain exhausted".to_string());

        while attempts < self.max_retries {
            let call_result = tokio::time::timeout(
                self.call_timeout,
                self.inner
                    .stream_completion(messages.clone(), tools.clone()),
            )
            .await;

            match call_result {
                Ok(Ok(stream)) => {
                    // True streaming: yield chunks directly as they arrive from provider.
                    // Cache is not written-through (trade-off: cache miss on exact duplicate
                    // requests, but TTFT is minimized).
                    self.breaker.record_success().await;
                    let cooldown_key = format!("{}:cooldown", self.chain_key);
                    self.cooldown.record_success(&cooldown_key).await;

                    return Ok(stream);
                }
                Ok(Err(e)) => {
                    let category = classify_error(&e);
                    attempts += 1;

                    tracing::warn!(
                        "[{}] Attempt {} failed: {:?} ({})",
                        self.chain_key,
                        attempts,
                        category,
                        e
                    );

                    // PB-15: Extract retry-after from rate limit errors
                    let retry_after_secs = extract_retry_after_from_error(&e);
                    let cooldown_key = format!("{}:cooldown", self.chain_key);
                    self.cooldown
                        .record_failure_with_retry_after(&cooldown_key, category, retry_after_secs)
                        .await;
                    self.breaker.record_failure().await;

                    // Log circuit breaker state for diagnostics
                    let breaker_state = self.breaker.current_state().await;
                    if breaker_state == BreakerState::Open {
                        tracing::warn!(
                            "[{}] Circuit breaker OPEN after {} attempts",
                            self.chain_key,
                            attempts
                        );
                    }

                    if !Self::is_retryable(category) {
                        return Err(e);
                    }

                    last_error = e;

                    // Exponential backoff: 500ms * 2^attempt
                    let delay = Duration::from_millis(500 * 2u64.pow(attempts - 1));
                    tokio::time::sleep(delay).await;
                }
                Err(_elapsed) => {
                    attempts += 1;
                    let timeout_err = SavantError::Unknown(format!(
                        "[{}] Provider call timed out after {:?} (attempt {})",
                        self.chain_key, self.call_timeout, attempts
                    ));

                    tracing::warn!(
                        "[{}] Provider call timed out after {:?} (attempt {})",
                        self.chain_key,
                        self.call_timeout,
                        attempts
                    );

                    self.breaker.record_failure().await;
                    last_error = timeout_err;

                    // Exponential backoff: 500ms * 2^attempt
                    let delay = Duration::from_millis(500 * 2u64.pow(attempts - 1));
                    tokio::time::sleep(delay).await;
                }
            }
        }

        // Fallback provider — try when primary is exhausted
        if let Some(ref fallback_provider) = self.fallback {
            tracing::info!(
                "[{}] Primary provider exhausted — trying fallback provider",
                self.chain_key,
            );
            match fallback_provider.stream_completion(messages, tools).await {
                Ok(stream) => {
                    tracing::info!("[{}] Fallback provider succeeded", self.chain_key,);
                    return Ok(stream);
                }
                Err(e) => {
                    tracing::warn!("[{}] Fallback provider also failed: {}", self.chain_key, e,);
                    // Return the primary's error, not the fallback's
                }
            }
        }

        // All providers exhausted — return user-friendly error
        tracing::error!(
            "[{}] All providers exhausted after {} retries. Fallback also failed.",
            self.chain_key,
            self.max_retries,
        );
        Err(SavantError::Unknown(format!(
            "All LLM providers are currently unavailable (tried {} times, fallback failed). \
             The provider may be experiencing high load or an outage. \
             Please try again in a moment. Original error: {}",
            self.max_retries, last_error
        )))
    }

    fn context_window(&self) -> Option<usize> {
        self.inner.context_window()
    }

    fn supports_multimodal(&self) -> bool {
        self.inner.supports_multimodal()
    }
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)]
mod tests {
    use super::*;
    use savant_core::types::{ChatMessage, ChatRole};
    use std::pin::Pin;

    /// Mock provider that always fails.
    struct FailingProvider;
    #[async_trait::async_trait]
    impl LlmProvider for FailingProvider {
        async fn stream_completion(
            &self,
            _messages: Vec<ChatMessage>,
            _tools: Vec<serde_json::Value>,
        ) -> Result<Pin<Box<dyn Stream<Item = Result<ChatChunk, SavantError>> + Send>>, SavantError>
        {
            Err(SavantError::Unknown("mock provider failed".to_string()))
        }
        fn context_window(&self) -> Option<usize> {
            Some(4096)
        }
    }

    /// Mock provider that succeeds with a simple response.
    struct SuccessProvider;
    #[async_trait::async_trait]
    impl LlmProvider for SuccessProvider {
        async fn stream_completion(
            &self,
            _messages: Vec<ChatMessage>,
            _tools: Vec<serde_json::Value>,
        ) -> Result<Pin<Box<dyn Stream<Item = Result<ChatChunk, SavantError>> + Send>>, SavantError>
        {
            let chunk = ChatChunk {
                agent_name: "test".to_string(),
                agent_id: "test".to_string(),
                content: "hello".to_string(),
                is_final: true,
                session_id: None,
                channel: savant_core::types::AgentOutputChannel::Chat,
                logprob: None,
                is_telemetry: false,
                reasoning: None,
                tool_calls: None,
            };
            Ok(Box::pin(futures::stream::iter(vec![Ok(chunk)])))
        }
        fn context_window(&self) -> Option<usize> {
            Some(4096)
        }
    }

    fn test_message(content: &str) -> ChatMessage {
        ChatMessage {
            role: ChatRole::User,
            content: content.to_string(),
            sender: None,
            recipient: None,
            agent_id: None,
            session_id: None,
            channel: savant_core::types::AgentOutputChannel::Chat,
            is_telemetry: false,
            images: Vec::new(),
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn test_fallback_on_primary_failure() {
        let mut chain = ProviderChain::new(
            Arc::new(FailingProvider),
            "test".to_string(),
            ChainConfig::default(),
        );
        chain = chain.with_fallback(Arc::new(SuccessProvider));

        let result = chain
            .stream_completion(vec![test_message("hello")], vec![])
            .await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_circuit_breaker_opens_after_threshold() {
        let breaker = CircuitBreaker::new(2, Duration::from_secs(60));

        // Record 2 failures — should open
        breaker.record_failure().await;
        assert_eq!(breaker.current_state().await, BreakerState::Closed);

        breaker.record_failure().await;
        assert_eq!(breaker.current_state().await, BreakerState::Open);
    }

    #[tokio::test]
    async fn test_circuit_breaker_half_open_after_cooldown() {
        let breaker = CircuitBreaker::new(1, Duration::from_millis(50));

        breaker.record_failure().await;
        assert_eq!(breaker.current_state().await, BreakerState::Open);

        // Wait for cooldown
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Maybe transition
        breaker.maybe_transition_to_half_open().await;
        assert_eq!(breaker.current_state().await, BreakerState::HalfOpen);
    }

    #[tokio::test]
    async fn test_circuit_breaker_success_resets() {
        let breaker = CircuitBreaker::new(2, Duration::from_secs(60));

        breaker.record_failure().await;
        breaker.record_failure().await;
        assert_eq!(breaker.current_state().await, BreakerState::Open);

        breaker.record_success().await;
        assert_eq!(breaker.current_state().await, BreakerState::Closed);
    }

    #[tokio::test]
    async fn test_successful_call_records_success() {
        let chain = ProviderChain::new(
            Arc::new(SuccessProvider),
            "test-success".to_string(),
            ChainConfig::default(),
        );

        let result = chain
            .stream_completion(vec![test_message("hello")], vec![])
            .await;
        assert!(result.is_ok());

        let state = chain.breaker.current_state().await;
        assert_eq!(state, BreakerState::Closed);
    }

    #[test]
    fn test_chain_config_defaults() {
        let config = ChainConfig::default();
        assert_eq!(config.max_retries, 3);
        assert_eq!(config.failure_threshold, 5);
        assert_eq!(config.call_timeout, Duration::from_secs(120));
    }
}
