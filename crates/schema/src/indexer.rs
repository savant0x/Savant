//! Incremental code indexing with file watching.

use crate::db::SchemaDb;
use crate::extract::{default_extractors, detect_language, LanguageExtractor};
use crate::symbols::IndexedFile;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

/// Default file extensions to index.
const INDEXABLE_EXTENSIONS: &[&str] = &["rs", "ts", "tsx", "js", "jsx", "py", "pyi"];

/// Configuration for the indexer.
pub struct IndexerConfig {
    /// Root directory to index.
    pub root: PathBuf,
    /// Patterns to ignore (gitignore-style, simplified).
    pub ignore_patterns: Vec<String>,
    /// Maximum file size in bytes to index.
    pub max_file_size: u64,
}

impl IndexerConfig {
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            ignore_patterns: vec![
                "node_modules".to_string(),
                "target".to_string(),
                ".git".to_string(),
                "__pycache__".to_string(),
                "dist".to_string(),
                "build".to_string(),
                ".savant".to_string(),
            ],
            max_file_size: 1_000_000, // 1MB
        }
    }
}

/// The code indexer — manages parsing, storage, and incremental updates.
pub struct Indexer {
    db: SchemaDb,
    config: IndexerConfig,
    extractors: Vec<Box<dyn LanguageExtractor>>,
}

impl Indexer {
    /// Create a new indexer with the given database and config.
    pub fn new(db: SchemaDb, config: IndexerConfig) -> Self {
        let extractors = default_extractors();
        Self {
            db,
            config,
            extractors,
        }
    }

    /// Get a reference to the underlying database.
    pub fn db(&self) -> &SchemaDb {
        &self.db
    }

    /// Full index of all files in the root directory.
    pub fn index_all(&self) -> IndexStats {
        let mut stats = IndexStats::default();

        for entry in WalkDir::new(&self.config.root)
            .into_iter()
            .filter_entry(|e| !self.should_ignore(e.path()))
        {
            let entry = match entry {
                Ok(e) => e,
                Err(_) => continue,
            };

            if !entry.file_type().is_file() {
                continue;
            }

            let path = entry.path();
            let rel_path = match path.strip_prefix(&self.config.root) {
                Ok(p) => p.to_path_buf(),
                Err(_) => continue,
            };

            // Check file extension
            let ext = match path.extension().and_then(|e| e.to_str()) {
                Some(e) => e,
                None => continue,
            };
            if !INDEXABLE_EXTENSIONS.contains(&ext) {
                continue;
            }

            // Check file size
            let metadata = match std::fs::metadata(path) {
                Ok(m) => m,
                Err(_) => continue,
            };
            if metadata.len() > self.config.max_file_size {
                stats.skipped += 1;
                continue;
            }

            // Check if file needs re-indexing
            let content = match std::fs::read(path) {
                Ok(c) => c,
                Err(_) => continue,
            };
            let content_hash = compute_hash(&content);

            if let Ok(Some(existing)) = self.db.get_file(&rel_path.to_string_lossy()) {
                if existing.content_hash == content_hash {
                    stats.unchanged += 1;
                    continue;
                }
                // File changed — delete old data
                if let Err(e) = self.db.delete_file_data(&rel_path.to_string_lossy()) {
                    tracing::warn!(
                        "[schema] Failed to delete old file data for {}: {}",
                        rel_path.display(),
                        e
                    );
                }
            }

            // Detect language and extract
            let language = match detect_language(&self.extractors, path) {
                Some(lang) => lang,
                None => continue,
            };

            let extractor = match self.extractors.iter().find(|e| e.language() == language) {
                Some(e) => e,
                None => continue,
            };

            let result = extractor.extract(&content, &rel_path);

            // Store symbols
            for sym in &result.symbols {
                if let Err(e) = self.db.upsert_symbol(sym) {
                    tracing::warn!("Failed to store symbol {}: {}", sym.name, e);
                }
            }

            // Store edges
            for edge in &result.edges {
                if let Err(e) = self.db.upsert_edge(edge) {
                    tracing::warn!("Failed to store edge {}: {}", edge.id, e);
                }
            }

            // Store file record
            let file_record = IndexedFile {
                path: rel_path.to_string_lossy().to_string(),
                language,
                mtime: metadata
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_secs() as i64)
                    .unwrap_or(0),
                content_hash,
            };
            if let Err(e) = self.db.upsert_file(&file_record) {
                tracing::warn!(
                    "[schema] Failed to store file record for {}: {}",
                    file_record.path,
                    e
                );
            }

            stats.files_indexed += 1;
            stats.symbols_found += result.symbols.len() as u32;
            stats.edges_found += result.edges.len() as u32;
        }

