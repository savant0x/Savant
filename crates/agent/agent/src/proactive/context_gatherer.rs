//! Proactive Context Gathering
//!
//! Gathers relevant context BEFORE the user asks by searching memories,
//! code references, recent git log, and related files in parallel.
//!
//! Component 2 of FID-20260525-AGENT-INTELLIGENCE.

use std::path::Path;
use std::sync::Arc;

/// Gathered context from multiple sources.
#[derive(Debug, Default)]
pub struct GatheredContext {
    pub memories: Vec<String>,
    pub code_references: Vec<String>,
    pub recent_changes: Vec<String>,
    pub related_files: Vec<String>,
}

impl GatheredContext {
    /// Check if any context was gathered.
    pub fn is_empty(&self) -> bool {
        self.memories.is_empty()
            && self.code_references.is_empty()
            && self.recent_changes.is_empty()
            && self.related_files.is_empty()
    }

    /// Format as a context block for injection into system prompt.
    pub fn format_for_prompt(&self) -> String {
        let mut out = String::new();

        if !self.memories.is_empty() {
            out.push_str("RELEVANT MEMORIES:\n");
            for m in &self.memories {
                out.push_str(&format!("- {}\n", m));
            }
            out.push('\n');
        }

        if !self.code_references.is_empty() {
            out.push_str("CODE REFERENCES:\n");
            for c in &self.code_references {
                out.push_str(&format!("- {}\n", c));
            }
            out.push('\n');
        }

        if !self.recent_changes.is_empty() {
            out.push_str("RECENT CHANGES:\n");
            for c in self.recent_changes.iter().take(5) {
                out.push_str(&format!("- {}\n", c));
            }
            out.push('\n');
        }

        out
    }
}

/// Proactively gathers context from multiple sources in parallel.
pub struct ProactiveContextGatherer {
    max_results: usize,
}

impl Default for ProactiveContextGatherer {
    fn default() -> Self {
        Self::new()
    }
}

impl ProactiveContextGatherer {
    pub fn new() -> Self {
        Self { max_results: 5 }
    }

    /// Gather context from memories, code, git, and files in parallel.
    #[allow(clippy::disallowed_methods)]
    pub async fn gather(
        &self,
        topic: &str,
        memory: &Arc<dyn savant_core::traits::MemoryBackend>,
        workspace: &Path,
    ) -> GatheredContext {
        let (mem_result, git_result) = tokio::join!(
            self.search_memories(topic, memory),
            self.recent_git_log(workspace, 10),
        );

        GatheredContext {
            memories: mem_result.unwrap_or_default(),
            recent_changes: git_result.unwrap_or_default(),
            ..Default::default()
        }
    }

    async fn search_memories(
        &self,
        topic: &str,
        memory: &Arc<dyn savant_core::traits::MemoryBackend>,
    ) -> Result<Vec<String>, savant_core::error::SavantError> {
        let results = memory.retrieve("system", topic, self.max_results).await?;
        Ok(results.into_iter().map(|m| m.content).collect())
    }

    async fn recent_git_log(
        &self,
        workspace: &Path,
        count: usize,
    ) -> Result<Vec<String>, savant_core::error::SavantError> {
        let output = tokio::process::Command::new("git")
            .args(["log", "--oneline", "-n", &count.to_string()])
            .current_dir(workspace)
            .output()
            .await;

        match output {
            Ok(output) if output.status.success() => Ok(String::from_utf8_lossy(&output.stdout)
                .lines()
                .map(String::from)
                .collect()),
            _ => Ok(Vec::new()),
        }
    }
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)]
mod tests {
    use super::*;

    #[test]
    fn test_gathered_context_empty() {
        let ctx = GatheredContext::default();
        assert!(ctx.is_empty());
    }

    #[test]
    fn test_gathered_context_format() {
        let ctx = GatheredContext {
            memories: vec!["memory1".into(), "memory2".into()],
            code_references: vec!["src/main.rs:main".into()],
            ..Default::default()
        };
        let formatted = ctx.format_for_prompt();
        assert!(formatted.contains("RELEVANT MEMORIES"));
        assert!(formatted.contains("memory1"));
        assert!(formatted.contains("CODE REFERENCES"));
        assert!(formatted.contains("src/main.rs"));
    }

    #[test]
    fn test_gathered_context_not_empty() {
        let ctx = GatheredContext {
            memories: vec!["test".into()],
            ..Default::default()
        };
        assert!(!ctx.is_empty());
    }
}
