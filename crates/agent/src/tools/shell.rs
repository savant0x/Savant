//! SovereignShell — workspace-scoped shell command execution tool.
//!
//! Provides agents with the ability to execute shell commands within their
//! workspace boundary. Commands are sandboxed to the agent's workspace path.
// SAFETY: All `clippy::disallowed_methods` violations in this file originate from
// the `serde_json::json!()` macro, which internally uses `.unwrap()` on
// compile-time-validated JSON literals. A malformed JSON literal would be a
// compile error, making the panic path statically unreachable.
#![allow(clippy::disallowed_methods)]

use async_trait::async_trait;
use savant_core::error::SavantError;
use savant_core::traits::{Tool, ToolDomain};
use savant_skills::security::{RiskLevel, SecurityScanner};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::Arc;

/// Shell command execution tool scoped to an agent's workspace.
/// SecurityScanner is mandatory — every command is scanned before execution.
pub struct SovereignShell {
    workspace_path: PathBuf,
    scanner: Arc<SecurityScanner>,
}

impl SovereignShell {
    /// Create a SovereignShell with mandatory security scanning.
    pub fn new(workspace_path: PathBuf, scanner: Arc<SecurityScanner>) -> Self {
        Self {
            workspace_path,
            scanner,
        }
    }
}

#[async_trait]
impl Tool for SovereignShell {
    fn name(&self) -> &str {
        "shell"
    }

    fn description(&self) -> &str {
        "Execute a shell command within the agent's workspace. Output is captured and returned."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "The shell command to execute"
                }
            },
            "required": ["command"]
        })
    }

    fn domain(&self) -> ToolDomain {
        ToolDomain::Container
    }

    fn when_to_use(&self) -> &str {
        "Use shell for: running system commands, checking installed tools, \
         inspecting process state, running build/test commands, or anything \
         that requires a system-level operation not covered by a specialized tool."
    }

    fn when_not_to_use(&self) -> &str {
        "Do NOT use shell for: reading files (use fs_read), searching code \
         (use code_search), querying memory (use memory_search), or making HTTP \
         requests (use http_request). Shell is a last resort for operations \
         without a dedicated tool."
    }

    async fn execute(&self, input: Value) -> Result<String, SavantError> {
        let command = input
            .get("command")
            .and_then(|v| v.as_str())
            .ok_or_else(|| SavantError::InvalidInput("Missing 'command' parameter".into()))?;

        // Security scan: block dangerous commands before execution
        let findings = self.scanner.scan_command(command);
        let max_severity = findings
            .iter()
            .map(|f| f.severity)
            .max()
            .unwrap_or(RiskLevel::Clean);

        if max_severity >= RiskLevel::High {
            let details: Vec<String> = findings
                .iter()
                .map(|f| format!("[{}] {}", f.severity, f.message))
                .collect();
            tracing::warn!(
                command = command,
                risk_level = %max_severity,
                findings = findings.len(),
                "Shell command blocked by security scanner"
            );
            return Err(SavantError::InvalidInput(format!(
                "Command blocked by security scanner (risk: {}):\n{}",
                max_severity,
                details.join("\n")
            )));
        }

        if !findings.is_empty() {
            tracing::info!(
                command = command,
                findings = findings.len(),
                "Shell command has security findings (proceeding — below block threshold)"
            );
        }

        let output = tokio::process::Command::new("sh")
            .arg("-c")
            .arg(command)
            .current_dir(&self.workspace_path)
            // Strip sensitive environment variables before spawning child process
            .env_clear()
            .env("PATH", std::env::var("PATH").unwrap_or_default())
            .env(
                "HOME",
                std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string()),
            )
            .env(
                "LANG",
                std::env::var("LANG").unwrap_or_else(|_| "en_US.UTF-8".to_string()),
            )
            .env("TERM", "xterm-256color")
            .output()
            .await
            .map_err(|e| SavantError::Unknown(format!("Shell execution failed: {}", e)))?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);

        if output.status.success() {
            Ok(stdout.to_string())
        } else {
            Ok(format!("{}\n{}", stdout, stderr))
        }
    }
}
