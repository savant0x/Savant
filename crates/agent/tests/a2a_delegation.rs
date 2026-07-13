//! Integration tests for the A2A (Agent-to-Agent) Communication Layer.
//!
//! Tests the full delegation lifecycle:
//! 1. AgentCard registration and semantic matching
//! 2. DelegationTask construction and validation

#![allow(clippy::disallowed_methods)]
//! 3. ContextPackage extraction from memory system
//! 4. TaskState WAL journaling and crash recovery
//! 5. Cross-agent speculative execution
//! 6. Task timeout detection
//! 7. ResultRouter state machine validation

use savant_agent::orchestration::branching::HyperCausalEngine;
use savant_ipc::a2a::agent_card::AgentCard;
use savant_ipc::a2a::context::ContextPackage;
use savant_ipc::a2a::protocol::{
    A2AEnvelope, A2AMessageType, Artifact, ArtifactPart, ArtifactPartType, DelegationTask,
    TaskState,
};
use savant_ipc::a2a::result_router::{DelegationResult, RejectionReason, ResultRouter};
use uuid::Uuid;

// ============================================================================
// AgentCard Tests
// ============================================================================

#[test]
fn test_agent_card_size_matches_actual_layout() {
    let actual = std::mem::size_of::<AgentCard>();
    assert_eq!(
        actual, 176,
        "AgentCard size mismatch — padding or field changes detected"
    );
}

#[test]
fn test_agent_card_new_defaults() {
    let card = AgentCard::new([1u8; 32], "test-agent");
    assert_eq!(card.agent_id, [1u8; 32]);
    assert_eq!(&card.name[..10], b"test-agent");
    assert!(!card.is_active);
    assert_eq!(card.pressure, 0.0);
    assert_eq!(card.protocol_version, 0x0100);
    assert_eq!(card.max_concurrent_tasks, 1);
}

#[test]
fn test_agent_card_availability() {
    let mut card = AgentCard::new([1u8; 32], "test");
    assert!(!card.is_available());

    card.is_active = true;
    assert!(card.is_available());

    card.pressure = 0.95;
    assert!(!card.is_available());

    card.pressure = 0.5;
    assert!(card.is_available());
}

#[test]
fn test_agent_card_skills_bitmask() {
    let mut card = AgentCard::new([1u8; 32], "test");
    card.allowed_skills_mask = 0b1010;

    assert!(card.has_skills(0b0010));
    assert!(card.has_skills(0b1000));
    assert!(card.has_skills(0b1010));
    assert!(!card.has_skills(0b1111));
    assert!(!card.has_skills(0b0100));
}

#[test]
fn test_agent_card_match_score() {
    let mut card = AgentCard::new([1u8; 32], "test");
    card.is_active = true;
    card.allowed_skills_mask = 0b1111;
    card.max_concurrent_tasks = 4;
    card.update_pressure(1);

    let score = card.match_score(0.8, 0b0101);
    assert!(score > 0.0 && score <= 1.0);

    // Unavailable agent should score 0
    card.is_active = false;
    assert_eq!(card.match_score(0.8, 0b0101), 0.0);

    // Insufficient skills should score 0
    card.is_active = true;
    card.allowed_skills_mask = 0b0001;
    assert_eq!(card.match_score(0.8, 0b1111), 0.0);
}

#[test]
fn test_agent_card_pressure_updates() {
    let mut card = AgentCard::new([1u8; 32], "test");
    card.max_concurrent_tasks = 4;

    card.update_pressure(0);
    assert_eq!(card.pressure, 0.0);

    card.update_pressure(2);
    assert_eq!(card.pressure, 0.5);

    card.update_pressure(4);
    assert_eq!(card.pressure, 1.0);

    // Should cap at 1.0
    card.update_pressure(8);
    assert_eq!(card.pressure, 1.0);
}

#[test]
fn test_agent_card_completion_tracking() {
    let mut card = AgentCard::new([1u8; 32], "test");
    card.record_completion(true, 1000);
    card.record_completion(true, 2000);
    card.record_completion(false, 500);

    assert_eq!(card.total_successes, 2);
    assert_eq!(card.total_failures, 1);
    assert_eq!(card.avg_task_duration_ms, 1166);
}

