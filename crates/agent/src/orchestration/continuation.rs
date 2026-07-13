//! Anti-Dwindle Continuation Engine
//!
//! This module implements OpenClaw's CONTINUE_WORK token pattern,
//! allowing agents to yield control back to the Tokio executor without
//! going inert between ReAct loop turns.
//!
//! The dwindle pattern issue: When an agent completes a ReAct loop turn,
//! it becomes completely inert until an external event wakes it up. This
//! causes background research agents to lose hours of productive time.
//!
//! This engine solves that by:
//! 1. Recognizing CONTINUE_WORK tokens in LLM responses
//! 2. Safely yielding the task (not blocking OS threads)
//! 3. Automatic rescheduling with exponential backoff
//! 4. Token budget enforcement to prevent infinite loops

use tokio::time::{sleep, Duration};
use tracing::{debug, info, warn};

/// Configuration for the continuation engine.
#[derive(Debug, Clone)]
pub struct ContinuationConfig {
    /// Default continuation delay in milliseconds if not specified
    pub default_delay_ms: u64,
    /// Maximum allowed delay (prevents excessively long sleeps)
    pub max_delay_ms: u64,
    /// Maximum number of continuations per agent session (safety guard)
    pub max_continuations: u32,
    /// Whether to use exponential backoff for repeated continuations
    pub use_exponential_backoff: bool,
}

impl Default for ContinuationConfig {
    fn default() -> Self {
        Self {
            default_delay_ms: 5000,
            max_delay_ms: 30000,
            max_continuations: 100,
            use_exponential_backoff: true,
        }
    }
}

/// The Continuation Engine manages task lifecycle and continuation logic.
///
/// It handles the parsing, scheduling, and execution of CONTINUE_WORK signals
/// to prevent agents from going inert between ReAct loop iterations.
pub struct ContinuationEngine {
    config: ContinuationConfig,
    continuation_count: std::collections::HashMap<String, u32>, // agent_id -> count
}

impl ContinuationEngine {
    /// Creates a new continuation engine with the given configuration.
    pub fn new(config: ContinuationConfig) -> Self {
        Self {
            config,
            continuation_count: std::collections::HashMap::new(),
        }
    }

    /// Parses a CONTINUE_WORK token from the LLM response.
    ///
    /// Format: `CONTINUE_WORK[:<delay_ms>]`
    /// - If delay is not specified, uses the default
    /// - Delay must be <= max_delay_ms
    ///
    /// # Arguments
    /// * `response` - The raw LLM response text
    ///
    /// # Returns
    /// * `Some(delay_ms)` if a CONTINUE_WORK token was found
    /// * `None` otherwise
    ///
    /// # Examples
    /// ```
    /// # use savant_agent::orchestration::continuation::ContinuationEngine;
    /// # let engine = ContinuationEngine::default();
    /// let resp = "I need to continue working. CONTINUE_WORK:2000";
    /// assert_eq!(engine.parse_delay(resp), Some(2000));
    ///
    /// let resp2 = "Just checking in. CONTINUE_WORK";
    /// assert_eq!(engine.parse_delay(resp2), Some(5000)); // default
    /// ```
    pub fn parse_delay(&self, response: &str) -> Option<u64> {
        // Look for CONTINUE_WORK token
        if !response.contains("CONTINUE_WORK") {
            return None;
        }

        // Try to extract delay after colon
        if let Some(idx) = response.find("CONTINUE_WORK:") {
            // Everything after "CONTINUE_WORK:" (14 chars)
            let delay_str = &response[idx + 14..];
            // Take only digits
            let digits: String = delay_str
                .chars()
                .take_while(|c| c.is_ascii_digit())
                .collect();
            if let Ok(delay) = digits.parse::<u64>() {
                return Some(delay.min(self.config.max_delay_ms));
            }
        }

        // No explicit delay, use default
        Some(self.config.default_delay_ms)
    }

    /// Checks if the response contains a continuation request.
    pub fn should_continue(&self, response: &str) -> bool {
        response.contains("CONTINUE_WORK")
    }

