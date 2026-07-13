//! ClawHub client for skill discovery and installation
//!
//! Provides integration with the ClawHub skill registry for:
//! - Searching for skills
//! - Installing skills with automatic security scanning
//! - Checking for updates
//! - Managing installed skills

use crate::parser::{InstallResult, SecurityGateResult};
use crate::security::SecurityScanner;
use regex;
use serde::{Deserialize, Serialize};
use std::path::Path;
use tracing::{info, warn};

/// RAII guard that ensures temp directory cleanup on drop
struct TempDirGuard {
    path: Option<std::path::PathBuf>,
}

impl TempDirGuard {
    fn new(path: std::path::PathBuf) -> Self {
        Self { path: Some(path) }
    }

    /// Consume the guard without cleanup (e.g., on successful move)
    fn keep(mut self) {
        self.path = None;
    }
}

impl Drop for TempDirGuard {
    fn drop(&mut self) {
        if let Some(ref path) = self.path {
            if let Err(e) = std::fs::remove_dir_all(path) {
                tracing::warn!(
                    "[skills::clawhub] Failed to remove temp directory {}: {}",
                    path.display(),
                    e
                );
            }
        }
    }
}

/// ClawHub API base URL
const CLAWHUB_API_BASE: &str = "https://api.clawhub.com/v1";
/// ClawHub web base URL
const CLAWHUB_WEB_BASE: &str = "https://clawhub.com";

/// Skill search result from ClawHub
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillSearchResult {
    /// Unique slug identifier (e.g., "username/skill-name")
    pub slug: String,
    /// Skill name
    pub name: String,
    /// Short description
    pub description: String,
    /// Author username
    pub author: String,
    /// Download count
    pub downloads: u64,
    /// Average rating (0-5)
    pub rating: f32,
    /// Number of reviews
    pub review_count: u32,
    /// When the skill was last updated
    pub updated_at: String,
    /// Version string
    pub version: String,
    /// Whether the skill is verified
    pub verified: bool,
    /// Categories/tags
    pub tags: Vec<String>,
}

/// Detailed skill info from ClawHub
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillDetail {
    /// Slug identifier
    pub slug: String,
    /// Full skill name
    pub name: String,
    /// Description
    pub description: String,
    /// Author info
    pub author: SkillAuthor,
    /// Full SKILL.md content
    pub content: String,
    /// Additional files in the skill package
    pub files: Vec<SkillFileInfo>,
    /// Download count
    pub downloads: u64,
    /// Rating
    pub rating: f32,
    /// Version
    pub version: String,
    /// Release date
    pub released_at: String,
    /// Last update
    pub updated_at: String,
    /// Changelog/release notes for this version
    pub changelog: Option<String>,
    /// License
    pub license: Option<String>,
    /// Homepage URL
    pub homepage: Option<String>,
    /// Repository URL
    pub repository: Option<String>,
    /// Dependencies/requirements
    pub requirements: SkillRequirements,
    /// Security scan status from ClawHub
    pub security_status: ClawHubSecurityStatus,
}

/// A file included in a skill package
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillFileInfo {
    /// Relative path within the skill directory
    pub path: String,
    /// File content (base64 encoded for binary, plain text for text)
    pub content: String,
    /// File size in bytes
    pub size: u64,
}

/// Author information
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillAuthor {
    pub username: String,
    pub display_name: Option<String>,
    pub verified: bool,
    pub skill_count: u32,
}

/// Skill requirements
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillRequirements {
    /// Required binaries
    pub bins: Vec<String>,
    /// Required environment variables
    pub env: Vec<String>,
    /// Supported operating systems
    pub os: Vec<String>,
    /// Minimum Savant version
    pub min_version: Option<String>,
}

/// ClawHub security status for a skill
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ClawHubSecurityStatus {
    /// Passed automated security scan
    Clean,
    /// Flagged for manual review
    UnderReview,
    /// Known malicious content
    Blocked,
    /// Not yet scanned
    Unscanned,
}

