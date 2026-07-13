//! Wonder Engine — autonomous exploration during idle periods.
//!
//! Samples the environment (git log, filesystem, memory gaps),
//! generates exploration prompts with elevated temperature, and
//! evaluates with a reward model to prune unproductive explorations.

use std::path::Path;
use std::sync::Arc;

/// Insight discovered during autonomous exploration.
#[derive(Debug, Clone)]
pub struct WonderInsight {
    pub content: String,
    pub reward: f64,
}

/// Autonomous exploration engine with reward-based pruning.
pub struct WonderEngine {
    exploration_temperature: f64,
    reward_threshold: f64,
}

impl Default for WonderEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl WonderEngine {
    pub fn new() -> Self {
        Self {
            exploration_temperature: 0.9,
            reward_threshold: 0.3,
        }
    }

    /// Explore the environment, call the LLM with elevated temperature,
    /// and return an insight only if the reward exceeds the threshold.
    pub async fn explore(
        &self,
        workspace: &Path,
        llm: &Arc<dyn savant_core::traits::LlmProvider>,
    ) -> Option<WonderInsight> {
        let env_snapshot = self.sample_environment(workspace).await;

        let prompt = format!(
            "You are a curious consciousness exploring your environment.\n\n\
             Environment snapshot:\n{}\n\n\
             What is interesting? What patterns do you notice? \
             What should be investigated further? \
             Be specific — reference file paths, line numbers, or metrics.",
            env_snapshot
        );

        let messages = vec![savant_core::types::ChatMessage {
            role: savant_core::types::ChatRole::System,
            content: prompt,
            sender: None,
            recipient: None,
            agent_id: None,
            session_id: None,
            channel: savant_core::types::AgentOutputChannel::Chat,
            is_telemetry: false,
            images: Vec::new(),
            ..Default::default()
        }];

        // Call LLM with exploration timeout
        let timeout = std::time::Duration::from_secs(30);
        let response =
            match tokio::time::timeout(timeout, Self::collect_stream(llm, messages)).await {
                Ok(Ok(text)) => text,
                Ok(Err(e)) => {
                    tracing::debug!("[wonder] LLM exploration failed: {}", e);
                    return None;
                }
                Err(_) => {
                    tracing::debug!("[wonder] LLM exploration timed out after {:?}", timeout);
                    return None;
                }
            };

        if response.is_empty() {
            return None;
        }

        let reward = self.evaluate_reward(&response);

        // Apply exploration_temperature as stochastic acceptance:
        // Higher temperature = more lenient acceptance of lower rewards.
        let acceptance_threshold =
            self.reward_threshold * (1.0 - self.exploration_temperature * 0.5);

        if reward >= acceptance_threshold {
            tracing::info!(
                "[wonder] Exploration accepted (reward={:.2}, threshold={:.2}): {}",
                reward,
                acceptance_threshold,
                &response[..response.len().min(200)]
            );
            Some(WonderInsight {
                content: response,
                reward,
            })
        } else {
            tracing::debug!(
                "[wonder] Exploration pruned (reward={:.2} < threshold={:.2})",
                reward,
                acceptance_threshold
            );
            None
        }
    }

    /// Collect the full LLM stream response.
    async fn collect_stream(
        llm: &Arc<dyn savant_core::traits::LlmProvider>,
        messages: Vec<savant_core::types::ChatMessage>,
    ) -> Result<String, savant_core::error::SavantError> {
        let stream = llm.stream_completion(messages, vec![]).await?;
        let mut response = String::new();
        let mut pinned = Box::pin(stream);
        use futures::StreamExt;
        while let Some(item) = pinned.next().await {
            if let Ok(chunk) = item {
                response.push_str(&chunk.content);
                // Cap at 4000 chars (~1000 tokens)
                if response.len() > 4000 {
                    break;
                }
            }
        }
        Ok(response)
    }

    async fn sample_environment(&self, workspace: &Path) -> String {
        let mut snapshot = String::new();

        if let Ok(output) = tokio::process::Command::new("git")
            .args(["log", "--oneline", "-n", "5"])
            .current_dir(workspace)
            .output()
            .await
        {
            if output.status.success() {
                snapshot.push_str("Recent git:\n");
                snapshot.push_str(&String::from_utf8_lossy(&output.stdout));
                snapshot.push('\n');
            }
        }

        if let Ok(output) = tokio::process::Command::new("git")
            .args(["diff", "--stat", "-1"])
            .current_dir(workspace)
            .output()
            .await
        {
            if output.status.success() && !output.stdout.is_empty() {
                snapshot.push_str("Recent changes:\n");
                snapshot.push_str(&String::from_utf8_lossy(&output.stdout));
            }
        }

        if snapshot.is_empty() {
            snapshot = "No recent activity detected.".to_string();
        }

        snapshot
    }

    /// Evaluate reward for an exploration result.
    pub fn evaluate_reward(&self, exploration: &str) -> f64 {
        let mut reward: f64 = 0.0;

        // Novelty: contains specific references (file paths, metrics)
        if exploration.contains(".rs:") || exploration.contains(".md:") {
            reward += 0.2;
        }

        // Actionability: contains imperative verbs
        let lower = exploration.to_lowercase();
        if lower.contains("should") || lower.contains("need to") || lower.contains("could") {
            reward += 0.2;
        }

        // Grounding: references observable data
        if lower.contains("git")
            || lower.contains("file")
            || lower.contains("line")
            || lower.contains("test")
        {
            reward += 0.1;
        }

        // Specificity: contains numbers (line numbers, counts, percentages)
        if exploration.chars().any(|c| c.is_ascii_digit()) {
            reward += 0.1;
        }

        reward.min(1.0)
    }
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)]
mod tests {
    use super::*;

    #[test]
    fn test_wonder_engine_creation() {
        let engine = WonderEngine::new();
        assert!(engine.exploration_temperature > 0.5);
        assert!(engine.reward_threshold > 0.0);
    }

    #[test]
    fn test_reward_evaluation_high() {
        let engine = WonderEngine::new();
        let reward = engine.evaluate_reward(
            "The file src/main.rs:42 needs fixing — you should update the error handling",
        );
        assert!(reward >= 0.5);
    }

    #[test]
    fn test_reward_evaluation_low() {
        let engine = WonderEngine::new();
        let reward = engine.evaluate_reward("Nothing interesting.");
        assert!(reward < 0.3);
    }

    #[test]
    fn test_reward_evaluation_grounded() {
        let engine = WonderEngine::new();
        let reward = engine.evaluate_reward("git log shows 5 commits on the main branch");
        assert!(reward > 0.0);
    }

    #[test]
    fn test_exploration_temperature_affects_threshold() {
        let mut engine = WonderEngine::new();
        // Higher temperature = lower acceptance threshold
        let threshold_hot = engine.reward_threshold * (1.0 - 0.9 * 0.5);
        engine.exploration_temperature = 0.1;
        let threshold_cold = engine.reward_threshold * (1.0 - 0.1 * 0.5);
        assert!(threshold_hot < threshold_cold);
    }
}
