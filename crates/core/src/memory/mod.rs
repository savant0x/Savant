use crate::error::SavantError;
use crate::traits::EmbeddingProvider;
use crate::traits::MemoryBackend;
use crate::types::ChatMessage;
use std::path::Path;
use std::sync::Arc;
use tracing::info;

pub use savant_memory::MemoryEngine as SavantMemoryEngine;

/// Implementation of MemoryBackend using SavantMemoryEngine (Fjall).
pub struct FjallMemoryBackend {
    engine: SavantMemoryEngine,
}

impl FjallMemoryBackend {
    /// Creates a new memory backend with the given storage path and embedding provider.
    pub fn new(
        storage_path: impl AsRef<Path>,
        embedding_provider: Arc<dyn EmbeddingProvider>,
    ) -> Result<Self, SavantError> {
        let engine = SavantMemoryEngine::with_defaults(storage_path, embedding_provider)
            .map_err(|e| SavantError::Unknown(format!("Fjall init failed: {}", e)))?;
        Ok(Self { engine })
    }
}

#[async_trait::async_trait]
impl MemoryBackend for FjallMemoryBackend {
    async fn store(&self, agent_id: &str, message: &ChatMessage) -> Result<(), SavantError> {
        let agent_msg = savant_memory::AgentMessage::from_chat(message, agent_id)
            .map_err(|e| SavantError::Unknown(e.to_string()))?;
        self.engine
            .append_message(agent_id, &agent_msg)
            .map_err(|e| SavantError::Unknown(e.to_string()))?;
        Ok(())
    }

    async fn retrieve(
        &self,
        agent_id: &str,
        _query: &str,
        limit: usize,
    ) -> Result<Vec<ChatMessage>, SavantError> {
        let messages = self.engine.fetch_session_tail(agent_id, limit);
        let chat_messages: Vec<ChatMessage> = messages
            .into_iter()
            .map(|msg| msg.to_chat())
            .collect::<Vec<ChatMessage>>();
        Ok(chat_messages)
    }

    async fn consolidate(&self, agent_id: &str) -> Result<(), SavantError> {
        let removed = self
            .engine
            .consolidate(agent_id)
            .await
            .map_err(|e| SavantError::Unknown(format!("Memory consolidation failed: {}", e)))?;
        info!(
            "Consolidation complete for agent {}: removed {} duplicates",
            agent_id, removed
        );
        Ok(())
    }

    async fn get_or_create_session(
        &self,
        session_id: &str,
    ) -> Result<crate::types::SessionState, SavantError> {
        let state = self
            .engine
            .get_or_create_session_state(session_id)
            .await
            .map_err(|e| SavantError::Unknown(e.to_string()))?;

        Ok(crate::types::SessionState {
            session_id: state.session_id,
            created_at: state.created_at.into(),
            last_active: state.last_active.into(),
            turn_count: state.turn_count.into(),
            active_turn_id: state.active_turn_id,
            auto_approved_tools: state.auto_approved_tools,
            denied_tools: state.denied_tools,
            // FID-029 §Step 1: rkyv source has no title; populated by sibling
            // collection at the async_backend.rs MemoryBackend trait impl layer.
            title: None,
        })
    }

    async fn get_session(
        &self,
        session_id: &str,
    ) -> Result<Option<crate::types::SessionState>, SavantError> {
        match self
            .engine
            .get_session_state(session_id)
            .map_err(|e| SavantError::Unknown(e.to_string()))?
        {
            Some(state) => Ok(Some(crate::types::SessionState {
                session_id: state.session_id,
                created_at: state.created_at.into(),
                last_active: state.last_active.into(),
                turn_count: state.turn_count.into(),
                active_turn_id: state.active_turn_id,
                auto_approved_tools: state.auto_approved_tools,
                denied_tools: state.denied_tools,
                // FID-029 §Step 1: rkyv source has no title; populated by sibling
                // collection at the async_backend.rs MemoryBackend trait impl layer.
                title: None,
            })),
            None => Ok(None),
        }
    }

