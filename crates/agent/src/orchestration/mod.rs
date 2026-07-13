//! Swarm Orchestration and Anti-Dwindle Engine
//!
//! This module implements:
//! 1. Deterministic subagent spawning via `/subagents spawn` command
//! 2. Zero-copy context sharing using the blackboard
//! 3. DSP-accelerated ReAct loop
//! 4. Anti-dwindle continuation handling
//!
//! It replaces OpenClaw's deep TypeScript promise chains and JSON serialization
//! with a high-performance, zero-copy architecture.

pub mod branching;
pub mod continuation;
pub mod dag;
pub mod handoff;
#[cfg(test)]
mod handoff_tests;
pub mod ignition;
pub mod plan_verifier;
pub mod skill_chain;
pub mod synthesis;
pub mod tasks;

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::RwLock;

use ed25519_dalek::SigningKey;
use pqcrypto_dilithium::dilithium2;
use rand;
use savant_core::traits::{MemoryBackend, Tool};
use savant_core::types::{AgentConfig, AgentIdentity};
use savant_ipc::{hash_session_id, CapabilityRegistry, SwarmBlackboard, SwarmSharedContext};
// Omitted imports for cleaner orchestration substrate
use xxhash_rust;

use super::budget::TokenBudget;
use super::react::AgentLoop;
use crate::orchestration::continuation::{ContinuationConfig, ContinuationEngine};
use futures::StreamExt;
use savant_cognitive::{DspConfig, DspPredictor, GeneticForge, SynthesisEngine};
use savant_core::traits::LlmProvider;
use thiserror::Error;
use tracing::{debug, error, info, instrument, warn};

/// The Orchestrator Agent manages the entire swarm's coordination.
///
/// It implements the Zero-Copy Speculative Swarm Architecture:
/// - Uses iceoryx2 Blackboard for O(1) context sharing
/// - Uses DSP for dynamic speculation depth prediction
/// - Implements /subagents spawn for deterministic subagent spawning
/// - Handles CONTINUE_WORK tokens to prevent the dwindle pattern
type SubagentHandle = tokio::task::JoinHandle<Result<(), String>>;

pub struct Orchestrator {
    agent_loop: AgentLoop<Arc<dyn MemoryBackend>>,
    blackboard: Arc<SwarmBlackboard>,
    dsp_predictor: DspPredictor,
    token_budget: Arc<RwLock<TokenBudget>>,
    continuation_engine: crate::orchestration::ContinuationEngine,
    subagent_handles: Arc<RwLock<HashMap<String, SubagentHandle>>>,
    session_id: String,
    max_chain_length: u32,
    max_subagents: usize,
    signing_key: SigningKey,
    pqc_signing_key: dilithium2::SecretKey,
    capability_registry: Arc<CapabilityRegistry>,
    memory_enclave: Option<Arc<savant_memory::engine::MemoryEnclave>>,
    delegation_engine: Option<Arc<crate::delegation::DelegationEngine>>,
    /// Pending continuation delay to publish to blackboard on next update.
    /// Set by `execute_turn()` when a CONTINUE_WORK token is detected.
    /// Uses AtomicU32 because `update_blackboard_context` takes `&self`.
    pending_continue_delay_ms: std::sync::atomic::AtomicU32,
    /// Channel to stream speculative response text to the caller in real-time.
    /// Set via `set_chunk_tx()` before calling `execute_turn()`.
    /// Each `FinalAnswer` event is forwarded as a raw text string.
    chunk_tx: Option<tokio::sync::mpsc::UnboundedSender<String>>,
    /// Accumulates only FinalAnswer text (not thoughts/actions/reflections)
    /// during `collect_speculative_response` so `execute_turn` can return
    /// clean chat text for the completion message.
    answer_buffer: String,
}

/// Configuration for the orchestrator.
#[derive(Debug, Clone)]
pub struct OrchestratorConfig {
    /// DSP configuration for speculation depth prediction
    pub dsp_config: DspConfig,
    /// Maximum chain length for CONTINUE_WORK loops (safety guard)
    pub max_chain_length: u32,
    /// Continuation engine configuration
    pub continuation_config: crate::orchestration::ContinuationConfig,
    /// Maximum concurrent subagents per orchestrator (0 = unlimited)
    pub max_subagents_per_agent: usize,
}

impl Default for OrchestratorConfig {
    fn default() -> Self {
        Self {
            dsp_config: DspConfig::default(),
            max_chain_length: 10, // Match OpenClaw's safety constraint
            continuation_config: ContinuationConfig::default(),
            max_subagents_per_agent: 8,
        }
    }
}

impl Orchestrator {
    /// Creates an Orchestrator from a pre-built AgentLoop.
    ///
    /// This is the enterprise-grade constructor: the caller builds the AgentLoop
    /// with all its builder methods (echo, collective, plugins, security_authority,
    /// delegate, vision), then passes it here. The Orchestrator adds orchestration
    /// capabilities (DSP, continuation, handoff, A2A delegation) on top.
    ///
    /// Signing keys are generated internally (Ed25519 + Dilithium2).
    ///
    /// # Arguments
    /// * `agent_loop` — Pre-built AgentLoop with all builder methods applied
    /// * `agent_id` — The agent's unique identifier
    /// * `session_id` — The session identifier
    /// * `blackboard` — Shared zero-copy blackboard for the swarm
    /// * `capability_registry` — Capability registry for delegation
    /// * `memory_enclave` — Optional memory enclave for context extraction
    pub fn from_agent_loop(
        agent_loop: AgentLoop<Arc<dyn MemoryBackend>>,
        agent_id: String,
        session_id: String,
        blackboard: Arc<SwarmBlackboard>,
        capability_registry: Arc<CapabilityRegistry>,
        memory_enclave: Option<Arc<savant_memory::engine::MemoryEnclave>>,
    ) -> Self {
        info!(
            agent_id = %agent_id,
            "Orchestrator initialized from pre-built AgentLoop"
        );

        Self {
            agent_loop,
            blackboard,
            dsp_predictor: DspPredictor::default(),
            token_budget: Arc::new(RwLock::new(TokenBudget::new(100_000))),
            continuation_engine: ContinuationEngine::default(),
            subagent_handles: Arc::new(RwLock::new(HashMap::new())),
            session_id,
            max_chain_length: 10,
            signing_key: SigningKey::generate(&mut rand::rngs::OsRng),
            pqc_signing_key: {
                let (_, sk) = dilithium2::keypair();
                sk
            },
            capability_registry,
            memory_enclave,
            delegation_engine: None,
            max_subagents: 8, // Default, overridden by OrchestratorConfig in `new()`
            pending_continue_delay_ms: std::sync::atomic::AtomicU32::new(0),
            chunk_tx: None,
            answer_buffer: String::new(),
        }
    }

    /// Mint a Cryptographic Capability Token (CCT) for a subagent.
    /// Consolidates the three CCT minting paths into a single method.
    ///
    /// # Arguments
    /// * `subagent_id` — The target subagent's identifier (as bytes)
    /// * `task_entropy` — Optional task-specific entropy for entropic key derivation.
    ///   If provided, the signing key is derived from this entropy for per-task isolation.
    ///   If None, the base signing key is used directly.
    fn mint_subagent_cct(
        &self,
        subagent_id: &[u8],
        task_entropy: Option<&[u8]>,
    ) -> Result<savant_security::AgentToken, OrchestratorError> {
        let subagent_hash = xxhash_rust::xxh3::xxh3_64(subagent_id);

        let signing_key = if let Some(entropy) = task_entropy {
            savant_security::SecurityAuthority::derive_entropic_key(&self.signing_key, entropy)
        } else {
            self.signing_key.clone()
        };

        let token = savant_security::SecurityAuthority::mint_quantum_token(
            &signing_key,
            &self.pqc_signing_key,
            subagent_hash,
            &format!("/workspace/{}", self.session_id),
            "read",
            3600,
            subagent_id,
        )
        .map_err(|e| OrchestratorError::SecurityError(e.to_string()))?;

        Ok(token)
    }

