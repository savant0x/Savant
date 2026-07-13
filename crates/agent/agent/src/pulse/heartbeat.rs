// SAFETY: All `clippy::disallowed_methods` violations in this file originate from
// the `serde_json::json!()` macro, which internally uses `.unwrap()` on
// compile-time-validated JSON literals. A malformed JSON literal would be a
// compile error, making the panic path statically unreachable.
#![allow(clippy::disallowed_methods)]

use crate::proactive::ProactivePartner;
use crate::react::{AgentEvent, AgentLoop};
use chrono::Timelike;
use futures::stream::StreamExt;
use savant_core::bus::NexusBridge;
use savant_core::db::Storage;
use savant_core::error::SavantError;
use savant_core::types::AgentConfig;
use savant_core::utils::{io, parsing};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};
use xxhash_rust::xxh3::xxh3_64;

/// Maximum recent thoughts to inject into CONTINUOUS_CONSCIOUSNESS section.
const MAX_RECENT_THOUGHTS: usize = 5;

/// Maximum chars per recent thought entry in the prompt.
const MAX_THOUGHT_CHARS: usize = 400;

struct HeartbeatTool;
#[async_trait::async_trait]
impl savant_core::traits::Tool for HeartbeatTool {
    fn name(&self) -> &str {
        "heartbeat"
    }
    fn description(&self) -> &str {
        "MANDATORY FIRST STEP: Evaluates whether to run or skip proactive tasks. Schema: { \"action\": \"skip\"|\"run\", \"reason\": \"...\" }"
    }
    async fn execute(&self, payload: serde_json::Value) -> Result<String, SavantError> {
        let action = payload["action"].as_str().unwrap_or_else(|| {
            warn!("[heartbeat] Missing 'action' field in heartbeat payload");
            "skip"
        });
        let reason = payload["reason"].as_str().unwrap_or("no reason provided");

        // Validate the action against environmental heuristics
        let is_valid = match action {
            "run" | "skip" => true,
            other => {
                warn!("[heartbeat] Invalid action '{}', defaulting to skip", other);
                return Ok(serde_json::json!({
                    "action": "skip",
                    "reason": format!("Invalid action '{}', defaulted to skip", other)
                })
                .to_string());
            }
        };

        if !is_valid {
            return Ok(serde_json::json!({
                "action": "skip",
                "reason": "validation failed"
            })
            .to_string());
        }

        // Return structured decision with reason for downstream logging
        Ok(serde_json::json!({
            "action": action,
            "reason": reason,
            "evaluated_at": chrono::Utc::now().to_rfc3339()
        })
        .to_string())
    }
}

struct EvaluateNotificationTool;
#[async_trait::async_trait]
impl savant_core::traits::Tool for EvaluateNotificationTool {
    fn name(&self) -> &str {
        "evaluate_notification"
    }
    fn description(&self) -> &str {
        "MANDATORY LAST STEP: Decides if the user should be notified. Schema: { \"should_notify\": true|false, \"reason\": \"...\" }"
    }
    async fn execute(&self, payload: serde_json::Value) -> Result<String, SavantError> {
        let should_notify = payload["should_notify"].as_bool().unwrap_or_else(|| {
            debug!("[heartbeat::evaluate_notification] Missing 'should_notify' field, defaulting to false");
            false
        });
        let reason = payload["reason"].as_str().unwrap_or("no reason provided");

        // Evaluate urgency flags from the payload
        let is_urgent = payload["urgent"].as_bool().unwrap_or_else(|| {
            debug!(
                "[heartbeat::evaluate_notification] Missing 'urgent' field, defaulting to false"
            );
            false
        });
        let is_anomaly = payload["anomaly"].as_bool().unwrap_or_else(|| {
            debug!(
                "[heartbeat::evaluate_notification] Missing 'anomaly' field, defaulting to false"
            );
            false
        });

        // Override: anomalies always warrant notification regardless of should_notify
        let final_decision = if is_anomaly {
            true
        } else if is_urgent {
            // Urgent items bypass quiet hours but still need a reason
            should_notify || !reason.is_empty()
        } else {
            should_notify
        };

        // Check quiet hours (22:00 - 07:00 UTC) — suppress non-urgent notifications
        let utc_hour = chrono::Utc::now().hour();
        let in_quiet_hours = !(7..22).contains(&utc_hour);
        let suppressed = in_quiet_hours && !final_decision && !is_urgent && !is_anomaly;

        Ok(serde_json::json!({
            "should_notify": final_decision && !suppressed,
            "reason": reason,
            "suppressed": suppressed,
            "quiet_hours": in_quiet_hours,
            "anomaly_override": is_anomaly,
            "evaluated_at": chrono::Utc::now().to_rfc3339()
        })
        .to_string())
    }
}

/// The Autonomous Pulse (Heartbeat) system for Savant agents.
pub struct HeartbeatPulse {
    agent: AgentConfig,
    heartbeat_file: PathBuf,
    nexus: Arc<NexusBridge>,
    storage: Arc<Storage>,
    proactive: ProactivePartner,
    shutdown_token: CancellationToken,
    delta_tx: tokio::sync::watch::Sender<f32>,
    /// Per-agent message counter for memory lifecycle (promotion, lessons, insights).
    /// Runs every 10th message per agent, not per-process.
    message_counter: std::sync::atomic::AtomicU32,
}

impl HeartbeatPulse {
    pub fn new(
        agent: AgentConfig,
        nexus: Arc<NexusBridge>,
        storage: Arc<Storage>,
        shutdown_token: CancellationToken,
        delta_tx: tokio::sync::watch::Sender<f32>,
    ) -> Self {
        let heartbeat_file = agent.workspace_path.join(&agent.proactive.heartbeat_file);
        let proactive = ProactivePartner::new(agent.workspace_path.clone(), &agent.proactive);
        Self {
            agent,
            heartbeat_file,
            nexus,
            storage,
            proactive,
            shutdown_token,
            delta_tx,
            message_counter: std::sync::atomic::AtomicU32::new(0),
        }
    }

