//! Tree-sitter AST parsing orchestration.
//!
//! Each language implements the `LanguageExtractor` trait to map tree-sitter
//! AST nodes into the common `Symbol` and `Edge` types.

pub mod languages;

use crate::symbols::{Edge, Symbol};
use std::path::Path;

/// Result of parsing a single file.
pub struct ExtractionResult {
    pub symbols: Vec<Symbol>,
    pub edges: Vec<Edge>,
}

/// Trait for language-specific extractors.
///
/// Implementations map tree-sitter AST nodes into the common `Symbol`/`Edge` types.
pub trait LanguageExtractor: Send + Sync {
    /// The language name (e.g., "rust", "typescript", "python").
    fn language(&self) -> &str;

    /// File extensions this extractor handles (e.g., ["rs"] for Rust).
    fn file_extensions(&self) -> &[&str];

    /// Parse source code and extract symbols + edges.
    fn extract(&self, source: &[u8], file_path: &Path) -> ExtractionResult;
}

/// Get all registered language extractors.
pub fn default_extractors() -> Vec<Box<dyn LanguageExtractor>> {
    vec![
        Box::new(languages::rust::RustExtractor::new()),
        Box::new(languages::typescript::TypeScriptExtractor::new()),
        Box::new(languages::python::PythonExtractor::new()),
    ]
}

/// Detect language from file extension.
pub fn detect_language(extractors: &[Box<dyn LanguageExtractor>], path: &Path) -> Option<String> {
    let ext = path.extension()?.to_str()?;
    for extractor in extractors {
        if extractor.file_extensions().contains(&ext) {
            return Some(extractor.language().to_string());
        }
    }
    None
}
