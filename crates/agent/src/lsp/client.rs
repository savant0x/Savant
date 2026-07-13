//! JSON-RPC LSP client that communicates with LSP servers over stdio.
//!
//! This module uses `serde_json::json!()` extensively for LSP protocol messages.
//! The macro internally uses `unwrap()` which is expected for compile-time-valid JSON literals.

#![allow(clippy::disallowed_methods)] // serde_json::json! macro in LSP protocol messages

use lsp_types::*;
use serde_json::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::{oneshot, Mutex, RwLock};

/// State of an LSP server.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServerState {
    Unstarted,
    Starting,
    Ready,
    Error,
    Stopped,
}

/// A pending JSON-RPC request awaiting a response.
struct PendingRequest {
    sender: oneshot::Sender<Result<Value, String>>,
}

/// JSON-RPC LSP client that communicates with a language server over stdio.
pub struct LspClient {
    name: String,
    language: String,
    file_extensions: Vec<String>,
    cwd: PathBuf,
    child: Option<Child>,
    stdin: Option<Mutex<tokio::process::ChildStdin>>,
    pending: Arc<Mutex<HashMap<i64, PendingRequest>>>,
    next_id: AtomicI64,
    state: Arc<RwLock<ServerState>>,
    capabilities: RwLock<Option<ServerCapabilities>>,
    open_files: RwLock<HashMap<String, i32>>,
    diagnostics: Arc<RwLock<HashMap<String, Vec<Diagnostic>>>>,
}