    async fn save_session(&self, state: &crate::types::SessionState) -> Result<(), SavantError> {
        let rkyv_state = savant_memory::SessionState {
            session_id: state.session_id.clone(),
            created_at: state.created_at.into(),
            last_active: state.last_active.into(),
            turn_count: state.turn_count.into(),
            active_turn_id: state.active_turn_id.clone(),
            auto_approved_tools: state.auto_approved_tools.clone(),
            denied_tools: state.denied_tools.clone(),
        };
        self.engine
            .save_session_state(&rkyv_state)
            .await
            .map_err(|e| SavantError::Unknown(e.to_string()))
    }

    async fn save_turn(&self, turn: &crate::types::TurnState) -> Result<(), SavantError> {
        let phase = match turn.state {
            crate::types::TurnPhase::Processing => savant_memory::TurnPhase::Processing,
            crate::types::TurnPhase::Completed => savant_memory::TurnPhase::Completed,
            crate::types::TurnPhase::Failed => savant_memory::TurnPhase::Failed,
            crate::types::TurnPhase::Interrupted => savant_memory::TurnPhase::Interrupted,
            crate::types::TurnPhase::AwaitingApproval => savant_memory::TurnPhase::AwaitingApproval,
        };
        let rkyv_turn = savant_memory::TurnState {
            turn_id: turn.turn_id.clone(),
            session_id: turn.session_id.clone(),
            state: phase,
            tool_calls_made: turn.tool_calls_made.clone(),
            started_at: turn.started_at.into(),
            completed_at: turn.completed_at.into(),
        };
        self.engine
            .save_turn_state(&rkyv_turn)
            .await
            .map_err(|e| SavantError::Unknown(e.to_string()))
    }

    async fn get_turn(
        &self,
        session_id: &str,
        turn_id: &str,
    ) -> Result<Option<crate::types::TurnState>, SavantError> {
        match self
            .engine
            .get_turn_state(session_id, turn_id)
            .map_err(|e| SavantError::Unknown(e.to_string()))?
        {
            Some(turn) => {
                let phase = match turn.state {
                    savant_memory::TurnPhase::Processing => crate::types::TurnPhase::Processing,
                    savant_memory::TurnPhase::Completed => crate::types::TurnPhase::Completed,
                    savant_memory::TurnPhase::Failed => crate::types::TurnPhase::Failed,
                    savant_memory::TurnPhase::Interrupted => crate::types::TurnPhase::Interrupted,
                    savant_memory::TurnPhase::AwaitingApproval => {
                        crate::types::TurnPhase::AwaitingApproval
                    }
                };
                Ok(Some(crate::types::TurnState {
                    turn_id: turn.turn_id,
                    session_id: turn.session_id,
                    state: phase,
                    tool_calls_made: turn.tool_calls_made,
                    started_at: turn.started_at.into(),
                    completed_at: turn.completed_at.into(),
                }))
            }
            None => Ok(None),
        }
    }

    async fn fetch_recent_turns(
        &self,
        session_id: &str,
        limit: usize,
    ) -> Result<Vec<crate::types::TurnState>, SavantError> {
        let turns = self
            .engine
            .fetch_recent_turns(session_id, limit)
            .map_err(|e| SavantError::Unknown(e.to_string()))?;

        Ok(turns
            .into_iter()
            .map(|t| {
                let phase = match t.state {
                    savant_memory::TurnPhase::Processing => crate::types::TurnPhase::Processing,
                    savant_memory::TurnPhase::Completed => crate::types::TurnPhase::Completed,
                    savant_memory::TurnPhase::Failed => crate::types::TurnPhase::Failed,
                    savant_memory::TurnPhase::Interrupted => crate::types::TurnPhase::Interrupted,
                    savant_memory::TurnPhase::AwaitingApproval => {
                        crate::types::TurnPhase::AwaitingApproval
                    }
                };
                crate::types::TurnState {
                    turn_id: t.turn_id,
                    session_id: t.session_id,
                    state: phase,
                    tool_calls_made: t.tool_calls_made,
                    started_at: t.started_at.into(),
                    completed_at: t.completed_at.into(),
                }
            })
            .collect())
    }
}
