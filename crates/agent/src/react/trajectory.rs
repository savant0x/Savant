//! Trajectory Recording — captures agent conversations as ShareGPT-compatible training data.
//!
//! Records each step of a ReAct loop (system prompt, user messages, assistant responses
//! with tool calls, tool results) and serializes to JSONL in ShareGPT format.
//! Failed trajectories are discarded — only successful interactions produce training data.
// SAFETY: All `clippy::disallowed_methods` violations in this file originate from
// the `serde_json::json!()` macro, which internally uses `.unwrap()` on
// compile-time-validated JSON literals. A malformed JSON literal would be a
// compile error, making the panic path statically unreachable.
#![allow(clippy::disallowed_methods)]

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// A single step in a recorded trajectory.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TrajectoryStep {
    System {
        content: String,
    },
    User {
        content: String,
    },
    Assistant {
        content: String,
        tool_calls: Vec<ToolCallRecord>,
    },
    ToolResult {
        name: String,
        result: String,
    },
}

/// A tool call record captured during trajectory recording.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallRecord {
    pub name: String,
    pub arguments: serde_json::Value,
}

/// Records agent conversations as ShareGPT-compatible training data.
pub struct TrajectoryRecorder {
    session_id: String,
    steps: Vec<TrajectoryStep>,
    started_at: i64,
    output_dir: PathBuf,
    enabled: bool,
}

impl TrajectoryRecorder {
    /// Creates a new trajectory recorder.
    pub fn new(session_id: String, output_dir: PathBuf, enabled: bool) -> Self {
        Self {
            session_id,
            steps: Vec::new(),
            started_at: chrono::Utc::now().timestamp(),
            output_dir,
            enabled,
        }
    }

    /// Record the system prompt as the first step.
    pub fn record_system_prompt(&mut self, content: &str) {
        if !self.enabled {
            return;
        }
        self.steps.push(TrajectoryStep::System {
            content: content.to_string(),
        });
    }

    /// Record a user message.
    pub fn record_user_message(&mut self, content: &str) {
        if !self.enabled {
            return;
        }
        self.steps.push(TrajectoryStep::User {
            content: content.to_string(),
        });
    }

    /// Record an assistant response, optionally with tool calls.
    pub fn record_assistant_response(&mut self, content: &str, tool_calls: Vec<ToolCallRecord>) {
        if !self.enabled {
            return;
        }
        self.steps.push(TrajectoryStep::Assistant {
            content: content.to_string(),
            tool_calls,
        });
    }

    /// Record a tool result. Applies TOON compression for uniform JSON arrays.
    pub fn record_tool_result(&mut self, name: &str, result: &str) {
        if !self.enabled {
            return;
        }
        // Apply TOON compression if result is a uniform JSON array
        let compressed = if let Ok(value) = serde_json::from_str::<serde_json::Value>(result) {
            if crate::react::toon::ToonEncoder::is_uniform_array(&value) {
                crate::react::toon::ToonEncoder::encode(&value)
            } else {
                result.to_string()
            }
        } else {
            result.to_string()
        };
        self.steps.push(TrajectoryStep::ToolResult {
            name: name.to_string(),
            result: compressed,
        });
    }

    /// Finalize the trajectory. Writes to disk if `success` is true.
    /// Discards the trajectory if `success` is false (don't reinforce failures).
    pub fn finish(&self, success: bool) -> Result<(), savant_core::error::SavantError> {
        if !self.enabled || self.steps.is_empty() {
            return Ok(());
        }

        if !success {
            tracing::debug!(
                "[trajectory] Discarding trajectory for session {} (task failed)",
                self.session_id
            );
            return Ok(());
        }

        // Ensure output directory exists
        std::fs::create_dir_all(&self.output_dir).map_err(|e| {
            savant_core::error::SavantError::IoError(std::io::Error::other(format!(
                "Failed to create trajectory output dir: {}",
                e
            )))
        })?;

        let timestamp = chrono::Utc::now().timestamp();
        let duration_ms = timestamp.saturating_sub(self.started_at) * 1000;
        tracing::debug!(
            "[trajectory] Session {} completed in {}ms, {} steps",
            self.session_id,
            duration_ms,
            self.steps.len()
        );
        let filename = format!("{}_{}.jsonl", self.session_id, timestamp);
        let filepath = self.output_dir.join(filename);

        let sharegpt = self.to_sharegpt();
        let json_line = serde_json::to_string(&sharegpt).map_err(|e| {
            savant_core::error::SavantError::Unknown(format!(
                "Failed to serialize trajectory: {}",
                e
            ))
        })?;

        std::fs::write(&filepath, format!("{}\n", json_line)).map_err(|e| {
            savant_core::error::SavantError::IoError(std::io::Error::other(format!(
                "Failed to write trajectory file: {}",
                e
            )))
        })?;

        tracing::info!(
            "[trajectory] Wrote trajectory for session {} ({} steps) to {}",
            self.session_id,
            self.steps.len(),
            filepath.display()
        );

        Ok(())
    }