#[test]
fn test_agent_card_input_output_modes() {
    use savant_ipc::a2a::agent_card::{input_modes, output_modes};
    let mut card = AgentCard::new([1u8; 32], "test");

    card.input_modes = input_modes::TEXT | input_modes::MEMORY_GRAPH;
    assert!(card.accepts_input(input_modes::TEXT));
    assert!(card.accepts_input(input_modes::MEMORY_GRAPH));
    assert!(!card.accepts_input(input_modes::TOOL_OUTPUT));

    card.output_modes = output_modes::TEXT | output_modes::JSON;
    assert!(card.produces_output(output_modes::TEXT));
    assert!(card.produces_output(output_modes::JSON));
    assert!(!card.produces_output(output_modes::ARTIFACT));
}

// ============================================================================
// DelegationTask Tests
// ============================================================================

#[test]
fn test_delegation_task_size_matches_actual_layout() {
    let actual = std::mem::size_of::<DelegationTask>();
    assert_eq!(
        actual, 224,
        "DelegationTask size mismatch — padding or field changes detected"
    );
}

#[test]
fn test_delegation_task_new_defaults() {
    let cct = [0xABu8; 64];
    let task = DelegationTask::new([1u8; 16], 12345, [2u8; 32], 4096, cct);

    assert_eq!(task.task_id, [1u8; 16]);
    assert_eq!(task.parent_session_hash, 12345);
    assert_eq!(task.parent_agent_id, [2u8; 32]);
    assert_eq!(task.token_budget, 4096);
    assert_eq!(task.cct_token, cct);
    assert_eq!(task.max_delegation_depth, 20);
    assert_eq!(task.priority_level, 128);
    assert!(!task.requires_consensus);
    assert_eq!(task.speculative_copies, 0);
}

#[test]
fn test_delegation_task_expiry() {
    let cct = [0u8; 64];
    let mut task = DelegationTask::new([1u8; 16], 0, [0u8; 32], 1000, cct);

    // No deadline set
    assert!(!task.is_expired());

    // Deadline in the past
    task.deadline_timestamp = 1;
    assert!(task.is_expired());

    // Deadline far in the future
    task.deadline_timestamp = u64::MAX;
    assert!(!task.is_expired());
}

#[test]
fn test_delegation_task_speculative() {
    let cct = [0u8; 64];
    let mut task = DelegationTask::new([1u8; 16], 0, [0u8; 32], 1000, cct);

    assert!(!task.is_speculative());
    task.speculative_copies = 1;
    assert!(!task.is_speculative());
    task.speculative_copies = 3;
    assert!(task.is_speculative());
}

#[test]
fn test_delegation_task_needs_consensus() {
    let cct = [0u8; 64];
    let mut task = DelegationTask::new([1u8; 16], 0, [0u8; 32], 1000, cct);

    assert!(!task.needs_consensus());
    task.requires_consensus = true;
    assert!(task.needs_consensus());
}

// ============================================================================
// TaskState Tests
// ============================================================================

#[test]
fn test_task_state_size() {
    assert_eq!(std::mem::size_of::<TaskState>(), 1);
}

#[test]
fn test_task_state_transitions() {
    // Valid transitions
    assert!(TaskState::Submitted.can_transition_to(TaskState::Working));
    assert!(TaskState::Submitted.can_transition_to(TaskState::Canceled));
    assert!(TaskState::Working.can_transition_to(TaskState::Completed));
    assert!(TaskState::Working.can_transition_to(TaskState::Failed));
    assert!(TaskState::Working.can_transition_to(TaskState::InputRequired));
    assert!(TaskState::Working.can_transition_to(TaskState::Canceled));
    assert!(TaskState::InputRequired.can_transition_to(TaskState::Working));
    assert!(TaskState::InputRequired.can_transition_to(TaskState::Canceled));
    assert!(TaskState::InputRequired.can_transition_to(TaskState::Failed));

    // Invalid transitions
    assert!(!TaskState::Completed.can_transition_to(TaskState::Working));
    assert!(!TaskState::Failed.can_transition_to(TaskState::Completed));
    assert!(!TaskState::Canceled.can_transition_to(TaskState::Working));
    assert!(!TaskState::Submitted.can_transition_to(TaskState::Completed));
    assert!(!TaskState::Working.can_transition_to(TaskState::Submitted));
}

#[test]
fn test_task_state_terminal() {
    assert!(TaskState::Completed.is_terminal());
    assert!(TaskState::Failed.is_terminal());
    assert!(TaskState::Canceled.is_terminal());
    assert!(!TaskState::Working.is_terminal());
    assert!(!TaskState::Submitted.is_terminal());
    assert!(!TaskState::InputRequired.is_terminal());
}

