//! Lessons & Insights Synthesis (MEM-12)
//!
//! Higher-order knowledge synthesis from the MAGMA graph.
//! Lessons are learned from repeated experiences. Insights are synthesized
//! from concept cluster analysis.
//!
//! Both are stored as MAGMA Entity graph nodes and participate in
//! the reflective consolidation pipeline.

use serde::{Deserialize, Serialize};

/// A lesson learned from repeated experiences.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Lesson {
    /// Unique lesson ID.
    pub id: u64,
    /// The lesson content.
    pub content: String,
    /// Confidence in this lesson (0.0 - 1.0).
    pub confidence: f32,
    /// Decay rate — how quickly this lesson fades without reinforcement.
    pub decay_rate: f32,
    /// Number of times this lesson has been reinforced.
    pub reinforcements: u32,
    /// Source memory IDs that contributed to this lesson.
    pub source_memories: Vec<u64>,
    /// Category (e.g., "debugging", "architecture", "workflow").
    pub category: String,
    /// Creation timestamp.
    pub created_at: i64,
    /// Last reinforced timestamp.
    pub last_reinforced_at: i64,
}

/// An insight synthesized from concept cluster analysis.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Insight {
    /// Unique insight ID.
    pub id: u64,
    /// Insight title.
    pub title: String,
    /// Detailed insight content.
    pub content: String,
    /// Confidence in this insight (0.0 - 1.0).
    pub confidence: f32,
    /// Source concept cluster (MAGMA node IDs).
    pub source_concept_cluster: Vec<u64>,
    /// Category.
    pub category: String,
    /// Creation timestamp.
    pub created_at: i64,
}

/// Lesson synthesizer that extracts lessons from recurring memory patterns.
pub struct LessonSynthesizer {
    /// Minimum reinforcements before a pattern becomes a lesson.
    pub min_reinforcements: u32,
    /// Minimum confidence to keep a lesson.
    pub min_confidence: f32,
}

impl LessonSynthesizer {
    pub fn new(min_reinforcements: u32, min_confidence: f32) -> Self {
        Self {
            min_reinforcements,
            min_confidence,
        }
    }

    /// Attempts to synthesize a lesson from a set of related memories.
    ///
    /// Returns a Lesson if the memories show a consistent pattern.
    pub fn synthesize(
        &self,
        memories: &[(u64, String, f32)], // (id, content, importance)
        category: &str,
    ) -> Option<Lesson> {
        if memories.len() < self.min_reinforcements as usize {
            return None;
        }

        // Find common keywords across memories
        let all_words: Vec<Vec<&str>> = memories
            .iter()
            .map(|(_, content, _)| content.split_whitespace().filter(|w| w.len() > 3).collect())
            .collect();

        // Count word frequencies
        let mut word_counts: std::collections::HashMap<&str, u32> =
            std::collections::HashMap::new();
        for words in &all_words {
            for word in words {
                *word_counts.entry(word).or_insert(0) += 1;
            }
        }

        // Find words that appear in most memories
        let threshold = (memories.len() as f32 * 0.6) as u32;
        let common_words: Vec<&str> = word_counts
            .iter()
            .filter(|(_, count)| **count >= threshold)
            .map(|(word, _)| *word)
            .collect();

        if common_words.is_empty() {
            return None;
        }

        // Synthesize lesson from common themes
        let avg_importance: f32 =
            memories.iter().map(|(_, _, imp)| imp).sum::<f32>() / memories.len() as f32;
        let confidence = (avg_importance / 10.0).min(1.0);

        if confidence < self.min_confidence {
            return None;
        }

        let content = format!(
            "Lesson from {} related memories: common themes include {}",
            memories.len(),
            common_words.join(", ")
        );

        let now = chrono::Utc::now().timestamp();
        let id = {
            let mut hasher = blake3::Hasher::new();
            hasher.update(content.as_bytes());
            let hash = hasher.finalize();
            u64::from_le_bytes(hash.as_bytes()[..8].try_into().unwrap_or([0u8; 8]))
        };

        Some(Lesson {
            id,
            content,
            confidence,
            decay_rate: 0.05,
            reinforcements: memories.len() as u32,
            source_memories: memories.iter().map(|(id, _, _)| *id).collect(),
            category: category.to_string(),
            created_at: now,
            last_reinforced_at: now,
        })
    }

    /// Reinforces a lesson by incrementing its reinforcement count.
    pub fn reinforce(&self, lesson: &mut Lesson) {
        lesson.reinforcements += 1;
        lesson.last_reinforced_at = chrono::Utc::now().timestamp();
        // Boost confidence on reinforcement (diminishing returns)
        lesson.confidence = (lesson.confidence + 0.05 * (1.0 - lesson.confidence)).min(1.0);
    }