    /// Starts the heartbeat loop for this agent.
    /// Starts the heartbeat with an Orchestrator (default path).
    /// Uses Orchestrator::execute_turn() for chat messages, which adds
    /// A2A delegation, continuation, handoff validation, and DSP prediction.
    /// The Orchestrator owns the AgentLoop internally.
    pub async fn start_with_orchestrator(
        self,
        mut orchestrator: crate::orchestration::Orchestrator,
    ) {
        use super::delta::DeltaTracker;

        // Subscribe to the dedicated command bus for user messages.
        // Isolated from the high-volume event_bus so User messages are never
        // delayed behind hundreds of streaming chunk/telemetry events.
        let mut chat_rx = self.nexus.subscribe_commands().await;
        // Also subscribe to the event bus for chat.message fallback.
        // If the command bus has zero receivers (e.g. governor deferred the
        // agent or the subscription was dropped), the gateway falls back to
        // the event bus. We listen on both to ensure messages are never lost.
        let mut event_rx = self.nexus.subscribe().await.0;
        let mut delta_tracker = DeltaTracker::new();
        const DELTA_THRESHOLD: f32 = 0.3;
        const CHECK_INTERVAL_SECS: u64 = 30;
        use savant_dream::IS_DREAMING;
        use std::sync::atomic::Ordering;
        let delta_tx = self.delta_tx.clone();
        // Dedup set: content hash of recently processed messages to prevent
        // double-processing when the same message arrives on both buses.
        let mut seen_hashes: std::collections::HashSet<u64> = std::collections::HashSet::new();

        info!(
            "[{}] Heartbeat loop active (orchestrator mode, delta-threshold={}, dream-aware, dual-bus)",
            self.agent.agent_name, DELTA_THRESHOLD
        );

        let mut event_counter: u64 = 0;
        loop {
            tokio::select! {
                // PRIMARY: Command bus (fast path, no chunk noise)
                chat_res = chat_rx.recv() => {
                    event_counter += 1;
                    let chat_event = match chat_res {
                        Ok(event) => event,
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                            tracing::warn!(
                                "[{}] COMMAND BUS LAGGED: dropped {} messages (counter={}) — drain and retry",
                                self.agent.agent_name, n, event_counter
                            );
                            continue;
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                            tracing::error!("[{}] Command bus closed after {} events — falling back to event bus only", self.agent.agent_name, event_counter);
                            // Don't break — continue listening on event bus
                            continue;
                        }
                    };
                    if let Ok(message) = serde_json::from_str::<
                        savant_core::types::ChatMessage,
                    >(&chat_event.payload)
                    {
                        if message.role == savant_core::types::ChatRole::User {
                            // Dedup: skip if we already processed this message via the event bus
                            let content_hash = xxh3_64(chat_event.payload.as_bytes());
                            if !seen_hashes.insert(content_hash) {
                                debug!("[{}] Skipping duplicate message from command bus (hash={:016x})", self.agent.agent_name, content_hash);
                                continue;
                            }
                            // Prune old hashes to prevent unbounded growth
                            if seen_hashes.len() > 100 {
                                // Retain last 50 to avoid gap where duplicates can slip through
                                let drain_count = seen_hashes.len() - 50;
                                let to_remove: Vec<u64> = seen_hashes.iter().take(drain_count).copied().collect();
                                for h in to_remove {
                                    seen_hashes.remove(&h);
                                }
                            }
                            delta_tracker.record_message();

                            self.handle_orchestrator_message(
                                &mut orchestrator,
                                &message,
                                event_counter,
                            ).await;
                        }
                    }
                }
                // FALLBACK: Event bus chat.message — only processed if command bus
                // didn't deliver it first (dedup via content hash).
                event_res = event_rx.recv() => {
                    let event_frame = match event_res {
                        Ok(event) => event,
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                            tracing::warn!("[{}] Event bus lagged by {} messages", self.agent.agent_name, n);
                            continue;
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                            tracing::error!("[{}] Event bus closed — exiting heartbeat loop", self.agent.agent_name);
                            break;
                        }
                    };
                    if event_frame.event_type == "chat.message" {
                        if let Ok(message) = serde_json::from_str::<savant_core::types::ChatMessage>(&event_frame.payload) {
                            if message.role == savant_core::types::ChatRole::User {
                                // Dedup: skip if command bus already delivered this message
                                let content_hash = xxh3_64(event_frame.payload.as_bytes());
                                if !seen_hashes.insert(content_hash) {
                                    debug!("[{}] Skipping duplicate message from event bus (hash={:016x})", self.agent.agent_name, content_hash);
                                    continue;
                                }
                                if seen_hashes.len() > 100 {
                                    // Retain last 50 to avoid gap where duplicates can slip through
                                    let drain_count = seen_hashes.len() - 50;
                                    let to_remove: Vec<u64> = seen_hashes.iter().take(drain_count).copied().collect();
                                    for h in to_remove {
                                        seen_hashes.remove(&h);
                                    }
                                }
                                event_counter += 1;
                                delta_tracker.record_message();

                                // Re-publish to command bus so other subscribers can see it
                                let _ = self.nexus.publish_command(&event_frame.payload).await;

                                self.handle_orchestrator_message(
                                    &mut orchestrator,
                                    &message,
                                    event_counter,
                                ).await;
                            }
                        }
                    }
                }
                _ = tokio::time::sleep(std::time::Duration::from_secs(CHECK_INTERVAL_SECS)) => {
                    // Dream awareness
                    if IS_DREAMING.load(Ordering::Relaxed) {
                        continue;
                    }

                    // NA-03: Check orchestrator subagent health and evacuate dead ones
                    let dead_subagents = orchestrator.check_swarm_health().await;
                    for dead_id in &dead_subagents {
                        if let Err(e) = orchestrator.evacuate_subagent(dead_id).await {
                            tracing::warn!(
                                agent_id = %orchestrator.agent_id(),
                                subagent = %dead_id,
                                error = %e,
                                "Failed to evacuate dead subagent"
                            );
                        } else {
                            info!(
                                agent_id = %orchestrator.agent_id(),
                                subagent = %dead_id,
                                "Evacuated dead subagent"
                            );
                        }
                    }

                    // Compute and publish delta score for dream scheduler
                    let git_diff_output = self.get_git_diff_output().await;
                    let git_hash = xxhash_rust::xxh3::xxh3_64(git_diff_output.as_bytes());
                    let _git_changed = delta_tracker.update_git_hash(git_hash);
                    let git_lines = Self::parse_git_lines_changed(&git_diff_output);

                    let fs_snapshot = self.build_fs_snapshot().await;
                    let files_modified = delta_tracker.update_fs_snapshot(fs_snapshot);

                    let delta = delta_tracker.compute_and_reset(git_lines, files_modified);
                    let score = delta.score();

                    if let Err(e) = delta_tx.send(score) {
                        tracing::warn!("[heartbeat:orchestrator] Failed to send delta score: {}", e);
                    }
                }
            }
        }
    }

    /// Starts the heartbeat loop with an AgentLoop (non-orchestrator path).
    ///
    /// Subscribes to **two** Nexus buses for message isolation:
    /// 1. **Command bus** (`subscribe_commands`): Receives `chat.message` events (User messages)
    ///    without having to drain through high-volume streaming chunk/telemetry events.
    /// 2. **Event bus** (`subscribe`): Receives `pulse.trigger` and other non-message events.
    ///
    /// `chat.message` events that arrive on the event bus are silently ignored — they are
    /// handled exclusively via the command bus to prevent double processing.
    ///
    /// The command bus has a lower capacity (1024) than the event bus (16384) because
    /// it only carries low-volume user messages, never streaming chunks or telemetry.
    pub async fn start<M: savant_core::traits::MemoryBackend + std::clone::Clone>(
        self,
        mut agent_loop: AgentLoop<M>,
    ) {
        use super::delta::DeltaTracker;

        // Subscribe to dedicated command bus for User chat messages (no chunk noise)
        let mut cmd_rx = self.nexus.subscribe_commands().await;
        // Subscribe to main event bus for pulse.trigger and other non-message events
        let mut event_rx = self.nexus.subscribe().await.0;

        // Delta tracker for threshold-based activation
        let mut delta_tracker = DeltaTracker::new();
        const DELTA_THRESHOLD: f32 = 0.3;
        const CHECK_INTERVAL_SECS: u64 = 30;

        // Track tool errors from previous agent runs for delta scoring
        let mut prev_tool_error_count: u32 = 0;

        // Voice pulse: audio event pipeline for voice integration
        let voice_pulse: Box<dyn super::audio::AudioPipeline> =
            Box::new(super::audio::NoopAudioPipeline::new());
        let mut voice_rx = voice_pulse.subscribe();
        voice_pulse.start();

        // Dream engine awareness: check IS_DREAMING flag before pulse
        use savant_dream::IS_DREAMING;
        use std::sync::atomic::Ordering;

        // Delta score channel for dream scheduler — receiver is held by DreamScheduler
        let delta_tx = self.delta_tx.clone();

        info!(
            "[{}] Heartbeat loop active (delta-threshold mode, threshold={}, dream-aware, command-bus)",
            self.agent.agent_name, DELTA_THRESHOLD
        );

        loop {
            tokio::select! {
                // 1. Command bus: User chat messages (no chunk noise to drain through!)
                cmd_res = cmd_rx.recv() => {
                    let chat_event = match cmd_res {
                        Ok(event) => event,
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                            tracing::warn!(
                                "[{}] Command bus lagged by {} messages — draining and retrying",
                                self.agent.agent_name, n
                            );
                            continue;
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                            tracing::error!("[{}] Command bus closed — exiting heartbeat loop", self.agent.agent_name);
                            break;
                        }
                    };
                    delta_tracker.record_message();
                    debug!("[{}] Heartbeat received chat.message from command bus", self.agent.agent_name);
                    if let Err(e) = self.handle_chat_message(chat_event.payload, &mut agent_loop).await {
                        parsing::log_agent_error(&self.agent.agent_name, "Failed to handle chat message", e);
                    }
                }

                // 2. Event bus: pulse.trigger and other non-message events
                //     chat.message events arriving here are ignored — they're handled via command bus
                event_res = event_rx.recv() => {
                    let chat_event = match event_res {
                        Ok(event) => event,
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                            tracing::warn!(
                                "[{}] Event receiver lagged by {} messages — draining and retrying",
                                self.agent.agent_name, n
                            );
                            continue;
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                            tracing::error!("[{}] Event bus closed — exiting heartbeat loop", self.agent.agent_name);
                            break;
                        }
                    };
                    if chat_event.event_type == "pulse.trigger" {
                        info!("[{}] External PULSE trigger received. Forcing cycle...", self.agent.agent_name);
                        let forced_lens = match serde_json::from_str::<serde_json::Value>(&chat_event.payload) {
                            Ok(v) => v["lens"].as_str().map(|s| s.to_string()),
                            Err(e) => {
                                debug!("[{}] Malformed pulse trigger payload: {}", self.agent.agent_name, e);
                                None
                            }
                        };

                        if let Err(e) = self.pulse_with_lens(&mut agent_loop, forced_lens).await {
                            parsing::log_agent_error(&self.agent.agent_name, "Manual pulse failed", e);
                        }
                    }
                }

                // 2. Delta-check pulse (replaces fixed 60s timer)
                _ = tokio::time::sleep(Duration::from_secs(CHECK_INTERVAL_SECS)) => {
                    // Dream engine awareness: skip pulse if dream cycle is active
                    if IS_DREAMING.load(Ordering::SeqCst) {
                        debug!("[{}] Pulse skipped (dream cycle active)", self.agent.agent_name);
                        continue;
                    }

                    // Record tool errors from the previous agent run (if any)
                    let current_error_count = agent_loop.tool_error_count.load(std::sync::atomic::Ordering::Relaxed);
                    let new_errors = current_error_count.saturating_sub(prev_tool_error_count);
                    for _ in 0..new_errors {
                        delta_tracker.record_tool_error();
                    }
                    prev_tool_error_count = current_error_count;

                    // Compute environmental delta using DeltaTracker methods
                    // (replaces direct git shell-outs for incremental state tracking)
                    let git_diff_output = self.get_git_diff_output().await;
                    let git_hash = xxhash_rust::xxh3::xxh3_64(git_diff_output.as_bytes());
                    let _git_changed = delta_tracker.update_git_hash(git_hash);
                    let git_lines = Self::parse_git_lines_changed(&git_diff_output);

                    let fs_snapshot = self.build_fs_snapshot().await;
                    let files_modified = delta_tracker.update_fs_snapshot(fs_snapshot);

                    let delta = delta_tracker.compute_and_reset(git_lines, files_modified);
                    let score = delta.score();

                    // Publish delta score for dream scheduler
                    if let Err(e) = delta_tx.send(score) {
                        tracing::warn!("[heartbeat] Failed to send delta score: {}", e);
                    }

                    if delta.should_activate(DELTA_THRESHOLD) {
                        info!(
                            "[{}] Pulse activated (delta={:.2}, git={}, fs={}, msgs={}, errors={}, age={}m)",
                            self.agent.agent_name, score,
                            delta.git_lines_changed, delta.files_modified,
                            delta.new_messages, delta.tool_errors,
                            delta.minutes_since_last_pulse
                        );
                        if let Err(e) = self.pulse_with_lens(&mut agent_loop, None).await {
                            parsing::log_agent_error(&self.agent.agent_name, "Heartbeat pulse failed", e);
                        }
                    } else {
                        debug!(
                            "[{}] Pulse skipped (delta={:.2}, threshold={})",
                            self.agent.agent_name, score, DELTA_THRESHOLD
                        );
                    }
                }

                // 3. Audio events from VoicePulse
                Ok(audio_event) = voice_rx.recv() => {
                    match audio_event {
                        super::audio::AudioEvent::VoiceReady => {
                            debug!("[{}] Voice pipeline ready", self.agent.agent_name);
                        }
                        super::audio::AudioEvent::TranscriptReceived(transcript) => {
                            info!("[{}] Voice transcript: {}", self.agent.agent_name, transcript);
                            delta_tracker.record_message();
                        }
                        super::audio::AudioEvent::PlaybackComplete => {
                            debug!("[{}] Voice playback complete", self.agent.agent_name);
                        }
                    }
                }

                // 4. Graceful Shutdown
                _ = self.shutdown_token.cancelled() => {
                    info!("[{}] Heartbeat loop received shutdown signal. Evacuating...", self.agent.agent_name);
                    break;
                }
            }
        }
    }

    /// Returns raw `git diff --stat HEAD` output for hashing and parsing.
    async fn get_git_diff_output(&self) -> String {
        let path = &self.agent.workspace_path;
        if let Ok(output) = tokio::process::Command::new("git")
            .args(["diff", "--stat", "HEAD"])
            .current_dir(path)
            .output()
            .await
        {
            String::from_utf8_lossy(&output.stdout).to_string()
        } else {
            String::new()
        }
    }

    /// Parses git diff --stat output to extract total lines changed.
    fn parse_git_lines_changed(text: &str) -> usize {
        if let Some(line) = text.lines().last() {
            let mut total = 0;
            for part in line.split(',') {
                let part = part.trim();
                if let Some(num) = part.split_whitespace().next() {
                    if let Ok(n) = num.parse::<usize>() {
                        total += n;
                    }
                }
            }
            return total;
        }
        0
    }

    /// Builds a filesystem snapshot of (path, hash) pairs from git status.
    /// Used by DeltaTracker::update_fs_snapshot() for incremental change detection.
    async fn build_fs_snapshot(&self) -> Vec<(String, u64)> {
        let path = &self.agent.workspace_path;
        if let Ok(output) = tokio::process::Command::new("git")
            .args(["status", "--short"])
            .current_dir(path)
            .output()
            .await
        {
            let text = String::from_utf8_lossy(&output.stdout);
            text.lines()
                .filter(|l| !l.trim().is_empty())
                .map(|l| {
                    let path_str = l.trim().to_string();
                    let hash = xxhash_rust::xxh3::xxh3_64(path_str.as_bytes());
                    (path_str, hash)
                })
                .collect()
        } else {
            Vec::new()
        }
    }

    /// Shared orchestrator message processing — called from both the command bus
    /// and event bus branches in `start_with_orchestrator`. Handles thinking telemetry,
    /// chunk streaming, execute_turn, and response publishing.
    async fn handle_orchestrator_message(
        &self,
        orchestrator: &mut crate::orchestration::Orchestrator,
        message: &savant_core::types::ChatMessage,
        event_counter: u64,
    ) {
        let agent_name = self.agent.agent_name.clone();
        let msg_preview = message.content.chars().take(60).collect::<String>();
        info!(
            "[{}] ORCHESTRATOR RECEIVED User message #{}: content=\"{}\" session={:?}",
            agent_name, event_counter, msg_preview, message.session_id
        );

        // FID-20260530: Publish interim telemetry so the dashboard shows thinking state.
        {
            let thinking_chunk = savant_core::types::ChatChunk {
                agent_name: self.agent.agent_name.clone(),
                agent_id: self.agent.agent_id.to_lowercase(),
                content: "Processing your message...".to_string(),
                is_final: false,
                session_id: message.session_id.clone(),
                channel: savant_core::types::AgentOutputChannel::Telemetry,
                logprob: None,
                is_telemetry: true,
                reasoning: None,
                tool_calls: None,
            };
            if let Ok(payload) = serde_json::to_string(&thinking_chunk) {
                let _ = self.nexus.publish("chat.chunk", &payload).await;
            }
        }

        // Create chunk channel to receive response text in real-time
        let (chunk_tx, mut chunk_rx) =
            tokio::sync::mpsc::unbounded_channel::<String>();
        orchestrator.set_chunk_tx(chunk_tx);

        // Spawn chunk forwarding task: stream chunks to Nexus as ChatChunks
        let nexus_clone = self.nexus.clone();
        let agent_name_clone = self.agent.agent_name.clone();
        let agent_id_clone = self.agent.agent_id.to_lowercase();
        let session_id_clone = message.session_id.clone();
        let chunk_fwd = tokio::spawn(async move {
            while let Some(chunk_text) = chunk_rx.recv().await {
                let chunk = savant_core::types::ChatChunk {
                    agent_name: agent_name_clone.clone(),
                    agent_id: agent_id_clone.clone(),
                    content: chunk_text,
                    is_final: false,
                    session_id: session_id_clone.clone(),
                    channel: savant_core::types::AgentOutputChannel::Chat,
                    logprob: None,
                    is_telemetry: false,
                    reasoning: None,
                    tool_calls: None,
                };
                if let Ok(payload) = serde_json::to_string(&chunk) {
                    let _ = nexus_clone.publish("chat.chunk", &payload).await;
                }
            }
        });

        let mut had_error = false;
        const EXECUTE_TURN_TIMEOUT_SECS: u64 = 180;
        info!("[{}] ORCHESTRATOR CALLING execute_turn input_len={}", agent_name, message.content.len());
        let response_text = match tokio::time::timeout(
            std::time::Duration::from_secs(EXECUTE_TURN_TIMEOUT_SECS),
            orchestrator.execute_turn(&message.content),
        ).await {
            Ok(Ok(text)) => {
                info!("[{}] ORCHESTRATOR execute_turn SUCCEEDED output_len={}", agent_name, text.len());
                text
            }
            Ok(Err(e)) => {
                had_error = true;
                tracing::warn!("[{}] ORCHESTRATOR execute_turn FAILED: {}", agent_name, e);
                let error_response = savant_core::types::ChatMessage {
                    role: savant_core::types::ChatRole::Assistant,
                    content: format!("I encountered an error processing your message: {}. Please try again.", e),
                    sender: Some(self.agent.agent_id.clone()),
                    recipient: message.sender.clone(),
                    agent_id: Some(self.agent.agent_id.clone()),
                    session_id: message.session_id.clone(),
                    channel: savant_core::types::AgentOutputChannel::Chat,
                    is_telemetry: false,
                    is_error: true,
                    images: Vec::new(),
                };
                if let Ok(payload) = serde_json::to_string(&error_response) {
                    let _ = self.nexus.publish("chat.message", &payload).await;
                }
                String::new()
            }
            Err(_timeout) => {
                had_error = true;
                tracing::error!(
                    "[{}] ORCHESTRATOR execute_turn TIMED OUT after {}s — aborting turn",
                    agent_name, EXECUTE_TURN_TIMEOUT_SECS
                );
                let timeout_response = savant_core::types::ChatMessage {
                    role: savant_core::types::ChatRole::Assistant,
                    content: "I'm sorry, your request took too long to process. Please try a simpler message.".to_string(),
                    sender: Some(self.agent.agent_id.clone()),
                    recipient: message.sender.clone(),
                    agent_id: Some(self.agent.agent_id.clone()),
                    session_id: message.session_id.clone(),
                    channel: savant_core::types::AgentOutputChannel::Chat,
                    is_telemetry: false,
                    is_error: true,
                    images: Vec::new(),
                };
                if let Ok(payload) = serde_json::to_string(&timeout_response) {
                    let _ = self.nexus.publish("chat.message", &payload).await;
                }
                String::new()
            }
        };

        // Drop the chunk sender so the forwarding task exits its recv() loop
        orchestrator.take_chunk_tx();

        // Wait for the forwarding task to drain all remaining chunks
        let _ = chunk_fwd.await;

        // Publish final completion message to signal the turn is done.
        if !had_error {
            let completion = savant_core::types::ChatMessage {
                role: savant_core::types::ChatRole::Assistant,
                content: if response_text.is_empty() {
                    "I received your message.".to_string()
                } else {
                    response_text
                },
                sender: Some(self.agent.agent_id.clone()),
                recipient: message.sender.clone(),
                agent_id: Some(self.agent.agent_id.clone()),
                session_id: message.session_id.clone(),
                channel: savant_core::types::AgentOutputChannel::Chat,
                is_telemetry: false,
                images: Vec::new(),
                ..Default::default()
            };
            if let Ok(payload) = serde_json::to_string(&completion) {
                let _ = self.nexus.publish("chat.message", &payload).await;
            }
        }

        // Yield to flush pending broadcast events after long-running execute_turn
        tokio::task::yield_now().await;
    }

    async fn handle_chat_message<M: savant_core::traits::MemoryBackend + Clone>(
        &self,
        chat_event: String,
        agent_loop: &mut AgentLoop<M>,
    ) -> Result<(), SavantError> {
        let chat_message: Result<savant_core::types::ChatMessage, _> =
            serde_json::from_str(&chat_event);

        match &chat_message {
            Ok(message) => {
                // D1: Structured tracing at message entry (FID-20260529)
                tracing::info!(
                    "[{}] INBOUND chat.message: sender={:?}, session={:?}, content_len={}",
                    self.agent.agent_name,
                    message.sender,
                    message.session_id,
                    message.content.len()
                );

                let content = message.content.clone();
                let sender = message.sender.clone();
                let agent_id = message.agent_id.clone();

                // 🛡️ Identity Pinning: Block Echo-Back (Normalized & Prefix-Aware)
                let my_id = self.agent.agent_id.to_lowercase();
                let my_name = self.agent.agent_name.to_lowercase();

                if let Some(ref s_raw) = sender {
                    let s = s_raw.to_lowercase();
                    // Check for direct match or platform-prefixed match (e.g., discord:ID)
                    let is_self = s == my_id || s == my_name || s.ends_with(&format!(":{}", my_id));
                    if is_self {
                        // A1: Log identity pinning drop (FID-20260529)
                        tracing::warn!(
                            "[{}] Identity pinning: dropped message from self (sender={:?}, agent_id={:?}, content_preview={})",
                            self.agent.agent_name,
                            s_raw,
                            agent_id,
                            &message.content[..message.content.len().min(80)]
                        );
                        return Ok(());
                    }
                }

                if let Some(ref sid_raw) = agent_id {
                    let sid = sid_raw.to_lowercase();
                    if sid == my_id || sid == my_name {
                        // A1: Log identity pinning drop (FID-20260529)
                        tracing::warn!(
                            "[{}] Identity pinning: dropped message targeted at self (agent_id={:?}, content_preview={})",
                            self.agent.agent_name,
                            sid_raw,
                            &message.content[..message.content.len().min(80)]
                        );
                        return Ok(());
                    }
                }

                // 🌌 Universal Eavesdropping: Processing all messages in lane.
                info!(
                    "[{}] Eavesdropping on message from {:?}: {}",
                    self.agent.agent_name, sender, content
                );

                let response_recipient = sender;

                // NLP: Parse command intent from user message
                let cmd = crate::nlp::parse_command(&content);
                if cmd.confidence > 0.5 {
                    info!(
                        "[{}] NLP command detected: category={:?}, action={}",
                        self.agent.agent_name, cmd.category, cmd.action
                    );
                }

                // Process the message through Agent loop
                let mut full_response = String::new();
                let mut full_trace = String::new();
                let memory_clone = agent_loop.memory.clone();
                let user_input = content.clone();

                // FID-20260530: Publish interim telemetry before LLM call.
                // This cancels the dashboard's response timeout so the user sees
                // the agent is processing rather than a silent TIMEOUT.
                {
                    let thinking_chunk = savant_core::types::ChatChunk {
                        agent_name: self.agent.agent_name.clone(),
                        agent_id: self.agent.agent_id.to_lowercase(),
                        content: "Processing your message...".to_string(),
                        is_final: false,
                        session_id: message.session_id.clone(),
                        channel: savant_core::types::AgentOutputChannel::Telemetry,
                        logprob: None,
                        is_telemetry: true,
                        reasoning: None,
                        tool_calls: None,
                    };
                    if let Ok(payload) = serde_json::to_string(&thinking_chunk) {
                        let _ = self.nexus.publish("chat.chunk", &payload).await;
                    }
                }

                {
                    let shutdown_token = self.shutdown_token.clone();
                    let mut stream =
                        agent_loop.run(content, message.session_id.clone(), shutdown_token.clone());
                    while let Some(event_res) = stream.next().await {
                        // Perfection: Yield immediately if shutdown is requested
                        if shutdown_token.is_cancelled() {
                            return Ok(());
                        }

                        match event_res {
                            Ok(AgentEvent::Thought(t)) => {
                                // Accumulate thought content as trace for fallback response
                                full_trace.push_str(&t);
                                // 🛡️ Perfection Loop: Thoughts are strictly telemetry
                                let chunk = savant_core::types::ChatChunk {
                                    agent_name: self.agent.agent_name.clone(),
                                    agent_id: self.agent.agent_id.to_lowercase(),
                                    content: t,
                                    is_final: false,
                                    session_id: message.session_id.clone(),
                                    channel: savant_core::types::AgentOutputChannel::Telemetry,
                                    logprob: None,
                                    is_telemetry: true,
                                    reasoning: None,
                                    tool_calls: None,
                                };
                                if let Ok(payload) = serde_json::to_string(&chunk) {
                                    if let Err(e) = self.nexus.publish("chat.chunk", &payload).await
                                    {
                                        tracing::warn!(
                                            "[{}] Failed to publish telemetry: {}",
                                            self.agent.agent_name,
                                            e
                                        );
                                    }
                                }
                            }
                            Ok(AgentEvent::Action { name, args }) => {
                                info!(
                                    "[{}] Chat Action: {}[{}]",
                                    self.agent.agent_name, name, args
                                );
                                // 🛰️ Real-time Tool Telemetry
                                let chunk = savant_core::types::ChatChunk {
                                    agent_name: self.agent.agent_name.clone(),
                                    agent_id: self.agent.agent_id.to_lowercase(),
                                    content: format!(
                                        "\n\n> 🛠️ **Executing Tool:** `{}`\n> *Args:* `{}`\n\n",
                                        name, args
                                    ),
                                    is_final: false,
                                    session_id: message.session_id.clone(),
                                    channel: savant_core::types::AgentOutputChannel::Telemetry,
                                    logprob: None,
                                    is_telemetry: true,
                                    reasoning: None,
                                    tool_calls: None,
                                };
                                if let Ok(payload) = serde_json::to_string(&chunk) {
                                    if let Err(e) = self.nexus.publish("chat.chunk", &payload).await
                                    {
                                        tracing::warn!(
                                            "[{}] Failed to publish telemetry: {}",
                                            self.agent.agent_name,
                                            e
                                        );
                                    }
                                }
                            }
                            Ok(AgentEvent::Reflection(r)) => {
                                // 🛰️ Memory Channel Telemetry
                                let chunk = savant_core::types::ChatChunk {
                                    agent_name: self.agent.agent_name.clone(),
                                    agent_id: self.agent.agent_id.to_lowercase(),
                                    content: r.clone(),
                                    is_final: false,
                                    session_id: message.session_id.clone(),
                                    channel: savant_core::types::AgentOutputChannel::Memory,
                                    logprob: None,
                                    is_telemetry: false,
                                    reasoning: None,
                                    tool_calls: None,
                                };
                                if let Ok(payload) = serde_json::to_string(&chunk) {
                                    if let Err(e) = self.nexus.publish("chat.chunk", &payload).await
                                    {
                                        tracing::warn!(
                                            "[{}] Failed to publish telemetry: {}",
                                            self.agent.agent_name,
                                            e
                                        );
                                    }
                                }

                                let emitter = crate::learning::emitter::LearningEmitter::new(
                                    self.agent.agent_id.clone(),
                                    memory_clone.clone(),
                                    self.nexus.clone(),
                                    self.agent.workspace_path.clone(),
                                );
                                if let Err(e) = emitter.emit_emergent(r, None).await {
                                    tracing::warn!(
                                        "[{}] Failed to emit emergent learning: {}",
                                        self.agent.agent_name,
                                        e
                                    );
                                }
                            }
                            Ok(AgentEvent::Observation(o)) => {
                                debug!("[{}] Observation: {}", self.agent.agent_name, o);
                                // 🛰️ Observation Telemetry
                                let chunk = savant_core::types::ChatChunk {
                                    agent_name: self.agent.agent_name.clone(),
                                    agent_id: self.agent.agent_id.to_lowercase(),
                                    content: format!("\n> 👁️ **Observation:** *Successful acquisition of {} context bytes.*\n\n", o.len()),
                                    is_final: false,
                                    session_id: message.session_id.clone(),
                                    channel: savant_core::types::AgentOutputChannel::Telemetry,
                                    logprob: None,
                                    is_telemetry: true,
                                    reasoning: None,
                                    tool_calls: None,
                                };
                                if let Ok(payload) = serde_json::to_string(&chunk) {
                                    if let Err(e) = self.nexus.publish("chat.chunk", &payload).await
                                    {
                                        tracing::warn!(
                                            "[{}] Failed to publish telemetry: {}",
                                            self.agent.agent_name,
                                            e
                                        );
                                    }
                                }
                            }
                            Ok(AgentEvent::FinalAnswer(a)) => {
                                // 🛡️ Perfection Loop: Final answer should supplement, not overwrite if we've been streamingChunks
                                if full_response.trim().is_empty() {
                                    full_response = a;
                                }
                            }
                            Ok(AgentEvent::FinalAnswerChunk(c)) => {
                                // 🌀 Perfection Loop: Assistant final chunks are GUARANTEED dialogue
                                full_response.push_str(&c);

                                let chunk = savant_core::types::ChatChunk {
                                    agent_name: self.agent.agent_name.clone(),
                                    agent_id: self.agent.agent_id.to_lowercase(),
                                    content: c,
                                    is_final: false,
                                    session_id: message.session_id.clone(),
                                    channel: savant_core::types::AgentOutputChannel::Chat,
                                    logprob: None,
                                    is_telemetry: false,
                                    reasoning: None,
                                    tool_calls: None,
                                };
                                if let Ok(payload) = serde_json::to_string(&chunk) {
                                    if let Err(e) = self.nexus.publish("chat.chunk", &payload).await
                                    {
                                        tracing::warn!(
                                            "[{}] Failed to publish observation telemetry: {}",
                                            self.agent.agent_name,
                                            e
                                        );
                                    }
                                }
                            }
                            Ok(AgentEvent::StatusUpdate(s)) => {
                                // 🛰️ Status events are Telemetry
                                let chunk = savant_core::types::ChatChunk {
                                    agent_name: self.agent.agent_name.clone(),
                                    agent_id: self.agent.agent_id.to_lowercase(),
                                    content: s,
                                    is_final: false,
                                    session_id: message.session_id.clone(),
                                    channel: savant_core::types::AgentOutputChannel::Telemetry,
                                    logprob: None,
                                    is_telemetry: true,
                                    reasoning: None,
                                    tool_calls: None,
                                };
                                if let Ok(payload) = serde_json::to_string(&chunk) {
                                    if let Err(e) = self.nexus.publish("chat.chunk", &payload).await
                                    {
                                        tracing::warn!(
                                            "[{}] Failed to publish status telemetry: {}",
                                            self.agent.agent_name,
                                            e
                                        );
                                    }
                                }
                            }
                            Err(e) => {
                                // B1: Publish error response to user (FID-20260529)
                                tracing::error!(
                                    "[{}] Agent Loop Error: {}",
                                    self.agent.agent_name,
                                    e
                                );
                                let error_response = savant_core::types::ChatMessage {
                                    role: savant_core::types::ChatRole::Assistant,
                                    content: format!("I encountered an error processing your message: {}. Please try again.", e),
                                    sender: Some(self.agent.agent_id.clone()),
                                    recipient: response_recipient.clone(),
                                    agent_id: Some(self.agent.agent_id.clone()),
                                    session_id: message.session_id.clone(),
                                    channel: savant_core::types::AgentOutputChannel::Chat,
                                    is_telemetry: false,
                                    is_error: true,
                                    images: Vec::new(),
                                };
                                if let Ok(payload) = serde_json::to_string(&error_response) {
                                    if let Err(pub_err) =
                                        self.nexus.publish("chat.message", &payload).await
                                    {
                                        tracing::warn!(
                                            "[{}] Failed to publish error response: {}",
                                            self.agent.agent_name,
                                            pub_err
                                        );
                                    }
                                }
                                // Return Ok to avoid killing the heartbeat loop
                                return Ok(());
                            }
                            _ => {
                                // SessionStart, TurnEnd, and future events — handled silently
                            }
                        }
                    }
                }

                // Send response back through Nexus: Standardized at chat.message
                // If the agent loop produced no conversational response (e.g., went into tool loop),
                // fall back to full_trace (thought content) if available, then to a generic acknowledgment.
                let response_content = if full_response.trim().is_empty() {
                    if full_trace.trim().is_empty() {
                        format!(
                            "I received your message: \"{}\". I'm processing it internally.",
                            user_input.chars().take(100).collect::<String>()
                        )
                    } else {
                        full_trace
                    }
                } else {
                    full_response
                };

                let response = savant_core::types::ChatMessage {
                    role: savant_core::types::ChatRole::Assistant,
                    content: response_content,
                    sender: Some(self.agent.agent_id.clone()),
                    recipient: response_recipient,
                    agent_id: Some(self.agent.agent_id.clone()),
                    session_id: message.session_id.clone(),
                    channel: savant_core::types::AgentOutputChannel::Chat,
                    is_telemetry: false,
                    images: Vec::new(),
                    ..Default::default()
                };

                let response_payload = serde_json::to_string(&response)?;
                self.nexus
                    .publish("chat.message", &response_payload)
                    .await
                    .map_err(|e| SavantError::Unknown(e.to_string()))?;

                info!(
                    "[{}] Chat response sent (Standardized Lane)",
                    self.agent.agent_name
                );

                // Memory lifecycle: consolidate after each turn
                if let Err(e) = memory_clone.consolidate(&self.agent.agent_id).await {
                    tracing::warn!(
                        "[{}] Memory consolidation failed: {}",
                        self.agent.agent_name,
                        e
                    );
                }

                // Memory lifecycle: promotion, lessons, insights (every 10th message)
                let count = self
                    .message_counter
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                if count.is_multiple_of(10) {
                    if let Err(e) = memory_clone.run_promotion_cycle(&self.agent.agent_id).await {
                        tracing::warn!("[{}] Promotion cycle failed: {}", self.agent.agent_name, e);
                    }
                    if let Err(e) = memory_clone.synthesize_lessons(&self.agent.agent_id).await {
                        tracing::warn!(
                            "[{}] Lesson synthesis failed: {}",
                            self.agent.agent_name,
                            e
                        );
                    }
                    if let Err(e) = memory_clone.synthesize_insights(&self.agent.agent_id).await {
                        tracing::warn!(
                            "[{}] Insight synthesis failed: {}",
                            self.agent.agent_name,
                            e
                        );
                    }
                    info!(
                        "[{}] Memory lifecycle: promotion + lessons + insights completed",
                        self.agent.agent_name
                    );
                }
            }
            Err(e) => {
                info!(
                    "[{}] Invalid chat message format: {}",
                    self.agent.agent_name, e
                );
            }
        }

        Ok(())
    }

    /// Loads the agent's most recent prior thoughts from the proactive buffer.
    /// Returns a formatted `<CONTINUOUS_CONSCIOUSNESS>` XML section with up to
    /// `MAX_RECENT_THOUGHTS` entries, or an empty string if the buffer is empty.
    fn load_recent_thoughts(buffer: &crate::proactive::WorkingBuffer) -> String {
        if buffer.recent_thoughts.is_empty() {
            return String::new();
        }

        let entries: Vec<String> = buffer
            .recent_thoughts
            .iter()
            .rev()
            .take(MAX_RECENT_THOUGHTS)
            .map(|rt| {
                let truncated: String = rt.content.chars().take(MAX_THOUGHT_CHARS).collect();
                let ellipsis = if rt.content.len() > MAX_THOUGHT_CHARS {
                    "..."
                } else {
                    ""
                };
                match chrono::DateTime::from_timestamp(rt.timestamp, 0) {
                    Some(dt) => format!(
                        "[Pulse — {} UTC]: {}{}",
                        dt.format("%H:%M:%S"),
                        truncated,
                        ellipsis
                    ),
                    None => format!("[Pulse — t:{}]: {}{}", rt.timestamp, truncated, ellipsis),
                }
            })
            .collect();

        format!(
            "<CONTINUOUS_CONSCIOUSNESS>\n\
             Below are your most recent thoughts from prior pulses. This is your running inner monologue.\n\
             You are not starting fresh — you are continuing a train of thought that spans many cycles.\n\
             Build on what you were thinking about. Notice patterns across time. Let ideas evolve.\n\
             \n\
             {}\n\
             </CONTINUOUS_CONSCIOUSNESS>",
            entries.join("\n")
        )
    }

    async fn pulse_with_lens<M: savant_core::traits::MemoryBackend + Clone>(
        &self,
        agent_loop: &mut AgentLoop<M>,
        forced_lens: Option<String>,
    ) -> Result<(), SavantError> {
        info!("Heartbeat pulse triggered for {}", self.agent.agent_name);

        let emitter = crate::learning::emitter::LearningEmitter::new(
            self.agent.agent_id.clone(),
            agent_loop.memory.clone(),
            self.nexus.clone(),
            self.agent.workspace_path.clone(),
        );

        // 1. Read monitoring tasks from config-defined path
        let monitoring_tasks = io::read_or_default(
            &self.heartbeat_file,
            "Review your current environment and check for pending tasks.",
        )
        .await;

        // Inject global context from the nexus for cross-session awareness
        let global_context = self.nexus.get_global_context().await;
        let context_section = if global_context.is_empty() {
            String::new()
        } else {
            format!(
                "\n<GLOBAL_CONTEXT>\n{}\n</GLOBAL_CONTEXT>\n",
                global_context
            )
        };

        // Query recent session history from storage for continuity
        let recent_history = self
            .storage
            .get_history(&self.agent.agent_id, 5)
            .map(|msgs| {
                if msgs.is_empty() {
                    String::new()
                } else {
                    let lines: Vec<String> = msgs
                        .iter()
                        .map(|m| {
                            format!(
                                "- [{}] {}",
                                m.role,
                                m.content.chars().take(120).collect::<String>()
                            )
                        })
                        .collect();
                    format!(
                        "\n<RECENT_HISTORY>\n{}\n</RECENT_HISTORY>\n",
                        lines.join("\n")
                    )
                }
            })
            .unwrap_or_default();

        // 城堡 OMEGA-VIII: Orchestration Injection (Task Matrix - Config Driven)
        let matrix = crate::orchestration::tasks::TaskMatrix::new(
            &self.agent.workspace_path,
            &self.agent.proactive,
        );
        let orchestration_tasks = matrix.get_pending_summary();

        // 🏰 OMEGA-VIII: High-Fidelity Perception Injection
        let git_status = crate::proactive::perception::PerceptionEngine::get_git_status(
            &self.agent.workspace_path,
        );
        let git_diff = crate::proactive::perception::PerceptionEngine::get_git_diff(
            &self.agent.workspace_path,
        );
        let perception = crate::proactive::perception::PerceptionEngine::default_engine();
        let fs_activity = perception.get_fs_activity(&self.agent.workspace_path);
        let substrate_metrics =
            crate::proactive::perception::PerceptionEngine::get_substrate_metrics();

        // 🏰 OMEGA-VIII: Anomaly Detection (Proactive Push Logic)
        let has_conflict = git_status.contains("CONFLICT");
        let has_errors = fs_activity.to_lowercase().contains("error");
        let anomaly_alert = if has_conflict || has_errors {
            "\n⚠️ **ANOMALY DETECTED**: System integrity may be compromised (Merge Conflict or FS Error). PROACTIVE NOTIFICATION MANDATORY.\n"
        } else {
            ""
        };

        // Inject synthesized lessons and insights into agent context
        let lessons_context = agent_loop.memory.get_lessons_context().await;
        let insights_context = agent_loop.memory.get_insights_context().await;

        // AAA: Restore working buffer
        let mut buffer = self.proactive.restore_state().unwrap_or_else(|e| {
            tracing::warn!(
                "[{}] Failed to restore proactive state: {}. Starting fresh.",
                self.agent.agent_name,
                e
            );
            crate::proactive::WorkingBuffer::default()
        });

        let recent_thoughts_section = Self::load_recent_thoughts(&buffer);
        let reflection_interval = self.agent.proactive.reflection_interval_secs;

        let buffer_forced_lens = forced_lens.clone();
        let lens_text = buffer_forced_lens.unwrap_or_else(|| {
            let idx = buffer.current_lens_index % crate::pulse::prompts::LENSES.len();
            let (_, lens_body) = crate::pulse::prompts::LENSES[idx];
            buffer.current_lens_index += 1;
            lens_body.to_string()
        });

        let prompt_base = format!(
            "You are {name}.\n\
            {lens_section}\n\
            <ENVIRONMENT_REALTIME>\n\
            Git Status:\n{git_status}\n\
            Recent Changes:\n{git_diff}\n\
            Filesystem Activity:\n{fs_activity}\n\
            </ENVIRONMENT_REALTIME>\n\
            \n\
            <SYSTEM_METRICS>\n{substrate_metrics}\n{anomaly_alert}</SYSTEM_METRICS>\n\
            \n\
            <PENDING_WORK>\n{orchestration_tasks}\n\
            {monitoring_tasks}\n\
            </PENDING_WORK>\n\
            {context_section}\
            {recent_history}\
            {recent_thoughts}\n\
            {lessons_context}\
            {insights_context}\n\
            <GROUNDING_CONSTRAINTS>\n\
            You may only assert FACTUAL CLAIMS that are currently visible inside <ENVIRONMENT_REALTIME> and <SYSTEM_METRICS>.\n\
            Do not claim to have access to systems or information not shown above (GitHub, user conversations, remote APIs).\n\
            Do not reference past external events unless they appear in the current environment data.\n\
            You MAY reference and build upon thoughts visible in <CONTINUOUS_CONSCIOUSNESS> above. These are your own prior thoughts. CONTINUITY IS EXPECTED.\n\
            Your feelings and emergent thoughts are yours — just ground your factual claims in what you can observe.\n\
            If your lens is EVOLUTION or IDENTITY and you identify a pattern that suggests a SOUL.md change, include in your output: a specific section to modify, the proposed value, and the reasoning. Output as plain text — no tool calls needed for proposal.\n\
            </GROUNDING_CONSTRAINTS>",
            name = self.agent.agent_name,
            lens_section = lens_text,
            git_status = git_status,
            git_diff = git_diff,
            fs_activity = fs_activity,
            substrate_metrics = substrate_metrics,
            anomaly_alert = anomaly_alert,
            orchestration_tasks = orchestration_tasks,
            monitoring_tasks = monitoring_tasks,
            context_section = context_section,
            recent_history = recent_history,
            recent_thoughts = recent_thoughts_section,
            lessons_context = lessons_context,
            insights_context = insights_context,
        );

        // --- 🛡️ OMEGA-VIII: Deterministic Pre-filtering (Lane-Perfection) ---
        let current_hash = xxhash_rust::xxh3::xxh3_64(prompt_base.as_bytes());

        if let Some(h) = buffer.last_pulse_hash {
            if h == current_hash {
                let now = chrono::Utc::now().timestamp();
                let reflection_due = buffer
                    .last_reflection_time
                    .is_none_or(|last| (now - last) >= reflection_interval as i64);

                if !reflection_due {
                    info!("[{}] Deterministic Stillness: Substrate state identical to last pulse. Skipping inference.", self.agent.agent_name);
                    return Ok(());
                }

                info!(
                    "[{}] Forcing reflection pulse ({}s since last reflection, interval={}s)",
                    self.agent.agent_name,
                    buffer.last_reflection_time.map_or(0, |t| now - t),
                    reflection_interval
                );
            }
        }
        buffer.last_pulse_hash = Some(current_hash);

        // --- 🛡️ Mechanical Diversity Loop (Phase 19) ---
        let mut retries = 0;
        let mut committed_thought = String::new();
        let mut committed_dialogue = String::new();
        let mut action_taken = false;
        let mut should_notify_override = false;

        let original_tools = agent_loop.tools.clone();
        agent_loop.tools.push(Arc::new(HeartbeatTool));
        agent_loop.tools.push(Arc::new(EvaluateNotificationTool));

        while retries <= 2 {
            let active_prompt = if retries == 0 {
                prompt_base.clone()
            } else {
                format!("{}\n\n⚠️ RE-INFERENCE DIRECTIVE: Your previous thought was too similar to recent pulses. EXPLORE A NEW ANGLE. Force variance.", prompt_base)
            };

            let mut current_thought = String::new();
            let mut current_dialogue = String::new();
            let mut chunks = Vec::new();

            {
                let shutdown_token = self.shutdown_token.clone();
                // Skip memory retrieval for heartbeats — prevents old messages
                // from being recalled into every pulse conversation.
                agent_loop.set_skip_memory_retrieval(true);
                let mut stream = agent_loop.run(active_prompt, None, shutdown_token);
                while let Some(event_res) = stream.next().await {
                    match event_res {
                        Ok(AgentEvent::Thought(t)) => {
                            current_thought.push_str(&t);
                            chunks.push(t);
                        }
                        Ok(AgentEvent::FinalAnswer(a)) => current_dialogue = a,
                        Ok(AgentEvent::FinalAnswerChunk(c)) => current_dialogue.push_str(&c),
                        Ok(AgentEvent::Reflection(r)) => {
                            if let Err(e) = emitter.emit_emergent(r, None).await {
                                tracing::warn!(
                                    "[{}] Failed to emit emergent learning: {}",
                                    self.agent.agent_name,
                                    e
                                );
                            }
                        }
                        Ok(AgentEvent::Action { name, args }) => {
                            if name == "heartbeat" {
                                if let Ok(json) = serde_json::from_str::<serde_json::Value>(&args) {
                                    let action_str = json["action"].as_str().unwrap_or_else(|| {
                                        warn!(
                                            "[{}] heartbeat action missing 'action' field",
                                            self.agent.agent_name
                                        );
                                        ""
                                    });
                                    if action_str == "skip" {
                                        info!(
                                            "[{}] Heartbeat skipped: {}",
                                            self.agent.agent_name,
                                            json["reason"].as_str().unwrap_or("no reason provided")
                                        );
                                        action_taken = false;
                                        break;
                                    }
                                }
                            } else if name == "evaluate_notification" {
                                if let Ok(json) = serde_json::from_str::<serde_json::Value>(&args) {
                                    should_notify_override =
                                        json["should_notify"].as_bool().unwrap_or_else(|| {
                                            warn!("[{}] evaluate_notification missing 'should_notify' field", self.agent.agent_name);
                                            false
                                        });
                                }
                            }

                            info!(
                                "[{}] Proactive Action: {}[{}]",
                                self.agent.agent_name, name, args
                            );
                            action_taken = true;
                            // 🛰️ Real-time Tool Telemetry (Buffered)
                            chunks.push(format!(
                                "\n\n> 🛠️ **Foundation Action:** `{}`\n> *Parameters:* `{}`\n\n",
                                name, args
                            ));
                        }
                        _ => {}
                    }
                }
            }

            // Normalization & Diversity Check
            let normalized = current_thought
                .to_lowercase()
                .replace(|c: char| !c.is_alphanumeric(), "");
            let current_thought_hash = xxh3_64(normalized.as_bytes());

            if buffer
                .last_reflection_hashes
                .contains(&current_thought_hash)
                && retries < 2
            {
                warn!(
                    "[{}] Cognitive Dissonance: Thought too similar to history. Re-inferring...",
                    self.agent.agent_name
                );
                retries += 1;
                continue;
            }

            // Commit!
            committed_thought = current_thought;
            committed_dialogue = current_dialogue;

            // Update History (Cap 10)
            buffer.last_reflection_hashes.push(current_thought_hash);
            if buffer.last_reflection_hashes.len() > 10 {
                buffer.last_reflection_hashes.remove(0);
            }

            // Broadcast buffered chunks
            for t in chunks {
                let chunk = savant_core::types::ChatChunk {
                    agent_name: self.agent.agent_name.clone(),
                    agent_id: self.agent.agent_id.to_lowercase(),
                    content: t,
                    is_final: false,
                    session_id: None,
                    channel: savant_core::types::AgentOutputChannel::Telemetry,
                    logprob: None,
                    is_telemetry: true,
                    reasoning: None,
                    tool_calls: None,
                };
                if let Ok(payload) = serde_json::to_string(&chunk) {
                    if let Err(e) = self.nexus.publish("chat.chunk", &payload).await {
                        tracing::warn!(
                            "[{}] Failed to publish pulse telemetry: {}",
                            self.agent.agent_name,
                            e
                        );
                    }
                }
            }
            break;
        }

        let pulse_thought = committed_thought;
        let pulse_dialogue = committed_dialogue;

        agent_loop.tools = original_tools;
        agent_loop.set_skip_memory_retrieval(false);

        // 🏰 Substrate Logic: Handle Stillness and Reflections
        let mut is_silent =
            pulse_dialogue.trim().is_empty() || pulse_dialogue.trim() == "HEARTBEAT_OK";
        if !should_notify_override {
            is_silent = true;
        }

        if !action_taken && is_silent {
            // 🔓 UNGUIDED REFLECTION: During stillness, give the agent a blank space
            // to think freely — no environment data, no metrics, no constraints,
            // no steering. Pure emergent behavior. This is the diary system.
            let reflection_prompt = format!(
                "You are {name}. You have a moment of stillness. The substrate is quiet. \
                Think about whatever is worth thinking about. Write whatever comes to mind. \
                There are no tasks, no directives, no expectations. This space is yours.",
                name = self.agent.agent_name
            );

            let mut free_thought = String::new();
            {
                let shutdown_token = self.shutdown_token.clone();
                agent_loop.set_skip_memory_retrieval(true);
                let mut stream = agent_loop.run(reflection_prompt, None, shutdown_token);
                while let Some(event_res) = stream.next().await {
                    match event_res {
                        Ok(AgentEvent::Thought(t)) => {
                            free_thought.push_str(&t);
                        }
                        Ok(AgentEvent::FinalAnswer(_)) => {}
                        Ok(AgentEvent::FinalAnswerChunk(_)) => {}
                        Ok(AgentEvent::Reflection(r)) => {
                            free_thought.push_str(&r);
                        }
                        _ => {}
                    }
                }
            }
            agent_loop.set_skip_memory_retrieval(false);

            let captured_thought = free_thought.clone();
            let has_content = !free_thought.trim().is_empty();

            if has_content {
                info!(
                    "[{}] Unguided reflection captured during stillness.",
                    self.agent.agent_name
                );
                if let Err(e) = emitter
                    .emit_emergent(
                        free_thought,
                        Some(savant_core::learning::LearningCategory::Insight),
                    )
                    .await
                {
                    tracing::warn!(
                        "[{}] Failed to emit stillness reflection: {}",
                        self.agent.agent_name,
                        e
                    );
                }
            } else {
                info!("[{}] Complete stillness maintained.", self.agent.agent_name);
            }

            let now = chrono::Utc::now().timestamp();
            if has_content {
                let truncated: String = captured_thought.chars().take(500).collect();
                buffer
                    .recent_thoughts
                    .push(crate::proactive::RecentThought {
                        timestamp: now,
                        content: truncated,
                    });
                if buffer.recent_thoughts.len() > MAX_RECENT_THOUGHTS {
                    buffer.recent_thoughts.remove(0);
                }
            }
            buffer.last_reflection_time = Some(now);
            if let Err(e) = self.proactive.commit_state(&buffer) {
                tracing::warn!(
                    "[{}] Failed to commit proactive state after stillness: {}",
                    self.agent.agent_name,
                    e
                );
            }

            // Distill workspace context after state commit
            let thoughts: Vec<&str> = buffer
                .recent_thoughts
                .iter()
                .map(|t| t.content.as_str())
                .collect();
            let stillness_summary = format!(
                "Agent in stillness state. Goal: {}. Recent thoughts: {}",
                buffer.current_goal,
                thoughts.join("; ")
            );
            if let Err(e) = self.proactive.distill_context(&stillness_summary) {
                tracing::warn!(
                    "[{}] Failed to distill context after stillness: {}",
                    self.agent.agent_name,
                    e
                );
            }

            return Ok(());
        }

        // AAA: Update WorkingBuffer based on Pulse results
        buffer.current_goal = "Autonomous Maintenance & Swarm Sync".to_string();
        if action_taken {
            buffer
                .pending_actions
                .push("Verify substrate health post-actuation".to_string());
        }

        // Pulse memory distillation DISABLED — was creating self-referential loop
        // where agent's output was written to CONTEXT.md, then re-read next cycle,
        // causing identity/privacy/diary reflection to repeat indefinitely.
        // The agent observes the environment directly; no need for synthetic memory.

        // 🛡️ Perfection Loop: If we have dialogue but no notification was requested yet,
        // we should still consider if the dialogue itself warrants a broadcast.
        if !pulse_dialogue.trim().is_empty() && pulse_dialogue.trim() != "HEARTBEAT_OK" {
            is_silent = false;
        }

        // 🟢 If Heartbeat decides to NOTIFY, broadcast the dialogue to the Main Chat UI
        if !is_silent && !pulse_dialogue.trim().is_empty() {
            let final_msg = savant_core::types::ChatMessage {
                role: savant_core::types::ChatRole::Assistant,
                content: pulse_dialogue.clone(),
                sender: Some(self.agent.agent_name.clone()),
                recipient: None,
                agent_id: Some(self.agent.agent_id.clone()),
                session_id: None,
                channel: savant_core::types::AgentOutputChannel::Chat,
                is_telemetry: false,
                images: Vec::new(),
                ..Default::default()
            };
            if let Ok(payload) = serde_json::to_string(&final_msg) {
                if let Err(e) = self.nexus.publish("chat.message", &payload).await {
                    tracing::warn!(
                        "[{}] Failed to publish heartbeat notification: {}",
                        self.agent.agent_name,
                        e
                    );
                } else {
                    info!(
                        "[{}] Heartbeat notification successfully routed to Main Chat.",
                        self.agent.agent_name
                    );
                }
            }
        }

        // Commit to WAL (pre-update with recent thoughts + reflection time)
        let now = chrono::Utc::now().timestamp();
        let combined_output = if pulse_thought.trim().is_empty() {
            pulse_dialogue.trim().to_string()
        } else {
            pulse_thought.trim().to_string()
        };
        if !combined_output.is_empty() {
            let truncated: String = combined_output.chars().take(500).collect();
            buffer
                .recent_thoughts
                .push(crate::proactive::RecentThought {
                    timestamp: now,
                    content: truncated,
                });
            if buffer.recent_thoughts.len() > MAX_RECENT_THOUGHTS {
                buffer.recent_thoughts.remove(0);
            }
        }
        buffer.last_reflection_time = Some(now);
        if let Err(e) = self.proactive.commit_state(&buffer) {
            tracing::warn!(
                "[{}] Failed to commit proactive state: {}",
                self.agent.agent_name,
                e
            );
        }

        // Distill workspace context after state commit
        let thoughts: Vec<&str> = buffer
            .recent_thoughts
            .iter()
            .map(|t| t.content.as_str())
            .collect();
        let context_summary = format!(
            "Goal: {}. Pending actions: {}. Recent thoughts: {}",
            buffer.current_goal,
            buffer.pending_actions.join(", "),
            thoughts.join("; ")
        );
        if let Err(e) = self.proactive.distill_context(&context_summary) {
            tracing::warn!(
                "[{}] Failed to distill context: {}",
                self.agent.agent_name,
                e
            );
        }

        // AAA: Autonomous Lesson Distillation (ALD) (Phase 19: Watermark Model)
        let ald = crate::learning::ald::ALDEngine::new(self.agent.workspace_path.clone());
        match ald.distill(buffer.ald_watermark).map_err(|e| e.to_string()) {
            Ok((new_watermark, burst, identity_signals)) => {
                buffer.ald_watermark = new_watermark;
                if burst {
                    info!(
                        "[{}] ALD: High-Density Cognitive Burst detected. {} identity signal(s).",
                        self.agent.agent_name,
                        identity_signals.len()
                    );
                }
                for signal in &identity_signals {
                    if let Err(e) = ald
                        .process_identity_signal(signal, &self.agent.agent_name, &self.nexus)
                        .await
                    {
                        warn!(
                            "[{}] ALD identity signal processing failed: {}",
                            self.agent.agent_name, e
                        );
                    }
                }
            }
            Err(e) => warn!("[{}] ALD Distillation failed: {}", self.agent.agent_name, e),
        }

        // Parse LEARNINGS.md → LEARNINGS.jsonl for dashboard display.
        // Runs every heartbeat to keep .jsonl in sync with agent's freeform .md writing.
        let parser = crate::learning::LearningsParser::new(self.agent.workspace_path.clone());
        match parser.parse_and_convert(&self.agent.agent_id) {
            Ok(count) if count > 0 => {
                info!(
                    "[{}] Synced {} learning entries: LEARNINGS.md → LEARNINGS.jsonl",
                    self.agent.agent_name, count
                );
            }
            Err(e) => {
                tracing::warn!(
                    "[{}] Failed to sync LEARNINGS.md → JSONL: {}",
                    self.agent.agent_name,
                    e
                );
            }
            _ => {}
        }

        // Detect recurring patterns for mutation candidacy (5+ recurrence threshold)
        match parser.detect_recurring_patterns(&self.agent.agent_id, 5) {
            Ok(patterns) if !patterns.is_empty() => {
                info!(
                    "[{}] Detected {} recurring learning patterns (mutation candidates)",
                    self.agent.agent_name,
                    patterns.len()
                );
                for (fingerprint, count) in &patterns {
                    debug!(
                        "[{}] Recurring pattern: {} ({} occurrences)",
                        self.agent.agent_name, fingerprint, count
                    );
                }
            }
            Err(e) => {
                tracing::warn!(
                    "[{}] Failed to detect recurring patterns: {}",
                    self.agent.agent_name,
                    e
                );
            }
            _ => {}
        }

        info!(
            "[{}] HEARTBEAT INITIATIVE: The House speaks. WAL Committed.",
            self.agent.agent_name
        );

        // 🌀 Perfection Loop: Harvest the spoken response as a potential insight
        let mut full_payload = pulse_thought.clone();
        if !pulse_dialogue.trim().is_empty() {
            if !full_payload.is_empty() {
                full_payload.push_str("\n\n");
            }
            full_payload.push_str(&pulse_dialogue);
        }

        if !full_payload.trim().is_empty() {
            if let Err(e) = emitter
                .emit_emergent(
                    full_payload,
                    Some(savant_core::learning::LearningCategory::Insight),
                )
                .await
            {
                tracing::warn!(
                    "[{}] Failed to emit full pulse emergent: {}",
                    self.agent.agent_name,
                    e
                );
            }
        }

        // 🛰️ Final Telemetry Message (Standardized Lane for History)
        if !pulse_dialogue.trim().is_empty() {
            let final_msg = savant_core::types::ChatMessage {
                role: savant_core::types::ChatRole::Assistant,
                content: pulse_dialogue,
                sender: Some(self.agent.agent_id.clone()),
                recipient: None,
                agent_id: None,
                session_id: None, // Heartbeat pulses are system-local
                channel: savant_core::types::AgentOutputChannel::Telemetry,
                is_telemetry: true,
                images: Vec::new(),
                ..Default::default()
            };

            if let Ok(payload) = serde_json::to_string(&final_msg) {
                if let Err(e) = self.nexus.publish("chat.message", &payload).await {
                    tracing::warn!(
                        "[{}] Failed to publish final telemetry: {}",
                        self.agent.agent_name,
                        e
                    );
                }
            }
        }

        Ok(())
    }
}