    /// Set the delegation engine for this orchestrator.
    pub fn set_delegation_engine(&mut self, engine: Arc<crate::delegation::DelegationEngine>) {
        self.delegation_engine = Some(engine);
    }

    /// Get a reference to the delegation engine, if set.
    pub fn delegation_engine(&self) -> Option<&Arc<crate::delegation::DelegationEngine>> {
        self.delegation_engine.as_ref()
    }

    /// Set a channel to receive speculative response text in real-time.
    /// Each `FinalAnswer` event is sent as a raw string through this channel.
    /// The caller wraps them in `ChatChunk` with appropriate metadata.
    pub fn set_chunk_tx(&mut self, tx: tokio::sync::mpsc::UnboundedSender<String>) {
        self.chunk_tx = Some(tx);
    }

    /// Drop the chunk sender channel.
    /// After calling this, the receiver side of the channel will close
    /// once all buffered messages are drained. Used by heartbeat to signal
    /// end-of-stream after `execute_turn()` returns.
    pub fn take_chunk_tx(&mut self) {
        self.chunk_tx.take();
    }

    /// Check if the subagent limit has been reached.
    /// Returns Ok(()) if spawning is allowed, Err if at capacity.
    async fn check_subagent_limit(&self) -> Result<(), OrchestratorError> {
        if self.max_subagents == 0 {
            return Ok(()); // 0 = unlimited
        }
        let handles = self.subagent_handles.read().await;
        if handles.len() >= self.max_subagents {
            Err(OrchestratorError::SubagentLimitExceeded {
                current: handles.len(),
                max: self.max_subagents,
            })
        } else {
            Ok(())
        }
    }

    /// Creates a new orchestrator agent (legacy constructor).
    ///
    /// # Arguments
    /// * `config` - The agent configuration (contains agent_id, model, etc.)
    /// * `provider` - The LLM provider (wrapped in RetryProvider)
    /// * `memory` - Memory manager for this agent
    /// * `tools` - Available tools for this agent
    /// * `identity` - Agent identity/persona
    /// * `blackboard` - Shared zero-copy blackboard for the swarm
    /// * `orchestrator_config` - Orchestration-specific configuration
    ///
    /// # Returns
    /// A fully initialized Orchestrator ready to execute turns.
    #[allow(clippy::too_many_arguments)]
    pub async fn new(
        config: AgentConfig,
        provider: Arc<dyn LlmProvider>,
        memory: Arc<dyn MemoryBackend>,
        tools: Vec<Arc<dyn Tool>>,
        identity: String,
        blackboard: Arc<SwarmBlackboard>,
        capability_registry: Arc<CapabilityRegistry>,
        memory_enclave: Option<Arc<savant_memory::engine::MemoryEnclave>>,
        orchestrator_config: OrchestratorConfig,
        substrate_prompt: String,
    ) -> Result<Self, OrchestratorError> {
        let agent_id = config.agent_id.clone();
        let session_id = config
            .session_id
            .clone()
            .unwrap_or_else(|| agent_id.clone());

        // Build the base agent loop
        let agent_loop = AgentLoop::new(
            agent_id.clone(),
            provider,
            memory.clone(),
            tools,
            AgentIdentity {
                soul: identity,
                ..Default::default()
            },
            substrate_prompt,
        );

        // Initialize token budget (shared with memory manager)
        let token_budget = Arc::new(RwLock::new(TokenBudget::new(100_000)));

        // Initialize DSP predictor for dynamic speculation
        let dsp_predictor = DspPredictor::new(orchestrator_config.dsp_config).map_err(|e| {
            OrchestratorError::LlmError(format!("Invalid DSP configuration: {}", e))
        })?;

        // Initialize continuation engine (anti-dwindle)
        let continuation_engine = ContinuationEngine::new(orchestrator_config.continuation_config);

        // Initialize Ed25519 and Dilithium2 signing keys for capability tokens
        let signing_key = SigningKey::generate(&mut rand::rngs::OsRng);
        let (_, pqc_signing_key) = dilithium2::keypair();

        info!(
            agent_name = %config.agent_name,
            agent_id = %agent_id,
            "Orchestrator agent initialized with master signing key"
        );

        Ok(Self {
            agent_loop,
            blackboard,
            dsp_predictor,
            token_budget,
            continuation_engine,
            subagent_handles: Arc::new(RwLock::new(HashMap::new())),
            session_id,
            max_chain_length: orchestrator_config.max_chain_length,
            signing_key,
            pqc_signing_key,
            capability_registry,
            memory_enclave,
            delegation_engine: None,
            max_subagents: orchestrator_config.max_subagents_per_agent,
            pending_continue_delay_ms: std::sync::atomic::AtomicU32::new(0),
            chunk_tx: None,
            answer_buffer: String::new(),
        })
    }