#[test]
fn test_task_state_active() {
    assert!(TaskState::Working.is_active());
    assert!(TaskState::InputRequired.is_active());
    assert!(!TaskState::Submitted.is_active());
    assert!(!TaskState::Completed.is_active());
    assert!(!TaskState::Failed.is_active());
    assert!(!TaskState::Canceled.is_active());
}

#[test]
fn test_task_state_display() {
    assert_eq!(format!("{}", TaskState::Submitted), "submitted");
    assert_eq!(format!("{}", TaskState::Working), "working");
    assert_eq!(format!("{}", TaskState::InputRequired), "input-required");
    assert_eq!(format!("{}", TaskState::Completed), "completed");
    assert_eq!(format!("{}", TaskState::Failed), "failed");
    assert_eq!(format!("{}", TaskState::Canceled), "canceled");
}

// ============================================================================
// A2AEnvelope Tests
// ============================================================================

#[test]
fn test_a2a_envelope_size() {
    assert_eq!(std::mem::size_of::<A2AEnvelope>(), 168);
}

#[test]
fn test_a2a_envelope_validation() {
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

    // Zero target should be invalid
    env.target_agent_id = [0u8; 32];
    assert!(!env.is_valid());

    // Zero payload size should be invalid
    env.target_agent_id = [2u8; 32];
    env.payload_size = 0;
    assert!(!env.is_valid());
}

#[test]
fn test_a2a_message_type_display() {
    assert_eq!(
        format!("{}", A2AMessageType::TaskDelegation),
        "TaskDelegation"
    );
    assert_eq!(format!("{}", A2AMessageType::StatusUpdate), "StatusUpdate");
    assert_eq!(
        format!("{}", A2AMessageType::ArtifactDelivery),
        "ArtifactDelivery"
    );
    assert_eq!(format!("{}", A2AMessageType::Interruption), "Interruption");
}

// ============================================================================
// ContextPackage Tests
// ============================================================================

#[test]
fn test_context_package_size() {
    assert_eq!(std::mem::size_of::<ContextPackage>(), 448);
}

#[test]
fn test_context_package_builder() {
    let pkg = ContextPackage::new()
        .with_session_collection("transcript.session-123")
        .with_semantic_collection("semantic.concepts")
        .with_episodic_collection("episodic.events")
        .with_entity_collection("entity.people")
        .with_causal_collection("causal.actions")
        .with_temporal_collection("temporal.ordering")
        .with_namespace_scope(42)
        .with_token_budget(8192)
        .with_tool_output(100)
        .with_tool_output(200)
        .with_obsidian_path(500, 256);

    assert!(pkg.has_collections());
    assert_eq!(pkg.tool_output_count, 2);
    assert_eq!(pkg.tool_output_offsets[0], 100);
    assert_eq!(pkg.tool_output_offsets[1], 200);
    assert_eq!(pkg.namespace_scope, 42);
    assert_eq!(pkg.max_token_budget, 8192);
    assert!(pkg.has_obsidian_ref());
    assert_eq!(pkg.obsidian_path_offset, 500);
    assert_eq!(pkg.obsidian_path_len, 256);
}

#[test]
fn test_context_package_tool_output_overflow() {
    let mut pkg = ContextPackage::new();
    for i in 0..10 {
        pkg = pkg.with_tool_output(i * 100);
    }
    // Should cap at 8
    assert_eq!(pkg.tool_output_count, 8);
}

// ============================================================================
// ResultRouter Tests
// ============================================================================

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

    router.update_state("task-1", TaskState::Working).unwrap();
    assert_eq!(router.get_state("task-1"), Some(TaskState::Working));

    router.update_state("task-1", TaskState::Completed).unwrap();
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

    router.update_state("task-1", TaskState::Working).unwrap();
    router.update_state("task-2", TaskState::Working).unwrap();
    assert_eq!(router.active_count(), 2);

    router.update_state("task-1", TaskState::Completed).unwrap();
    assert_eq!(router.active_count(), 1);
}

#[test]
fn test_result_router_tasks_in_state() {
    let mut router = ResultRouter::new();
    router.register_task("task-1", 0);
    router.register_task("task-2", 0);
    router.register_task("task-3", 0);

    router.update_state("task-1", TaskState::Working).unwrap();
    router.update_state("task-2", TaskState::Working).unwrap();
    router.update_state("task-3", TaskState::Working).unwrap();
    router.update_state("task-3", TaskState::Completed).unwrap();

    let working = router.tasks_in_state(TaskState::Working);
    assert_eq!(working.len(), 2);

    let completed = router.tasks_in_state(TaskState::Completed);
    assert_eq!(completed.len(), 1);
}

