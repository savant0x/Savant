//! LSP (Language Server Protocol) integration for Savant.
//!
//! Provides:
//! - JSON-RPC client for communicating with LSP servers over stdio
//! - Auto-discovery of installed LSP servers
//! - Multi-server manager (one server per language)
//! - On-demand server startup
//! - Tools: hover, goto-definition, find-references, diagnostics

pub mod client;
pub mod discovery;
pub mod manager;
pub mod tools;

pub use client::{LspClient, ServerState};
pub use discovery::{discover_servers, find_server_for_language, LspServerConfig};
pub use manager::LspManager;
pub use tools::{LspDiagnosticsTool, LspFindReferencesTool, LspGotoDefinitionTool, LspHoverTool};