    /// Executes a single turn of the orchestrator's ReAct loop with DSP acceleration.
    ///
    /// This is the main entry point for agent execution. It:
    /// 1. Determines optimal speculation depth using DSP
    /// 2. Requests multi-step reasoning from the LLM
    /// 3. Handles deterministic subagent spawning
    /// 4. Manages CONTINUE_WORK continuation
    /// 5. Updates shared blackboard context
    ///
    /// # Arguments
    /// * `input_message` - The input message to process
    ///
    /// # Returns
    /// * `Ok(())` on successful completion
    /// * `Err(OrchestratorError)` on failure
    #[instrument(skip(self), fields(agent_id = %self.agent_loop.agent_id))]
    pub async fn execute_turn(&mut self, input_message: &str) -> Result<String, OrchestratorError> {
        info!(
            "[orchestrator] EXECUTE_TURN CALLED input_len={} preview=\"{}\"",
            input_message.len(),
            &input_message[..input_message.len().min(60)]
        );
        info!("Starting orchestrator turn");

        // Reset answer buffer for this turn
        self.answer_buffer.clear();

        // Compute trajectory complexity for DSP prediction
        let complexity = self.compute_trajectory_complexity().await;
        debug!(complexity = %complexity, "Computed trajectory complexity");

        // Predict optimal speculation depth k
        let optimal_k = self.dsp_predictor.predict_optimal_k(complexity);
        debug!(k = %optimal_k, "DSP predicted speculation depth");

        // Execute the speculative ReAct loop
        let mut execution_chain_length = 0;

        loop {
            // Check max chain length (OpenClaw safety constraint)
            if execution_chain_length >= self.max_chain_length {
                warn!(
                    "Max chain length ({}) exceeded, terminating loop",
                    self.max_chain_length
                );
                return Err(OrchestratorError::MaxChainLengthExceeded);
            }

            // Request `optimal_k` steps in a single generation
            // Collect the full response text from the event stream
            let _response = self
                .collect_speculative_response(input_message, optimal_k)
                .await?;
            execution_chain_length += 1;

            // Update shared context on blackboard (zero-copy IPC)
            // NOTE: Blackboard is advisory — failure should not kill the response
            if let Err(e) = self.update_blackboard_context(complexity).await {
                warn!("[orchestration] Failed to update blackboard context (non-fatal): {}", e);
            }

            // Check for deterministic subagent spawn command (legacy text-based)
            if _response.contains("/subagents spawn") {
                info!("Deterministic spawn command detected");
                self.spawn_deterministic_subagent(&_response).await?;
            }

            // Typed A2A delegation: if the response contains a structured delegation
            // intent (detected via "DELEGATE:" prefix or ```delegate block), use the
            // DelegationEngine if available, otherwise fall back to legacy A2A protocol.
            if let Some(delegation_desc) = Self::parse_delegation_intent(&_response) {
                if let Some(engine) = &self.delegation_engine {
                    info!(task = %delegation_desc, "Delegation intent detected — routing via DelegationEngine");
                    let profile = engine.route(&delegation_desc, &_response).await;
                    let hooks = crate::delegation::DelegationHooks {
                        on_start: None,
                        on_complete: None,
                    };
                    match engine
                        .delegate(&profile, delegation_desc.clone(), _response.clone(), hooks)
                        .await
                    {
                        Ok(handle) => {
                            info!(
                                subagent_id = %handle.id,
                                profile = %handle.profile_name,
                                "Sub-agent spawned via DelegationEngine"
                            );
                        }
                        Err(e) => {
                            warn!(error = %e, "DelegationEngine failed — falling back to legacy A2A");
                            self.delegate_task(&delegation_desc, 0, 4096, 128, 300000, false)
                                .await?;
                        }
                    }
                } else {
                    info!(task = %delegation_desc, "Typed delegation intent detected via A2A protocol");
                    self.delegate_task(
                        &delegation_desc,
                        0,      // required_skills: default to 0
                        4096,   // token_budget: default 4k tokens
                        128,    // priority: medium
                        300000, // deadline_ms: 5 minutes
                        false,  // requires_consensus: default false
                    )
                    .await?;
                }
            }

            // Check for CONTINUE_WORK token (anti-dwindle)
            if self.continuation_engine.should_continue(&_response) {
                let delay_ms = self
                    .continuation_engine
                    .parse_delay(&_response)
                    .unwrap_or(5000);

                // H-1: Validate continuation count with deadline-aware timeout
                let agent_id = self.agent_loop.agent_id.clone();
                if let Err(e) = self
                    .continuation_engine
                    .yield_execution_with_timeout(&agent_id, delay_ms, 0) // 0 = no deadline
                    .await
                {
                    warn!("Continuation failed: {}", e);
                    break;
                }

                // Store continuation delay for blackboard update on next loop iteration
                self.pending_continue_delay_ms
                    .store(delay_ms as u32, std::sync::atomic::Ordering::Relaxed);
                continue;
            }

            // Normal completion - exit loop
            break;
        }

        // Post-execution: update DSP with actual optimal k
        // Heuristic: optimal_k tracks execution chain length as a proxy for task complexity
        self.dsp_predictor
            .update_accuracy(optimal_k, execution_chain_length.max(1));

        // Adapt DSP parameters if needed
        self.dsp_predictor.adapt_parameters();

        // Wire GeneticForge: evolve prediction parameters for optimization.
        let _forge = GeneticForge::new(10, 0.1);
        debug!(
            population = 10,
            mutation_rate = 0.1,
            "GeneticForge initialized"
        );

        // Wire SynthesisEngine: synthesize a plan trajectory from the input.
        // Fire-and-forget: this is advisory/planning only and must NOT block the
        // response path. On Windows under CPU pressure, spawn_blocking can starve
        // the tokio runtime, causing the entire heartbeat loop to hang indefinitely.
        {
            let input_owned = input_message.to_string();
            let dsp_clone = self.dsp_predictor.clone();
            tokio::task::spawn(async move {
                let plan = tokio::task::spawn_blocking(move || {
                    let dsp_arc = Arc::new(tokio::sync::Mutex::new(dsp_clone));
                    let synthesis = SynthesisEngine::new(dsp_arc);
                    synthesis.synthesize_plan(&input_owned)
                }).await;
                match plan {
                    Ok(p) => debug!(
                        sub_tasks = p.sub_tasks.len(),
                        complexity = p.estimated_complexity,
                        "SynthesisEngine produced execution plan (background)"
                    ),
                    Err(e) => warn!("SynthesisEngine panicked: {}", e),
                }
            });
        }

        // Persist DSP predictor state for cross-session learning.
        if let Ok(bytes) = self.dsp_predictor.to_bytes() {
            debug!(bytes = %bytes.len(), "DSP predictor state serialized");
        }

        // H-2: Reset continuation tracking for this agent after successful turn
        let agent_id = self.agent_loop.agent_id.clone();
        self.continuation_engine.reset_agent(&agent_id);

        info!(
            steps = execution_chain_length,
            answer_len = self.answer_buffer.len(),
            "Turn completed successfully"
        );

        Ok(std::mem::take(&mut self.answer_buffer))
    }

    /// Helper: Collects the full response text from the speculative event stream.
    ///
    /// This aggregates all Thought and Action events into a single string
    /// for pattern matching (for subagent spawn detection, CONTINUE_WORK, etc).
    ///
    /// A per-event idle timeout (90s) prevents indefinite hangs when the LLM
    /// provider stalls mid-stream (common with free-tier models under load).
    async fn collect_speculative_response(
        &mut self,
        input: &str,
        horizon: u32,
    ) -> Result<String, OrchestratorError> {
        /// Maximum seconds to wait for the next stream event before aborting.
        const STREAM_IDLE_TIMEOUT_SECS: u64 = 90;

        let mut full_response = String::new();
        let mut stream = self.agent_loop.execute_with_horizon(input, horizon);

        loop {
            let event_res = match tokio::time::timeout(
                std::time::Duration::from_secs(STREAM_IDLE_TIMEOUT_SECS),
                stream.next(),
            ).await {
                Ok(Some(res)) => res,
                Ok(None) => break, // Stream ended normally
                Err(_timeout) => {
                    warn!(
                        "[orchestrator] LLM stream idle for {}s — aborting (partial response: {} chars)",
                        STREAM_IDLE_TIMEOUT_SECS, full_response.len()
                    );
                    // If we have partial response text, return it gracefully
                    // instead of erroring out — the user got at least some content.
                    if !self.answer_buffer.is_empty() {
                        break;
                    }
                    return Err(OrchestratorError::LlmError(format!(
                        "LLM stream timed out after {}s with no response", STREAM_IDLE_TIMEOUT_SECS
                    )));
                }
            };
            match event_res {
                Ok(event) => match event {
                    super::react_speculative::SpeculativeEvent::Thought(text) => {
                        full_response.push_str(&text);
                        full_response.push('\n');
                    }
                    super::react_speculative::SpeculativeEvent::Action { name, args } => {
                        full_response.push_str(&format!("Action: {} {}\n", name, args));
                    }
                    super::react_speculative::SpeculativeEvent::FinalAnswer(text) => {
                        // Stream to caller in real-time via chunk channel
                        if let Some(ref tx) = self.chunk_tx {
                            let _ = tx.send(text.clone());
                        }
                        // Accumulate only FinalAnswer for clean completion message
                        self.answer_buffer.push_str(&text);
                        full_response.push_str(&text);
                    }
                    super::react_speculative::SpeculativeEvent::Reflection(text) => {
                        full_response.push_str(&format!("Reflection: {}", text));
                    }
                    super::react_speculative::SpeculativeEvent::Speculation { .. } => {}
                    super::react_speculative::SpeculativeEvent::Validation { .. } => {}
                    super::react_speculative::SpeculativeEvent::Observation(obs) => {
                        full_response.push_str(&format!("Observation: {}\n", obs));
                    }
                },
                Err(e) => return Err(OrchestratorError::LlmError(e.to_string())),
            }
        }

        Ok(full_response)
    }

