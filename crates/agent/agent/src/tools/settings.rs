// SAFETY: All `clippy::disallowed_methods` violations in this file originate from
// the `serde_json::json!()` macro, which internally uses `.unwrap()` on
// compile-time-validated JSON literals. A malformed JSON literal would be a
// compile error, making the panic path statically unreachable.
#![allow(clippy::disallowed_methods)]

use async_trait::async_trait;
use savant_core::error::SavantError;
use savant_core::traits::Tool;
use serde_json::Value;
use std::collections::HashMap;
use std::path::PathBuf;

/// Internal Settings Tool
/// Allows the agent to read and modify its own internal configuration state (settings.json).
#[derive(Debug, Clone)]
pub struct SettingsTool {
    settings_path: PathBuf,
}

impl Default for SettingsTool {
    fn default() -> Self {
        Self::new()
    }
}

impl SettingsTool {
    pub fn new() -> Self {
        Self {
            settings_path: PathBuf::from("settings.json"),
        }
    }

    /// PB-20: Creates a SettingsTool with the settings file resolved within the workspace.
    pub fn with_workspace(workspace_dir: &std::path::Path) -> Self {
        Self {
            settings_path: workspace_dir.join("settings.json"),
        }
    }

    async fn read_settings(&self) -> Result<HashMap<String, String>, String> {
        if !self.settings_path.exists() {
            return Ok(HashMap::new());
        }

        let content = tokio::fs::read_to_string(&self.settings_path)
            .await
            .map_err(|e| format!("Failed to read settings: {}", e))?;

        let settings: HashMap<String, String> = serde_json::from_str(&content)
            .map_err(|e| format!("Failed to parse settings: {}", e))?;

        Ok(settings)
    }

    async fn write_settings(&self, settings: &HashMap<String, String>) -> Result<(), String> {
        let content = serde_json::to_string_pretty(settings)
            .map_err(|e| format!("Failed to serialize settings: {}", e))?;

        tokio::fs::write(&self.settings_path, content)
            .await
            .map_err(|e| format!("Failed to write settings: {}", e))?;

        Ok(())
    }
}

#[async_trait]
impl Tool for SettingsTool {
    fn name(&self) -> &str {
        "savant_internal_settings"
    }

    fn description(&self) -> &str {
        "Read or modify internal agent settings."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "action": { "type": "string", "description": "Action to perform", "enum": ["get", "set", "list"] },
                "key": { "type": "string", "description": "Setting key (required for get/set)" },
                "value": { "type": "string", "description": "Setting value (required for set)" }
            },
            "required": ["action"]
        })
    }

    async fn execute(&self, input: Value) -> Result<String, SavantError> {
        let action = input.get("action").and_then(|v| v.as_str()).unwrap_or("");

        let mut settings = self
            .read_settings()
            .await
            .map_err(SavantError::OperationFailed)?;

        match action {
            "list" => {
                if settings.is_empty() {
                    return Ok("No internal settings found.".to_string());
                }
                let mut output = String::from("Current settings:\n");
                for (k, v) in &settings {
                    output.push_str(&format!("- {}: {}\n", k, v));
                }
                Ok(output)
            }
            "get" => {
                let key = input
                    .get("key")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| SavantError::OperationFailed("Missing 'key'".into()))?;
                if let Some(val) = settings.get(key) {
                    Ok(format!("{} = {}", key, val))
                } else {
                    Ok(format!("Setting '{}' is not defined.", key))
                }
            }
            "set" => {
                let key = input
                    .get("key")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| SavantError::OperationFailed("Missing 'key'".into()))?;
                let value = input
                    .get("value")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| SavantError::OperationFailed("Missing 'value'".into()))?;

                settings.insert(key.to_string(), value.to_string());
                self.write_settings(&settings)
                    .await
                    .map_err(SavantError::OperationFailed)?;

                Ok(format!("Successfully updated setting: {} = {}", key, value))
            }
            _ => Err(SavantError::OperationFailed(format!(
                "Unknown action: {}",
                action
            ))),
        }
    }
}
