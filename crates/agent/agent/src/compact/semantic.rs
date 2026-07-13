//! Semantic deduplication (L1.5) — cross-tool output similarity detection.

use crate::compact::schema::ToolOutput;
use std::collections::HashMap;

/// Lightweight semantic deduplicator using content hashing and trigram similarity.
///
/// Before applying L1 compression, checks if the tool output is semantically
/// identical or highly similar to a recent tool output already in context.
/// If so, replaces the output with a compact reference pointer.
#[derive(Debug, Clone)]
pub struct SemanticDeduplicator {
    /// Recent output hashes (content hash -> (tool name, timestamp, content)).
    recent_hashes: HashMap<u64, (String, i64, String)>,
    /// Maximum number of recent hashes to track.
    max_entries: usize,
    /// Similarity threshold (0.0-1.0). Used for fuzzy deduplication.
    threshold: f32,
}

impl SemanticDeduplicator {
    /// Creates a new deduplicator with the given capacity.
    pub fn new(max_entries: usize) -> Self {
        Self {
            recent_hashes: HashMap::with_capacity(max_entries),
            max_entries,
            threshold: 0.95,
        }
    }

    /// Checks if the output is a duplicate of a recent tool output.
    /// Returns Some(reference_string) if duplicate, None otherwise.
    /// Uses both exact hash match and fuzzy similarity (threshold-based).
    pub fn check_duplicate(&mut self, output: &ToolOutput) -> Option<String> {
        let hash = Self::compute_hash(&output.raw_output);

        // Check for exact match first
        if let Some((tool_name, _, _)) = self.recent_hashes.get(&hash) {
            return Some(format!(
                "[CompactRef: {} output identical to recent]",
                tool_name
            ));
        }

        // Check for fuzzy match using trigram similarity
        for (tool_name, _, content) in self.recent_hashes.values() {
            let similarity = Self::compute_similarity(&output.raw_output, content);
            if similarity >= self.threshold {
                return Some(format!(
                    "[CompactRef: {} output {:.0}% similar to recent]",
                    tool_name,
                    similarity * 100.0
                ));
            }
        }

        // Store hash with content for future fuzzy matching
        if self.recent_hashes.len() >= self.max_entries {
            // Evict oldest (simple: clear half)
            let keys_to_remove: Vec<u64> = self
                .recent_hashes
                .keys()
                .take(self.max_entries / 2)
                .copied()
                .collect();
            for k in keys_to_remove {
                self.recent_hashes.remove(&k);
            }
        }

        self.recent_hashes.insert(
            hash,
            (
                output.tool_name.clone(),
                chrono::Utc::now().timestamp(),
                output.raw_output.clone(),
            ),
        );

        None
    }

    /// Computes a fast hash of the output content.
    fn compute_hash(content: &str) -> u64 {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        let mut hasher = DefaultHasher::new();
        content.hash(&mut hasher);
        hasher.finish()
    }

    /// Computes n-gram similarity between two strings.
    /// Returns a value between 0.0 (completely different) and 1.0 (identical).
    fn compute_similarity(a: &str, b: &str) -> f32 {
        if a == b {
            return 1.0;
        }
        if a.is_empty() || b.is_empty() {
            return 0.0;
        }

        // Use 3-gram (trigram) similarity
        let n = 3;
        let a_ngrams: Vec<&str> = a
            .as_bytes()
            .windows(n)
            .map(|w| std::str::from_utf8(w).unwrap_or(""))
            .collect();
        let b_ngrams: Vec<&str> = b
            .as_bytes()
            .windows(n)
            .map(|w| std::str::from_utf8(w).unwrap_or(""))
            .collect();

        if a_ngrams.is_empty() || b_ngrams.is_empty() {
            return 0.0;
        }

        let mut matches = 0;
        let mut b_used = vec![false; b_ngrams.len()];

        for a_ng in &a_ngrams {
            for (j, b_ng) in b_ngrams.iter().enumerate() {
                if !b_used[j] && a_ng == b_ng {
                    matches += 1;
                    b_used[j] = true;
                    break;
                }
            }
        }

        (2 * matches) as f32 / (a_ngrams.len() + b_ngrams.len()) as f32
    }

    /// Clears all tracked hashes.
    pub fn clear(&mut self) {
        self.recent_hashes.clear();
    }
}

impl Default for SemanticDeduplicator {
    fn default() -> Self {
        Self::new(256)
    }
}

#[cfg(test)]
#[expect(clippy::disallowed_methods)]
mod tests {
    use super::*;

    #[test]
    fn test_exact_duplicate_detection() {
        let mut dedup = SemanticDeduplicator::new(100);
        let output1 = ToolOutput {
            tool_name: "git".to_string(),
            argv: vec!["git".to_string(), "status".to_string()],
            exit_code: 0,
            raw_output: "On branch main\nnothing to commit".to_string(),
            working_dir: None,
        };
        let output2 = ToolOutput {
            tool_name: "git".to_string(),
            argv: vec!["git".to_string(), "status".to_string()],
            exit_code: 0,
            raw_output: "On branch main\nnothing to commit".to_string(),
            working_dir: None,
        };

        assert!(dedup.check_duplicate(&output1).is_none());
        let result = dedup.check_duplicate(&output2);
        assert!(result.is_some());
        assert!(result.unwrap().contains("CompactRef"));
    }

    #[test]
    fn test_different_outputs_not_deduped() {
        let mut dedup = SemanticDeduplicator::new(100);
        let output1 = ToolOutput {
            tool_name: "git".to_string(),
            argv: vec!["git".to_string(), "status".to_string()],
            exit_code: 0,
            raw_output: "On branch main".to_string(),
            working_dir: None,
        };
        let output2 = ToolOutput {
            tool_name: "git".to_string(),
            argv: vec!["git".to_string(), "status".to_string()],
            exit_code: 0,
            raw_output: "On branch develop".to_string(),
            working_dir: None,
        };

        assert!(dedup.check_duplicate(&output1).is_none());
        assert!(dedup.check_duplicate(&output2).is_none());
    }
}