    /// Serialize to ShareGPT format.
    fn to_sharegpt(&self) -> serde_json::Value {
        let conversations: Vec<serde_json::Value> = self
            .steps
            .iter()
            .map(|step| match step {
                TrajectoryStep::System { content } => serde_json::json!({
                    "from": "system",
                    "value": content
                }),
                TrajectoryStep::User { content } => serde_json::json!({
                    "from": "human",
                    "value": content
                }),
                TrajectoryStep::Assistant {
                    content,
                    tool_calls,
                } => {
                    let mut value = String::new();
                    if !content.is_empty() {
                        value.push_str(&format!("<think>{}\n</think>\n", content));
                    }
                    for tc in tool_calls {
                        let tc_json = serde_json::to_string(&serde_json::json!({
                            "name": tc.name,
                            "arguments": tc.arguments
                        }))
                        .unwrap_or_else(|e| format!("{{\"error\": \"{}\"}}", e));
                        value.push_str(&format!("<tool_call>\n{}\n</tool_response>\n", tc_json));
                    }
                    serde_json::json!({ "from": "gpt", "value": value })
                }
                TrajectoryStep::ToolResult { name: _, result } => {
                    serde_json::json!({
                        "from": "tool",
                        "value": format!("<tool_response>\n{}\n</tool_response>", result)
                    })
                }
            })
            .collect();

        serde_json::json!({
            "conversations": conversations
        })
    }

    /// Returns the number of recorded steps.
    pub fn step_count(&self) -> usize {
        self.steps.len()
    }

    /// Returns whether recording is enabled.
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_record_and_finish_success() {
        let tmp = TempDir::new().unwrap();
        let mut rec =
            TrajectoryRecorder::new("test-session".to_string(), tmp.path().to_path_buf(), true);

        rec.record_user_message("What is Rust?");
        rec.record_assistant_response("Rust is a systems language.", vec![]);
        assert_eq!(rec.step_count(), 2);

        rec.finish(true).unwrap();

        let entries: Vec<_> = std::fs::read_dir(tmp.path()).unwrap().collect();
        assert_eq!(entries.len(), 1);
    }

    #[test]
    fn test_finish_failure_discards() {
        let tmp = TempDir::new().unwrap();
        let mut rec =
            TrajectoryRecorder::new("test-session".to_string(), tmp.path().to_path_buf(), true);

        rec.record_user_message("Do something");
        rec.finish(false).unwrap();

        let entries: Vec<_> = std::fs::read_dir(tmp.path()).unwrap().collect();
        assert_eq!(entries.len(), 0);
    }

    #[test]
    fn test_disabled_recorder() {
        let tmp = TempDir::new().unwrap();
        let mut rec = TrajectoryRecorder::new("test".to_string(), tmp.path().to_path_buf(), false);

        rec.record_user_message("This should not be recorded");
        assert_eq!(rec.step_count(), 0);

        rec.finish(true).unwrap();
        let entries: Vec<_> = std::fs::read_dir(tmp.path()).unwrap().collect();
        assert_eq!(entries.len(), 0);
    }

    #[test]
    fn test_sharegpt_format() {
        let tmp = TempDir::new().unwrap();
        let mut rec = TrajectoryRecorder::new("test".to_string(), tmp.path().to_path_buf(), true);

        rec.record_system_prompt("You are helpful.");
        rec.record_user_message("Hello");
        rec.record_assistant_response("Hi!", vec![]);
        rec.finish(true).unwrap();

        let entries: Vec<_> = std::fs::read_dir(tmp.path()).unwrap().collect();
        let path = entries[0].as_ref().unwrap().path();
        let content = std::fs::read_to_string(&path).unwrap();
        let json: serde_json::Value = serde_json::from_str(&content).unwrap();

        let convs = json["conversations"].as_array().unwrap();
        assert_eq!(convs.len(), 3);
        assert_eq!(convs[0]["from"], "system");
        assert_eq!(convs[1]["from"], "human");
        assert_eq!(convs[2]["from"], "gpt");
    }

    #[test]
    fn test_tool_calls_in_sharegpt() {
        let tmp = TempDir::new().unwrap();
        let mut rec = TrajectoryRecorder::new("test".to_string(), tmp.path().to_path_buf(), true);

        rec.record_user_message("Search for Rust");
        rec.record_assistant_response(
            "Let me search.",
            vec![ToolCallRecord {
                name: "web_search".to_string(),
                arguments: serde_json::json!({"query": "Rust language"}),
            }],
        );
        rec.record_tool_result("web_search", "Found 10 results");
        rec.record_assistant_response("Here are the results.", vec![]);
        rec.finish(true).unwrap();

        let entries: Vec<_> = std::fs::read_dir(tmp.path()).unwrap().collect();
        let path = entries[0].as_ref().unwrap().path();
        let content = std::fs::read_to_string(&path).unwrap();
        let json: serde_json::Value = serde_json::from_str(&content).unwrap();

        let convs = json["conversations"].as_array().unwrap();
        assert_eq!(convs.len(), 4);
        assert_eq!(convs[2]["from"], "tool");
        let gpt_value = convs[1]["value"].as_str().unwrap();
        assert!(gpt_value.contains("<tool_call>"));
        assert!(gpt_value.contains("web_search"));
    }

    #[test]
    fn test_empty_trajectory_no_write() {
        let tmp = TempDir::new().unwrap();
        let rec = TrajectoryRecorder::new("test".to_string(), tmp.path().to_path_buf(), true);

        rec.finish(true).unwrap();

        let entries: Vec<_> = std::fs::read_dir(tmp.path()).unwrap().collect();
        assert_eq!(entries.len(), 0);
    }
}