        stats
    }

    /// Re-index a single file (for incremental updates).
    pub fn index_file(&self, path: &Path) -> Result<(), String> {
        let rel_path = path
            .strip_prefix(&self.config.root)
            .map_err(|e| format!("Path not under root: {}", e))?;

        let content = std::fs::read(path).map_err(|e| format!("Failed to read file: {}", e))?;
        let content_hash = compute_hash(&content);

        // Delete old data
        if let Err(e) = self.db.delete_file_data(&rel_path.to_string_lossy()) {
            tracing::warn!(
                "[schema] Failed to delete old file data for {}: {}",
                rel_path.display(),
                e
            );
        }

        let language = detect_language(&self.extractors, path)
            .ok_or_else(|| format!("Unknown language for {}", path.display()))?;

        let extractor = self
            .extractors
            .iter()
            .find(|e| e.language() == language)
            .ok_or_else(|| format!("No extractor for language {}", language))?;

        let result = extractor.extract(&content, rel_path);

        for sym in &result.symbols {
            self.db
                .upsert_symbol(sym)
                .map_err(|e| format!("Failed to store symbol: {}", e))?;
        }
        for edge in &result.edges {
            self.db
                .upsert_edge(edge)
                .map_err(|e| format!("Failed to store edge: {}", e))?;
        }

        let metadata =
            std::fs::metadata(path).map_err(|e| format!("Failed to get metadata: {}", e))?;
        let file_record = IndexedFile {
            path: rel_path.to_string_lossy().to_string(),
            language,
            mtime: metadata
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0),
            content_hash,
        };
        self.db
            .upsert_file(&file_record)
            .map_err(|e| format!("Failed to store file: {}", e))?;

        Ok(())
    }

    fn should_ignore(&self, path: &Path) -> bool {
        let path_str = path.to_string_lossy();
        for pattern in &self.config.ignore_patterns {
            if path_str.contains(pattern) {
                return true;
            }
        }
        false
    }
}

/// Statistics from an indexing run.
#[derive(Debug, Default)]
pub struct IndexStats {
    pub files_indexed: u32,
    pub symbols_found: u32,
    pub edges_found: u32,
    pub unchanged: u32,
    pub skipped: u32,
}

fn compute_hash(data: &[u8]) -> String {
    let mut hasher = DefaultHasher::new();
    data.hash(&mut hasher);
    format!("{:x}", hasher.finish())
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_index_temp_project() {
        let tmp = std::env::temp_dir().join("savant_schema_test_index");
        let _ = fs::remove_dir_all(&tmp);
        fs::create_dir_all(tmp.join("src")).unwrap();

        fs::write(
            tmp.join("src/lib.rs"),
            r#"
fn main() {
    hello();
}

fn hello() {
    println!("hello");
}
"#,
        )
        .unwrap();

        let db = SchemaDb::open_memory().unwrap();
        let config = IndexerConfig::new(tmp.clone());
        let indexer = Indexer::new(db, config);

        let stats = indexer.index_all();
        assert!(stats.files_indexed >= 1);
        assert!(stats.symbols_found >= 2);

        let _ = fs::remove_dir_all(&tmp);
    }
}
