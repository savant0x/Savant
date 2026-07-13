//! Rust language extractor using tree-sitter.

use crate::extract::{ExtractionResult, LanguageExtractor};
use crate::symbols::{Edge, EdgeKind, NodeKind, Symbol};
use std::path::Path;
use tree_sitter::Parser;

pub struct RustExtractor {
    parser: std::sync::Mutex<Parser>,
}

impl Default for RustExtractor {
    fn default() -> Self {
        Self::new()
    }
}

impl RustExtractor {
    #[allow(clippy::disallowed_methods)] // set_language is infallible — grammar compiled in
    pub fn new() -> Self {
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_rust::LANGUAGE.into())
            .expect("Failed to set Rust language");
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
            "function_item" => NodeKind::Function,
            "function_signature_item" => NodeKind::Function,
            "impl_item" => NodeKind::Impl,
            "trait_item" => NodeKind::Trait,
            "struct_item" => NodeKind::Struct,
            "enum_item" => NodeKind::Enum,
            "type_item" => NodeKind::TypeAlias,
            "const_item" => NodeKind::Constant,
            "static_item" => NodeKind::Constant,
            "mod_item" => NodeKind::Module,
            "macro_definition" => NodeKind::Macro,
            _ => return None,
        };

        // Extract the name from the child "identifier" or "type_identifier" node
        let name = Self::extract_name(node, source)?;
        let file_str = file_path.to_string_lossy().to_string();
        let qualified_name = if let Some(parent) = parent_id {
            format!("{}::{}", parent, name)
        } else {
            format!("crate::{}", name)
        };

        let id = format!("{}:{}:{}", file_str, node.start_position().row + 1, name);

        let documentation = Self::extract_doc_comment(node, source);
        let signature = Self::extract_signature(node, source);

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
            language: "rust".to_string(),
            documentation,
            signature,
            parent_id: parent_id.map(|s| s.to_string()),
        })
    }

    fn extract_name(node: tree_sitter::Node, source: &[u8]) -> Option<String> {
        let name_field = match node.kind() {
            "function_item" | "function_signature_item" => "name",
            "impl_item" => return Self::extract_impl_name(node, source),
            "trait_item" | "struct_item" | "enum_item" | "type_item" => "name",
            "const_item" | "static_item" => "name",
            "mod_item" => "name",
            "macro_definition" => "name",
            _ => return None,
        };

        let child = node.child_by_field_name(name_field)?;
        let text = child.utf8_text(source).ok()?;
        Some(text.to_string())
    }

    fn extract_impl_name(node: tree_sitter::Node, source: &[u8]) -> Option<String> {
        // For impl blocks, use the type name
        let type_node = node.child_by_field_name("type")?;
        type_node.utf8_text(source).ok().map(|s| s.to_string())
    }

    fn extract_doc_comment(node: tree_sitter::Node, source: &[u8]) -> Option<String> {
        // Walk previous siblings looking for line_comment nodes that start with "///"
        let mut comments = Vec::new();
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if child.kind() == "attribute_item" || child.kind() == "inner_attribute_item" {
                continue;
            }
            if child.kind() == "line_comment" {
                let text = child.utf8_text(source).ok()?;
                if text.starts_with("///") {
                    comments.push(text.trim_start_matches("///").trim().to_string());
                }
            }
        }

        // Also check preceding siblings
        let mut prev = node.prev_sibling();
        while let Some(sibling) = prev {
            if sibling.kind() == "line_comment" {
                let text = sibling.utf8_text(source).ok()?;
                if text.starts_with("///") {
                    comments.insert(0, text.trim_start_matches("///").trim().to_string());
                }
            } else {
                break;
            }
            prev = sibling.prev_sibling();
        }

        if comments.is_empty() {
            None
        } else {
            Some(comments.join(" "))
        }
    }

    fn extract_signature(node: tree_sitter::Node, source: &[u8]) -> Option<String> {
        match node.kind() {
            "function_item" | "function_signature_item" => {
                // Extract up to the first '{' or ';' for the signature
                let text = node.utf8_text(source).ok()?;
                let sig_end = text
                    .find('{')
                    .unwrap_or(text.find(';').unwrap_or(text.len()));
                Some(text[..sig_end].trim().to_string())
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

        // Walk the AST looking for call_expression nodes
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
        if node.kind() == "call_expression" {
            if let Some(func_node) = node.child_by_field_name("function") {
                if let Ok(text) = func_node.utf8_text(source) {
                    let target_name = text.to_string();
                    let edge_id = format!(
                        "{}:{}:{}->{}",
                        file_path,
                        node.start_position().row + 1,
                        parent_id,
                        target_name
                    );
                    edges.push(Edge {
                        id: edge_id,
                        source_id: parent_id.to_string(),
                        target_id: format!("crate::{}", target_name),
                        kind: EdgeKind::Calls,
                        file_path: file_path.to_string(),
                        line: (node.start_position().row + 1) as u32,
                    });
                }
            }
        }

        // Recurse into children
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            Self::walk_for_calls(child, source, file_path, parent_id, edges);
        }
    }
}

