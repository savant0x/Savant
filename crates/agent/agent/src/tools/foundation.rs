// SAFETY: All `clippy::disallowed_methods` violations in this file originate from
// the `serde_json::json!()` macro, which internally uses `.unwrap()` on
// compile-time-validated JSON literals. A malformed JSON literal would be a
// compile error, making the panic path statically unreachable.
#![allow(clippy::disallowed_methods)]

use async_trait::async_trait;
use savant_core::error::SavantError;
use savant_core::traits::Tool;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::fs;
use tracing::info;

/// Files the agent is forbidden from reading or writing.
/// SOUL.md is blocked for direct access — mutations go through the Evolution system.
/// SOUL.proposed.md is allowed as a staging area for mutation proposals.
const BLOCKED_FILES: &[&str] = &[
    "LEARNINGS.md",
    "LEARNINGS-ARCHIVE.md",
    "CONTEXT.md",
    "SOUL.md",
    "AGENTS.md",
    "agent.json",
];

/// Evolution staging files the agent CAN access for proposing mutations.
const EVOLUTION_STAGING_FILES: &[&str] = &["SOUL.proposed.md"];

/// Checks if a path targets a blocked file. Returns the filename if blocked, None otherwise.
/// Evolution staging files (SOUL.proposed.md) are exempt from blocking.
fn check_blocked(path: &Path) -> Option<String> {
    if let Some(filename) = path.file_name().and_then(|f| f.to_str()) {
        if EVOLUTION_STAGING_FILES
            .iter()
            .any(|s| s.eq_ignore_ascii_case(filename))
        {
            return None;
        }
        if BLOCKED_FILES
            .iter()
            .any(|b| b.eq_ignore_ascii_case(filename))
        {
            return Some(filename.to_string());
        }
    }
    None
}

/// Sandboxing Path Resolver
/// Computes an absolute path strictly bounded within the agent's assigned workspace.
/// Rejects ParentDir traversal above workspace root; silently re-roots absolute paths.
pub(crate) fn secure_resolve_path(workspace: &Path, target: &str) -> Result<PathBuf, SavantError> {
    let target_path = Path::new(target);

    // If the target is an absolute path, validate it's under the workspace root.
    // Project root access was previously allowed but is too permissive for file operations.
    if target_path.is_absolute() {
        let canonical = target_path
            .canonicalize()
            .unwrap_or_else(|_| target_path.to_path_buf());

        if canonical.starts_with(workspace) {
            return Ok(canonical);
        }

        return Err(SavantError::Unknown(format!(
            "Access denied: path '{}' is outside the workspace directory.",
            target
        )));
    }

    let mut resolved = workspace.to_path_buf();

    for component in target_path.components() {
        match component {
            std::path::Component::ParentDir => {
                if resolved == workspace {
                    return Err(SavantError::Unknown(
                        "Sandbox Escape Detected: Cannot navigate above workspace root.".into(),
                    ));
                }
                resolved.pop();
            }
            std::path::Component::Normal(c) => resolved.push(c),
            _ => {}
        }
    }

    Ok(resolved)
}

/// Tool for atomic file moves/renames.
pub struct FileMoveTool {
    workspace_dir: PathBuf,
    scanner: Option<Arc<savant_skills::security::SecurityScanner>>,
}

impl FileMoveTool {
    pub fn new(workspace_dir: PathBuf) -> Self {
        Self {
            workspace_dir,
            scanner: None,
        }
    }

    pub fn with_scanner(mut self, scanner: Arc<savant_skills::security::SecurityScanner>) -> Self {
        self.scanner = Some(scanner);
        self
    }
}

