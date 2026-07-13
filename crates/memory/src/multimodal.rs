//! Image/Multimodal Memory (MEM-15)
//!
//! Supports storing and retrieving images alongside text memories.
//! Uses CLIP-style embedding for cross-modal search (text query -> image
//! results and vice versa).
//!
//! Images are stored as file references (not inline bytes) with
//! CLIP-generated text descriptions for searchability.
//!
//! Feature-gated behind `multimodal` — disabled by default.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tracing::{info, warn};

/// A multimodal memory entry that references an image.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MultimodalMemory {
    /// Unique ID.
    pub id: u64,
    /// Session this belongs to.
    pub session_id: String,
    /// File path to the image (relative to workspace).
    pub file_path: String,
    /// CLIP-generated text description of the image.
    pub description: String,
    /// CLIP embedding vector (512 dims for ViT-B/32).
    pub clip_embedding: Vec<f32>,
    /// Text embedding of the description (for text-based search).
    pub text_embedding: Vec<f32>,
    /// Tags for categorization.
    pub tags: Vec<String>,
    /// Importance score.
    pub importance: u8,
    /// Creation timestamp.
    pub created_at: i64,
    /// Image width in pixels (if known).
    pub width: Option<u32>,
    /// Image height in pixels (if known).
    pub height: Option<u32>,
    /// MIME type (e.g., "image/png", "image/jpeg").
    pub mime_type: String,
}

/// Cross-modal search result.
#[derive(Debug, Clone)]
pub struct CrossModalResult {
    /// The multimodal memory entry.
    pub memory: MultimodalMemory,
    /// Similarity score (0.0 - 1.0).
    pub score: f32,
    /// Whether this was matched via text or image embedding.
    pub match_type: MatchType,
}

/// How a cross-modal result was matched.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MatchType {
    /// Matched via text embedding similarity.
    TextToText,
    /// Matched via CLIP cross-modal similarity.
    TextToImage,
    /// Matched via image-to-image similarity.
    ImageToImage,
}

/// Multimodal memory store with optional file persistence.
///
/// When constructed with `new_with_path`, entries are persisted to disk
/// after each mutation (`store`, `remove`) and loaded at startup.
/// The persistence file is `{storage_path}/multimodal_store.json`.
pub struct MultimodalStore {
    entries: Vec<MultimodalMemory>,
    /// Directory for persistence file. If None, store is in-memory only.
    storage_path: Option<PathBuf>,
}

impl MultimodalStore {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            storage_path: None,
        }
    }

    /// Creates a new store with disk persistence at the given path.
    /// Loads existing entries from disk if the persistence file exists.
    pub fn new_with_path(storage_path: PathBuf) -> Self {
        let mut store = Self {
            entries: Vec::new(),
            storage_path: Some(storage_path),
        };
        if let Err(e) = store.load() {
            warn!("Failed to load multimodal store from disk: {}", e);
        }
        store
    }

    /// Returns the path to the persistence file.
    fn persist_path(&self) -> Option<PathBuf> {
        self.storage_path
            .as_ref()
            .map(|p| p.join("multimodal_store.json"))
    }

    /// Persists all entries to disk. Called after every mutation.
    fn persist_to_disk(&self) {
        if let Some(path) = self.persist_path() {
            match serde_json::to_string_pretty(&self.entries) {
                Ok(json) => {
                    if let Some(parent) = path.parent() {
                        if let Err(e) = std::fs::create_dir_all(parent) {
                            warn!(
                                "Failed to create multimodal store directory {:?}: {}",
                                parent, e
                            );
                            return;
                        }
                    }
                    if let Err(e) = std::fs::write(&path, json) {
                        warn!("Failed to persist multimodal store: {}", e);
                    } else {
                        info!(
                            entries = self.entries.len(),
                            "Multimodal store persisted to disk"
                        );
                    }
                }
                Err(e) => warn!("Failed to serialize multimodal store: {}", e),
            }
        }
    }

    /// Loads entries from disk. Called at startup.
    pub fn load(&mut self) -> Result<(), String> {
        if let Some(path) = self.persist_path() {
            if path.exists() {
                let data = std::fs::read_to_string(&path)
                    .map_err(|e| format!("Failed to read {}: {}", path.display(), e))?;
                self.entries = serde_json::from_str(&data)
                    .map_err(|e| format!("Failed to parse {}: {}", path.display(), e))?;
                info!(
                    entries = self.entries.len(),
                    "Multimodal store loaded from disk"
                );
            }
        }
        Ok(())
    }

    /// Stores a multimodal memory entry. Persists to disk if configured.
    pub fn store(&mut self, memory: MultimodalMemory) {
        self.entries.push(memory);
        self.persist_to_disk();
    }

    /// Searches by text query. Returns results sorted by cross-modal similarity.
    pub fn search_by_text(
        &self,
        query_embedding: &[f32],
        query_text_embedding: &[f32],
        limit: usize,
    ) -> Vec<CrossModalResult> {
        let mut results: Vec<CrossModalResult> = self
            .entries
            .iter()
            .map(|memory| {
                // Text-to-text similarity
                let text_sim = cosine_similarity(query_text_embedding, &memory.text_embedding);
                // Text-to-image cross-modal similarity
                let clip_sim = cosine_similarity(query_embedding, &memory.clip_embedding);

                // Use the higher of the two
                if text_sim >= clip_sim {
                    CrossModalResult {
                        memory: memory.clone(),
                        score: text_sim,
                        match_type: MatchType::TextToText,
                    }
                } else {
                    CrossModalResult {
                        memory: memory.clone(),
                        score: clip_sim,
                        match_type: MatchType::TextToImage,
                    }
                }
            })
            .collect();

        results.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        results.truncate(limit);
        results
    }

    /// Searches by image embedding. Returns results sorted by image similarity.
    pub fn search_by_image(&self, image_embedding: &[f32], limit: usize) -> Vec<CrossModalResult> {
        let mut results: Vec<CrossModalResult> = self
            .entries
            .iter()
            .map(|memory| CrossModalResult {
                memory: memory.clone(),
                score: cosine_similarity(image_embedding, &memory.clip_embedding),
                match_type: MatchType::ImageToImage,
            })
            .collect();

        results.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        results.truncate(limit);
        results
    }

    /// Returns all entries.
    pub fn entries(&self) -> &[MultimodalMemory] {
        &self.entries
    }

    /// Returns the number of stored entries.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Returns `true` if the store is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Removes an entry by ID. Persists to disk if configured.
    pub fn remove(&mut self, id: u64) -> bool {
        let before = self.entries.len();
        self.entries.retain(|e| e.id != id);
        let changed = self.entries.len() < before;
        if changed {
            self.persist_to_disk();
        }
        changed
    }
}

