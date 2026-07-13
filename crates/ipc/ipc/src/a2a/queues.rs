//! A2A Task Queues — iceoryx2 request-response queues for inter-agent delegation.
//!
//! Each agent that can receive delegated tasks gets a dedicated iceoryx2
//! request-response port. The Orchestrator pushes DelegationTask structs to
//! the target's queue. The target agent pops tasks and executes them.
//!
//! Backpressure: if a queue is full, the push returns `QueueFull`. The
//! Orchestrator catches this and applies exponential backoff via the
//! ContinuationEngine, then tries the next best agent.

use crate::error::SwarmIpcError;

/// Bounded capacity for agent task queues.
pub const DEFAULT_QUEUE_CAPACITY: usize = 64;

/// Maximum number of retry attempts when a queue is full.
pub const MAX_QUEUE_RETRIES: u32 = 3;

/// Base backoff duration in milliseconds when queue is full.
pub const QUEUE_FULL_BACKOFF_MS: u64 = 100;

/// Handle to an agent's inbound task queue.
///
/// Wraps an in-memory bounded queue. Each agent that can receive
/// delegated tasks creates one of these at startup.
/// Can be replaced with iceoryx2 request-response port for zero-copy IPC.
pub struct AgentTaskQueue {
    service_name: String,
    capacity: usize,
    queue: tokio::sync::Mutex<std::collections::VecDeque<super::protocol::DelegationTask>>,
}

impl AgentTaskQueue {
    /// Creates a new task queue for the given agent.
    ///
    /// The `service_name` should be unique per agent (e.g., "savant_agent_{agent_id}").
    /// The queue supports up to `capacity` pending tasks.
    pub fn new(service_name: &str, capacity: usize) -> Result<Self, SwarmIpcError> {
        if service_name.is_empty() || service_name.len() > 255 {
            return Err(SwarmIpcError::InvalidServiceName(format!(
                "Service name must be 1-255 characters, got {}",
                service_name.len()
            )));
        }
        Ok(Self {
            service_name: service_name.to_string(),
            capacity,
            queue: tokio::sync::Mutex::new(std::collections::VecDeque::with_capacity(capacity)),
        })
    }

    /// Returns the service name for this queue.
    pub fn service_name(&self) -> &str {
        &self.service_name
    }

    /// Returns the queue capacity.
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Computes the backoff duration for a given retry attempt.
    /// Uses exponential backoff: base * 2^attempt
    pub fn backoff_for_retry(attempt: u32) -> u64 {
        QUEUE_FULL_BACKOFF_MS * 2u64.pow(attempt)
    }

    /// Pushes a task to the queue. Returns QueueFull if at capacity.
    pub async fn push(&self, task: super::protocol::DelegationTask) -> Result<(), TaskQueueError> {
        let mut queue = self.queue.lock().await;
        if queue.len() >= self.capacity {
            return Err(TaskQueueError::QueueFull {
                capacity: self.capacity,
            });
        }
        queue.push_back(task);
        Ok(())
    }

    /// Pops a task from the queue. Returns None if empty.
    pub async fn pop(&self) -> Option<super::protocol::DelegationTask> {
        let mut queue = self.queue.lock().await;
        queue.pop_front()
    }

    /// Returns the number of pending tasks.
    pub async fn len(&self) -> usize {
        let queue = self.queue.lock().await;
        queue.len()
    }

    /// Returns true if the queue is empty.
    pub async fn is_empty(&self) -> bool {
        let queue = self.queue.lock().await;
        queue.is_empty()
    }
}

/// Errors that can occur during task queue operations.
#[derive(Debug, thiserror::Error)]
pub enum TaskQueueError {
    #[error("Task queue is full for agent. Capacity: {capacity}")]
    QueueFull { capacity: usize },
    #[error("Task queue not found: {0}")]
    QueueNotFound(String),
    #[error("Failed to push task: {0}")]
    PushFailed(String),
    #[error("Failed to pop task: {0}")]
    PopFailed(String),
    #[error("IPC error: {0}")]
    IpcError(#[from] SwarmIpcError),
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)]
mod tests {
    use super::*;

    #[test]
    fn test_queue_creation() {
        let queue = AgentTaskQueue::new("test_agent", 64).expect("queue creation should succeed");
        assert_eq!(queue.service_name(), "test_agent");
        assert_eq!(queue.capacity(), 64);
    }

    #[test]
    fn test_queue_creation_invalid_name() {
        let result = AgentTaskQueue::new("", 64);
        assert!(result.is_err());
    }

    #[test]
    fn test_exponential_backoff() {
        assert_eq!(AgentTaskQueue::backoff_for_retry(0), 100);
        assert_eq!(AgentTaskQueue::backoff_for_retry(1), 200);
        assert_eq!(AgentTaskQueue::backoff_for_retry(2), 400);
        assert_eq!(AgentTaskQueue::backoff_for_retry(3), 800);
    }
}
