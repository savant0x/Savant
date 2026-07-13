use crate::budget::TokenBudget;
use crate::context::ContextAssembler;
use crate::learning::filter::OutputFilter;
use crate::plugins::WasmToolHost;
use futures::StreamExt;
use savant_cognitive::DspPredictor;
use savant_core::traits::{LlmProvider, MemoryBackend, Tool, VisionProvider};
use savant_core::types::AgentIdentity;
use savant_echo::{ComponentMetrics, HotSwappableRegistry};
use savant_ipc::CollectiveBlackboard;
use std::sync::Arc;

pub mod autopilot;
pub mod compaction;
pub mod events;
pub mod reactor;
pub mod self_repair;
pub mod stream;
pub mod toon;
pub mod trajectory;

pub use events::AgentEvent;
use savant_core::types::ChatMessage;

#[derive(Debug, Clone, Default)]
pub struct HeuristicState {
    pub failures: usize,
    pub retries: usize,
    pub depth: u32,
    pub last_stable_checkpoint: Option<Vec<ChatMessage>>,
}

pub enum LoopSignal {
    Continue,
    Terminate,
}

pub enum LoopOutcome {
    Success(String),
    Failure(String),
}

pub struct ChatResponse {
    pub content: String,
    pub tool_calls: Vec<savant_core::types::ProviderToolCall>,
}

pub enum TextAction {
    ParseActions,
    Ignore,
}

pub struct LoopContext<'a, M: MemoryBackend> {
    pub loop_state: &'a mut AgentLoop<M>,
    pub trace: &'a mut String,
}

