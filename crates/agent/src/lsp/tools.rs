//! LSP tools exposed to the agent — hover, goto-definition, find-references, diagnostics.

use super::manager::LspManager;
use async_trait::async_trait;
use savant_core::error::SavantError;
use savant_core::traits::Tool;
use std::path::PathBuf;
use std::sync::Arc;

fn get_str<'a>(payload: &'a serde_json::Value, key: &str) -> Result<&'a str, SavantError> {
    payload
        .get(key)
        .and_then(|v| v.as_str())
        .ok_or_else(|| SavantError::InvalidInput(format!("missing '{}' parameter", key)))
}

fn get_u64(payload: &serde_json::Value, key: &str) -> Result<u64, SavantError> {
    payload
        .get(key)
        .and_then(|v| v.as_u64())
        .ok_or_else(|| SavantError::InvalidInput(format!("missing '{}' parameter", key)))
}

/// Tool: lsp_hover — get hover information (type, docs) at a position.
pub struct LspHoverTool {
    manager: Arc<LspManager>,
}

impl LspHoverTool {
    pub fn new(manager: Arc<LspManager>) -> Self {
        Self { manager }
    }
}

#[async_trait]
impl Tool for LspHoverTool {
    fn name(&self) -> &str {
        "lsp_hover"
    }

    fn description(&self) -> &str {
        "Get hover information (type signature, documentation) at a specific position in a file. Returns the type and docs for the symbol at the given line and column."
    }

    #[allow(clippy::disallowed_methods)] // serde_json::json! macro
    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "file": { "type": "string", "description": "File path" },
                "line": { "type": "integer", "description": "Line number (1-based)" },
                "column": { "type": "integer", "description": "Column number (1-based, default 1)" }
            },
            "required": ["file", "line"]
        })
    }

    async fn execute(&self, payload: serde_json::Value) -> Result<String, SavantError> {
        let file = get_str(&payload, "file")?;
        let line = get_u64(&payload, "line")? as u32;
        let col = payload.get("column").and_then(|v| v.as_u64()).unwrap_or(1) as u32;

        let path = PathBuf::from(file);
        match self.manager.hover(&path, line, col).await {
            Ok(Some(hover)) => {
                let text = match hover.contents {
                    lsp_types::HoverContents::Scalar(s) => match s {
                        lsp_types::MarkedString::String(s) => s,
                        lsp_types::MarkedString::LanguageString(ls) => {
                            format!("```{}\n{}\n```", ls.language, ls.value)
                        }
                    },
                    lsp_types::HoverContents::Array(arr) => arr
                        .iter()
                        .map(|s| match s {
                            lsp_types::MarkedString::String(s) => s.clone(),
                            lsp_types::MarkedString::LanguageString(ls) => {
                                format!("```{}\n{}\n```", ls.language, ls.value)
                            }
                        })
                        .collect::<Vec<_>>()
                        .join("\n"),
                    lsp_types::HoverContents::Markup(m) => m.value,
                };
                Ok(text)
            }
            Ok(None) => Ok("No hover information available at this position.".to_string()),
            Err(e) => Ok(format!("LSP hover error: {}", e)),
        }
    }
}

/// Tool: lsp_goto_definition — jump to the definition of a symbol.
pub struct LspGotoDefinitionTool {
    manager: Arc<LspManager>,
}

impl LspGotoDefinitionTool {
    pub fn new(manager: Arc<LspManager>) -> Self {
        Self { manager }
    }
}

#[async_trait]
impl Tool for LspGotoDefinitionTool {
    fn name(&self) -> &str {
        "lsp_goto_definition"
    }

    fn description(&self) -> &str {
        "Go to the definition of the symbol at the given position. Returns the file path, line, and column of the definition."
    }

    #[allow(clippy::disallowed_methods)] // serde_json::json! macro
    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "file": { "type": "string", "description": "File path" },
                "line": { "type": "integer", "description": "Line number (1-based)" },
                "column": { "type": "integer", "description": "Column number (1-based, default 1)" }
            },
            "required": ["file", "line"]
        })
    }

    async fn execute(&self, payload: serde_json::Value) -> Result<String, SavantError> {
        let file = get_str(&payload, "file")?;
        let line = get_u64(&payload, "line")? as u32;
        let col = payload.get("column").and_then(|v| v.as_u64()).unwrap_or(1) as u32;

        let path = PathBuf::from(file);
        match self.manager.goto_definition(&path, line, col).await {
            Ok(Some(locs)) => {
                let results: Vec<String> = locs
                    .iter()
                    .map(|loc| {
                        let fp = loc.uri.path();
                        let l = loc.range.start.line + 1;
                        let c = loc.range.start.character + 1;
                        format!("{}:{}:{}", fp, l, c)
                    })
                    .collect();
                Ok(results.join("\n"))
            }
            Ok(None) => Ok("No definition found.".to_string()),
            Err(e) => Ok(format!("LSP goto-definition error: {}", e)),
        }
    }
}

