use crate::error::SavantError;
use crate::types::{ChatChunk, ChatMessage, EventFrame};
pub use async_trait::async_trait;
use futures::stream::Stream;
use std::pin::Pin;

/// Channel Adapter Trait
#[async_trait]
pub trait ChannelAdapter: Send + Sync {
    /// Retrieve the adapter name.
    fn name(&self) -> &str;

    /// Send an event frame (outbound).
    async fn send_event(&self, event: EventFrame) -> Result<(), SavantError>;

    /// Handle an incoming event (inbound).
    async fn handle_event(&self, event: EventFrame) -> Result<(), SavantError>;
}

/// Language Model Provider Trait
#[async_trait]
pub trait LlmProvider: Send + Sync {
    /// Request a chat completion, returning a standardized stream of chunks.
    /// The `tools` parameter provides JSON Schema tool definitions to the LLM API.
    async fn stream_completion(
        &self,
        messages: Vec<ChatMessage>,
        tools: Vec<serde_json::Value>,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<ChatChunk, SavantError>> + Send>>, SavantError>;

    /// Returns the context window size in tokens for the underlying model.
    /// Discovery-based: the provider queries the model's capabilities at construction.
    /// Returns None if the context window is unknown.
    fn context_window(&self) -> Option<usize> {
        None
    }

    /// Returns true if the underlying model supports multimodal input (images).
    /// When true, the provider should handle `ChatMessage.images` as inline image data.
    /// When false, the agent loop will use the vision service to describe images instead.
    fn supports_multimodal(&self) -> bool {
        false
    }
}

/// OMEGA-VIII: Semantic Embedding Provider Trait
#[async_trait]
pub trait EmbeddingProvider: Send + Sync {
    /// Generates an embedding vector for the given text.
    async fn embed(&self, text: &str) -> Result<Vec<f32>, SavantError>;

    /// Generates embeddings for multiple texts in a single batch.
    async fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, SavantError>;

    /// Dimensionality of the produced embeddings.
    fn dimensions(&self) -> usize;
}

/// Vision Model Provider Trait
#[async_trait]
pub trait VisionProvider: Send + Sync {
    /// Describe an image given its base64-encoded data and a prompt.
    async fn describe_image(&self, image_base64: &str, prompt: &str)
        -> Result<String, SavantError>;

    /// Check if the vision model is available.
    async fn is_available(&self) -> bool;

    /// Unload the vision model from memory to free resources.
    async fn unload_model(&self) -> Result<(), SavantError>;
}

/// Memory Backend Trait (LSM-tree / Vector / KV)
#[async_trait]
pub trait MemoryBackend: Send + Sync {
    /// Store a message in persistent session memory.
    async fn store(&self, agent_id: &str, message: &ChatMessage) -> Result<(), SavantError>;

    /// Retrieve relevant context from memory.
    async fn retrieve(
        &self,
        agent_id: &str,
        query: &str,
        limit: usize,
    ) -> Result<Vec<ChatMessage>, SavantError>;

    /// Finalize and optimize memory state.
    async fn consolidate(&self, agent_id: &str) -> Result<(), SavantError>;

    // --- Session / Turn State ---

    /// Gets or creates a session state. Creates a new one if none exists.
    async fn get_or_create_session(
        &self,
        session_id: &str,
    ) -> Result<crate::types::SessionState, SavantError>;

    /// Gets an existing session state. Returns None if no state stored.
    async fn get_session(
        &self,
        session_id: &str,
    ) -> Result<Option<crate::types::SessionState>, SavantError>;

    /// Saves session state (creates or updates).
    async fn save_session(&self, state: &crate::types::SessionState) -> Result<(), SavantError>;

    /// Saves a turn state record.
    async fn save_turn(&self, turn: &crate::types::TurnState) -> Result<(), SavantError>;

    /// Gets a specific turn state. Returns None if not found.
    async fn get_turn(
        &self,
        session_id: &str,
        turn_id: &str,
    ) -> Result<Option<crate::types::TurnState>, SavantError>;

    /// Fetches the most recent N turns for a session (newest first).
    async fn fetch_recent_turns(
        &self,
        session_id: &str,
        limit: usize,
    ) -> Result<Vec<crate::types::TurnState>, SavantError>;

    // --- Memory Lifecycle Operations (default no-ops for backends that don't support them) ---

    /// Run the promotion cycle — score, archive, and promote memories based on access patterns.
    async fn run_promotion_cycle(&self, _agent_id: &str) -> Result<(), SavantError> {
        Ok(())
    }

    /// Synthesize lessons from recurring memory patterns.
    async fn synthesize_lessons(&self, _agent_id: &str) -> Result<(), SavantError> {
        Ok(())
    }

    /// Synthesize insights from concept clusters in the MAGMA graph.
    async fn synthesize_insights(&self, _agent_id: &str) -> Result<(), SavantError> {
        Ok(())
    }

    /// Retrieve synthesized lessons as formatted context for agent injection.
    async fn get_lessons_context(&self) -> String {
        String::new()
    }

    /// Retrieve synthesized insights as formatted context for agent injection.
    async fn get_insights_context(&self) -> String {
        String::new()
    }

    /// Extract entities from recent memories and populate the entity graph.
    async fn extract_entities(&self, _agent_id: &str) -> Result<(), SavantError> {
        Ok(())
    }