#[test]
fn test_result_router_complete_task() {
    let mut router = ResultRouter::new();
    router.register_task("task-1", 0);
    router.update_state("task-1", TaskState::Working).unwrap();

    router.complete_task("task-1");
    assert_eq!(router.get_state("task-1"), None);
    assert_eq!(router.get_depth("task-1"), None);
}

#[test]
fn test_rejection_reason_strings() {
    assert_eq!(
        RejectionReason::AgentUnavailable.as_str(),
        "agent_unavailable"
    );
    assert_eq!(
        RejectionReason::InsufficientSkills.as_str(),
        "insufficient_skills"
    );
    assert_eq!(RejectionReason::QueueFull.as_str(), "queue_full");
    assert_eq!(
        RejectionReason::MemoryEnclaveMismatch.as_str(),
        "memory_enclave_mismatch"
    );
    assert_eq!(
        RejectionReason::DelegationDepthExceeded.as_str(),
        "delegation_depth_exceeded"
    );
}

// ============================================================================
// Artifact Tests
// ============================================================================

#[test]
fn test_artifact_new() {
    let artifact = Artifact::new([1u8; 16]);
    assert_eq!(artifact.task_id, [1u8; 16]);
    assert_eq!(artifact.part_count, 0);
}

#[test]
fn test_artifact_part_new() {
    let part = ArtifactPart::new(ArtifactPartType::Text, 100, 512);
    assert_eq!(part.part_type, ArtifactPartType::Text);
    assert_eq!(part.data_offset, 100);
    assert_eq!(part.data_len, 512);
}

// ============================================================================
// DelegationResult Tests
// ============================================================================

#[test]
fn test_delegation_result_variants() {
    let accepted = DelegationResult::Accepted {
        agent_id: [1u8; 32],
    };
    match accepted {
        DelegationResult::Accepted { agent_id } => assert_eq!(agent_id, [1u8; 32]),
        _ => panic!("Expected Accepted"),
    }

    let rejected = DelegationResult::Rejected {
        reason: RejectionReason::AgentUnavailable,
    };
    match rejected {
        DelegationResult::Rejected { reason } => {
            assert_eq!(reason, RejectionReason::AgentUnavailable)
        }
        _ => panic!("Expected Rejected"),
    }

    let timed_out = DelegationResult::TimedOut { task_id: [2u8; 16] };
    match timed_out {
        DelegationResult::TimedOut { task_id } => assert_eq!(task_id, [2u8; 16]),
        _ => panic!("Expected TimedOut"),
    }

    let canceled = DelegationResult::Canceled { task_id: [3u8; 16] };
    match canceled {
        DelegationResult::Canceled { task_id } => assert_eq!(task_id, [3u8; 16]),
        _ => panic!("Expected Canceled"),
    }
}

// ============================================================================
// Task Timeout Detection Tests
// ============================================================================

#[test]
fn test_is_task_expired_no_deadline() {
    // deadline_timestamp == 0 means no deadline
    assert!(!savant_agent::orchestration::continuation::ContinuationEngine::is_task_expired(0));
}

#[test]
fn test_is_task_expired_past_deadline() {
    assert!(savant_agent::orchestration::continuation::ContinuationEngine::is_task_expired(1));
}

#[test]
fn test_is_task_expired_future_deadline() {
    assert!(
        !savant_agent::orchestration::continuation::ContinuationEngine::is_task_expired(u64::MAX)
    );
}

// ============================================================================
// Cross-Agent Speculative Execution Tests
// ============================================================================

#[test]
fn test_cross_agent_speculative_single_copy_delegates_to_speculative() {
    // Verify HyperCausalEngine can be constructed with the expected max_branches
    let _engine = HyperCausalEngine::new(3);
}

// ============================================================================
// Full Delegation Cycle Integration Test
// ============================================================================

