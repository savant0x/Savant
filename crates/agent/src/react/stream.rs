// SAFETY: All `clippy::disallowed_methods` violations in this file originate from
// the `serde_json::json!()` macro, which internally uses `.unwrap()` on
// compile-time-validated JSON literals. A malformed JSON literal would be a
// compile error, making the panic path statically unreachable.
#![allow(clippy::disallowed_methods)]

use crate::react::{AgentEvent, AgentLoop};
use futures::stream::{FuturesUnordered, Stream, StreamExt};
use regex::Regex;
use savant_core::error::SavantError;
use savant_core::traits::MemoryBackend;
use savant_core::types::{ChatMessage, ChatRole};
use savant_core::utils::parsing;
use savant_core::utils::token_count;
use savant_panopticon::replay::ReplayEventType;
use std::collections::HashSet;
use std::pin::Pin;
use std::sync::{Arc, LazyLock};
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};

/// Pre-compiled redaction patterns for security-sensitive tokens.
/// SAFETY: All regex patterns are hardcoded string literals validated at compile time.
/// The `.expect()` calls on `Regex::new()` can never fail because the patterns are
/// static constants (not user-provided), making the panic path unreachable.
#[allow(clippy::disallowed_methods)]
static REDACT_PATTERNS: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    vec![
        Regex::new(r"(?i)(sk-[a-zA-Z0-9]{20,})").expect("REDACT[0]: static regex is valid"),
        Regex::new(r"(?i)(key[=:\s]+[a-zA-Z0-9\-_]{20,})")
            .expect("REDACT[1]: static regex is valid"),
        Regex::new(r"(?i)(token[=:\s]+[a-zA-Z0-9\-_]{20,})")
            .expect("REDACT[2]: static regex is valid"),
        Regex::new(r"(?i)(bearer\s+[a-zA-Z0-9\-_\.]{20,})")
            .expect("REDACT[3]: static regex is valid"),
    ]
});

/// Redact security-sensitive tokens from text before trajectory recording.
fn redact_sensitive(text: &str) -> String {
    let mut result = text.to_string();
    for re in REDACT_PATTERNS.iter() {
        let replaced: String = re.replace_all(&result, "[REDACTED]").into_owned();
        result = replaced;
    }
    result
}

impl<M: MemoryBackend> AgentLoop<M> {
    /// OMEGA-IX: Assembles cognitive context from episodic memory and workspace state.
    async fn assemble_context(
        &mut self,
        user_input: &str,
        session_id: &Option<savant_core::types::SessionId>,
        history: &[ChatMessage],
    ) -> Result<Vec<ChatMessage>, SavantError> {
        let effective_sid = session_id
            .as_ref()
            .map(|s| s.0.clone())
            .unwrap_or_else(|| self.agent_id.clone());

        let mut current_history = if self.skip_memory_retrieval {
            // Heartbeat mode: skip memory retrieval to prevent old messages
            // from being recalled. The heartbeat prompt is self-contained.
            Vec::new()
        } else {
            // Auto-recall: retrieve relevant memories with semantic search.
            // Memories are injected into the system prompt ONLY (not conversation history)
            // to avoid wasting attention on duplicate content.
            let recalled = match self.memory.auto_recall(&effective_sid, user_input).await {
                Ok(recalled) if !recalled.is_empty() => recalled,
                _ => self.memory.retrieve(&effective_sid, user_input, 10).await?,
            };

            // Format recalled memories as auto-recall block for system prompt injection
            if !recalled.is_empty() {
                let mut recall_block =
                    String::from("CONTEXT RECALL (automatically retrieved from memory):\n");
                for msg in recalled.iter().take(10) {
                    let preview: String = msg.content.chars().take(200).collect();
                    recall_block.push_str(&format!("- [{}] {}\n", msg.role, preview));
                }
                recall_block.push('\n');
                self.context.set_auto_recall(recall_block);
            }

            // Return empty — recalled memories are in system prompt, not conversation
            Vec::new()
        };

        // === Conversation history (actual messages, no recalled duplicates) ===
        current_history.extend(history.to_vec());

        // Facet extraction: extract user preferences from recent conversation
        let facets = self.facet_extractor.extract(&current_history);
        for facet in facets {
            self.facet_cache.observe(facet);
        }
        let stable = self.facet_cache.stable_facets();
        if !stable.is_empty() {
            let prefs_text = crate::learning::FacetExtractor::render_preferences(&stable);
            self.context.set_user_preferences(prefs_text);
        }

        let mut messages = self.context.build_messages(current_history);

        // SemanticWindow: evict low-importance messages when context is large
        if messages.len() > 50 {
            let sw = crate::semantic_window::SemanticWindow::new(
                crate::semantic_window::window::WindowConfig::default(),
            );
            let result = sw.manage(&messages, user_input);
            if !result.evicted.is_empty() {
                tracing::debug!(
                    "[{}] SemanticWindow evicted {} messages, retained {}",
                    self.agent_id,
                    result.evicted.len(),
                    result.retained.len()
                );
                messages = result.retained;
            }
        }

        // Plugin execute_before_llm_call is invoked once per iteration in the main loop (stream.rs).
        // Previously it ran here AND in the main loop, causing double-execution.
        Ok(messages)
    }

    /// Process images in messages that haven't been handled by the provider.
    /// If the provider supports multimodal, images are passed through to the API.
    /// If not, uses the vision service to describe images and appends descriptions.
    /// Should be called after message assembly but before provider streaming.
    async fn process_images(&self, messages: &mut [ChatMessage]) -> Result<(), SavantError> {
        // If provider supports multimodal, no processing needed — images are formatted
        // at the provider level (OpenAI content array, Anthropic image blocks, Ollama images field).
        if self.provider.supports_multimodal() {
            return Ok(());
        }

        // Provider doesn't support multimodal — use vision service to describe images.
        let vision = match &self.vision_service {
            Some(v) => v,
            None => {
                // Check if any message actually has images before warning
                let has_images = messages.iter().any(|m| !m.images.is_empty());
                if has_images {
                    warn!(
                        "[{}] Message contains images but no vision service available \
                         and provider does not support multimodal. Images will be ignored.",
                        self.agent_id
                    );
                }
                return Ok(());
            }
        };

        let mut described = false;
        for msg in messages.iter_mut() {
            if msg.images.is_empty() || msg.role != ChatRole::User {
                continue;
            }
            for img in &msg.images {
                match vision
                    .describe_image(img, "Describe this image in detail for context.")
                    .await
                {
                    Ok(description) => {
                        msg.content
                            .push_str(&format!("\n\n[Image: {}]", description));
                        described = true;
                    }
                    Err(e) => {
                        warn!(
                            "[{}] Failed to describe image: {}. Skipping.",
                            self.agent_id, e
                        );
                    }
                }
            }
            // Clear images after description to prevent reprocessing
            msg.images.clear();
        }

        // Unload vision model to free VRAM after processing
        if described {
            if let Err(e) = vision.unload_model().await {
                debug!(
                    "[{}] Failed to unload vision model (non-fatal): {}",
                    self.agent_id, e
                );
            }
        }

        Ok(())
    }

    pub fn run(
        &mut self,
        user_input: String,
        session_id: Option<savant_core::types::SessionId>,
        shutdown_token: CancellationToken,
    ) -> Pin<Box<dyn Stream<Item = Result<AgentEvent, SavantError>> + Send + '_>> {
        let user_msg = ChatMessage {
            is_telemetry: false,
            role: ChatRole::User,
            content: user_input.clone(),
            sender: Some("USER".to_string()),
            recipient: None,
            agent_id: None,
            session_id: session_id.clone(),
            channel: savant_core::types::AgentOutputChannel::Chat,
            images: Vec::new(),
            ..Default::default()
        };
        let mut history = vec![user_msg];

