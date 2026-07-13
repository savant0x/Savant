//! Call graph traversal and impact radius analysis.

use crate::db::SchemaDb;
use crate::symbols::Symbol;
use std::collections::{HashSet, VecDeque};

/// Compute the impact radius of a symbol change.
///
/// Given a symbol that has changed, find all symbols that may be affected
/// by traversing the call graph (callers of callers, etc.) up to `max_depth`.
///
/// Returns a list of affected symbols, ordered by distance from the changed symbol.
pub fn get_impact_radius(db: &SchemaDb, symbol_id: &str, max_depth: u32) -> Vec<ImpactEntry> {
    let mut visited = HashSet::new();
    let mut result = Vec::new();
    let mut queue = VecDeque::new();

    queue.push_back((symbol_id.to_string(), 0u32));
    visited.insert(symbol_id.to_string());

    while let Some((current_id, depth)) = queue.pop_front() {
        if depth > max_depth {
            continue;
        }

        // Get all callers of this symbol
        match db.get_callers(&current_id) {
            Ok(callers) => {
                for caller in callers {
                    if !visited.contains(&caller.id) {
                        visited.insert(caller.id.clone());
                        result.push(ImpactEntry {
                            symbol: caller.clone(),
                            distance: depth + 1,
                        });
                        queue.push_back((caller.id.clone(), depth + 1));
                    }
                }
            }
            Err(_) => continue,
        }
    }

    // Sort by distance (closest first)
    result.sort_by_key(|e| e.distance);
    result
}

/// An entry in the impact radius result.
#[derive(Debug, Clone)]
pub struct ImpactEntry {
    /// The affected symbol.
    pub symbol: Symbol,
    /// Distance from the changed symbol (1 = direct caller, 2 = caller's caller, etc.).
    pub distance: u32,
}

/// Get the full call chain from a symbol to all its transitive callees.
pub fn get_transitive_callees(db: &SchemaDb, symbol_id: &str, max_depth: u32) -> Vec<Symbol> {
    let mut visited = HashSet::new();
    let mut result = Vec::new();
    let mut queue = VecDeque::new();

    queue.push_back((symbol_id.to_string(), 0u32));
    visited.insert(symbol_id.to_string());

    while let Some((current_id, depth)) = queue.pop_front() {
        if depth > max_depth {
            continue;
        }

        match db.get_callees(&current_id) {
            Ok(callees) => {
                for callee in callees {
                    if !visited.contains(&callee.id) {
                        visited.insert(callee.id.clone());
                        result.push(callee.clone());
                        queue.push_back((callee.id.clone(), depth + 1));
                    }
                }
            }
            Err(_) => continue,
        }
    }

    result
}

/// Build a markdown summary of the call graph around a symbol.
pub fn build_context_markdown(db: &SchemaDb, symbol_id: &str) -> String {
    let mut output = String::new();

    if let Ok(Some(sym)) = db.get_symbol(symbol_id) {
        output.push_str(&format!(
            "## Symbol: `{}` ({:?})\n\n",
            sym.qualified_name, sym.kind
        ));
        output.push_str(&format!(
            "- **File:** {}:{}-{}\n",
            sym.file_path, sym.start_line, sym.end_line
        ));

        if let Some(ref sig) = sym.signature {
            output.push_str(&format!("- **Signature:** `{}`\n", sig));
        }
        if let Some(ref doc) = sym.documentation {
            output.push_str(&format!("- **Documentation:** {}\n", doc));
        }
        output.push('\n');

        // Callers
        if let Ok(callers) = db.get_callers(symbol_id) {
            if !callers.is_empty() {
                output.push_str("### Called by\n\n");
                for caller in &callers {
                    output.push_str(&format!(
                        "- `{}` ({}:{})\n",
                        caller.qualified_name, caller.file_path, caller.start_line
                    ));
                }
                output.push('\n');
            }
        }

        // Callees
        if let Ok(callees) = db.get_callees(symbol_id) {
            if !callees.is_empty() {
                output.push_str("### Calls\n\n");
                for callee in &callees {
                    output.push_str(&format!(
                        "- `{}` ({}:{})\n",
                        callee.qualified_name, callee.file_path, callee.start_line
                    ));
                }
                output.push('\n');
            }
        }
    }

    output
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)]
mod tests {
    use super::*;
    use crate::symbols::{Edge, EdgeKind, NodeKind};