    /// Restore memory state from a snapshot.
    async fn restore_state(&self, _agent_id: &str) -> Result<(), SavantError> {
        Ok(())
    }

    /// Auto-recall relevant memories for the current context.
    async fn auto_recall(
        &self,
        _agent_id: &str,
        _query: &str,
    ) -> Result<Vec<ChatMessage>, SavantError> {
        Ok(vec![])
    }
}

#[async_trait]
impl<M: MemoryBackend + ?Sized> MemoryBackend for std::sync::Arc<M> {
    async fn store(&self, agent_id: &str, message: &ChatMessage) -> Result<(), SavantError> {
        self.as_ref().store(agent_id, message).await
    }

    async fn retrieve(
        &self,
        agent_id: &str,
        query: &str,
        limit: usize,
    ) -> Result<Vec<ChatMessage>, SavantError> {
        self.as_ref().retrieve(agent_id, query, limit).await
    }

    async fn consolidate(&self, agent_id: &str) -> Result<(), SavantError> {
        self.as_ref().consolidate(agent_id).await
    }

    async fn get_or_create_session(
        &self,
        session_id: &str,
    ) -> Result<crate::types::SessionState, SavantError> {
        self.as_ref().get_or_create_session(session_id).await
    }

    async fn get_session(
        &self,
        session_id: &str,
    ) -> Result<Option<crate::types::SessionState>, SavantError> {
        self.as_ref().get_session(session_id).await
    }

    async fn save_session(&self, state: &crate::types::SessionState) -> Result<(), SavantError> {
        self.as_ref().save_session(state).await
    }

    async fn save_turn(&self, turn: &crate::types::TurnState) -> Result<(), SavantError> {
        self.as_ref().save_turn(turn).await
    }

    async fn get_turn(
        &self,
        session_id: &str,
        turn_id: &str,
    ) -> Result<Option<crate::types::TurnState>, SavantError> {
        self.as_ref().get_turn(session_id, turn_id).await
    }

    async fn fetch_recent_turns(
        &self,
        session_id: &str,
        limit: usize,
    ) -> Result<Vec<crate::types::TurnState>, SavantError> {
        self.as_ref().fetch_recent_turns(session_id, limit).await
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolDomain {
    Orchestrator,
    Container,
}

/// Approval requirement for tool execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalRequirement {
    /// Never needs approval — execute immediately
    Never,
    /// Needs approval unless explicitly auto-approved by session
    Conditional,
    /// Always needs approval — human must consent
    Always,
}

/// Tool/Capability Trait (OpenClaw/ZeroClaw/IronClaw compatible)
#[async_trait]
pub trait Tool: Send + Sync {
    /// Tool name.
    fn name(&self) -> &str;

    /// Detailed description for LLM guidance.
    fn description(&self) -> &str;

    /// JSON Schema describing the tool's parameters.
    /// This is sent to the LLM API as the tool's `parameters` field.
    /// Default: empty object (no parameters).
    fn parameters_schema(&self) -> serde_json::Value {
        #[allow(clippy::disallowed_methods)]
        let schema = serde_json::json!({
            "type": "object",
            "properties": {},
            "required": []
        });
        schema
    }

    /// Whether this tool requires human approval before execution.
    /// Default: Never (most tools execute immediately).
    fn requires_approval(&self) -> ApprovalRequirement {
        ApprovalRequirement::Never
    }

    /// Explicit capabilities granted to this tool.
    fn capabilities(&self) -> crate::types::CapabilityGrants {
        crate::types::CapabilityGrants::default()
    }

    /// Primary domain this tool operates in.
    fn domain(&self) -> ToolDomain {
        ToolDomain::Orchestrator
    }

    /// Maximum characters in tool output before truncation.
    /// Default: 16,000. Override for tools with large outputs.
    fn max_output_chars(&self) -> usize {
        16_000
    }

    /// Execution timeout in seconds.
    /// Default: 60. Override for tools that need more time.
    fn timeout_secs(&self) -> u64 {
        60
    }

    /// When this tool should be used. Helps the LLM pick the right tool.
    /// Default: empty (no guidance). Override to provide usage heuristics.
    fn when_to_use(&self) -> &str {
        ""
    }

    /// When this tool should NOT be used. Helps prevent wrong tool selection.
    /// Default: empty (no guidance). Override to provide anti-patterns.
    fn when_not_to_use(&self) -> &str {
        ""
    }

    /// Execute the tool with a JSON payload.
    async fn execute(&self, payload: serde_json::Value) -> Result<String, SavantError>;
}

/// OMEGA-VII: Symbolic Browser Projection Trait
///
/// Decouples browser interaction from mutable state, allowing for
/// Intent-Substrate Coherence (ISC) verification.
#[async_trait]
pub trait SymbolicBrowser: Send + Sync {
    /// Projects the current DOM into a symbolic representation.
    async fn project_dom(&self) -> Result<serde_json::Value, SavantError>;

    /// Proves that a browser action matches the intended cognitive outcome.
    async fn prove_intent_coherence(
        &self,
        action: &str,
        selector: &str,
        intent_matrix: serde_json::Value,
    ) -> Result<bool, SavantError>;

    /// Executes the action on the substrate only after verification.
    async fn execute_verified(&self, action: serde_json::Value) -> Result<String, SavantError>;
}