impl LanguageExtractor for RustExtractor {
    fn language(&self) -> &str {
        "rust"
    }

    fn file_extensions(&self) -> &[&str] {
        &["rs"]
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
        let root = tree.root_node();

        Self::walk_top_level(root, source, file_path, None, &mut symbols, &mut edges);

        ExtractionResult { symbols, edges }
    }
}

impl RustExtractor {
    fn walk_top_level(
        node: tree_sitter::Node,
        source: &[u8],
        file_path: &Path,
        parent_id: Option<&str>,
        symbols: &mut Vec<Symbol>,
        edges: &mut Vec<Edge>,
    ) {
        // Check if this node is a symbol we care about
        if let Some(sym) = Self::node_to_symbol(node, source, file_path, parent_id) {
            let sym_id = sym.id.clone();

            // Extract call edges from this symbol's body
            let call_edges = Self::extract_call_edges(node, source, file_path, &sym_id);
            edges.extend(call_edges);

            symbols.push(sym);

            // Recurse into children for nested items (e.g., methods in impl blocks)
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                Self::walk_top_level(child, source, file_path, Some(&sym_id), symbols, edges);
            }
        } else {
            // Not a symbol, but recurse to find nested symbols
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                Self::walk_top_level(child, source, file_path, parent_id, symbols, edges);
            }
        }
    }
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn extract_rust(source: &str) -> ExtractionResult {
        let extractor = RustExtractor::new();
        extractor.extract(source.as_bytes(), &PathBuf::from("src/lib.rs"))
    }

    #[test]
    fn test_extract_function() {
        let result = extract_rust(
            r#"
/// A test function.
fn hello_world() {
    println!("Hello!");
}
"#,
        );
        assert!(!result.symbols.is_empty());
        let sym = &result.symbols[0];
        assert_eq!(sym.name, "hello_world");
        assert_eq!(sym.kind, NodeKind::Function);
        assert_eq!(sym.language, "rust");
    }

    #[test]
    fn test_extract_struct() {
        let result = extract_rust(
            r#"
pub struct MyStruct {
    field: String,
}
"#,
        );
        let sym = result
            .symbols
            .iter()
            .find(|s| s.name == "MyStruct")
            .unwrap();
        assert_eq!(sym.kind, NodeKind::Struct);
    }

    #[test]
    fn test_extract_trait() {
        let result = extract_rust(
            r#"
pub trait MyTrait {
    fn method(&self);
}
"#,
        );
        let sym = result.symbols.iter().find(|s| s.name == "MyTrait").unwrap();
        assert_eq!(sym.kind, NodeKind::Trait);
    }

    #[test]
    fn test_extract_enum() {
        let result = extract_rust(
            r#"
enum Color {
    Red,
    Green,
    Blue,
}
"#,
        );
        let sym = result.symbols.iter().find(|s| s.name == "Color").unwrap();
        assert_eq!(sym.kind, NodeKind::Enum);
    }

    #[test]
    fn test_extract_impl() {
        let result = extract_rust(
            r#"
impl MyStruct {
    fn new() -> Self { Self { field: String::new() } }
    fn method(&self) {}
}
"#,
        );
        let impl_sym = result.symbols.iter().find(|s| s.kind == NodeKind::Impl);
        assert!(impl_sym.is_some());
    }

    #[test]
    fn test_extract_call_edges() {
        let result = extract_rust(
            r#"
fn caller() {
    callee();
}
"#,
        );
        assert!(!result.edges.is_empty());
        let edge = &result.edges[0];
        assert_eq!(edge.kind, EdgeKind::Calls);
    }

    #[test]
    fn test_extract_module() {
        let result = extract_rust(
            r#"
mod my_module {
    fn inner() {}
}
"#,
        );
        let sym = result
            .symbols
            .iter()
            .find(|s| s.name == "my_module")
            .unwrap();
        assert_eq!(sym.kind, NodeKind::Module);
    }

    #[test]
    fn test_doc_comment_extraction() {
        let result = extract_rust(
            r#"
/// This function does something important.
/// It has multiple doc lines.
fn documented() {}
"#,
        );
        let sym = &result.symbols[0];
        assert!(sym.documentation.is_some());
        assert!(sym.documentation.as_ref().unwrap().contains("important"));
    }
}