    fn setup_test_graph() -> SchemaDb {
        let db = SchemaDb::open_memory().unwrap();

        // Create symbols: main -> process -> validate, process -> format
        let symbols = vec![
            Symbol {
                id: "src/main.rs:1:main".into(),
                name: "main".into(),
                qualified_name: "crate::main".into(),
                kind: NodeKind::Function,
                file_path: "src/main.rs".into(),
                start_line: 1,
                end_line: 10,
                start_byte: 0,
                end_byte: 100,
                language: "rust".into(),
                documentation: None,
                signature: Some("fn main()".into()),
                parent_id: None,
            },
            Symbol {
                id: "src/lib.rs:1:process".into(),
                name: "process".into(),
                qualified_name: "crate::process".into(),
                kind: NodeKind::Function,
                file_path: "src/lib.rs".into(),
                start_line: 1,
                end_line: 20,
                start_byte: 0,
                end_byte: 200,
                language: "rust".into(),
                documentation: None,
                signature: Some("fn process()".into()),
                parent_id: None,
            },
            Symbol {
                id: "src/validate.rs:1:validate".into(),
                name: "validate".into(),
                qualified_name: "crate::validate".into(),
                kind: NodeKind::Function,
                file_path: "src/validate.rs".into(),
                start_line: 1,
                end_line: 5,
                start_byte: 0,
                end_byte: 50,
                language: "rust".into(),
                documentation: None,
                signature: Some("fn validate()".into()),
                parent_id: None,
            },
            Symbol {
                id: "src/format.rs:1:format".into(),
                name: "format".into(),
                qualified_name: "crate::format".into(),
                kind: NodeKind::Function,
                file_path: "src/format.rs".into(),
                start_line: 1,
                end_line: 5,
                start_byte: 0,
                end_byte: 50,
                language: "rust".into(),
                documentation: None,
                signature: Some("fn format()".into()),
                parent_id: None,
            },
        ];

        for sym in &symbols {
            db.upsert_symbol(sym).unwrap();
        }

        // main -> process, process -> validate, process -> format
        let edges = vec![
            Edge {
                id: "main->process".into(),
                source_id: "src/main.rs:1:main".into(),
                target_id: "src/lib.rs:1:process".into(),
                kind: EdgeKind::Calls,
                file_path: "src/main.rs".into(),
                line: 3,
            },
            Edge {
                id: "process->validate".into(),
                source_id: "src/lib.rs:1:process".into(),
                target_id: "src/validate.rs:1:validate".into(),
                kind: EdgeKind::Calls,
                file_path: "src/lib.rs".into(),
                line: 5,
            },
            Edge {
                id: "process->format".into(),
                source_id: "src/lib.rs:1:process".into(),
                target_id: "src/format.rs:1:format".into(),
                kind: EdgeKind::Calls,
                file_path: "src/lib.rs".into(),
                line: 8,
            },
        ];

        for edge in &edges {
            db.upsert_edge(edge).unwrap();
        }

        db
    }

    #[test]
    fn test_impact_radius_direct() {
        let db = setup_test_graph();
        let impact = get_impact_radius(&db, "src/lib.rs:1:process", 10);
        // process is called by main
        assert_eq!(impact.len(), 1);
        assert_eq!(impact[0].symbol.name, "main");
        assert_eq!(impact[0].distance, 1);
    }

    #[test]
    fn test_impact_radius_deep() {
        let db = setup_test_graph();
        let impact = get_impact_radius(&db, "src/validate.rs:1:validate", 10);
        // validate is called by process, which is called by main
        assert_eq!(impact.len(), 2);
        assert_eq!(impact[0].symbol.name, "process");
        assert_eq!(impact[0].distance, 1);
        assert_eq!(impact[1].symbol.name, "main");
        assert_eq!(impact[1].distance, 2);
    }

    #[test]
    fn test_transitive_callees() {
        let db = setup_test_graph();
        let callees = get_transitive_callees(&db, "src/main.rs:1:main", 10);
        // main -> process -> validate, format
        assert_eq!(callees.len(), 3);
        let names: Vec<_> = callees.iter().map(|s| s.name.as_str()).collect();
        assert!(names.contains(&"process"));
        assert!(names.contains(&"validate"));
        assert!(names.contains(&"format"));
    }

    #[test]
    fn test_context_markdown() {
        let db = setup_test_graph();
        let md = build_context_markdown(&db, "src/lib.rs:1:process");
        assert!(md.contains("process"));
        assert!(md.contains("Called by"));
        assert!(md.contains("Calls"));
    }
}