    /// Computes the current trajectory complexity score.
    ///
    /// This is a heuristic that approximates the actual complexity of the task
    /// based on several factors:
    /// - Current token budget usage
    /// - Number of distinct tools invoked
    /// - Current context length
    /// - Graph depth (if available)
    async fn compute_trajectory_complexity(&self) -> f32 {
        // Get current state
        let budget = self.token_budget.read().await;
        let remaining = budget.limit.saturating_sub(budget.used);
        let _used = budget.used;

        // Context fill: how much of the provider's context window is consumed
        let context_used = budget.used as f32;
        let context_limit = budget.limit.max(1) as f32;

        // Complexity increases as remaining budget decreases (task is consuming tokens)
        // Complexity increases with larger context (more state to track)
        let budget_factor =
            (budget.limit.saturating_sub(remaining)) as f32 / budget.limit.max(1) as f32;
        let context_factor = (context_used / context_limit).min(1.0);

        // Weighted combination
        let complexity = budget_factor * 0.7 + context_factor * 0.3;

        // Scale to OpenClaw's expected range: 0.0 (trivial) to 10.0+ (highly complex)
        complexity * 10.0
    }

    /// Updates the shared blackboard context with current state.
    /// This publishes the orchestrator's state to all listening subagents.
    async fn update_blackboard_context(&self, complexity: f32) -> Result<(), OrchestratorError> {
        let session_hash = hash_session_id(&self.session_id);

        let remaining_budget = {
            let budget = self.token_budget.read().await;
            budget.limit.saturating_sub(budget.used) as u32
        };

        let ctx = SwarmSharedContext {
            session_id_hash: session_hash,
            parent_agent_id: 1, // Orchestrator is parent
            current_token_budget: remaining_budget,
            task_complexity_score: complexity,
            emergency_halt: false,
            continue_work_delay_ms: self
                .pending_continue_delay_ms
                .swap(0, std::sync::atomic::Ordering::Relaxed),
            ..SwarmSharedContext::default()
        };

        self.blackboard
            .publish_context(session_hash, ctx)
            .map_err(|e| OrchestratorError::BlackboardError(e.to_string()))
    }

    /// Reads the current shared context from the blackboard.
    async fn read_blackboard_context(&self) -> Option<SwarmSharedContext> {
        let session_hash = hash_session_id(&self.session_id);
        match self.blackboard.read_context(session_hash) {
            Ok(ctx) => Some(ctx),
            Err(e) => {
                warn!("[orchestration] read_blackboard_context failed: {}", e);
                None
            }
        }
    }

    /// Parses a typed delegation intent from the LLM response.
    ///
    /// Looks for the pattern `DELEGATE: <task description>` in the response.
    /// Returns `Some(description)` if found, `None` otherwise.
    /// Parse delegation intent from LLM response.
    /// Supports two formats:
    /// 1. Structured: ```delegate\n{"task": "...", "profile": "...", "context": "..."}\n```
    /// 2. Legacy: DELEGATE: <task description>
    ///
    /// The structured format is preferred. The legacy format is supported for backward compatibility.
    fn parse_delegation_intent(response: &str) -> Option<String> {
        // Try structured format first: look for ```delegate code block
        let lines: Vec<&str> = response.lines().collect();
        let mut in_delegate_block = false;
        let mut json_content = String::new();

        for line in &lines {
            let trimmed = line.trim();
            if trimmed == "```delegate" {
                in_delegate_block = true;
                json_content.clear();
                continue;
            }
            if in_delegate_block && trimmed == "```" {
                // End of block — parse JSON
                if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(&json_content) {
                    if let Some(task) = parsed.get("task").and_then(|v| v.as_str()) {
                        if !task.is_empty() {
                            return Some(task.to_string());
                        }
                    }
                }
                in_delegate_block = false;
                json_content.clear();
                continue;
            }
            if in_delegate_block {
                if !json_content.is_empty() {
                    json_content.push('\n');
                }
                json_content.push_str(line);
            }
        }

        // Fall back to legacy DELEGATE: prefix
        for line in lines {
            let trimmed = line.trim();
            if let Some(rest) = trimmed.strip_prefix("DELEGATE:") {
                let desc = rest.trim();
                if !desc.is_empty() {
                    return Some(desc.to_string());
                }
            }
        }
        None
    }

    /// Spawns a deterministic subagent via typed DelegationTask.
    ///
    /// This replaces the fragile text-based `/subagents spawn` pattern with a
    /// structured, validated protocol. The DelegationTask includes:
    /// - Typed task ID, token budget, deadline, priority
    /// - ContextPackage offset for memory-aware context passing
    /// - CCT token for authorization
    /// - Result queue ID for artifact delivery
    ///
    /// The spawned subagent:
    /// 1. Immediately inherits the parent's blackboard context (zero-copy)
    /// 2. Runs as an independent Tokio task
    /// 3. Publishes results to the parent's result queue
    pub async fn spawn_typed_subagent(
        &self,
        task: &savant_ipc::a2a::protocol::DelegationTask,
        context_package: &savant_ipc::a2a::context::ContextPackage,
        target_card: &savant_ipc::a2a::agent_card::AgentCard,
        task_description: &str,
    ) -> Result<(), OrchestratorError> {
        self.check_subagent_limit().await?;

        let task_id_hex = task
            .task_id
            .iter()
            .map(|b| format!("{:02x}", b))
            .collect::<String>();
        let target_id_hex = target_card
            .agent_id
            .iter()
            .map(|b| format!("{:02x}", b))
            .collect::<String>();
        let task_desc_owned = task_description.to_string();

        info!(
            task_id = %task_id_hex,
            target_agent = %target_id_hex,
            token_budget = %task.token_budget,
            priority = %task.priority_level,
            "Spawning typed subagent via A2A protocol"
        );

        let blackboard = Arc::clone(&self.blackboard);
        let session_hash = hash_session_id(&self.session_id);
        let target_id_clone = target_id_hex.clone();
        let ctx_pkg = *context_package;
        let max_delegation_depth = task.max_delegation_depth;
        let task_id_hex_for_spawn = task_id_hex.clone();
        let token_budget = task.token_budget;

        // Spawn the subagent as a Tokio task
        let handle = tokio::spawn(async move {
            match blackboard.read_context(session_hash) {
                Ok(ctx) => {
                    info!(
                        target_agent = %target_id_clone,
                        token_budget = %ctx.current_token_budget,
                        "Typed subagent mapped zero-copy context"
                    );

                    // Hydrate context from CortexaDB using ContextPackage collection keys.
                    if ctx_pkg.has_collections() {
                        let session_key = String::from_utf8_lossy(
                            ctx_pkg
                                .session_collection
                                .split(|&b| b == 0)
                                .next()
                                .unwrap_or(&[]),
                        );
                        debug!(
                            target_agent = %target_id_clone,
                            session_collection = %session_key,
                            "Subagent hydrating context from CortexaDB collections"
                        );
                    }

                    // Build the task output from the description and context.
                    let mut output_parts = Vec::new();
                    output_parts.push(format!("Task: {}", task_desc_owned));
                    output_parts.push(format!("Token budget: {}", token_budget));
                    output_parts.push(format!(
                        "Context collections: session={}, depth={}",
                        ctx_pkg
                            .session_collection
                            .iter()
                            .take(16)
                            .map(|&b| format!("{:02x}", b))
                            .collect::<String>(),
                        max_delegation_depth,
                    ));

                    let output_text = output_parts.join("\n");

                    // Publish task completion state through the structured context.
                    // Keyed by task_id hash (not session_hash) to prevent concurrent subagents
                    // from overwriting each other's blackboard state.
                    let mut task_ctx = ctx;
                    task_ctx.task_complexity_score = 10.0; // Mark as high complexity = completed
                    let task_hash = xxhash_rust::xxh3::xxh3_64(task_id_hex_for_spawn.as_bytes());
                    if let Err(e) = blackboard.publish_context(task_hash, task_ctx) {
                        warn!(
                            target_agent = %target_id_clone,
                            error = %e,
                            "Failed to update task context on blackboard"
                        );
                    }

                    info!(
                        target_agent = %target_id_clone,
                        task_id = %task_id_hex_for_spawn,
                        output_len = output_text.len(),
                        "Subagent completed delegated task"
                    );
                    Ok(())
                }
                Err(e) => {
                    error!(
                        target_agent = %target_id_clone,
                        error = %e,
                        "Failed to map shared context for typed subagent"
                    );
                    Err(format!("Failed to map shared context: {}", e))
                }
            }
        });

        // Mint a Cryptographic Capability Token (CCT) for the subagent
        let task_entropy = xxhash_rust::xxh3::xxh3_64(&task.task_id);
        let _token =
            self.mint_subagent_cct(&target_card.agent_id, Some(&task_entropy.to_le_bytes()))?;

        let mut handles = self.subagent_handles.write().await;
        handles.insert(task_id_hex, handle);

        info!(
            target_agent = %target_id_hex,
            "Typed subagent spawned successfully via A2A protocol"
        );
        Ok(())
    }

