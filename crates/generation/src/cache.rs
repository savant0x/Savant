//! Image Cache
//!
//! Hash-based image caching for generated images. Avoids regenerating
//! the same image when the same prompt and parameters are used.

use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::PathBuf;
use tracing::{debug, info};

use crate::{GenerationError, GenerationResult};

/// Image cache for generated images
///
/// Caches generated images based on prompt + parameters hash.
/// Avoids regenerating the same image multiple times.
pub struct ImageCache {
    /// Cache directory
    cache_dir: PathBuf,
    /// In-memory cache (hash -> file path)
    index: HashMap<String, PathBuf>,
    /// Max cache size in bytes
    max_size_bytes: u64,
    /// Current cache size in bytes
    current_size_bytes: u64,
}

impl ImageCache {
    /// Create a new image cache
    pub fn new(cache_dir: PathBuf, max_size_mb: u64) -> Result<Self, GenerationError> {
        // Create cache directory if it doesn't exist
        std::fs::create_dir_all(&cache_dir)
            .map_err(|e| GenerationError::Cache(format!("Failed to create cache dir: {}", e)))?;

        let mut cache = Self {
            cache_dir,
            index: HashMap::new(),
            max_size_bytes: max_size_mb * 1024 * 1024,
            current_size_bytes: 0,
        };

        // Scan existing cache files
        cache.scan_cache()?;

        info!(
            "Image cache initialized: {} files, {:.1} MB",
            cache.index.len(),
            cache.current_size_bytes as f64 / (1024.0 * 1024.0)
        );

        Ok(cache)
    }

    /// Generate cache key from prompt and parameters
    pub fn cache_key(prompt: &str, style: &str, aspect_ratio: &str, quality: &str) -> String {
        let mut hasher = Sha256::new();
        hasher.update(prompt.as_bytes());
        hasher.update(style.as_bytes());
        hasher.update(aspect_ratio.as_bytes());
        hasher.update(quality.as_bytes());
        format!("{:x}", hasher.finalize())
    }

    /// Check if an image exists in cache
    pub fn get(&self, key: &str) -> Option<PathBuf> {
        self.index.get(key).cloned()
    }

    /// Store an image in cache
    pub fn put(
        &mut self,
        key: &str,
        result: &GenerationResult,
    ) -> Result<PathBuf, GenerationError> {
        let filename = format!("{}.webp", key);
        let filepath = self.cache_dir.join(&filename);

        // Write image to cache
        std::fs::write(&filepath, &result.data)
            .map_err(|e| GenerationError::Cache(format!("Failed to write cache file: {}", e)))?;

        let size = result.data.len() as u64;

        // Update index
        self.index.insert(key.to_string(), filepath.clone());
        self.current_size_bytes += size;

        // Evict if over limit
        self.evict_if_needed()?;

        debug!("Cached image: {} ({} bytes)", key, size);

        Ok(filepath)
    }

    /// Get or generate an image
    pub async fn get_or_generate<F, Fut>(
        &mut self,
        key: &str,
        generate: F,
    ) -> Result<GenerationResult, GenerationError>
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = Result<GenerationResult, GenerationError>>,
    {
        // Check cache first
        if let Some(path) = self.get(key) {
            let data = std::fs::read(&path)
                .map_err(|e| GenerationError::Cache(format!("Failed to read cache: {}", e)))?;

            info!("Cache hit: {}", key);

            return Ok(GenerationResult {
                id: format!("cached-{}", &key[..12]),
                data,
                cached: true,
                ..Default::default()
            });
        }

        // Generate new image
        let result = generate().await?;

        // Cache the result
        self.put(key, &result)?;

        Ok(result)
    }

    /// Scan existing cache files
    fn scan_cache(&mut self) -> Result<(), GenerationError> {
        let entries = std::fs::read_dir(&self.cache_dir)
            .map_err(|e| GenerationError::Cache(format!("Failed to read cache dir: {}", e)))?;

        for entry in entries {
            let entry = entry
                .map_err(|e| GenerationError::Cache(format!("Failed to read entry: {}", e)))?;
            let path = entry.path();

            if path.extension().is_some_and(|ext| ext == "webp") {
                let metadata = std::fs::metadata(&path).map_err(|e| {
                    GenerationError::Cache(format!("Failed to read metadata: {}", e))
                })?;

                let key = path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("")
                    .to_string();

                self.index.insert(key, path);
                self.current_size_bytes += metadata.len();
            }
        }

        Ok(())
    }

    /// Evict oldest files if over size limit
    fn evict_if_needed(&mut self) -> Result<(), GenerationError> {
        while self.current_size_bytes > self.max_size_bytes && !self.index.is_empty() {
            // Find oldest file
            let oldest = self
                .index
                .iter()
                .filter_map(|(key, path)| {
                    std::fs::metadata(path)
                        .ok()
                        .and_then(|m| m.modified().ok())
                        .map(|t| (key.clone(), path.clone(), t))
                })
                .min_by_key(|(_, _, t)| *t);

            if let Some((key, path, _)) = oldest {
                let size = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);

                std::fs::remove_file(&path).map_err(|e| {
                    GenerationError::Cache(format!("Failed to delete cache file: {}", e))
                })?;

                self.index.remove(&key);
                self.current_size_bytes -= size;

                debug!("Evicted cache entry: {} ({} bytes)", key, size);
            } else {
                break;
            }
        }

        Ok(())
    }

    /// Clear the entire cache
    pub fn clear(&mut self) -> Result<(), GenerationError> {
        for (_, path) in self.index.drain() {
            let _ = std::fs::remove_file(path);
        }
        self.current_size_bytes = 0;
        Ok(())
    }

    /// Get cache statistics
    pub fn stats(&self) -> CacheStats {
        CacheStats {
            file_count: self.index.len(),
            total_size_bytes: self.current_size_bytes,
            max_size_bytes: self.max_size_bytes,
        }
    }
}

/// Cache statistics
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CacheStats {
    /// Number of cached files
    pub file_count: usize,
    /// Total cache size in bytes
    pub total_size_bytes: u64,
    /// Max cache size in bytes
    pub max_size_bytes: u64,
}

#[allow(clippy::derivable_impls)]
impl Default for GenerationResult {
    fn default() -> Self {
        Self {
            id: String::new(),
            prompt: String::new(),
            expanded_prompt: String::new(),
            data: Vec::new(),
            mime_type: String::new(),
            width: 0,
            height: 0,
            duration_ms: 0,
            model: String::new(),
            backend: String::new(),
            cached: false,
        }
    }
}
