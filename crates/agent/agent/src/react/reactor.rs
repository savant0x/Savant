use crate::react::AgentLoop;
use savant_core::error::SavantError;
use savant_core::traits::MemoryBackend;
use savant_core::types::ChatMessage;
use tracing::{info, warn};

/// Extracts data references from tool arguments for taint tracking.
/// Looks for patterns like "web:", "file:", "memory:" prefixes.
fn extract_data_refs(payload: &str) -> Vec<String> {
    let mut refs = Vec::new();
    // Simple pattern matching for data references
    for prefix in &["web:", "file:", "memory:", "external:"] {
        if let Some(start) = payload.find(prefix) {
            // Extract the reference up to the next whitespace or quote
            let end = payload[start..]
                .find(|c: char| c.is_whitespace() || c == '"' || c == '}' || c == ']')
                .map(|i| start + i)
                .unwrap_or(payload.len());
            refs.push(payload[start..end].to_string());
        }
    }
    refs
}

/// Outcome of heuristic resolution attempt.
/// Signals to the agent loop whether to continue with a hint, rollback state, or abort.
pub(crate) enum HeuristicOutcome {
    /// Recovery hint — agent continues with additional context
    Hint(String),
    /// Rollback — restore message history to last stable checkpoint
    Rollback {
        messages: Vec<ChatMessage>,
        hint: String,
    },
    /// Fatal — unrecoverable, abort the turn
    Fatal(SavantError),
}

impl<M: MemoryBackend> AgentLoop<M> {
    /// Verifies if the current agent has the required security token to execute a specific tool.
    pub(crate) fn verify_tool_access(&self, tool_name: &str) -> Result<(), SavantError> {
        info!(
            "Security Enclave: Verifying access for tool [{}] for agent [{}]",
            tool_name, self.agent_id
        );

        // 🛡️ AAA Logic: If security is enabled (token present), enforce it.
        if let Some(token) = &self.security_token {
            if !token.assignee_matches(self.agent_id_hash) {
                warn!(
                    "CCT VIOLATION: Token assignee mismatch for agent [{}]",
                    self.agent_id
                );
                return Err(SavantError::AuthError(
                    "Security token binding failed: ID mismatch".into(),
                ));
            }

            let resource = format!("savant://tools/{}", tool_name);
            if !token.verify_capability(&resource, "execute") {
                warn!(
                    "CCT VIOLATION: Permitted action 'execute' denied for resource [{}]",
                    resource
                );
                return Err(SavantError::AuthError(format!(
                    "Capability denied for tool: {}",
                    tool_name
                )));
            }
            info!("Security Enclave: Access GRANTED for tool [{}]", tool_name);
        } else {
            warn!(
                "SECURITY WARNING: Agent [{}] executing tool [{}] without a CCT.",
                self.agent_id, tool_name
            );
        }
        Ok(())
    }

