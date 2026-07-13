use crate::error::SwarmIpcError;
use iceoryx2::prelude::*;
use iceoryx2::service::port_factory::blackboard::PortFactory;
use std::sync::Arc;
use tracing::{debug, info};

/// Individual Agent Entry in the Collective Blackboard.
///
/// MUST be `#[repr(C)]` for zero-copy sharing.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, ZeroCopySend)]
pub struct AgentEntry {
    /// Total successful tool executions by this agent
    pub successes: u64,
    /// Total tool failures by this agent
    pub failures: u64,
    /// Agent-specific task pressure (0.0 to 1.0)
    pub pressure: f32,
    /// Whether the agent is currently participating in the swarm pulse
    pub is_active: bool,
    /// Epoch-relative index of the agent (1-128)
    pub agent_index: u8,
    /// Reserved for future expansion
    pub reserved: [u8; 10],
}

impl Default for AgentEntry {
    fn default() -> Self {
        Self {
            successes: 0,
            failures: 0,
            pressure: 0.0,
            is_active: false,
            agent_index: 0,
            reserved: [0; 10],
        }
    }
}

/// Swarm-wide Collective Intelligence State (Global Entry 0)
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, ZeroCopySend)]
pub struct GlobalState {
    /// Swarm-wide heuristic version (incremented on major insights)
    pub heuristic_version: u64,
    /// Aggregate swarm pressure (mean of all active agents)
    pub swarm_pressure: f32,
    /// Swarm-wide aggregate successes
    pub total_successes: u64,
    /// Swarm-wide aggregate failures
    pub total_failures: u64,

    // --- Swarm Consensus Phase (Voting) ---
    /// The current proposal hash (XXH3) undergoing voting
    pub active_proposal_hash: u64,
    /// Type of proposal (0=None, 1=Destructive Edit, 2=Security Polish)
    pub proposal_type: u8,
    /// Bitmask of "Approve" votes (Supports up to 128 agents)
    pub approve_mask: [u64; 2],
    /// Bitmask of "Veto" votes
    pub veto_mask: [u64; 2],
    /// Threshold required for consensus
    pub quorum_threshold: u8,

    /// Reserved for future swarm expansion
    pub reserved: [u8; 31],
}

impl Default for GlobalState {
    fn default() -> Self {
        Self {
            heuristic_version: 0,
            swarm_pressure: 0.0,
            total_successes: 0,
            total_failures: 0,
            active_proposal_hash: 0,
            proposal_type: 0,
            approve_mask: [0; 2],
            veto_mask: [0; 2],
            quorum_threshold: 3,
            reserved: [0; 31],
        }
    }
}

/// The Collective Blackboard Service.
///
/// Implements a distributed entry model where each agent owns a specific
/// index in the blackboard to avoid concurrency race conditions.
pub struct CollectiveBlackboard {
    _node: Arc<Node<ipc::Service>>,
    service: PortFactory<ipc::Service, u64>,
}

impl CollectiveBlackboard {
    /// Initializes the collective blackboard with 129 entries (0=Global, 1-128=Agents).
    pub fn new(service_name: &str) -> Result<Self, SwarmIpcError> {
        info!(
            "Initializing Distributed Collective Blackboard '{}' (129 entries)",
            service_name
        );

        let node = NodeBuilder::new()
            .create::<ipc::Service>()
            .map_err(|e| SwarmIpcError::NodeCreation(e.to_string()))?;

        // Clean up stale shared memory from dead processes before creating.
        let cleanup = Node::<ipc::Service>::cleanup_dead_nodes(Config::global_config());
        if cleanup.cleanups > 0 || cleanup.failed_cleanups > 0 {
            debug!(
                "CollectiveBlackboard: stale node cleanup — {} removed, {} failed",
                cleanup.cleanups, cleanup.failed_cleanups
            );
        }

        // Retry-with-suffix on collision. Stale shared memory from a prior
        // crashed process can cause create() to fail on Windows.
        let base_name = service_name.to_string();
        let mut attempt: u32 = 0;
        let max_attempts: u32 = 5;

        let service = loop {
            let candidate = if attempt == 0 {
                base_name.clone()
            } else {
                format!("{}_{}", base_name, attempt)
            };

            let iox_name: iceoryx2::prelude::ServiceName = candidate.as_str().try_into().map_err(
                |e: iceoryx2::service::service_name::ServiceNameError| {
                    SwarmIpcError::ServiceCreation(e.to_string())
                },
            )?;

            let mut builder = node
                .service_builder(&iox_name)
                .blackboard_creator::<u64>()
                .max_readers(1024)
                .max_nodes(128)
                .add::<GlobalState>(0, GlobalState::default());

            for i in 1..=128 {
                builder = builder.add::<AgentEntry>(i as u64, AgentEntry::default());
            }

            match builder.create() {
                Ok(svc) => break svc,
                Err(e) if attempt < max_attempts => {
                    attempt += 1;
                    debug!(
                        "Collective Blackboard '{}' creation failed (attempt {}/{}), retrying as '{}': {}",
                        base_name, attempt, max_attempts, candidate, e
                    );
                }
                Err(e) => {
                    return Err(SwarmIpcError::ServiceCreation(format!(
                        "Collective Blackboard '{}' creation failed after {} attempts: {}",
                        base_name,
                        max_attempts + 1,
                        e
                    )));
                }
            }
        };

        info!("Distributed Collective Blackboard initialized.");

        Ok(Self {
            _node: Arc::new(node),
            service,
        })
    }

