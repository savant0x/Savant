//! A2A Protocol — Typed agent-to-agent communication layer.
//!
//! This module implements the core protocol types for inter-agent delegation:
//! - `A2AEnvelope`: The foundational message envelope for all agent communication
//! - `DelegationTask`: Structured task assignment (replaces text-based /subagents spawn)
//! - `TaskState`: Lifecycle state machine for delegated tasks
//! - `Artifact`: Structured result delivery from subagents
//! - `AgentCard`: Capability advertisement per agent
//!
//! All types are `#[repr(C)]` and rkyv-serialized for zero-copy shared memory IPC.

use rkyv::{Archive, Deserialize, Serialize};
use std::fmt;

/// Message type discriminator for A2A communication.
#[repr(u8)]
#[derive(Debug, Copy, Clone, PartialEq, Eq, Archive, Serialize, Deserialize)]
pub enum A2AMessageType {
    TaskDelegation = 0,
    StatusUpdate = 1,
    ArtifactDelivery = 2,
    Interruption = 3,
}

/// The foundational message envelope for all inter-agent communication.
///
/// Stored in iceoryx2 shared memory. The actual payload (Task, Status, Artifact)
/// is stored at a separate offset in the shared memory segment, referenced by
/// `payload_offset` and `payload_size`.
///
/// Size: 168 bytes
#[repr(C)]
#[derive(Debug, Copy, Clone, PartialEq, Eq, Archive, Serialize, Deserialize)]
pub struct A2AEnvelope {
    pub message_type: A2AMessageType,
    pub session_id_hash: u64,
    pub source_agent_id: [u8; 32],
    pub target_agent_id: [u8; 32],
    pub payload_offset: u32,
    pub payload_size: u32,
    pub cct_signature: [u8; 64],
    pub trace_id: [u8; 16],
}

impl A2AEnvelope {
    /// Creates a new A2A envelope.
    ///
    /// # Arguments
    /// * `message_type` — The type of A2A message
    /// * `session_id_hash` — The hashed session ID
    /// * `source` — The source agent ID (32 bytes)
    /// * `target` — The target agent ID (32 bytes)
    /// * `payload_offset` — Offset to payload in shared memory
    /// * `payload_size` — Size of the payload in bytes
    /// * `cct_signature` — The CCT signature (64 bytes)
    /// * `trace_id` — W3C TraceContext ID (16 bytes)
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        message_type: A2AMessageType,
        session_id_hash: u64,
        source: [u8; 32],
        target: [u8; 32],
        payload_offset: u32,
        payload_size: u32,
        cct_signature: [u8; 64],
        trace_id: [u8; 16],
    ) -> Self {
        Self {
            message_type,
            session_id_hash,
            source_agent_id: source,
            target_agent_id: target,
            payload_offset,
            payload_size,
            cct_signature,
            trace_id,
        }
    }

    /// Validates the envelope has non-zero target and non-empty payload.
    pub fn is_valid(&self) -> bool {
        self.target_agent_id != [0u8; 32] && self.payload_size > 0
    }
}

impl fmt::Display for A2AMessageType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            A2AMessageType::TaskDelegation => write!(f, "TaskDelegation"),
            A2AMessageType::StatusUpdate => write!(f, "StatusUpdate"),
            A2AMessageType::ArtifactDelivery => write!(f, "ArtifactDelivery"),
            A2AMessageType::Interruption => write!(f, "Interruption"),
        }
    }
}

/// Structured task assignment from orchestrator to subagent.
///
/// Replaces the fragile text-based `/subagents spawn <agentId> <task>` pattern.
/// Stored in iceoryx2 shared memory. The task description string is stored
/// separately in the shared memory payload segment, referenced by
/// `task_description_offset` and `task_description_len`.
///
/// Size: 224 bytes
#[repr(C)]
#[derive(Debug, Copy, Clone, PartialEq, Eq, Archive, Serialize, Deserialize)]
pub struct DelegationTask {
    pub task_id: [u8; 16],
    pub parent_session_hash: u64,
    pub parent_agent_id: [u8; 32],
    pub token_budget: u32,
    pub max_delegation_depth: u8,
    pub priority_level: u8,
    pub deadline_timestamp: u64,
    pub cct_token: [u8; 64],
    pub context_package_offset: u32,
    pub expected_schema_hash: [u8; 32],
    pub task_description_offset: u32,
    pub task_description_len: u16,
    pub requires_consensus: bool,
    pub speculative_copies: u8,
    pub result_queue_id: u64,
    pub memory_enclave_id: u64,
    pub trace_id: [u8; 16],
    pub _padding: [u8; 3],
}

impl DelegationTask {
    pub fn new(
        task_id: [u8; 16],
        parent_session_hash: u64,
        parent_agent_id: [u8; 32],
        token_budget: u32,
        cct_token: [u8; 64],
    ) -> Self {
        Self {
            task_id,
            parent_session_hash,
            parent_agent_id,
            token_budget,
            max_delegation_depth: 20,
            priority_level: 128,
            deadline_timestamp: 0,
            cct_token,
            context_package_offset: 0,
            expected_schema_hash: [0u8; 32],
            task_description_offset: 0,
            task_description_len: 0,
            requires_consensus: false,
            speculative_copies: 0,
            result_queue_id: 0,
            memory_enclave_id: 0,
            trace_id: [0u8; 16],
            _padding: [0u8; 3],
        }
    }

    /// Returns true if this task has expired based on deadline_timestamp.
    pub fn is_expired(&self) -> bool {
        if self.deadline_timestamp == 0 {
            return false;
        }
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        now > self.deadline_timestamp
    }

    /// Returns true if this task requires swarm consensus before execution.
    pub fn needs_consensus(&self) -> bool {
        self.requires_consensus
    }

