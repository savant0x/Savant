//! Procedural Memory Layer (MEM-11)
//!
//! Extracts recurring tool-call patterns across sessions as procedures.
//! A procedure is a named sequence of steps that the agent has learned
//! through repeated successful execution.
//!
//! Procedures are stored as MAGMA Causal graph nodes and can be
//! retrieved by trigger condition matching.

use serde::{Deserialize, Serialize};

/// A learned procedure extracted from recurring patterns.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProceduralMemory {
    /// Unique procedure ID.
    pub id: u64,
    /// Human-readable procedure name.
    pub name: String,
    /// Ordered steps (tool calls or descriptions).
    pub steps: Vec<ProcedureStep>,
    /// Condition that triggers this procedure.
    pub trigger_condition: String,
    /// How many times this pattern has been observed.
    pub frequency: u32,
    /// Confidence in this procedure (0.0 - 1.0).
    pub strength: f32,
    /// Tags for categorization.
    pub tags: Vec<String>,
    /// Session IDs where this pattern was observed.
    pub source_sessions: Vec<String>,
    /// Creation timestamp.
    pub created_at: i64,
    /// Last observed timestamp.
    pub last_observed_at: i64,
}

/// A single step in a procedure.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcedureStep {
    /// Step index (0-based).
    pub index: u32,
    /// Tool name or action description.
    pub action: String,
    /// Expected input pattern (optional).
    pub input_pattern: Option<String>,
    /// Expected output pattern (optional).
    pub output_pattern: Option<String>,
    /// Whether this step is critical (failure aborts procedure).
    pub critical: bool,
}

/// Pattern extractor that identifies recurring tool-call sequences.
pub struct PatternExtractor {
    /// Minimum frequency to consider a pattern significant.
    pub min_frequency: u32,
    /// Minimum sequence length to consider.
    pub min_sequence_length: usize,
    /// Maximum sequence length to consider.
    pub max_sequence_length: usize,
}

impl PatternExtractor {
    pub fn new(min_frequency: u32) -> Self {
        Self {
            min_frequency,
            min_sequence_length: 2,
            max_sequence_length: 10,
        }
    }

    /// Extracts recurring tool-call patterns from a sequence of tool calls.
    ///
    /// Returns patterns that appear at least `min_frequency` times.
    pub fn extract_patterns(
        &self,
        tool_calls: &[(String, String)], // (tool_name, session_id)
    ) -> Vec<(Vec<String>, u32)> {
        if tool_calls.len() < self.min_sequence_length {
            return Vec::new();
        }

        // Extract tool names only
        let tools: Vec<String> = tool_calls.iter().map(|(t, _)| t.clone()).collect();

        // Find all subsequences of length min_sequence_length..=max_sequence_length
        let mut pattern_counts: std::collections::HashMap<Vec<String>, u32> =
            std::collections::HashMap::new();

        for len in self.min_sequence_length..=self.max_sequence_length.min(tools.len()) {
            for window in tools.windows(len) {
                let pattern = window.to_vec();
                *pattern_counts.entry(pattern).or_insert(0) += 1;
            }
        }

        // Filter by minimum frequency
        let mut patterns: Vec<(Vec<String>, u32)> = pattern_counts
            .into_iter()
            .filter(|(_, count)| *count >= self.min_frequency)
            .collect();

        // Sort by frequency descending
        patterns.sort_by(|a, b| b.1.cmp(&a.1));
        patterns
    }

    /// Creates a ProceduralMemory from a detected pattern.
    pub fn create_procedure(
        &self,
        pattern: &[String],
        frequency: u32,
        trigger: &str,
        session_ids: &[String],
    ) -> ProceduralMemory {
        let now = chrono::Utc::now().timestamp();
        let id = {
            let mut hasher = blake3::Hasher::new();
            for step in pattern {
                hasher.update(step.as_bytes());
            }
            let hash = hasher.finalize();
            u64::from_le_bytes(hash.as_bytes()[..8].try_into().unwrap_or([0u8; 8]))
        };

        ProceduralMemory {
            id,
            name: format!("Procedure: {}", pattern.join(" -> ")),
            steps: pattern
                .iter()
                .enumerate()
                .map(|(i, action)| ProcedureStep {
                    index: i as u32,
                    action: action.clone(),
                    input_pattern: None,
                    output_pattern: None,
                    critical: false,
                })
                .collect(),
            trigger_condition: trigger.to_string(),
            frequency,
            strength: (frequency as f32 / 10.0).min(1.0),
            tags: vec!["auto-extracted".to_string()],
            source_sessions: session_ids.to_vec(),
            created_at: now,
            last_observed_at: now,
        }
    }
}

impl Default for PatternExtractor {
    fn default() -> Self {
        Self::new(3)
    }
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_basic_pattern() {
        let extractor = PatternExtractor::new(2);
        let calls = vec![
            ("read_file".to_string(), "s1".to_string()),
            ("parse_json".to_string(), "s1".to_string()),
            ("read_file".to_string(), "s2".to_string()),
            ("parse_json".to_string(), "s2".to_string()),
        ];
        let patterns = extractor.extract_patterns(&calls);
        assert!(!patterns.is_empty());
        assert!(patterns
            .iter()
            .any(|(p, c)| p == &["read_file", "parse_json"] && *c >= 2));
    }

    #[test]
    fn test_no_pattern_below_threshold() {
        let extractor = PatternExtractor::new(5);
        let calls = vec![
            ("a".to_string(), "s1".to_string()),
            ("b".to_string(), "s1".to_string()),
        ];
        let patterns = extractor.extract_patterns(&calls);
        assert!(patterns.is_empty());
    }

    #[test]
    fn test_create_procedure() {
        let extractor = PatternExtractor::new(1);
        let proc = extractor.create_procedure(
            &["read_file".to_string(), "parse_json".to_string()],
            5,
            "user asks to read a JSON file",
            &["s1".to_string(), "s2".to_string()],
        );
        assert_eq!(proc.steps.len(), 2);
        assert_eq!(proc.frequency, 5);
        assert!(proc.strength > 0.0);
    }

    #[test]
    fn test_pattern_too_short() {
        let extractor = PatternExtractor::new(1);
        let calls = vec![("a".to_string(), "s1".to_string())];
        let patterns = extractor.extract_patterns(&calls);
        assert!(patterns.is_empty());
    }
}