        Box::pin({
            use async_stream::stream;
            stream! {
                    let mut depth = 0;

                    // D1: Persist user message immediately (crash recovery)
                    if let Some(ref sid) = session_id {
                        let sid_str = sid.0.clone();
                        if let Err(e) = self.memory.store(&sid_str, &history[0]).await {
                            tracing::warn!("[{}] Failed to persist user message: {}", self.agent_id, e);
                        }
                    }
                    let mut seen_actions: HashSet<String> = HashSet::new();
                    let sid = session_id.as_ref().map(|s| s.0.clone()).unwrap_or_else(|| self.agent_id.clone());

                    // === Session / Turn Initialization ===
                    let turn_id = uuid::Uuid::new_v4().to_string();

                    // Get or create session state
                    let mut session_state = match self.memory.get_or_create_session(&sid).await {
                        Ok(s) => s,
                        Err(e) => {
                            warn!("[{}] Failed to load session state: {}. Creating ephemeral.", self.agent_id, e);savant_core::types::SessionState {
    session_id: sid.clone(),
    created_at: chrono::Utc::now().timestamp_millis(),
    last_active: chrono::Utc::now().timestamp_millis(),
    turn_count: 0,
    active_turn_id: None,
    auto_approved_tools: vec![],
    denied_tools: vec![],
    parent_session_id: None,
    fork_point_turn_id: None,
    // FID-029 §Step 1: sibling-collection design — title is populated
    // separately by load_session_title() at hydrate time.
    title: None,
}
                        }
                    };

                    // Begin turn
                    session_state.active_turn_id = Some(turn_id.clone());
                    session_state.turn_count += 1;

                    // === Hook: Turn Start ===
                    let turn_start_ctx = savant_core::hooks::HookContext {
                        event: savant_core::hooks::HookEvent::TurnStart,
                        session_id: Some(sid.clone()),
                        agent_id: Some(self.agent_id.clone()),
                        tool_name: None,
                        content: Some(user_input.clone()),
                        error: None,
                        metadata: std::collections::HashMap::new(),
                    };
                    self.hooks.run_void(&turn_start_ctx).await;
                    session_state.last_active = chrono::Utc::now().timestamp_millis();
                    if let Err(e) = self.memory.save_session(&session_state).await { tracing::warn!("[{}] Failed to save session: {}", self.agent_id, e); }

                    // Trajectory recording: record user message
                    if let Some(ref recorder) = self.trajectory_recorder {
                        recorder.lock().await.record_user_message(&user_input);
                    }

                    let turn_state = savant_core::types::TurnState {
                        turn_id: turn_id.clone(),
                        session_id: sid.clone(),
                        state: savant_core::types::TurnPhase::Processing,
                        tool_calls_made: Vec::new(),
                        started_at: chrono::Utc::now().timestamp_millis(),
                        completed_at: 0,
                    };
                    if let Err(e) = self.memory.save_turn(&turn_state).await { tracing::warn!("[{}] Failed to save turn: {}", self.agent_id, e); }

                    // Emit SessionStart event
                    yield Ok(AgentEvent::SessionStart {
                        session_id: sid.clone(),
                        turn_id: turn_id.clone(),
                    });

                    // Track tool calls made during this turn
                    let mut turn_tool_calls: Vec<String> = Vec::new();
                    let mut turn_failed = false;

                    // NLP command parsing: handle natural language commands directly
                    let nlp_intent = crate::nlp::parse_command(&user_input);
                    if nlp_intent.category != crate::nlp::CommandCategory::Unknown && nlp_intent.confidence > 0.7 {
                        let response = match nlp_intent.category {
                            crate::nlp::CommandCategory::Help => {
                                "Available commands:\n- Show agents: list all agents\n- Restart agent <name>: restart an agent\n- Switch to <model>: change LLM model\n- Restart/stop discord/telegram/whatsapp/matrix: manage channels\n- Show status: system health\n- What's using the most memory: diagnostics\n- Why did agent <name> fail: failure analysis".to_string()
                            }
                            crate::nlp::CommandCategory::Status => {
                                format!("[{}] System operational. Turn {}, depth {}.", self.agent_id, turn_id, depth)
                            }
                            crate::nlp::CommandCategory::AgentManagement => {
                                format!("[{}] Agent command received: {} (target: {})", self.agent_id, nlp_intent.action, nlp_intent.target.as_deref().unwrap_or("all"))
                            }
                            crate::nlp::CommandCategory::ModelSwitch => {
                                format!("[{}] Model switch requested: {}", self.agent_id, nlp_intent.target.as_deref().unwrap_or("unknown"))
                            }
                            crate::nlp::CommandCategory::ChannelControl => {
                                format!("[{}] Channel command: {} {}", self.agent_id, nlp_intent.action, nlp_intent.target.as_deref().unwrap_or(""))
                            }
                            crate::nlp::CommandCategory::Diagnostics => {
                                format!("[{}] Diagnostics query: {}", self.agent_id, nlp_intent.action)
                            }
                            _ => String::new(), // fall through to LLM — empty string signals no command
                        };
                        if !response.is_empty() {
                            yield Ok(AgentEvent::Observation(response));
                            return;
                        }
                    }

                    // Register with security circuit breaker for this turn
                    let cb_task_id = format!("{}:{}", self.agent_id, turn_id);
                    self.circuit_breaker.register_task(
                        &cb_task_id,
                        savant_security::continuous::circuit_breaker::TaskClass::Standard,
                    ).await;

                    // Preserve last clean_answer across loop scope for reflection generation
                    let mut last_clean_answer = String::new();

                    while depth < self.max_tool_iterations as u32 {
                        info!("[{}] Agent loop cycle start (depth={})", self.agent_id, depth);

                        // Circuit breaker: check task timeout at each iteration
                        let max_task_duration = std::time::Duration::from_secs(300); // 5 min default
                        if let Err(e) = self.circuit_breaker.check_timeout(&cb_task_id, max_task_duration).await {
                            yield Err(e);
                            break;
                        }

                        // === Delegate Checkpoint: check_signals ===
                        if let Some(ref delegate_mtx) = self.delegate {
                            let delegate = delegate_mtx.lock().await;
                            if let crate::react::LoopSignal::Terminate = delegate.check_signals().await {
                                warn!("[{}] Delegate signaled termination at depth={}", self.agent_id, depth);
                                break;
                            }
                        }

                        let mut messages = self.assemble_context(&user_input, &session_id, &history).await?;

                        // Trajectory recording: capture system prompt on first iteration
                        if depth == 0 {
                            if let Some(ref recorder) = self.trajectory_recorder {
                                if let Some(sys_msg) = messages.iter().find(|m| m.role == savant_core::types::ChatRole::System) {
                                    // Redact security-sensitive tokens from trajectory recording
                                    let redacted = redact_sensitive(&sys_msg.content);
                                    recorder.lock().await.record_system_prompt(&redacted);
                                }
                            }
                        }

                        // === Context Compaction Check ===
                        // ContextCompressor: LLM-based summarization with fallback to truncation
                        let estimated_tokens: usize = messages.iter().map(|m| token_count(&m.content)).sum();
                        // Use persistent compressor (field on AgentLoop) so cooldown Mutex survives across iterations.
                        if self.context_compressor.should_compress(&messages, estimated_tokens, self.context_window).await {
                            let (head, middle, tail) = self.context_compressor.partition(&messages);
                            let compression_prompt = crate::context_compressor::ContextCompressor::build_compression_prompt(&middle);
                            info!("[{}] Context at ~{:.0}% — attempting LLM-based compression", self.agent_id, (estimated_tokens as f64 / self.context_window as f64) * 100.0);
                            yield Ok(AgentEvent::StatusUpdate("CONTEXT_COMPRESSING: LLM summarization".to_string()));

                            // Try LLM-based compression
                            let summary_result = self.provider.stream_completion(
                                vec![ChatMessage {
                                    is_telemetry: false,
                                    role: ChatRole::User,
                                    content: compression_prompt,
                                    sender: Some("SYSTEM".to_string()),
                                    recipient: None,
                                    agent_id: None,
                                    session_id: Some(savant_core::types::SessionId(sid.clone())),
                                    channel: savant_core::types::AgentOutputChannel::Chat,
                                    images: Vec::new(),
                ..Default::default()
            }],
                                vec![],
                            ).await;

                            match summary_result {
                                Ok(mut summary_stream) => {
                                    let mut summary = String::new();
                                    while let Some(Ok(chunk)) = summary_stream.next().await {
                                        if !chunk.content.is_empty() {
                                            summary.push_str(&chunk.content);
                                        }
                                    }
                                    if !summary.is_empty() {
                                        // G-1: Truncate summary to max_summary_tokens
                                        let max_tokens = self.context_compressor.max_summary_tokens();
                                        let summary_token_count = token_count(&summary);
                                        let truncated_summary = if summary_token_count > max_tokens {
                                            // Truncate by character estimate (4 chars ≈ 1 token)
                                            let max_chars = max_tokens * 4;
                                            if summary.len() > max_chars {
                                                format!("{}...", &summary[..max_chars])
                                            } else {
                                                summary
                                            }
                                        } else {
                                            summary
                                        };
                                        // Build compressed context: head + summary + tail
                                        let mut compressed: Vec<ChatMessage> = head.into_iter().cloned().collect();
                                        compressed.push(ChatMessage {
                                            is_telemetry: false,
                                            role: ChatRole::Assistant,
                                            content: format!("[Context Summary]\n{}", truncated_summary),
                                            sender: Some("SYSTEM".to_string()),
                                            recipient: None,
                                            agent_id: None,
                                            session_id: Some(savant_core::types::SessionId(sid.clone())),
                                            channel: savant_core::types::AgentOutputChannel::Chat,
                                            images: Vec::new(),
                ..Default::default()
            });
                                        compressed.extend(tail.into_iter().cloned());
                                        messages = compressed;
                                        info!("[{}] LLM compression successful: {} → {} messages", self.agent_id, estimated_tokens, messages.iter().map(|m| token_count(&m.content)).sum::<usize>());
                                        yield Ok(AgentEvent::StatusUpdate("CONTEXT_COMPRESSED: LLM summary applied".to_string()));
                                        // Skip the truncation fallback below
                                        history = messages.iter()
                                            .filter(|m| m.role != savant_core::types::ChatRole::System)
                                            .cloned()
                                            .collect();
                                    } else {
                                        // Empty summary — fall through to truncation fallback
                                        warn!("[{}] LLM compression returned empty summary, falling back to truncation", self.agent_id);
                                    }
                                }
                                Err(e) => {
                                    // LLM failed — fall through to truncation fallback
                                    warn!("[{}] LLM compression failed: {}, falling back to truncation", self.agent_id, e);
                                }
                            }
                        }

                        // NS-02 + NS-04: Use L2Compressor thresholds scaled by OCEAN personality
                        let l2_base = crate::compact::l2::L2Thresholds::default();
                        let l2_adjusted = if let Some(ocean) = self.context.personality_traits() {
                            crate::compact::ocean::OceanScaler::scale_thresholds(&l2_base, ocean)
                        } else {
                            l2_base
                        };
                        let compaction_thresholds = crate::react::compaction::L2CompactionThresholds::from(l2_adjusted);
                        let monitor = crate::react::compaction::ContextMonitor::with_thresholds(self.context_window, compaction_thresholds);
                        if let Some(strategy) = monitor.suggest(&messages) {
                            let usage = monitor.usage_ratio(&messages);
                            info!(
                                "[{}] Context at {:.0}% — applying {:?} compaction",
                                self.agent_id,
                                usage * 100.0,
                                strategy
                            );
                            yield Ok(AgentEvent::StatusUpdate(
                                format!("CONTEXT_COMPACTING: {:.0}% usage → {:?}", usage * 100.0, strategy)
                            ));
                            // Scale keep_recent proportionally to context window size
                            // 128K context → 10 messages, 256K → 20, 32K → 5
                            let keep_recent = (self.context_window / 12_800).max(5);
                            messages = crate::react::compaction::Compactor::compact(messages, strategy, keep_recent);
                            // Rebuild history from compacted messages (skip system prompt)
                            history = messages.iter()
                                .filter(|m| m.role != savant_core::types::ChatRole::System)
                                .cloned()
                                .collect();
                        }

                        // Vision: process images in messages
                        // For multimodal providers, images are formatted at the provider level.
                        // For non-multimodal providers, the vision service describes images as text.
                        self.process_images(&mut messages).await?;

                        let complexity = (history.len() as f32 * 0.5) + (depth as f32);
                        let k = self.predictor.predict_optimal_k(complexity);
                        debug!("DSP Prediction: k={} for complexity={}", k, complexity);

                        if let Some(host) = &self.plugin_host {
                            for plugin in &self.plugins {
                                let mut combined_prompt = String::new();
                                for msg in &messages { combined_prompt.push_str(&msg.content); }

                                if let Ok(res) = host.execute_before_llm_call(plugin, &combined_prompt, self.agent_id_hash, self.security_token.clone()).await {
                                    match res {
                                        crate::plugins::wasm_host::exports::savant::agent_hooks::hooks::HookResult::Modified(new_prompt) => {
                                            if let Some(last) = messages.last_mut() { last.content = new_prompt; }
                                        }
                                        crate::plugins::wasm_host::exports::savant::agent_hooks::hooks::HookResult::Halt(reason) => {
                                            yield Err(SavantError::Unknown(format!("Halted by plugin: {}", reason)));
                                            return;
                                        }
                                        crate::plugins::wasm_host::exports::savant::agent_hooks::hooks::HookResult::Continue => {}
                                    }
                                }
                            }
                        }

                        // Build tool schemas for LLM API
                        let tool_schemas: Vec<serde_json::Value> = self.tools.iter().map(|t| {
                            serde_json::json!({
                                "type": "function",
                                "function": {
                                    "name": t.name(),
                                    "description": t.description(),
                                    "parameters": t.parameters_schema(),
                                }
                            })
                        }).collect();

                        // === Hook: Before LLM Call (modifying — can cancel) ===
                        let mut before_llm_ctx = savant_core::hooks::HookContext {
                            event: savant_core::hooks::HookEvent::BeforeLlmCall,
                            session_id: Some(sid.clone()),
                            agent_id: Some(self.agent_id.clone()),
                            tool_name: None,
                            content: Some(format!("{} messages, {} tools", messages.len(), tool_schemas.len())),
                            error: None,
                            metadata: std::collections::HashMap::new(),
                        };
                        if let savant_core::hooks::HookResult::Cancel(reason) = self.hooks.run_modifying(&mut before_llm_ctx).await {
                            yield Ok(AgentEvent::Observation(format!("LLM call cancelled by hook: {}", reason)));
                            break;
                        }

                        // Security circuit breaker: check API call limits before LLM call
                        if let Err(e) = self.circuit_breaker.check_api_call(&cb_task_id).await {
                            yield Err(e);
                            break;
                        }

                        // === Delegate Checkpoint: before_llm_call ===
                        // Clone delegate Arc to avoid borrow conflict with &mut self.
                        let delegate_arc = self.delegate.as_ref().map(Arc::clone);
                        if let Some(ref delegate_mtx) = delegate_arc {
                            let delegate = delegate_mtx.lock().await;
                            let mut trace_buf = String::new();
                            let mut ctx = crate::react::LoopContext { loop_state: &mut *self, trace: &mut trace_buf };
                            if let Some(outcome) = delegate.before_llm_call(&mut ctx).await {
                                match outcome {
                                    crate::react::LoopOutcome::Success(msg) => {
                                        yield Ok(AgentEvent::Observation(msg));
                                        break;
                                    }
                                    crate::react::LoopOutcome::Failure(msg) => {
                                        yield Err(SavantError::Unknown(msg));
                                        return;
                                    }
                                }
                            }
                        }

                        // === Delegate Checkpoint: call_llm ===
                        // Clone delegate Arc to avoid borrow conflict with &mut self.
                        let delegate_arc_call = self.delegate.as_ref().map(Arc::clone);
                        let delegate_response: Option<crate::react::ChatResponse> = if let Some(ref delegate_mtx) = delegate_arc_call {
                            let delegate = delegate_mtx.lock().await;
                            let mut trace_buf = String::new();
                            let mut ctx = crate::react::LoopContext { loop_state: &mut *self, trace: &mut trace_buf };
                            match delegate.call_llm(&mut ctx).await {
                                Ok(resp) => Some(resp),
                                Err(e) => {
                                    warn!("[{}] Delegate call_llm failed: {}, falling back to default provider", self.agent_id, e);
                                    None
                                }
                            }
                        } else {
                            None
                        };

                        // clean_answer is populated during stream processing and used for reflection
                        let mut clean_answer = String::new();

                        // If delegate provided a response, convert to stream. Otherwise use default provider.
                        let response_stream = if let Some(resp) = delegate_response {
                            // Convert delegate response into a single-chunk stream
                            let agent_id = self.agent_id.clone();
                            let sid_clone = sid.clone();
                            Box::pin(futures::stream::once(async move {
                                Ok(savant_core::types::ChatChunk {
                                    agent_name: agent_id.clone(),
                                    agent_id,
                                    content: resp.content,
                                    is_final: true,
                                    session_id: Some(savant_core::types::SessionId(sid_clone)),
                                    channel: savant_core::types::AgentOutputChannel::Chat,
                                    logprob: None,
                                    is_telemetry: false,
                                    reasoning: None,
                                    tool_calls: if resp.tool_calls.is_empty() { None } else { Some(resp.tool_calls) },
                                })
                            })) as std::pin::Pin<Box<dyn futures::Stream<Item = Result<savant_core::types::ChatChunk, SavantError>> + Send>>
                        } else {
                        // Rate limiter check before LLM call
                        if let Some(ref limiter) = self.rate_limiter {
                            let estimated_tokens: u32 = messages.iter()
                                .map(|m| savant_core::utils::token_count(&m.content) as u32)
                                .sum();
                            if let Err(wait_ms) = limiter.check(estimated_tokens).await {
                                tracing::warn!("[{}] Rate limited — waiting {}ms before retry", self.agent_id, wait_ms);
                                tokio::time::sleep(std::time::Duration::from_millis(wait_ms)).await;
                                if let Err(retry_wait) = limiter.check(estimated_tokens).await {
                                    tracing::error!("[{}] Rate limit exceeded after retry ({}ms) — aborting turn", self.agent_id, retry_wait);
                                    yield Ok(AgentEvent::StatusUpdate("RATE_LIMITED".to_string()));
                                    break;
                                }
                            }
                        }

                        // RT #7: Pre-send context window validation
                        let total_tokens: usize = messages.iter()
                            .map(|m| savant_core::utils::token_count(&m.content))
                            .sum();
                        if let Some(window) = self.provider.context_window() {
                            if total_tokens > window {
                                tracing::warn!(
                                    "[{}] Context overflow: {} tokens > {} window. Emergency compaction.",
                                    self.agent_id, total_tokens, window
                                );
                                messages = crate::react::compaction::Compactor::compact(
                                    messages,
                                    crate::react::compaction::CompactionStrategy::Truncate,
                                    10, // Keep 10 most recent messages
                                );
                            }
                        }

                        // Proactive context gathering — inject relevant memories
                        // before LLM call. Uses the agent's memory backend directly.
                        if !self.skip_memory_retrieval {
                            match self.memory.retrieve("system", &user_input, 5).await {
                                Ok(recalled) if !recalled.is_empty() => {
                                    let mut context_block = String::from("PROACTIVE CONTEXT (auto-gathered):\n");
                                    for msg in recalled.iter().take(5) {
                                        let preview: String = msg.content.chars().take(200).collect();
                                        context_block.push_str(&format!("- {}\n", preview));
                                    }
                                    if let Some(first) = messages.first_mut() {
                                        first.content = format!("{}\n\n{}", first.content, context_block);
                                    }
                                }
                                _ => {}
                            }
                        }

                        // Cost-aware routing — classify prompt complexity for observability
                        let complexity = crate::providers::cost_router::CostAwareRouter::classify(&user_input);
                        tracing::debug!(
                            "[{}] Prompt complexity: {:?}",
                            self.agent_id,
                            complexity,
                        );

                        match self.provider.stream_completion(messages.clone(), tool_schemas.clone()).await {
                            Ok(stream) => stream,
                            Err(e) => {
                                if let Some(fallback) = &self.fallback_provider {
                                    warn!("[{}] Primary provider failed: {}. Triggering OMEGA-VIII Fallback.", self.agent_id, e);
                                    yield Ok(AgentEvent::StatusUpdate("FALLBACK_PROVIDER_ACTIVATED".to_string()));
                                    fallback.stream_completion(messages.clone(), tool_schemas.clone()).await?
                                } else {
                                    // Finalize turn state before error return
                                    let final_turn = savant_core::types::TurnState {
                                        turn_id: turn_id.clone(),
                                        session_id: sid.clone(),
                                        state: savant_core::types::TurnPhase::Failed,
                                        tool_calls_made: turn_tool_calls.clone(),
                                        started_at: turn_state.started_at,
                                        completed_at: chrono::Utc::now().timestamp_millis(),
                                    };
                                    if let Err(e) = self.memory.save_turn(&final_turn).await { tracing::warn!("[{}] Failed to save turn: {}", self.agent_id, e); }
                                    session_state.active_turn_id = None;
                                    session_state.last_active = chrono::Utc::now().timestamp_millis();
                                    if let Err(e) = self.memory.save_session(&session_state).await { tracing::warn!("[{}] Failed to save session: {}", self.agent_id, e); }

                                    yield Err(e);
                                    return;
                                }
                            }
                        }
                        };

                        // RC-12/RC-13: Buffer size limits to prevent unbounded growth
                        const MAX_FRAGMENT_BUFFER_SIZE: usize = 1_000_000; // 1MB
                        const MAX_TRACE_SIZE: usize = 500_000; // 500KB

                        let mut full_trace = String::new();
                        let mut llm_stream = response_stream;
                        let mut fragment_buffer = String::new();
                        let mut in_hidden_tag = false;
                        let mut hidden_tag_name = String::new();
                        // All known thinking/reasoning tag formats across models
                        const THOUGHT_TAGS: &[(&str, &str)] = &[
                            ("<think>", "</think>"),
                            ("<thinking>", "</thinking>"),
                            ("<thought>", "</thought>"),
                            ("<reasoning>", "</reasoning>"),
                        ];
                        // Tags that should be hidden from user output (tool calls, environment details, etc.)
                        const HIDDEN_TAGS: &[&str] = &[
                            "environment_details", "use_mcp_tool", "read_file", "write_to_file",
                            "execute_command", "ask_followup_question", "attempt_completion",
                            "search_files", "list_files", "replace_in_file",
                            "browser_action", "mcp_response", "tool_result",
                            // Savant tool tags
                            "file_create", "file_move", "file_delete", "file_atomic_edit",
                            "foundation", "memory_search", "memory_append",
                            "web_search", "web_fetch", "task_matrix", "settings",
                            "librarian", "web_projection", "tool_call", "final_answer",
                        ];

                        while let Some(chunk_res) = llm_stream.next().await {
                            match chunk_res {
                                Ok(chunk) => {
                                    // Handle provider-level reasoning (2026 standard: delta.reasoning)
                                    if let Some(ref reasoning) = chunk.reasoning {
                                        if !reasoning.trim().is_empty() {
                                            if full_trace.len() < MAX_TRACE_SIZE { full_trace.push_str(reasoning); }
                                            yield Ok(AgentEvent::Thought(reasoning.clone()));
                                            // Panopticon replay: record reasoning trace
                                            if let Some(ref recorder) = self.replay_recorder {
                                                recorder.record(savant_panopticon::replay::ReplayEvent {
                                                    id: uuid::Uuid::new_v4().to_string(),
                                                    agent_id: self.agent_id.clone(),
                                                    timestamp: chrono::Utc::now().timestamp_millis(),
                                                    event_type: ReplayEventType::Thought,
                                                    content: redact_sensitive(reasoning),
                                                    metadata: None,
                                                }).await;
                                            }
                                        }
                                    }

                                    if let Some(calls) = &chunk.tool_calls {
                                        for call in calls {
                                            let markup = format!("<tool_call>\n<name>{}</name>\n<arguments>{}</arguments>\n</tool_call>\n", call.name, call.arguments);
                                            if full_trace.len() < MAX_TRACE_SIZE { full_trace.push_str(&markup); }
                                        }
                                    }

                                    if !chunk.content.is_empty() {
                                        let content = chunk.content;
                                        // RC-13: Cap full_trace to prevent unbounded growth
                                        if full_trace.len() + content.len() <= MAX_TRACE_SIZE {
                                            full_trace.push_str(&content);
                                        }
                                        // RC-12: Cap fragment_buffer to prevent unbounded growth
                                        if fragment_buffer.len() + content.len() <= MAX_FRAGMENT_BUFFER_SIZE {
                                            fragment_buffer.push_str(&content);
                                        } else {
                                            // Flush the buffer and start fresh
                                            fragment_buffer.clear();
                                            fragment_buffer.push_str(&content);
                                        }

                                        loop {
                                            if !in_hidden_tag {
                                                // Check for any thought/reasoning tag format
                                                if let Some((pos, start_tag, end_tag)) = find_thought_tag(&fragment_buffer, THOUGHT_TAGS) {
                                                    let dialogue_part = &fragment_buffer[..pos];
                                                    if !dialogue_part.trim().is_empty() {
                                                        clean_answer.push_str(dialogue_part);
                                                        yield Ok(AgentEvent::FinalAnswerChunk(dialogue_part.to_string()));
                                                    }
                                                    fragment_buffer = fragment_buffer[pos + start_tag.len()..].to_string();
                                                    in_hidden_tag = true;
                                                    hidden_tag_name = end_tag.to_string();
                                                }
                                                // Check for hidden tool tags
                                                else if let Some((tag_start, tag_name)) = find_hidden_tag_start(&fragment_buffer, HIDDEN_TAGS) {
                                                    let dialogue_part = &fragment_buffer[..tag_start];
                                                    if !dialogue_part.trim().is_empty() {
                                                        clean_answer.push_str(dialogue_part);
                                                        yield Ok(AgentEvent::FinalAnswerChunk(dialogue_part.to_string()));
                                                    }
                                                    let end_tag = format!("</{}>", tag_name);
                                                    if let Some(end_pos) = fragment_buffer[tag_start..].find(&end_tag) {
                                                        // Tag fully contained - push to full_trace for parsing, skip from user output
                                                        let tag_content = &fragment_buffer[tag_start..tag_start + end_pos + end_tag.len()];
                                                        if full_trace.len() < MAX_TRACE_SIZE { full_trace.push_str(tag_content); }
                                                        fragment_buffer = fragment_buffer[tag_start + end_pos + end_tag.len()..].to_string();
                                                        in_hidden_tag = false;
                                                    } else {
                                                        // Tag continues past buffer - enter hidden mode, push opening to full_trace
                                                        fragment_buffer = fragment_buffer[tag_start..].to_string();
                                                        if full_trace.len() < MAX_TRACE_SIZE { full_trace.push_str(&fragment_buffer); }
                                                        in_hidden_tag = true;
                                                        // Store full closing tag (e.g. "</think>") to match thought tag behavior.
                                                        // Previously stored bare name (e.g. "tool_call") which could match inside content.
                                                        hidden_tag_name = format!("</{}>", tag_name);
                                                    }
                                                } else {
                                                    // No tag found - flush safe content
                                                    let safe_to_flush = find_safe_flush_length(&fragment_buffer, THOUGHT_TAGS, HIDDEN_TAGS);
                                                    if safe_to_flush > 0 {
                                                        let dialogue_chunk: String = fragment_buffer.drain(..safe_to_flush).collect();
                                                        clean_answer.push_str(&dialogue_chunk);
                                                        yield Ok(AgentEvent::FinalAnswerChunk(dialogue_chunk));
                                                    }
                                                    break;
                                                }
                                            } else {
                                                // Inside a hidden tag - look for closing tag
                                                let end_tag = &hidden_tag_name;
                                                if let Some(pos) = fragment_buffer.find(end_tag.as_str()) {
                                                    // Check if this is a thought tag (any of the known formats)
                                                    let is_thought = THOUGHT_TAGS.iter().any(|(_, et)| *et == end_tag);
                                                    if is_thought {
                                                        let thought_part = &fragment_buffer[..pos];
                                                        if !thought_part.is_empty() {
                                                            yield Ok(AgentEvent::Thought(thought_part.to_string()));
                                                            // Also emit as FinalAnswerChunk so the user sees the thinking content
                                                            yield Ok(AgentEvent::FinalAnswerChunk(thought_part.to_string()));
                                                        }
                                                    }
                                                    // Push closing tag to full_trace for action parsing
                                                    full_trace.push_str(&fragment_buffer[..pos + end_tag.len()]);
                                                    fragment_buffer = fragment_buffer[pos + end_tag.len()..].to_string();
                                                    in_hidden_tag = false;
                                                    hidden_tag_name.clear();
                                                } else {
                                                    // Still inside hidden tag - consume buffer but keep tail for partial match
                                                    let safe_len = fragment_buffer.len().saturating_sub(end_tag.len());
                                                    if safe_len > 0 {
                                                        let consumed: String = fragment_buffer.drain(..safe_len).collect();
                                                        full_trace.push_str(&consumed);
                                                    }
                                                    break;
                                                }
                                            }
                                        }
                                    }
                                    if chunk.is_final {
                                        // Flush any remaining fragment buffer content before breaking
                                        if !fragment_buffer.is_empty() {
                                            if in_hidden_tag {
                                                // Check if it's a thought tag - emit as both Thought and FinalAnswerChunk
                                                let is_thought = THOUGHT_TAGS.iter().any(|(_, et)| hidden_tag_name.contains(et));
                                                if is_thought && !fragment_buffer.trim().is_empty() {
                                                    yield Ok(AgentEvent::Thought(fragment_buffer.clone()));
                                                    yield Ok(AgentEvent::FinalAnswerChunk(fragment_buffer.clone()));
                                                } else {
                                                    full_trace.push_str(&fragment_buffer);
                                                }
                                            } else if !fragment_buffer.trim().is_empty() {
                                                clean_answer.push_str(&fragment_buffer);
                                                yield Ok(AgentEvent::FinalAnswerChunk(fragment_buffer.clone()));
                                            }
                                        }
                                        break;
                                    }
                                }
                                Err(e) => {
                                    // Mid-stream retry: wait 2s, retry once before failing
                                    tracing::warn!(
                                        "[{}] Stream chunk error: {}. Attempting mid-stream retry...",
                                        self.agent_id, e
                                    );
                                    tokio::time::sleep(std::time::Duration::from_secs(2)).await;

                                    // Retry the LLM call with same messages
                                    match self.provider.stream_completion(messages.clone(), tool_schemas.clone()).await {
                                        Ok(retry_stream) => {
                                            tracing::info!("[{}] Mid-stream retry succeeded", self.agent_id);
                                            llm_stream = retry_stream;
                                            continue;
                                        }
                                        Err(retry_err) => {
                                            tracing::error!(
                                                "[{}] Mid-stream retry also failed: {}",
                                                self.agent_id, retry_err
                                            );
                                            // Finalize turn state before error return
                                            let final_turn = savant_core::types::TurnState {
                                                turn_id: turn_id.clone(),
                                                session_id: sid.clone(),
                                                state: savant_core::types::TurnPhase::Failed,
                                                tool_calls_made: turn_tool_calls.clone(),
                                                started_at: turn_state.started_at,
                                                completed_at: chrono::Utc::now().timestamp_millis(),
                                            };
                                            if let Err(e) = self.memory.save_turn(&final_turn).await { tracing::warn!("[{}] Failed to save turn: {}", self.agent_id, e); }
                                            session_state.active_turn_id = None;
                                            session_state.last_active = chrono::Utc::now().timestamp_millis();
                                            if let Err(e) = self.memory.save_session(&session_state).await { tracing::warn!("[{}] Failed to save session: {}", self.agent_id, e); }

                                            yield Err(retry_err);
                                            return;
                                        }
                                    }
                                }
                            }
                        }

                        if !fragment_buffer.is_empty() {
                            if in_hidden_tag { yield Ok(AgentEvent::Thought(fragment_buffer)); }
                            else if !fragment_buffer.trim().is_empty() {
                                clean_answer.push_str(&fragment_buffer);
                                yield Ok(AgentEvent::FinalAnswerChunk(fragment_buffer));
                            }
                        }

                        // Circuit breaker: track LLM cost from token usage
                        // Estimate: ~$0.01 per 1K tokens (rough average across providers)
                        let estimated_tokens = token_count(&full_trace);
                        let estimated_cost = (estimated_tokens as f32) * 0.00001;
                        if let Err(e) = self.circuit_breaker.add_cost(&cb_task_id, estimated_cost).await {
                            warn!("[{}] Cost limit exceeded: {}", self.agent_id, e);
                            yield Err(e);
                            break;
                        }

                        let mut actions = parsing::parse_actions(&full_trace);
                        if actions.is_empty() {
                            // Fallback: try single-action parser for simpler LLM outputs
                            if let Some(single) = parsing::parse_action(&full_trace) {
                                actions.push(single);
                            } else {
                                debug!("[{}] No actions parsed from trace (len={})", self.agent_id, full_trace.len());
                            }
                        }

                        // Trajectory recording: record assistant response with tool calls
                        if let Some(ref recorder) = self.trajectory_recorder {
                            let tool_call_records: Vec<crate::react::trajectory::ToolCallRecord> = actions.iter().map(|(name, args)| {
                                crate::react::trajectory::ToolCallRecord {
                                    name: name.clone(),
                                    arguments: serde_json::from_str(args).unwrap_or(serde_json::json!({})),
                                }
                            }).collect();
                            recorder.lock().await.record_assistant_response(&clean_answer, tool_call_records);
                        }

                        // === Delegate Checkpoint: handle_text_response ===
                        let delegate_arc_text = self.delegate.clone();
                        if let Some(ref delegate_mtx) = delegate_arc_text {
                            let delegate = delegate_mtx.lock().await;
                            let mut trace_buf = String::new();
                            let mut ctx = crate::react::LoopContext { loop_state: &mut *self, trace: &mut trace_buf };
                            match delegate.handle_text_response(&clean_answer, &mut ctx).await {
                                crate::react::TextAction::ParseActions => { /* continue to action parsing */ }
                                crate::react::TextAction::Ignore => {
                                    debug!("[{}] Delegate chose to ignore text response, skipping action parsing", self.agent_id);
                                    actions.clear();
                                }
                            }
                        }

                        // --- OMEGA: Autonomous Ambiguity Synthesis ---
                        if actions.is_empty()
                            && (full_trace.contains("Action:") || full_trace.contains("thought"))
                            && !full_trace.contains("Action: None")
                        {
                            warn!("[{}] Ambiguity detected: LLM suggested action but parser failed. Triggering Heuristic Synthesis.", self.agent_id);
                            yield Ok(AgentEvent::StatusUpdate("HEURISTIC_AMBIGUITY_DETECTED".to_string()));

                            // Heuristic: If we see a pattern like "Action: SomeTool(args)", try manually extraction
                            if let Some(start) = full_trace.find("Action:") {
                                let subset = &full_trace[start..];
                                if let Some(end) = subset.find('\n').or(Some(subset.len())) {
                                    let line = &subset[..end];
                                    // Attempt simple parse
                                    if let Some(bracket_start) = line.find('[') {
                                        if let Some(bracket_end) = line.rfind(']') {
                                            let name = line["Action:".len()..bracket_start].trim();
                                            let args = &line[bracket_start+1..bracket_end];
                                            actions.push((name.to_string(), args.to_string()));
                                            info!("[{}] Heuristic: Synthesized action [{}] from ambiguous trace", self.agent_id, name);
                                        }
                                    } else {
                                        // AAA: Robust Fallback for missing brackets
                                        let name = line["Action:".len()..].trim();
                                        if !name.is_empty() {
                                            warn!(
                                                "[{}] Heuristic: Fallback parse for ambiguous action line (no brackets): '{}'",
                                                self.agent_id, name
                                            );
                                            actions.push((name.to_string(), "{}".to_string()));
                                            info!("[{}] Heuristic: Extracted action name '{}' from ambiguous trace", self.agent_id, name);
                                        }
                                    }
                                }
                            }
                        }

                        let mut actual_steps = 0;

                        // === Self-Repair: Check for stuck agent ===
                        let content_hash = xxhash_rust::xxh3::xxh3_64(full_trace.as_bytes());
                        if self.self_repair.check_stuck(content_hash).await {
                            warn!("[{}] Agent appears stuck — injecting recovery hint", self.agent_id);
                            let hint = self.self_repair.recovery_hint().await;
                            yield Ok(AgentEvent::StatusUpdate(format!("STUCK_DETECTED: {}", hint)));
                            history.push(ChatMessage {
                                is_telemetry: false,
                                role: ChatRole::System,
                                content: hint,
                                sender: Some("SELF_REPAIR".to_string()),
                                recipient: None,
                                agent_id: None,
                                session_id: session_id.clone(),
                                channel: savant_core::types::AgentOutputChannel::Telemetry,
                                images: Vec::new(),
                ..Default::default()
            });
                            self.self_repair.reset_stuck().await;
                        }

                        // === Autopilot: Parameter diversity + success rate loop detection ===
                        let autopilot_v = self.autopilot.verdict();
                        match autopilot_v {
                            crate::react::autopilot::AutopilotVerdict::Stuck => {
                                if self.autopilot.record_stuck() {
                                    warn!("[{}] Autopilot: stuck loop detected (low diversity + low success), agent will terminate", self.agent_id);
                                    yield Ok(AgentEvent::StatusUpdate("AUTOPILOT_STUCK: Parameter diversity and success rate too low. Terminating.".to_string()));
                                    break;
                                }
                                warn!("[{}] Autopilot: stuck pattern (round {}/{}), injecting recovery hint", self.agent_id, self.autopilot.stuck_rounds(), self.autopilot.max_stuck_rounds());
                                history.push(ChatMessage {
                                    is_telemetry: false,
                                    role: ChatRole::System,
                                    content: "You appear to be repeating similar tool calls without making progress. Try a completely different approach or explain what's blocking you.".to_string(),
                                    sender: Some("AUTOPILOT".to_string()),
                                    recipient: None,
                                    agent_id: None,
                                    session_id: session_id.clone(),
                                    channel: savant_core::types::AgentOutputChannel::Telemetry,
                                    images: Vec::new(),
                ..Default::default()
            });
                            }
                            crate::react::autopilot::AutopilotVerdict::Suspicious => {
                                history.push(ChatMessage {
                                    is_telemetry: false,
                                    role: ChatRole::System,
                                    content: "Your recent tool calls show low diversity. Consider whether you're making progress or repeating yourself.".to_string(),
                                    sender: Some("AUTOPILOT".to_string()),
                                    recipient: None,
                                    agent_id: None,
                                    session_id: session_id.clone(),
                                    channel: savant_core::types::AgentOutputChannel::Telemetry,
                                    images: Vec::new(),
                ..Default::default()
            });
                            }
                            crate::react::autopilot::AutopilotVerdict::Productive => {
                                self.autopilot.reset_stuck();
                            }
                        }

                        // === Self-Repair: Get excluded tools ===
                        let excluded_tools = self.self_repair.get_excluded_tools().await;

                        // === Delegate Checkpoint: execute_tool_calls ===
                        // If delegate handles tool calls, use its result. Otherwise fall through to default DAG execution.
                        let delegate_handled_tools = if !actions.is_empty() {
                            if let Some(delegate_arc_tools) = self.delegate.clone() {
                                let provider_calls: Vec<savant_core::types::ProviderToolCall> = actions.iter().enumerate().map(|(i, (name, args))| {
                                    savant_core::types::ProviderToolCall {
                                        id: format!("delegate-call-{}", i),
                                        name: name.clone(),
                                        arguments: args.clone(),
                                    }
                                }).collect();
                                let delegate = delegate_arc_tools.lock().await;
                                let mut trace_buf = String::new();
                                let mut ctx = crate::react::LoopContext { loop_state: &mut *self, trace: &mut trace_buf };
                                delegate.execute_tool_calls(provider_calls, &mut ctx).await?
                            } else {
                                None
                            }
                        } else {
                            None
                        };

                        if let Some(outcome) = delegate_handled_tools {
                            match outcome {
                                crate::react::LoopOutcome::Success(msg) => {
                                    yield Ok(AgentEvent::Observation(msg));
                                    break;
                                }
                                crate::react::LoopOutcome::Failure(msg) => {
                                    yield Err(SavantError::Unknown(msg));
                                    return;
                                }
                            }
                        }

                        if !actions.is_empty() {
                            // Panopticon replay: record tool calls
                            if let Some(ref recorder) = self.replay_recorder {
                                for (name, args) in &actions {
                                    recorder.record(savant_panopticon::replay::ReplayEvent {
                                        id: uuid::Uuid::new_v4().to_string(),
                                        agent_id: self.agent_id.clone(),
                                        timestamp: chrono::Utc::now().timestamp_millis(),
                                        event_type: ReplayEventType::ToolCall,
                                        content: format!("{}: {}", name, redact_sensitive(args)),
                                        metadata: Some(serde_json::json!({ "tool": name })),
                                    }).await;
                                }
                            }

                            // --- OMEGA: Transactional Checkpoint ---
                            // Save current history as a stable state before potential side-effects
                            self.heuristic.last_stable_checkpoint = Some(history.clone());
                            info!("[{}] Checkpoint created: Session state stabilized before action execution.", self.agent_id);

                            // Security circuit breaker: check recursion depth before tool execution
                            if let Err(e) = self.circuit_breaker.check_recursion(&cb_task_id).await {
                                yield Err(e);
                                break;
                            }

                            let dag = crate::orchestration::dag::parse_sequential_dag(actions);
                            let lanes = dag.partition_lanes();

                            let tools = self.tools.clone();
                            let hc = self.hyper_causal.clone();
                            let registry = self.echo_registry.clone();
                            let e_host = self.echo_host.clone();
                            let agent_id_hash = self.agent_id_hash;
                            let security_token = self.security_token.clone();
                            let plugin_host = self.plugin_host.clone();
                            let plugins = self.plugins.clone();
                            let self_repair = self.self_repair.clone();

                            'lane_loop: for lane in lanes {
                                let mut queue = FuturesUnordered::new();
                                let mut lane_remaining: Vec<usize> = lane;

                                while !lane_remaining.is_empty() {
                                    let batch_size = self.max_parallel_tools.min(lane_remaining.len());
                                    let batch: Vec<usize> = lane_remaining.drain(..batch_size).collect();

                                    for idx in batch {
                                        let node = &dag.nodes[idx];
                                        let node_name = node.name.clone();
                                        let node_args = node.args.clone();

                                        // Canonical dedup
                                        let mut canonical_args = node_args.clone();
                                        if let Ok(val) = serde_json::from_str::<serde_json::Value>(&node_args) {
                                            if let Ok(serialized) = serde_json::to_string(&val) {
                                                canonical_args = serialized;
                                            }
                                        }
                                        let sig = format!("{}:{}", node_name, canonical_args);
                                        if seen_actions.contains(&sig) {
                                            tracing::warn!("[{}] Skipping duplicate action: {}", self.agent_id, sig);
                                            continue;
                                        }
                                        seen_actions.insert(sig);

                                        // Track tool call for session turn state
                                        turn_tool_calls.push(node_name.clone());

                                        yield Ok(AgentEvent::Action { name: node_name.clone(), args: node_args.clone() });

                                        let tools_inner = tools.clone();
                                        let hc_inner = hc.clone();
                                        let reg_inner = registry.clone();
                                        let host_inner = e_host.clone();
                                        let security_token_inner = security_token.clone();
                                        let excluded_tools_inner = excluded_tools.clone();
                                        let node_name_inner = node_name.clone();
                                        let node_args_inner = node_args.clone();
                                        let agent_id_inner = self.agent_id.clone();
                                        let auto_approved = session_state.auto_approved_tools.clone();
                                        let denied = session_state.denied_tools.clone();

                                        queue.push(async move {
                                            // Self-Repair: Skip excluded tools (marked broken by health tracker)
                                            if excluded_tools_inner.contains(&node_name_inner) {
                                                let err_name = node_name_inner.clone();
                                                return (idx, node_name_inner, String::new(), Err(SavantError::Unknown(
                                                    format!("Tool excluded by self-repair: {}", err_name)
                                                )));
                                            }

                                            // Approval gate: check denied list first
                                            if denied.contains(&node_name_inner) {
                                                return (idx, node_name_inner.clone(), String::new(), Err(SavantError::Unknown(
                                                    format!("Tool '{}' denied by session policy", node_name_inner)
                                                )));
                                            }

                                            let mut result = Err(SavantError::Unknown(format!("Tool access denied or not found: {}", node_name_inner)));
                                            // C3: All agents use CCT — no hardcoded bypasses
                                            let access_granted = if let Some(token) = &security_token_inner {
                                                // Token rotation check: warn if token should be rotated
                                                if token.should_rotate() {
                                                    warn!("[{}] Security token should be rotated (80% lifetime elapsed)", agent_id_inner);
                                                }
                                                let resource = format!("savant://tools/{}", node_name_inner);
                                                token.assignee_matches(agent_id_hash) && token.verify_capability(&resource, "execute")
                                            } else {
                                                // C2: No token = deny access (was: true)
                                                warn!("[{}] No security token for agent — denying access to tool '{}'", agent_id_inner, node_name_inner);
                                                false
                                            };

                                            if access_granted {
                                                debug!("[{}] Attempting to match tool [{}]", agent_id_inner, node_name_inner);
                                                for tool in &tools_inner {
                                                    debug!("[{}] Comparing against tool [{}]", agent_id_inner, tool.name());
                                                    if tool.name().to_lowercase() == node_name_inner.to_lowercase() {
                                                        let mut payload: serde_json::Value = match serde_json::from_str(&node_args_inner) {
                                                            Ok(p) => p,
                                                            Err(e) => {
                                                                let name_for_err = node_name_inner.clone();
                                                                warn!("[agent::stream] Failed to parse args for tool '{}': {}. Args: {}", node_name_inner, e, node_args_inner);
                                                                return (idx, node_name_inner, node_args_inner.clone(), Err(SavantError::Unknown(
                                                                    format!("Invalid JSON arguments for tool '{}': {}", name_for_err, e)
                                                                )));
                                                            }
                                                        };
                                                        // Coerce arguments against tool's JSON Schema
                                                        let schema = tool.parameters_schema();
                                                        if schema.get("type").is_some() {
                                                            payload = crate::tools::coercion::prepare_tool_params(&payload, &schema);
                                                        }
                                                        // Approval gate: check requires_approval() against session state
                                                        use savant_core::traits::ApprovalRequirement;
                                                        match tool.requires_approval() {
                                                            ApprovalRequirement::Always => {
                                                                if !auto_approved.contains(&tool.name().to_string()) {
                                                                    warn!("[{}] Tool '{}' requires approval but is not auto-approved — denying", agent_id_inner, tool.name());
                                                                    result = Err(SavantError::Unknown(
                                                                        format!("Tool '{}' requires user approval. Add to auto_approved_tools or approve via dashboard.", tool.name())
                                                                    ));
                                                                    break;
                                                                }
                                                            }
                                                            ApprovalRequirement::Conditional => {
                                                                if !auto_approved.contains(&tool.name().to_string()) {
                                                                    // Check if conditional criteria are met
                                                                    // For now, treat as needs-approval if not in auto_approved
                                                                    warn!("[{}] Tool '{}' needs conditional approval — not auto-approved", agent_id_inner, tool.name());
                                                                    result = Err(SavantError::Unknown(
                                                                        format!("Tool '{}' needs approval. Add to auto_approved_tools or approve via dashboard.", tool.name())
                                                                    ));
                                                                    break;
                                                                }
                                                            }
                                                            ApprovalRequirement::Never => { /* proceed */ }
                                                        }
                                                        debug!("[{}] Tool [{}] matched. Executing...", agent_id_inner, node_name_inner);
                                                        result = hc_inner.execute_speculative(tool.clone(), payload).await;
                                                        break;
                                                    }
                                                }
                                                if result.is_err() {
                                                    if let (Some(reg), Some(host)) = (&reg_inner, &host_inner) {
                                                        if let Some(cap) = reg.get_tool(&node_name_inner) {
                                                            result = host.execute_tool(&cap.module, &node_args_inner).await.map_err(|e| SavantError::Unknown(e.to_string()));
                                                        }
                                                    }
                                                }
                                            } else { result = Err(SavantError::AuthError(format!("CCT Policy Denied: {}", node_name_inner))); }
                                            (idx, node_name_inner, node_args_inner, result)
                                        });
                                    }

                                    while let Some((_idx, name, tool_args, result)) = queue.next().await {
                                        actual_steps += 1;
                                        match result {
                                            Ok(mut obs) => {
                                                if let Some(host) = &plugin_host {
                                                    for plugin in &plugins {
                                                        match host.execute_after_tool_call(plugin, &name, &obs, agent_id_hash, security_token.clone()).await {
                                                            Ok(crate::plugins::wasm_host::exports::savant::agent_hooks::hooks::HookResult::Modified(new_obs)) => { obs = new_obs; }
                                                            Ok(crate::plugins::wasm_host::exports::savant::agent_hooks::hooks::HookResult::Halt(reason)) => {
                                                                yield Err(SavantError::Unknown(format!("Halted by plugin: {}", reason)));
                                                                return;
                                                            }
                                                            _ => {}
                                                        }
                                                    }
                                                }
                                                // L1 compact compression for DAG parallel path (matches reactor.rs:240)
                                                let parsed_args = crate::compact::integration::parse_argv_from_args(&tool_args);
                                                let compacted = crate::compact::integration::compact_output(&name, &parsed_args, 0, &obs, None).await;
                                                if !compacted.output.is_empty() {
                                                    obs = compacted.output;
                                                }

                                                // Tool result governance: cap at 50,000 chars (~12.5K tokens)
                                                const MAX_TOOL_OUTPUT_CHARS: usize = 50_000;
                                                if obs.len() > MAX_TOOL_OUTPUT_CHARS {
                                                    let truncated: String = obs.chars().take(MAX_TOOL_OUTPUT_CHARS).collect();
                                                    obs = format!(
                                                        "{}\n\n... [truncated: {} chars total, showing first {}]",
                                                        truncated,
                                                        obs.len(),
                                                        MAX_TOOL_OUTPUT_CHARS
                                                    );
                                                    tracing::warn!(
                                                        "[{}] Tool '{}' output truncated: {} → {} chars",
                                                        self.agent_id, name,
                                                        obs.len(), MAX_TOOL_OUTPUT_CHARS
                                                    );
                                                }

                                                yield Ok(AgentEvent::Observation(obs.clone()));

                                                // Trajectory recording: record tool result
                                                if let Some(ref recorder) = self.trajectory_recorder {
                                                    recorder.lock().await.record_tool_result(&name, &obs);
                                                }

                                                // Panopticon replay: record observation
                                                if let Some(ref recorder) = self.replay_recorder {
                                                    recorder.record(savant_panopticon::replay::ReplayEvent {
                                                        id: uuid::Uuid::new_v4().to_string(),
                                                        agent_id: self.agent_id.clone(),
                                                        timestamp: chrono::Utc::now().timestamp_millis(),
                                                        event_type: ReplayEventType::Observation,
                                                        content: redact_sensitive(&obs),
                                                        metadata: Some(serde_json::json!({ "tool": name })),
                                                    }).await;
                                                }

                                                // === Hook: After Tool Call (void) ===
                                                let after_tool_ctx = savant_core::hooks::HookContext {
                                                    event: savant_core::hooks::HookEvent::AfterToolCall,
                                                    session_id: Some(sid.clone()),
                                                    agent_id: Some(self.agent_id.clone()),
                                                    tool_name: Some(name.clone()),
                                                    content: Some(obs.clone()),
                                                    error: None,
                                                    metadata: std::collections::HashMap::new(),
                                                };
                                                self.hooks.run_void(&after_tool_ctx).await;
                                                let safe_obs = savant_core::utils::parsing::scrub_secrets(&obs);
                                                let obs_msg = ChatMessage { role: ChatRole::User, content: format!("Observation ({}): {}", name, safe_obs), sender: Some("SYSTEM".to_string()), recipient: None, agent_id: None, session_id: session_id.clone(), channel: savant_core::types::AgentOutputChannel::Telemetry, is_telemetry: false, is_error: false, images: Vec::new() };
                                                history.push(obs_msg);

                                                // Report success to collective blackboard
                                                if let Some(cb) = &self.collective_blackboard {
                                                    let pressure = history.len() as f32 / 100.0;
                                                    if let Err(e) = cb.update_agent_metrics(self.agent_index, true, pressure) {
                                                        tracing::warn!("[agent::stream] Failed to update success metrics on collective blackboard: {}", e);
                                                    }
                                                }

                                                // Self-Repair: Record tool success
                                                self_repair.on_tool_result(&name, &Ok(obs.clone())).await;
                                                // Autopilot: Record successful tool call for diversity tracking
                                                self.autopilot.record(&name, &tool_args, true);
                                            }
                                            Err(e) => {
                                                turn_failed = true;
                                                // Track tool error for delta calculation
                                                self.tool_error_count.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                                                // Self-Repair: Record tool failure
                                                self_repair.on_tool_result(&name, &Err(SavantError::Unknown(format!("{}", e)))).await;
                                                // Autopilot: Record failed tool call for diversity tracking
                                                self.autopilot.record(&name, &tool_args, false);

                                                // Report failure to collective blackboard
                                                if let Some(cb) = &self.collective_blackboard {
                                                    let pressure = history.len() as f32 / 100.0;
                                                    if let Err(e) = cb.update_agent_metrics(self.agent_index, false, pressure) {
                                                        tracing::warn!("[agent::stream] Failed to update failure metrics on collective blackboard: {}", e);
                                                    }
                                                }

                                                match self.handle_heuristic_resolution(&name, e).await {
                                                    crate::react::reactor::HeuristicOutcome::Hint(hint) => {
                                                        yield Ok(AgentEvent::Observation(hint.clone()));
                                                        let hint_msg = ChatMessage {
                                                            is_telemetry: false,
                                                            role: ChatRole::User,
                                                            content: format!("Recovery Hint ({}): {}", name, hint),
                                                            sender: Some("SYSTEM".to_string()),
                                                            recipient: None,
                                                            agent_id: None,
                                                            session_id: session_id.clone(),
                                                            channel: savant_core::types::AgentOutputChannel::Telemetry,
                                                            images: Vec::new(),
                ..Default::default()
            };
                                                        history.push(hint_msg);
                                                        break 'lane_loop;
                                                    }
                                                    crate::react::reactor::HeuristicOutcome::Rollback { messages, hint } => {
                                                        info!("[{}] HEURISTIC: Rolling back history from {} to {} messages", self.agent_id, history.len(), messages.len());
                                                        history = messages;
                                                        yield Ok(AgentEvent::Observation(hint.clone()));
                                                        let hint_msg = ChatMessage {
                                                            is_telemetry: false,
                                                            role: ChatRole::User,
                                                            content: format!("Recovery Hint ({}): {}", name, hint),
                                                            sender: Some("SYSTEM".to_string()),
                                                            recipient: None,
                                                            agent_id: None,
                                                            session_id: session_id.clone(),
                                                            channel: savant_core::types::AgentOutputChannel::Telemetry,
                                                            images: Vec::new(),
                ..Default::default()
            };
                                                        history.push(hint_msg);
                                                        break 'lane_loop;
                                                    }
                                                    crate::react::reactor::HeuristicOutcome::Fatal(fatal) => {
                                                        let final_turn = savant_core::types::TurnState {
                                                            turn_id: turn_id.clone(),
                                                            session_id: sid.clone(),
                                                            state: savant_core::types::TurnPhase::Failed,
                                                            tool_calls_made: turn_tool_calls.clone(),
                                                            started_at: turn_state.started_at,
                                                            completed_at: chrono::Utc::now().timestamp_millis(),
                                                        };
                                                        if let Err(e) = self.memory.save_turn(&final_turn).await { tracing::warn!("[{}] Failed to save turn: {}", self.agent_id, e); }
                                                        session_state.active_turn_id = None;
                                                        session_state.last_active = chrono::Utc::now().timestamp_millis();
                                                        if let Err(e) = self.memory.save_session(&session_state).await { tracing::warn!("[{}] Failed to save session: {}", self.agent_id, e); }

                                                        yield Err(fatal);
                                                        return;
                                                    }
                                                }
                                            }
                                        }
                                    }
                                    tokio::select! {
                                        _ = shutdown_token.cancelled() => { return; }
                                        else => {}
                                    }
                                }
                            }
                            self.predictor.update_accuracy(k, actual_steps);
                            if self.predictor.prediction_count().is_multiple_of(5) { self.predictor.adapt_parameters(); }
                        } else {
                            let mut final_response = full_trace.clone();

                            // === Hook: After LLM Call (modifying — can modify response) ===
                            let mut after_llm_ctx = savant_core::hooks::HookContext {
                                event: savant_core::hooks::HookEvent::AfterLlmCall,
                                session_id: None,
                                agent_id: Some(self.agent_id.clone()),
                                tool_name: None,
                                content: Some(final_response.clone()),
                                error: None,
                                metadata: std::collections::HashMap::new(),
                            };
                            match self.hooks.run_modifying(&mut after_llm_ctx).await {
                                savant_core::hooks::HookResult::Modified(new_content) => {
                                    debug!("[{}] AfterLlmCall hook modified response", self.agent_id);
                                    final_response = new_content;
                                }
                                savant_core::hooks::HookResult::Cancel(reason) => {
                                    warn!("[{}] AfterLlmCall hook cancelled response: {}", self.agent_id, reason);
                                    yield Ok(AgentEvent::Observation(format!("Response blocked by guardrail: {}", reason)));
                                    break;
                                }
                                savant_core::hooks::HookResult::Unchanged => {}
                            }

                            if let Some(host) = &self.plugin_host {
                                for plugin in &self.plugins {
                                    if let Ok(res) = host.execute_before_response_emit(plugin, &final_response, self.agent_id_hash, self.security_token.clone()).await {
                                        match res {
                                            crate::plugins::wasm_host::exports::savant::agent_hooks::hooks::HookResult::Modified(new_resp) => { final_response = new_resp; }
                                            crate::plugins::wasm_host::exports::savant::agent_hooks::hooks::HookResult::Halt(reason) => {
                                                yield Err(SavantError::Unknown(format!("Halted by plugin: {}", reason)));
                                                return;
                                            }
                                            crate::plugins::wasm_host::exports::savant::agent_hooks::hooks::HookResult::Continue => {}
                                        }
                                    }
                                }
                            }

                            yield Ok(AgentEvent::FinalAnswer(clean_answer.trim().to_string()));

                            // D3: Persist assistant response immediately (crash recovery)
                            let d3_msg = ChatMessage { role: ChatRole::Assistant, content: final_response.clone(), sender: Some(self.agent_id.clone()), recipient: None, agent_id: None, session_id: session_id.clone(), channel: savant_core::types::AgentOutputChannel::Chat, is_telemetry: false, images: Vec::new(), is_error: false };
                            if let Err(e) = self.memory.store(&sid, &d3_msg).await {
                                tracing::warn!("[{}] D3: Failed to persist assistant response: {}", self.agent_id, e);
                            }

                            if let Some(collective) = &self.collective_blackboard {
                                if let Ok(mut state) = collective.read_global_state() {
                                    state.heuristic_version = state.heuristic_version.wrapping_add(1);
                                    if let Err(e) = collective.publish_global_state(state) { tracing::warn!("[{}] Failed to publish collective state: {}", self.agent_id, e); }
                                    if let Err(e) = collective.aggregate_swarm_metrics() { tracing::warn!("[{}] Failed to aggregate swarm metrics: {}", self.agent_id, e); }
                                }
                            }

                            // Persist entire conversation turn to memory atomically
                            for msg in &history {
                                if let Err(e) = self.memory.store(&sid, msg).await {
                                    tracing::warn!("[{}] Failed to persist turn message to memory: {}", self.agent_id, e);
                                }
                            }
                            let final_msg = ChatMessage { role: ChatRole::Assistant, content: final_response, sender: Some(self.agent_id.clone()), recipient: None, agent_id: None, session_id: session_id.clone(), channel: savant_core::types::AgentOutputChannel::Chat, is_telemetry: false, images: Vec::new(), is_error: false };
                            if let Err(e) = self.memory.store(&sid, &final_msg).await {
                                tracing::warn!("[{}] Failed to persist assistant response to memory: {}", self.agent_id, e);
                            }
                            break;
                        }
                        // Release circuit breaker recursion depth after tool execution
                        self.circuit_breaker.pop_recursion(&cb_task_id).await;

                        // Preserve clean_answer for post-loop reflection generation
                        last_clean_answer = clean_answer.clone();

                        depth += 1;
                        if depth >= self.max_tool_iterations as u32 {
                            let msg = format!("[SYSTEM] Maximum tool iterations ({}) reached to prevent runaway loops.", self.max_tool_iterations);
                            yield Ok(AgentEvent::FinalAnswer(msg.clone()));
                            // Persist entire turn even on max-iterations
                            for msg in &history {
                                if let Err(e) = self.memory.store(&sid, msg).await {
                                    tracing::warn!("[{}] Failed to persist turn message to memory: {}", self.agent_id, e);
                                }
                            }
                            let final_msg = ChatMessage { role: ChatRole::Assistant, content: msg, sender: Some(self.agent_id.clone()), recipient: None, agent_id: None, session_id: session_id.clone(), channel: savant_core::types::AgentOutputChannel::Chat, is_telemetry: false, images: Vec::new(), is_error: false };
                            if let Err(e) = self.memory.store(&sid, &final_msg).await {
                                tracing::warn!("[{}] Failed to persist assistant response to memory: {}", self.agent_id, e);
                            }
                            break;
                        }
                    }

                    // === Generate and emit reflection ===
                    // After all tool iterations complete, generate a reflection synthesizing
                    // the conversation and yield it as AgentEvent::Reflection for consumers
                    // (e.g. HeartbeatPulse which accumulates reflections for consciousness).
                    if !turn_failed {
                        match self.generate_reflection(&history, &last_clean_answer).await {
                            Ok(reflection) => {
                                yield Ok(AgentEvent::Reflection(reflection.clone()));
                                // Persist reflection to memory
                                let reflection_msg = ChatMessage {
                                    is_telemetry: false,
                                    role: ChatRole::Assistant,
                                    content: reflection,
                                    sender: Some(self.agent_id.clone()),
                                    recipient: None,
                                    agent_id: None,
                                    session_id: session_id.clone(),
                                    channel: savant_core::types::AgentOutputChannel::Chat,
                                    images: Vec::new(),
                ..Default::default()
            };
                                if let Err(e) = self.memory.store(&sid, &reflection_msg).await {
                                    tracing::warn!("[{}] Failed to persist reflection to memory: {}", self.agent_id, e);
                                }
                            }
                            Err(e) => {
                                debug!("[{}] Reflection generation failed (non-fatal): {}", self.agent_id, e);
                            }
                        }
                    }

                    // === Turn Finalization ===
                    let final_state = if turn_failed {
                        savant_core::types::TurnPhase::Failed
                    } else {
                        savant_core::types::TurnPhase::Completed
                    };

                    let final_turn = savant_core::types::TurnState {
                        turn_id: turn_id.clone(),
                        session_id: sid.clone(),
                        state: final_state,
                        tool_calls_made: turn_tool_calls.clone(),
                        started_at: turn_state.started_at,
                        completed_at: chrono::Utc::now().timestamp_millis(),
                    };
                    if let Err(e) = self.memory.save_turn(&final_turn).await { tracing::warn!("[{}] Failed to save turn: {}", self.agent_id, e); }

                    session_state.active_turn_id = None;
                    session_state.last_active = chrono::Utc::now().timestamp_millis();
                    if let Err(e) = self.memory.save_session(&session_state).await { tracing::warn!("[{}] Failed to save session: {}", self.agent_id, e); }

                    // === Hook: Turn End (void) ===
                    let turn_end_ctx = savant_core::hooks::HookContext {
                        event: savant_core::hooks::HookEvent::TurnEnd,
                        session_id: Some(sid.clone()),
                        agent_id: Some(self.agent_id.clone()),
                        tool_name: None,
                        content: None,
                        error: if turn_failed { Some("Turn failed".to_string()) } else { None },
                        metadata: std::collections::HashMap::new(),
                    };
                    self.hooks.run_void(&turn_end_ctx).await;

                    // Circuit breaker: emit trip history as telemetry before unregister
                    let trip_history = self.circuit_breaker.trip_history().await;
                    if !trip_history.is_empty() {
                        for trip in &trip_history {
                            warn!(
                                "[{}] Circuit breaker trip: task={}, reason={}, depth={}, calls={}, cost=${:.2}",
                                self.agent_id, trip.task_id, trip.reason, trip.recursion_depth, trip.api_call_count, trip.cumulative_cost_usd
                            );
                        }
                    }

                    // Unregister from security circuit breaker
                    self.circuit_breaker.unregister_task(&cb_task_id).await;

                    // Trajectory recording: finalize trajectory (success if no failure)
                    if let Some(ref recorder) = self.trajectory_recorder {
                        if let Err(e) = recorder.lock().await.finish(!turn_failed) {
                            tracing::warn!("[{}] Trajectory finish error: {}", self.agent_id, e);
                        }
                    }

                    yield Ok(AgentEvent::TurnEnd {
                        session_id: sid.clone(),
                        turn_id: turn_id.clone(),
                        turn_count: session_state.turn_count,
                        tool_calls: turn_tool_calls,
                    });
                }
        })
    }

    pub(crate) async fn generate_reflection(
        &self,
        history: &[ChatMessage],
        last_answer: &str,
    ) -> Result<String, SavantError> {
        let mut ref_history = history.to_vec();
        ref_history.push(ChatMessage {
            is_telemetry: false,
            role: ChatRole::Assistant,
            content: last_answer.to_string(),
            sender: Some(self.agent_id.clone()),
            recipient: None,
            agent_id: None,
            session_id: None,
            channel: savant_core::types::AgentOutputChannel::Memory,
            images: Vec::new(),
            ..Default::default()
        });

        let messages = self.context.build_messages(ref_history);
        let mut stream = self.provider.stream_completion(messages, vec![]).await?;
        let mut reflection = String::new();
        while let Some(chunk_res) = stream.next().await {
            if let Ok(chunk) = chunk_res {
                reflection.push_str(&chunk.content);
                if chunk.is_final {
                    break;
                }
            }
        }
        Ok(reflection)
    }
}

