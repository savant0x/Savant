//! REM Phase — Adversarial Latent Space Exploration.
//!
//! Explores underutilized regions of the memory vector space,
//! performs cross-domain concept recombination, and generates
//! novel associations through constrained adversarial exploration.

use std::sync::Arc;
use std::time::Instant;

use rand::seq::SliceRandom;
use savant_memory::MemoryEngine;
use tracing::{debug, info, warn};

use super::vendi;

/// Result of a REM exploration cycle.
#[derive(Debug, Clone)]
pub struct RemResult {
    /// Novel associations generated.
    pub associations: Vec<DreamAssociation>,
    /// Vendi Score of outputs (diversity metric).
    pub vendi_score: f32,
    /// Whether outputs passed diversity threshold.
    pub passed_filter: bool,
    /// Duration in milliseconds.
    pub duration_ms: u64,
}

/// A novel association discovered during REM exploration.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DreamAssociation {
    /// Unique identifier for this association.
    pub id: String,
    /// Source concept cluster A.
    pub source_a: String,
    /// Source concept cluster B.
    pub source_b: String,
    /// Novel synthesis from cross-domain recombination.
    pub synthesis: String,
    /// Confidence in this association (0.0 - 1.0).
    pub confidence: f32,
    /// Tags for categorization.
    pub tags: Vec<String>,
}

/// REM controller for adversarial latent space exploration.
pub struct RemController {
    /// Number of concept clusters to sample for recombination.
    pub cluster_sample_count: usize,
    /// Number of associations to generate per cycle.
    pub max_associations: usize,
    /// Embedding vector dimension for random probes.
    pub embedding_dimension: usize,
}

impl RemController {
    /// Creates a new REM controller with the given parameters.
    pub fn new(
        cluster_sample_count: usize,
        max_associations: usize,
        embedding_dimension: usize,
    ) -> Self {
        Self {
            cluster_sample_count,
            max_associations,
            embedding_dimension,
        }
    }

    /// Creates a default REM controller (embedding dimension 768).
    pub fn default_controller() -> Self {
        Self::new(4, 6, 768)
    }

    /// Runs the REM exploration cycle.
    ///
    /// # Process
    /// 1. Query underutilized vector space regions via semantic search with low-similarity probes
    /// 2. Sample concept clusters for cross-domain recombination
    /// 3. Generate novel associations by blending concepts from distinct clusters
    /// 4. Evaluate diversity with Vendi Score
    pub async fn run(
        &self,
        memory: &Arc<MemoryEngine>,
        vendi_threshold: f32,
    ) -> Result<RemResult, super::DreamError> {
        let start = Instant::now();
        info!("[REM] Starting exploration cycle");

        // Phase 1: Discover concept clusters from existing memory
        let clusters =
            discover_concept_clusters(memory, self.cluster_sample_count, self.embedding_dimension)
                .await;

        if clusters.len() < 2 {
            debug!(
                "[REM] Insufficient clusters ({}) for recombination",
                clusters.len()
            );
            return Ok(RemResult {
                associations: vec![],
                vendi_score: 0.0,
                passed_filter: false,
                duration_ms: start.elapsed().as_millis() as u64,
            });
        }

        // Phase 2: Generate novel associations via cross-domain recombination
        let mut associations = Vec::with_capacity(self.max_associations);
        let mut rng = rand::thread_rng();

        for _ in 0..self.max_associations {
            // Select two distinct clusters
            let cluster_pair: Vec<_> = clusters.choose_multiple(&mut rng, 2).collect();

            if cluster_pair.len() < 2 {
                break;
            }

            let a = cluster_pair[0];
            let b = cluster_pair[1];

            // Generate cross-domain association
            let association = DreamAssociation {
                id: uuid::Uuid::new_v4().to_string(),
                source_a: a.label.clone(),
                source_b: b.label.clone(),
                synthesis: format!(
                    "Cross-domain synthesis: [{}] x [{}] — exploring conceptual boundary between {} and {}",
                    a.label, b.label, a.representative_content, b.representative_content
                ),
                confidence: compute_association_confidence(a, b),
                tags: vec![
                    "rem_dream".to_string(),
                    "cross_domain".to_string(),
                    format!("{}__{}", a.label, b.label),
                ],
            };

            associations.push(association);
        }

        // Phase 3: Evaluate diversity with Vendi Score
        let texts: Vec<String> = associations.iter().map(|a| a.synthesis.clone()).collect();
        let score = vendi::vendi_score_from_text(&texts);
        let passed = score >= vendi_threshold;

        let duration_ms = start.elapsed().as_millis() as u64;

        if !passed {
            warn!(
                "[REM] Vendi Score {:.2} below threshold {:.2} — outputs pruned",
                score, vendi_threshold
            );
        }

        info!(
            "[REM] Complete: {} associations, Vendi Score {:.2}, passed={} ({}ms)",
            associations.len(),
            score,
            passed,
            duration_ms
        );

        Ok(RemResult {
            associations,
            vendi_score: score,
            passed_filter: passed,
            duration_ms,
        })
    }
}