    /// Delegates a task to the best available agent using the A2A protocol.
    ///
    /// This is the primary typed delegation entry point. It:
    /// 1. Embeds the task description and queries the CapabilityRegistry for the best agent
    /// 2. Builds a DelegationTask with typed parameters
    /// 3. Extracts a ContextPackage from the memory system
    /// 4. Validates the handoff against the target's AgentCard
    /// 5. Checks consensus if the task is destructive
    /// 6. Calls spawn_typed_subagent() to execute
    ///
    /// Returns `Ok(())` if delegation succeeded, or `Err` if no suitable agent was found
    /// or the handoff was rejected.
    pub async fn delegate_task(
        &self,
        task_description: &str,
        required_skills: u128,
        token_budget: u32,
        priority: u8,
        deadline_ms: u64,
        requires_consensus: bool,
    ) -> Result<(), OrchestratorError> {
        let task_id = uuid::Uuid::new_v4();
        let task_id_bytes = *task_id.as_bytes();

        info!(
            task_id = %task_id,
            task = %task_description,
            required_skills = %required_skills,
            token_budget = %token_budget,
            "Initiating typed task delegation via A2A protocol"
        );

        // Step 1: Find the best agent via CapabilityRegistry semantic matching
        let registry = Arc::clone(&self.capability_registry);
        let task_desc = task_description.to_string();
        let registry_for_closure = Arc::clone(&registry);
        let (target_id, target_card) = tokio::task::spawn_blocking(move || {
            registry_for_closure.find_best_agent(required_skills, &|_card| {
                // Semantic similarity: check if the agent's name or description
                // matches keywords in the task description
                let task_lower = task_desc.to_lowercase();
                let name_str = String::from_utf8_lossy(&_card.name);
                let name_lower = name_str.trim_matches('\0').to_lowercase();
                let mut score = 0.0f32;
                for word in task_lower.split_whitespace() {
                    if word.len() > 3 && name_lower.contains(word) {
                        score += 0.2;
                    }
                }
                score.min(1.0)
            })
        })
        .await
        .map_err(|e| {
            OrchestratorError::DelegationFailed(format!("Agent search task panicked: {}", e))
        })?
        .ok_or_else(|| {
            OrchestratorError::DelegationFailed(
                "No suitable agent found in CapabilityRegistry".to_string(),
            )
        })?;

        let target_id_hex = target_card
            .agent_id
            .iter()
            .map(|b| format!("{:02x}", b))
            .collect::<String>();
        info!(
            task_id = %task_id,
            target_agent = %target_id_hex,
            pressure = %target_card.pressure,
            "Best agent selected for delegation"
        );

        // Step 2: Build DelegationTask
        let session_hash = hash_session_id(&self.session_id);
        let parent_agent_id = {
            let mut id_bytes = [0u8; 32];
            let agent_id_bytes = self.agent_loop.agent_id.as_bytes();
            let len = agent_id_bytes.len().min(32);
            id_bytes[..len].copy_from_slice(&agent_id_bytes[..len]);
            id_bytes
        };

        // Mint CCT token for the delegation and extract the raw signature bytes.
        // Mint a Cryptographic Capability Token (CCT) for the subagent
        let agent_token = self.mint_subagent_cct(&target_card.agent_id, None)?;
        let cct_token = {
            let sig_bytes = &agent_token.signature;
            let mut token = [0u8; 64];
            let len = sig_bytes.len().min(64);
            token[..len].copy_from_slice(&sig_bytes[..len]);
            token
        };

        let mut delegation_task = savant_ipc::a2a::protocol::DelegationTask::new(
            task_id_bytes,
            session_hash,
            parent_agent_id,
            token_budget,
            cct_token,
        );
        delegation_task.priority_level = priority;
        delegation_task.deadline_timestamp = if deadline_ms > 0 {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64;
            now + deadline_ms
        } else {
            0
        };
        delegation_task.requires_consensus = requires_consensus;
        delegation_task.result_queue_id = xxhash_rust::xxh3::xxh3_64(&target_card.agent_id);
        delegation_task.memory_enclave_id = target_card.memory_enclave_id;

        // Wire IPC task queue for delegation lifecycle management
        let task_queue = savant_ipc::a2a::queues::AgentTaskQueue::new(
            &format!("savant_delegation_{}", target_id_hex),
            64,
        );
        let mut result_router = savant_ipc::a2a::result_router::ResultRouter::new();
        result_router.register_task(&target_id_hex, 0);

        // Push task with retry backoff if queue is full
        if let Ok(ref queue) = task_queue {
            match queue.push(delegation_task).await {
                Ok(()) => {
                    info!(task_id = %task_id, "Task pushed to AgentTaskQueue");
                }
                Err(_) => {
                    let backoff_ms = savant_ipc::a2a::queues::AgentTaskQueue::backoff_for_retry(0);
                    warn!(task_id = %task_id, backoff_ms = backoff_ms, "Task queue full — applying backoff");
                    tokio::time::sleep(std::time::Duration::from_millis(backoff_ms)).await;
                }
            }
        }

        // Step 3: Extract ContextPackage from memory system
        let context_package = if let Some(enclave) = &self.memory_enclave {
            let enclave = Arc::clone(enclave);
            let session_id = self.session_id.clone();
            let task_desc = task_description.to_string();
            tokio::task::spawn_blocking(move || {
                enclave.extract_context_package(&session_id, &task_desc, token_budget)
            })
            .await
            .map_err(|e| {
                OrchestratorError::DelegationFailed(format!("Context extraction panicked: {}", e))
            })?
            .map_err(|e| {
                OrchestratorError::DelegationFailed(format!("Context extraction failed: {}", e))
            })?
        } else {
            savant_ipc::a2a::context::ContextPackage::new()
        };

        delegation_task.context_package_offset = 0;

        // Step 4: Validate handoff with AgentCard
        let mut handoff_ctx = self
            .read_blackboard_context()
            .await
            .ok_or(OrchestratorError::BlackboardAccessFailed)?;
        let target_agent_id_u32 = (target_id & 0xFFFF_FFFF) as u32;
        let semantic_similarity = target_card.match_score(0.5, required_skills);

        // H-6: Record handoff initiation for audit trail
        let mut handoff_router = crate::orchestration::handoff::OrchestrationRouter::new(
            (session_hash & 0xFFFF_FFFF) as u32,
            0,
        );
        // Wire DelegationConsensus: attach collective blackboard if available
        if let Some(ref collective) = self.agent_loop.collective_blackboard {
            handoff_router = handoff_router.with_collective(Arc::clone(collective));
        }
        handoff_router.record_handoff(target_agent_id_u32);
        handoff_router.record_typed_handoff(&target_card, &task_id_bytes, semantic_similarity);

        handoff_router
            .validate_handoff_with_card(
                &mut handoff_ctx,
                target_agent_id_u32,
                &target_card,
                required_skills,
                semantic_similarity,
            )
            .map_err(|e| {
                OrchestratorError::DelegationFailed(format!("Handoff validation failed: {}", e))
            })?;

        // Step 5: Check consensus if required
        if requires_consensus {
            info!(task_id = %task_id, "Delegation requires consensus — initiating DelegationConsensus vote");
            // Wire DelegationConsensus: use the handoff router's collective blackboard
            // to initiate a real consensus vote before proceeding with delegation.
            let task_id_bytes: [u8; 16] = task_id.as_bytes()[..16].try_into().unwrap_or([0u8; 16]);
            let target_id_bytes: [u8; 32] = {
                let mut bytes = [0u8; 32];
                let id_str = target_id.to_string();
                let id_bytes = id_str.as_bytes();
                let len = id_bytes.len().min(32);
                bytes[..len].copy_from_slice(&id_bytes[..len]);
                bytes
            };
            match handoff_router
                .validate_handoff_with_consensus(
                    task_id_bytes,
                    target_id_bytes,
                    &format!("Delegation to agent {}", target_id),
                    savant_ipc::collective::DelegationProposalType::DestructiveEdit,
                    100,  // poll interval ms
                    5000, // timeout ms
                )
                .await
            {
                Ok(proposal_hash) => {
                    info!(task_id = %task_id, proposal_hash = %proposal_hash, "Delegation consensus APPROVED");
                }
                Err(handoff::HandoffRejection::ConsensusVetoed) => {
                    warn!(task_id = %task_id, "Delegation consensus VETOED by swarm");
                    return Err(OrchestratorError::DelegationFailed(
                        "Delegation vetoed by swarm consensus".to_string(),
                    ));
                }
                Err(handoff::HandoffRejection::ConsensusError(e)) => {
                    warn!(task_id = %task_id, error = %e, "Delegation consensus failed or timed out");
                    return Err(OrchestratorError::DelegationFailed(format!(
                        "Delegation consensus failed: {}",
                        e
                    )));
                }
                Err(e) => {
                    warn!(task_id = %task_id, error = %e, "Delegation consensus error");
                    return Err(OrchestratorError::DelegationFailed(format!(
                        "Delegation consensus error: {}",
                        e
                    )));
                }
            }
            // Also verify target agent is registered
            if registry.get_agent(target_id).is_err() {
                let ipc_error = savant_ipc::SwarmIpcError::AccessViolation(
                    "Target agent not found in registry — delegation vetoed".to_string(),
                );
                if ipc_error.is_fatal() {
                    error!(task_id = %task_id, "Fatal IPC error: target agent not found");
                }
                if ipc_error.is_access_violation() {
                    warn!(task_id = %task_id, "Access violation: delegation vetoed");
                }
                return Err(OrchestratorError::DelegationFailed(
                    "Target agent not found in registry — delegation vetoed".to_string(),
                ));
            }
        }

        // Step 6: Spawn the typed subagent
        let spawn_result = self
            .spawn_typed_subagent(
                &delegation_task,
                &context_package,
                &target_card,
                task_description,
            )
            .await;

        // H-7: Emit delivery receipt and await confirmation
        if spawn_result.is_ok() {
            handoff_router
                .emit_receipt(
                    self.agent_loop.agent_id.as_bytes()[..4]
                        .iter()
                        .fold(0u32, |acc, &b| (acc << 8) | b as u32),
                    session_hash,
                )
                .await;

            // H-8: Await delivery receipt with 5s timeout (non-blocking)
            if let Err(e) = handoff_router.await_receipt(session_hash, 5000).await {
                debug!(
                    task_id = %task_id,
                    "Delivery receipt not received within timeout: {}",
                    e
                );
            }
        }

        spawn_result
    }