impl Default for MultimodalStore {
    fn default() -> Self {
        Self::new()
    }
}

// Serde requirements for Vec<MultimodalMemory> serialization (already covered by derive).
// The store persists entries as a JSON array.

/// Cosine similarity between two vectors.
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

    fn make_memory(id: u64, clip: Vec<f32>, text: Vec<f32>) -> MultimodalMemory {
        MultimodalMemory {
            id,
            session_id: "s1".to_string(),
            file_path: format!("images/{}.png", id),
            description: format!("Image {}", id),
            clip_embedding: clip,
            text_embedding: text,
            tags: vec![],
            importance: 5,
            created_at: 0,
            width: Some(100),
            height: Some(100),
            mime_type: "image/png".to_string(),
        }
    }

    #[test]
    fn test_multimodal_store_and_search() {
        let mut store = MultimodalStore::new();
        store.store(make_memory(1, vec![1.0, 0.0, 0.0], vec![0.0, 1.0, 0.0]));
        store.store(make_memory(2, vec![0.0, 0.0, 1.0], vec![0.0, 0.0, 1.0]));

        let results = store.search_by_image(&[1.0, 0.0, 0.0], 10);
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].memory.id, 1); // closest match
    }

    #[test]
    fn test_cross_modal_text_to_image() {
        let mut store = MultimodalStore::new();
        // Image 1: CLIP=[1,0,0], text=[0,1,0]
        store.store(make_memory(1, vec![1.0, 0.0, 0.0], vec![0.0, 1.0, 0.0]));

        // Query: text_embedding matches the description text, CLIP matches image
        let results = store.search_by_text(&[1.0, 0.0, 0.0], &[0.0, 1.0, 0.0], 10);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].memory.id, 1);
    }

    #[test]
    fn test_multimodal_remove() {
        let mut store = MultimodalStore::new();
        store.store(make_memory(1, vec![1.0], vec![1.0]));
        assert_eq!(store.len(), 1);
        assert!(store.remove(1));
        assert_eq!(store.len(), 0);
    }

    #[test]
    fn test_multimodal_empty_search() {
        let store = MultimodalStore::new();
        let results = store.search_by_image(&[1.0, 0.0], 10);
        assert!(results.is_empty());
    }

    #[test]
    fn test_multimodal_limit() {
        let mut store = MultimodalStore::new();
        for i in 0..100 {
            store.store(make_memory(i, vec![i as f32], vec![i as f32]));
        }
        let results = store.search_by_image(&[50.0], 10);
        assert!(results.len() <= 10);
    }
}
