//! Core symbol types for code intelligence.

use serde::{Deserialize, Serialize};

/// The kind of a symbol (node) in the code graph.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum NodeKind {
    Function,
    Method,
    Class,
    Struct,
    Trait,
    Interface,
    Enum,
    EnumVariant,
    Module,
    Namespace,
    Variable,
    Constant,
    TypeAlias,
    Field,
    Parameter,
    Property,
    Import,
    Macro,
    Impl,
}

/// The kind of a relationship (edge) between symbols.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum EdgeKind {
    /// Function/method calls another function/method.
    Calls,
    /// Type/type implements a trait/interface.
    Implements,
    /// Module/type imports or uses another module/type.
    Imports,
    /// Type inherits from another type.
    Extends,
    /// Function/method uses a variable/constant.
    Uses,
    /// Module contains a function/class/etc.
    Contains,
    /// Type has a field.
    HasField,
    /// A type is defined in a module.
    DefinedIn,
}

/// A symbol extracted from source code.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Symbol {
    /// Unique identifier (file_path + name + range).
    pub id: String,
    /// Symbol name (function name, class name, etc.).
    pub name: String,
    /// Fully qualified name (e.g., `crate::module::function`).
    pub qualified_name: String,
    /// Kind of symbol.
    pub kind: NodeKind,
    /// File path relative to project root.
    pub file_path: String,
    /// Start line (1-based).
    pub start_line: u32,
    /// End line (1-based).
    pub end_line: u32,
    /// Start byte offset in file.
    pub start_byte: u32,
    /// End byte offset in file.
    pub end_byte: u32,
    /// Language (rust, typescript, python).
    pub language: String,
    /// Doc comment or docstring, if present.
    pub documentation: Option<String>,
    /// Signature (function parameters, type definition, etc.).
    pub signature: Option<String>,
    /// Parent symbol ID (e.g., method's parent is a class).
    pub parent_id: Option<String>,
}

/// A relationship between two symbols.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Edge {
    /// Unique identifier.
    pub id: String,
    /// Source symbol ID.
    pub source_id: String,
    /// Target symbol ID.
    pub target_id: String,
    /// Kind of relationship.
    pub kind: EdgeKind,
    /// File path where this relationship occurs.
    pub file_path: String,
    /// Line number where this relationship occurs.
    pub line: u32,
}

/// A file tracked by the code index.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexedFile {
    /// File path relative to project root.
    pub path: String,
    /// Language detected for this file.
    pub language: String,
    /// Last modification time (Unix timestamp).
    pub mtime: i64,
    /// Content hash for change detection.
    pub content_hash: String,
}

/// Result of a code search.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    /// The matching symbol.
    pub symbol: Symbol,
    /// Relevance score (0.0 - 1.0).
    pub score: f64,
    /// Matching text snippet with context.
    pub snippet: String,
}