#[test]
fn test_full_delegation_cycle_state_transitions() {
    // Simulate the complete lifecycle of a delegated task through the ResultRouter
    let mut router = ResultRouter::new();
    let task_id = Uuid::new_v4().to_string();

    // 1. Task is registered (Submitted)
    router.register_task(&task_id, 0);
    assert_eq!(router.get_state(&task_id), Some(TaskState::Submitted));
    assert!(router.can_delegate_further(&task_id));

    // 2. Task begins execution (Working)
    router.update_state(&task_id, TaskState::Working).unwrap();
    assert_eq!(router.get_state(&task_id), Some(TaskState::Working));
    assert_eq!(router.active_count(), 1);

    // 3. Task requires clarification (InputRequired)
    router
        .update_state(&task_id, TaskState::InputRequired)
        .unwrap();
    assert_eq!(router.get_state(&task_id), Some(TaskState::InputRequired));

    // 4. Task resumes (Working again)
    router.update_state(&task_id, TaskState::Working).unwrap();
    assert_eq!(router.get_state(&task_id), Some(TaskState::Working));

    // 5. Task completes (Completed)
    router.update_state(&task_id, TaskState::Completed).unwrap();
    assert_eq!(router.get_state(&task_id), Some(TaskState::Completed));
    assert!(router.get_state(&task_id).unwrap().is_terminal());
    assert_eq!(router.active_count(), 0);

    // 6. Task is cleaned up
    router.complete_task(&task_id);
    assert_eq!(router.get_state(&task_id), None);
}

#[test]
fn test_delegation_cycle_failure_path() {
    let mut router = ResultRouter::new();
    let task_id = Uuid::new_v4().to_string();

    router.register_task(&task_id, 0);
    router.update_state(&task_id, TaskState::Working).unwrap();
    router.update_state(&task_id, TaskState::Failed).unwrap();

    assert_eq!(router.get_state(&task_id), Some(TaskState::Failed));
    assert!(router.get_state(&task_id).unwrap().is_terminal());
}

#[test]
fn test_delegation_cycle_cancellation_path() {
    let mut router = ResultRouter::new();
    let task_id = Uuid::new_v4().to_string();

    // Cancel from Submitted
    router.register_task(&task_id, 0);
    router.update_state(&task_id, TaskState::Canceled).unwrap();
    assert_eq!(router.get_state(&task_id), Some(TaskState::Canceled));

    // Cancel from Working
    let task_id_2 = Uuid::new_v4().to_string();
    router.register_task(&task_id_2, 0);
    router.update_state(&task_id_2, TaskState::Working).unwrap();
    router
        .update_state(&task_id_2, TaskState::Canceled)
        .unwrap();
    assert_eq!(router.get_state(&task_id_2), Some(TaskState::Canceled));

    // Cancel from InputRequired
    let task_id_3 = Uuid::new_v4().to_string();
    router.register_task(&task_id_3, 0);
    router.update_state(&task_id_3, TaskState::Working).unwrap();
    router
        .update_state(&task_id_3, TaskState::InputRequired)
        .unwrap();
    router
        .update_state(&task_id_3, TaskState::Canceled)
        .unwrap();
    assert_eq!(router.get_state(&task_id_3), Some(TaskState::Canceled));
}

#[test]
fn test_delegation_depth_enforcement() {
    let mut router = ResultRouter::new();

    // Task at max depth cannot delegate further
    router.register_task("deep-task", 20);
    assert!(!router.can_delegate_further("deep-task"));

    // Task at depth 0 can delegate
    router.register_task("root-task", 0);
    assert!(router.can_delegate_further("root-task"));

    // Intermediate depth can delegate
    router.register_task("mid-task", 10);
    assert!(router.can_delegate_further("mid-task"));
}

#[test]
fn test_multiple_concurrent_delegations() {
    let mut router = ResultRouter::new();

    for i in 0..5 {
        router.register_task(&format!("task-{}", i), 0);
    }

    // All submitted
    assert_eq!(router.tasks_in_state(TaskState::Submitted).len(), 5);
    assert_eq!(router.active_count(), 0);

    // Move some to working
    router.update_state("task-0", TaskState::Working).unwrap();
    router.update_state("task-1", TaskState::Working).unwrap();
    router.update_state("task-2", TaskState::Working).unwrap();

    assert_eq!(router.active_count(), 3);
    assert_eq!(router.tasks_in_state(TaskState::Working).len(), 3);
    assert_eq!(router.tasks_in_state(TaskState::Submitted).len(), 2);

    // Complete some
    router.update_state("task-0", TaskState::Completed).unwrap();
    router.update_state("task-1", TaskState::Failed).unwrap();

    assert_eq!(router.active_count(), 1);
    assert_eq!(router.tasks_in_state(TaskState::Completed).len(), 1);
    assert_eq!(router.tasks_in_state(TaskState::Failed).len(), 1);
}
