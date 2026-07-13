//! Compact engine — main orchestrator for L1 tool output compression.

use crate::compact::classify::RuleMatcher;
use crate::compact::overlay::ThreeLayerOverlay;
use crate::compact::reduce::ReductionPipeline;
use crate::compact::schema::*;
use crate::compact::semantic::SemanticDeduplicator;
use crate::compact::telemetry::CompressionEvent;
use std::path::PathBuf;
use tracing::{debug, info};

/// Main entry point for the Compact compression engine.
pub struct CompactEngine {
    /// Rule matcher (Aho-Corasick trie).
    matcher: RuleMatcher,
    /// Three-layer overlay manager.
    overlay: ThreeLayerOverlay,
    /// NS-03: Semantic deduplicator for cross-tool output similarity detection.
    /// Uses `std::sync::Mutex` for interior mutability since `compact()` takes `&self`.
    deduplicator: std::sync::Mutex<SemanticDeduplicator>,
    /// Whether compression is enabled.
    enabled: bool,
}

impl CompactEngine {
    /// Creates a new CompactEngine with rules from the given directories.
    pub fn new(user_rules_dir: PathBuf, project_rules_dir: PathBuf) -> Self {
        let overlay = ThreeLayerOverlay::new(user_rules_dir, project_rules_dir);
        let rules = overlay.all_rules();
        let matcher = RuleMatcher::new(rules);
        info!(
            "[compact] Engine initialized with {} rules",
            matcher.rule_count()
        );
        Self {
            matcher,
            overlay,
            deduplicator: std::sync::Mutex::new(SemanticDeduplicator::default()),
            enabled: true,
        }
    }

    /// Compacts a tool output using the L1 deterministic pipeline.
    pub fn compact(&self, output: &ToolOutput) -> CompactionResult {
        self.compact_with_telemetry(output).0
    }

    /// Compacts a tool output and returns both the result and a telemetry event.
    /// NA-01: Telemetry event is created here; caller is responsible for emission.
    pub fn compact_with_telemetry(
        &self,
        output: &ToolOutput,
    ) -> (CompactionResult, CompressionEvent) {
        let start = std::time::Instant::now();

        if !self.enabled {
            let result = CompactionResult::passthrough(&output.raw_output);
            let event = CompressionEvent::with_context(&result, &output.tool_name, "passthrough");
            return (result, event);
        }

        // NS-03: Check for semantic deduplication before L1 compression
        let dedup_ref = {
            let mut dedup = self.deduplicator.lock().unwrap_or_else(|e| e.into_inner());
            dedup.check_duplicate(output)
        };
        if let Some(reference) = dedup_ref {
            debug!("[compact] Semantic duplicate detected: {}", reference);
            let original_bytes = output.raw_output.len();
            let compressed_bytes = reference.len();
            let result = CompactionResult {
                output: reference,
                rule_id: "semantic_dedup".to_string(),
                original_bytes,
                compressed_bytes,
                ratio: compressed_bytes as f32 / original_bytes.max(1) as f32,
                counters: std::collections::HashMap::new(),
                was_truncated: true,
                processing_us: start.elapsed().as_micros() as u64,
            };
            let event = CompressionEvent::with_context(&result, &output.tool_name, "dedup");
            return (result, event);
        }

        // Step 1: Classify
        let classification = self.matcher.classify(output);

        // Step 2: Handle binary
        if classification.is_binary {
            debug!("[compact] Binary output detected, skipping compression");
            let result = CompactionResult {
                output: "[binary output omitted]".to_string(),
                rule_id: "binary_skip".to_string(),
                original_bytes: output.raw_output.len(),
                compressed_bytes: 22,
                ratio: 22.0 / output.raw_output.len().max(1) as f32,
                counters: std::collections::HashMap::new(),
                was_truncated: true,
                processing_us: start.elapsed().as_micros() as u64,
            };
            let event = CompressionEvent::with_context(&result, &output.tool_name, "binary");
            return (result, event);
        }

        // Step 3: Apply matched rule or fallback
        let result = match classification.matched_rule {
            Some(ref rule) => {
                debug!(
                    "[compact] Rule matched: {} (score: {})",
                    rule.rule.id, classification.score
                );
                ReductionPipeline::apply(rule.as_ref(), output)
            }
            None => {
                debug!("[compact] No rule matched, using fallback");
                if let Some(fallback) = self.matcher.find_fallback() {
                    ReductionPipeline::apply(fallback.as_ref(), output)
                } else {
                    CompactionResult::passthrough(&output.raw_output)
                }
            }
        };

        // NA-01: Create telemetry event with full tool context
        let family = classification
            .matched_rule
            .as_ref()
            .map(|r| r.rule.family.to_string())
            .unwrap_or_else(|| "unknown".to_string());
        let event = CompressionEvent::with_context(&result, &output.tool_name, &family);

        (result, event)
    }

    /// Returns the number of registered rules.
    pub fn rule_count(&self) -> usize {
        self.matcher.rule_count()
    }

    /// Returns the compiled tool name patterns for diagnostic display.
    pub fn tool_patterns(&self) -> &[String] {
        self.matcher.tool_patterns()
    }

    /// Enables or disables compression.
    pub fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
    }

    /// Reloads rules from disk (hot-reload).
    pub fn reload(&mut self) {
        self.overlay.reload();
        let rules = self.overlay.all_rules();
        self.matcher = RuleMatcher::new(rules);
        info!(
            "[compact] Rules reloaded: {} rules",
            self.matcher.rule_count()
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_output(tool: &str, args: &[&str], output: &str) -> ToolOutput {
        ToolOutput {
            tool_name: tool.to_string(),
            argv: args.iter().map(|s| s.to_string()).collect(),
            exit_code: 0,
            raw_output: output.to_string(),
            working_dir: None,
        }
    }

    #[test]
    fn test_engine_new() {
        let engine = CompactEngine::new(
            PathBuf::from("/nonexistent/user"),
            PathBuf::from("/nonexistent/project"),
        );
        assert!(engine.rule_count() >= 10);
    }

    #[test]
    fn test_compact_short_output_passthrough() {
        let engine = CompactEngine::new(
            PathBuf::from("/nonexistent/user"),
            PathBuf::from("/nonexistent/project"),
        );
        let output = make_output("echo", &["echo", "hello"], "hello");
        let result = engine.compact(&output);
        assert_eq!(result.rule_id, "passthrough");
        assert_eq!(result.output, "hello");
    }

    #[test]
    fn test_compact_git_status() {
        let engine = CompactEngine::new(
            PathBuf::from("/nonexistent/user"),
            PathBuf::from("/nonexistent/project"),
        );
        let raw = "On branch main\nChanges not staged for commit:\n  modified:   src/main.rs\n  modified:   src/lib.rs\nUntracked files:\n  new_file.rs\n";
        let output = make_output("git", &["git", "status"], raw);
        let result = engine.compact(&output);
        assert!(result.compressed_bytes <= raw.len());
    }

    #[test]
    fn test_compact_disabled() {
        let mut engine = CompactEngine::new(
            PathBuf::from("/nonexistent/user"),
            PathBuf::from("/nonexistent/project"),
        );
        engine.set_enabled(false);
        let output = make_output(
            "git",
            &["git", "status"],
            "some long output that would normally be compressed",
        );
        let result = engine.compact(&output);
        assert_eq!(result.rule_id, "passthrough");
    }
}