    /// Returns true if this task should be executed speculatively across multiple agents.
    pub fn is_speculative(&self) -> bool {
        self.speculative_copies > 1
    }
}

/// Lifecycle state machine for delegated tasks.
///
/// Every state transition is journaled to CortexaDB WAL for crash recovery.
#[repr(u8)]
#[derive(Debug, Copy, Clone, PartialEq, Eq, Archive, Serialize, Deserialize)]
pub enum TaskState {
    Submitted = 0,
    Working = 1,
    InputRequired = 2,
    Completed = 3,
    Failed = 4,
    Canceled = 5,
}

impl TaskState {
    /// Returns true if this is a terminal state (no further transitions expected).
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            TaskState::Completed | TaskState::Failed | TaskState::Canceled
        )
    }

    /// Returns true if the task is actively being worked on.
    pub fn is_active(&self) -> bool {
        matches!(self, TaskState::Working | TaskState::InputRequired)
    }

    /// Validates that a state transition is legal.
    pub fn can_transition_to(self, next: TaskState) -> bool {
        matches!(
            (self, next),
            (TaskState::Submitted, TaskState::Working)
                | (TaskState::Submitted, TaskState::Canceled)
                | (TaskState::Working, TaskState::Completed)
                | (TaskState::Working, TaskState::Failed)
                | (TaskState::Working, TaskState::InputRequired)
                | (TaskState::Working, TaskState::Canceled)
                | (TaskState::InputRequired, TaskState::Working)
                | (TaskState::InputRequired, TaskState::Canceled)
                | (TaskState::InputRequired, TaskState::Failed)
        )
    }
}

impl fmt::Display for TaskState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TaskState::Submitted => write!(f, "submitted"),
            TaskState::Working => write!(f, "working"),
            TaskState::InputRequired => write!(f, "input-required"),
            TaskState::Completed => write!(f, "completed"),
            TaskState::Failed => write!(f, "failed"),
            TaskState::Canceled => write!(f, "canceled"),
        }
    }
}

/// Structured result delivered from subagent back to parent orchestrator.
///
/// Contains typed parts (text, JSON data, file references) that the parent
/// can consume without parsing raw LLM output.
#[repr(C)]
#[derive(Debug, Clone, PartialEq, Eq, Archive, Serialize, Deserialize)]
pub struct Artifact {
    pub task_id: [u8; 16],
    pub part_count: u8,
    pub _padding: [u8; 7],
    // Parts follow this struct in shared memory:
    // [ArtifactPart; part_count]
}

/// A single part of an artifact result.
#[repr(C)]
#[derive(Debug, Clone, PartialEq, Eq, Archive, Serialize, Deserialize)]
pub struct ArtifactPart {
    pub part_type: ArtifactPartType,
    pub data_offset: u32,
    pub data_len: u32,
}

/// Type of artifact part content.
#[repr(u8)]
#[derive(Debug, Copy, Clone, PartialEq, Eq, Archive, Serialize, Deserialize)]
pub enum ArtifactPartType {
    Text = 0,
    Json = 1,
    FileReference = 2,
}

impl Artifact {
    pub fn new(task_id: [u8; 16]) -> Self {
        Self {
            task_id,
            part_count: 0,
            _padding: [0u8; 7],
        }
    }
}

impl ArtifactPart {
    pub fn new(part_type: ArtifactPartType, data_offset: u32, data_len: u32) -> Self {
        Self {
            part_type,
            data_offset,
            data_len,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_a2a_envelope_size() {
        assert_eq!(std::mem::size_of::<A2AEnvelope>(), 168);
    }

    #[test]
    fn test_delegation_task_size() {
        assert_eq!(std::mem::size_of::<DelegationTask>(), 224);
    }

    #[test]
    fn test_task_state_transitions() {
        assert!(TaskState::Submitted.can_transition_to(TaskState::Working));
        assert!(TaskState::Working.can_transition_to(TaskState::Completed));
        assert!(TaskState::Working.can_transition_to(TaskState::Failed));
        assert!(TaskState::Working.can_transition_to(TaskState::InputRequired));
        assert!(TaskState::InputRequired.can_transition_to(TaskState::Working));
        assert!(!TaskState::Completed.can_transition_to(TaskState::Working));
        assert!(!TaskState::Failed.can_transition_to(TaskState::Completed));
    }

    #[test]
    fn test_task_state_terminal() {
        assert!(TaskState::Completed.is_terminal());
        assert!(TaskState::Failed.is_terminal());
        assert!(TaskState::Canceled.is_terminal());
        assert!(!TaskState::Working.is_terminal());
        assert!(!TaskState::Submitted.is_terminal());
    }

    #[test]
    fn test_delegation_task_expiry() {
        let mut task = DelegationTask::new([1u8; 16], 12345, [2u8; 32], 1000, [3u8; 64]);
        assert!(!task.is_expired());
        task.deadline_timestamp = 1; // Epoch + 1ms — definitely expired
        assert!(task.is_expired());
    }

    #[test]
    fn test_delegation_task_speculative() {
        let mut task = DelegationTask::new([1u8; 16], 12345, [2u8; 32], 1000, [3u8; 64]);
        assert!(!task.is_speculative());
        task.speculative_copies = 3;
        assert!(task.is_speculative());
    }

    #[test]
    fn test_envelope_validation() {
        let mut env = A2AEnvelope::new(
            A2AMessageType::TaskDelegation,
            12345,
            [1u8; 32],
            [2u8; 32],
            100,
            50,
            [3u8; 64],
            [4u8; 16],
        );
        assert!(env.is_valid());
        env.target_agent_id = [0u8; 32];
        assert!(!env.is_valid());
    }
}