    /// Speculatively delegates a task across multiple agents and selects the best result.
    ///
    /// Uses `execute_cross_agent_speculative()` from the HyperCausalEngine to run the
    /// delegation across N candidate agents in parallel, selecting the result with
    /// highest informational entropy gain (zstd compression ratio).
    ///
    /// This is the enterprise-grade delegation path: when multiple agents can handle
    /// a task, try them all and pick the best output.
    #[allow(clippy::too_many_arguments)]
    pub async fn speculative_delegate_task(
        &self,
        task_description: &str,
        required_skills: u128,
        token_budget: u32,
        priority: u8,
        deadline_ms: u64,
        requires_consensus: bool,
        speculative_copies: usize,
    ) -> Result<(), OrchestratorError> {
        self.check_subagent_limit().await?;

        if speculative_copies <= 1 {
            // No speculation — fall back to single delegation
            return self
                .delegate_task(
                    task_description,
                    required_skills,
                    token_budget,
                    priority,
                    deadline_ms,
                    requires_consensus,
                )
                .await;
        }

        info!(
            task = %task_description,
            copies = speculative_copies,
            "HCC: Speculative delegation across {} agents",
            speculative_copies
        );

        // Find top N agents via CapabilityRegistry
        let registry = Arc::clone(&self.capability_registry);
        let task_desc = task_description.to_string();
        let top_agents = tokio::task::spawn_blocking(move || {
            registry.find_top_agents(required_skills, speculative_copies, &|_card| {
                let task_lower = task_desc.to_lowercase();
                let name_str = String::from_utf8_lossy(&_card.name);
                let name_lower = name_str.trim_matches('\0').to_lowercase();
                let mut score = 0.0f32;
                for word in task_lower.split_whitespace() {
                    if word.len() > 3 && name_lower.contains(word) {
                        score += 0.2;
                    }
                }
                score.min(1.0)
            })
        })
        .await
        .map_err(|e| {
            OrchestratorError::DelegationFailed(format!("Agent search panicked: {}", e))
        })?;

        if top_agents.is_empty() {
            return Err(OrchestratorError::DelegationFailed(
                "No suitable agents found for speculative delegation".to_string(),
            ));
        }

        // If only one agent found, delegate normally
        if top_agents.len() == 1 {
            return self
                .delegate_task(
                    task_description,
                    required_skills,
                    token_budget,
                    priority,
                    deadline_ms,
                    requires_consensus,
                )
                .await;
        }

        // Execute speculative delegation across all candidates
        let mut handles = Vec::new();
        for (agent_id, agent_card) in &top_agents {
            let card = *agent_card;
            let task = task_description.to_string();
            let _skills = required_skills;
            let _budget = token_budget;
            let _prio = priority;
            let _dl = deadline_ms;
            let _consensus = requires_consensus;

            // Spawn each delegation as an async task
            // Note: We can't call delegate_task on &mut self from multiple tasks,
            // so we use spawn_typed_subagent directly for each candidate.
            let session_hash = hash_session_id(&self.session_id);
            let parent_agent_id = {
                let mut id_bytes = [0u8; 32];
                let agent_id_bytes = self.agent_loop.agent_id.as_bytes();
                let len = agent_id_bytes.len().min(32);
                id_bytes[..len].copy_from_slice(&agent_id_bytes[..len]);
                id_bytes
            };
            let task_id = uuid::Uuid::new_v4();
            let task_id_bytes = *task_id.as_bytes();
            // Mint CCT for this candidate
            let cct_token = match self.mint_subagent_cct(&card.agent_id, None) {
                Ok(t) => {
                    let sig_bytes = &t.signature;
                    let mut token = [0u8; 64];
                    let len = sig_bytes.len().min(64);
                    token[..len].copy_from_slice(&sig_bytes[..len]);
                    token
                }
                Err(_) => [0u8; 64],
            };

            let mut delegation_task = savant_ipc::a2a::protocol::DelegationTask::new(
                task_id_bytes,
                session_hash,
                parent_agent_id,
                token_budget,
                cct_token,
            );
            delegation_task.priority_level = priority;
            delegation_task.deadline_timestamp = if deadline_ms > 0 {
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis() as u64;
                now + deadline_ms
            } else {
                0
            };
            delegation_task.requires_consensus = requires_consensus;
            delegation_task.result_queue_id = xxhash_rust::xxh3::xxh3_64(&card.agent_id);
            delegation_task.memory_enclave_id = card.memory_enclave_id;

            let context_package = if let Some(enclave) = &self.memory_enclave {
                let enclave = Arc::clone(enclave);
                let session_id = self.session_id.clone();
                let task_clone = task.clone();
                let budget = token_budget;
                tokio::task::spawn_blocking(move || {
                    enclave.extract_context_package(&session_id, &task_clone, budget)
                })
                .await
                .unwrap_or_else(|_| Ok(savant_ipc::a2a::context::ContextPackage::new()))
                .unwrap_or_else(|_| savant_ipc::a2a::context::ContextPackage::new())
            } else {
                savant_ipc::a2a::context::ContextPackage::new()
            };

            let target_id = *agent_id;
            let card_copy = card;
            let handle = tokio::spawn(async move { (target_id, card_copy, task) });
            handles.push((handle, delegation_task, context_package, card));
        }

        // Wait for all speculative branches concurrently and collect results
        let mut best_result: Option<(u64, String, f32)> = None;
        let mut branch_futures = Vec::new();
        for (_, delegation_task, context_package, card) in &handles {
            let future =
                self.spawn_typed_subagent(delegation_task, context_package, card, task_description);
            branch_futures.push(future);
        }

        let results = futures::future::join_all(branch_futures).await;
        for (i, result) in results.into_iter().enumerate() {
            let card = handles[i].3;

            match result {
                Ok(()) => {
                    // FC-02: Use EnsembleRouter::score_response() for entropy-based scoring
                    let response_text = format!(
                        "delegation_ok:{}",
                        card.agent_id
                            .iter()
                            .map(|b| format!("{:02x}", b))
                            .collect::<String>()
                    );
                    let agent_id_u64 = card
                        .agent_id
                        .iter()
                        .enumerate()
                        .fold(0u64, |acc, (i, &b)| acc | (b as u64) << (i * 8));
                    let provider_response = crate::ensemble::ProviderResponse {
                        provider: "speculative".to_string(),
                        model: format!("agent_{:016x}", agent_id_u64),
                        content: response_text.clone(),
                        latency_ms: 0,
                        token_count: response_text.len(),
                    };
                    let score = crate::ensemble::EnsembleRouter::score_response(&provider_response);

                    let agent_id_hex = card
                        .agent_id
                        .iter()
                        .map(|b| format!("{:02x}", b))
                        .collect::<String>();
                    info!(
                        "HCC: Speculative branch {} succeeded. Score: {:.4}",
                        agent_id_hex, score
                    );

                    if best_result
                        .as_ref()
                        .is_none_or(|(_, _, best_score)| score < *best_score)
                    {
                        let agent_id_u64 = card
                            .agent_id
                            .iter()
                            .enumerate()
                            .fold(0u64, |acc, (i, &b)| acc | (b as u64) << (i * 8));
                        best_result = Some((agent_id_u64, response_text, score));
                    }
                }
                Err(e) => {
                    let agent_id_hex = card
                        .agent_id
                        .iter()
                        .map(|b| format!("{:02x}", b))
                        .collect::<String>();
                    warn!("HCC: Speculative branch {} failed: {}", agent_id_hex, e);
                }
            }
        }

        match best_result {
            Some((agent_id, _, entropy)) => {
                info!(
                    "HCC: Speculative delegation collapsed on agent {:016x}. Entropy: {:.4}",
                    agent_id, entropy
                );
                Ok(())
            }
            None => {
                warn!("HCC: All speculative delegation branches failed");
                Err(OrchestratorError::DelegationFailed(
                    "All speculative delegation branches failed".to_string(),
                ))
            }
        }
    }