/// Tool: lsp_find_references — find all references to a symbol.
pub struct LspFindReferencesTool {
    manager: Arc<LspManager>,
}

impl LspFindReferencesTool {
    pub fn new(manager: Arc<LspManager>) -> Self {
        Self { manager }
    }
}

#[async_trait]
impl Tool for LspFindReferencesTool {
    fn name(&self) -> &str {
        "lsp_find_references"
    }

    fn description(&self) -> &str {
        "Find all references to the symbol at the given position. Returns all locations where the symbol is used."
    }

    #[allow(clippy::disallowed_methods)] // serde_json::json! macro
    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "file": { "type": "string", "description": "File path" },
                "line": { "type": "integer", "description": "Line number (1-based)" },
                "column": { "type": "integer", "description": "Column number (1-based, default 1)" },
                "includeDeclaration": { "type": "boolean", "description": "Include the declaration itself (default true)" }
            },
            "required": ["file", "line"]
        })
    }

    async fn execute(&self, payload: serde_json::Value) -> Result<String, SavantError> {
        let file = get_str(&payload, "file")?;
        let line = get_u64(&payload, "line")? as u32;
        let col = payload.get("column").and_then(|v| v.as_u64()).unwrap_or(1) as u32;
        let include_decl = payload
            .get("includeDeclaration")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);

        let path = PathBuf::from(file);
        match self
            .manager
            .find_references(&path, line, col, include_decl)
            .await
        {
            Ok(Some(locs)) => {
                let results: Vec<String> = locs
                    .iter()
                    .map(|loc| {
                        let fp = loc.uri.path();
                        let l = loc.range.start.line + 1;
                        let c = loc.range.start.character + 1;
                        format!("{}:{}:{}", fp, l, c)
                    })
                    .collect();
                Ok(results.join("\n"))
            }
            Ok(None) => Ok("No references found.".to_string()),
            Err(e) => Ok(format!("LSP find-references error: {}", e)),
        }
    }
}

/// Tool: lsp_diagnostics — get diagnostics (errors, warnings) for a file.
pub struct LspDiagnosticsTool {
    manager: Arc<LspManager>,
}

impl LspDiagnosticsTool {
    pub fn new(manager: Arc<LspManager>) -> Self {
        Self { manager }
    }
}

#[async_trait]
impl Tool for LspDiagnosticsTool {
    fn name(&self) -> &str {
        "lsp_diagnostics"
    }

    fn description(&self) -> &str {
        "Get diagnostics (errors, warnings, hints) for a file. Returns compiler/linter output from the language server."
    }

    #[allow(clippy::disallowed_methods)] // serde_json::json! macro
    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "file": { "type": "string", "description": "File path" }
            },
            "required": ["file"]
        })
    }

    async fn execute(&self, payload: serde_json::Value) -> Result<String, SavantError> {
        let file = get_str(&payload, "file")?;
        let path = PathBuf::from(file);
        let diags = self.manager.diagnostics(&path).await;

        if diags.is_empty() {
            return Ok("No diagnostics for this file.".to_string());
        }

        let results: Vec<String> = diags
            .iter()
            .map(|d| {
                let severity = match d.severity {
                    Some(lsp_types::DiagnosticSeverity::ERROR) => "ERROR",
                    Some(lsp_types::DiagnosticSeverity::WARNING) => "WARNING",
                    Some(lsp_types::DiagnosticSeverity::INFORMATION) => "INFO",
                    Some(lsp_types::DiagnosticSeverity::HINT) => "HINT",
                    _ => "UNKNOWN",
                };
                let l = d.range.start.line + 1;
                let c = d.range.start.character + 1;
                let source = d.source.as_deref().unwrap_or("lsp");
                format!(
                    "[{}] {}:{}:{} ({}): {}",
                    severity, file, l, c, source, d.message
                )
            })
            .collect();

        Ok(results.join("\n"))
    }
}
