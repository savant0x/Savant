//! TypeScript/JavaScript language extractor using tree-sitter.

use crate::extract::{ExtractionResult, LanguageExtractor};
use crate::symbols::{Edge, EdgeKind, NodeKind, Symbol};
use std::path::Path;
use tree_sitter::Parser;

pub struct TypeScriptExtractor {
    parser: std::sync::Mutex<Parser>,
}

impl Default for TypeScriptExtractor {
    fn default() -> Self {
        Self::new()
    }
}

impl TypeScriptExtractor {
    #[allow(clippy::disallowed_methods)] // set_language is infallible — grammar compiled in
    pub fn new() -> Self {
        let mut parser = Parser::new();
        parser
            .set_language(&tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into())
            .expect("Failed to set TypeScript language");
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
            "function_declaration" | "function" => NodeKind::Function,
            "method_definition" | "method_signature" => NodeKind::Method,
            "class_declaration" => NodeKind::Class,
            "interface_declaration" => NodeKind::Interface,
            "type_alias_declaration" => NodeKind::TypeAlias,
            "enum_declaration" => NodeKind::Enum,
            "variable_declarator" => NodeKind::Variable,
            "export_statement" => return Self::extract_export(node, source, file_path, parent_id),
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
            language: "typescript".to_string(),
            documentation: Self::extract_jsdoc(node, source),
            signature: Self::extract_signature(node, source),
            parent_id: parent_id.map(|s| s.to_string()),
        })
    }

    fn extract_export(
        node: tree_sitter::Node,
        source: &[u8],
        file_path: &Path,
        parent_id: Option<&str>,
    ) -> Option<Symbol> {
        // For export statements, look at the declaration inside
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if let Some(sym) = Self::node_to_symbol(child, source, file_path, parent_id) {
                return Some(sym);
            }
        }
        None
    }

    fn extract_name(node: tree_sitter::Node, source: &[u8]) -> Option<String> {
        let name_field = match node.kind() {
            "function_declaration" | "function" => "name",
            "method_definition" | "method_signature" => "name",
            "class_declaration" => "name",
            "interface_declaration" => "name",
            "type_alias_declaration" => "name",
            "enum_declaration" => "name",
            "variable_declarator" => "name",
            _ => return None,
        };
        let child = node.child_by_field_name(name_field)?;
        child.utf8_text(source).ok().map(|s| s.to_string())
    }

    fn extract_jsdoc(node: tree_sitter::Node, source: &[u8]) -> Option<String> {
        let mut prev = node.prev_sibling();
        while let Some(sibling) = prev {
            if sibling.kind() == "comment" {
                let text = sibling.utf8_text(source).ok()?;
                if text.starts_with("/**") {
                    let cleaned: String = text
                        .lines()
                        .map(|l| {
                            l.trim_start_matches("///")
                                .trim_start_matches("/**")
                                .trim_start_matches("*/")
                                .trim_start_matches('*')
                                .trim()
                        })
                        .filter(|l| !l.is_empty())
                        .collect::<Vec<_>>()
                        .join(" ");
                    return Some(cleaned);
                }
            } else {
                break;
            }
            prev = sibling.prev_sibling();
        }
        None
    }

    fn extract_signature(node: tree_sitter::Node, source: &[u8]) -> Option<String> {
        match node.kind() {
            "function_declaration" | "method_definition" => {
                let text = node.utf8_text(source).ok()?;
                let sig_end = text.find('{').unwrap_or(text.len());
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

impl LanguageExtractor for TypeScriptExtractor {
    fn language(&self) -> &str {
        "typescript"
    }

    fn file_extensions(&self) -> &[&str] {
        &["ts", "tsx", "js", "jsx"]
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

    fn extract_ts(source: &str) -> ExtractionResult {
        let extractor = TypeScriptExtractor::new();
        extractor.extract(source.as_bytes(), &PathBuf::from("src/index.ts"))
    }

    #[test]
    fn test_extract_function() {
        let result = extract_ts(
            r#"
function hello(): void {
    console.log("hello");
}
"#,
        );
        assert!(!result.symbols.is_empty());
        assert_eq!(result.symbols[0].name, "hello");
        assert_eq!(result.symbols[0].kind, NodeKind::Function);
    }

    #[test]
    fn test_extract_class() {
        let result = extract_ts(
            r#"
class MyClass {
    method(): void {}
}
"#,
        );
        let sym = result.symbols.iter().find(|s| s.name == "MyClass").unwrap();
        assert_eq!(sym.kind, NodeKind::Class);
    }

    #[test]
    fn test_extract_interface() {
        let result = extract_ts(
            r#"
interface MyInterface {
    prop: string;
}
"#,
        );
        let sym = result
            .symbols
            .iter()
            .find(|s| s.name == "MyInterface")
            .unwrap();
        assert_eq!(sym.kind, NodeKind::Interface);
    }
}
