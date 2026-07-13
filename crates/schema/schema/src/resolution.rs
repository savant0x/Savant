//! Import resolution, path alias resolution, and framework pattern detection.
//!
//! This module resolves import statements to their source symbols,
//! builds a map from imported names to symbol IDs, and detects
//! framework-specific patterns (Axum routes, Express handlers, etc.).

use crate::db::SchemaDb;
use crate::symbols::{Edge, EdgeKind, NodeKind, Symbol};
use std::collections::HashMap;

/// Result of resolving imports in a project.
#[derive(Debug)]
pub struct ResolutionResult {
    /// New edges representing resolved imports.
    pub edges: Vec<Edge>,
    /// Map from local alias → resolved qualified name.
    pub alias_map: HashMap<String, String>,
    /// Detected framework patterns.
    pub patterns: Vec<FrameworkPattern>,
}

/// A detected framework pattern.
#[derive(Debug, Clone)]
pub struct FrameworkPattern {
    /// The framework name (e.g., "axum", "express", "fastapi").
    pub framework: String,
    /// The pattern type (e.g., "route", "handler", "middleware").
    pub pattern_type: String,
    /// The symbol that uses this pattern.
    pub symbol_id: String,
    /// Additional context (e.g., route path "/api/users").
    pub detail: Option<String>,
}

/// Resolve all imports in the database and build a name → symbol map.
///
/// This walks all symbols in the database, finds import statements,
/// and creates `ResolvedImport` edges linking the import to the source symbol.
pub fn resolve_imports(db: &SchemaDb) -> ResolutionResult {
    let mut edges = Vec::new();
    let mut alias_map = HashMap::new();
    let mut patterns = Vec::new();

    // Build a map of qualified_name → symbol_id for all symbols
    let mut name_index: HashMap<String, String> = HashMap::new();

    // Index all symbols by name and qualified_name
    let all_files = get_all_files(db);
    for file_path in &all_files {
        if let Ok(syms) = db.get_symbols_in_file(file_path) {
            for sym in &syms {
                // Don't index import symbols — they resolve to other symbols
                if sym.kind == NodeKind::Import {
                    continue;
                }
                name_index.insert(sym.name.clone(), sym.id.clone());
                name_index.insert(sym.qualified_name.clone(), sym.id.clone());
            }
        }
    }

    // Find all import-type symbols and resolve them
    for file_path in &all_files {
        if let Ok(syms) = db.get_symbols_in_file(file_path) {
            for sym in &syms {
                if sym.kind == NodeKind::Import {
                    resolve_single_import(sym, &name_index, file_path, &mut edges, &mut alias_map);
                }

                // Detect framework patterns
                detect_framework_patterns(sym, &mut patterns);
            }
        }
    }

    // Store resolved edges
    for edge in &edges {
        if let Err(e) = db.upsert_edge(edge) {
            tracing::warn!(
                "[resolution] Failed to store resolved edge {}: {}",
                edge.id,
                e
            );
        }
    }

    ResolutionResult {
        edges,
        alias_map,
        patterns,
    }
}

fn resolve_single_import(
    import_sym: &Symbol,
    name_index: &HashMap<String, String>,
    file_path: &str,
    edges: &mut Vec<Edge>,
    alias_map: &mut HashMap<String, String>,
) {
    // The import's name is the imported item (e.g., "MyStruct" from "use crate::module::MyStruct")
    let imported_name = &import_sym.name;
    let qualified = &import_sym.qualified_name;

    // Try to find the source symbol by matching against the qualified path
    // For Rust: "use crate::module::Item" → look for "crate::module::Item" in name_index
    // For TypeScript: "import { Item } from './module'" → look for "Item" in name_index
    // For Python: "from module import Item" → look for "Item" in name_index

    let target_id = if let Some(id) = name_index.get(qualified) {
        if id != &import_sym.id {
            Some(id.clone())
        } else {
            None
        }
    } else if let Some(id) = name_index.get(imported_name) {
        if id != &import_sym.id {
            Some(id.clone())
        } else {
            None
        }
    } else {
        // Try partial match: strip "crate::" prefix and match
        let stripped = qualified.strip_prefix("crate::").unwrap_or(qualified);
        name_index.get(stripped).and_then(|id| {
            if id != &import_sym.id {
                Some(id.clone())
            } else {
                None
            }
        })
    };

    if let Some(target_id) = target_id {
        let edge_id = format!("import:{}->{}", import_sym.id, target_id);
        edges.push(Edge {
            id: edge_id,
            source_id: import_sym.id.clone(),
            target_id: target_id.clone(),
            kind: EdgeKind::Imports,
            file_path: file_path.to_string(),
            line: import_sym.start_line,
        });

        alias_map.insert(imported_name.clone(), target_id);
    }
}