    /// Publishes the global swarm state.
    pub fn publish_global_state(&self, state: GlobalState) -> Result<(), SwarmIpcError> {
        let writer = self.service.writer_builder().create().map_err(|e| {
            SwarmIpcError::AccessViolation(format!("Failed to create global writer: {}", e))
        })?;

        let entry = writer.entry::<GlobalState>(&0).map_err(|e| {
            SwarmIpcError::AccessViolation(format!("Global state entry not found: {}", e))
        })?;

        entry.update_with_copy(state);
        Ok(())
    }

    /// Reads the global swarm state.
    pub fn read_global_state(&self) -> Result<GlobalState, SwarmIpcError> {
        let reader = self.service.reader_builder().create().map_err(|e| {
            SwarmIpcError::AccessViolation(format!("Failed to create global reader: {}", e))
        })?;

        if let Ok(entry) = reader.entry::<GlobalState>(&0) {
            Ok(*entry.get())
        } else {
            Err(SwarmIpcError::AccessViolation(
                "Global state entry not found".to_string(),
            ))
        }
    }

    /// Updates metrics for a specific agent.
    pub fn update_agent_metrics(
        &self,
        agent_index: u8,
        success: bool,
        pressure: f32,
    ) -> Result<(), SwarmIpcError> {
        if agent_index == 0 || agent_index > 128 {
            return Err(SwarmIpcError::AccessViolation(format!(
                "Invalid agent index: {}",
                agent_index
            )));
        }

        // We need a reader to get the current state and a writer to update it
        let reader = self.service.reader_builder().create().map_err(|e| {
            SwarmIpcError::AccessViolation(format!("Failed to create agent reader: {}", e))
        })?;

        let writer = self.service.writer_builder().create().map_err(|e| {
            SwarmIpcError::AccessViolation(format!("Failed to create agent writer: {}", e))
        })?;

        let id = agent_index as u64;
        let entry_ref = writer.entry::<AgentEntry>(&id).map_err(|e| {
            SwarmIpcError::AccessViolation(format!("Agent entry {} not found: {}", id, e))
        })?;

        let mut entry = if let Ok(reader_entry) = reader.entry::<AgentEntry>(&id) {
            *reader_entry.get()
        } else {
            AgentEntry::default()
        };

        if success {
            entry.successes += 1;
        } else {
            entry.failures += 1;
        }
        entry.pressure = pressure;
        entry.is_active = true;
        entry.agent_index = agent_index;

        entry_ref.update_with_copy(entry);
        Ok(())
    }

    /// Aggregates all agent metrics and publishes them to the global state.
    ///
    /// This should typically be called by a designated "Swarm Leader" or periodically by agents.
    pub fn aggregate_swarm_metrics(&self) -> Result<GlobalState, SwarmIpcError> {
        let reader = self.service.reader_builder().create().map_err(|e| {
            SwarmIpcError::AccessViolation(format!("Failed to create aggregation reader: {}", e))
        })?;

        let mut global = self.read_global_state()?;
        let mut total_successes = 0;
        let mut total_failures = 0;
        let mut total_pressure = 0.0;
        let mut active_count = 0;

        for i in 1..=128 {
            if let Ok(entry) = reader.entry::<AgentEntry>(&(i as u64)) {
                let data = entry.get();
                if data.is_active {
                    total_successes += data.successes;
                    total_failures += data.failures;
                    total_pressure += data.pressure;
                    active_count += 1;
                }
            }
        }

        global.total_successes = total_successes;
        global.total_failures = total_failures;
        if active_count > 0 {
            global.swarm_pressure = total_pressure / (active_count as f32);
        }

        self.publish_global_state(global)?;
        Ok(global)
    }

    /// Setting the quorum threshold dynamically.
    pub fn set_quorum_threshold(&self, threshold: u8) -> Result<(), SwarmIpcError> {
        let mut state = self.read_global_state()?;
        state.quorum_threshold = threshold;
        self.publish_global_state(state)
    }

