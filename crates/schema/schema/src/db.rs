//! SQLite storage for symbols, edges, and files with FTS5 search.

use crate::symbols::{Edge, IndexedFile, NodeKind, Symbol};
use rusqlite::{params, Connection, Result as SqlResult};
use std::path::Path;
use std::sync::Mutex;

/// Database for storing and querying the code graph.
///
/// Wraps `rusqlite::Connection` in a `Mutex` to make `SchemaDb` `Send + Sync`.
pub struct SchemaDb {
    conn: Mutex<Connection>,
}

impl SchemaDb {
    /// Acquire the database connection lock.
    /// Mutex poison is converted to a rusqlite error (unrecoverable state).
    fn conn(&self) -> SqlResult<std::sync::MutexGuard<'_, Connection>> {
        self.conn
            .lock()
            .map_err(|_| rusqlite::Error::InvalidParameterName("Database lock poisoned".into()))
    }

    /// Open or create a database at the given path.
    pub fn open(db_path: &Path) -> SqlResult<Self> {
        let conn = Connection::open(db_path)?;
        let db = Self {
            conn: Mutex::new(conn),
        };
        db.init_schema()?;
        Ok(db)
    }

    /// Open an in-memory database (for testing).
    pub fn open_memory() -> SqlResult<Self> {
        let conn = Connection::open_in_memory()?;
        let db = Self {
            conn: Mutex::new(conn),
        };
        db.init_schema()?;
        Ok(db)
    }

    fn init_schema(&self) -> SqlResult<()> {
        let conn = self.conn()?;
        conn.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS symbols (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                qualified_name TEXT NOT NULL,
                kind TEXT NOT NULL,
                file_path TEXT NOT NULL,
                start_line INTEGER NOT NULL,
                end_line INTEGER NOT NULL,
                start_byte INTEGER NOT NULL,
                end_byte INTEGER NOT NULL,
                language TEXT NOT NULL,
                documentation TEXT,
                signature TEXT,
                parent_id TEXT
            );

            CREATE TABLE IF NOT EXISTS edges (
                id TEXT PRIMARY KEY,
                source_id TEXT NOT NULL,
                target_id TEXT NOT NULL,
                kind TEXT NOT NULL,
                file_path TEXT NOT NULL,
                line INTEGER NOT NULL
            );

            CREATE TABLE IF NOT EXISTS files (
                path TEXT PRIMARY KEY,
                language TEXT NOT NULL,
                mtime INTEGER NOT NULL,
                content_hash TEXT NOT NULL
            );

            CREATE INDEX IF NOT EXISTS idx_symbols_file ON symbols(file_path);
            CREATE INDEX IF NOT EXISTS idx_symbols_kind ON symbols(kind);
            CREATE INDEX IF NOT EXISTS idx_symbols_name ON symbols(name);
            CREATE INDEX IF NOT EXISTS idx_edges_source ON edges(source_id);
            CREATE INDEX IF NOT EXISTS idx_edges_target ON edges(target_id);
            CREATE INDEX IF NOT EXISTS idx_edges_kind ON edges(kind);

            CREATE VIRTUAL TABLE IF NOT EXISTS symbols_fts USING fts5(
                name, qualified_name, documentation, signature,
                content='symbols',
                content_rowid='rowid'
            );

            CREATE TRIGGER IF NOT EXISTS symbols_ai AFTER INSERT ON symbols BEGIN
                INSERT INTO symbols_fts(rowid, name, qualified_name, documentation, signature)
                VALUES (new.rowid, new.name, new.qualified_name, new.documentation, new.signature);
            END;

            CREATE TRIGGER IF NOT EXISTS symbols_ad AFTER DELETE ON symbols BEGIN
                INSERT INTO symbols_fts(symbols_fts, rowid, name, qualified_name, documentation, signature)
                VALUES ('delete', old.rowid, old.name, old.qualified_name, old.documentation, old.signature);
            END;

            CREATE TRIGGER IF NOT EXISTS symbols_au AFTER UPDATE ON symbols BEGIN
                INSERT INTO symbols_fts(symbols_fts, rowid, name, qualified_name, documentation, signature)
                VALUES ('delete', old.rowid, old.name, old.qualified_name, old.documentation, old.signature);
                INSERT INTO symbols_fts(rowid, name, qualified_name, documentation, signature)
                VALUES (new.rowid, new.name, new.qualified_name, new.documentation, new.signature);
            END;
            ",
        )?;
        Ok(())
    }

    /// Insert or replace a symbol.
    pub fn upsert_symbol(&self, sym: &Symbol) -> SqlResult<()> {
        let conn = self.conn()?;
        conn.execute(
            "INSERT OR REPLACE INTO symbols (id, name, qualified_name, kind, file_path, start_line, end_line, start_byte, end_byte, language, documentation, signature, parent_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
            params![
                sym.id,
                sym.name,
                sym.qualified_name,
                serde_json::to_string(&sym.kind).unwrap_or_default(),
                sym.file_path,
                sym.start_line,
                sym.end_line,
                sym.start_byte,
                sym.end_byte,
                sym.language,
                sym.documentation,
                sym.signature,
                sym.parent_id,
            ],
        )?;
        Ok(())
    }

    /// Insert or replace an edge.
    pub fn upsert_edge(&self, edge: &Edge) -> SqlResult<()> {
        let conn = self.conn()?;
        conn.execute(
            "INSERT OR REPLACE INTO edges (id, source_id, target_id, kind, file_path, line)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                edge.id,
                edge.source_id,
                edge.target_id,
                serde_json::to_string(&edge.kind).unwrap_or_default(),
                edge.file_path,
                edge.line,
            ],
        )?;
        Ok(())
    }

    /// Insert or replace a file record.
    pub fn upsert_file(&self, file: &IndexedFile) -> SqlResult<()> {
        let conn = self.conn()?;
        conn.execute(
            "INSERT OR REPLACE INTO files (path, language, mtime, content_hash)
             VALUES (?1, ?2, ?3, ?4)",
            params![file.path, file.language, file.mtime, file.content_hash],
        )?;
        Ok(())
    }

    /// Delete all symbols and edges for a file (for re-indexing).
    pub fn delete_file_data(&self, file_path: &str) -> SqlResult<()> {
        let conn = self.conn()?;
        conn.execute(
            "DELETE FROM symbols WHERE file_path = ?1",
            params![file_path],
        )?;
        conn.execute("DELETE FROM edges WHERE file_path = ?1", params![file_path])?;
        conn.execute("DELETE FROM files WHERE path = ?1", params![file_path])?;
        Ok(())
    }

    /// Get a symbol by ID.
    pub fn get_symbol(&self, id: &str) -> SqlResult<Option<Symbol>> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare(
            "SELECT id, name, qualified_name, kind, file_path, start_line, end_line, start_byte, end_byte, language, documentation, signature, parent_id FROM symbols WHERE id = ?1",
        )?;
        let mut rows = stmt.query_map(params![id], |row| {
            Ok(Symbol {
                id: row.get(0)?,
                name: row.get(1)?,
                qualified_name: row.get(2)?,
                kind: serde_json::from_str(&row.get::<_, String>(3)?).unwrap_or(NodeKind::Function),
                file_path: row.get(4)?,
                start_line: row.get(5)?,
                end_line: row.get(6)?,
                start_byte: row.get(7)?,
                end_byte: row.get(8)?,
                language: row.get(9)?,
                documentation: row.get(10)?,
                signature: row.get(11)?,
                parent_id: row.get(12)?,
            })
        })?;
        rows.next().transpose()
    }

    /// Get all symbols in a file.
    pub fn get_symbols_in_file(&self, file_path: &str) -> SqlResult<Vec<Symbol>> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare(
            "SELECT id, name, qualified_name, kind, file_path, start_line, end_line, start_byte, end_byte, language, documentation, signature, parent_id FROM symbols WHERE file_path = ?1 ORDER BY start_line",
        )?;
        let rows = stmt.query_map(params![file_path], |row| {
            Ok(Symbol {
                id: row.get(0)?,
                name: row.get(1)?,
                qualified_name: row.get(2)?,
                kind: serde_json::from_str(&row.get::<_, String>(3)?).unwrap_or(NodeKind::Function),
                file_path: row.get(4)?,
                start_line: row.get(5)?,
                end_line: row.get(6)?,
                start_byte: row.get(7)?,
                end_byte: row.get(8)?,
                language: row.get(9)?,
                documentation: row.get(10)?,
                signature: row.get(11)?,
                parent_id: row.get(12)?,
            })
        })?;
        rows.collect()
    }

    /// Get all callers of a symbol (incoming Calls edges).
    pub fn get_callers(&self, symbol_id: &str) -> SqlResult<Vec<Symbol>> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare(
            "SELECT s.id, s.name, s.qualified_name, s.kind, s.file_path, s.start_line, s.end_line, s.start_byte, s.end_byte, s.language, s.documentation, s.signature, s.parent_id
             FROM symbols s
             JOIN edges e ON s.id = e.source_id
             WHERE e.target_id = ?1 AND e.kind = '\"Calls\"'
             ORDER BY s.file_path, s.start_line",
        )?;
        let rows = stmt.query_map(params![symbol_id], |row| {
            Ok(Symbol {
                id: row.get(0)?,
                name: row.get(1)?,
                qualified_name: row.get(2)?,
                kind: serde_json::from_str(&row.get::<_, String>(3)?).unwrap_or(NodeKind::Function),
                file_path: row.get(4)?,
                start_line: row.get(5)?,
                end_line: row.get(6)?,
                start_byte: row.get(7)?,
                end_byte: row.get(8)?,
                language: row.get(9)?,
                documentation: row.get(10)?,
                signature: row.get(11)?,
                parent_id: row.get(12)?,
            })
        })?;
        rows.collect()
    }

    /// Get all callees of a symbol (outgoing Calls edges).
    pub fn get_callees(&self, symbol_id: &str) -> SqlResult<Vec<Symbol>> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare(
            "SELECT s.id, s.name, s.qualified_name, s.kind, s.file_path, s.start_line, s.end_line, s.start_byte, s.end_byte, s.language, s.documentation, s.signature, s.parent_id
             FROM symbols s
             JOIN edges e ON s.id = e.target_id
             WHERE e.source_id = ?1 AND e.kind = '\"Calls\"'
             ORDER BY s.file_path, s.start_line",
        )?;
        let rows = stmt.query_map(params![symbol_id], |row| {
            Ok(Symbol {
                id: row.get(0)?,
                name: row.get(1)?,
                qualified_name: row.get(2)?,
                kind: serde_json::from_str(&row.get::<_, String>(3)?).unwrap_or(NodeKind::Function),
                file_path: row.get(4)?,
                start_line: row.get(5)?,
                end_line: row.get(6)?,
                start_byte: row.get(7)?,
                end_byte: row.get(8)?,
                language: row.get(9)?,
                documentation: row.get(10)?,
                signature: row.get(11)?,
                parent_id: row.get(12)?,
            })
        })?;
        rows.collect()
    }

    /// Full-text search over symbols.
    pub fn search_symbols(&self, query: &str, limit: u32) -> SqlResult<Vec<Symbol>> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare(
            "SELECT s.id, s.name, s.qualified_name, s.kind, s.file_path, s.start_line, s.end_line, s.start_byte, s.end_byte, s.language, s.documentation, s.signature, s.parent_id
             FROM symbols s
             JOIN symbols_fts fts ON s.rowid = fts.rowid
             WHERE symbols_fts MATCH ?1
             ORDER BY rank
             LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![query, limit], |row| {
            Ok(Symbol {
                id: row.get(0)?,
                name: row.get(1)?,
                qualified_name: row.get(2)?,
                kind: serde_json::from_str(&row.get::<_, String>(3)?).unwrap_or(NodeKind::Function),
                file_path: row.get(4)?,
                start_line: row.get(5)?,
                end_line: row.get(6)?,
                start_byte: row.get(7)?,
                end_byte: row.get(8)?,
                language: row.get(9)?,
                documentation: row.get(10)?,
                signature: row.get(11)?,
                parent_id: row.get(12)?,
            })
        })?;
        rows.collect()
    }

    /// Get the file record for a path.
    pub fn get_file(&self, path: &str) -> SqlResult<Option<IndexedFile>> {
        let conn = self.conn()?;
        let mut stmt =
            conn.prepare("SELECT path, language, mtime, content_hash FROM files WHERE path = ?1")?;
        let mut rows = stmt.query_map(params![path], |row| {
            Ok(IndexedFile {
                path: row.get(0)?,
                language: row.get(1)?,
                mtime: row.get(2)?,
                content_hash: row.get(3)?,
            })
        })?;
        rows.next().transpose()
    }

    /// Get all file paths from the symbols table.
    pub fn get_all_file_paths(&self) -> SqlResult<Vec<String>> {
        let conn = self.conn()?;
        let mut stmt = conn.prepare("SELECT DISTINCT file_path FROM symbols ORDER BY file_path")?;
        let rows = stmt.query_map([], |row| row.get(0))?;
        rows.collect()
    }

    /// Get total symbol count.
    pub fn symbol_count(&self) -> SqlResult<u32> {
        let conn = self.conn()?;
        conn.query_row("SELECT COUNT(*) FROM symbols", [], |row| row.get(0))
    }

    /// Get total edge count.
    pub fn edge_count(&self) -> SqlResult<u32> {
        let conn = self.conn()?;
        conn.query_row("SELECT COUNT(*) FROM edges", [], |row| row.get(0))
    }

    /// Get total file count.
    pub fn file_count(&self) -> SqlResult<u32> {
        let conn = self.conn()?;
        conn.query_row("SELECT COUNT(*) FROM files", [], |row| row.get(0))
    }

    /// Clear all data.
    pub fn clear(&self) -> SqlResult<()> {
        let conn = self.conn()?;
        conn.execute_batch("DELETE FROM symbols; DELETE FROM edges; DELETE FROM files;")?;
        Ok(())
    }
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)]
mod tests {
    use super::*;
    use crate::symbols::{EdgeKind, NodeKind};

