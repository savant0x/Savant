use anyhow::{anyhow, Result};
use savant_ipc::a2a::agent_card::AgentCard;
use savant_ipc::blackboard::SwarmSharedContext;
use savant_ipc::collective::{
    CollectiveBlackboard, ConsensusResult, DelegationConsensus, DelegationProposal,
    DelegationProposalType,
};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{oneshot, Mutex};
use tracing::{info, warn};

/// Manages handoffs between agents and ensures cycle prevention.
pub struct OrchestrationRouter {
    agent_id: u32,
    /// Pending delivery receipt channels keyed by session_hash.
    /// Sender is stored here; `await_receipt` holds the receiver.
    pending_receipts: Arc<Mutex<HashMap<u64, oneshot::Sender<()>>>>,
    /// Collective blackboard for delegation consensus voting.
    collective: Option<Arc<CollectiveBlackboard>>,
}

impl OrchestrationRouter {
    pub fn new(agent_id: u32, _host_id: u32) -> Self {
        Self {
            agent_id,
            pending_receipts: Arc::new(Mutex::new(HashMap::new())),
            collective: None,
        }
    }

    /// Sets the collective blackboard for delegation consensus support.
    pub fn with_collective(mut self, collective: Arc<CollectiveBlackboard>) -> Self {
        self.collective = Some(collective);
        self
    }

    /// Validates a handoff request and checks for delegation cycles.
    ///
    /// Returns Ok(()) if the handoff is safe, or an error if a cycle is detected
    /// or delegation depth is exceeded.
    pub fn validate_handoff(
        &self,
        ctx: &mut SwarmSharedContext,
        target_agent_id: u32,
    ) -> anyhow::Result<()> {
        // Inject distributed trace context for cross-agent observability
        savant_panopticon::inject_trace_context(ctx);

        // Check if current depth already hit the limit
        if ctx.delegation_filter.depth_count >= ctx.max_delegation_depth {
            warn!(
                "Delegation depth exceeded ({} >= {})",
                ctx.delegation_filter.depth_count, ctx.max_delegation_depth
            );
            return Err(anyhow::anyhow!("Delegation depth exceeded"));
        }

        // Add current agent to the bloom filter trace (increments depth)
        ctx.delegation_filter.add_agent(self.agent_id.into());

        // Check if target agent is already in the trace path
        if ctx.delegation_filter.contains_agent(target_agent_id.into()) {
            warn!(
                "Cycle detected: Target agent {} has already processed this session",
                target_agent_id
            );
            return Err(anyhow!("Circular delegation detected"));
        }

        Ok(())
    }

    /// Validates a handoff against an AgentCard — checks capability match.
    ///
    /// This is the new typed delegation path. Instead of relying on hardcoded
    /// agent IDs, the Orchestrator checks the target's AgentCard for:
    /// - Availability (is_active, pressure < 0.9)
    /// - Required skills (bitwise AND with required_skills_mask)
    /// - Semantic match score (cosine similarity of description vectors)
    pub fn validate_handoff_with_card(
        &self,
        ctx: &mut SwarmSharedContext,
        target_agent_id: u32,
        target_card: &AgentCard,
        required_skills: u128,
        semantic_similarity: f32,
    ) -> Result<(), HandoffRejection> {
        // First: standard cycle prevention
        if let Err(e) = self.validate_handoff(ctx, target_agent_id) {
            return Err(HandoffRejection::CycleDetected(e.to_string()));
        }

        // Check agent availability
        if !target_card.is_available() {
            return Err(HandoffRejection::AgentUnavailable);
        }

        // Check required skills
        if !target_card.has_skills(required_skills) {
            return Err(HandoffRejection::InsufficientSkills);
        }

        // Check semantic match quality (minimum 0.3 similarity)
        if semantic_similarity < 0.3 {
            return Err(HandoffRejection::LowSemanticMatch {
                score: semantic_similarity,
                threshold: 0.3,
            });
        }

        // Check composite match score
        let score = target_card.match_score(semantic_similarity, required_skills);
        if score < 0.2 {
            return Err(HandoffRejection::LowCompositeScore {
                score,
                threshold: 0.2,
            });
        }

        info!(
            target_agent = %hex_encode_32(&target_card.agent_id),
            score = %score,
            "Handoff validated with AgentCard capability match"
        );
        Ok(())
    }

