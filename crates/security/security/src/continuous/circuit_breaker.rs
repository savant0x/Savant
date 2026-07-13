//! Deterministic Circuit Breakers — Runaway loop prevention.
//!
//! Independent monitors (NOT LLM-based) that analyze execution graphs
//! and terminate runaway recursive cycles without relying on LLM judgment.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tracing::warn;

/// Task class determines circuit breaker thresholds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TaskClass {
    /// Standard task with default limits.
    Standard,
    /// Long-running task with relaxed limits.
    LongRunning,
}

/// Circuit breaker configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CircuitBreakerConfig {
    /// Maximum recursion depth before tripping.
    pub max_depth: usize,
    /// Maximum API calls per task before tripping.
    pub max_api_calls: usize,
    /// Maximum cost in USD per task before tripping.
    pub max_cost_usd: f32,
}

impl CircuitBreakerConfig {
    /// Returns the config for a given task class.
    pub fn for_class(class: TaskClass) -> Self {
        match class {
            TaskClass::Standard => Self {
                max_depth: 10,
                max_api_calls: 100,
                max_cost_usd: 5.0,
            },
            TaskClass::LongRunning => Self {
                max_depth: 50,
                max_api_calls: 1000,
                max_cost_usd: 50.0,
            },
        }
    }
}

impl Default for CircuitBreakerConfig {
    fn default() -> Self {
        Self::for_class(TaskClass::Standard)
    }
}

/// Record of a circuit breaker trip.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TripRecord {
    pub task_id: String,
    pub reason: String,
    pub timestamp: i64,
    pub recursion_depth: usize,
    pub api_call_count: usize,
    pub cumulative_cost_usd: f32,
}

/// Per-task execution tracker.
#[derive(Debug)]
struct TaskTracker {
    recursion_depth: AtomicU64,
    api_call_count: AtomicU64,
    cumulative_cost: Arc<RwLock<f32>>,
    config: CircuitBreakerConfig,
    start_time: Instant,
}

/// Deterministic circuit breaker.
///
/// Monitors task execution and trips when hard limits are exceeded.
/// Does NOT rely on LLM judgment — purely deterministic.
pub struct CircuitBreaker {
    tasks: Arc<RwLock<HashMap<String, TaskTracker>>>,
    trip_log: Arc<RwLock<Vec<TripRecord>>>,
}

impl CircuitBreaker {
    /// Creates a new circuit breaker.
    pub fn new() -> Self {
        Self {
            tasks: Arc::new(RwLock::new(HashMap::new())),
            trip_log: Arc::new(RwLock::new(Vec::new())),
        }
    }

    /// Registers a task with the circuit breaker.
    pub async fn register_task(&self, task_id: &str, class: TaskClass) {
        let config = CircuitBreakerConfig::for_class(class);
        let tracker = TaskTracker {
            recursion_depth: AtomicU64::new(0),
            api_call_count: AtomicU64::new(0),
            cumulative_cost: Arc::new(RwLock::new(0.0)),
            config,
            start_time: Instant::now(),
        };

        self.tasks
            .write()
            .await
            .insert(task_id.to_string(), tracker);
    }

    /// Unregisters a task (normal completion).
    pub async fn unregister_task(&self, task_id: &str) {
        self.tasks.write().await.remove(task_id);
    }

    /// Checks recursion depth and returns an error if limit exceeded.
    pub async fn check_recursion(
        &self,
        task_id: &str,
    ) -> Result<(), savant_core::error::SavantError> {
        let trip_reason = {
            let tasks = self.tasks.read().await;
            if let Some(tracker) = tasks.get(task_id) {
                // SEC-06: Atomic check-and-increment to prevent TOCTOU race
                let prev = tracker.recursion_depth.fetch_add(1, Ordering::AcqRel);
                if prev as usize >= tracker.config.max_depth {
                    Some(format!(
                        "Recursion depth {} exceeds limit {}",
                        prev, tracker.config.max_depth
                    ))
                } else {
                    None
                }
            } else {
                None
            }
        }; // read lock dropped here

        if let Some(reason) = trip_reason {
            self.record_trip(task_id, &reason).await;
            return Err(savant_core::error::SavantError::CircuitBreakerTripped(
                reason,
            ));
        }
        Ok(())
    }

    /// Decrements recursion depth (on return from recursive call).
    pub async fn pop_recursion(&self, task_id: &str) {
        let tasks = self.tasks.read().await;
        if let Some(tracker) = tasks.get(task_id) {
            tracker.recursion_depth.fetch_sub(1, Ordering::Relaxed);
        }
    }

    /// Checks API call count and returns an error if limit exceeded.
    pub async fn check_api_call(
        &self,
        task_id: &str,
    ) -> Result<(), savant_core::error::SavantError> {
        let trip_reason = {
            let tasks = self.tasks.read().await;
            if let Some(tracker) = tasks.get(task_id) {
                let count = tracker.api_call_count.load(Ordering::Relaxed);
                if count as usize >= tracker.config.max_api_calls {
                    Some(format!(
                        "API call count {} exceeds limit {}",
                        count, tracker.config.max_api_calls
                    ))
                } else {
                    tracker.api_call_count.fetch_add(1, Ordering::Relaxed);
                    None
                }
            } else {
                None
            }
        }; // read lock dropped here

        if let Some(reason) = trip_reason {
            self.record_trip(task_id, &reason).await;
            return Err(savant_core::error::SavantError::CircuitBreakerTripped(
                reason,
            ));
        }
        Ok(())
    }

