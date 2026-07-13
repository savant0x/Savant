//! Triple-Stream RRF Search Fusion (MEM-05)
//!
//! Fuses results from BM25 keyword search, HNSW semantic search, and
//! MAGMA graph traversal using Reciprocal Rank Fusion (RRF).
//!
//! RRF formula: score = Σ 1 / (k + rank_i) where k=60 (standard constant).

use std::collections::HashMap;

/// RRF fusion constant. Higher values reduce the impact of top-ranked results.
const RRF_K: f32 = 60.0;

/// Default weights for each search stream.
const DEFAULT_BM25_WEIGHT: f32 = 0.4;
const DEFAULT_VECTOR_WEIGHT: f32 = 0.6;
const DEFAULT_GRAPH_WEIGHT: f32 = 0.3;

/// Maximum results from a single session (diversification).
const DEFAULT_SESSION_DIVERSITY_LIMIT: usize = 3;

/// Result from a single search stream.
#[derive(Debug, Clone)]
pub struct StreamResult {
    /// Document/memory ID.
    pub doc_id: u64,
    /// Score from this stream (higher is better).
    pub score: f32,
    /// Session ID for diversification.
    pub session_id: String,
}

/// Configuration for RRF fusion.
#[derive(Debug, Clone)]
pub struct RrfConfig {
    /// Weight for BM25 stream.
    pub bm25_weight: f32,
    /// Weight for vector/HNSW stream.
    pub vector_weight: f32,
    /// Weight for MAGMA graph stream.
    pub graph_weight: f32,
    /// Maximum results per session (diversification).
    pub session_diversity_limit: usize,
}

impl Default for RrfConfig {
    fn default() -> Self {
        Self {
            bm25_weight: DEFAULT_BM25_WEIGHT,
            vector_weight: DEFAULT_VECTOR_WEIGHT,
            graph_weight: DEFAULT_GRAPH_WEIGHT,
            session_diversity_limit: DEFAULT_SESSION_DIVERSITY_LIMIT,
        }
    }
}

/// Fuses results from multiple search streams using Reciprocal Rank Fusion.
///
/// Each stream provides a ranked list of (doc_id, score) pairs.
/// The fusion produces a single ranked list where documents appearing
/// in multiple streams are boosted.
///
/// If a stream is empty, its weight is redistributed to the other streams.
pub fn fuse_results(
    bm25_results: &[StreamResult],
    vector_results: &[StreamResult],
    graph_results: &[StreamResult],
    config: &RrfConfig,
    limit: usize,
) -> Vec<(u64, f32)> {
    // Normalize weights — redistribute weight from empty streams
    let has_bm25 = !bm25_results.is_empty();
    let has_vector = !vector_results.is_empty();
    let has_graph = !graph_results.is_empty();

    let total_weight = (if has_bm25 { config.bm25_weight } else { 0.0 })
        + (if has_vector {
            config.vector_weight
        } else {
            0.0
        })
        + (if has_graph { config.graph_weight } else { 0.0 });

    if total_weight == 0.0 {
        return Vec::new();
    }

    let bm25_w = if has_bm25 {
        config.bm25_weight / total_weight
    } else {
        0.0
    };
    let vector_w = if has_vector {
        config.vector_weight / total_weight
    } else {
        0.0
    };
    let graph_w = if has_graph {
        config.graph_weight / total_weight
    } else {
        0.0
    };

    // Compute RRF scores: Σ weight / (k + rank)
    let mut scores: HashMap<u64, f32> = HashMap::new();
    let mut sessions: HashMap<u64, String> = HashMap::new();

    for (rank, result) in bm25_results.iter().enumerate() {
        let rrf_score = bm25_w / (RRF_K + rank as f32);
        *scores.entry(result.doc_id).or_insert(0.0) += rrf_score;
        sessions
            .entry(result.doc_id)
            .or_insert_with(|| result.session_id.clone());
    }

    for (rank, result) in vector_results.iter().enumerate() {
        let rrf_score = vector_w / (RRF_K + rank as f32);
        *scores.entry(result.doc_id).or_insert(0.0) += rrf_score;
        sessions
            .entry(result.doc_id)
            .or_insert_with(|| result.session_id.clone());
    }

    for (rank, result) in graph_results.iter().enumerate() {
        let rrf_score = graph_w / (RRF_K + rank as f32);
        *scores.entry(result.doc_id).or_insert(0.0) += rrf_score;
        sessions
            .entry(result.doc_id)
            .or_insert_with(|| result.session_id.clone());
    }

    // Sort by RRF score descending
    let mut results: Vec<(u64, f32)> = scores.into_iter().collect();
    results.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

    // Apply session diversity: limit results per session
    let mut session_counts: HashMap<String, usize> = HashMap::new();
    let mut diversified = Vec::new();

    for (doc_id, score) in results {
        let session = sessions.get(&doc_id).cloned().unwrap_or_default();
        let count = session_counts.entry(session).or_insert(0);
        if *count < config.session_diversity_limit {
            diversified.push((doc_id, score));
            *count += 1;
        }
        if diversified.len() >= limit {
            break;
        }
    }

    diversified
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)]
mod tests {
    use super::*;

    fn make_result(doc_id: u64, score: f32, session: &str) -> StreamResult {
        StreamResult {
            doc_id,
            score,
            session_id: session.to_string(),
        }
    }

    #[test]
    fn test_rrf_single_stream() {
        let bm25 = vec![make_result(1, 10.0, "s1"), make_result(2, 5.0, "s1")];
        let results = fuse_results(&bm25, &[], &[], &RrfConfig::default(), 10);
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].0, 1); // doc 1 ranked first
    }

    #[test]
    fn test_rrf_multi_stream_boost() {
        // Doc 1 appears in both BM25 and vector — should be boosted
        let bm25 = vec![make_result(1, 10.0, "s1"), make_result(2, 5.0, "s1")];
        let vector = vec![make_result(1, 0.9, "s1"), make_result(3, 0.8, "s2")];
        let results = fuse_results(&bm25, &vector, &[], &RrfConfig::default(), 10);

        assert_eq!(results[0].0, 1); // doc 1 should be top (appears in both)
    }

    #[test]
    fn test_rrf_empty_stream_redistribution() {
        // Only BM25 has results — vector weight should be redistributed
        let bm25 = vec![make_result(1, 10.0, "s1")];
        let results = fuse_results(&bm25, &[], &[], &RrfConfig::default(), 10);
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn test_rrf_all_empty() {
        let results = fuse_results(&[], &[], &[], &RrfConfig::default(), 10);
        assert!(results.is_empty());
    }

    #[test]
    fn test_rrf_session_diversity() {
        let bm25 = vec![
            make_result(1, 10.0, "s1"),
            make_result(2, 9.0, "s1"),
            make_result(3, 8.0, "s1"),
            make_result(4, 7.0, "s1"),
            make_result(5, 6.0, "s2"),
        ];
        let config = RrfConfig {
            session_diversity_limit: 2,
            ..Default::default()
        };
        let results = fuse_results(&bm25, &[], &[], &config, 10);

        // Only 2 from s1 + 1 from s2 = 3 total
        let s1_count = results.iter().filter(|(_, _)| true).count();
        assert!(s1_count <= 3);
    }

    #[test]
    fn test_rrf_limit() {
        let bm25: Vec<StreamResult> = (0..100)
            .map(|i| make_result(i, 100.0 - i as f32, "s1"))
            .collect();
        let config = RrfConfig {
            session_diversity_limit: 100,
            ..Default::default()
        };
        let results = fuse_results(&bm25, &[], &[], &config, 10);
        assert!(results.len() <= 10);
    }
}
