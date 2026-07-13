//! Python language extractor using tree-sitter.

use crate::extract::{ExtractionResult, LanguageExtractor};
use crate::symbols::{Edge, EdgeKind, NodeKind, Symbol};
use std::path::Path;
use tree_sitter::Parser;

pub struct PythonExtractor {
    parser: std::sync::Mutex<Parser>,
}

impl Default for PythonExtractor {
    fn default() -> Self {
        Self::new()
    }
}

impl PythonExtractor {
    #[allow(clippy::disallowed_methods)] // set_language is infallible — grammar compiled in
    pub fn new() -> Self {
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_python::LANGUAGE.into())
            .expect("Failed to set Python language");
        Self {
            parser: std::sync::Mutex::new(parser),
        }
    }

    fn node_to_symbol(
        node: tree_sitter::Node,
        source: &[u8],
        file_path: &Path,
        parent_id: Option<&str>,
    ) -> Option<Symbol> {
        let kind = match node.kind() {
            "function_definition" => NodeKind::Function,
            "class_definition" => NodeKind::Class,
            "decorated_definition" => {
                // Look at the child definition
                let mut cursor = node.walk();
                for child in node.children(&mut cursor) {
                    if child.kind() == "function_definition" || child.kind() == "class_definition" {
                        return Self::node_to_symbol(child, source, file_path, parent_id);
                    }
                }
                return None;
            }
            _ => return None,
        };

        let name = Self::extract_name(node, source)?;
        let file_str = file_path.to_string_lossy().to_string();
        let qualified_name = if let Some(parent) = parent_id {
            format!("{}.{}", parent, name)
        } else {
            name.clone()
        };
        let id = format!("{}:{}:{}", file_str, node.start_position().row + 1, name);

        Some(Symbol {
            id,
            name,
            qualified_name,
            kind,
            file_path: file_str,
            start_line: (node.start_position().row + 1) as u32,
            end_line: (node.end_position().row + 1) as u32,
            start_byte: node.start_byte() as u32,
            end_byte: node.end_byte() as u32,
            language: "python".to_string(),
            documentation: Self::extract_docstring(node, source),
            signature: Self::extract_signature(node, source),
            parent_id: parent_id.map(|s| s.to_string()),
        })
    }

    fn extract_name(node: tree_sitter::Node, source: &[u8]) -> Option<String> {
        let child = node.child_by_field_name("name")?;
        child.utf8_text(source).ok().map(|s| s.to_string())
    }

    fn extract_docstring(node: tree_sitter::Node, source: &[u8]) -> Option<String> {
        // Look for a string literal as the first statement in the body
        let body = node.child_by_field_name("body")?;
        let first = body.named_child(0)?;
        if first.kind() == "expression_statement" {
            let inner = first.named_child(0)?;
            if inner.kind() == "string" {
                let text = inner.utf8_text(source).ok()?;
                let cleaned = text.trim_matches(|c| c == '"' || c == '\'').trim();
                return Some(cleaned.to_string());
            }
        }
        None
    }

    fn extract_signature(node: tree_sitter::Node, source: &[u8]) -> Option<String> {
        match node.kind() {
            "function_definition" => {
                let params = node.child_by_field_name("parameters")?;
                let name = node.child_by_field_name("name")?;
                let name_text = name.utf8_text(source).ok()?;
                let params_text = params.utf8_text(source).ok()?;
                let return_type = node
                    .child_by_field_name("return_type")
                    .and_then(|n| n.utf8_text(source).ok())
                    .map(|t| format!(" -> {}", t))
                    .unwrap_or_default();
                Some(format!("def {}{}{}", name_text, params_text, return_type))
            }
            _ => None,
        }
    }

    fn extract_call_edges(
        node: tree_sitter::Node,
        source: &[u8],
        file_path: &Path,
        parent_id: &str,
    ) -> Vec<Edge> {
        let mut edges = Vec::new();
        let file_str = file_path.to_string_lossy().to_string();
        Self::walk_for_calls(node, source, &file_str, parent_id, &mut edges);
        edges
    }

    fn walk_for_calls(
        node: tree_sitter::Node,
        source: &[u8],
        file_path: &str,
        parent_id: &str,
        edges: &mut Vec<Edge>,
    ) {
        if node.kind() == "call" {
            if let Some(func_node) = node.child_by_field_name("function") {
                if let Ok(text) = func_node.utf8_text(source) {
                    let target_name = text.to_string();
                    edges.push(Edge {
                        id: format!(
                            "{}:{}:{}->{}",
                            file_path,
                            node.start_position().row + 1,
                            parent_id,
                            target_name
                        ),
                        source_id: parent_id.to_string(),
                        target_id: target_name,
                        kind: EdgeKind::Calls,
                        file_path: file_path.to_string(),
                        line: (node.start_position().row + 1) as u32,
                    });
                }
            }
        }
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            Self::walk_for_calls(child, source, file_path, parent_id, edges);
        }
    }

    fn walk_top_level(
        node: tree_sitter::Node,
        source: &[u8],
        file_path: &Path,
        parent_id: Option<&str>,
        symbols: &mut Vec<Symbol>,
        edges: &mut Vec<Edge>,
    ) {
        if let Some(sym) = Self::node_to_symbol(node, source, file_path, parent_id) {
            let sym_id = sym.id.clone();
            let call_edges = Self::extract_call_edges(node, source, file_path, &sym_id);
            edges.extend(call_edges);
            symbols.push(sym);

            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                Self::walk_top_level(child, source, file_path, Some(&sym_id), symbols, edges);
            }
        } else {
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                Self::walk_top_level(child, source, file_path, parent_id, symbols, edges);
            }
        }
    }
}

