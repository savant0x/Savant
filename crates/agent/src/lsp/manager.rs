//! Multi-server LSP manager — routes requests to the correct server by file extension.

use super::client::{LspClient, ServerState};
use super::discovery::{discover_servers, find_server_for_extension, LspServerConfig};
use lsp_types::*;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::RwLock;

/// Manages multiple LSP servers, one per language.
pub struct LspManager {
    /// Active LSP clients, keyed by language.
    clients: Arc<RwLock<HashMap<String, LspClient>>>,
    /// Project root directory.
    project_root: PathBuf,
    /// Discovered server configurations.
    configs: Vec<LspServerConfig>,
}

impl LspManager {
    /// Create a new LSP manager for the given project root.
    pub fn new(project_root: PathBuf) -> Self {
        let configs = discover_servers();
        tracing::info!(
            "LSP Manager: discovered {} servers: {:?}",
            configs.len(),
            configs.iter().map(|c| &c.language).collect::<Vec<_>>()
        );
        Self {
            clients: Arc::new(RwLock::new(HashMap::new())),
            project_root,
            configs,
        }
    }

    /// Get or start an LSP server for the given file.
    /// Returns None if no server is available for that language.
    async fn get_or_start(&self, path: &Path) -> Option<(String, LspServerConfig)> {
        let ext = path.extension()?.to_str()?;
        let config = find_server_for_extension(ext)?;

        let mut clients = self.clients.write().await;
        if clients.contains_key(&config.language) {
            return Some((config.language.clone(), config));
        }

        // Start the server
        let mut client = LspClient::new(
            config.command.clone(),
            config.language.clone(),
            config.file_extensions.clone(),
            self.project_root.clone(),
        );

        let args: Vec<&str> = config.args.iter().map(|s| s.as_str()).collect();
        match client.start(&config.command, &args).await {
            Ok(()) => {
                tracing::info!(
                    "LSP server '{}' started for {}",
                    config.command,
                    config.language
                );
                let lang = config.language.clone();
                clients.insert(lang.clone(), client);
                Some((lang, config))
            }
            Err(e) => {
                tracing::warn!("Failed to start LSP server '{}': {}", config.command, e);
                None
            }
        }
    }

    /// Notify the LSP server that a file was opened.
    pub async fn did_open(&self, path: &Path, text: &str) -> Result<(), String> {
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        let lang_id = match ext {
            "rs" => "rust",
            "ts" | "tsx" => "typescript",
            "js" | "jsx" => "javascript",
            "py" => "python",
            "go" => "go",
            "c" | "h" => "c",
            "cpp" | "hpp" | "cc" => "cpp",
            _ => return Ok(()),
        };

        if let Some((lang, _)) = self.get_or_start(path).await {
            let clients = self.clients.read().await;
            if let Some(client) = clients.get(&lang) {
                client.did_open(path, lang_id, text).await?;
            }
        }
        Ok(())
    }

    /// Notify the LSP server that a file was changed.
    pub async fn did_change(&self, path: &Path, text: &str) -> Result<(), String> {
        if let Some((lang, _)) = self.get_or_start(path).await {
            let clients = self.clients.read().await;
            if let Some(client) = clients.get(&lang) {
                client.did_change(path, text).await?;
            }
        }
        Ok(())
    }

    /// Notify the LSP server that a file was closed.
    pub async fn did_close(&self, path: &Path) -> Result<(), String> {
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        let lang = match ext {
            "rs" => "rust",
            "ts" | "tsx" | "js" | "jsx" => "typescript",
            "py" => "python",
            "go" => "go",
            "c" | "h" | "cpp" | "hpp" | "cc" => "c",
            _ => return Ok(()),
        };

        let clients = self.clients.read().await;
        if let Some(client) = clients.get(lang) {
            client.did_close(path).await?;
        }
        Ok(())
    }

    /// Request hover information at a position.
    pub async fn hover(
        &self,
        path: &Path,
        line: u32,
        character: u32,
    ) -> Result<Option<Hover>, String> {
        let (lang, _) = self
            .get_or_start(path)
            .await
            .ok_or_else(|| format!("No LSP server for {}", path.display()))?;
        let clients = self.clients.read().await;
        let client = clients
            .get(&lang)
            .ok_or_else(|| format!("LSP client not found for {}", lang))?;
        client.hover(path, line, character).await
    }

    /// Request goto definition at a position.
    pub async fn goto_definition(
        &self,
        path: &Path,
        line: u32,
        character: u32,
    ) -> Result<Option<Vec<Location>>, String> {
        let (lang, _) = self
            .get_or_start(path)
            .await
            .ok_or_else(|| format!("No LSP server for {}", path.display()))?;
        let clients = self.clients.read().await;
        let client = clients
            .get(&lang)
            .ok_or_else(|| format!("LSP client not found for {}", lang))?;
        client.goto_definition(path, line, character).await
    }

    /// Request references at a position.
    pub async fn find_references(
        &self,
        path: &Path,
        line: u32,
        character: u32,
        include_declaration: bool,
    ) -> Result<Option<Vec<Location>>, String> {
        let (lang, _) = self
            .get_or_start(path)
            .await
            .ok_or_else(|| format!("No LSP server for {}", path.display()))?;
        let clients = self.clients.read().await;
        let client = clients
            .get(&lang)
            .ok_or_else(|| format!("LSP client not found for {}", lang))?;
        client
            .find_references(path, line, character, include_declaration)
            .await
    }

    /// Get cached diagnostics for a file.
    pub async fn diagnostics(&self, path: &Path) -> Vec<Diagnostic> {
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        let lang = match ext {
            "rs" => "rust",
            "ts" | "tsx" | "js" | "jsx" => "typescript",
            "py" => "python",
            "go" => "go",
            "c" | "h" | "cpp" | "hpp" | "cc" => "c",
            _ => return vec![],
        };

        let clients = self.clients.read().await;
        if let Some(client) = clients.get(lang) {
            client.diagnostics(path).await
        } else {
            vec![]
        }
    }

    /// List all active servers and their states.
    pub async fn list_servers(&self) -> Vec<(String, ServerState)> {
        let clients = self.clients.read().await;
        let mut result = Vec::new();
        for (lang, client) in clients.iter() {
            result.push((lang.clone(), client.state().await));
        }
        result
    }

    /// Get available LSP server languages (discovered or active).
    pub fn available_languages(&self) -> Vec<String> {
        self.configs.iter().map(|c| c.language.clone()).collect()
    }

    /// Shutdown all LSP servers gracefully.
    pub async fn shutdown_all(&self) {
        let mut clients = self.clients.write().await;
        for (lang, client) in clients.iter_mut() {
            tracing::info!("Shutting down LSP server for {}", lang);
            if let Err(e) = client.shutdown().await {
                tracing::warn!("Error shutting down {}: {}", lang, e);
            }
        }
        clients.clear();
    }
}

impl Drop for LspManager {
    fn drop(&mut self) {
        // Can't async in drop, so just kill all servers
        // The shutdown_all() should be called before dropping
    }
}