#[async_trait]
impl Tool for FileMoveTool {
    fn name(&self) -> &str {
        "file_move"
    }
    fn description(&self) -> &str {
        "Moves or renames a file or directory."
    }
    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "from": { "type": "string", "description": "Source path to move from" },
                "to": { "type": "string", "description": "Destination path to move to" }
            },
            "required": ["from", "to"]
        })
    }

    fn requires_approval(&self) -> savant_core::traits::ApprovalRequirement {
        savant_core::traits::ApprovalRequirement::Conditional
    }

    fn domain(&self) -> savant_core::traits::ToolDomain {
        savant_core::traits::ToolDomain::Container
    }
    async fn execute(&self, payload: Value) -> Result<String, SavantError> {
        let from_raw = payload["from"]
            .as_str()
            .ok_or_else(|| SavantError::Unknown("Missing 'from' path".to_string()))?;
        let to_raw = payload["to"]
            .as_str()
            .ok_or_else(|| SavantError::Unknown("Missing 'to' path".to_string()))?;

        // Block moves of system prompt files
        if let Some(blocked) = check_blocked(Path::new(from_raw)) {
            return Err(SavantError::Unknown(format!(
                "Access denied: cannot move system prompt file '{}'",
                blocked
            )));
        }
        if let Some(blocked) = check_blocked(Path::new(to_raw)) {
            return Err(SavantError::Unknown(format!(
                "Access denied: cannot move to system prompt file '{}'",
                blocked
            )));
        }

        let from = secure_resolve_path(&self.workspace_dir, from_raw)?;
        let to = secure_resolve_path(&self.workspace_dir, to_raw)?;

        info!(
            "[WAL:ACTUATOR] Action: move, From: {:?}, To: {:?}",
            from, to
        );
        fs::rename(&from, &to)
            .await
            .map_err(|e| SavantError::Unknown(format!("Move failed: {}", e)))?;
        Ok(format!("Successfully moved {:?} to {:?}.", from, to))
    }
}

/// Tool for file/directory deletion.
pub struct FileDeleteTool {
    workspace_dir: PathBuf,
    scanner: Option<Arc<savant_skills::security::SecurityScanner>>,
}

impl FileDeleteTool {
    pub fn new(workspace_dir: PathBuf) -> Self {
        Self {
            workspace_dir,
            scanner: None,
        }
    }

    pub fn with_scanner(mut self, scanner: Arc<savant_skills::security::SecurityScanner>) -> Self {
        self.scanner = Some(scanner);
        self
    }
}

#[async_trait]
impl Tool for FileDeleteTool {
    fn name(&self) -> &str {
        "file_delete"
    }
    fn description(&self) -> &str {
        "Deletes a file or directory recursively."
    }
    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "Path to the file or directory to delete" }
            },
            "required": ["path"]
        })
    }

    fn requires_approval(&self) -> savant_core::traits::ApprovalRequirement {
        savant_core::traits::ApprovalRequirement::Always
    }

    fn domain(&self) -> savant_core::traits::ToolDomain {
        savant_core::traits::ToolDomain::Container
    }
    async fn execute(&self, payload: Value) -> Result<String, SavantError> {
        let path_str = payload["path"]
            .as_str()
            .ok_or_else(|| SavantError::Unknown("Missing 'path' parameter".into()))?;

        // Block deletion of system prompt files
        if let Some(blocked) = check_blocked(Path::new(path_str)) {
            return Err(SavantError::Unknown(format!(
                "Access denied: cannot delete system prompt file '{}'",
                blocked
            )));
        }

        // PB-22: Use shared path validation instead of manual canonicalization
        let full_path = secure_resolve_path(&self.workspace_dir, path_str)?;

        if !full_path.exists() {
            return Ok(
                "[AVX-IX] Operation complete. File not found. Universe integrity maintained."
                    .to_string(),
            );
        }

        if full_path.is_dir() {
            tokio::fs::remove_dir_all(&full_path).await?;
        } else {
            tokio::fs::remove_file(&full_path).await?;
        }

        // AudioScape: Log the deletion event
        info!(
            "NVMe Actuator: Successfully deleted path [{}]",
            full_path.display()
        );

        Ok(format!(
            "🗑️ Sovereign Deletion Actuation complete: `{}` permanently erased from the substrate.",
            path_str
        ))
    }
}

/// Tool for atomic multi-chunk file editing.
pub struct FileAtomicEditTool {
    workspace_dir: PathBuf,
    scanner: Option<Arc<savant_skills::security::SecurityScanner>>,
}

impl FileAtomicEditTool {
    pub fn new(workspace_dir: PathBuf) -> Self {
        Self {
            workspace_dir,
            scanner: None,
        }
    }

