//! Skill Chain Execution Engine
//!
//! Executes skill chains: sequential steps with conditional execution
//! and output passing between steps.
//!
//! Component from FID-20260526-WIRING-SPRINT.

use savant_core::error::SavantError;
use savant_core::types::{SkillChain, SkillChainStep};
use std::collections::HashMap;
use std::sync::Arc;

/// Result of executing a single skill chain step.
#[derive(Debug, Clone)]
pub struct StepResult {
    pub step_index: usize,
    pub skill_name: String,
    pub output: String,
    pub success: bool,
}

/// Result of executing an entire skill chain.
#[derive(Debug, Clone)]
pub struct ChainResult {
    pub chain_name: String,
    pub steps_executed: Vec<StepResult>,
    pub success: bool,
    pub error: Option<String>,
}

/// Executes skill chains with conditional execution and output passing.
pub struct SkillChainExecutor {
    /// Maximum steps before aborting (prevents infinite loops).
    max_steps: usize,
}

impl Default for SkillChainExecutor {
    fn default() -> Self {
        Self::new()
    }
}

impl SkillChainExecutor {
    pub fn new() -> Self {
        Self { max_steps: 50 }
    }

    /// Execute a skill chain.
    pub async fn execute(
        &self,
        chain: &SkillChain,
        tools: &HashMap<String, Arc<dyn savant_core::traits::Tool>>,
        context: &str,
    ) -> ChainResult {
        let mut step_results = Vec::new();
        let mut previous_output = String::new();

        if chain.steps.len() > self.max_steps {
            return ChainResult {
                chain_name: chain.name.clone(),
                steps_executed: step_results,
                success: false,
                error: Some(format!(
                    "Chain exceeds max steps ({} > {})",
                    chain.steps.len(),
                    self.max_steps
                )),
            };
        }

        for (index, step) in chain.steps.iter().enumerate() {
            // Check condition if specified
            if let Some(ref condition) = step.condition {
                if !self.evaluate_condition(condition, context, &previous_output) {
                    tracing::debug!(
                        "[skill-chain] Step {} ({}) skipped — condition '{}' not met",
                        index,
                        step.skill_name,
                        condition
                    );
                    step_results.push(StepResult {
                        step_index: index,
                        skill_name: step.skill_name.clone(),
                        output: String::new(),
                        success: true, // Skipped = not failed
                    });
                    continue;
                }
            }

            // Prepare input from previous step's output
            let input = if let Some(ref pass_as) = step.pass_output_as {
                format!("{}: {}", pass_as, previous_output)
            } else {
                previous_output.clone()
            };

            // Execute the skill
            match self.execute_step(step, tools, &input).await {
                Ok(output) => {
                    step_results.push(StepResult {
                        step_index: index,
                        skill_name: step.skill_name.clone(),
                        output: output.clone(),
                        success: true,
                    });
                    previous_output = output;
                }
                Err(e) => {
                    tracing::error!(
                        "[skill-chain] Step {} ({}) failed: {}",
                        index,
                        step.skill_name,
                        e
                    );
                    step_results.push(StepResult {
                        step_index: index,
                        skill_name: step.skill_name.clone(),
                        output: format!("ERROR: {}", e),
                        success: false,
                    });
                    return ChainResult {
                        chain_name: chain.name.clone(),
                        steps_executed: step_results,
                        success: false,
                        error: Some(format!(
                            "Step {} ({}) failed: {}",
                            index, step.skill_name, e
                        )),
                    };
                }
            }
        }

        ChainResult {
            chain_name: chain.name.clone(),
            steps_executed: step_results,
            success: true,
            error: None,
        }
    }

    /// Execute a single step by finding and invoking the tool.
    #[allow(clippy::disallowed_methods)]
    async fn execute_step(
        &self,
        step: &SkillChainStep,
        tools: &HashMap<String, Arc<dyn savant_core::traits::Tool>>,
        input: &str,
    ) -> Result<String, SavantError> {
        let tool = tools.get(&step.skill_name).ok_or_else(|| {
            SavantError::Unknown(format!(
                "Skill '{}' not found in tool registry",
                step.skill_name
            ))
        })?;

        let payload = serde_json::json!({ "input": input });
        tool.execute(payload).await
    }

    /// Evaluate a condition string against context and previous output.
    fn evaluate_condition(&self, condition: &str, context: &str, previous_output: &str) -> bool {
        let condition_lower = condition.to_lowercase();
        let combined = format!("{} {}", context, previous_output).to_lowercase();

        // Simple keyword-based condition evaluation
        // Conditions like "podcast-guest-today" check if context contains "podcast" and "guest"
        let keywords: Vec<&str> = condition_lower.split('-').filter(|w| w.len() > 2).collect();
        if keywords.is_empty() {
            return true; // Empty condition = always pass
        }

        keywords.iter().all(|kw| combined.contains(kw))
    }
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)]
mod tests {
    use super::*;
    use savant_core::types::SkillChainStep;

    fn make_chain(name: &str, steps: Vec<SkillChainStep>) -> SkillChain {
        SkillChain {
            name: name.to_string(),
            steps,
        }
    }

    #[tokio::test]
    async fn test_empty_chain() {
        let executor = SkillChainExecutor::new();
        let chain = make_chain("empty", vec![]);
        let tools = HashMap::new();
        let result = executor.execute(&chain, &tools, "").await;
        assert!(result.success);
        assert!(result.steps_executed.is_empty());
    }

    #[tokio::test]
    async fn test_chain_step_not_found() {
        let executor = SkillChainExecutor::new();
        let chain = make_chain(
            "test",
            vec![SkillChainStep {
                skill_name: "nonexistent".to_string(),
                condition: None,
                pass_output_as: None,
            }],
        );
        let tools = HashMap::new();
        let result = executor.execute(&chain, &tools, "").await;
        assert!(!result.success);
        assert!(result.error.unwrap().contains("not found"));
    }

    #[tokio::test]
    async fn test_chain_skips_on_condition() {
        let executor = SkillChainExecutor::new();
        let chain = make_chain(
            "conditional",
            vec![SkillChainStep {
                skill_name: "step1".to_string(),
                condition: Some("impossible-condition-xyz".to_string()),
                pass_output_as: None,
            }],
        );
        let tools = HashMap::new();
        let result = executor.execute(&chain, &tools, "no match here").await;
        assert!(result.success); // Skipped = not failed
        assert_eq!(result.steps_executed.len(), 1);
    }

    #[tokio::test]
    async fn test_max_steps_exceeded() {
        let executor = SkillChainExecutor { max_steps: 2 };
        let chain = make_chain(
            "long",
            vec![
                SkillChainStep {
                    skill_name: "a".to_string(),
                    condition: None,
                    pass_output_as: None,
                },
                SkillChainStep {
                    skill_name: "b".to_string(),
                    condition: None,
                    pass_output_as: None,
                },
                SkillChainStep {
                    skill_name: "c".to_string(),
                    condition: None,
                    pass_output_as: None,
                },
            ],
        );
        let tools = HashMap::new();
        let result = executor.execute(&chain, &tools, "").await;
        assert!(!result.success);
        assert!(result.error.unwrap().contains("exceeds max steps"));
    }

    #[test]
    fn test_condition_evaluation() {
        let executor = SkillChainExecutor::new();
        assert!(executor.evaluate_condition("", "any context", ""));
        assert!(executor.evaluate_condition("podcast-guest", "podcast with guest John", ""));
        assert!(!executor.evaluate_condition("podcast-guest", "no match here", ""));
    }
}
