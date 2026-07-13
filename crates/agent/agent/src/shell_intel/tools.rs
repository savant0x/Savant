//! Shell intelligence tools — command explanation and risk detection.

use super::explainer::{explain, RiskLevel};
use super::parser::parse_command;
use async_trait::async_trait;
use savant_core::error::SavantError;
use savant_core::traits::Tool;

/// Tool: explain_command — explain what a shell command does and detect risks.
pub struct ExplainCommandTool;

#[async_trait]
impl Tool for ExplainCommandTool {
    fn name(&self) -> &str {
        "explain_command"
    }

    fn description(&self) -> &str {
        "Explain what a shell command does and detect security risks. Returns a human-readable explanation with risk level (Safe/Warning/Danger)."
    }

    #[allow(clippy::disallowed_methods)] // serde_json::json! macro
    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "command": { "type": "string", "description": "The shell command to explain" }
            },
            "required": ["command"]
        })
    }

    async fn execute(&self, payload: serde_json::Value) -> Result<String, SavantError> {
        let command = payload
            .get("command")
            .and_then(|v| v.as_str())
            .ok_or_else(|| SavantError::InvalidInput("missing 'command' parameter".into()))?;

        let analysis = parse_command(command);
        let explanation = explain(command, &analysis);

        let risk_label = match explanation.risk_level {
            RiskLevel::Safe => "SAFE",
            RiskLevel::Warning => "WARNING",
            RiskLevel::Danger => "DANGER",
        };

        let mut output = format!(
            "[{}] {}\n\n{}",
            risk_label, command, explanation.explanation
        );

        if !explanation.risks.is_empty() {
            output.push_str("\n\nRisks:\n");
            for risk in &explanation.risks {
                output.push_str(&format!("  - {}\n", risk));
            }
        }

        if !explanation.sub_explanations.is_empty() && explanation.sub_explanations.len() > 1 {
            output.push_str("\n\nBreakdown:\n");
            for (i, sub) in explanation.sub_explanations.iter().enumerate() {
                output.push_str(&format!("  {}. {}\n", i + 1, sub));
            }
        }

        Ok(output)
    }
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_explain_command_tool() {
        let tool = ExplainCommandTool;
        let result = tool
            .execute(serde_json::json!({"command": "ls -la /tmp"}))
            .await
            .unwrap();
        assert!(result.contains("SAFE"));
        assert!(result.contains("List directory"));
    }

    #[tokio::test]
    async fn test_explain_dangerous_command() {
        let tool = ExplainCommandTool;
        let result = tool
            .execute(serde_json::json!({"command": "bash -c 'rm -rf /'"}))
            .await
            .unwrap();
        assert!(result.contains("DANGER"));
    }
}
