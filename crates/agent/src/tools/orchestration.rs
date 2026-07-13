// SAFETY: All `clippy::disallowed_methods` violations in this file originate from
// the `serde_json::json!()` macro, which internally uses `.unwrap()` on
// compile-time-validated JSON literals. A malformed JSON literal would be a
// compile error, making the panic path statically unreachable.
#![allow(clippy::disallowed_methods)]

use crate::orchestration::tasks::{TaskMatrix, TaskStatus};
use async_trait::async_trait;
use savant_core::error::SavantError;
use savant_core::traits::Tool;
use std::path::PathBuf;

/// OMEGA-VIII: Task Matrix Management Tool
/// Allows agents to autonomously update their orchestration state.
pub struct TaskMatrixTool {
    workspace_path: PathBuf,
    config: savant_core::config::ProactiveConfig,
}

impl TaskMatrixTool {
    pub fn new(workspace_path: PathBuf, config: savant_core::config::ProactiveConfig) -> Self {
        Self {
            workspace_path,
            config,
        }
    }
}

#[async_trait]
impl Tool for TaskMatrixTool {
    fn name(&self) -> &str {
        "update_task_status"
    }

    fn description(&self) -> &str {
        "Update the status of a task in the orchestration matrix."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "description": { "type": "string", "description": "Task description" },
                "status": { "type": "string", "description": "New status", "enum": ["pending", "in_progress", "completed", "failed"] }
            },
            "required": ["description", "status"]
        })
    }

    fn capabilities(&self) -> savant_core::types::CapabilityGrants {
        savant_core::types::CapabilityGrants::default()
    }

    async fn execute(&self, payload: serde_json::Value) -> Result<String, SavantError> {
        let matrix = TaskMatrix::new(&self.workspace_path, &self.config);

        let description = payload["description"]
            .as_str()
            .ok_or_else(|| SavantError::Unknown("Missing 'description' parameter".to_string()))?;

        // NA-03: Support "create" action via add_task()
        let action = payload["action"]
            .as_str()
            .unwrap_or("update")
            .to_lowercase();

        if action == "create" {
            return match matrix.add_task(description) {
                Ok(_) => Ok(format!("Successfully created task '{}'", description)),
                Err(e) => Err(SavantError::OperationFailed(format!(
                    "Error creating task: {}",
                    e
                ))),
            };
        }

        let status_str = payload["status"]
            .as_str()
            .ok_or_else(|| SavantError::Unknown("Missing 'status' parameter".to_string()))?
            .to_lowercase();

        let status = match status_str.as_str() {
            "pending" => TaskStatus::Pending,
            "inprogress" | "in_progress" => TaskStatus::InProgress,
            "completed" => TaskStatus::Completed,
            "failed" => TaskStatus::Failed,
            _ => {
                return Err(SavantError::OperationFailed(format!(
                    "Invalid status '{}'",
                    status_str
                )))
            }
        };

        match matrix.toggle_task(description, status) {
            Ok(_) => Ok(format!(
                "Successfully updated task '{}' to {:?}",
                description, status
            )),
            Err(e) => Err(SavantError::OperationFailed(format!(
                "Error updating task: {}",
                e
            ))),
        }
    }
}

/// NS-10: Sovereign Synthesizer Tool
/// Exposes the autonomous WASI-sandboxed tool synthesis capability as a tool.
/// Allows agents to autonomously create new tools from natural language descriptions.
pub struct SovereignSynthesizerTool {
    synthesizer: crate::orchestration::synthesis::SovereignSynthesizer,
}

impl SovereignSynthesizerTool {
    pub fn new(workspace_dir: PathBuf) -> Self {
        Self {
            synthesizer: crate::orchestration::synthesis::SovereignSynthesizer::new(workspace_dir),
        }
    }
}

#[async_trait]
impl Tool for SovereignSynthesizerTool {
    fn name(&self) -> &str {
        "synthesize_skill"
    }

    fn description(&self) -> &str {
        "Autonomously synthesize a new WASI-sandboxed skill/tool from a natural language description. \
         Uses the Omega-III synthesis loop with self-healing verification."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "skill_name": {
                    "type": "string",
                    "description": "Name for the new skill"
                },
                "logic_prompt": {
                    "type": "string",
                    "description": "Natural language description of what the skill should do"
                }
            },
            "required": ["skill_name", "logic_prompt"]
        })
    }

    fn domain(&self) -> savant_core::traits::ToolDomain {
        savant_core::traits::ToolDomain::Orchestrator
    }

    fn timeout_secs(&self) -> u64 {
        300 // Synthesis can take a while
    }

    async fn execute(&self, payload: serde_json::Value) -> Result<String, SavantError> {
        let skill_name = payload["skill_name"]
            .as_str()
            .ok_or_else(|| SavantError::Unknown("Missing 'skill_name' parameter".to_string()))?;

        let logic_prompt = payload["logic_prompt"]
            .as_str()
            .ok_or_else(|| SavantError::Unknown("Missing 'logic_prompt' parameter".to_string()))?;

        match self
            .synthesizer
            .synthesize_skill(skill_name, logic_prompt)
            .await
        {
            Ok(path) => Ok(format!(
                "Skill '{}' synthesized successfully at: {}",
                skill_name,
                path.display()
            )),
            Err(e) => Err(SavantError::Unknown(format!(
                "Skill synthesis failed: {}",
                e
            ))),
        }
    }
}