/// Update information for an installed skill
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateInfo {
    /// Skill slug
    pub slug: String,
    /// Currently installed version
    pub current_version: String,
    /// Available version
    pub available_version: String,
    /// Update changelog
    pub changelog: Option<String>,
    /// Whether this is a security update
    pub security_update: bool,
}

/// ClawHub API client
pub struct ClawHubClient {
    http_client: reqwest::Client,
    api_base: String,
    web_base: String,
}

impl ClawHubClient {
    /// Create a new ClawHub client
    pub fn new() -> Self {
        Self {
            http_client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .unwrap_or_default(),
            api_base: CLAWHUB_API_BASE.to_string(),
            web_base: CLAWHUB_WEB_BASE.to_string(),
        }
    }

    /// Create a client with custom base URLs (for testing)
    #[cfg(test)]
    pub fn with_base_urls(api_base: &str, web_base: &str) -> Self {
        Self {
            http_client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .unwrap_or_default(),
            api_base: api_base.to_string(),
            web_base: web_base.to_string(),
        }
    }

    /// Search for skills on ClawHub
    pub async fn search(&self, query: &str) -> Result<Vec<SkillSearchResult>, ClawHubError> {
        let url = format!("{}/skills/search", self.api_base);

        let response = self
            .http_client
            .get(&url)
            .query(&[("q", query), ("limit", "20")])
            .send()
            .await
            .map_err(|e| ClawHubError::NetworkError(e.to_string()))?;

        if !response.status().is_success() {
            return Err(ClawHubError::ApiError(
                response.status().as_u16(),
                format!("Search failed: {}", response.status()),
            ));
        }

        let results: Vec<SkillSearchResult> = response
            .json()
            .await
            .map_err(|e| ClawHubError::ParseError(e.to_string()))?;

        Ok(results)
    }

    /// Get detailed info about a skill
    pub async fn get_skill_info(&self, slug: &str) -> Result<SkillDetail, ClawHubError> {
        let url = format!("{}/skills/{}", self.api_base, slug);

        let response = self
            .http_client
            .get(&url)
            .send()
            .await
            .map_err(|e| ClawHubError::NetworkError(e.to_string()))?;

        if !response.status().is_success() {
            return Err(ClawHubError::ApiError(
                response.status().as_u16(),
                format!("Skill not found: {}", slug),
            ));
        }

        let detail: SkillDetail = response
            .json()
            .await
            .map_err(|e| ClawHubError::ParseError(e.to_string()))?;

        Ok(detail)
    }