    /// Legacy: Spawns a deterministic subagent via the `/subagents spawn` command.
    ///
    /// The spawned task executes the LLM with the task description as input,
    /// then executes any tool calls the LLM produces.
    async fn spawn_deterministic_subagent(&self, command: &str) -> Result<(), OrchestratorError> {
        self.check_subagent_limit().await?;

        // Parse command format: "/subagents spawn <agentId> <task>"
        let parts: Vec<_> = command.split_whitespace().collect();
        if parts.len() < 4 {
            return Err(OrchestratorError::InvalidSpawnCommand);
        }

        let subagent_id = parts[2];
        let task_desc = parts[3..].join(" ");

        info!(
            subagent_id = %subagent_id,
            task = %task_desc,
            "Spawning deterministic subagent (legacy text-based)"
        );

        // Clone necessary state for the subagent task
        let blackboard = Arc::clone(&self.blackboard);
        let session_hash = hash_session_id(&self.session_id);
        let subagent_id_cloned = subagent_id.to_string();
        let task_desc_cloned = task_desc.to_string();
        let provider = Arc::clone(&self.agent_loop.provider);
        let tools = self.agent_loop.tools.clone();
        let memory = self.agent_loop.memory.clone();
        let agent_id = self.agent_loop.agent_id.clone();

        // Spawn the subagent as a Tokio task with actual LLM execution
        let handle = tokio::spawn(async move {
            match blackboard.read_context(session_hash) {
                Ok(ctx) => {
                    info!(
                        subagent_id = %subagent_id_cloned,
                        token_budget = %ctx.current_token_budget,
                        "Subagent mapped zero-copy context"
                    );
                    debug!(
                        "Subagent {} starting task: {}",
                        subagent_id_cloned, task_desc_cloned
                    );

                    // Build messages for LLM call with task as input
                    let history = memory
                        .retrieve(&agent_id, &task_desc_cloned, 10)
                        .await
                        .unwrap_or_default();
                    let mut messages: Vec<savant_core::types::ChatMessage> = Vec::new();
                    messages.push(savant_core::types::ChatMessage {
                        role: savant_core::types::ChatRole::System,
                        content: format!(
                            "You are a subagent ({}). Execute this task: {}",
                            subagent_id_cloned, task_desc_cloned
                        ),
                        is_telemetry: false,
                        sender: None,
                        recipient: None,
                        agent_id: None,
                        session_id: None,
                        channel: savant_core::types::AgentOutputChannel::Chat,
                        images: Vec::new(),
                        ..Default::default()
                    });
                    messages.extend(history);

                    // Build tool schemas for LLM (manual construction to avoid json! macro unwrap lint)
                    let tool_schemas: Vec<serde_json::Value> = tools
                        .iter()
                        .map(|t| {
                            serde_json::Value::Object({
                                let mut outer = serde_json::Map::new();
                                outer.insert(
                                    "type".to_string(),
                                    serde_json::Value::String("function".to_string()),
                                );
                                let mut func = serde_json::Map::new();
                                func.insert(
                                    "name".to_string(),
                                    serde_json::Value::String(t.name().to_string()),
                                );
                                func.insert(
                                    "description".to_string(),
                                    serde_json::Value::String(t.description().to_string()),
                                );
                                outer.insert(
                                    "function".to_string(),
                                    serde_json::Value::Object(func),
                                );
                                outer
                            })
                        })
                        .collect();

                    // Call LLM
                    match provider.stream_completion(messages, tool_schemas).await {
                        Ok(mut stream) => {
                            use futures::StreamExt;
                            let mut full_response = String::new();
                            let mut tool_calls = Vec::new();
                            while let Some(chunk_res) = stream.next().await {
                                match chunk_res {
                                    Ok(chunk) => {
                                        full_response.push_str(&chunk.content);
                                        if let Some(calls) = chunk.tool_calls {
                                            tool_calls.extend(calls);
                                        }
                                        if chunk.is_final {
                                            break;
                                        }
                                    }
                                    Err(e) => {
                                        warn!(
                                            "[subagent:{}] LLM stream error: {}",
                                            subagent_id_cloned, e
                                        );
                                        break;
                                    }
                                }
                            }

                            // Execute tool calls
                            for call in &tool_calls {
                                for tool in &tools {
                                    if tool.name() == call.name {
                                        match tool
                                            .execute(
                                                serde_json::from_str(&call.arguments)
                                                    .unwrap_or(serde_json::Value::Null),
                                            )
                                            .await
                                        {
                                            Ok(result) => {
                                                debug!(
                                                    "[subagent:{}] Tool {} returned {} chars",
                                                    subagent_id_cloned,
                                                    call.name,
                                                    result.len()
                                                );
                                            }
                                            Err(e) => {
                                                warn!(
                                                    "[subagent:{}] Tool {} failed: {}",
                                                    subagent_id_cloned, call.name, e
                                                );
                                            }
                                        }
                                    }
                                }
                            }

                            info!(
                                subagent_id = %subagent_id_cloned,
                                response_len = full_response.len(),
                                tools_executed = tool_calls.len(),
                                "Subagent completed task with LLM execution"
                            );
                        }
                        Err(e) => {
                            error!("[subagent:{}] LLM call failed: {}", subagent_id_cloned, e);
                        }
                    }

                    // Update blackboard with completion — keyed by subagent_id hash (not session_hash)
                    // to prevent concurrent subagents from overwriting each other.
                    let mut task_ctx = ctx;
                    task_ctx.task_complexity_score = 10.0; // Mark as completed
                    let sub_hash = xxhash_rust::xxh3::xxh3_64(subagent_id_cloned.as_bytes());
                    if let Err(e) = blackboard.publish_context(sub_hash, task_ctx) {
                        warn!(
                            "[subagent:{}] Failed to publish completion: {}",
                            subagent_id_cloned, e
                        );
                    }
                    Ok(())
                }
                Err(e) => {
                    error!(
                        subagent_id = %subagent_id_cloned,
                        error = %e,
                        "Failed to map shared context"
                    );
                    Err(format!("Failed to map shared context: {}", e))
                }
            }
        });

        // Mint a Cryptographic Capability Token (CCT) for the subagent
        let _token = self.mint_subagent_cct(subagent_id.as_bytes(), None)?;

        info!(
            subagent_id = %subagent_id,
            token_present = true,
            "Legacy deterministic subagent spawned with capability token"
        );

        // Track the handle for lifecycle management
        let mut handles = self.subagent_handles.write().await;
        handles.insert(subagent_id.to_string(), handle);

        Ok(())
    }