    pub(crate) async fn execute_tool(&self, name: &str, args: &str) -> Result<String, SavantError> {
        // Check cancellation before executing
        if let Some(token) = &self.cancellation_token {
            if token.is_cancelled() {
                return Err(SavantError::Unknown("Tool execution cancelled".to_string()));
            }
        }

        // CRITICAL: Full cryptographic verification via SecurityAuthority when available
        if let (Some(token), Some(authority)) = (&self.security_token, &self.security_authority) {
            // Check token expiry before verification
            if token.is_expired() {
                warn!(
                    "[{}] Security token expired for tool [{}]. Requesting refresh.",
                    self.agent_id, name
                );
                // Attempt to get a fresh ephemeral token from the broker
                let _resource = format!("savant://tools/{}", name);
                if let Ok(_ephemeral) = self
                    .credential_broker
                    .get_credential(
                        "openrouter",
                        &self.agent_id,
                        std::time::Duration::from_secs(3600),
                    )
                    .await
                {
                    info!(
                        "[{}] CredentialBroker: issued fresh token for tool [{}]",
                        self.agent_id, name
                    );
                }
                return Err(SavantError::AuthError("CCT token expired".to_string()));
            }
            let resource = format!("savant://tools/{}", name);
            authority
                .verify_token_and_action(token, self.agent_id_hash, &resource, "execute")
                .map_err(|e| {
                    SavantError::AuthError(format!("CCT Crypto Verification Failed: {}", e))
                })?;
            info!(
                "Security Enclave: Full crypto verification GRANTED for tool [{}]",
                name
            );
        } else {
            // Fallback: lightweight assignee + capability check
            self.verify_tool_access(name)?;
        }

        for tool in &self.tools {
            if tool.name().to_lowercase() == name.to_lowercase() {
                let mut payload = serde_json::from_str(args).map_err(|e| {
                    warn!(
                        "[{}] Failed to parse tool args for '{}': {}. Args: {}",
                        self.agent_id, name, e, args
                    );
                    SavantError::Unknown(format!(
                        "Invalid JSON arguments for tool '{}': {}",
                        name, e
                    ))
                })?;

                // Validate and coerce arguments against tool's JSON Schema
                let schema = tool.parameters_schema();
                if schema.get("type").is_some() {
                    // Schema validation (lenient — log warnings, don't block)
                    if let Err(e) = crate::tools::schema_validator::validate_tool_schema(&schema) {
                        warn!(
                            "[{}] Tool schema validation warning for '{}': {:?}",
                            self.agent_id, name, e
                        );
                    }
                    // Coerce arguments
                    payload = crate::tools::coercion::prepare_tool_params(&payload, &schema);
                }

                // Approval gate (reactor path): block Always-requiring tools
                // Full session-aware check is in stream.rs DAG path
                use savant_core::traits::ApprovalRequirement;
                if tool.requires_approval() == ApprovalRequirement::Always {
                    return Err(SavantError::Unknown(
                        format!("Tool '{}' requires user approval (reactor path). Use DAG execution path for approval support.", tool.name())
                    ));
                }

                // Taint tracking: check if tool arguments contain references to untrusted data
                let payload_str = payload.to_string();
                let data_refs: Vec<String> = extract_data_refs(&payload_str);
                for data_id in &data_refs {
                    if let Some(tag) = self.taint_tracker.get_tag(data_id) {
                        if tag.requires_human_verification() {
                            warn!(
                                "[{}] Tool '{}' accessing untrusted data '{}': trust={:.2}, requires_human_verification=true",
                                self.agent_id, name, data_id, tag.trust_level
                            );
                        }
                        if self.taint_tracker.requires_verification(data_id) {
                            warn!(
                                "[{}] Tool '{}' accessing data requiring verification '{}': trust={:.2}",
                                self.agent_id, name, data_id, tag.trust_level
                            );
                        }
                        // is_trusted takes a threshold parameter (0.5 = moderate trust)
                        if !self.taint_tracker.is_trusted(data_id, 0.5) {
                            warn!(
                                "[{}] Tool '{}' accessing untrusted data '{}': trust={:.2}",
                                self.agent_id, name, data_id, tag.trust_level
                            );
                        }
                    }
                }

                // Compound taint: if multiple data sources are referenced, mark compound
                // Note: add_transformation is on TaintTag (cloned), used for audit trail
                if data_refs.len() > 1 {
                    for data_id in &data_refs {
                        if let Some(mut tag) = self.taint_tracker.get_tag(data_id) {
                            tag.add_transformation(&format!("compound_with_{}", data_refs.len()));
                            // Re-tag with the transformed version
                            self.taint_tracker.tag(data_id, tag);
                        }
                    }
                }

                // Taint monitoring: log count of tracked data items
                let taint_count = self.taint_tracker.count();
                if taint_count > 100 {
                    warn!(
                        "[{}] Taint tracker has {} tracked items — clearing oldest",
                        self.agent_id, taint_count
                    );
                    // Clear all taint entries when tracker gets too large
                    // clear() takes a data_id — clear each ref individually
                    for data_id in &data_refs {
                        self.taint_tracker.clear(data_id);
                    }
                }

                // Execute with timeout
                let timeout_secs = tool.timeout_secs();
                let max_output = tool.max_output_chars();
                let tool_clone = tool.clone();
                let result = tokio::time::timeout(
                    std::time::Duration::from_secs(timeout_secs),
                    self.hyper_causal.execute_speculative(tool_clone, payload),
                )
                .await;

                return match result {
                    Ok(inner_result) => {
                        inner_result.map(|output| {
                            // Taint tagging: tag tool results based on tool type
                            let data_id = format!(
                                "tool:{}:{}",
                                name,
                                self.tool_error_count
                                    .load(std::sync::atomic::Ordering::Relaxed)
                            );
                            let taint = match name {
                                "file_read" | "file_write" | "file_create" | "file_move"
                                | "file_delete" | "file_atomic_edit" => {
                                    savant_security::continuous::taint::TaintTag::user_file()
                                }
                                "shell" | "process" | "exec" => {
                                    savant_security::continuous::taint::TaintTag::system()
                                }
                                "memory_recall" | "memory_consolidate" => {
                                    savant_security::continuous::taint::TaintTag::nrem_replay()
                                }
                                // C6: Web/HTTP tools produce external data with lower trust
                                "web_search" | "web_fetch" | "http" | "browser"
                                | "web_sovereign" => {
                                    savant_security::continuous::taint::TaintTag::external_web()
                                }
                                _ => savant_security::continuous::taint::TaintTag::system(),
                            };
                            self.taint_tracker.tag(&data_id, taint);

                            // PB-11: Truncate native tool output using tool's configured limit
                            let truncated = truncate_output(&output, max_output);
                            // Use Compact engine for L1 tool output compression
                            crate::compact::integration::compact_output_sync(
                                name, args, 0, &truncated, None,
                            )
                            .output
                        })
                    }
                    Err(_) => Err(SavantError::Unknown(format!(
                        "Tool '{}' timed out after {} seconds",
                        name, timeout_secs
                    ))),
                };
            }
        }

        if let (Some(registry), Some(host)) = (&self.echo_registry, &self.echo_host) {
            if let Some(capability) = registry.get_tool(name) {
                match host.execute_tool(&capability.module, args).await {
                    Ok(res) => {
                        if let Some(metrics) = &self.echo_metrics {
                            metrics.record_outcome(true);
                        }
                        // Truncate large outputs to prevent context overflow
                        let truncated = truncate_output(&res, MAX_TOOL_OUTPUT_CHARS);
                        return Ok(truncated);
                    }
                    Err(e) => {
                        if let Some(metrics) = &self.echo_metrics {
                            metrics.record_outcome(false);
                        }
                        return Err(SavantError::Unknown(e.to_string()));
                    }
                }
            }
        }
        Err(SavantError::Unknown(format!("Tool not found: {}", name)))
    }