    /// Install a skill from ClawHub with security scanning
    #[allow(clippy::disallowed_methods)]
    pub async fn install(
        &self,
        slug: &str,
        target_dir: &Path,
        scanner: &SecurityScanner,
    ) -> Result<InstallResult, ClawHubError> {
        // Validate slug format
        use std::sync::LazyLock;
        static SLUG_REGEX: LazyLock<regex::Regex> =
            LazyLock::new(|| regex::Regex::new(r"^[a-zA-Z0-9_-]+$").unwrap());
        let normalized_slug = slug.replace('/', "-");
        if slug.contains("..")
            || slug.starts_with('.')
            || slug.starts_with('-')
            || !SLUG_REGEX.is_match(&normalized_slug)
        {
            return Err(ClawHubError::ParseError(format!(
                "Invalid slug '{}': must match ^[a-zA-Z0-9_-]+$ (after replacing / with -), \
                 and must not contain '..' or start with '.' or '-'",
                slug
            )));
        }

        info!("Installing skill from ClawHub: {}", slug);

        // 1. Get skill info to verify it exists
        let info = self.get_skill_info(slug).await?;

        // 2. Check ClawHub security status
        match info.security_status {
            ClawHubSecurityStatus::Blocked => {
                return Err(ClawHubError::SecurityBlocked(
                    "This skill has been blocked by ClawHub for security reasons".to_string(),
                ));
            }
            ClawHubSecurityStatus::UnderReview => {
                warn!("Skill '{}' is under security review on ClawHub", slug);
            }
            _ => {}
        }

        // 3. Download skill content to temp directory for scanning
        let temp_dir =
            std::env::temp_dir().join(format!("clawhub-scan-{}", slug.replace('/', "-")));
        tokio::fs::create_dir_all(&temp_dir)
            .await
            .map_err(|e| ClawHubError::FileSystemError(e.to_string()))?;

        // RAII guard ensures cleanup on any exit path
        let _cleanup = TempDirGuard::new(temp_dir.clone());

        // Write SKILL.md to temp location
        tokio::fs::write(temp_dir.join("SKILL.md"), &info.content)
            .await
            .map_err(|e| ClawHubError::FileSystemError(e.to_string()))?;

        // Download additional files if any (templates, assets, etc.)
        for file in &info.files {
            // Path traversal protection: reject absolute paths and parent directory references
            if file.path.starts_with('/') || file.path.starts_with('\\') || file.path.contains("..")
            {
                warn!("Rejected file with path traversal attempt: {}", file.path);
                continue;
            }
            let file_path = temp_dir.join(&file.path);
            // Verify the resolved path stays within temp_dir
            if let Ok(canonical_temp) = temp_dir.canonicalize() {
                let resolved = file_path.canonicalize().or_else(|_| {
                    // File doesn't exist yet, check parent
                    file_path
                        .parent()
                        .and_then(|p| p.canonicalize().ok())
                        .ok_or_else(|| {
                            std::io::Error::new(std::io::ErrorKind::NotFound, "parent not found")
                        })
                });
                if let Ok(canonical_file) = resolved {
                    if !canonical_file.starts_with(&canonical_temp) {
                        warn!("Rejected file escaping temp directory: {}", file.path);
                        continue;
                    }
                }
            }
            if let Some(parent) = file_path.parent() {
                if let Err(e) = tokio::fs::create_dir_all(parent).await {
                    warn!("[clawhub] Failed to create parent directory: {}", e);
                }
            }
            if let Err(e) = tokio::fs::write(&file_path, &file.content).await {
                warn!(
                    "[clawhub] Failed to write file {}: {}",
                    file_path.display(),
                    e
                );
            }
        }

        // 4. MANDATORY SECURITY SCAN BEFORE INSTALLING
        let scan_result = scanner
            .scan_skill_mandatory(&temp_dir)
            .await
            .map_err(|e| ClawHubError::ScanError(e.to_string()))?;

        let gate_result = SecurityGateResult::from_risk_level(scan_result);

        // If auto-approved (Clean/Low), move to final location
        if gate_result.is_approved() {
            let skill_dir = target_dir.join(slug.replace('/', "-"));
            tokio::fs::create_dir_all(&skill_dir)
                .await
                .map_err(|e| ClawHubError::FileSystemError(e.to_string()))?;

            // Move files from temp to final location
            move_dir_contents(&temp_dir, &skill_dir).await?;
            _cleanup.keep(); // Files moved, guard should not clean up

            info!("Successfully installed skill: {}", slug);

            return Ok(InstallResult {
                success: true,
                skill_name: info.name.clone(),
                gate_result: Some(gate_result),
                message: format!("Skill '{}' installed successfully", info.name),
            });
        }

        // If needs approval, keep temp files and return pending result
        let clicks_required = gate_result.required_clicks();
        info!(
            "Skill '{}' requires {} click(s) of user approval",
            slug, clicks_required
        );

        Ok(InstallResult {
            success: false,
            skill_name: info.name.clone(),
            gate_result: Some(gate_result),
            message: format!(
                "Skill '{}' requires {} click(s) of approval before installation",
                info.name, clicks_required
            ),
        })
    }

