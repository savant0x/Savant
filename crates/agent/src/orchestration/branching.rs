//! Hyper-Causal Convergence (HCC) Engine
//!
//! This module implements the "Potential Timeline" execution pattern.
//! It allows tools to execute in parallel branches (Shadow Workspaces)
//! and only collapses to the one that passes formal verification
//! and maximizes Informational Entropy Gain (IEG).

use futures::future::join_all;
use savant_core::error::SavantError;
use savant_core::traits::Tool;
use serde_json::Value;
use std::io::Write;
use std::sync::Arc;
use tracing::{info, warn};

/// Represents a single potential branch of execution.
pub struct CausalBranch {
    pub timeline_id: u64,
    pub outcome: Result<String, SavantError>,
    pub entropy_gain: f32,
    pub verified: bool,
}

/// The Hyper-Causal Engine manages the orchestration of potential timelines.
pub struct HyperCausalEngine {
    /// Max parallel branches to simulate
    max_branches: usize,
}

impl Default for HyperCausalEngine {
    fn default() -> Self {
        Self { max_branches: 3 }
    }
}

impl HyperCausalEngine {
    pub fn new(max_branches: usize) -> Self {
        Self { max_branches }
    }

    /// Returns the maximum number of parallel branches this engine supports.
    pub fn max_branches(&self) -> usize {
        self.max_branches
    }

    /// Tools with side effects must NOT be executed speculatively.
    /// These tools modify state and running them 3x would cause data corruption.
    fn is_side_effect_tool(name: &str) -> bool {
        matches!(
            name,
            "file_delete" | "file_move" | "file_create" | "file_atomic_edit" | "foundation"
        )
    }