    /// Computes the current strength of a lesson considering decay.
    pub fn current_strength(&self, lesson: &Lesson, now: i64) -> f32 {
        let days_since_reinforced = (now - lesson.last_reinforced_at).max(0) as f32 / 86400.0;
        let decay = (-lesson.decay_rate * days_since_reinforced).exp();
        lesson.confidence * decay
    }
}

impl Default for LessonSynthesizer {
    fn default() -> Self {
        Self::new(3, 0.5)
    }
}

/// Insight synthesizer that generates insights from concept clusters.
pub struct InsightSynthesizer {
    /// Minimum cluster size to generate an insight.
    pub min_cluster_size: usize,
}

impl InsightSynthesizer {
    pub fn new(min_cluster_size: usize) -> Self {
        Self { min_cluster_size }
    }

    /// Synthesizes an insight from a cluster of related concepts.
    pub fn synthesize(
        &self,
        cluster: &[(u64, String)], // (concept_id, concept_label)
        category: &str,
    ) -> Option<Insight> {
        if cluster.len() < self.min_cluster_size {
            return None;
        }

        let labels: Vec<&str> = cluster.iter().map(|(_, l)| l.as_str()).collect();
        let title = format!("Insight: {} related concepts", cluster.len());
        let content = format!(
            "Analysis of {} related concepts reveals a cluster around: {}",
            cluster.len(),
            labels.join(", ")
        );

        let now = chrono::Utc::now().timestamp();
        let id = {
            let mut hasher = blake3::Hasher::new();
            for (_, label) in cluster {
                hasher.update(label.as_bytes());
            }
            let hash = hasher.finalize();
            u64::from_le_bytes(hash.as_bytes()[..8].try_into().unwrap_or([0u8; 8]))
        };

        Some(Insight {
            id,
            title,
            content,
            confidence: (cluster.len() as f32 / 20.0).min(1.0),
            source_concept_cluster: cluster.iter().map(|(id, _)| *id).collect(),
            category: category.to_string(),
            created_at: now,
        })
    }
}

impl Default for InsightSynthesizer {
    fn default() -> Self {
        Self::new(3)
    }
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)]
mod tests {
    use super::*;

    #[test]
    fn test_synthesize_lesson() {
        let synthesizer = LessonSynthesizer::default();
        let memories = vec![
            (
                1u64,
                "always validate input before processing".to_string(),
                8.0f32,
            ),
            (
                2,
                "validate input to prevent injection attacks".to_string(),
                9.0,
            ),
            (
                3,
                "input validation is critical for security".to_string(),
                7.0,
            ),
        ];
        let lesson = synthesizer.synthesize(&memories, "security");
        assert!(lesson.is_some());
        let lesson = lesson.unwrap();
        assert!(lesson.confidence > 0.5);
        assert_eq!(lesson.reinforcements, 3);
    }

    #[test]
    fn test_synthesize_lesson_too_few() {
        let synthesizer = LessonSynthesizer::new(5, 0.5);
        let memories = vec![
            (1u64, "test".to_string(), 5.0f32),
            (2, "test".to_string(), 5.0),
        ];
        let lesson = synthesizer.synthesize(&memories, "test");
        assert!(lesson.is_none());
    }

    #[test]
    fn test_reinforce_lesson() {
        let synthesizer = LessonSynthesizer::default();
        let mut lesson = Lesson {
            id: 1,
            content: "test".to_string(),
            confidence: 0.6,
            decay_rate: 0.05,
            reinforcements: 3,
            source_memories: vec![],
            category: "test".to_string(),
            created_at: 0,
            last_reinforced_at: 0,
        };
        synthesizer.reinforce(&mut lesson);
        assert_eq!(lesson.reinforcements, 4);
        assert!(lesson.confidence > 0.6);
    }

    #[test]
    fn test_lesson_strength_decay() {
        let synthesizer = LessonSynthesizer::default();
        let now = 1700000000;
        let lesson = Lesson {
            id: 1,
            content: "test".to_string(),
            confidence: 1.0,
            decay_rate: 0.1,
            reinforcements: 5,
            source_memories: vec![],
            category: "test".to_string(),
            created_at: now,
            last_reinforced_at: now - 86400 * 30, // 30 days ago
        };
        let strength = synthesizer.current_strength(&lesson, now);
        assert!(strength < 1.0); // decayed
        assert!(strength > 0.0); // not dead
    }

    #[test]
    fn test_synthesize_insight() {
        let synthesizer = InsightSynthesizer::default();
        let cluster = vec![
            (1, "authentication".to_string()),
            (2, "authorization".to_string()),
            (3, "access control".to_string()),
        ];
        let insight = synthesizer.synthesize(&cluster, "security");
        assert!(insight.is_some());
    }

    #[test]
    fn test_synthesize_insight_too_small() {
        let synthesizer = InsightSynthesizer::new(5);
        let cluster = vec![(1, "a".to_string())];
        let insight = synthesizer.synthesize(&cluster, "test");
        assert!(insight.is_none());
    }
}
