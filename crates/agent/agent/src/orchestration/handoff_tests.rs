use crate::orchestration::handoff::OrchestrationRouter;
use savant_ipc::blackboard::{DelegationBloomFilter, SwarmSharedContext};

#[test]
fn test_cycle_prevention_logic() {
    let mut ctx = SwarmSharedContext {
        session_id_hash: 12345,
        parent_agent_id: 0,
        current_token_budget: 1000,
        task_complexity_score: 1.0,
        emergency_halt: false,
        continue_work_delay_ms: 0,
        trace_id: [0; 16],
        span_id: [0; 8],
        delegation_filter: DelegationBloomFilter::new(),
        max_delegation_depth: 10,
        reserved: [0; 25],
    };

    let router_a = OrchestrationRouter::new(1, 100);
    let router_b = OrchestrationRouter::new(2, 101);
    let router_c = OrchestrationRouter::new(3, 102);

    // Initial handoffs: A -> B -> C
    assert!(router_a.validate_handoff(&mut ctx, 2).is_ok());
    assert!(router_b.validate_handoff(&mut ctx, 3).is_ok());

    // Cycle check: C -> A should fail because A is already in the filter
    assert!(router_c.validate_handoff(&mut ctx, 1).is_err());
}

#[test]
fn test_depth_limit_enforcement() {
    let mut ctx = SwarmSharedContext {
        session_id_hash: 12345,
        parent_agent_id: 0,
        current_token_budget: 1000,
        task_complexity_score: 1.0,
        emergency_halt: false,
        continue_work_delay_ms: 0,
        trace_id: [0; 16],
        span_id: [0; 8],
        delegation_filter: DelegationBloomFilter::new(),
        max_delegation_depth: 2, // Set low limit
        reserved: [0; 25],
    };

    let router = OrchestrationRouter::new(1, 100);

    // Handoffs: 1 -> 2, 2 -> 3
    assert!(router.validate_handoff(&mut ctx, 2).is_ok());
    assert!(router.validate_handoff(&mut ctx, 3).is_ok());

    // 3 -> 4 should fail due to depth
    assert!(router.validate_handoff(&mut ctx, 4).is_err());
}