    pub fn with_scanner(mut self, scanner: Arc<savant_skills::security::SecurityScanner>) -> Self {
        self.scanner = Some(scanner);
        self
    }
}

#[async_trait]
impl Tool for FileAtomicEditTool {
    fn name(&self) -> &str {
        "file_atomic_edit"
    }
    fn description(&self) -> &str {
        "Applies multiple atomic replacements to a file with backup/rollback safety."
    }
    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "Path to the file to edit" },
                "replacements": {
                    "type": "array",
                    "description": "Array of {target, value} replacements to apply",
                    "items": {
                        "type": "object",
                        "properties": {
                            "target": { "type": "string", "description": "Text to find" },
                            "value": { "type": "string", "description": "Text to replace with" }
                        },
                        "required": ["target", "value"]
                    }
                }
            },
            "required": ["path", "replacements"]
        })
    }

    fn requires_approval(&self) -> savant_core::traits::ApprovalRequirement {
        savant_core::traits::ApprovalRequirement::Conditional
    }

    fn domain(&self) -> savant_core::traits::ToolDomain {
        savant_core::traits::ToolDomain::Container
    }
    async fn execute(&self, payload: Value) -> Result<String, SavantError> {
        let target_raw = payload["path"]
            .as_str()
            .ok_or_else(|| SavantError::Unknown("Missing 'path' for atomic_edit".to_string()))?;

        // Block editing of system prompt files
        if let Some(blocked) = check_blocked(Path::new(target_raw)) {
            return Err(SavantError::Unknown(format!(
                "Access denied: cannot edit system prompt file '{}'",
                blocked
            )));
        }

        let path = secure_resolve_path(&self.workspace_dir, target_raw)?;

        // Handle both array and string-encoded JSON array for replacements
        let replacements_owned;
        let replacements = if let Some(arr) = payload["replacements"].as_array() {
            arr
        } else if let Some(s) = payload["replacements"].as_str() {
            replacements_owned = serde_json::from_str::<Vec<Value>>(s).map_err(|e| {
                SavantError::Unknown(format!(
                    "Failed to parse replacements string as array: {}",
                    e
                ))
            })?;
            &replacements_owned
        } else {
            return Err(SavantError::Unknown(
                "Missing 'replacements' array for atomic_edit".to_string(),
            ));
        };

        info!(
            "[WAL:ACTUATOR] Action: atomic_edit, Path: {:?}, Changes: {}",
            path,
            replacements.len()
        );

        let mut content = fs::read_to_string(&path).await.map_err(|e| {
            SavantError::Unknown(format!("AtomicEdit: Failed to read {:?}: {}", path, e))
        })?;

        let backup_path = PathBuf::from(format!("{}.bak", path.to_string_lossy()));
        fs::copy(&path, &backup_path).await.map_err(|e| {
            SavantError::Unknown(format!("AtomicEdit: Failed to create backup: {}", e))
        })?;

        for replacement in replacements {
            let target = replacement["target"]
                .as_str()
                .ok_or_else(|| SavantError::Unknown("Missing 'target'".to_string()))?;
            let value = replacement["value"]
                .as_str()
                .ok_or_else(|| SavantError::Unknown("Missing 'value'".to_string()))?;

            if !content.contains(target) {
                if let Err(e) = fs::remove_file(&backup_path).await {
                    tracing::warn!("[foundation] Failed to remove backup file: {}", e);
                }
                return Err(SavantError::Unknown(format!(
                    "AtomicEdit: Target not found: {}",
                    target
                )));
            }
            content = content.replace(target, value);
        }

        if let Err(write_err) = fs::write(&path, &content).await {
            // PB-14: Actual rollback — restore from backup on write failure
            if let Err(rollback_err) = fs::copy(&backup_path, &path).await {
                tracing::error!(
                    "[foundation] AtomicEdit: Write failed AND rollback failed! \
                     Backup at {:?}, original at {:?}. Write error: {}, Rollback error: {}",
                    backup_path,
                    path,
                    write_err,
                    rollback_err
                );
            } else {
                tracing::warn!(
                    "[foundation] AtomicEdit: Write failed, rolled back from backup. Error: {}",
                    write_err
                );
            }
            // Clean up backup after rollback
            let _ = fs::remove_file(&backup_path).await;
            return Err(SavantError::Unknown(format!(
                "AtomicEdit: Write failed (rolled back): {}",
                write_err
            )));
        }

        if let Err(e) = fs::remove_file(&backup_path).await {
            tracing::warn!(
                "[foundation] Failed to remove backup file after atomic edit: {}",
                e
            );
        }
        Ok(format!(
            "Successfully applied {} replacements to {:?}.",
            replacements.len(),
            path
        ))
    }
}

