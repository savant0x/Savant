//! Schema (code intelligence) tools — search, callers, impact, symbols.

// serde_json::json!() macro internally uses unwrap() on literal serialization (always safe).
// This triggers clippy::disallowed_methods which is configured project-wide.
#![allow(clippy::disallowed_methods)]

use async_trait::async_trait;
use savant_core::error::SavantError;
use savant_core::traits::Tool;
use std::sync::Arc;

/// Tool: code_search — full-text search over indexed code symbols.
pub struct CodeSearchTool {
    index: Arc<savant_schema::SchemaIndex>,
}

impl CodeSearchTool {
    pub fn new(index: Arc<savant_schema::SchemaIndex>) -> Self {
        Self { index }
    }
}

#[async_trait]
impl Tool for CodeSearchTool {
    fn name(&self) -> &str {
        "code_search"
    }

    fn description(&self) -> &str {
        "Search indexed code for symbols (functions, classes, structs, traits) by name or documentation. Returns matching symbols with their file locations and types."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "query": { "type": "string", "description": "Search query (symbol name, keyword, or documentation text)" },
                "limit": { "type": "integer", "description": "Maximum results (default 10)" }
            },
            "required": ["query"]
        })
    }

    async fn execute(&self, payload: serde_json::Value) -> Result<String, SavantError> {
        let query = payload
            .get("query")
            .and_then(|v| v.as_str())
            .ok_or_else(|| SavantError::InvalidInput("missing 'query' parameter".into()))?;
        let limit = payload.get("limit").and_then(|v| v.as_u64()).unwrap_or(10) as u32;

        match self.index.search(query, limit) {
            Ok(results) => {
                if results.is_empty() {
                    return Ok(format!("No symbols found for '{}'.", query));
                }
                let lines: Vec<String> = results
                    .iter()
                    .map(|r| {
                        format!(
                            "- `{}` ({:?}) — {}",
                            r.symbol.qualified_name, r.symbol.kind, r.snippet
                        )
                    })
                    .collect();
                Ok(format!(
                    "Found {} symbols:\n{}",
                    results.len(),
                    lines.join("\n")
                ))
            }
            Err(e) => Ok(format!("Search error: {}", e)),
        }
    }
}

/// Tool: get_callers — find all callers of a symbol.
pub struct GetCallersTool {
    index: Arc<savant_schema::SchemaIndex>,
}

impl GetCallersTool {
    pub fn new(index: Arc<savant_schema::SchemaIndex>) -> Self {
        Self { index }
    }
}

#[async_trait]
impl Tool for GetCallersTool {
    fn name(&self) -> &str {
        "get_callers"
    }

    fn description(&self) -> &str {
        "Find all functions/methods that call the specified symbol. Returns the list of callers with their file locations."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "symbol_id": { "type": "string", "description": "The symbol ID (e.g., 'src/lib.rs:1:my_function')" }
            },
            "required": ["symbol_id"]
        })
    }

    async fn execute(&self, payload: serde_json::Value) -> Result<String, SavantError> {
        let symbol_id = payload
            .get("symbol_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| SavantError::InvalidInput("missing 'symbol_id' parameter".into()))?;

        match self.index.get_callers(symbol_id) {
            Ok(callers) => {
                if callers.is_empty() {
                    return Ok(format!("No callers found for '{}'.", symbol_id));
                }
                let lines: Vec<String> = callers
                    .iter()
                    .map(|c| {
                        format!(
                            "- `{}` ({}:{})",
                            c.qualified_name, c.file_path, c.start_line
                        )
                    })
                    .collect();
                Ok(format!(
                    "{} callers of `{}`:\n{}",
                    callers.len(),
                    symbol_id,
                    lines.join("\n")
                ))
            }
            Err(e) => Ok(format!("Error: {}", e)),
        }
    }
}

/// Tool: get_impact — compute impact radius of a symbol change.
pub struct GetImpactTool {
    index: Arc<savant_schema::SchemaIndex>,
}

impl GetImpactTool {
    pub fn new(index: Arc<savant_schema::SchemaIndex>) -> Self {
        Self { index }
    }
}

#[async_trait]
impl Tool for GetImpactTool {
    fn name(&self) -> &str {
        "get_impact"
    }