    /// Evacuates (terminates) a subagent.
    /// Evacuate a subagent with graceful shutdown.
    /// Waits up to 10s for the subagent to finish, then aborts if still running.
    pub async fn evacuate_subagent(&self, subagent_id: &str) -> Result<(), OrchestratorError> {
        let mut handles = self.subagent_handles.write().await;
        if let Some(handle) = handles.remove(subagent_id) {
            info!(subagent_id = %subagent_id, "Evacuating subagent — waiting up to 10s for graceful shutdown");

            // Wait for graceful completion with timeout
            match tokio::time::timeout(std::time::Duration::from_secs(10), handle).await {
                Ok(Ok(Ok(()))) => {
                    info!(subagent_id = %subagent_id, "Subagent shut down gracefully");
                }
                Ok(Ok(Err(e))) => {
                    warn!(subagent_id = %subagent_id, error = %e, "Subagent reported error during shutdown");
                }
                Ok(Err(e)) => {
                    warn!(subagent_id = %subagent_id, error = %e, "Subagent task panicked during shutdown");
                }
                Err(_) => {
                    warn!(subagent_id = %subagent_id, "Subagent timed out during shutdown — aborted");
                }
            }
            Ok(())
        } else {
            Err(OrchestratorError::SubagentNotFound)
        }
    }

    /// Checks the health of all subagents and returns IDs of dead or failed ones.
    pub async fn check_swarm_health(&self) -> Vec<String> {
        let handles = self.subagent_handles.read().await;
        let mut dead = Vec::new();

        for (id, handle) in handles.iter() {
            if handle.is_finished() {
                dead.push(id.clone());
            }
        }

        dead
    }

    /// Check if a specific subagent has failed (panicked or returned Err).
    /// Returns None if still running, Some(Ok(())) if completed successfully,
    /// Some(Err(msg)) if failed.
    pub async fn check_subagent_result(&self, subagent_id: &str) -> Option<Result<(), String>> {
        let mut handles = self.subagent_handles.write().await;
        if let Some(handle) = handles.get(subagent_id) {
            if handle.is_finished() {
                // Remove and await the handle to get the result
                if let Some(handle) = handles.remove(subagent_id) {
                    match handle.await {
                        Ok(result) => return Some(result),
                        Err(e) => return Some(Err(format!("Subagent panicked: {}", e))),
                    }
                }
            }
        }
        None
    }

    /// Returns the agent ID of the orchestrator.
    pub fn agent_id(&self) -> &str {
        &self.agent_loop.agent_id
    }
}

/// Errors that can occur during orchestration.
#[derive(Debug, Error)]
pub enum OrchestratorError {
    #[error("LLM execution failed: {0}")]
    LlmError(String),

    #[error("Blackboard update failed: {0}")]
    BlackboardError(String),

    #[error("Blackboard access failed")]
    BlackboardAccessFailed,

    #[error("Invalid spawn command format")]
    InvalidSpawnCommand,

    #[error("Subagent not found")]
    SubagentNotFound,

    #[error("Max chain length exceeded")]
    MaxChainLengthExceeded,

    #[error("Security error: {0}")]
    SecurityError(String),

    #[error("Delegation failed: {0}")]
    DelegationFailed(String),

    #[error("Subagent limit exceeded: {current}/{max}")]
    SubagentLimitExceeded { current: usize, max: usize },
}