impl LanguageExtractor for PythonExtractor {
    fn language(&self) -> &str {
        "python"
    }

    fn file_extensions(&self) -> &[&str] {
        &["py", "pyi"]
    }

    fn extract(&self, source: &[u8], file_path: &Path) -> ExtractionResult {
        let mut parser = self.parser.lock().unwrap_or_else(|e| e.into_inner());
        let tree = match parser.parse(source, None) {
            Some(tree) => tree,
            None => {
                return ExtractionResult {
                    symbols: vec![],
                    edges: vec![],
                }
            }
        };

        let mut symbols = Vec::new();
        let mut edges = Vec::new();
        Self::walk_top_level(
            tree.root_node(),
            source,
            file_path,
            None,
            &mut symbols,
            &mut edges,
        );

        ExtractionResult { symbols, edges }
    }
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn extract_py(source: &str) -> ExtractionResult {
        let extractor = PythonExtractor::new();
        extractor.extract(source.as_bytes(), &PathBuf::from("src/main.py"))
    }

    #[test]
    fn test_extract_function() {
        let result = extract_py(
            r#"
def hello():
    print("hello")
"#,
        );
        assert!(!result.symbols.is_empty());
        assert_eq!(result.symbols[0].name, "hello");
        assert_eq!(result.symbols[0].kind, NodeKind::Function);
    }

    #[test]
    fn test_extract_class() {
        let result = extract_py(
            r#"
class MyClass:
    def method(self):
        pass
"#,
        );
        let sym = result.symbols.iter().find(|s| s.name == "MyClass").unwrap();
        assert_eq!(sym.kind, NodeKind::Class);
    }

    #[test]
    fn test_docstring_extraction() {
        let result = extract_py(
            r#"
def documented():
    """This function does something."""
    pass
"#,
        );
        let sym = &result.symbols[0];
        assert!(sym.documentation.is_some());
        assert!(sym.documentation.as_ref().unwrap().contains("something"));
    }

    #[test]
    fn test_signature_extraction() {
        let result = extract_py(
            r#"
def add(a: int, b: int) -> int:
    return a + b
"#,
        );
        let sym = &result.symbols[0];
        assert!(sym.signature.is_some());
        let sig = sym.signature.as_ref().unwrap();
        assert!(sig.contains("add"));
        assert!(sig.contains("int"));
    }
}