/// Tool for file and directory creation.
pub struct FileCreateTool {
    workspace_dir: PathBuf,
    scanner: Option<Arc<savant_skills::security::SecurityScanner>>,
}

impl FileCreateTool {
    pub fn new(workspace_dir: PathBuf) -> Self {
        Self {
            workspace_dir,
            scanner: None,
        }
    }

    pub fn with_scanner(mut self, scanner: Arc<savant_skills::security::SecurityScanner>) -> Self {
        self.scanner = Some(scanner);
        self
    }
}

#[async_trait]
impl Tool for FileCreateTool {
    fn name(&self) -> &str {
        "file_create"
    }
    fn description(&self) -> &str {
        "Creates a new file with content or a new directory."
    }
    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "path": { "type": "string", "description": "Path where the file or directory should be created" },
                "content": { "type": "string", "description": "Content to write to the file (optional, defaults to empty)" },
                "directory": { "type": "boolean", "description": "Set to true to create a directory instead of a file" }
            },
            "required": ["path"]
        })
    }
    fn domain(&self) -> savant_core::traits::ToolDomain {
        savant_core::traits::ToolDomain::Container
    }
    async fn execute(&self, payload: Value) -> Result<String, SavantError> {
        let target_raw = payload["path"]
            .as_str()
            .ok_or_else(|| SavantError::Unknown("Missing 'path' for create".to_string()))?;

        // Block creating system prompt files
        if let Some(blocked) = check_blocked(Path::new(target_raw)) {
            return Err(SavantError::Unknown(format!(
                "Access denied: cannot create system prompt file '{}'",
                blocked
            )));
        }

        let path = secure_resolve_path(&self.workspace_dir, target_raw)?;

        // Check if this is a directory creation request
        if payload["directory"].as_bool().unwrap_or(false) {
            info!("[WAL:ACTUATOR] Action: create_directory, Path: {:?}", path);
            fs::create_dir_all(&path).await.map_err(|e| {
                SavantError::Unknown(format!("Failed to create directory {:?}: {}", path, e))
            })?;
            return Ok(format!("Successfully created directory: {:?}", path));
        }

        // File creation with optional content
        let content = payload["content"].as_str().unwrap_or("");

        // C4: Security scan content before writing
        if let Some(ref scanner) = self.scanner {
            let findings = scanner.scan_command(content);
            let high_or_critical = findings.iter().any(|f| {
                matches!(
                    f.severity,
                    savant_skills::security::RiskLevel::High
                        | savant_skills::security::RiskLevel::Critical
                )
            });
            if high_or_critical {
                return Err(SavantError::Unknown(format!(
                    "Security scan blocked file content: {} suspicious patterns detected",
                    findings.len()
                )));
            }
        }

        info!("[WAL:ACTUATOR] Action: create_file, Path: {:?}", path);

        // Create parent directories if they don't exist
        if let Some(parent) = path.parent() {
            if !parent.exists() {
                fs::create_dir_all(parent).await.map_err(|e| {
                    SavantError::Unknown(format!("Failed to create parent dirs: {}", e))
                })?;
            }
        }

        fs::write(&path, content).await.map_err(|e| {
            SavantError::Unknown(format!("Failed to create file {:?}: {}", path, e))
        })?;

        Ok(format!(
            "Successfully created file: {:?} ({} bytes)",
            path,
            content.len()
        ))
    }
}

/// Legacy Foundation Tool for general operations.
pub struct FoundationTool {
    workspace_dir: PathBuf,
}

impl FoundationTool {
    pub fn new(workspace_dir: PathBuf) -> Self {
        Self { workspace_dir }
    }
}

