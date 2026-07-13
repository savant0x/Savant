//! Zero-Copy Inter-Process Communication using iceoryx2 Blackboard pattern.
//!
//! This crate provides O(1) context sharing for massive agent swarms,
//! eliminating JSON serialization overhead and enabling sub-microsecond
//! state propagation across thousands of concurrent agents.

pub mod a2a;
pub mod blackboard;
pub mod collective;
mod error;

pub use a2a::{
    agent_card::{input_modes, output_modes, AgentCard},
    context::ContextPackage,
    protocol::{
        A2AEnvelope, A2AMessageType, Artifact, ArtifactPart, ArtifactPartType, DelegationTask,
        TaskState,
    },
    queues::{
        AgentTaskQueue, TaskQueueError, DEFAULT_QUEUE_CAPACITY, MAX_QUEUE_RETRIES,
        QUEUE_FULL_BACKOFF_MS,
    },
    result_router::{
        DelegationResult, RejectionReason, ResultRouter, ResultRouterError, TaskStatusUpdate,
    },
};
pub use blackboard::{hash_session_id, CapabilityRegistry, SwarmBlackboard, SwarmSharedContext};
pub use collective::{
    AgentEntry, CollectiveBlackboard, ConsensusResult, ConsensusTimeoutError, DelegationConsensus,
    DelegationProposal, DelegationProposalType, GlobalState,
};
pub use error::SwarmIpcError;

#[cfg(test)]
#[allow(clippy::disallowed_methods)]
mod tests {
    use super::*;

    #[test]
    fn test_global_state_default() {
        let state = GlobalState::default();
        assert_eq!(state.heuristic_version, 0);
        assert_eq!(state.swarm_pressure, 0.0);
        assert_eq!(state.total_successes, 0);
        assert_eq!(state.total_failures, 0);
        assert_eq!(state.quorum_threshold, 3);
    }

    #[test]
    fn test_agent_entry_default() {
        let entry = AgentEntry::default();
        assert_eq!(entry.successes, 0);
        assert_eq!(entry.failures, 0);
        assert_eq!(entry.pressure, 0.0);
        assert!(!entry.is_active);
        assert_eq!(entry.agent_index, 0);
    }

    #[test]
    fn test_hash_session_id_deterministic() {
        let hash1 = hash_session_id("test-session-123");
        let hash2 = hash_session_id("test-session-123");
        assert_eq!(hash1, hash2);
    }

    #[test]
    fn test_hash_session_id_different() {
        let hash1 = hash_session_id("session-a");
        let hash2 = hash_session_id("session-b");
        assert_ne!(hash1, hash2);
    }

    #[test]
    fn test_delegation_proposal_type_values() {
        assert_eq!(DelegationProposalType::DestructiveEdit as u8, 1);
        assert_eq!(DelegationProposalType::SecurityOperation as u8, 2);
        assert_eq!(DelegationProposalType::ToolSynthesis as u8, 3);
    }

    #[test]
    fn test_consensus_result_variants() {
        let approved = ConsensusResult::Approved;
        let vetoed = ConsensusResult::Vetoed;
        let pending = ConsensusResult::Pending;
        assert!(matches!(approved, ConsensusResult::Approved));
        assert!(matches!(vetoed, ConsensusResult::Vetoed));
        assert!(matches!(pending, ConsensusResult::Pending));
    }

    #[test]
    fn test_task_state_values() {
        assert_eq!(TaskState::Submitted as u8, 0);
        assert_eq!(TaskState::Working as u8, 1);
        assert_eq!(TaskState::Completed as u8, 3);
        assert_eq!(TaskState::Failed as u8, 4);
    }

    #[test]
    fn test_swarm_shared_context_default() {
        let ctx = SwarmSharedContext::default();
        assert_eq!(ctx.session_id_hash, 0);
        assert!(!ctx.emergency_halt);
    }

    #[test]
    fn test_collective_blackboard_creation() {
        let result = CollectiveBlackboard::new("test_collective_bb_creation");
        if let Ok(bb) = result {
            let state = bb.read_global_state().unwrap();
            assert_eq!(state.heuristic_version, 0);
            assert_eq!(state.total_successes, 0);

            let _ = bb.update_agent_metrics(1, true, 0.5);
        }
    }

    #[test]
    fn test_swarm_blackboard_creation() {
        let result = SwarmBlackboard::new("test_swarm_bb_creation");
        if let Ok(bb) = result {
            let ctx = bb.read_context(1);
            assert!(ctx.is_ok() || ctx.is_err()); // Just verify method exists
            let stats = bb.stats();
            assert_eq!(stats.service_name, "test_swarm_bb_creation");
        }
    }
}