    /// Participating in a vote using the per-agent isolated masks in GlobalState.
    pub fn cast_vote(&self, agent_index: u8, approve: bool) -> Result<(), SwarmIpcError> {
        // IPC-03: Validate agent_index to prevent underflow panic
        if agent_index == 0 {
            return Err(SwarmIpcError::AccessViolation(
                "invalid agent index or mask bounds exceeded".to_string(),
            ));
        }

        // Note: consensus voting still requires a read-modify-write on GlobalState
        // but it is less frequent than metrics updates.
        let mut state = self.read_global_state()?;

        // Agent indices are 1-based for blackboard, but 0-based for masks
        let mask_index = (agent_index - 1) as usize;
        let mask_idx = mask_index / 64;
        let bit_idx = mask_index % 64;

        // IPC-03: Bounds-check mask index to prevent out-of-bounds panic
        if mask_idx >= state.approve_mask.len() {
            return Err(SwarmIpcError::AccessViolation(
                "invalid agent index or mask bounds exceeded".to_string(),
            ));
        }

        if approve {
            state.approve_mask[mask_idx] |= 1 << bit_idx;
        } else {
            state.veto_mask[mask_idx] |= 1 << bit_idx;
        }

        self.publish_global_state(state)
    }

    /// Checking if consensus is reached.
    pub fn check_consensus(&self) -> ConsensusResult {
        let Ok(state) = self.read_global_state() else {
            return ConsensusResult::Pending;
        };

        if state.veto_mask[0] != 0 || state.veto_mask[1] != 0 {
            return ConsensusResult::Vetoed;
        }

        let total_approvals =
            state.approve_mask[0].count_ones() + state.approve_mask[1].count_ones();
        if total_approvals >= state.quorum_threshold as u32 {
            ConsensusResult::Approved
        } else {
            ConsensusResult::Pending
        }
    }
}

pub enum ConsensusResult {
    Approved,
    Vetoed,
    Pending,
}

// ============================================================================
// Delegation Consensus Extension
// ============================================================================
// Provides high-level methods for initiating and tracking consensus votes
// on delegated tasks. When a DelegationTask has requires_consensus=true,
// the Orchestrator initiates a vote before the task is executed.

/// A consensus proposal for a delegated task.
#[derive(Debug, Clone)]
pub struct DelegationProposal {
    pub task_id: [u8; 16],
    pub target_agent_id: [u8; 32],
    pub description: String,
    pub proposal_type: DelegationProposalType,
}

/// Type of delegation proposal requiring consensus.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DelegationProposalType {
    /// Destructive file operation (delete, move, edit)
    DestructiveEdit = 1,
    /// Security-sensitive operation (credential access, network call)
    SecurityOperation = 2,
    /// Tool synthesis (new WASM tool creation)
    ToolSynthesis = 3,
}

impl DelegationProposal {
    pub fn new(
        task_id: [u8; 16],
        target_agent_id: [u8; 32],
        description: &str,
        proposal_type: DelegationProposalType,
    ) -> Self {
        Self {
            task_id,
            target_agent_id,
            description: description.to_string(),
            proposal_type,
        }
    }

    /// Computes the proposal hash for the active_proposal_hash field.
    pub fn hash(&self) -> u64 {
        use xxhash_rust::xxh3::xxh3_64;
        let mut input = Vec::new();
        input.extend_from_slice(&self.task_id);
        input.extend_from_slice(&self.target_agent_id);
        input.extend_from_slice(self.description.as_bytes());
        input.push(self.proposal_type as u8);
        xxh3_64(&input)
    }
}

/// High-level consensus operations for delegation tasks.
pub struct DelegationConsensus<'a> {
    collective: &'a CollectiveBlackboard,
}

impl<'a> DelegationConsensus<'a> {
    pub fn new(collective: &'a CollectiveBlackboard) -> Self {
        Self { collective }
    }

    /// Initiates a consensus vote for a delegated task.
    ///
    /// Returns the proposal hash on success. Agents should then cast their votes
    /// via `cast_delegation_vote()`.
    pub fn propose(&self, proposal: &DelegationProposal) -> Result<u64, SwarmIpcError> {
        let mut state = self.collective.read_global_state()?;
        let hash = proposal.hash();

        state.active_proposal_hash = hash;
        state.proposal_type = proposal.proposal_type as u8;
        // Reset vote masks
        state.approve_mask = [0; 2];
        state.veto_mask = [0; 2];

        self.collective.publish_global_state(state)?;
        info!(
            task_id = %hex_encode(&proposal.task_id),
            proposal_type = %proposal.proposal_type as u8,
            "Delegation consensus proposal initiated"
        );
        Ok(hash)
    }