    /// Executes a continuation pause with proper yielding.
    ///
    /// This is the critical anti-dwindle mechanism: instead of blocking
    /// the OS thread, we use Tokio's cooperative yielding which allows
    /// thousands of agents to sleep efficiently while one active agent computes.
    ///
    /// # Arguments
    /// * `agent_id` - The agent's unique identifier
    /// * `delay_ms` - The delay in milliseconds
    ///
    /// # Returns
    /// * `Ok(())` after the sleep completes
    /// * `Err` if the maximum continuation limit is exceeded
    pub async fn yield_execution(
        &mut self,
        agent_id: &str,
        delay_ms: u64,
    ) -> Result<(), ContinuationError> {
        // Increment and check continuation count
        let count = self
            .continuation_count
            .entry(agent_id.to_string())
            .or_insert(0);
        *count += 1;

        if *count > self.config.max_continuations {
            return Err(ContinuationError::MaxContinuationsExceeded {
                agent_id: agent_id.to_string(),
                count: *count,
                limit: self.config.max_continuations,
            });
        }

        let actual_delay = if self.config.use_exponential_backoff {
            // Exponential backoff: delay = min(base * 2^(n-1), max)
            let backoff = self.config.default_delay_ms * 2u64.pow(*count - 1);
            backoff.min(self.config.max_delay_ms).min(delay_ms * 2)
        } else {
            delay_ms
        };

        info!(
            agent_id = %agent_id,
            continuation = *count,
            delay_ms = %actual_delay,
            "Agent yielding execution"
        );

        // Critical: use Tokio's sleep which yields the task instead of blocking the thread
        sleep(Duration::from_millis(actual_delay)).await;

        debug!(
            agent_id = %agent_id,
            "Agent resumed after continuation"
        );

        Ok(())
    }

    /// Resets continuation count for a specific agent.
    ///
    /// Call this when an agent completes its task or enters a new phase.
    pub fn reset_agent(&mut self, agent_id: &str) {
        self.continuation_count.remove(agent_id);
    }

    /// Returns the current continuation count for an agent.
    pub fn continuation_count(&self, agent_id: &str) -> u32 {
        self.continuation_count.get(agent_id).copied().unwrap_or(0)
    }

    /// Checks whether a delegated task has exceeded its deadline.
    ///
    /// Compares the current time against the `deadline_timestamp` from a
    /// `DelegationTask`. Returns `true` if the task has expired.
    ///
    /// # Arguments
    /// * `deadline_timestamp` — The deadline in epoch milliseconds (0 means no deadline)
    pub fn is_task_expired(deadline_timestamp: u64) -> bool {
        if deadline_timestamp == 0 {
            return false;
        }
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        now > deadline_timestamp
    }

    /// Executes a continuation pause with task timeout enforcement.
    ///
    /// Before yielding, checks if the task has expired. If expired, returns
    /// `ContinuationError::TaskExpired` instead of sleeping. Otherwise behaves
    /// identically to `yield_execution`.
    ///
    /// # Arguments
    /// * `agent_id` — The agent's unique identifier
    /// * `delay_ms` — The delay in milliseconds
    /// * `deadline_timestamp` — The task deadline in epoch milliseconds (0 = no deadline)
    pub async fn yield_execution_with_timeout(
        &mut self,
        agent_id: &str,
        delay_ms: u64,
        deadline_timestamp: u64,
    ) -> Result<(), ContinuationError> {
        if Self::is_task_expired(deadline_timestamp) {
            warn!(
                agent_id = %agent_id,
                deadline_timestamp = %deadline_timestamp,
                "Task expired — refusing continuation pause"
            );
            return Err(ContinuationError::TaskExpired {
                agent_id: agent_id.to_string(),
                deadline_ms: deadline_timestamp,
            });
        }
        self.yield_execution(agent_id, delay_ms).await
    }

    /// Clears all continuation tracking (useful for agent lifecycle).
    pub fn clear(&mut self) {
        self.continuation_count.clear();
    }
}

/// Errors that can occur during continuation handling.
#[derive(Debug, thiserror::Error)]
pub enum ContinuationError {
    #[error("Agent {agent_id} exceeded maximum continuation limit ({count}/{limit})")]
    MaxContinuationsExceeded {
        agent_id: String,
        count: u32,
        limit: u32,
    },

    #[error("Task expired for agent {agent_id} — deadline was {deadline_ms}ms")]
    TaskExpired { agent_id: String, deadline_ms: u64 },
}

impl Default for ContinuationEngine {
    fn default() -> Self {
        Self::new(ContinuationConfig::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_delay_explicit() {
        let engine = ContinuationEngine::default();
        let resp = "CONTINUE_WORK:2500";
        assert_eq!(engine.parse_delay(resp), Some(2500));
    }

    #[test]
    fn test_parse_delay_default() {
        let engine = ContinuationEngine::default();
        let resp = "I need to continue. CONTINUE_WORK";
        assert_eq!(engine.parse_delay(resp), Some(5000));
    }

    #[test]
    fn test_parse_delay_no_continuation() {
        let engine = ContinuationEngine::default();
        let resp = "Just a normal response";
        assert_eq!(engine.parse_delay(resp), None);
    }

    #[test]
    fn test_parse_delay_exceeds_max() {
        let engine = ContinuationEngine::new(ContinuationConfig {
            default_delay_ms: 5000,
            max_delay_ms: 10000,
            ..Default::default()
        });
        let resp = "CONTINUE_WORK:50000";
        assert_eq!(engine.parse_delay(resp), Some(10000)); // capped at max
    }

    #[test]
    fn test_should_continue() {
        let engine = ContinuationEngine::default();
        assert!(engine.should_continue("some text CONTINUE_WORK more text"));
        assert!(!engine.should_continue("no continuation here"));
    }
}