#[async_trait::async_trait]
pub trait LoopDelegate<M: MemoryBackend>: Send + Sync {
    async fn check_signals(&self) -> LoopSignal;
    async fn before_llm_call(&self, ctx: &mut LoopContext<'_, M>) -> Option<LoopOutcome>;
    async fn call_llm(
        &self,
        ctx: &mut LoopContext<'_, M>,
    ) -> Result<ChatResponse, savant_core::error::SavantError>;
    async fn handle_text_response(&self, text: &str, ctx: &mut LoopContext<'_, M>) -> TextAction;
    async fn execute_tool_calls(
        &self,
        calls: Vec<savant_core::types::ProviderToolCall>,
        ctx: &mut LoopContext<'_, M>,
    ) -> Result<Option<LoopOutcome>, savant_core::error::SavantError>;
}

pub struct ChatDelegate;
#[async_trait::async_trait]
impl<M: MemoryBackend> LoopDelegate<M> for ChatDelegate {
    async fn check_signals(&self) -> LoopSignal {
        LoopSignal::Continue
    }
    async fn before_llm_call(&self, ctx: &mut LoopContext<'_, M>) -> Option<LoopOutcome> {
        // Inject context summary: log message count and tools available
        let tool_count = ctx.loop_state.tools.len();
        tracing::debug!("[CHAT_DELEGATE] LLM call: {} tools available", tool_count);
        None
    }
    async fn call_llm(
        &self,
        ctx: &mut LoopContext<'_, M>,
    ) -> Result<ChatResponse, savant_core::error::SavantError> {
        let tool_schemas: Vec<serde_json::Value> = ctx
            .loop_state
            .tools
            .iter()
            .map(|t| t.parameters_schema())
            .collect();
        let messages = ctx.loop_state.context.build_messages(vec![]);
        let mut stream = ctx
            .loop_state
            .provider
            .stream_completion(messages, tool_schemas)
            .await?;
        let mut content = String::new();
        let mut tool_calls = Vec::new();
        while let Some(chunk_res) = stream.next().await {
            let chunk = chunk_res?;
            content.push_str(&chunk.content);
            if let Some(calls) = chunk.tool_calls {
                tool_calls.extend(calls);
            }
        }
        Ok(ChatResponse {
            content,
            tool_calls,
        })
    }
    async fn handle_text_response(&self, _text: &str, _ctx: &mut LoopContext<'_, M>) -> TextAction {
        TextAction::ParseActions
    }
    async fn execute_tool_calls(
        &self,
        _calls: Vec<savant_core::types::ProviderToolCall>,
        _ctx: &mut LoopContext<'_, M>,
    ) -> Result<Option<LoopOutcome>, savant_core::error::SavantError> {
        Ok(None)
    }
}

pub struct HeartbeatDelegate {
    turn_start: std::sync::atomic::AtomicI64,
    ensemble_router: Option<Arc<crate::ensemble::EnsembleRouter>>,
}

impl HeartbeatDelegate {
    pub fn new() -> Self {
        Self {
            turn_start: std::sync::atomic::AtomicI64::new(chrono::Utc::now().timestamp_millis()),
            ensemble_router: None,
        }
    }

    pub fn with_ensemble_router(mut self, router: Arc<crate::ensemble::EnsembleRouter>) -> Self {
        self.ensemble_router = Some(router);
        self
    }
}

impl Default for HeartbeatDelegate {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl<M: MemoryBackend> LoopDelegate<M> for HeartbeatDelegate {
    async fn check_signals(&self) -> LoopSignal {
        // Monitor turn duration — warn if turn exceeds 5 minutes
        let elapsed = chrono::Utc::now().timestamp_millis()
            - self.turn_start.load(std::sync::atomic::Ordering::Relaxed);
        if elapsed > 300_000 {
            tracing::warn!(
                "[HEARTBEAT_DELEGATE] Turn duration {}ms exceeds 5-minute threshold",
                elapsed
            );
        }
        // Circuit breaker checks (timeout, API limits) are already performed in
        // stream.rs before this delegate checkpoint. Returning Continue here.
        LoopSignal::Continue
    }

    async fn before_llm_call(&self, ctx: &mut LoopContext<'_, M>) -> Option<LoopOutcome> {
        // Check context window pressure via ContextCompressor
        let messages = ctx.loop_state.context.build_messages(vec![]);
        let estimated_tokens: usize = messages
            .iter()
            .map(|m| savant_core::utils::token_count(&m.content))
            .sum();
        let usage_ratio = estimated_tokens as f64 / ctx.loop_state.context_window.max(1) as f64;
        if usage_ratio > 0.9 {
            tracing::warn!(
                "[HEARTBEAT_DELEGATE] Context window critical at {:.0}% — compression needed",
                usage_ratio * 100.0
            );
            return Some(LoopOutcome::Failure(
                "Context window critical — compression needed".to_string(),
            ));
        }
        None
    }

    async fn call_llm(
        &self,
        ctx: &mut LoopContext<'_, M>,
    ) -> Result<ChatResponse, savant_core::error::SavantError> {
        let tool_schemas: Vec<serde_json::Value> = ctx
            .loop_state
            .tools
            .iter()
            .map(|t| t.parameters_schema())
            .collect();
        let messages = ctx.loop_state.context.build_messages(vec![]);

        // Primary LLM call
        match ctx
            .loop_state
            .provider
            .stream_completion(messages.clone(), tool_schemas.clone())
            .await
        {
            Ok(mut stream) => {
                let mut content = String::new();
                let mut tool_calls = Vec::new();
                while let Some(chunk_res) = stream.next().await {
                    let chunk = chunk_res?;
                    content.push_str(&chunk.content);
                    if let Some(calls) = chunk.tool_calls {
                        tool_calls.extend(calls);
                    }
                }
                Ok(ChatResponse {
                    content,
                    tool_calls,
                })
            }
            Err(primary_err) => {
                // Fallback: try ensemble router's fallback model via fallback_provider
                if let Some(ref router) = self.ensemble_router {
                    if let Some(fallback_model) = router.select_model(0) {
                        tracing::warn!(
                            "[HEARTBEAT_DELEGATE] Primary LLM failed: {}. Falling back to ensemble model '{}'",
                            primary_err,
                            fallback_model.model
                        );
                        // Use fallback_provider which handles model routing internally
                        if let Some(ref fallback) = ctx.loop_state.fallback_provider {
                            let mut stream =
                                fallback.stream_completion(messages, tool_schemas).await?;
                            let mut content = String::new();
                            let mut tool_calls = Vec::new();
                            while let Some(chunk_res) = stream.next().await {
                                let chunk = chunk_res?;
                                content.push_str(&chunk.content);
                                if let Some(calls) = chunk.tool_calls {
                                    tool_calls.extend(calls);
                                }
                            }
                            return Ok(ChatResponse {
                                content,
                                tool_calls,
                            });
                        }
                    }
                }
                Err(primary_err)
            }
        }
    }

    async fn handle_text_response(&self, text: &str, _ctx: &mut LoopContext<'_, M>) -> TextAction {
        // Grounding check: reject fabricated content
        if !OutputFilter::is_grounded(text) {
            tracing::debug!(
                "[HEARTBEAT_DELEGATE] OutputFilter rejected ungrounded response (len={})",
                text.len()
            );
            return TextAction::Ignore;
        }
        TextAction::ParseActions
    }

    async fn execute_tool_calls(
        &self,
        calls: Vec<savant_core::types::ProviderToolCall>,
        ctx: &mut LoopContext<'_, M>,
    ) -> Result<Option<LoopOutcome>, savant_core::error::SavantError> {
        // Log tool call names and count to trajectory recorder
        let tool_names: Vec<&str> = calls.iter().map(|c| c.name.as_str()).collect();
        tracing::debug!(
            "[HEARTBEAT_DELEGATE] Processing {} tool calls: {:?}",
            calls.len(),
            tool_names
        );
        if let Some(ref recorder) = ctx.loop_state.trajectory_recorder {
            let mut recorder = recorder.lock().await;
            for call in &calls {
                recorder.record_tool_result(&call.name, &call.arguments);
            }
        }
        // Return None to let the default tool execution path proceed
        Ok(None)
    }
}

pub struct SpeculativeDelegate;
#[async_trait::async_trait]
impl<M: MemoryBackend> LoopDelegate<M> for SpeculativeDelegate {
    async fn check_signals(&self) -> LoopSignal {
        LoopSignal::Continue
    }
    async fn before_llm_call(&self, ctx: &mut LoopContext<'_, M>) -> Option<LoopOutcome> {
        // Speculative pre-loading: log available tools for LLM context awareness
        if !ctx.loop_state.tools.is_empty() {
            let tool_names: Vec<&str> = ctx.loop_state.tools.iter().map(|t| t.name()).collect();
            tracing::debug!(
                "[SPECULATIVE_DELEGATE] Pre-loading context: {} tools available ({:?})",
                tool_names.len(),
                tool_names
            );
        }
        None
    }
    async fn call_llm(
        &self,
        ctx: &mut LoopContext<'_, M>,
    ) -> Result<ChatResponse, savant_core::error::SavantError> {
        let tool_schemas: Vec<serde_json::Value> = ctx
            .loop_state
            .tools
            .iter()
            .map(|t| t.parameters_schema())
            .collect();
        let messages = ctx.loop_state.context.build_messages(vec![]);
        let mut stream = ctx
            .loop_state
            .provider
            .stream_completion(messages, tool_schemas)
            .await?;
        let mut content = String::new();
        let mut tool_calls = Vec::new();
        while let Some(chunk_res) = stream.next().await {
            let chunk = chunk_res?;
            content.push_str(&chunk.content);
            if let Some(calls) = chunk.tool_calls {
                tool_calls.extend(calls);
            }
        }
        Ok(ChatResponse {
            content,
            tool_calls,
        })
    }
    async fn handle_text_response(&self, _text: &str, _ctx: &mut LoopContext<'_, M>) -> TextAction {
        TextAction::ParseActions
    }
    async fn execute_tool_calls(
        &self,
        calls: Vec<savant_core::types::ProviderToolCall>,
        _ctx: &mut LoopContext<'_, M>,
    ) -> Result<Option<LoopOutcome>, savant_core::error::SavantError> {
        // Log tool call plan for debugging
        for call in &calls {
            tracing::debug!(
                "[SPECULATIVE_DELEGATE] Tool planned: {}({})",
                call.name,
                call.arguments.chars().take(100).collect::<String>()
            );
        }
        Ok(None)
    }
}

pub struct AgentLoop<M: MemoryBackend> {
    pub(crate) agent_id: String,
    pub(crate) agent_id_hash: u64,
    pub(crate) provider: Arc<dyn LlmProvider>,
    pub(crate) fallback_provider: Option<Arc<dyn LlmProvider>>,
    pub(crate) memory: M,
    pub(crate) tools: Vec<Arc<dyn Tool>>,
    pub(crate) context: ContextAssembler,
    pub(crate) plugin_host: Option<Arc<crate::plugins::WasmPluginHost>>,
    pub(crate) plugins: Vec<wasmtime::component::Component>,
    pub(crate) security_token: Option<savant_security::AgentToken>,
    pub(crate) security_authority: Option<Arc<savant_security::SecurityAuthority>>,
    pub(crate) predictor: DspPredictor,
    pub(crate) echo_registry: Option<Arc<HotSwappableRegistry>>,
    pub(crate) echo_metrics: Option<Arc<ComponentMetrics>>,
    pub(crate) echo_host: Option<Arc<WasmToolHost>>,
    pub(crate) collective_blackboard: Option<Arc<CollectiveBlackboard>>,
    pub(crate) hyper_causal: Arc<crate::orchestration::branching::HyperCausalEngine>,
    pub(crate) agent_index: u8,
    pub(crate) max_parallel_tools: usize,
    pub(crate) max_tool_iterations: usize,
    pub(crate) heuristic: HeuristicState,
    pub(crate) vision_service: Option<Arc<dyn VisionProvider>>,
    pub(crate) self_repair: crate::react::self_repair::SelfRepair,
    /// Security circuit breaker for recursion/API/cost/timeout limits.
    pub(crate) circuit_breaker: savant_security::continuous::circuit_breaker::CircuitBreaker,
    /// Taint tracker for data provenance — tracks external data through the system.
    pub(crate) taint_tracker: Arc<savant_security::continuous::taint::TaintTracker>,
    /// Counter for tool execution errors (for delta tracking).
    pub(crate) tool_error_count: std::sync::atomic::AtomicU32,
    /// Discovery-based context window size from the provider.
    /// Used for TokenBudget, ContextMonitor, and Compactor scaling.
    pub(crate) context_window: usize,
    /// Optional delegate for controlling the agent loop phases.
    /// Wrapped in tokio::sync::Mutex for interior mutability — the delegate lock
    /// is released before creating LoopContext (which borrows &mut self),
    /// avoiding split-borrow conflicts in the try_stream! macro.
    pub(crate) delegate: Option<Arc<tokio::sync::Mutex<Box<dyn LoopDelegate<M>>>>>,
    /// Hook registry for lifecycle extensibility.
    /// Void hooks: fire-and-forget (logging, telemetry).
    /// Modifying hooks: sequential with cancel support (approval, context injection).
    pub(crate) hooks: Arc<savant_core::hooks::HookRegistry>,
    /// When true, skip memory retrieval during context assembly.
    /// Used for heartbeats to prevent old messages from being recalled.
    pub(crate) skip_memory_retrieval: bool,
    /// Trajectory recorder for capturing training data from successful interactions.
    /// Uses Mutex for interior mutability since the stream! macro borrows &self.
    pub(crate) trajectory_recorder:
        Option<tokio::sync::Mutex<crate::react::trajectory::TrajectoryRecorder>>,
    /// Context compressor for LLM-based summarization with cooldown tracking.
    /// Persisted on the struct so the cooldown Mutex survives across loop iterations.
    pub(crate) context_compressor: crate::context_compressor::ContextCompressor,
    /// Dynamic credential broker for per-task ephemeral token management.
    /// Injects secrets on a per-task basis. Agents never hold static, long-lived API keys.
    pub(crate) credential_broker: Arc<savant_security::continuous::credentials::CredentialBroker>,
    /// Panopticon replay recorder for agent reasoning trace.
    /// Records thoughts, tool calls, and observations as structured events.
    pub(crate) replay_recorder: Option<Arc<savant_panopticon::replay::ReplayRecorder>>,
    /// Autopilot for parameter diversity scoring — detects stuck loops
    /// where tool calls succeed but make no progress.
    pub(crate) autopilot: autopilot::ToolCallTracker,
    /// Facet extractor for user preference learning from conversation history.
    pub(crate) facet_extractor: crate::learning::FacetExtractor,
    /// Cache for extracted user preference facets with observation counting.
    pub(crate) facet_cache: crate::learning::FacetCache,
    /// Agent-side rate limiter for LLM API call throttling.
    pub(crate) rate_limiter: Option<Arc<crate::rate_limiter::RateLimiter>>,
    /// CancellationToken for graceful shutdown and sub-agent cancellation.
    /// Checked before each tool execution. If cancelled, tool returns immediately.
    pub(crate) cancellation_token: Option<tokio_util::sync::CancellationToken>,
}

impl<M: MemoryBackend> AgentLoop<M> {
    pub fn new(
        agent_id: String,
        provider: Arc<dyn LlmProvider>,
        memory: M,
        tools: Vec<Arc<dyn Tool>>,
        identity: AgentIdentity,
        substrate_prompt: String,
    ) -> Self {
        let mut skills_summary = String::from("Available Tools:\n");
        for tool in &tools {
            let schema = tool.parameters_schema();
            let schema_hint = if schema.is_object() {
                if let Some(props) = schema.get("properties") {
                    let keys: Vec<&str> = props
                        .as_object()
                        .map(|m| m.keys().map(|k| k.as_str()).collect())
                        .unwrap_or_default();
                    if keys.is_empty() {
                        String::new()
                    } else {
                        format!(" [params: {}]", keys.join(", "))
                    }
                } else {
                    String::new()
                }
            } else {
                String::new()
            };
            skills_summary.push_str(&format!(
                "- {}: {}{}\n",
                tool.name(),
                tool.description(),
                schema_hint
            ));
        }
        let skills_list = if tools.is_empty() {
            None
        } else {
            Some(skills_summary)
        };

        let agent_id_hash = xxhash_rust::xxh3::xxh3_64(agent_id.as_bytes());

        // Discovery-based: use provider's context window, fall back to 128K default
        let context_window = provider.context_window().unwrap_or(128_000);

        Self {
            agent_id,
            agent_id_hash,
            provider,
            fallback_provider: None,
            memory,
            tools,
            context: ContextAssembler::new(
                identity,
                TokenBudget::new(context_window),
                skills_list,
                substrate_prompt,
                crate::proactive::perception::PerceptionEngine::get_substrate_metrics(),
            ),
            plugin_host: None,
            plugins: Vec::new(),
            security_token: None,
            security_authority: None,
            predictor: DspPredictor::default(),
            echo_registry: None,
            echo_metrics: None,
            echo_host: None,
            collective_blackboard: None,
            hyper_causal: Arc::new(crate::orchestration::branching::HyperCausalEngine::default()),
            agent_index: 0,
            max_parallel_tools: 5,
            max_tool_iterations: 10,
            heuristic: HeuristicState::default(),
            vision_service: None,
            self_repair: crate::react::self_repair::SelfRepair::with_defaults(),
            autopilot: crate::react::autopilot::ToolCallTracker::new(),
            circuit_breaker: savant_security::continuous::circuit_breaker::CircuitBreaker::new(),
            taint_tracker: Arc::new(savant_security::continuous::taint::TaintTracker::new()),
            tool_error_count: std::sync::atomic::AtomicU32::new(0),
            context_window,
            delegate: None,
            hooks: Arc::new(savant_core::hooks::HookRegistry::new()),
            skip_memory_retrieval: false,
            trajectory_recorder: None,
            context_compressor: crate::context_compressor::ContextCompressor::new(
                true, // enabled
                0.8,  // trigger at 80% of context window
                3,    // preserve 3 head turns
                5,    // preserve 5 tail turns
                500,  // max summary tokens
                60,   // 60 second cooldown
            ),
            credential_broker: Arc::new(
                savant_security::continuous::credentials::CredentialBroker::new(),
            ),
            replay_recorder: None,
            facet_extractor: crate::learning::FacetExtractor::new(),
            facet_cache: crate::learning::FacetCache::new(),
            rate_limiter: None,
            cancellation_token: None,
        }
    }

    /// Skip memory retrieval during context assembly.
    /// Use for heartbeats to prevent old messages from being recalled into the conversation.
    pub fn set_skip_memory_retrieval(&mut self, skip: bool) {
        self.skip_memory_retrieval = skip;
    }

    pub fn with_plugins(
        mut self,
        host: Arc<crate::plugins::WasmPluginHost>,
        plugins: Vec<wasmtime::component::Component>,
        token: Option<savant_security::AgentToken>,
    ) -> Self {
        self.plugin_host = Some(host);
        self.plugins = plugins;
        self.security_token = token;
        self
    }

    pub fn with_vision(mut self, vision: Arc<dyn VisionProvider>) -> Self {
        self.vision_service = Some(vision);
        self
    }

    pub fn with_echo(
        mut self,
        registry: Arc<HotSwappableRegistry>,
        metrics: Arc<ComponentMetrics>,
        host: Arc<WasmToolHost>,
    ) -> Self {
        self.echo_registry = Some(registry);
        self.echo_metrics = Some(metrics);
        self.echo_host = Some(host);
        self
    }

    pub fn with_fallback(mut self, provider: Arc<dyn LlmProvider>) -> Self {
        self.fallback_provider = Some(provider);
        self
    }

    pub fn with_collective(mut self, blackboard: Arc<CollectiveBlackboard>, index: u8) -> Self {
        self.collective_blackboard = Some(blackboard);
        self.agent_index = index;
        self
    }

    pub fn with_security_authority(
        mut self,
        authority: Arc<savant_security::SecurityAuthority>,
    ) -> Self {
        self.security_authority = Some(authority);
        self
    }

    /// Filters tools based on an allowed tool list (for sub-agent profile restrictions).
    /// If `allowed` is empty, all tools pass through (no filtering).
    pub fn with_tool_filter(mut self, allowed: Vec<String>) -> Self {
        if allowed.is_empty() {
            return self;
        }
        let allowed_set: std::collections::HashSet<String> = allowed.into_iter().collect();
        let original_count = self.tools.len();
        self.tools.retain(|t| allowed_set.contains(t.name()));
        tracing::info!(
            "[{}] ToolFilter applied: {}/{} tools available",
            self.agent_id,
            self.tools.len(),
            original_count
        );
        // Rebuild skills summary with filtered tools
        let mut skills_summary = String::from("Available Tools:\n");
        for tool in &self.tools {
            skills_summary.push_str(&format!("- {}: {}\n", tool.name(), tool.description()));
        }
        self.context.update_skills_list(Some(skills_summary));
        self
    }

    /// Rotates the root authority key on the security authority.
    /// Creates a new SecurityAuthority with the rotated key and replaces the existing one.
    pub fn rotate_root_authority(&mut self, next_authority: ed25519_dalek::VerifyingKey) {
        if let Some(ref authority) = self.security_authority {
            let new_authority =
                savant_security::SecurityAuthority::new(next_authority, authority.pqc_authority);
            self.security_authority = Some(Arc::new(new_authority));
            tracing::info!("[{}] Root authority rotated successfully", self.agent_id);
        } else {
            tracing::warn!(
                "[{}] Cannot rotate root authority: no security authority configured",
                self.agent_id
            );
        }
    }

    /// Injects a credential broker for per-task ephemeral token management.
    pub fn with_credential_broker(
        mut self,
        broker: Arc<savant_security::continuous::credentials::CredentialBroker>,
    ) -> Self {
        self.credential_broker = broker;
        self
    }

    pub fn with_hyper_causal(
        mut self,
        engine: crate::orchestration::branching::HyperCausalEngine,
    ) -> Self {
        self.hyper_causal = Arc::new(engine);
        self
    }

    pub fn with_trajectory_recorder(
        mut self,
        recorder: crate::react::trajectory::TrajectoryRecorder,
    ) -> Self {
        self.trajectory_recorder = Some(tokio::sync::Mutex::new(recorder));
        self
    }

    /// Injects a Panopticon replay recorder for agent reasoning trace.
    pub fn with_replay_recorder(
        mut self,
        recorder: Arc<savant_panopticon::replay::ReplayRecorder>,
    ) -> Self {
        self.replay_recorder = Some(recorder);
        self
    }

    /// Injects a rate limiter for LLM API call throttling.
    pub fn with_rate_limiter(mut self, limiter: Arc<crate::rate_limiter::RateLimiter>) -> Self {
        self.rate_limiter = Some(limiter);
        self
    }

    /// Sets a delegate for controlling the agent loop phases.
    ///
    /// The delegate's methods are called at each phase checkpoint in the loop:
    /// - `check_signals()` — before each iteration
    /// - `before_llm_call()` — before LLM call
    /// - `call_llm()` — instead of default LLM call
    /// - `handle_text_response()` — after text response
    /// - `execute_tool_calls()` — instead of default tool execution
    pub fn with_delegate(mut self, delegate: Box<dyn LoopDelegate<M>>) -> Self {
        self.delegate = Some(Arc::new(tokio::sync::Mutex::new(delegate)));
        self
    }

    /// Registers the 7 built-in hooks into the hook registry.
    /// Called during agent construction in swarm.rs.
    pub async fn register_default_hooks(&self) {
        use savant_core::hooks::*;
        self.hooks.register_void(BeforeToolCallLogger).await;
        self.hooks.register_void(ToolCallLogger).await;
        self.hooks.register_void(LlmInputLogger).await;
        self.hooks.register_void(LlmOutputLogger).await;
        self.hooks.register_void(HealthMonitorHook).await;
        self.hooks.register_void(SessionLifecycleHook).await;
        self.hooks.register_void(SessionEndHook).await;
    }
}

#[cfg(test)]
mod heuristic_tests;