impl LspClient {
    pub fn new(name: String, language: String, file_extensions: Vec<String>, cwd: PathBuf) -> Self {
        Self {
            name,
            language,
            file_extensions,
            cwd,
            child: None,
            stdin: None,
            pending: Arc::new(Mutex::new(HashMap::new())),
            next_id: AtomicI64::new(1),
            state: Arc::new(RwLock::new(ServerState::Unstarted)),
            capabilities: RwLock::new(None),
            open_files: RwLock::new(HashMap::new()),
            diagnostics: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }
    pub fn language(&self) -> &str {
        &self.language
    }
    pub fn file_extensions(&self) -> &[String] {
        &self.file_extensions
    }

    pub fn handles_file(&self, path: &Path) -> bool {
        match path.extension().and_then(|e| e.to_str()) {
            Some(ext) => self.file_extensions.iter().any(|fe| fe == ext),
            None => false,
        }
    }

    pub async fn state(&self) -> ServerState {
        *self.state.read().await
    }

    /// Start the LSP server process and initialize it.
    #[allow(clippy::disallowed_methods)] // serde_json::json! macro for LSP protocol messages
    pub async fn start(&mut self, command: &str, args: &[&str]) -> Result<(), String> {
        let mut cmd = Command::new(command);
        cmd.args(args)
            .current_dir(&self.cwd)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null());

        let mut child = cmd
            .spawn()
            .map_err(|e| format!("Failed to spawn {}: {}", self.name, e))?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| format!("{}: no stdin", self.name))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| format!("{}: no stdout", self.name))?;

        self.stdin = Some(Mutex::new(stdin));
        self.child = Some(child);
        *self.state.write().await = ServerState::Starting;

        // Spawn reader task
        let pending = Arc::clone(&self.pending);
        let diagnostics = Arc::clone(&self.diagnostics);
        let state = Arc::clone(&self.state);

        tokio::spawn(async move {
            let mut reader = BufReader::new(stdout);
            loop {
                let mut content_length: Option<usize> = None;
                loop {
                    let mut line = String::new();
                    match reader.read_line(&mut line).await {
                        Ok(0) | Err(_) => break,
                        _ => {}
                    }
                    let trimmed = line.trim();
                    if trimmed.is_empty() {
                        break;
                    }
                    if let Some(len_str) = trimmed.strip_prefix("Content-Length: ") {
                        content_length = len_str.parse().ok();
                    }
                }

                let len = match content_length {
                    Some(l) => l,
                    None => break,
                };

                let mut buf = vec![0u8; len];
                if reader.read_exact(&mut buf).await.is_err() {
                    break;
                }

                let msg: Value = match serde_json::from_slice(&buf) {
                    Ok(v) => v,
                    Err(_) => continue,
                };

                if let Some(id) = msg.get("id").and_then(|v| v.as_i64()) {
                    let mut pending = pending.lock().await;
                    if let Some(p) = pending.remove(&id) {
                        if let Some(error) = msg.get("error") {
                            let _ = p.sender.send(Err(format!("LSP error: {}", error)));
                        } else {
                            let result = msg.get("result").cloned().unwrap_or(Value::Null);
                            let _ = p.sender.send(Ok(result));
                        }
                    }
                } else if let Some(method) = msg.get("method").and_then(|v| v.as_str()) {
                    if method == "textDocument/publishDiagnostics" {
                        if let Some(params) = msg.get("params") {
                            if let Ok(diag_params) =
                                serde_json::from_value::<PublishDiagnosticsParams>(params.clone())
                            {
                                let uri = diag_params.uri.to_string();
                                let diags = diag_params.diagnostics;
                                let mut cache = diagnostics.write().await;
                                cache.insert(uri, diags);
                            }
                        }
                    }
                }
            }
            *state.write().await = ServerState::Stopped;
        });

        // Build workspace URI
        let workspace_str = path_to_uri(&self.cwd);
        let workspace_uri: Uri = workspace_str
            .parse()
            .map_err(|e| format!("Invalid workspace URI: {}", e))?;

        // Send initialize request (workspace_folders is preferred over deprecated root_uri)
        let init_params = InitializeParams {
            process_id: Some(std::process::id()),
            capabilities: ClientCapabilities {
                text_document: Some(TextDocumentClientCapabilities {
                    hover: Some(HoverClientCapabilities {
                        content_format: Some(vec![MarkupKind::Markdown]),
                        ..Default::default()
                    }),
                    definition: Some(GotoCapability {
                        dynamic_registration: Some(false),
                        link_support: Some(false),
                    }),
                    references: Some(ReferenceClientCapabilities {
                        dynamic_registration: Some(false),
                    }),
                    publish_diagnostics: Some(PublishDiagnosticsClientCapabilities::default()),
                    ..Default::default()
                }),
                ..Default::default()
            },
            workspace_folders: Some(vec![WorkspaceFolder {
                uri: workspace_uri,
                name: self
                    .cwd
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("workspace")
                    .to_string(),
            }]),
            ..Default::default()
        };

        let init_result = self
            .send_request(
                "initialize",
                serde_json::to_value(init_params)
                    .map_err(|e| format!("Failed to serialize init params: {}", e))?,
            )
            .await?;

        if let Ok(caps) = serde_json::from_value::<InitializeResult>(init_result) {
            *self.capabilities.write().await = Some(caps.capabilities);
        }

        self.send_notification("initialized", serde_json::json!({}))
            .await?;
        *self.state.write().await = ServerState::Ready;
        Ok(())
    }

    async fn send_request(&self, method: &str, params: Value) -> Result<Value, String> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let msg = serde_json::json!({
            "jsonrpc": "2.0", "id": id, "method": method, "params": params,
        });
        let (tx, rx) = oneshot::channel();
        {
            self.pending
                .lock()
                .await
                .insert(id, PendingRequest { sender: tx });
        }
        self.send_message(&msg).await?;
        tokio::time::timeout(std::time::Duration::from_secs(30), rx)
            .await
            .map_err(|_| format!("{}: request '{}' timed out", self.name, method))?
            .map_err(|_| format!("{}: request '{}' channel closed", self.name, method))?
    }

    async fn send_notification(&self, method: &str, params: Value) -> Result<(), String> {
        let msg = serde_json::json!({
            "jsonrpc": "2.0", "method": method, "params": params,
        });
        self.send_message(&msg).await
    }

    async fn send_message(&self, msg: &Value) -> Result<(), String> {
        let body = serde_json::to_string(msg)
            .map_err(|e| format!("{}: serialization error: {}", self.name, e))?;
        let header = format!("Content-Length: {}\r\n\r\n", body.len());

        let stdin = self
            .stdin
            .as_ref()
            .ok_or_else(|| format!("{}: stdin not available", self.name))?;
        let mut stdin = stdin.lock().await;
        stdin
            .write_all(header.as_bytes())
            .await
            .map_err(|e| format!("{}: write error: {}", self.name, e))?;
        stdin
            .write_all(body.as_bytes())
            .await
            .map_err(|e| format!("{}: write error: {}", self.name, e))?;
        stdin
            .flush()
            .await
            .map_err(|e| format!("{}: flush error: {}", self.name, e))?;
        Ok(())
    }

    #[allow(clippy::disallowed_methods)] // serde_json::json! macro for LSP protocol messages
    pub async fn did_open(&self, path: &Path, language_id: &str, text: &str) -> Result<(), String> {
        let uri = path_to_uri(path);
        let version = 1;
        {
            self.open_files.write().await.insert(uri.clone(), version);
        }
        self.send_notification("textDocument/didOpen", serde_json::json!({
            "textDocument": { "uri": uri, "languageId": language_id, "version": version, "text": text }
        })).await
    }

    #[allow(clippy::disallowed_methods)] // serde_json::json! macro for LSP protocol messages
    pub async fn did_change(&self, path: &Path, text: &str) -> Result<(), String> {
        let uri = path_to_uri(path);
        let version = {
            let mut files = self.open_files.write().await;
            let v = files.get(&uri).copied().unwrap_or(0) + 1;
            files.insert(uri.clone(), v);
            v
        };
        self.send_notification(
            "textDocument/didChange",
            serde_json::json!({
                "textDocument": { "uri": uri, "version": version },
                "contentChanges": [{ "text": text }]
            }),
        )
        .await
    }

    #[allow(clippy::disallowed_methods)] // serde_json::json! macro for LSP protocol messages
    pub async fn did_close(&self, path: &Path) -> Result<(), String> {
        let uri = path_to_uri(path);
        {
            self.open_files.write().await.remove(&uri);
        }
        self.send_notification(
            "textDocument/didClose",
            serde_json::json!({
                "textDocument": { "uri": uri }
            }),
        )
        .await
    }

    #[allow(clippy::disallowed_methods)] // serde_json::json! macro for LSP protocol messages
    pub async fn hover(
        &self,
        path: &Path,
        line: u32,
        character: u32,
    ) -> Result<Option<Hover>, String> {
        let uri = path_to_uri(path);
        let result = self.send_request("textDocument/hover", serde_json::json!({
            "textDocument": { "uri": uri },
            "position": { "line": line.saturating_sub(1), "character": character.saturating_sub(1) },
        })).await?;
        if result.is_null() {
            return Ok(None);
        }
        serde_json::from_value(result).map_err(|e| format!("Failed to parse hover: {}", e))
    }

    #[allow(clippy::disallowed_methods)] // serde_json::json! macro for LSP protocol messages
    pub async fn goto_definition(
        &self,
        path: &Path,
        line: u32,
        character: u32,
    ) -> Result<Option<Vec<Location>>, String> {
        let uri = path_to_uri(path);
        let result = self.send_request("textDocument/definition", serde_json::json!({
            "textDocument": { "uri": uri },
            "position": { "line": line.saturating_sub(1), "character": character.saturating_sub(1) },
        })).await?;
        if result.is_null() {
            return Ok(None);
        }
        if result.is_array() {
            serde_json::from_value(result)
                .map_err(|e| format!("Failed to parse definitions: {}", e))
        } else {
            let loc: Location = serde_json::from_value(result)
                .map_err(|e| format!("Failed to parse definition: {}", e))?;
            Ok(Some(vec![loc]))
        }
    }

    #[allow(clippy::disallowed_methods)] // serde_json::json! macro for LSP protocol messages
    pub async fn find_references(
        &self,
        path: &Path,
        line: u32,
        character: u32,
        include_declaration: bool,
    ) -> Result<Option<Vec<Location>>, String> {
        let uri = path_to_uri(path);
        let result = self.send_request("textDocument/references", serde_json::json!({
            "textDocument": { "uri": uri },
            "position": { "line": line.saturating_sub(1), "character": character.saturating_sub(1) },
            "context": { "includeDeclaration": include_declaration },
        })).await?;
        if result.is_null() {
            return Ok(None);
        }
        serde_json::from_value(result).map_err(|e| format!("Failed to parse references: {}", e))
    }

    pub async fn diagnostics(&self, path: &Path) -> Vec<Diagnostic> {
        let uri = path_to_uri(path);
        self.diagnostics
            .read()
            .await
            .get(&uri)
            .cloned()
            .unwrap_or_default()
    }

    pub async fn shutdown(&mut self) -> Result<(), String> {
        if *self.state.read().await != ServerState::Ready {
            return Ok(());
        }
        let _ = self.send_request("shutdown", Value::Null).await;
        let _ = self.send_notification("exit", Value::Null).await;
        if let Some(mut child) = self.child.take() {
            let _ = tokio::time::timeout(std::time::Duration::from_secs(5), child.wait()).await;
            let _ = child.kill().await;
        }
        *self.state.write().await = ServerState::Stopped;
        self.stdin = None;
        Ok(())
    }
}

impl Drop for LspClient {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.start_kill();
        }
    }
}

/// Convert a file path to a file:// URI string.
fn path_to_uri(path: &Path) -> String {
    let path_str = path.display().to_string().replace('\\', "/");
    format!("file:///{}", path_str)
}