/// Find the start of a hidden tag in the buffer. Returns (position, tag_name) if found.
/// Find the earliest thought/reasoning tag in the buffer.
/// Returns (position, start_tag, end_tag) or None.
fn find_thought_tag<'a>(
    buffer: &str,
    tags: &'a [(&str, &str)],
) -> Option<(usize, &'a str, &'a str)> {
    let mut earliest: Option<(usize, &'a str, &'a str)> = None;
    for (start, end) in tags {
        if let Some(pos) = buffer.find(start) {
            if earliest.as_ref().is_none_or(|(ep, _, _)| pos < *ep) {
                earliest = Some((pos, start, end));
            }
        }
    }
    earliest
}

fn find_hidden_tag_start(buffer: &str, hidden_tags: &[&str]) -> Option<(usize, String)> {
    let mut earliest: Option<(usize, String)> = None;

    // Check for exact named tags
    for tag in hidden_tags {
        let open = format!("<{}", tag);
        if let Some(pos) = buffer.find(&open) {
            let after = buffer[pos + open.len()..].chars().next();
            if after.is_none_or(|c| c == '>' || c == ' ' || c == '/')
                && earliest.as_ref().is_none_or(|(ep, _)| pos < *ep)
            {
                earliest = Some((pos, tag.to_string()));
            }
        }
    }

    // Check for <function=...> tags (dynamic tag names like <function=file_atomic_edit>)
    if let Some(pos) = buffer.find("<function=") {
        if earliest.as_ref().is_none_or(|(ep, _)| pos < *ep) {
            earliest = Some((pos, "function".to_string()));
        }
    }

    // Check for <tool_call> tags
    if let Some(pos) = buffer.find("<tool_call>") {
        if earliest.as_ref().is_none_or(|(ep, _)| pos < *ep) {
            earliest = Some((pos, "tool_call".to_string()));
        }
    }

    earliest
}

/// Find how much of the buffer is safe to flush (no tag starting within it).
fn find_safe_flush_length(
    buffer: &str,
    thought_tags: &[(&str, &str)],
    hidden_tags: &[&str],
) -> usize {
    let mut min_pos = buffer.len();
    // Check all thought tag prefixes
    for (start_tag, _) in thought_tags {
        for i in 1..start_tag.len() {
            if buffer.ends_with(&start_tag[..i]) {
                min_pos = min_pos.min(buffer.len() - i);
            }
        }
    }
    // Check hidden tags
    for tag in hidden_tags {
        let open = format!("<{}", tag);
        for i in 1..open.len() {
            if buffer.ends_with(&open[..i]) {
                min_pos = min_pos.min(buffer.len() - i);
            }
        }
    }
    // Check function call prefixes
    for prefix in &["<function=", "<tool_call>"] {
        for i in 1..prefix.len() {
            if buffer.ends_with(&prefix[..i]) {
                min_pos = min_pos.min(buffer.len() - i);
            }
        }
    }
    min_pos
}
