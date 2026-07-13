//! Cross-Encoder Reranker (MEM-07)
//!
//! Optional post-processing step that reranks top-N search results using a
//! cross-encoder model for higher precision. Falls back to raw scores if
//! the model is unavailable.
//!
//! The reranker uses the existing fastembed model (AllMiniLML6V2) for
//! cross-encoder scoring when available. This avoids loading a second model
//! and keeps memory usage reasonable.

/// A candidate result to be reranked.
#[derive(Debug, Clone)]
pub struct RerankCandidate {
    /// Document/memory ID.
    pub doc_id: u64,
    /// Original score from the search stream.
    pub original_score: f32,
    /// The content/text of the document for reranking.
    pub content: String,
    /// Session ID for context.
    pub session_id: String,
}

/// Reranked result with updated score.
#[derive(Debug, Clone)]
pub struct RerankedResult {
    /// Document/memory ID.
    pub doc_id: u64,
    /// Reranked score (higher is better).
    pub reranked_score: f32,
    /// Original score before reranking.
    pub original_score: f32,
    /// Session ID.
    pub session_id: String,
}

/// Reranks candidates using dot-product similarity between query embedding
/// and document embeddings. This is a lightweight approximation of full
/// cross-encoder reranking that works with any embedding model.
///
/// For true cross-encoder scoring, a dedicated model (ms-marco-MiniLM)
/// would be needed. This implementation uses the existing embedding model
/// for a reasonable trade-off between quality and resource usage.
pub fn rerank(
    query: &str,
    candidates: Vec<RerankCandidate>,
    embed_fn: &dyn Fn(&str) -> Result<Vec<f32>, String>,
    top_n: usize,
) -> Vec<RerankedResult> {
    if candidates.is_empty() {
        return Vec::new();
    }

    // Get query embedding
    let query_embedding = match embed_fn(query) {
        Ok(emb) => emb,
        Err(e) => {
            // Fallback: return candidates with original scores
            tracing::warn!("Query embedding failed ({}), returning unranked results", e);
            return candidates
                .into_iter()
                .take(top_n)
                .map(|c| RerankedResult {
                    doc_id: c.doc_id,
                    reranked_score: c.original_score,
                    original_score: c.original_score,
                    session_id: c.session_id,
                })
                .collect();
        }
    };

    // Score each candidate by cosine similarity to query
    let mut scored: Vec<(RerankCandidate, f32)> = candidates
        .into_iter()
        .map(|candidate| {
            let original_score = candidate.original_score;
            let doc_embedding = match embed_fn(&candidate.content) {
                Ok(emb) => emb,
                Err(e) => {
                    tracing::warn!("Document embedding failed ({}), using original score", e);
                    return (candidate, original_score);
                }
            };
            let similarity = cosine_similarity(&query_embedding, &doc_embedding);
            (candidate, similarity)
        })
        .collect();

    // Sort by reranked score descending
    scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

    scored
        .into_iter()
        .take(top_n)
        .map(|(candidate, score)| RerankedResult {
            doc_id: candidate.doc_id,
            reranked_score: score,
            original_score: candidate.original_score,
            session_id: candidate.session_id,
        })
        .collect()
}

/// Computes cosine similarity between two vectors.
fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }

    let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();

    if norm_a == 0.0 || norm_b == 0.0 {
        0.0
    } else {
        dot / (norm_a * norm_b)
    }
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)]
mod tests {
    use super::*;

    #[test]
    fn test_cosine_similarity_identical() {
        let a = vec![1.0, 0.0, 0.0];
        let b = vec![1.0, 0.0, 0.0];
        assert!((cosine_similarity(&a, &b) - 1.0).abs() < 0.001);
    }

    #[test]
    fn test_cosine_similarity_orthogonal() {
        let a = vec![1.0, 0.0, 0.0];
        let b = vec![0.0, 1.0, 0.0];
        assert!((cosine_similarity(&a, &b) - 0.0).abs() < 0.001);
    }

    #[test]
    fn test_cosine_similarity_opposite() {
        let a = vec![1.0, 0.0];
        let b = vec![-1.0, 0.0];
        assert!((cosine_similarity(&a, &b) - (-1.0)).abs() < 0.001);
    }

    #[test]
    fn test_cosine_similarity_empty() {
        assert_eq!(cosine_similarity(&[], &[]), 0.0);
    }

    #[test]
    fn test_rerank_fallback_on_error() {
        let candidates = vec![
            RerankCandidate {
                doc_id: 1,
                original_score: 0.9,
                content: "hello".to_string(),
                session_id: "s1".to_string(),
            },
            RerankCandidate {
                doc_id: 2,
                original_score: 0.8,
                content: "world".to_string(),
                session_id: "s1".to_string(),
            },
        ];

        let embed_fn = |_text: &str| Err("model unavailable".to_string());
        let results = rerank("query", candidates, &embed_fn, 10);
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].doc_id, 1); // preserves original order
    }

    #[test]
    fn test_rerank_with_mock_embeddings() {
        let candidates = vec![
            RerankCandidate {
                doc_id: 1,
                original_score: 0.5,
                content: "similar content".to_string(),
                session_id: "s1".to_string(),
            },
            RerankCandidate {
                doc_id: 2,
                original_score: 0.9,
                content: "different content".to_string(),
                session_id: "s1".to_string(),
            },
        ];

        // Mock: "query" and "similar content" both embed to [1,0,0],
        // "different content" embeds to [0,1,0]
        let embed_fn = |text: &str| -> Result<Vec<f32>, String> {
            if text.contains("query") || text.contains("similar") {
                Ok(vec![1.0, 0.0, 0.0])
            } else {
                Ok(vec![0.0, 1.0, 0.0])
            }
        };

        let results = rerank("query", candidates, &embed_fn, 10);
        assert_eq!(results[0].doc_id, 1); // similar content should rank first
    }

    #[test]
    fn test_rerank_empty_candidates() {
        let embed_fn = |_text: &str| Ok(vec![1.0, 0.0]);
        let results = rerank("query", vec![], &embed_fn, 10);
        assert!(results.is_empty());
    }

    #[test]
    fn test_rerank_top_n_limit() {
        let candidates: Vec<RerankCandidate> = (0..100)
            .map(|i| RerankCandidate {
                doc_id: i,
                original_score: 1.0 - i as f32 * 0.01,
                content: format!("doc {}", i),
                session_id: "s1".to_string(),
            })
            .collect();

        let embed_fn = |_text: &str| Ok(vec![1.0, 0.0, 0.0]);
        let results = rerank("query", candidates, &embed_fn, 10);
        assert!(results.len() <= 10);
    }
}
