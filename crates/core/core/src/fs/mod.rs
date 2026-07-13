use crate::error::SavantError;
use crate::types::MemoryEntry;
use rusqlite::{params, Connection, OptionalExtension};
use std::path::{Path, PathBuf};
use tokio::task;

pub mod registry;

/// Filesystem Watcher and Indexer
pub struct FileIndexer {
    db_path: PathBuf,
}

impl FileIndexer {
    pub fn new(db_path: PathBuf) -> Self {
        Self { db_path }
    }

    fn open_connection(&self) -> Result<Connection, SavantError> {
        Connection::open(&self.db_path).map_err(|e| SavantError::IoError(std::io::Error::other(e)))
    }

    /// Initializes the database tables and enables WAL mode.
    pub fn init_db(&self) -> Result<(), SavantError> {
        let conn = self.open_connection()?;

        conn.pragma_update(None, "journal_mode", "WAL")
            .map_err(|e| SavantError::Unknown(format!("Failed to enable WAL: {}", e)))?;

        conn.execute(
            "CREATE TABLE IF NOT EXISTS memory_chunks (
                id INTEGER PRIMARY KEY,
                content TEXT,
                embedding BLOB,
                file_path TEXT,
                agent_id TEXT,
                timestamp INTEGER
            )",
            [],
        )
        .map_err(|e| SavantError::Unknown(format!("DB error: {}", e)))?;

        conn.execute(
            "CREATE TABLE IF NOT EXISTS indexed_files (
                path TEXT PRIMARY KEY,
                hash TEXT,
                last_indexed INTEGER
            )",
            [],
        )
        .map_err(|e| SavantError::Unknown(format!("DB error: {}", e)))?;

        Ok(())
    }

    /// Indexes a directory recursively. Uses spawn_blocking for file I/O.
    pub async fn index_directory(
        &self,
        agent_id: &str,
        base_path: &Path,
    ) -> Result<(), SavantError> {
        let agent_id = agent_id.to_string();
        let base_path = base_path.to_path_buf();
        let db_path = self.db_path.clone();

        task::spawn_blocking(move || Self::scan_and_index_blocking(&db_path, &agent_id, &base_path))
            .await
            .map_err(|e| SavantError::Unknown(format!("Task join error: {}", e)))?
    }

    fn scan_and_index_blocking(
        db_path: &Path,
        agent_id: &str,
        base_path: &Path,
    ) -> Result<(), SavantError> {
        tracing::info!(
            "Indexing directory for agent {}: {}",
            agent_id,
            base_path.display()
        );

        let conn = Connection::open(db_path)
            .map_err(|e| SavantError::IoError(std::io::Error::other(e)))?;

        for entry in walkdir::WalkDir::new(base_path)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            if entry.file_type().is_file() {
                let path = entry.path();
                if Self::should_index(path) {
                    Self::index_file_blocking(&conn, agent_id, path)?;
                }
            }
        }

        Ok(())
    }

    fn should_index(path: &Path) -> bool {
        let extension = path.extension().and_then(|s| s.to_str()).unwrap_or("");
        matches!(extension, "md" | "txt" | "json" | "toml")
    }

    fn index_file_blocking(
        conn: &Connection,
        agent_id: &str,
        path: &Path,
    ) -> Result<(), SavantError> {
        let content = std::fs::read_to_string(path)?;
        let hash = blake3::hash(content.as_bytes()).to_hex().to_string();

        let mut stmt = conn
            .prepare("SELECT hash FROM indexed_files WHERE path = ?")
            .map_err(|e| SavantError::Unknown(format!("DB prepare error: {}", e)))?;

        let path_str = path
            .to_str()
            .ok_or_else(|| SavantError::Unknown("Invalid path encoding".to_string()))?;

        let existing_hash: Option<String> = stmt
            .query_row(params![path_str], |row| row.get(0))
            .optional()
            .map_err(|e| SavantError::Unknown(format!("DB query error: {}", e)))?;

        if Some(hash.clone()) == existing_hash {
            return Ok(());
        }

        tracing::info!("Indexing file: {}", path.display());

        conn.execute(
            "INSERT OR REPLACE INTO indexed_files (path, hash, last_indexed) VALUES (?, ?, ?)",
            params![path_str, hash, 0],
        )
        .map_err(|e| SavantError::Unknown(format!("DB error: {}", e)))?;

        conn.execute(
            "INSERT INTO memory_chunks (content, file_path, agent_id, timestamp) VALUES (?, ?, ?, ?)",
            params![content, path_str, agent_id, 0],
        ).map_err(|e| SavantError::Unknown(format!("DB error: {}", e)))?;

        Ok(())
    }

    pub async fn watch_and_index(
        &self,
        agent_id: &str,
        base_path: &Path,
    ) -> Result<(), SavantError> {
        tracing::info!("Starting filesystem watcher for {}", base_path.display());
        self.index_directory(agent_id, base_path).await
    }

    /// Semantic search delegates to full_text_search until embedding service is ready.
    pub fn semantic_search(&self, query: &str, limit: usize) -> Vec<MemoryEntry> {
        self.full_text_search(query)
            .into_iter()
            .take(limit)
            .collect()
    }

    pub fn full_text_search(&self, query: &str) -> Vec<MemoryEntry> {
        let conn = match self.open_connection() {
            Ok(c) => c,
            Err(_) => return Vec::new(),
        };
        let mut stmt = match conn
            .prepare("SELECT id, content, timestamp FROM memory_chunks WHERE content LIKE ?")
        {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };
        let escaped_query = query
            .replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_");
        let rows = match stmt.query_map(params![format!("%{}%", escaped_query)], |row| {
            Ok(MemoryEntry {
                id: row.get(0)?,
                content: row.get(1)?,
                timestamp: row.get(2)?,
                category: crate::types::MemoryCategory::Observation,
                importance: 5,
                associations: Vec::new(),
                embedding: None,
            })
        }) {
            Ok(r) => r,
            Err(_) => return Vec::new(),
        };

        rows.filter_map(|r| r.ok()).collect()
    }
}