    fn test_symbol(name: &str, file: &str) -> Symbol {
        Symbol {
            id: format!("{}::{}", file, name),
            name: name.to_string(),
            qualified_name: format!("crate::{}", name),
            kind: NodeKind::Function,
            file_path: file.to_string(),
            start_line: 1,
            end_line: 10,
            start_byte: 0,
            end_byte: 100,
            language: "rust".to_string(),
            documentation: Some(format!("Documentation for {}", name)),
            signature: Some(format!("fn {}()", name)),
            parent_id: None,
        }
    }

    #[test]
    fn test_create_db() {
        let db = SchemaDb::open_memory().unwrap();
        assert_eq!(db.symbol_count().unwrap(), 0);
    }

    #[test]
    fn test_upsert_and_get_symbol() {
        let db = SchemaDb::open_memory().unwrap();
        let sym = test_symbol("my_func", "src/lib.rs");
        db.upsert_symbol(&sym).unwrap();

        let got = db.get_symbol(&sym.id).unwrap().unwrap();
        assert_eq!(got.name, "my_func");
        assert_eq!(got.kind, NodeKind::Function);
    }

    #[test]
    fn test_get_symbols_in_file() {
        let db = SchemaDb::open_memory().unwrap();
        db.upsert_symbol(&test_symbol("func_a", "src/lib.rs"))
            .unwrap();
        db.upsert_symbol(&test_symbol("func_b", "src/lib.rs"))
            .unwrap();
        db.upsert_symbol(&test_symbol("func_c", "src/other.rs"))
            .unwrap();

        let syms = db.get_symbols_in_file("src/lib.rs").unwrap();
        assert_eq!(syms.len(), 2);
    }

