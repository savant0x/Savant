#![allow(clippy::disallowed_methods)]

#[cfg(test)]
use super::*;
use async_trait::async_trait;
use futures::stream::StreamExt;
use futures::Stream;
use savant_core::error::SavantError;
use savant_core::traits::{LlmProvider, MemoryBackend};
use savant_core::types::{AgentIdentity, AgentOutputChannel, ChatMessage};
use std::pin::Pin;
use tokio_util::sync::CancellationToken;
struct AmbiguousLlm {
    responses: Vec<String>,
    call_count: std::sync::atomic::AtomicUsize,
}

#[async_trait]
impl LlmProvider for AmbiguousLlm {
    async fn stream_completion(
        &self,
        _messages: Vec<ChatMessage>,
        _tools: Vec<serde_json::Value>,
    ) -> Result<
        Pin<Box<dyn Stream<Item = Result<savant_core::types::ChatChunk, SavantError>> + Send>>,
        SavantError,
    > {
        let idx = self
            .call_count
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let response = self
            .responses
            .get(idx)
            .cloned()
            .unwrap_or_else(|| "Thought: Done.\nAction: None".to_string());
        let chunk = savant_core::types::ChatChunk {
            agent_name: "test".to_string(),
            agent_id: "test".to_string(),
            content: response,
            is_final: true,
            session_id: None,
            channel: AgentOutputChannel::Chat,
            logprob: None,
            is_telemetry: false,
            reasoning: None,
            tool_calls: None,
        };
        Ok(Box::pin(futures::stream::iter(vec![Ok(chunk)])))
    }
}

struct MockMemory;
#[async_trait]
impl MemoryBackend for MockMemory {
    async fn store(&self, _agent_id: &str, _msg: &ChatMessage) -> Result<(), SavantError> {
        Ok(())
    }
    async fn retrieve(
        &self,
        _agent_id: &str,
        _query: &str,
        _limit: usize,
    ) -> Result<Vec<ChatMessage>, SavantError> {
        Ok(vec![])
    }
    async fn consolidate(&self, _agent_id: &str) -> Result<(), SavantError> {
        Ok(())
    }
    async fn get_or_create_session(
        &self,
        _session_id: &str,
    ) -> Result<savant_core::types::SessionState, SavantError> {
Ok(savant_core::types::SessionState {
    session_id: "mock".to_string(),
    created_at: 0,
    last_active: 0,
    turn_count: 0,
    active_turn_id: None,
    auto_approved_tools: vec![],
    denied_tools: vec![],
    parent_session_id: None,
    fork_point_turn_id: None,
    // FID-029 §Step 1: mock initializer; title is populated by sibling
    // collection at hydrate time in async_backend.rs.
    title: None,
})
    }
    async fn get_session(
        &self,
        _session_id: &str,
    ) -> Result<Option<savant_core::types::SessionState>, SavantError> {
        Ok(None)
    }
    async fn save_session(
        &self,
        _state: &savant_core::types::SessionState,
    ) -> Result<(), SavantError> {
        Ok(())
    }
    async fn save_turn(&self, _turn: &savant_core::types::TurnState) -> Result<(), SavantError> {
        Ok(())
    }
    async fn get_turn(
        &self,
        _session_id: &str,
        _turn_id: &str,
    ) -> Result<Option<savant_core::types::TurnState>, SavantError> {
        Ok(None)
    }
    async fn fetch_recent_turns(
        &self,
        _session_id: &str,
        _limit: usize,
    ) -> Result<Vec<savant_core::types::TurnState>, SavantError> {
        Ok(vec![])
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn test_autonomous_ambiguity_synthesis() {
    let provider = Arc::new(AmbiguousLlm {
        responses: vec![
            "Thought: I should use a tool.\nAction: MockTool missing_brackets".to_string(),
            "Thought: Done.\nAction: None".to_string(),
        ],
        call_count: std::sync::atomic::AtomicUsize::new(0),
    });

    let mut agent = AgentLoop::new(
        "test_agent".into(),
        provider,
        MockMemory,
        vec![], // No tools, but we want to see if it parses
        AgentIdentity::default(),
        String::new(),
    );

    let mut stream = agent.run("start".into(), None, CancellationToken::new());
    let mut ambiguity_detected = false;
    let mut synthesized_action = false;

    while let Some(res) = stream.next().await {
        match res {
            Ok(AgentEvent::StatusUpdate(s)) if s == "HEURISTIC_AMBIGUITY_DETECTED" => {
                ambiguity_detected = true;
            }
            Ok(AgentEvent::Action { name, .. }) if name.contains("MockTool") => {
                synthesized_action = true;
            }
            _ => {}
        }
    }
    drop(stream);

    assert!(
        ambiguity_detected,
        "Should have detected ambiguity in malformed Action: line"
    );
    assert!(
        synthesized_action,
        "Should have synthesized the MockTool action"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn test_checkpoint_creation() {
    let provider = Arc::new(AmbiguousLlm {
        responses: vec!["Action: Tool1[]".to_string()],
        call_count: std::sync::atomic::AtomicUsize::new(0),
    });

    let mut agent = AgentLoop::new(
        "test_agent".into(),
        provider,
        MockMemory,
        vec![],
        AgentIdentity::default(),
        String::new(),
    );

    assert!(agent.heuristic.last_stable_checkpoint.is_none());

    let mut stream = agent.run("test".into(), None, CancellationToken::new());
    // Run until action execution
    while let Some(res) = stream.next().await {
        if let Ok(AgentEvent::Action { .. }) = res {
            break;
        }
    }
    drop(stream);

    assert!(
        agent.heuristic.last_stable_checkpoint.is_some(),
        "Checkpoint should be created before actions"
    );
    assert_eq!(
        agent
            .heuristic
            .last_stable_checkpoint
            .as_ref()
            .unwrap()
            .len(),
        1,
        "Checkpoint should contain at least the user message"
    );
}
