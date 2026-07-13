//! Agent-side rate limiting with sliding window.
//!
//! Prevents runaway loops from burning through tokens before the
//! provider's own rate limits kick in. Tracks requests/min and
//! tokens/min per session.

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::Mutex;

/// Configuration for the rate limiter.
#[derive(Debug, Clone)]
pub struct RateLimiterConfig {
    /// Maximum requests per minute. 0 = unlimited.
    pub requests_per_min: u32,
    /// Maximum tokens per minute. 0 = unlimited.
    pub tokens_per_min: u32,
    /// Window duration in seconds.
    pub window_secs: u64,
}

impl Default for RateLimiterConfig {
    fn default() -> Self {
        Self {
            requests_per_min: 60,
            tokens_per_min: 500_000,
            window_secs: 60,
        }
    }
}

/// A single request record in the sliding window.
struct RequestRecord {
    timestamp: Instant,
    tokens: u32,
}

/// Agent-side rate limiter with sliding window.
///
/// Tracks request count and token count within a rolling time window.
/// Returns an error with wait duration when limits are exceeded.
pub struct RateLimiter {
    config: RateLimiterConfig,
    records: Arc<Mutex<VecDeque<RequestRecord>>>,
}

impl RateLimiter {
    pub fn new(config: RateLimiterConfig) -> Self {
        Self {
            config,
            records: Arc::new(Mutex::new(VecDeque::new())),
        }
    }

    /// Check if a request with the given token count is allowed.
    ///
    /// Returns `Ok(())` if allowed, or `Err(wait_ms)` if rate limited.
    /// `wait_ms` is how long to wait before retrying.
    pub async fn check(&self, estimated_tokens: u32) -> Result<(), u64> {
        if self.config.requests_per_min == 0 && self.config.tokens_per_min == 0 {
            return Ok(());
        }

        let now = Instant::now();
        let window = std::time::Duration::from_secs(self.config.window_secs);
        let mut records = self.records.lock().await;

        // Evict expired entries
        while let Some(front) = records.front() {
            if now.duration_since(front.timestamp) > window {
                records.pop_front();
            } else {
                break;
            }
        }

        // Check request count limit
        if self.config.requests_per_min > 0 {
            let request_count = records.len() as u32;
            if request_count >= self.config.requests_per_min {
                if let Some(oldest) = records.front() {
                    let wait = window
                        .checked_sub(now.duration_since(oldest.timestamp))
                        .unwrap_or_default();
                    return Err(wait.as_millis() as u64);
                }
            }
        }

        // Check token count limit
        if self.config.tokens_per_min > 0 {
            let token_sum: u32 = records.iter().map(|r| r.tokens).sum();
            if token_sum + estimated_tokens > self.config.tokens_per_min {
                if let Some(oldest) = records.front() {
                    let wait = window
                        .checked_sub(now.duration_since(oldest.timestamp))
                        .unwrap_or_default();
                    return Err(wait.as_millis() as u64);
                }
            }
        }

        // Record this request
        records.push_back(RequestRecord {
            timestamp: now,
            tokens: estimated_tokens,
        });

        Ok(())
    }

    /// Get current stats for logging/debugging.
    pub async fn stats(&self) -> (u32, u32) {
        let now = Instant::now();
        let window = std::time::Duration::from_secs(self.config.window_secs);
        let records = self.records.lock().await;
        let count = records
            .iter()
            .filter(|r| now.duration_since(r.timestamp) <= window)
            .count() as u32;
        let tokens: u32 = records
            .iter()
            .filter(|r| now.duration_since(r.timestamp) <= window)
            .map(|r| r.tokens)
            .sum();
        (count, tokens)
    }
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_rate_limiter_allows_within_limits() {
        let limiter = RateLimiter::new(RateLimiterConfig {
            requests_per_min: 10,
            tokens_per_min: 100_000,
            window_secs: 60,
        });
        // First request should be allowed
        assert!(limiter.check(1000).await.is_ok());
        assert!(limiter.check(2000).await.is_ok());
    }

    #[tokio::test]
    async fn test_rate_limiter_blocks_at_request_limit() {
        let limiter = RateLimiter::new(RateLimiterConfig {
            requests_per_min: 3,
            tokens_per_min: 0, // unlimited tokens
            window_secs: 60,
        });
        assert!(limiter.check(100).await.is_ok());
        assert!(limiter.check(100).await.is_ok());
        assert!(limiter.check(100).await.is_ok());
        // 4th request should be blocked
        assert!(limiter.check(100).await.is_err());
    }

    #[tokio::test]
    async fn test_rate_limiter_blocks_at_token_limit() {
        let limiter = RateLimiter::new(RateLimiterConfig {
            requests_per_min: 0, // unlimited requests
            tokens_per_min: 1000,
            window_secs: 60,
        });
        assert!(limiter.check(500).await.is_ok());
        assert!(limiter.check(400).await.is_ok());
        // Total is 900, adding 200 would exceed 1000
        assert!(limiter.check(200).await.is_err());
    }

    #[tokio::test]
    async fn test_rate_limiter_unlimited() {
        let limiter = RateLimiter::new(RateLimiterConfig {
            requests_per_min: 0,
            tokens_per_min: 0,
            window_secs: 60,
        });
        // Should always allow when both limits are 0
        for _ in 0..100 {
            assert!(limiter.check(1000).await.is_ok());
        }
    }

    #[tokio::test]
    async fn test_rate_limiter_stats() {
        let limiter = RateLimiter::new(RateLimiterConfig {
            requests_per_min: 60,
            tokens_per_min: 500_000,
            window_secs: 60,
        });
        limiter.check(1000).await.unwrap();
        limiter.check(2000).await.unwrap();
        let (count, tokens) = limiter.stats().await;
        assert_eq!(count, 2);
        assert_eq!(tokens, 3000);
    }
}