/// Detect framework-specific patterns from symbol names and attributes.
fn detect_framework_patterns(sym: &Symbol, patterns: &mut Vec<FrameworkPattern>) {
    let sig = sym.signature.as_deref().unwrap_or("");
    let doc = sym.documentation.as_deref().unwrap_or("");

    // Axum patterns
    if sig.contains("Router")
        || sig.contains("axum::")
        || doc.contains("#[get]")
        || doc.contains("#[post]")
    {
        patterns.push(FrameworkPattern {
            framework: "axum".to_string(),
            pattern_type: if sig.contains("Router") {
                "route"
            } else {
                "handler"
            }
            .to_string(),
            symbol_id: sym.id.clone(),
            detail: extract_route_path(sig, doc),
        });
    }

    // Express patterns
    if sig.contains("app.get") || sig.contains("app.post") || sig.contains("router.") {
        patterns.push(FrameworkPattern {
            framework: "express".to_string(),
            pattern_type: "route".to_string(),
            symbol_id: sym.id.clone(),
            detail: extract_express_path(sig),
        });
    }

    // FastAPI patterns
    if sig.contains("@app.")
        || sig.contains("APIRouter")
        || doc.contains("@app.get")
        || doc.contains("@app.post")
    {
        patterns.push(FrameworkPattern {
            framework: "fastapi".to_string(),
            pattern_type: "route".to_string(),
            symbol_id: sym.id.clone(),
            detail: extract_fastapi_path(sig, doc),
        });
    }

    // Actix patterns
    if sig.contains("actix_web") || doc.contains("#[get]") || doc.contains("#[post]") {
        patterns.push(FrameworkPattern {
            framework: "actix".to_string(),
            pattern_type: "handler".to_string(),
            symbol_id: sym.id.clone(),
            detail: extract_route_path(sig, doc),
        });
    }
}

fn extract_route_path(sig: &str, doc: &str) -> Option<String> {
    // Look for path in quotes: "/api/users", "/health"
    for text in &[sig, doc] {
        if let Some(start) = text.find('"') {
            let rest = &text[start + 1..];
            if let Some(end) = rest.find('"') {
                let path = &rest[..end];
                if path.starts_with('/') {
                    return Some(path.to_string());
                }
            }
        }
    }
    None
}

fn extract_express_path(sig: &str) -> Option<String> {
    // Look for: app.get("/path", ...) or router.post("/path", ...)
    if let Some(start) = sig.find("(\"") {
        let rest = &sig[start + 2..];
        if let Some(end) = rest.find('"') {
            return Some(rest[..end].to_string());
        }
    }
    None
}

fn extract_fastapi_path(sig: &str, doc: &str) -> Option<String> {
    // Look for: @app.get("/path") or @app.post("/path")
    for text in &[sig, doc] {
        if let Some(start) = text.find("(\"") {
            let rest = &text[start + 2..];
            if let Some(end) = rest.find('"') {
                let path = &rest[..end];
                if path.starts_with('/') {
                    return Some(path.to_string());
                }
            }
        }
    }
    None
}

fn get_all_files(db: &SchemaDb) -> Vec<String> {
    db.get_all_file_paths().unwrap_or_default()
}