    /// OMEGA: Heuristic Resolution Matrix.
    /// Handles tool failures by attempting specific resolution paths.
    pub(crate) async fn handle_heuristic_resolution(
        &mut self,
        tool_name: &str,
        error: SavantError,
    ) -> HeuristicOutcome {
        info!(
            "[{}] HEURISTIC: Triggering resolution path for tool [{}] failure: {:?}",
            self.agent_id, tool_name, error
        );
        self.heuristic.failures += 1;

        match self.heuristic.failures {
            1 => {
                // Path 1: Contextual Expansion
                info!(
                    "[{}] HEURISTIC: Path 1 - Contextual Expansion triggered.",
                    self.agent_id
                );
                HeuristicOutcome::Hint("Recovery hint: Try to re-read the documentation for the tool and verify arguments.".to_string())
            }
            2 => {
                // Path 2: Technical Refinement (Rollback)
                if let Some(checkpoint) = self.heuristic.last_stable_checkpoint.take() {
                    info!("[{}] HEURISTIC: Path 2 - Triggering state rollback to last stable checkpoint ({} messages).", self.agent_id, checkpoint.len());
                    HeuristicOutcome::Rollback {
                        messages: checkpoint,
                        hint: "Recovery hint: System state inconsistent. Rolling back to last stable checkpoint. Please simplify the request.".to_string(),
                    }
                } else {
                    info!("[{}] HEURISTIC: Path 2 - No checkpoint available, attempting alternate strategy.", self.agent_id);
                    HeuristicOutcome::Hint(
                        "Recovery hint: Attempting alternate tool strategy.".to_string(),
                    )
                }
            }
            _ => {
                // Path 3: Architectural Pivot
                warn!(
                    "[{}] HEURISTIC: Maximum retries reached. Failing session.",
                    self.agent_id
                );
                HeuristicOutcome::Fatal(SavantError::HeuristicFailure(format!(
                    "Recursive failure loop detected for tool: {}",
                    tool_name
                )))
            }
        }
    }
}

/// Maximum tool output size in characters before truncation.
const MAX_TOOL_OUTPUT_CHARS: usize = 50_000;

/// Truncate tool output with head+tail preservation.
/// Uses char count (not byte count) for accurate multi-byte content handling.
fn truncate_output(output: &str, max_chars: usize) -> String {
    let char_count = output.chars().count();
    if char_count <= max_chars {
        return output.to_string();
    }
    let head_size = (max_chars * 60) / 100;
    let tail_size = (max_chars * 40) / 100;

    let head: String = output.chars().take(head_size).collect();
    let tail: String = output.chars().skip(char_count - tail_size).collect();

    format!("{}\n\n[... truncated ...]\n\n{}", head, tail)
}