    #[test]
    fn test_edges_and_callers() {
        let db = SchemaDb::open_memory().unwrap();
        let caller = test_symbol("caller", "src/lib.rs");
        let callee = test_symbol("callee", "src/util.rs");
        db.upsert_symbol(&caller).unwrap();
        db.upsert_symbol(&callee).unwrap();

        let edge = Edge {
            id: format!("{}->{}", caller.id, callee.id),
            source_id: caller.id.clone(),
            target_id: callee.id.clone(),
            kind: EdgeKind::Calls,
            file_path: "src/lib.rs".to_string(),
            line: 5,
        };
        db.upsert_edge(&edge).unwrap();

        let callers = db.get_callers(&callee.id).unwrap();
        assert_eq!(callers.len(), 1);
        assert_eq!(callers[0].name, "caller");

        let callees = db.get_callees(&caller.id).unwrap();
        assert_eq!(callees.len(), 1);
        assert_eq!(callees[0].name, "callee");
    }

    #[test]
    fn test_fts_search() {
        let db = SchemaDb::open_memory().unwrap();
        let mut sym = test_symbol("process_request", "src/handler.rs");
        sym.documentation = Some("Handles incoming HTTP requests".to_string());
        db.upsert_symbol(&sym).unwrap();

        let results = db.search_symbols("request", 10).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "process_request");
    }

    #[test]
    fn test_delete_file_data() {
        let db = SchemaDb::open_memory().unwrap();
        db.upsert_symbol(&test_symbol("func_a", "src/lib.rs"))
            .unwrap();
        db.upsert_symbol(&test_symbol("func_b", "src/lib.rs"))
            .unwrap();

        db.delete_file_data("src/lib.rs").unwrap();
        assert_eq!(db.get_symbols_in_file("src/lib.rs").unwrap().len(), 0);
    }

    #[test]
    fn test_file_tracking() {
        let db = SchemaDb::open_memory().unwrap();
        let file = IndexedFile {
            path: "src/lib.rs".to_string(),
            language: "rust".to_string(),
            mtime: 1234567890,
            content_hash: "abc123".to_string(),
        };
        db.upsert_file(&file).unwrap();

        let got = db.get_file("src/lib.rs").unwrap().unwrap();
        assert_eq!(got.language, "rust");
        assert_eq!(got.mtime, 1234567890);
    }
}
