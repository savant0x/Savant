//! Secure Filesystem Skill with Path Validation
//!
//! Provides sandboxed filesystem operations with:
//! - Path canonicalization to prevent traversal attacks
//! - Workspace boundary enforcement
//! - File size limits to prevent DoS
//! - Async I/O to avoid blocking the runtime

use async_trait::async_trait;
use savant_core::error::SavantError;
use std::path::{Path, PathBuf};
use tokio::fs;
use tracing::{debug, warn};

/// Maximum file size for read operations (10MB)
const MAX_READ_SIZE: u64 = 10 * 1024 * 1024;

/// Maximum content size for write operations (10MB)
const MAX_WRITE_SIZE: usize = 10 * 1024 * 1024;

/// A secure filesystem skill with workspace boundary enforcement.
pub struct FileSystemSkill {
    workspace_root: PathBuf,
}

impl FileSystemSkill {
    /// Creates a new FileSystemSkill with the specified workspace root.
    /// All file operations will be restricted to paths within this root.
    #[must_use]
    pub fn new(workspace_root: PathBuf) -> Self {
        Self { workspace_root }
    }

    /// Creates a FileSystemSkill with the current directory as workspace root.
    pub fn default_root() -> Self {
        Self {
            workspace_root: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
        }
    }

    /// Validates and canonicalizes a path, ensuring it's within the workspace boundary.
    fn validate_path(&self, path_str: &str) -> Result<PathBuf, SavantError> {
        // Reject empty paths
        if path_str.is_empty() {
            return Err(SavantError::InvalidInput("Path cannot be empty".into()));
        }

        // Reject paths with null bytes
        if path_str.contains('\0') {
            return Err(SavantError::InvalidInput(
                "Path contains invalid characters".into(),
            ));
        }

        let path = Path::new(path_str);

        // Attempt to canonicalize the path
        let canonical = match path.canonicalize() {
            Ok(p) => p,
            Err(_) => {
                // If canonicalize fails (file doesn't exist yet),
                // resolve relative to workspace and check parent exists
                let resolved = if path.is_relative() {
                    self.workspace_root.join(path)
                } else {
                    path.to_path_buf()
                };

                // Check parent directory exists and is within workspace
                if let Some(parent) = resolved.parent() {
                    let canonical_parent = parent.canonicalize().map_err(|_| {
                        SavantError::InvalidInput("Parent directory does not exist".into())
                    })?;

                    if !canonical_parent.starts_with(&self.workspace_root) {
                        warn!(
                            "Path traversal attempt blocked: {} (parent outside workspace)",
                            path_str
                        );
                        return Err(SavantError::AuthError(
                            "Path outside workspace boundary".into(),
                        ));
                    }
                }

                resolved
            }
        };

        // Final boundary check
        if !canonical.starts_with(&self.workspace_root) {
            warn!(
                "Path traversal attempt blocked: {} (resolved to {})",
                path_str,
                canonical.display()
            );
            return Err(SavantError::AuthError(
                "Path outside workspace boundary".into(),
            ));
        }

        debug!("Path validated: {} -> {}", path_str, canonical.display());
        Ok(canonical)
    }
}

impl Default for FileSystemSkill {
    fn default() -> Self {
        Self::default_root()
    }
}

#[async_trait]
impl savant_core::traits::Tool for FileSystemSkill {
    fn name(&self) -> &str {
        "filesystem"
    }

    fn description(&self) -> &str {
        "Secure filesystem operations with workspace boundary enforcement. \
         Supports 'read' and 'write' actions. All paths are validated and \
         restricted to the workspace root to prevent path traversal attacks."
    }