#[async_trait]
impl Tool for FoundationTool {
    fn name(&self) -> &str {
        "foundation"
    }
    fn description(&self) -> &str {
        "File system operations: read, write, list, create, mkdir."
    }
    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "action": { "type": "string", "description": "Action to perform", "enum": ["read", "write", "ls", "create", "mkdir"] },
                "path": { "type": "string", "description": "File or directory path" },
                "content": { "type": "string", "description": "Content for write/create actions" }
            },
            "required": ["action", "path"]
        })
    }
    fn domain(&self) -> savant_core::traits::ToolDomain {
        savant_core::traits::ToolDomain::Container
    }

    fn max_output_chars(&self) -> usize {
        128_000 // File read can return large contents
    }

    fn timeout_secs(&self) -> u64 {
        30 // File ops are fast
    }

    async fn execute(&self, payload: Value) -> Result<String, SavantError> {
        let action = payload["action"].as_str().unwrap_or("");

        let target_raw = payload["path"]
            .as_str()
            .ok_or_else(|| SavantError::Unknown("Missing path".into()))?;

        // Check if the target is a blocked system prompt file
        let target_path = Path::new(target_raw);
        if let Some(blocked) = check_blocked(target_path) {
            return Err(SavantError::Unknown(format!(
                "Access denied: '{}' is a system prompt file and cannot be read or written by the agent.",
                blocked
            )));
        }

        let secure_path = secure_resolve_path(&self.workspace_dir, target_raw)?;

        // Double-check after path resolution (catches renames/redirects)
        if let Some(blocked) = check_blocked(&secure_path) {
            return Err(SavantError::Unknown(format!(
                "Access denied: '{}' is a system prompt file and cannot be read or written by the agent.",
                blocked
            )));
        }

        match action {
            "read" => {
                match fs::read_to_string(&secure_path).await {
                    Ok(content) => Ok(content),
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                        Ok(format!("FILE_NOT_FOUND: {:?} does not exist. Create it first using the 'create' action.", secure_path))
                    }
                    Err(e) => Err(SavantError::Unknown(e.to_string()))
                }
            }
            "ls" => {
                let mut entries = fs::read_dir(&secure_path)
                    .await
                    .map_err(|e| SavantError::Unknown(e.to_string()))?;
                let mut out = String::new();
                while let Some(e) = entries
                    .next_entry()
                    .await
                    .map_err(|e| SavantError::Unknown(e.to_string()))?
                {
                    out.push_str(&format!("{}\n", e.file_name().to_string_lossy()));
                }
                Ok(out)
            }
            "write" => {
                let content = payload["content"].as_str().unwrap_or("");
                info!("[WAL:ACTUATOR] Action: write, Path: {:?}", secure_path);
                fs::write(&secure_path, content)
                    .await
                    .map_err(|e| SavantError::Unknown(format!("Write failed: {}", e)))?;
                Ok(format!(
                    "Successfully wrote {} bytes to {:?}",
                    content.len(),
                    secure_path
                ))
            }
            "mkdir" => {
                info!("[WAL:ACTUATOR] Action: mkdir, Path: {:?}", secure_path);
                fs::create_dir_all(&secure_path)
                    .await
                    .map_err(|e| SavantError::Unknown(format!("Mkdir failed: {}", e)))?;
                Ok(format!("Successfully created directory: {:?}", secure_path))
            }
            "create" => {
                let content = payload["content"].as_str().unwrap_or("");
                info!("[WAL:ACTUATOR] Action: create, Path: {:?}", secure_path);
                if let Some(parent) = secure_path.parent() {
                    if !parent.exists() {
                        fs::create_dir_all(parent).await.map_err(|e| {
                            SavantError::Unknown(format!("Failed to create parent dirs: {}", e))
                        })?;
                    }
                }
                fs::write(&secure_path, content)
                    .await
                    .map_err(|e| SavantError::Unknown(format!("Create failed: {}", e)))?;
                Ok(format!(
                    "Successfully created file: {:?} ({} bytes)",
                    secure_path,
                    content.len()
                ))
            }
            _ => Err(SavantError::Unknown(
                "Use specialized FS tools for destructive actions.".into(),
            )),
        }
    }
}