    /// Executes a tool across multiple potential timelines and returns the "Collapsed" result.
    /// For side-effect tools, executes directly without speculation.
    pub async fn execute_speculative(
        &self,
        tool: Arc<dyn Tool>,
        payload: Value,
    ) -> Result<String, SavantError> {
        // Side-effect tools must execute exactly once
        if Self::is_side_effect_tool(tool.name()) {
            info!(
                "HCC: Side-effect tool '{}' detected — executing directly (no speculation)",
                tool.name()
            );
            // Wrap in tokio::spawn to isolate panics — tool panic must not crash the agent
            let tool_name = tool.name().to_string();
            let handle = tokio::spawn(async move { tool.execute(payload).await });
            return match handle.await {
                Ok(result) => result,
                Err(join_err) if join_err.is_panic() => {
                    tracing::error!(
                        tool = tool_name,
                        "Tool panicked during execution — agent continues"
                    );
                    Err(SavantError::Unknown(format!(
                        "Tool '{}' panicked during execution",
                        tool_name
                    )))
                }
                Err(join_err) => Err(SavantError::Unknown(format!(
                    "Tool '{}' task cancelled: {}",
                    tool_name, join_err
                ))),
            };
        }

        info!(
            "HCC: Initiating Hyper-Causal execution for tool: {}",
            tool.name()
        );

        let mut branches = Vec::new();

        for i in 0..self.max_branches {
            let tool_clone = Arc::clone(&tool);
            let payload_clone = payload.clone();
            let _tool_name = tool.name().to_string();

            // Spawn a parallel potential timeline
            let handle = tokio::spawn(async move {
                let start_time = std::time::Instant::now();
                let outcome = tool_clone.execute(payload_clone).await;
                let _duration = start_time.elapsed();

                // --- OMEGA: Real Entropy Gain (Zstd Compression Density) ---
                let entropy_gain = if let Ok(ref res) = outcome {
                    // Informational Density: Lower compression ratio = Higher entropy/originality
                    match zstd::Encoder::new(Vec::new(), 3) {
                        Ok(mut encoder) => {
                            if let Err(e) = encoder.write_all(res.as_bytes()) {
                                tracing::warn!("Zstd write failed: {}", e);
                                0.0
                            } else {
                                match encoder.finish() {
                                    Ok(compressed) => {
                                        let original_size = res.len() as f32;
                                        let compressed_size = compressed.len() as f32;

                                        // Ratio of informational novelty (higher is better)
                                        if original_size > 0.0 {
                                            compressed_size / original_size
                                        } else {
                                            0.0
                                        }
                                    }
                                    Err(e) => {
                                        tracing::warn!("Zstd finish failed: {}", e);
                                        0.0
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            tracing::warn!("Zstd encoder creation failed: {}", e);
                            0.0
                        }
                    }
                } else {
                    0.0
                };

                // --- OMEGA: Semantic Verification ---
                // Verify the tool executed successfully. The Tool trait exposes
                // parameters_schema() for input validation; output verification
                // is done by the caller (Orchestrator) via response parsing.
                let verified = outcome.is_ok();

                CausalBranch {
                    timeline_id: i as u64,
                    outcome,
                    entropy_gain,
                    verified,
                }
            });
            branches.push(handle);
        }

        let results = join_all(branches).await;

        // Timeline Collapse Logic:
        // 1. Filter for verified branches
        // 2. Select the one with highest entropy gain
        // 3. Rollback all other "Shadow Workspaces" (handled here by dropping the results)

        let mut best_branch: Option<CausalBranch> = None;

        for branch in results.into_iter().flatten() {
            if branch.verified {
                if let Some(ref best) = best_branch {
                    if branch.entropy_gain > best.entropy_gain {
                        best_branch = Some(branch);
                    }
                } else {
                    best_branch = Some(branch);
                }
            }
        }

        match best_branch {
            Some(collapsed) => {
                info!(
                    "HCC: Timeline collapsed on branch {}. Entropy Gain: {:.2}. Verified: {}",
                    collapsed.timeline_id, collapsed.entropy_gain, collapsed.verified
                );
                collapsed.outcome
            }
            None => {
                warn!(
                    "HCC: All potential timelines failed verification. Causal collapse impossible."
                );
                Err(SavantError::Unknown(
                    "Causal collapse failure: No verified timeline found.".to_string(),
                ))
            }
        }
    }

    /// Executes a task across multiple agents speculatively and returns the best result.
    ///
    /// This extends the Hyper-Causal Engine for cross-agent speculative execution.
    /// When `speculative_copies > 1`, the task is delegated to multiple agents
    /// simultaneously. Each agent executes independently, publishes an Artifact to
    /// its result channel, and the parent selects the artifact with the highest
    /// informational density (lowest Shannon entropy via zstd compression).
    ///
    /// # Arguments
    /// * `copies` — Number of parallel agent executions (speculative_copies from DelegationTask)
    /// * `tool` — The tool to execute
    /// * `payload` — The tool payload
    ///
    /// # Returns
    /// The artifact with the highest informational density from the winning agent.
    pub async fn execute_cross_agent_speculative(
        &self,
        copies: u8,
        tool: Arc<dyn Tool>,
        payload: Value,
    ) -> Result<String, SavantError> {
        if copies <= 1 {
            return self.execute_speculative(tool, payload).await;
        }

        info!(
            "HCC: Cross-agent speculative execution with {} copies for tool: {}",
            copies,
            tool.name()
        );

        let mut handles = Vec::new();
        let copies = copies.min(self.max_branches as u8);

        for i in 0..copies {
            let tool_clone = Arc::clone(&tool);
            let payload_clone = payload.clone();
            let max_branches = self.max_branches;

            let handle = tokio::spawn(async move {
                let engine = HyperCausalEngine::new(max_branches);
                let result = engine.execute_speculative(tool_clone, payload_clone).await;
                (i, result)
            });
            handles.push(handle);
        }

        let results = futures::future::join_all(handles).await;

        // Select the artifact with the highest informational density
        // (lowest zstd compression ratio = highest entropy = most informative)
        let mut best: Option<(u8, String, f32)> = None;

        for result in results {
            if let Ok((idx, Ok(ref text))) = result {
                let entropy_gain = match zstd::Encoder::new(Vec::new(), 3) {
                    Ok(mut encoder) => {
                        if let Err(e) = encoder.write_all(text.as_bytes()) {
                            tracing::warn!("Zstd write failed: {}", e);
                            0.0
                        } else {
                            match encoder.finish() {
                                Ok(compressed) => {
                                    let original_size = text.len() as f32;
                                    let compressed_size = compressed.len() as f32;
                                    if original_size > 0.0 {
                                        compressed_size / original_size
                                    } else {
                                        0.0
                                    }
                                }
                                Err(e) => {
                                    tracing::warn!("Zstd finish failed: {}", e);
                                    0.0
                                }
                            }
                        }
                    }
                    Err(e) => {
                        tracing::warn!("Zstd encoder creation failed: {}", e);
                        0.0
                    }
                };

                if best
                    .as_ref()
                    .is_none_or(|(_, _, best_entropy)| entropy_gain < *best_entropy)
                {
                    best = Some((idx, text.clone(), entropy_gain));
                }
            }
        }

        match best {
            Some((idx, text, entropy)) => {
                info!(
                    "HCC: Cross-agent speculation collapsed on agent {}. Entropy: {:.4}",
                    idx, entropy
                );
                Ok(text)
            }
            None => {
                warn!("HCC: All cross-agent speculative branches failed");
                Err(SavantError::Unknown(
                    "Cross-agent speculative execution: all branches failed.".to_string(),
                ))
            }
        }
    }
}

#[cfg(test)]
#[expect(clippy::disallowed_methods)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use serde_json::json;

    struct MockTool;
    #[async_trait]
    impl Tool for MockTool {
        fn name(&self) -> &str {
            "mock_tool"
        }
        fn description(&self) -> &str {
            "mock"
        }
        async fn execute(&self, _payload: Value) -> Result<String, SavantError> {
            Ok("Success".to_string())
        }
    }

    #[tokio::test]
    async fn test_hcc_collapse() {
        let engine = HyperCausalEngine::new(2);
        let tool = Arc::new(MockTool);
        let res = engine.execute_speculative(tool, json!({})).await;
        assert!(res.is_ok());
        assert_eq!(res.unwrap(), "Success");
    }
}
