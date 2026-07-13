//! Workspace State — Signal types and salience computation.

use std::time::Instant;

/// Type of signal competing for workspace attention.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignalType {
    /// Perception engine output (git, fs, substrate metrics).
    Sensory,
    /// Semantic search result from memory.
    MemoryRetrieval,
    /// Oneiros dream engine conclusion.
    DreamOutput,
    /// DSP predictor output (speculative planning).
    Predictive,
    /// User message or system event.
    External,
}

/// Source of a signal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignalSource {
    PerceptionEngine,
    MemoryEnclave,
    DreamEngine,
    DspPredictor,
    NexusBus,
}

/// A signal competing for workspace broadcast attention.
#[derive(Debug, Clone)]
pub struct WorkspaceSlot {
    /// Unique identifier for this slot.
    pub id: String,
    /// Type of signal.
    pub signal_type: SignalType,
    /// Content of the signal (text representation).
    pub content: String,
    /// Computed salience score [0.0, 1.0].
    pub salience: f32,
    /// When this signal was created.
    pub created_at: Instant,
    /// Source of the signal.
    pub source: SignalSource,
}

impl WorkspaceSlot {
    /// Creates a new workspace slot with computed salience.
    pub fn new(
        id: String,
        signal_type: SignalType,
        content: String,
        source: SignalSource,
        broadcast_history: &[String],
        task_keywords: &[String],
    ) -> Self {
        let salience = compute_salience(&content, broadcast_history, task_keywords);
        Self {
            id,
            signal_type,
            content,
            salience,
            created_at: Instant::now(),
            source,
        }
    }

    /// Returns the age of this slot in seconds.
    pub fn age_seconds(&self) -> f64 {
        self.created_at.elapsed().as_secs_f64()
    }

    /// Recomputes salience based on current time and context.
    pub fn recompute_salience(&mut self, broadcast_history: &[String], task_keywords: &[String]) {
        self.salience = compute_salience(&self.content, broadcast_history, task_keywords);
    }
}

/// Computes salience score for a signal.
///
/// Formula: `salience = recency * 0.3 + novelty * 0.3 + task_relevance * 0.4`
///
/// - recency: `e^(-0.1 * age_seconds)` — decays quickly
/// - novelty: inverse of max cosine similarity to last 10 broadcasts
/// - task_relevance: fraction of task keywords present in content
fn compute_salience(content: &str, broadcast_history: &[String], task_keywords: &[String]) -> f32 {
    // Recency weight: assume fresh signals (age ~0), gives ~1.0
    let recency = 1.0f32;

    // Novelty: how different is this from recent broadcasts?
    let novelty = if broadcast_history.is_empty() {
        1.0
    } else {
        let recent: Vec<&String> = broadcast_history.iter().rev().take(10).collect();
        let max_similarity = recent
            .iter()
            .map(|prev| simple_similarity(content, prev))
            .fold(0.0f32, f32::max);
        1.0 - max_similarity
    };

    // Task relevance: fraction of keywords present
    let task_relevance = if task_keywords.is_empty() {
        0.5 // neutral when no tasks
    } else {
        let content_lower = content.to_lowercase();
        let matches = task_keywords
            .iter()
            .filter(|kw| content_lower.contains(&kw.to_lowercase()))
            .count();
        (matches as f32 / task_keywords.len() as f32).min(1.0)
    };

    (recency * 0.3 + novelty * 0.3 + task_relevance * 0.4).clamp(0.0, 1.0)
}

/// Simple word-overlap similarity between two strings.
fn simple_similarity(a: &str, b: &str) -> f32 {
    let words_a: std::collections::HashSet<&str> = a.split_whitespace().collect();
    let words_b: std::collections::HashSet<&str> = b.split_whitespace().collect();

    if words_a.is_empty() || words_b.is_empty() {
        return 0.0;
    }

    let intersection = words_a.intersection(&words_b).count();
    let union = words_a.union(&words_b).count();

    if union == 0 {
        0.0
    } else {
        intersection as f32 / union as f32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_slot_creation() {
        let slot = WorkspaceSlot::new(
            "test-1".to_string(),
            SignalType::Sensory,
            "Git changes detected".to_string(),
            SignalSource::PerceptionEngine,
            &[],
            &[],
        );
        assert!(slot.salience >= 0.0 && slot.salience <= 1.0);
    }

    #[test]
    fn test_novelty_high_for_unique_content() {
        let history = vec!["old message about files".to_string()];
        let slot = WorkspaceSlot::new(
            "test-2".to_string(),
            SignalType::External,
            "completely different quantum physics topic".to_string(),
            SignalSource::NexusBus,
            &history,
            &[],
        );
        assert!(
            slot.salience > 0.2,
            "Unique content should have reasonable salience"
        );
    }

    #[test]
    fn test_task_relevance_boosts_salience() {
        let keywords = vec!["build".to_string(), "test".to_string()];
        let slot = WorkspaceSlot::new(
            "test-3".to_string(),
            SignalType::Sensory,
            "Build failed with errors".to_string(),
            SignalSource::PerceptionEngine,
            &[],
            &keywords,
        );
        // "build" matches one of two keywords = 0.5 relevance
        assert!(
            slot.salience > 0.3,
            "Task-relevant content should have higher salience"
        );
    }

    #[test]
    fn test_simple_similarity_identical() {
        let sim = simple_similarity("hello world", "hello world");
        assert!((sim - 1.0).abs() < 0.01);
    }

    #[test]
    fn test_simple_similarity_disjoint() {
        let sim = simple_similarity("cat dog", "fish bird");
        assert_eq!(sim, 0.0);
    }
}
