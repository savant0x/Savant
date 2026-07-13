//! # savant_schema — Code Intelligence for Savant
//!
//! AST parsing, symbol extraction, call graph analysis, FTS5 search,
//! and impact radius computation for Rust, TypeScript, and Python.
//!
//! ## Quick Start
//!
//! ```rust,no_run
//! use savant_schema::SchemaIndex;
//! use std::path::PathBuf;
//!
//! let index = SchemaIndex::open(PathBuf::from(".")).unwrap();
//! let stats = index.index_all();
//! println!("Indexed {} files, {} symbols", stats.files_indexed, stats.symbols_found);
//!
//! // Search for symbols
//! let results = index.search("process", 10).unwrap();
//!
//! // Get callers of a function
//! let callers = index.get_callers("src/lib.rs:1:process").unwrap();
//!
//! // Get impact radius
//! let impact = index.get_impact("src/lib.rs:1:process", 3);
//! ```

pub mod db;
pub mod extract;
pub mod graph;
pub mod indexer;
pub mod resolution;
pub mod symbols;

use db::SchemaDb;
use graph::ImpactEntry;
use indexer::{IndexStats, Indexer, IndexerConfig};
use resolution::{FrameworkPattern, ResolutionResult};
use std::path::PathBuf;
use symbols::{SearchResult, Symbol};

/// The main entry point for code intelligence.
///
/// Manages the database, indexer, and query API in one unified interface.
pub struct SchemaIndex {
    indexer: Indexer,
}

impl SchemaIndex {
    /// Open or create a code index at the given project root.
    ///
    /// Database is stored at `<root>/.savant/schema/code.db`.
    pub fn open(project_root: PathBuf) -> Result<Self, SchemaIndexError> {
        let db_dir = project_root.join(".savant").join("schema");
        std::fs::create_dir_all(&db_dir).map_err(|e| SchemaIndexError::IoError(e.to_string()))?;
        let db_path = db_dir.join("code.db");
        let db =
            SchemaDb::open(&db_path).map_err(|e| SchemaIndexError::DatabaseError(e.to_string()))?;
        let config = IndexerConfig::new(project_root);
        let indexer = Indexer::new(db, config);
        Ok(Self { indexer })
    }

    /// Open an in-memory index (for testing).
    pub fn open_memory(project_root: PathBuf) -> Result<Self, SchemaIndexError> {
        let db =
            SchemaDb::open_memory().map_err(|e| SchemaIndexError::DatabaseError(e.to_string()))?;
        let config = IndexerConfig::new(project_root);
        let indexer = Indexer::new(db, config);
        Ok(Self { indexer })
    }

    /// Full index of all files in the project.
    pub fn index_all(&self) -> IndexStats {
        self.indexer.index_all()
    }

    /// Re-index a single file (incremental update).
    pub fn index_file(&self, path: &std::path::Path) -> Result<(), String> {
        self.indexer.index_file(path)
    }

    /// Full-text search over symbol names, qualified names, and documentation.
    pub fn search(&self, query: &str, limit: u32) -> Result<Vec<SearchResult>, SchemaIndexError> {
        let symbols = self
            .indexer
            .db()
            .search_symbols(query, limit)
            .map_err(|e| SchemaIndexError::DatabaseError(e.to_string()))?;
        let results = symbols
            .into_iter()
            .map(|symbol| SearchResult {
                score: 1.0, // FTS5 rank is already factored in ordering
                snippet: format!(
                    "{} ({}:{})",
                    symbol.qualified_name, symbol.file_path, symbol.start_line
                ),
                symbol,
            })
            .collect();
        Ok(results)
    }

    /// Get a symbol by its ID.
    pub fn get_symbol(&self, id: &str) -> Result<Option<Symbol>, SchemaIndexError> {
        self.indexer
            .db()
            .get_symbol(id)
            .map_err(|e| SchemaIndexError::DatabaseError(e.to_string()))
    }

    /// Get all symbols in a file.
    pub fn get_symbols_in_file(&self, file_path: &str) -> Result<Vec<Symbol>, SchemaIndexError> {
        self.indexer
            .db()
            .get_symbols_in_file(file_path)
            .map_err(|e| SchemaIndexError::DatabaseError(e.to_string()))
    }

    /// Get all callers of a symbol (who calls this function?).
    pub fn get_callers(&self, symbol_id: &str) -> Result<Vec<Symbol>, SchemaIndexError> {
        self.indexer
            .db()
            .get_callers(symbol_id)
            .map_err(|e| SchemaIndexError::DatabaseError(e.to_string()))
    }

    /// Get all callees of a symbol (what does this function call?).
    pub fn get_callees(&self, symbol_id: &str) -> Result<Vec<Symbol>, SchemaIndexError> {
        self.indexer
            .db()
            .get_callees(symbol_id)
            .map_err(|e| SchemaIndexError::DatabaseError(e.to_string()))
    }

    /// Compute the impact radius of a symbol change.
    pub fn get_impact(&self, symbol_id: &str, max_depth: u32) -> Vec<ImpactEntry> {
        graph::get_impact_radius(self.indexer.db(), symbol_id, max_depth)
    }

    /// Build a markdown context summary for a symbol (for LLM consumption).
    pub fn build_context(&self, symbol_id: &str) -> String {
        graph::build_context_markdown(self.indexer.db(), symbol_id)
    }

    /// Resolve all imports in the indexed codebase.
    /// Links import statements to their source symbols and detects framework patterns.
    pub fn resolve_imports(&self) -> ResolutionResult {
        resolution::resolve_imports(self.indexer.db())
    }

    /// Get all detected framework patterns (Axum, Express, FastAPI, Actix).
    pub fn get_framework_patterns(&self) -> Vec<FrameworkPattern> {
        self.resolve_imports().patterns
    }

    /// Get index statistics.
    pub fn stats(&self) -> Result<(u32, u32, u32), SchemaIndexError> {
        let db = self.indexer.db();
        let symbols = db
            .symbol_count()
            .map_err(|e| SchemaIndexError::DatabaseError(e.to_string()))?;
        let edges = db
            .edge_count()
            .map_err(|e| SchemaIndexError::DatabaseError(e.to_string()))?;
        let files = db
            .file_count()
            .map_err(|e| SchemaIndexError::DatabaseError(e.to_string()))?;
        Ok((symbols, edges, files))
    }

    /// Clear all indexed data.
    pub fn clear(&self) -> Result<(), SchemaIndexError> {
        self.indexer
            .db()
            .clear()
            .map_err(|e| SchemaIndexError::DatabaseError(e.to_string()))
    }
}

/// Errors from the code intelligence index.
#[derive(Debug, thiserror::Error)]
pub enum SchemaIndexError {
    #[error("IO error: {0}")]
    IoError(String),
    #[error("Database error: {0}")]
    DatabaseError(String),
}
