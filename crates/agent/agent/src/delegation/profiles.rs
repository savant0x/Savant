//! Profile loader — loads sub-agent profiles from `profiles/` directories.
//!
//! Each profile directory must contain `SOUL.md` (required) and may contain
//! `tools.toml` and `constraints.toml` (optional, defaults applied if missing).

use savant_core::types::SubAgentProfile;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::RwLock;

/// Loads and caches sub-agent profiles from workspace directories.
pub struct ProfileLoader {
    profiles: Arc<RwLock<HashMap<String, SubAgentProfile>>>,
    workspace_roots: Vec<PathBuf>,
}

impl ProfileLoader {
    /// Create a new profile loader with the given workspace roots.
    pub fn new(workspace_roots: Vec<PathBuf>) -> Self {
        Self {
            profiles: Arc::new(RwLock::new(HashMap::new())),
            workspace_roots,
        }
    }

    /// Load all profiles from all workspace roots.
    pub async fn load_all(&self) -> Result<usize, String> {
        let mut profiles = self.profiles.write().await;
        let mut count = 0;

        for root in &self.workspace_roots {
            let profiles_dir = root.join("profiles");
            if !profiles_dir.exists() {
                continue;
            }

            let entries = std::fs::read_dir(&profiles_dir)
                .map_err(|e| format!("Failed to read profiles dir: {}", e))?;

            for entry in entries {
                let entry = entry.map_err(|e| format!("Failed to read entry: {}", e))?;
                let path = entry.path();
                if !path.is_dir() {
                    continue;
                }

                match self.load_profile(&path).await {
                    Ok(profile) => {
                        count += 1;
                        profiles.insert(profile.name.clone(), profile);
                    }
                    Err(e) => {
                        tracing::warn!(
                            path = %path.display(),
                            error = %e,
                            "Failed to load profile — skipping"
                        );
                    }
                }
            }
        }

        tracing::info!(count = count, "Loaded sub-agent profiles");
        Ok(count)
    }

    /// Load a single profile from a directory.
    async fn load_profile(&self, dir: &Path) -> Result<SubAgentProfile, String> {
        let name = dir
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or("Invalid profile directory name")?
            .to_string();

        // Read SOUL.md (required)
        let soul_path = dir.join("SOUL.md");
        if !soul_path.exists() {
            return Err(format!("Missing SOUL.md in profile '{}'", name));
        }
        let soul = std::fs::read_to_string(&soul_path)
            .map_err(|e| format!("Failed to read SOUL.md: {}", e))?;

        // Read tools.toml (optional)
        let tools_path = dir.join("tools.toml");
        let allowed_tools = if tools_path.exists() {
            let content = std::fs::read_to_string(&tools_path)
                .map_err(|e| format!("Failed to read tools.toml: {}", e))?;
            Self::parse_tools_list(&content)
        } else {
            Vec::new()
        };

        // Read constraints.toml (optional)
        let constraints_path = dir.join("constraints.toml");
        let (max_iterations, timeout_secs, can_delegate, max_tokens) = if constraints_path.exists()
        {
            let content = std::fs::read_to_string(&constraints_path)
                .map_err(|e| format!("Failed to read constraints.toml: {}", e))?;
            Self::parse_constraints(&content)
        } else {
            (50, 300, false, 0)
        };

        Ok(SubAgentProfile {
            name,
            soul,
            allowed_tools,
            max_iterations,
            timeout_secs,
            can_delegate,
            preferred_model: None,
            max_concurrent: 8,
            max_tokens,
        })
    }

