//! Workspace Guard — path validation for sub-agent file operations.
//!
//! Sub-agents operate in a temporary directory. The WorkspaceGuard validates
//! that all file paths are within the allowed root, preventing accidental
//! modification of the parent's workspace files.

use std::path::{Path, PathBuf};

/// Validates file paths for sub-agent operations.
pub struct WorkspaceGuard {
    root: PathBuf,
    read_only_mounts: Vec<PathBuf>,
}

impl WorkspaceGuard {
    /// Create a new workspace guard with the given root directory.
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            read_only_mounts: Vec::new(),
        }
    }

    /// Add a read-only mount point.
    pub fn with_read_only_mount(mut self, path: PathBuf) -> Self {
        self.read_only_mounts.push(path);
        self
    }

    /// Validate that a path is within the allowed root or a read-only mount.
    /// Returns the canonical path if valid, or an error if outside bounds.
    pub fn validate_path(&self, path: &Path) -> Result<PathBuf, String> {
        // Try to canonicalize, fall back to the original path if it doesn't exist yet
        let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());

        if canonical.starts_with(&self.root) {
            return Ok(canonical);
        }

        for mount in &self.read_only_mounts {
            if canonical.starts_with(mount) {
                return Ok(canonical);
            }
        }

        Err(format!(
            "Path '{}' is outside workspace root '{}'",
            path.display(),
            self.root.display()
        ))
    }

    /// Check if a path is writable (must be within root, not a read-only mount).
    pub fn is_writable(&self, path: &Path) -> bool {
        let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());

        // Must be within root
        if !canonical.starts_with(&self.root) {
            return false;
        }

        // Must not be in a read-only mount
        for mount in &self.read_only_mounts {
            if canonical.starts_with(mount) {
                return false;
            }
        }

        true
    }

    /// Get the workspace root.
    pub fn root(&self) -> &Path {
        &self.root
    }
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_validate_path_within_root() {
        let guard = WorkspaceGuard::new(PathBuf::from("/tmp/workspace"));
        let result = guard.validate_path(Path::new("/tmp/workspace/file.rs"));
        assert!(result.is_ok());
    }

    #[test]
    fn test_validate_path_outside_root() {
        let guard = WorkspaceGuard::new(PathBuf::from("/tmp/workspace"));
        let result = guard.validate_path(Path::new("/etc/passwd"));
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_path_read_only_mount() {
        let guard = WorkspaceGuard::new(PathBuf::from("/tmp/workspace"))
            .with_read_only_mount(PathBuf::from("/tmp/shared"));
        let result = guard.validate_path(Path::new("/tmp/shared/data.rs"));
        assert!(result.is_ok());
        assert!(!guard.is_writable(Path::new("/tmp/shared/data.rs")));
    }

    #[test]
    fn test_is_writable() {
        let guard = WorkspaceGuard::new(PathBuf::from("/tmp/workspace"))
            .with_read_only_mount(PathBuf::from("/tmp/shared"));
        assert!(guard.is_writable(Path::new("/tmp/workspace/file.rs")));
        assert!(!guard.is_writable(Path::new("/tmp/shared/file.rs")));
        assert!(!guard.is_writable(Path::new("/etc/passwd")));
    }
}