    /// Adds cost and checks if cumulative cost exceeds limit.
    pub async fn add_cost(
        &self,
        task_id: &str,
        cost: f32,
    ) -> Result<(), savant_core::error::SavantError> {
        let trip_reason = {
            let tasks = self.tasks.read().await;
            if let Some(tracker) = tasks.get(task_id) {
                let mut cumulative = tracker.cumulative_cost.write().await;
                *cumulative += cost;
                if *cumulative >= tracker.config.max_cost_usd {
                    Some(format!(
                        "Cumulative cost ${:.2} exceeds limit ${:.2}",
                        *cumulative, tracker.config.max_cost_usd
                    ))
                } else {
                    None
                }
            } else {
                None
            }
        }; // locks dropped here

        if let Some(reason) = trip_reason {
            self.record_trip(task_id, &reason).await;
            return Err(savant_core::error::SavantError::CircuitBreakerTripped(
                reason,
            ));
        }
        Ok(())
    }

    /// Records a circuit breaker trip.
    async fn record_trip(&self, task_id: &str, reason: &str) {
        let tasks = self.tasks.read().await;
        let (depth, calls, cost) = if let Some(tracker) = tasks.get(task_id) {
            (
                tracker.recursion_depth.load(Ordering::Relaxed) as usize,
                tracker.api_call_count.load(Ordering::Relaxed) as usize,
                *tracker.cumulative_cost.read().await,
            )
        } else {
            (0, 0, 0.0)
        };

        let record = TripRecord {
            task_id: task_id.to_string(),
            reason: reason.to_string(),
            timestamp: chrono::Utc::now().timestamp(),
            recursion_depth: depth,
            api_call_count: calls,
            cumulative_cost_usd: cost,
        };

        warn!("[CircuitBreaker] TRIPPED for task {}: {}", task_id, reason);

        self.trip_log.write().await.push(record);
    }

    /// Returns all trip records (for audit).
    pub async fn trip_history(&self) -> Vec<TripRecord> {
        self.trip_log.read().await.clone()
    }

    /// Checks if a task has exceeded its execution timeout.
    ///
    /// Returns the elapsed duration if the task is still running,
    /// or an error if the task has exceeded a reasonable timeout.
    pub async fn check_timeout(
        &self,
        task_id: &str,
        max_duration: std::time::Duration,
    ) -> Result<std::time::Duration, savant_core::error::SavantError> {
        let tasks = self.tasks.read().await;
        if let Some(tracker) = tasks.get(task_id) {
            let elapsed = tracker.start_time.elapsed();
            if elapsed > max_duration {
                let reason = format!(
                    "Task execution time {:?} exceeds timeout {:?}",
                    elapsed, max_duration
                );
                self.record_trip(task_id, &reason).await;
                Err(savant_core::error::SavantError::CircuitBreakerTripped(
                    reason,
                ))
            } else {
                Ok(elapsed)
            }
        } else {
            Err(savant_core::error::SavantError::Unknown(format!(
                "Task not registered: {}",
                task_id
            )))
        }
    }
}

impl Default for CircuitBreaker {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_register_and_unregister() {
        let cb = CircuitBreaker::new();
        cb.register_task("task1", TaskClass::Standard).await;
        cb.unregister_task("task1").await;
        // No panic = success
    }

    #[tokio::test]
    async fn test_recursion_limit_trips() {
        let cb = CircuitBreaker::new();
        cb.register_task("task1", TaskClass::Standard).await;

        // Push to the limit
        for _ in 0..10 {
            let result = cb.check_recursion("task1").await;
            assert!(result.is_ok());
        }

        // Next one should trip
        let result = cb.check_recursion("task1").await;
        assert!(result.is_err());
        match result {
            Err(savant_core::error::SavantError::CircuitBreakerTripped(_)) => {}
            _ => panic!("Expected CircuitBreakerTripped"),
        }
    }

    #[tokio::test]
    async fn test_api_call_limit_trips() {
        let cb = CircuitBreaker::new();
        cb.register_task("task1", TaskClass::Standard).await;

        for _ in 0..100 {
            let _ = cb.check_api_call("task1").await;
        }

        let result = cb.check_api_call("task1").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_cost_limit_trips() {
        let cb = CircuitBreaker::new();
        cb.register_task("task1", TaskClass::Standard).await;

        let result = cb.add_cost("task1", 4.99).await;
        assert!(result.is_ok());

        let result = cb.add_cost("task1", 0.02).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_trip_history() {
        let cb = CircuitBreaker::new();
        cb.register_task("task1", TaskClass::Standard).await;

        for _ in 0..11 {
            let _ = cb.check_recursion("task1").await;
        }

        let history = cb.trip_history().await;
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].task_id, "task1");
    }

    #[test]
    fn test_long_running_class_relaxed() {
        let config = CircuitBreakerConfig::for_class(TaskClass::LongRunning);
        assert_eq!(config.max_depth, 50);
        assert_eq!(config.max_api_calls, 1000);
        assert_eq!(config.max_cost_usd, 50.0);
    }
}
