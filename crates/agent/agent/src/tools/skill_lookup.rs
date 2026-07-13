// SAFETY: All `clippy::disallowed_methods` violations in this file originate from
// the `serde_json::json!()` macro, which internally uses `.unwrap()` on
// compile-time-validated JSON literals. A malformed JSON literal would be a
// compile error, making the panic path statically unreachable.
#![allow(clippy::disallowed_methods)]

use async_trait::async_trait;
use savant_core::error::SavantError;
use savant_core::traits::Tool;
use serde_json::Value;
use std::sync::Arc;
use tokio::sync::Mutex;

/// On-demand skill instruction lookup tool.
///
/// The agent only sees skill names + descriptions in the system prompt.
/// When it needs full instructions for a specific skill, it calls this tool
/// to retrieve the complete SKILL.md content. This keeps the system prompt
/// lean while still providing full detail on demand.
pub struct SkillLookupTool {
    skill_manager: Arc<Mutex<savant_skills::parser::SkillManager>>,
}

impl SkillLookupTool {
    pub fn new(skill_manager: Arc<Mutex<savant_skills::parser::SkillManager>>) -> Self {
        Self { skill_manager }
    }
}

#[async_trait]
impl Tool for SkillLookupTool {
    fn name(&self) -> &str {
        "skill_lookup"
    }

    fn description(&self) -> &str {
        "Retrieve full instructions for a skill by name. Use when you need detailed \
         usage info, parameter descriptions, or examples for a specific skill. \
         Call with the skill name to get the complete SKILL.md content."
    }

    fn parameters_schema(&self) -> Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "The skill name to look up (e.g. 'coding', 'research')"
                }
            },
            "required": ["name"]
        })
    }

    async fn execute(&self, payload: Value) -> Result<String, SavantError> {
        let skill_name = payload["name"]
            .as_str()
            .ok_or_else(|| SavantError::Unknown("Missing required field 'name'".to_string()))?;

        let manager = self.skill_manager.lock().await;
        let registry = manager.registry();

        match registry.get_skill_instructions(skill_name) {
            Some(instructions) => Ok(instructions),
            None => {
                let available: Vec<String> = registry.manifests.keys().cloned().collect();
                if available.is_empty() {
                    Ok(format!(
                        "Skill '{}' not found. No skills are currently loaded.",
                        skill_name
                    ))
                } else {
                    Ok(format!(
                        "Skill '{}' not found. Available skills: {}",
                        skill_name,
                        available.join(", ")
                    ))
                }
            }
        }
    }
}