    async fn execute(&self, payload: serde_json::Value) -> Result<String, SavantError> {
        let action = payload
            .get("action")
            .and_then(|v| v.as_str())
            .ok_or_else(|| SavantError::InvalidInput("Missing 'action' field".into()))?;

        match action {
            "read" => {
                let path_str = payload
                    .get("path")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| SavantError::InvalidInput("Missing 'path' field".into()))?;

                let canonical_path = self.validate_path(path_str)?;

                // Check file size before reading
                let metadata = fs::metadata(&canonical_path).await.map_err(|e| {
                    SavantError::IoError(std::io::Error::new(
                        std::io::ErrorKind::NotFound,
                        format!("File not found: {}", e),
                    ))
                })?;

                if metadata.len() > MAX_READ_SIZE {
                    return Err(SavantError::InvalidInput(format!(
                        "File too large: {} bytes (max: {} bytes)",
                        metadata.len(),
                        MAX_READ_SIZE
                    )));
                }

                let content = fs::read_to_string(&canonical_path)
                    .await
                    .map_err(SavantError::IoError)?;

                debug!(
                    "Read {} bytes from {}",
                    content.len(),
                    canonical_path.display()
                );
                Ok(content)
            }
            "write" => {
                let path_str = payload
                    .get("path")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| SavantError::InvalidInput("Missing 'path' field".into()))?;

                let content = payload
                    .get("content")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| SavantError::InvalidInput("Missing 'content' field".into()))?;

                // Check content size before writing
                if content.len() > MAX_WRITE_SIZE {
                    return Err(SavantError::InvalidInput(format!(
                        "Content too large: {} bytes (max: {} bytes)",
                        content.len(),
                        MAX_WRITE_SIZE
                    )));
                }

                let canonical_path = self.validate_path(path_str)?;

                // Create parent directories if they don't exist
                if let Some(parent) = canonical_path.parent() {
                    fs::create_dir_all(parent)
                        .await
                        .map_err(SavantError::IoError)?;
                }

                fs::write(&canonical_path, content)
                    .await
                    .map_err(SavantError::IoError)?;

                debug!(
                    "Wrote {} bytes to {}",
                    content.len(),
                    canonical_path.display()
                );
                Ok(format!(
                    "Successfully wrote {} bytes to {}",
                    content.len(),
                    path_str
                ))
            }
            "list" => {
                let path_str = payload
                    .get("path")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| SavantError::InvalidInput("Missing 'path' field".into()))?;

                let canonical_path = self.validate_path(path_str)?;

                let mut entries = fs::read_dir(&canonical_path)
                    .await
                    .map_err(SavantError::IoError)?;

                let mut result = Vec::new();
                while let Some(entry) = entries.next_entry().await.map_err(SavantError::IoError)? {
                    let name = entry.file_name().to_string_lossy().to_string();
                    let metadata = entry.metadata().await.map_err(SavantError::IoError)?;

                    let entry_type = if metadata.is_dir() { "dir" } else { "file" };
                    result.push(format!("{} [{}]", name, entry_type));
                }

                Ok(result.join("\n"))
            }
            _ => Err(SavantError::InvalidInput(format!(
                "Unknown action: {}",
                action
            ))),
        }
    }
}

#[cfg(test)]
#[cfg(not(windows))] // Unix path tests - Windows has different path semantics
mod tests {
    use super::*;
    use savant_core::traits::Tool;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_path_traversal_blocked() {
        let dir = tempdir().unwrap();
        let skill = FileSystemSkill::new(dir.path().to_path_buf());

        // Attempt to read outside workspace
        let payload = serde_json::json!({
            "action": "read",
            "path": "../../../etc/passwd"
        });

        let result = skill.execute(payload).await;
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("workspace boundary"));
    }

    #[tokio::test]
    async fn test_valid_read_write() {
        let dir = tempdir().unwrap();
        let skill = FileSystemSkill::new(dir.path().to_path_buf());

        // Write a file
        let write_payload = serde_json::json!({
            "action": "write",
            "path": "test.txt",
            "content": "Hello, World!"
        });

        let write_result = skill.execute(write_payload).await;
        assert!(write_result.is_ok());

        // Read it back
        let read_payload = serde_json::json!({
            "action": "read",
            "path": "test.txt"
        });

        let read_result = skill.execute(read_payload).await;
        assert_eq!(read_result.unwrap(), "Hello, World!");
    }

    #[tokio::test]
    async fn test_file_size_limit() {
        let dir = tempdir().unwrap();
        let skill = FileSystemSkill::new(dir.path().to_path_buf());

        // Attempt to write content exceeding limit
        let large_content = "x".repeat(MAX_WRITE_SIZE + 1);
        let payload = serde_json::json!({
            "action": "write",
            "path": "large.txt",
            "content": large_content
        });

        let result = skill.execute(payload).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("too large"));
    }
}