    /// Parse tools.toml content into a list of allowed tool names.
    /// Expected format:
    /// ```toml
    /// tools = ["read", "write", "edit", "cargo"]
    /// ```
    fn parse_tools_list(content: &str) -> Vec<String> {
        // Simple parser — look for `tools = [...]` line
        for line in content.lines() {
            let trimmed = line.trim();
            if let Some(rest) = trimmed.strip_prefix("tools") {
                let rest = rest.trim();
                if let Some(array) = rest.strip_prefix('=').map(|s| s.trim()) {
                    if array.starts_with('[') && array.ends_with(']') {
                        let inner = &array[1..array.len() - 1];
                        return inner
                            .split(',')
                            .map(|s| s.trim().trim_matches('"').trim().to_string())
                            .filter(|s| !s.is_empty())
                            .collect();
                    }
                }
            }
        }
        Vec::new()
    }

    /// Parse constraints.toml content into (max_iterations, timeout_secs, can_delegate, max_tokens).
    /// Expected format:
    /// ```toml
    /// max_iterations = 50
    /// timeout_secs = 300
    /// can_delegate = false
    /// max_tokens = 0
    /// ```
    fn parse_constraints(content: &str) -> (usize, u64, bool, usize) {
        let mut max_iterations = 50;
        let mut timeout_secs = 300;
        let mut can_delegate = false;
        let mut max_tokens = 0;

        for line in content.lines() {
            let trimmed = line.trim();
            if let Some(rest) = trimmed.strip_prefix("max_iterations") {
                if let Some(val) = rest.trim().strip_prefix('=').map(|s| s.trim()) {
                    max_iterations = val.parse().unwrap_or(50);
                }
            } else if let Some(rest) = trimmed.strip_prefix("timeout_secs") {
                if let Some(val) = rest.trim().strip_prefix('=').map(|s| s.trim()) {
                    timeout_secs = val.parse().unwrap_or(300);
                }
            } else if let Some(rest) = trimmed.strip_prefix("can_delegate") {
                if let Some(val) = rest.trim().strip_prefix('=').map(|s| s.trim()) {
                    can_delegate = val.parse().unwrap_or(false);
                }
            } else if let Some(rest) = trimmed.strip_prefix("max_tokens") {
                if let Some(val) = rest.trim().strip_prefix('=').map(|s| s.trim()) {
                    max_tokens = val.parse().unwrap_or(0);
                }
            }
        }

        (max_iterations, timeout_secs, can_delegate, max_tokens)
    }

    /// Get a profile by name.
    pub async fn get(&self, name: &str) -> Option<SubAgentProfile> {
        let profiles = self.profiles.read().await;
        profiles.get(name).cloned()
    }

    /// Get all profile names.
    pub async fn names(&self) -> Vec<String> {
        let profiles = self.profiles.read().await;
        profiles.keys().cloned().collect()
    }

    /// Refresh profiles from disk (hot-reload).
    pub async fn refresh(&self) -> Result<usize, String> {
        self.load_all().await
    }
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_tools_list() {
        let content = r#"
tools = ["read", "write", "edit", "cargo"]
"#;
        let tools = ProfileLoader::parse_tools_list(content);
        assert_eq!(tools, vec!["read", "write", "edit", "cargo"]);
    }

    #[test]
    fn test_parse_tools_list_empty() {
        let content = "# no tools here\n";
        let tools = ProfileLoader::parse_tools_list(content);
        assert!(tools.is_empty());
    }

    #[test]
    fn test_parse_constraints() {
        let content = r#"
max_iterations = 30
timeout_secs = 180
can_delegate = true
max_tokens = 50000
"#;
        let (iter, timeout, delegate, tokens) = ProfileLoader::parse_constraints(content);
        assert_eq!(iter, 30);
        assert_eq!(timeout, 180);
        assert!(delegate);
        assert_eq!(tokens, 50000);
    }

    #[test]
    fn test_parse_constraints_defaults() {
        let content = "# empty\n";
        let (iter, timeout, delegate, tokens) = ProfileLoader::parse_constraints(content);
        assert_eq!(iter, 50);
        assert_eq!(timeout, 300);
        assert!(!delegate);
        assert_eq!(tokens, 0);
    }
}