/// A concept cluster discovered from memory.
#[derive(Debug, Clone)]
struct ConceptCluster {
    label: String,
    representative_content: String,
    member_count: usize,
}

/// Discovers concept clusters from existing memory entries.
///
/// Uses semantic search with random probe vectors to find
/// underutilized regions of the vector space.
async fn discover_concept_clusters(
    memory: &Arc<MemoryEngine>,
    count: usize,
    dimension: usize,
) -> Vec<ConceptCluster> {
    let mut clusters = Vec::with_capacity(count);

    for i in 0..count {
        // Generate a random probe vector to explore different regions
        let probe: Vec<f32> = (0..dimension)
            .map(|_| rand::random::<f32>() * 2.0 - 1.0)
            .collect();

        match memory.enclave().semantic_search(&probe, 5) {
            Ok(results) => {
                if results.is_empty() {
                    continue;
                }

                let representative = results[0].document_id.clone();
                clusters.push(ConceptCluster {
                    label: format!("cluster_{}", i),
                    representative_content: format!(
                        "{} memories in vector region {}",
                        results.len(),
                        representative
                    ),
                    member_count: results.len(),
                });
            }
            Err(e) => {
                debug!("[REM] Semantic search failed for probe {}: {}", i, e);
            }
        }
    }

    clusters
}

/// Computes confidence in a cross-domain association.
/// Based on cluster size difference (smaller difference = higher confidence).
fn compute_association_confidence(a: &ConceptCluster, b: &ConceptCluster) -> f32 {
    let size_diff = (a.member_count as f32 - b.member_count as f32).abs();
    let max_size = a.member_count.max(b.member_count) as f32;
    let size_similarity = 1.0 - (size_diff / max_size.max(1.0));
    size_similarity.clamp(0.1, 0.9)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rem_controller_default() {
        let controller = RemController::default_controller();
        assert_eq!(controller.cluster_sample_count, 4);
        assert_eq!(controller.max_associations, 6);
        assert_eq!(controller.embedding_dimension, 768);
    }

    #[test]
    fn test_association_confidence_equal_size() {
        let a = ConceptCluster {
            label: "a".to_string(),
            representative_content: "test".to_string(),
            member_count: 5,
        };
        let b = ConceptCluster {
            label: "b".to_string(),
            representative_content: "test".to_string(),
            member_count: 5,
        };
        let conf = compute_association_confidence(&a, &b);
        assert!(
            (conf - 0.9).abs() < 0.01,
            "Equal size clusters should have confidence ~0.9 (clamped), got {}",
            conf
        );
    }

    #[test]
    fn test_association_confidence_different_size() {
        let a = ConceptCluster {
            label: "a".to_string(),
            representative_content: "test".to_string(),
            member_count: 1,
        };
        let b = ConceptCluster {
            label: "b".to_string(),
            representative_content: "test".to_string(),
            member_count: 10,
        };
        let conf = compute_association_confidence(&a, &b);
        assert!(
            conf < 0.5,
            "Very different size clusters should have lower confidence"
        );
    }
}