    /// Validates a handoff requiring consensus approval from the swarm.
    ///
    /// Creates a DelegationProposal and initiates consensus voting. Used for
    /// destructive operations, security-sensitive tasks, or tool synthesis.
    /// Returns Ok(proposal_hash) if consensus is reached, Err if vetoed or timed out.
    pub async fn validate_handoff_with_consensus(
        &self,
        task_id: [u8; 16],
        target_agent_id: [u8; 32],
        description: &str,
        proposal_type: DelegationProposalType,
        poll_interval_ms: u64,
        timeout_ms: u64,
    ) -> Result<u64, HandoffRejection> {
        let collective = self.collective.as_ref().ok_or_else(|| {
            HandoffRejection::ConsensusError("No collective blackboard configured".to_string())
        })?;

        let consensus = DelegationConsensus::new(collective);
        let proposal =
            DelegationProposal::new(task_id, target_agent_id, description, proposal_type);

        let hash = consensus.propose(&proposal).map_err(|e| {
            HandoffRejection::ConsensusError(format!("Failed to create proposal: {}", e))
        })?;

        info!(
            proposal_hash = %hash,
            proposal_type = %proposal_type as u8,
            "Delegation consensus vote initiated"
        );

        match consensus
            .await_consensus(poll_interval_ms, timeout_ms)
            .await
        {
            Ok(ConsensusResult::Approved) => {
                info!(proposal_hash = %hash, "Delegation consensus APPROVED");
                Ok(hash)
            }
            Ok(ConsensusResult::Vetoed) => {
                warn!(proposal_hash = %hash, "Delegation consensus VETOED");
                if let Err(e) = consensus.clear_proposal() {
                    warn!("Consensus proposal cleanup failed after veto: {}", e);
                }
                Err(HandoffRejection::ConsensusVetoed)
            }
            Ok(ConsensusResult::Pending) => {
                warn!(proposal_hash = %hash, "Delegation consensus still pending after timeout");
                if let Err(e) = consensus.clear_proposal() {
                    warn!("Consensus proposal cleanup failed after timeout: {}", e);
                }
                Err(HandoffRejection::ConsensusError(
                    "Consensus still pending".to_string(),
                ))
            }
            Err(e) => {
                warn!(proposal_hash = %hash, error = %e, "Delegation consensus failed");
                if let Err(clear_err) = consensus.clear_proposal() {
                    warn!(
                        "Consensus proposal cleanup failed after error: {}",
                        clear_err
                    );
                }
                Err(HandoffRejection::ConsensusError(e.to_string()))
            }
        }
    }

    /// Records the initiation of a handoff.
    pub fn record_handoff(&self, target_agent_id: u32) {
        info!(
            "Handoff initiated: Agent {} -> Agent {}",
            self.agent_id, target_agent_id
        );
    }

    /// Records a typed handoff with AgentCard metadata.
    pub fn record_typed_handoff(
        &self,
        target_card: &AgentCard,
        task_id: &[u8; 16],
        semantic_score: f32,
    ) {
        info!(
            target_agent = %hex_encode_32(&target_card.agent_id),
            task_id = %hex_encode(task_id),
            semantic_score = %semantic_score,
            pressure = %target_card.pressure,
            "Typed handoff initiated with capability match"
        );
    }

    /// Awaits a delivery receipt from the target agent.
    ///
    /// Creates a oneshot channel, stores the sender keyed by `session_hash`,
    /// and awaits the receiver with the specified timeout. The target agent
    /// calls `emit_receipt` to signal delivery.
    pub async fn await_receipt(&self, session_hash: u64, timeout_ms: u64) -> Result<()> {
        info!(
            session_hash = %session_hash,
            timeout_ms = %timeout_ms,
            "Awaiting delivery receipt"
        );

        let (tx, rx) = oneshot::channel::<()>();
        {
            let mut pending = self.pending_receipts.lock().await;
            pending.insert(session_hash, tx);
        }

        let timeout = tokio::time::Duration::from_millis(timeout_ms);
        match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(())) => {
                info!(
                    session_hash = %session_hash,
                    "Delivery receipt received"
                );
                Ok(())
            }
            Ok(Err(_)) => {
                warn!(
                    session_hash = %session_hash,
                    "Delivery receipt channel dropped before receipt was sent"
                );
                Err(anyhow!(
                    "Receipt channel dropped for session {}",
                    session_hash
                ))
            }
            Err(_) => {
                // Timeout — clean up the pending sender
                let mut pending = self.pending_receipts.lock().await;
                pending.remove(&session_hash);
                warn!(
                    session_hash = %session_hash,
                    timeout_ms = %timeout_ms,
                    "Delivery receipt timed out"
                );
                Err(anyhow!(
                    "Receipt timeout for session {} after {}ms",
                    session_hash,
                    timeout_ms
                ))
            }
        }
    }

    /// Emits a delivery receipt for a received session.
    ///
    /// Looks up the pending oneshot sender for `session_hash` and sends the
    /// signal, unblocking the corresponding `await_receipt` call.
    pub async fn emit_receipt(&self, sender_agent_id: u32, session_hash: u64) {
        let tx = {
            let mut pending = self.pending_receipts.lock().await;
            pending.remove(&session_hash)
        };

        match tx {
            Some(tx) => {
                if tx.send(()).is_ok() {
                    info!(
                        session_hash = %session_hash,
                        sender_agent = %sender_agent_id,
                        "Delivery receipt emitted"
                    );
                } else {
                    warn!(
                        session_hash = %session_hash,
                        "Receipt receiver already dropped — send failed"
                    );
                }
            }
            None => {
                warn!(
                    session_hash = %session_hash,
                    "No pending receipt found for session — emit_receipt called without await_receipt"
                );
            }
        }
    }
}