    /// Check for updates for installed skills
    pub async fn check_updates(
        &self,
        installed_slugs: &[String],
        installed_versions: &[String],
    ) -> Result<Vec<UpdateInfo>, ClawHubError> {
        let mut updates = Vec::new();

        for (slug, current_version) in installed_slugs.iter().zip(installed_versions.iter()) {
            match self.get_skill_info(slug).await {
                Ok(info) => {
                    if info.version != *current_version {
                        updates.push(UpdateInfo {
                            slug: slug.clone(),
                            current_version: current_version.clone(),
                            available_version: info.version,
                            changelog: info.changelog,
                            security_update: matches!(
                                info.security_status,
                                ClawHubSecurityStatus::Clean
                            ),
                        });
                    }
                }
                Err(e) => {
                    warn!("Failed to check update for {}: {}", slug, e);
                }
            }
        }

        Ok(updates)
    }

    /// Get the web URL for a skill
    pub fn get_skill_url(&self, slug: &str) -> String {
        format!("{}/skill/{}", self.web_base, slug)
    }

    /// Get the search URL
    pub fn get_search_url(&self, query: &str) -> String {
        format!("{}/search?q={}", self.web_base, urlencoding::encode(query))
    }
}

impl Default for ClawHubClient {
    fn default() -> Self {
        Self::new()
    }
}

/// Errors from ClawHub operations
#[derive(Debug, thiserror::Error)]
pub enum ClawHubError {
    #[error("Network error: {0}")]
    NetworkError(String),

    #[error("API error {0}: {1}")]
    ApiError(u16, String),

    #[error("Parse error: {0}")]
    ParseError(String),

    #[error("Skill blocked for security: {0}")]
    SecurityBlocked(String),

    #[error("Security scan failed: {0}")]
    ScanError(String),

    #[error("File system error: {0}")]
    FileSystemError(String),

    #[error("Skill not found: {0}")]
    NotFound(String),
}

/// Simple URL encoding for search queries
mod urlencoding {
    pub fn encode(s: &str) -> String {
        s.chars()
            .map(|c| match c {
                'a'..='z' | 'A'..='Z' | '0'..='9' | '-' | '_' | '.' | '~' => c.to_string(),
                ' ' => "+".to_string(),
                _ => format!("%{:02X}", c as u8),
            })
            .collect()
    }
}

/// Move directory contents from source to destination
async fn move_dir_contents(
    src: &std::path::Path,
    dst: &std::path::Path,
) -> Result<(), ClawHubError> {
    let mut entries = tokio::fs::read_dir(src)
        .await
        .map_err(|e| ClawHubError::FileSystemError(e.to_string()))?;

    while let Some(entry) = entries
        .next_entry()
        .await
        .map_err(|e| ClawHubError::FileSystemError(e.to_string()))?
    {
        let src_path = entry.path();
        let file_name = src_path
            .file_name()
            .ok_or_else(|| ClawHubError::FileSystemError("Invalid filename".to_string()))?;
        let dst_path = dst.join(file_name);

        if src_path.is_dir() {
            tokio::fs::create_dir_all(&dst_path)
                .await
                .map_err(|e| ClawHubError::FileSystemError(e.to_string()))?;
            Box::pin(move_dir_contents(&src_path, &dst_path)).await?;
        } else {
            tokio::fs::rename(&src_path, &dst_path)
                .await
                .map_err(|e| ClawHubError::FileSystemError(e.to_string()))?;
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_url_encoding() {
        assert_eq!(urlencoding::encode("hello world"), "hello+world");
        assert_eq!(urlencoding::encode("test&query"), "test%26query");
    }

    #[test]
    fn test_skill_url_generation() {
        let client = ClawHubClient::new();
        assert_eq!(
            client.get_skill_url("user/my-skill"),
            "https://clawhub.com/skill/user/my-skill"
        );
    }

    #[test]
    fn test_search_url_generation() {
        let client = ClawHubClient::new();
        assert_eq!(
            client.get_search_url("google"),
            "https://clawhub.com/search?q=google"
        );
    }
}