    fn description(&self) -> &str {
        "Compute the impact radius of changing a symbol. Returns all symbols that would be affected by a change, ordered by distance from the changed symbol."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "symbol_id": { "type": "string", "description": "The symbol ID to analyze" },
                "max_depth": { "type": "integer", "description": "Maximum call chain depth (default 3)" }
            },
            "required": ["symbol_id"]
        })
    }

    async fn execute(&self, payload: serde_json::Value) -> Result<String, SavantError> {
        let symbol_id = payload
            .get("symbol_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| SavantError::InvalidInput("missing 'symbol_id' parameter".into()))?;
        let max_depth = payload
            .get("max_depth")
            .and_then(|v| v.as_u64())
            .unwrap_or(3) as u32;

        let impact = self.index.get_impact(symbol_id, max_depth);
        if impact.is_empty() {
            return Ok(format!("No impact — '{}' has no callers.", symbol_id));
        }

        let lines: Vec<String> = impact
            .iter()
            .map(|entry| {
                format!(
                    "- {} (depth {}): `{}` ({}:{})",
                    entry.symbol.qualified_name,
                    entry.distance,
                    entry.symbol.name,
                    entry.symbol.file_path,
                    entry.symbol.start_line
                )
            })
            .collect();
        Ok(format!(
            "Impact radius of '{}' ({} affected symbols):\n{}",
            symbol_id,
            impact.len(),
            lines.join("\n")
        ))
    }
}

/// Tool: get_symbols — list all symbols in a file.
pub struct GetSymbolsTool {
    index: Arc<savant_schema::SchemaIndex>,
}

impl GetSymbolsTool {
    pub fn new(index: Arc<savant_schema::SchemaIndex>) -> Self {
        Self { index }
    }
}

#[async_trait]
impl Tool for GetSymbolsTool {
    fn name(&self) -> &str {
        "get_symbols_in_file"
    }

    fn description(&self) -> &str {
        "List all symbols (functions, classes, structs, etc.) in a file. Returns symbols with their types, line ranges, and signatures."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "file_path": { "type": "string", "description": "File path relative to project root" }
            },
            "required": ["file_path"]
        })
    }

    async fn execute(&self, payload: serde_json::Value) -> Result<String, SavantError> {
        let file_path = payload
            .get("file_path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| SavantError::InvalidInput("missing 'file_path' parameter".into()))?;

        match self.index.get_symbols_in_file(file_path) {
            Ok(symbols) => {
                if symbols.is_empty() {
                    return Ok(format!("No symbols found in '{}'.", file_path));
                }
                let lines: Vec<String> = symbols
                    .iter()
                    .map(|s| {
                        let sig = s.signature.as_deref().unwrap_or("");
                        format!(
                            "- `{}` ({:?}) at {}:{}{}",
                            s.qualified_name,
                            s.kind,
                            s.file_path,
                            s.start_line,
                            if sig.is_empty() {
                                String::new()
                            } else {
                                format!(" — {}", sig)
                            }
                        )
                    })
                    .collect();
                Ok(format!(
                    "{} symbols in `{}`:\n{}",
                    symbols.len(),
                    file_path,
                    lines.join("\n")
                ))
            }
            Err(e) => Ok(format!("Error: {}", e)),
        }
    }
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn make_index() -> Arc<savant_schema::SchemaIndex> {
        let index = savant_schema::SchemaIndex::open_memory(PathBuf::from(".")).unwrap();
        Arc::new(index)
    }

    #[tokio::test]
    async fn test_code_search_empty() {
        let index = make_index();
        let tool = CodeSearchTool::new(index);
        let result = tool
            .execute(serde_json::json!({"query": "test"}))
            .await
            .unwrap();
        assert!(result.contains("No symbols found"));
    }

    #[tokio::test]
    async fn test_get_callers_empty() {
        let index = make_index();
        let tool = GetCallersTool::new(index);
        let result = tool
            .execute(serde_json::json!({"symbol_id": "src/lib.rs:1:main"}))
            .await
            .unwrap();
        assert!(result.contains("No callers found"));
    }

    #[tokio::test]
    async fn test_get_impact_empty() {
        let index = make_index();
        let tool = GetImpactTool::new(index);
        let result = tool
            .execute(serde_json::json!({"symbol_id": "src/lib.rs:1:main"}))
            .await
            .unwrap();
        assert!(result.contains("No impact"));
    }

    #[tokio::test]
    async fn test_get_symbols_empty() {
        let index = make_index();
        let tool = GetSymbolsTool::new(index);
        let result = tool
            .execute(serde_json::json!({"file_path": "src/lib.rs"}))
            .await
            .unwrap();
        assert!(result.contains("No symbols found"));
    }
}
