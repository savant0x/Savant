//! Classification engine — Aho-Corasick trie + heuristic content probing.

use crate::compact::schema::*;
use aho_corasick::AhoCorasick;
use std::sync::{Arc, LazyLock};

/// Empty Aho-Corasick automaton used as fallback when pattern compilation fails.
#[expect(
    clippy::disallowed_methods,
    reason = "empty string pattern is a known-valid Aho-Corasick input"
)]
static EMPTY_AC: LazyLock<AhoCorasick> =
    LazyLock::new(|| AhoCorasick::new([""]).expect("empty pattern is always valid"));

/// Result of classifying a tool output against the rule registry.
#[derive(Debug, Clone)]
pub struct ClassificationResult {
    /// Matched rule (if any).
    pub matched_rule: Option<Arc<CompiledRule>>,
    /// Match score (higher = better match).
    pub score: u32,
    /// Whether the output was detected as binary.
    pub is_binary: bool,
    /// Detected output type hint from content probing.
    pub detected_hint: OutputHint,
}

/// High-speed rule matcher using Aho-Corasick automaton.
pub struct RuleMatcher {
    /// Aho-Corasick automaton for tool name matching.
    tool_ac: AhoCorasick,
    /// Pattern strings (for index -> rule mapping). Exposed for diagnostic use.
    tool_patterns: Vec<String>,
    /// Rule indices corresponding to patterns.
    tool_rule_indices: Vec<Vec<usize>>,
    /// All compiled rules.
    rules: Vec<Arc<CompiledRule>>,
}

impl RuleMatcher {
    /// Builds a new rule matcher from the registry.
    pub fn new(rules: Vec<Arc<CompiledRule>>) -> Self {
        let mut tool_patterns = Vec::new();
        let mut tool_rule_indices: Vec<Vec<usize>> = Vec::new();

        // Build a map from tool name pattern -> rule indices
        let mut pattern_to_indices: std::collections::HashMap<String, Vec<usize>> =
            std::collections::HashMap::new();

        for (idx, rule) in rules.iter().enumerate() {
            for name in &rule.rule.match_criteria.tool_names {
                pattern_to_indices
                    .entry(name.to_lowercase())
                    .or_default()
                    .push(idx);
            }
        }

        for (pattern, indices) in pattern_to_indices {
            tool_patterns.push(pattern);
            tool_rule_indices.push(indices);
        }

        let tool_ac = AhoCorasick::new(&tool_patterns).unwrap_or_else(|_| EMPTY_AC.clone());

        Self {
            tool_ac,
            tool_patterns,
            tool_rule_indices,
            rules,
        }
    }

    /// Classifies a tool output and returns the best matching rule.
    pub fn classify(&self, output: &ToolOutput) -> ClassificationResult {
        // Step 1: Check for binary content
        let is_binary = Self::detect_binary(&output.raw_output);
        if is_binary {
            return ClassificationResult {
                matched_rule: None,
                score: 0,
                is_binary: true,
                detected_hint: OutputHint::Binary,
            };
        }

        // Step 2: Probe output content for type detection
        let detected_hint = Self::probe_output_type(&output.raw_output);

        // Step 3: Aho-Corasick tool name matching
        let tool_lower = output.tool_name.to_lowercase();
        let mut best_score = 0u32;
        let mut best_rule_idx = None;

        for mat in self.tool_ac.find_iter(&tool_lower) {
            let pattern_idx = mat.pattern().as_usize();
            if let Some(indices) = self.tool_rule_indices.get(pattern_idx) {
                for &rule_idx in indices {
                    if let Some(rule) = self.rules.get(rule_idx) {
                        let score = self.score_match(rule, output);
                        if score > best_score {
                            best_score = score;
                            best_rule_idx = Some(rule_idx);
                        }
                    }
                }
            }
        }

        // Step 4: If no tool name match, try argv_includes matching
        if best_rule_idx.is_none() {
            let argv_str = output.argv.join(" ").to_lowercase();
            for (idx, rule) in self.rules.iter().enumerate() {
                let score = self.score_argv_match(rule, &argv_str, &output.raw_output);
                if score > best_score {
                    best_score = score;
                    best_rule_idx = Some(idx);
                }
            }
        }

        let matched_rule = best_rule_idx.and_then(|idx| self.rules.get(idx).cloned());

        ClassificationResult {
            matched_rule,
            score: best_score,
            is_binary: false,
            detected_hint,
        }
    }