/// Build a markdown summary of detected framework patterns.
pub fn build_framework_context(patterns: &[FrameworkPattern]) -> String {
    if patterns.is_empty() {
        return String::new();
    }

    let mut output = String::from("## Framework Patterns\n\n");

    // Group by framework
    let mut by_framework: HashMap<&str, Vec<&FrameworkPattern>> = HashMap::new();
    for p in patterns {
        by_framework
            .entry(p.framework.as_str())
            .or_default()
            .push(p);
    }

    for (framework, pats) in &by_framework {
        output.push_str(&format!("### {}\n\n", framework));
        for p in pats {
            if let Some(ref detail) = p.detail {
                output.push_str(&format!(
                    "- {} `{}` ({})\n",
                    p.pattern_type, p.symbol_id, detail
                ));
            } else {
                output.push_str(&format!("- {} `{}`\n", p.pattern_type, p.symbol_id));
            }
        }
        output.push('\n');
    }

    output
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)]
mod tests {
    use super::*;
    use crate::symbols::{EdgeKind, NodeKind};

    #[test]
    fn test_resolve_import_found() {
        let db = SchemaDb::open_memory().unwrap();

        // Create source symbol
        let source = Symbol {
            id: "src/util.rs:1:helper".into(),
            name: "helper".into(),
            qualified_name: "crate::util::helper".into(),
            kind: NodeKind::Function,
            file_path: "src/util.rs".into(),
            start_line: 1,
            end_line: 5,
            start_byte: 0,
            end_byte: 50,
            language: "rust".into(),
            documentation: None,
            signature: None,
            parent_id: None,
        };
        db.upsert_symbol(&source).unwrap();

        // Create import symbol
        let import = Symbol {
            id: "src/main.rs:1:helper".into(),
            name: "helper".into(),
            qualified_name: "crate::util::helper".into(),
            kind: NodeKind::Import,
            file_path: "src/main.rs".into(),
            start_line: 1,
            end_line: 1,
            start_byte: 0,
            end_byte: 30,
            language: "rust".into(),
            documentation: None,
            signature: None,
            parent_id: None,
        };
        db.upsert_symbol(&import).unwrap();

        let result = resolve_imports(&db);
        assert_eq!(result.edges.len(), 1);
        assert_eq!(result.edges[0].kind, EdgeKind::Imports);
        assert_eq!(
            result.alias_map.get("helper"),
            Some(&"src/util.rs:1:helper".to_string())
        );
    }

    #[test]
    fn test_resolve_import_not_found() {
        let db = SchemaDb::open_memory().unwrap();

        let import = Symbol {
            id: "src/main.rs:1:unknown".into(),
            name: "unknown".into(),
            qualified_name: "crate::nonexistent::unknown".into(),
            kind: NodeKind::Import,
            file_path: "src/main.rs".into(),
            start_line: 1,
            end_line: 1,
            start_byte: 0,
            end_byte: 30,
            language: "rust".into(),
            documentation: None,
            signature: None,
            parent_id: None,
        };
        db.upsert_symbol(&import).unwrap();

        let result = resolve_imports(&db);
        assert!(result.edges.is_empty());
        assert!(result.alias_map.is_empty());
    }

    #[test]
    fn test_framework_pattern_axum() {
        let mut patterns = Vec::new();
        let sym = Symbol {
            id: "src/routes.rs:1:get_users".into(),
            name: "get_users".into(),
            qualified_name: "crate::routes::get_users".into(),
            kind: NodeKind::Function,
            file_path: "src/routes.rs".into(),
            start_line: 1,
            end_line: 5,
            start_byte: 0,
            end_byte: 50,
            language: "rust".into(),
            documentation: None,
            signature: Some("async fn get_users() -> impl axum::Router".into()),
            parent_id: None,
        };
        detect_framework_patterns(&sym, &mut patterns);
        assert_eq!(patterns.len(), 1);
        assert_eq!(patterns[0].framework, "axum");
    }

    #[test]
    fn test_build_framework_context_empty() {
        let result = build_framework_context(&[]);
        assert!(result.is_empty());
    }
}
