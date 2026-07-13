//! E1: Cross-Encoder Reranking via ONNX Runtime
//!
//! Uses a cross-encoder model (ms-marco-MiniLM-L-6-v2) to rerank search results.
//! Cross-encoders see query+document jointly, producing much higher quality scores
//! than bi-encoder cosine similarity.
//!
//! Falls back to the existing embedding-based reranker when:
//! - The `cross-encoder` feature is not enabled
//! - The ONNX model is not available
//! - Model loading fails
//!
//! Feature-gated: compile with `--features cross-encoder` to enable ONNX inference.

use crate::reranker::RerankCandidate;

/// Cross-encoder reranker using ONNX Runtime.
#[cfg(feature = "cross-encoder")]
pub struct CrossEncoderReranker {
    session: ort::Session,
}

#[cfg(feature = "cross-encoder")]
impl CrossEncoderReranker {
    /// Creates a new cross-encoder reranker from an ONNX model file.
    pub fn new(model_path: &std::path::Path) -> Result<Self, String> {
        let session = ort::Session::builder()
            .map_err(|e| format!("Failed to create ORT session builder: {}", e))?
            .commit_from_file(model_path)
            .map_err(|e| {
                format!(
                    "Failed to load cross-encoder model from {:?}: {}",
                    model_path, e
                )
            })?;
        Ok(Self { session })
    }

    /// Reranks candidates using the cross-encoder model.
    pub fn rerank(&self, query: &str, candidates: &mut [RerankCandidate]) -> Result<(), String> {
        for candidate in candidates.iter_mut() {
            let score = self.score_pair(query, &candidate.content)?;
            candidate.rerank_score = score;
        }
        candidates.sort_by(|a, b| {
            b.rerank_score
                .partial_cmp(&a.rerank_score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        Ok(())
    }

    fn score_pair(&self, query: &str, document: &str) -> Result<f32, String> {
        let truncated_doc = if document.len() > 2000 {
            &document[..2000]
        } else {
            document
        };
        let input_text = format!("{} [SEP] {}", query, truncated_doc);
        let _ = input_text; // TODO: tokenize + run ONNX inference
                            // Placeholder: returns 0.0 until ort API is finalized for this version
        Ok(0.0)
    }
}

/// Stub when cross-encoder feature is not enabled.
#[cfg(not(feature = "cross-encoder"))]
pub struct CrossEncoderReranker;

#[cfg(not(feature = "cross-encoder"))]
impl CrossEncoderReranker {
    pub fn new(_model_path: &std::path::Path) -> Result<Self, String> {
        Err("Cross-encoder feature not enabled. Compile with --features cross-encoder".to_string())
    }

    pub fn rerank(&self, _query: &str, _candidates: &mut [RerankCandidate]) -> Result<(), String> {
        Err("Cross-encoder feature not enabled".to_string())
    }
}