    /// Casts a vote on the active delegation proposal.
    pub fn cast_vote(&self, agent_index: u8, approve: bool) -> Result<(), SwarmIpcError> {
        if agent_index == 0 || agent_index > 128 {
            return Err(SwarmIpcError::AccessViolation(format!(
                "Invalid agent index: {}",
                agent_index
            )));
        }
        self.collective.cast_vote(agent_index, approve)
    }

    /// Checks the current consensus status of the active proposal.
    pub fn check_consensus(&self) -> ConsensusResult {
        self.collective.check_consensus()
    }

    /// Waits for consensus to be reached, polling at the given interval.
    ///
    /// Returns `Ok(ConsensusResult::Approved)` if consensus is reached,
    /// or `Err` if the timeout expires or a veto is detected.
    pub async fn await_consensus(
        &self,
        poll_interval_ms: u64,
        timeout_ms: u64,
    ) -> Result<ConsensusResult, ConsensusTimeoutError> {
        let start = std::time::Instant::now();
        let timeout = std::time::Duration::from_millis(timeout_ms);
        let interval = std::time::Duration::from_millis(poll_interval_ms);

        loop {
            let result = self.check_consensus();
            match result {
                ConsensusResult::Approved => return Ok(result),
                ConsensusResult::Vetoed => {
                    return Err(ConsensusTimeoutError::Vetoed);
                }
                ConsensusResult::Pending => {
                    if start.elapsed() >= timeout {
                        return Err(ConsensusTimeoutError::TimedOut {
                            elapsed_ms: start.elapsed().as_millis() as u64,
                        });
                    }
                    tokio::time::sleep(interval).await;
                }
            }
        }
    }

    /// Clears the active proposal (after execution or cancellation).
    pub fn clear_proposal(&self) -> Result<(), SwarmIpcError> {
        let mut state = self.collective.read_global_state()?;
        state.active_proposal_hash = 0;
        state.proposal_type = 0;
        state.approve_mask = [0; 2];
        state.veto_mask = [0; 2];
        self.collective.publish_global_state(state)?;
        Ok(())
    }
}

/// Errors that can occur during delegation consensus.
#[derive(Debug, thiserror::Error)]
pub enum ConsensusTimeoutError {
    #[error("Delegation consensus was vetoed by an agent")]
    Vetoed,
    #[error("Delegation consensus timed out after {elapsed_ms}ms")]
    TimedOut { elapsed_ms: u64 },
}

/// Utility: encode bytes as hex string for logging.
fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{:02x}", b)).collect()
}

#[cfg(test)]
mod delegation_consensus_tests {
    use super::*;

    #[test]
    fn test_delegation_proposal_hash() {
        let p1 = DelegationProposal::new(
            [1u8; 16],
            [2u8; 32],
            "test proposal",
            DelegationProposalType::DestructiveEdit,
        );
        let p2 = DelegationProposal::new(
            [1u8; 16],
            [2u8; 32],
            "test proposal",
            DelegationProposalType::DestructiveEdit,
        );
        let p3 = DelegationProposal::new(
            [3u8; 16],
            [2u8; 32],
            "different",
            DelegationProposalType::SecurityOperation,
        );
        assert_eq!(p1.hash(), p2.hash());
        assert_ne!(p1.hash(), p3.hash());
    }

    #[test]
    fn test_delegation_proposal_type_values() {
        assert_eq!(DelegationProposalType::DestructiveEdit as u8, 1);
        assert_eq!(DelegationProposalType::SecurityOperation as u8, 2);
        assert_eq!(DelegationProposalType::ToolSynthesis as u8, 3);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_collective_state_voting_logic() {
        let mut state = GlobalState {
            quorum_threshold: 2,
            ..Default::default()
        };

        // Agent 1 approves (mask index 0, bit 0 as it's 1-based index)
        let mask_idx = 0;
        let bit_idx = 0;
        state.approve_mask[mask_idx] |= 1 << bit_idx;

        // Check consensus (Pending as 1 < 2)
        assert_eq!(state.approve_mask[0].count_ones(), 1);

        // Agent 2 approves
        let bit_idx2 = 1;
        state.approve_mask[0] |= 1 << bit_idx2;

        assert_eq!(state.approve_mask[0].count_ones(), 2);
    }

    #[test]
    fn test_collective_veto_overrides_approval() {
        let mut state = GlobalState {
            quorum_threshold: 1,
            ..Default::default()
        };

        // Approve
        state.approve_mask[0] |= 1 << 5;
        // Veto
        state.veto_mask[0] |= 1 << 10;

        // Manual verification of the logic used in check_consensus
        let has_veto = state.veto_mask[0] != 0 || state.veto_mask[1] != 0;
        assert!(has_veto);
    }
}