    /// Scores a rule match based on tool name + argv + output heuristics.
    fn score_match(&self, rule: &CompiledRule, output: &ToolOutput) -> u32 {
        let mut score: u32 = 100; // Base score for tool name match

        // Argv pattern matching
        let argv_str = output.argv.join(" ").to_lowercase();
        for pattern in &rule.rule.match_criteria.argv_includes {
            if argv_str.contains(&pattern.to_lowercase()) {
                score += 40;
            }
        }
        for pattern in &rule.rule.match_criteria.argv0 {
            if output
                .argv
                .first()
                .is_some_and(|a| a.to_lowercase() == pattern.to_lowercase())
            {
                score += 60;
            }
        }

        // Output heuristic matching (first 512 bytes)
        let probe = if output.raw_output.len() > 512 {
            &output.raw_output[..512]
        } else {
            &output.raw_output
        };
        for heuristic in &rule.heuristic_regexes {
            if heuristic.is_match(probe) {
                score += 20;
            }
        }

        score
    }

    /// Scores a rule match based on argv content only (no tool name match).
    fn score_argv_match(&self, rule: &CompiledRule, argv_str: &str, output: &str) -> u32 {
        let mut score: u32 = 0;

        for pattern in &rule.rule.match_criteria.argv_includes {
            if argv_str.contains(&pattern.to_lowercase()) {
                score += 30;
            }
        }

        // Command includes matching
        for pattern in &rule.rule.match_criteria.command_includes {
            if argv_str.contains(&pattern.to_lowercase()) {
                score += 20;
            }
        }

        // Output heuristic matching
        let probe = if output.len() > 512 {
            &output[..512]
        } else {
            output
        };
        for heuristic in &rule.heuristic_regexes {
            if heuristic.is_match(probe) {
                score += 15;
            }
        }

        score
    }

    /// Detects binary content by checking for null bytes and high entropy.
    fn detect_binary(output: &str) -> bool {
        // Check for null bytes
        if output.as_bytes().contains(&0) {
            return true;
        }
        // Check entropy of first 256 bytes
        let sample = if output.len() > 256 {
            &output[..256]
        } else {
            output
        };
        Self::calculate_entropy(sample) > 7.5
    }

    /// Calculates Shannon entropy of a string.
    fn calculate_entropy(s: &str) -> f32 {
        let mut counts = [0u32; 256];
        let len = s.len() as f32;
        if len == 0.0 {
            return 0.0;
        }
        for b in s.as_bytes() {
            counts[*b as usize] += 1;
        }
        let mut entropy = 0.0f32;
        for &count in &counts {
            if count > 0 {
                let p = count as f32 / len;
                entropy -= p * p.log2();
            }
        }
        entropy
    }

    /// Probes output content to detect structured data types.
    fn probe_output_type(output: &str) -> OutputHint {
        let trimmed = output.trim_start();
        if trimmed.is_empty() {
            return OutputHint::PlainText;
        }
        // JSON detection
        if (trimmed.starts_with('{') && trimmed.contains('}'))
            || (trimmed.starts_with('[') && trimmed.contains(']'))
        {
            return OutputHint::Json;
        }
        // YAML detection
        if trimmed.starts_with("---") || (trimmed.contains(": ") && !trimmed.contains('\t')) {
            return OutputHint::Yaml;
        }
        // Table detection (multiple lines with consistent column separators)
        let lines: Vec<&str> = trimmed.lines().take(5).collect();
        if lines.len() >= 3 {
            let has_pipes = lines.iter().all(|l| l.contains('|'));
            let has_tabs = lines.iter().all(|l| l.contains('\t'));
            if has_pipes || has_tabs {
                return OutputHint::Table;
            }
        }
        OutputHint::PlainText
    }
}

impl RuleMatcher {
    /// Returns the number of registered rules.
    pub fn rule_count(&self) -> usize {
        self.rules.len()
    }

    /// Returns the compiled tool name patterns for diagnostic display.
    pub fn tool_patterns(&self) -> &[String] {
        &self.tool_patterns
    }

    /// Finds the generic/fallback rule.
    pub fn find_fallback(&self) -> Option<Arc<CompiledRule>> {
        self.rules
            .iter()
            .find(|r| r.rule.id == "generic/fallback")
            .cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_binary_detection() {
        assert!(RuleMatcher::detect_binary("hello\x00world"));
        assert!(!RuleMatcher::detect_binary("hello world"));
    }

    #[test]
    fn test_json_detection() {
        assert_eq!(
            RuleMatcher::probe_output_type("{\"key\": \"value\"}"),
            OutputHint::Json
        );
        assert_eq!(
            RuleMatcher::probe_output_type("[1, 2, 3]"),
            OutputHint::Json
        );
    }

    #[test]
    fn test_yaml_detection() {
        assert_eq!(
            RuleMatcher::probe_output_type("---\nkey: value"),
            OutputHint::Yaml
        );
    }

    #[test]
    fn test_table_detection() {
        assert_eq!(
            RuleMatcher::probe_output_type("| col1 | col2 |\n| --- | --- |\n| val1 | val2 |"),
            OutputHint::Table
        );
    }

    #[test]
    fn test_entropy_calculation() {
        let low = RuleMatcher::calculate_entropy("aaaa");
        assert_eq!(low, 0.0);
        let high = RuleMatcher::calculate_entropy("abcd");
        assert!(high > 0.0);
    }
}