/// Reasons a typed handoff may be rejected.
#[derive(Debug, thiserror::Error)]
pub enum HandoffRejection {
    #[error("Delegation cycle detected: {0}")]
    CycleDetected(String),
    #[error("Target agent is not available (inactive or overloaded)")]
    AgentUnavailable,
    #[error("Target agent lacks required skills")]
    InsufficientSkills,
    #[error("Semantic match too low: {score} < {threshold}")]
    LowSemanticMatch { score: f32, threshold: f32 },
    #[error("Composite match score too low: {score} < {threshold}")]
    LowCompositeScore { score: f32, threshold: f32 },
    #[error("Delegation consensus was vetoed")]
    ConsensusVetoed,
    #[error("Delegation consensus error: {0}")]
    ConsensusError(String),
}

/// Utility: encode bytes as hex string for logging.
fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{:02x}", b)).collect()
}

/// Utility: encode 32-byte agent ID as hex string.
fn hex_encode_32(bytes: &[u8; 32]) -> String {
    hex_encode(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_handoff_with_card_success() {
        let router = OrchestrationRouter::new(1, 0);
        let mut ctx = SwarmSharedContext::default();
        let mut card = AgentCard::new([2u8; 32], "test-agent");
        card.is_active = true;
        card.allowed_skills_mask = 0b1111;
        card.pressure = 0.1;

        let result = router.validate_handoff_with_card(&mut ctx, 2, &card, 0b0101, 0.8);
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_handoff_with_card_unavailable() {
        let router = OrchestrationRouter::new(1, 0);
        let mut ctx = SwarmSharedContext::default();
        let mut card = AgentCard::new([2u8; 32], "test-agent");
        card.is_active = false;

        let result = router.validate_handoff_with_card(&mut ctx, 2, &card, 0b0001, 0.8);
        assert!(matches!(result, Err(HandoffRejection::AgentUnavailable)));
    }

    #[test]
    fn test_validate_handoff_with_card_insufficient_skills() {
        let router = OrchestrationRouter::new(1, 0);
        let mut ctx = SwarmSharedContext::default();
        let mut card = AgentCard::new([2u8; 32], "test-agent");
        card.is_active = true;
        card.allowed_skills_mask = 0b0001;

        let result = router.validate_handoff_with_card(&mut ctx, 2, &card, 0b1111, 0.8);
        assert!(matches!(result, Err(HandoffRejection::InsufficientSkills)));
    }

    #[test]
    fn test_validate_handoff_with_card_low_semantic() {
        let router = OrchestrationRouter::new(1, 0);
        let mut ctx = SwarmSharedContext::default();
        let mut card = AgentCard::new([2u8; 32], "test-agent");
        card.is_active = true;
        card.allowed_skills_mask = 0b1111;

        let result = router.validate_handoff_with_card(&mut ctx, 2, &card, 0b0001, 0.1);
        assert!(matches!(
            result,
            Err(HandoffRejection::LowSemanticMatch { .. })
        ));
    }

    #[test]
    fn test_record_typed_handoff() {
        let router = OrchestrationRouter::new(1, 0);
        let card = AgentCard::new([2u8; 32], "test-agent");
        router.record_typed_handoff(&card, &[1u8; 16], 0.85);
    }

    #[tokio::test]
    async fn test_receipt_emit_before_await() {
        let router = OrchestrationRouter::new(1, 0);
        // Emit before await — should warn but not panic
        router.emit_receipt(2, 12345).await;
    }

    #[tokio::test]
    async fn test_receipt_await_then_emit() {
        let router = OrchestrationRouter::new(1, 0);
        let session_hash = 99999u64;

        // Spawn await in background
        let pending = Arc::clone(&router.pending_receipts);

        // Register a sender manually to simulate the flow
        let (tx, rx) = tokio::sync::oneshot::channel::<()>();
        {
            let mut map = pending.lock().await;
            map.insert(session_hash, tx);
        }

        // Emit should find and complete the sender
        router.emit_receipt(2, session_hash).await;

        // Receiver should get the signal
        let result = tokio::time::timeout(std::time::Duration::from_millis(100), rx).await;
        assert!(result.is_ok());
        assert!(result.unwrap().is_ok());
    }
}
