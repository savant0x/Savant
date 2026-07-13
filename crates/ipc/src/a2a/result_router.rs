//! Inter-agent result routing — typed result delivery from subagents to parents.
//!
//! Each agent that can receive delegated tasks gets a dedicated iceoryx2
//! request-response port for result delivery. The parent polls this port
//! for ArtifactDelivery events.

use rkyv::{Archive, Deserialize, Serialize};

/// Result of a delegation attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DelegationResult {
    /// Task was accepted and is being worked on.
    Accepted { agent_id: [u8; 32] },
    /// Task was rejected — agent unavailable or lacks skills.
    Rejected { reason: RejectionReason },
    /// Task timed out before completion.
    TimedOut { task_id: [u8; 16] },
    /// Task was canceled by the parent.
    Canceled { task_id: [u8; 16] },
}

/// Reason a delegation was rejected.
#[derive(Debug, Clone, PartialEq, Eq, Archive, Serialize, Deserialize)]
pub enum RejectionReason {
    AgentUnavailable,
    InsufficientSkills,
    QueueFull,
    MemoryEnclaveMismatch,
    DelegationDepthExceeded,
}

impl RejectionReason {
    pub fn as_str(&self) -> &'static str {
        match self {
            RejectionReason::AgentUnavailable => "agent_unavailable",
            RejectionReason::InsufficientSkills => "insufficient_skills",
            RejectionReason::QueueFull => "queue_full",
            RejectionReason::MemoryEnclaveMismatch => "memory_enclave_mismatch",
            RejectionReason::DelegationDepthExceeded => "delegation_depth_exceeded",
        }
    }
}

/// Status update published by a subagent during task execution.
#[repr(C)]
#[derive(Debug, Copy, Clone, PartialEq, Eq, Archive, Serialize, Deserialize)]
pub struct TaskStatusUpdate {
    pub task_id: [u8; 16],
    pub state: super::protocol::TaskState,
    pub progress_pct: u8,
    pub tokens_consumed: u32,
    pub elapsed_ms: u64,
    pub _padding: [u8; 3],
}

impl TaskStatusUpdate {
    pub fn new(task_id: [u8; 16], state: super::protocol::TaskState) -> Self {
        Self {
            task_id,
            state,
            progress_pct: 0,
            tokens_consumed: 0,
            elapsed_ms: 0,
            _padding: [0u8; 3],
        }
    }
}

/// Routes results from subagents back to their parent orchestrators.
///
/// Each agent that can be delegated to has a result queue. When a subagent
/// completes a task, it publishes an ArtifactDelivery to the parent's result queue.
pub struct ResultRouter {
    task_states: std::collections::HashMap<String, super::protocol::TaskState>,
    delegation_depth: std::collections::HashMap<String, u8>,
}

impl ResultRouter {
    pub fn new() -> Self {
        Self {
            task_states: std::collections::HashMap::new(),
            delegation_depth: std::collections::HashMap::new(),
        }
    }

    /// Registers a new delegated task for tracking.
    pub fn register_task(&mut self, task_id: &str, parent_depth: u8) {
        self.task_states
            .insert(task_id.to_string(), super::protocol::TaskState::Submitted);
        self.delegation_depth
            .insert(task_id.to_string(), parent_depth);
    }

    /// Updates the state of a tracked task.
    pub fn update_state(
        &mut self,
        task_id: &str,
        new_state: super::protocol::TaskState,
    ) -> Result<(), ResultRouterError> {
        let current = self
            .task_states
            .get(task_id)
            .ok_or_else(|| ResultRouterError::UnknownTask(task_id.to_string()))?;
        if !current.can_transition_to(new_state) {
            return Err(ResultRouterError::InvalidTransition {
                from: *current,
                to: new_state,
            });
        }
        self.task_states.insert(task_id.to_string(), new_state);
        Ok(())
    }

    /// Returns the current state of a tracked task.
    pub fn get_state(&self, task_id: &str) -> Option<super::protocol::TaskState> {
        self.task_states.get(task_id).copied()
    }

    /// Returns the delegation depth for a tracked task.
    pub fn get_depth(&self, task_id: &str) -> Option<u8> {
        self.delegation_depth.get(task_id).copied()
    }

    /// Checks if a task can be further delegated (depth < 20).
    pub fn can_delegate_further(&self, task_id: &str) -> bool {
        matches!(self.get_depth(task_id), Some(d) if d < 20)
    }

    /// Removes a completed/failed/canceled task from tracking.
    pub fn complete_task(&mut self, task_id: &str) {
        self.task_states.remove(task_id);
        self.delegation_depth.remove(task_id);
    }

    /// Returns all task IDs in a given state.
    pub fn tasks_in_state(&self, state: super::protocol::TaskState) -> Vec<&str> {
        self.task_states
            .iter()
            .filter(|(_, s)| **s == state)
            .map(|(id, _)| id.as_str())
            .collect()
    }

    /// Returns the number of actively tracked tasks.
    pub fn active_count(&self) -> usize {
        self.task_states.values().filter(|s| s.is_active()).count()
    }
}

impl Default for ResultRouter {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ResultRouterError {
    #[error("Unknown task: {0}")]
    UnknownTask(String),
    #[error("Invalid state transition from {from} to {to}")]
    InvalidTransition {
        from: super::protocol::TaskState,
        to: super::protocol::TaskState,
    },
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)]
mod tests {
    use super::super::protocol::TaskState;
    use super::*;

    #[test]
    fn test_result_router_register_and_track() {
        let mut router = ResultRouter::new();
        router.register_task("task-1", 0);
        assert_eq!(router.get_state("task-1"), Some(TaskState::Submitted));
        assert_eq!(router.get_depth("task-1"), Some(0));
    }

    #[test]
    fn test_result_router_state_transitions() {
        let mut router = ResultRouter::new();
        router.register_task("task-1", 0);
        router
            .update_state("task-1", TaskState::Working)
            .expect("update_state should succeed");
        assert_eq!(router.get_state("task-1"), Some(TaskState::Working));
        router
            .update_state("task-1", TaskState::Completed)
            .expect("update_state should succeed");
        assert_eq!(router.get_state("task-1"), Some(TaskState::Completed));
    }

    #[test]
    fn test_result_router_invalid_transition() {
        let mut router = ResultRouter::new();
        router.register_task("task-1", 0);
        let result = router.update_state("task-1", TaskState::Completed);
        assert!(result.is_err());
    }

    #[test]
    fn test_result_router_delegation_depth() {
        let mut router = ResultRouter::new();
        router.register_task("task-1", 20);
        assert!(!router.can_delegate_further("task-1"));
        router.register_task("task-2", 5);
        assert!(router.can_delegate_further("task-2"));
    }

    #[test]
    fn test_result_router_active_count() {
        let mut router = ResultRouter::new();
        router.register_task("task-1", 0);
        router.register_task("task-2", 0);
        router.register_task("task-3", 0);
        assert_eq!(router.active_count(), 0);
        router
            .update_state("task-1", TaskState::Working)
            .expect("update_state should succeed");
        router
            .update_state("task-2", TaskState::Working)
            .expect("update_state should succeed");
        assert_eq!(router.active_count(), 2);
    }

    #[test]
    fn test_rejection_reason_strings() {
        assert_eq!(
            RejectionReason::AgentUnavailable.as_str(),
            "agent_unavailable"
        );
        assert_eq!(RejectionReason::QueueFull.as_str(), "queue_full");
    }

    #[test]
    fn test_task_status_update_size() {
        assert_eq!(std::mem::size_of::<TaskStatusUpdate>(), 40);
    }
}
