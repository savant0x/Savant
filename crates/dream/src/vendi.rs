//! Vendi Score — Diversity metric for dream output filtering.
//!
//! Approximates the Vendi Score (diversity evaluation metric) using pairwise
//! distance variance over dream output embeddings. Higher scores indicate
//! more diverse outputs.
//!
//! # Formula
//! `score = variance / (1.0 + variance)`
//!
//! Note: The true Vendi Score uses the harmonic mean of eigenvalues of the
//! similarity matrix. This approximation is sufficient for filtering.

/// Computes an approximate Vendi Score for a set of embedding vectors.
///
/// Returns a score in [0, 1] where higher = more diverse.
/// Returns 0.0 if fewer than 2 embeddings are provided.
pub fn vendi_score(embeddings: &[Vec<f32>]) -> f32 {
    if embeddings.len() < 2 {
        return 0.0;
    }

    let n = embeddings.len();
    let mut distances = Vec::with_capacity(n * (n - 1) / 2);

    for i in 0..n {
        for j in (i + 1)..n {
            let dist = cosine_distance(&embeddings[i], &embeddings[j]);
            distances.push(dist);
        }
    }

    if distances.is_empty() {
        return 0.0;
    }

    let mean = distances.iter().sum::<f32>() / distances.len() as f32;
    let variance =
        distances.iter().map(|d| (d - mean).powi(2)).sum::<f32>() / distances.len() as f32;

    // Higher variance = more diverse = higher score
    variance / (1.0 + variance)
}

/// Computes Vendi Score from raw text contents using a simple bag-of-words approach.
/// This is a fallback when embeddings are not available.
pub fn vendi_score_from_text(contents: &[String]) -> f32 {
    if contents.len() < 2 {
        return 0.0;
    }

    let n = contents.len();
    let mut distances = Vec::with_capacity(n * (n - 1) / 2);

    for i in 0..n {
        for j in (i + 1)..n {
            let dist = jaccard_distance(&contents[i], &contents[j]);
            distances.push(dist);
        }
    }

    if distances.is_empty() {
        return 0.0;
    }

    let mean = distances.iter().sum::<f32>() / distances.len() as f32;
    let variance =
        distances.iter().map(|d| (d - mean).powi(2)).sum::<f32>() / distances.len() as f32;

    variance / (1.0 + variance)
}

/// Computes cosine distance between two vectors (1 - cosine similarity).
/// Returns 0.0 for identical vectors, 2.0 for opposite vectors.
fn cosine_distance(a: &[f32], b: &[f32]) -> f32 {
    let len = a.len().min(b.len());
    if len == 0 {
        return 0.0;
    }

    let mut dot = 0.0f32;
    let mut norm_a = 0.0f32;
    let mut norm_b = 0.0f32;

    for i in 0..len {
        dot += a[i] * b[i];
        norm_a += a[i] * a[i];
        norm_b += b[i] * b[i];
    }

    let denom = (norm_a.sqrt() * norm_b.sqrt()).max(f32::EPSILON);
    let similarity = (dot / denom).clamp(-1.0, 1.0);
    1.0 - similarity
}

/// Jaccard distance between two strings (1 - |intersection| / |union| of word sets).
fn jaccard_distance(a: &str, b: &str) -> f32 {
    use std::collections::HashSet;

    let words_a: HashSet<&str> = a.split_whitespace().collect();
    let words_b: HashSet<&str> = b.split_whitespace().collect();

    let intersection = words_a.intersection(&words_b).count();
    let union = words_a.union(&words_b).count();

    if union == 0 {
        return 0.0;
    }

    1.0 - (intersection as f32 / union as f32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_vendi_score_identical_vectors() {
        let embeddings = vec![vec![1.0, 0.0, 0.0]; 5];
        let score = vendi_score(&embeddings);
        assert!(
            score < 0.01,
            "Identical vectors should have near-zero score (zero variance)"
        );
    }

    #[test]
    fn test_vendi_score_diverse_vectors() {
        let embeddings = vec![
            vec![1.0, 0.0, 0.0],
            vec![0.0, 1.0, 0.0],
            vec![0.0, 0.0, 1.0],
            vec![-1.0, 0.0, 0.0],
        ];
        let score = vendi_score(&embeddings);
        assert!(
            score > 0.1,
            "Diverse vectors should have higher score (higher variance), got {}",
            score
        );
    }

    #[test]
    fn test_vendi_score_single_vector() {
        let embeddings = vec![vec![1.0, 0.0]];
        assert_eq!(vendi_score(&embeddings), 0.0);
    }

    #[test]
    fn test_vendi_score_empty() {
        let embeddings: Vec<Vec<f32>> = vec![];
        assert_eq!(vendi_score(&embeddings), 0.0);
    }

    #[test]
    fn test_cosine_distance_identical() {
        let a = vec![1.0, 2.0, 3.0];
        let dist = cosine_distance(&a, &a);
        assert!(dist < 0.001, "Identical vectors should have ~0 distance");
    }

    #[test]
    fn test_cosine_distance_orthogonal() {
        let a = vec![1.0, 0.0];
        let b = vec![0.0, 1.0];
        let dist = cosine_distance(&a, &b);
        assert!(
            (dist - 1.0).abs() < 0.001,
            "Orthogonal vectors should have distance ~1"
        );
    }

    #[test]
    fn test_vendi_score_from_text_diverse() {
        let texts = vec![
            "the cat sat on the mat".to_string(),
            "dogs run in the park".to_string(),
            "quantum physics is fascinating".to_string(),
        ];
        let score = vendi_score_from_text(&texts);
        assert!(score > 0.0 && score <= 1.0, "Score should be in [0, 1]");
    }

    #[test]
    fn test_vendi_score_from_text_identical() {
        let texts = vec!["hello world".to_string(), "hello world".to_string()];
        let score = vendi_score_from_text(&texts);
        assert!(score < 0.01, "Identical texts should have near-zero score");
    }

    #[test]
    fn test_jaccard_distance_identical() {
        let dist = jaccard_distance("hello world", "hello world");
        assert_eq!(dist, 0.0);
    }

    #[test]
    fn test_jaccard_distance_disjoint() {
        let dist = jaccard_distance("cat dog", "fish bird");
        assert_eq!(dist, 1.0);
    }
}
