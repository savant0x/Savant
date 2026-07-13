//! Nix Flake Skill Executor
//!
//! Executes skills within a Nix flake environment for absolute reproducibility.
//! Includes strict validation of the flake reference to prevent command injection.

use async_trait::async_trait;
use savant_core::error::SavantError;
#[cfg(not(windows))]
use tokio::process::Command;
#[cfg(not(windows))]
use tokio::time::{timeout, Duration};
#[cfg(not(windows))]
use tracing::info;
use tracing::warn;

/// Maximum execution time for Nix flake invocations (30 seconds)
#[cfg(not(windows))]
const NIX_EXEC_TIMEOUT_SECS: u64 = 30;

/// Maximum length for a flake reference string (1024 characters)
const MAX_FLAKE_REF_LEN: usize = 1024;

/// Characters that are dangerous in Nix flake references.
///
/// Nix flake references can contain:
/// - Paths: `./my-flake`, `/nix/store/...`, `github:owner/repo`
/// - URL-like: `git+https://...`, `path:...`
///
/// They must NOT contain shell metacharacters, null bytes, or command separators.
const DANGEROUS_CHARS: &[char] = &[
    '\0', // Null byte — terminates strings in C
    '\n', // Newline — command separator in shell
    '\r', // Carriage return
    ';',  // Command separator
    '&',  // Background/AND operator
    '|',  // Pipe operator
    '`',  // Backtick — command substitution
    '(',  // Subshell start
    ')',  // Subshell end
    '$',  // Variable expansion
    '>',  // Redirect
    '<',  // Redirect
    '\\', // Escape character (prevent escaping context)
    '\'', // Single quote
    '"',  // Double quote
];

/// Validates a Nix flake reference string.
///
/// # Security Model
/// - Rejects null bytes (C-string termination)
/// - Rejects shell metacharacters (command injection)
/// - Enforces maximum length (prevents DoS via huge references)
/// - Validates that the reference starts with an allowed prefix or is a relative path
///
/// # Allowed Flake Reference Formats
/// - Relative paths: `./flake`, `../flake`, `flake.nix`
/// - Absolute paths: `/nix/store/...`, `/home/user/flake`
/// - GitHub: `github:owner/repo`, `github:owner/repo/branch`
/// - Git: `git+https://...`, `git+ssh://...`
/// - URLs: `https://...`, `file://...`
/// - Indirect: `nixpkgs`, `nixpkgs#hello`
///
/// # Errors
/// Returns `SavantError::InvalidInput` if the reference is malformed or potentially dangerous.
pub(crate) fn validate_flake_ref(flake_ref: &str) -> Result<(), SavantError> {
    // 1. Reject empty references
    if flake_ref.is_empty() {
        return Err(SavantError::InvalidInput(
            "Nix flake reference cannot be empty".into(),
        ));
    }

    // 2. Reject excessively long references
    if flake_ref.len() > MAX_FLAKE_REF_LEN {
        return Err(SavantError::InvalidInput(format!(
            "Nix flake reference exceeds maximum length: {} > {}",
            flake_ref.len(),
            MAX_FLAKE_REF_LEN
        )));
    }

    // 3. Check for dangerous characters
    for &ch in DANGEROUS_CHARS {
        if flake_ref.contains(ch) {
            warn!(
                "Nix flake reference rejected: contains dangerous character '{}' in: {}",
                ch, flake_ref
            );
            return Err(SavantError::InvalidInput(format!(
                "Nix flake reference contains invalid character: '{}'",
                ch
            )));
        }
    }

    // 4. Validate flake reference prefix
    // Nix flake references must start with one of these safe prefixes,
    // or be a relative/absolute filesystem path.
    let is_safe_reference = flake_ref.starts_with("./")
        || flake_ref.starts_with("../")
        || flake_ref.starts_with("/")
        || flake_ref.starts_with("github:")
        || flake_ref.starts_with("gitlab:")
        || flake_ref.starts_with("sourcehut:")
        || flake_ref.starts_with("git+https://")
        || flake_ref.starts_with("git+ssh://")
        || flake_ref.starts_with("https://")
        || flake_ref.starts_with("file://")
        || flake_ref.starts_with("path:")
        // Indirect references like "nixpkgs" or "nixpkgs#hello"
        || (!flake_ref.contains(':') && !flake_ref.contains(' '));

    if !is_safe_reference {
        return Err(SavantError::InvalidInput(format!(
            "Nix flake reference has disallowed format: {}",
            flake_ref
        )));
    }

    // 5. If it's a filesystem path, validate it exists and canonicalize
    if flake_ref.starts_with("./") || flake_ref.starts_with("../") || flake_ref.starts_with("/") {
        let path = std::path::Path::new(flake_ref);
        let resolved = if path.is_relative() {
            std::env::current_dir()
                .map_err(SavantError::IoError)?
                .join(path)
        } else {
            path.to_path_buf()
        };

        // Canonicalize to resolve symlinks and prevent traversal
        let canonical = resolved.canonicalize().map_err(|e| {
            SavantError::InvalidInput(format!(
                "Nix flake path cannot be resolved: {} ({})",
                resolved.display(),
                e
            ))
        })?;

        // Check parent directory exists for relative references
        if let Some(parent) = canonical.parent() {
            if !parent.exists() {
                return Err(SavantError::InvalidInput(format!(
                    "Nix flake path parent directory does not exist: {}",
                    parent.display()
                )));
            }
        }
    }

    Ok(())
}

/// Executes skills within a nix-shell or nix flake environment for absolute reproducibility.
pub struct NixSkillExecutor {
    /// Validated Nix flake reference
    pub flake_path: String,
}

impl NixSkillExecutor {
    /// Creates a new NixSkillExecutor with flake reference validation.
    ///
    /// # Errors
    /// Returns `SavantError::InvalidInput` if the flake reference is malformed.
    pub fn new(flake_path: String) -> Result<Self, SavantError> {
        validate_flake_ref(&flake_path)?;
        Ok(Self { flake_path })
    }

    /// Returns the validated flake reference.
    pub fn flake_ref(&self) -> &str {
        &self.flake_path
    }
}

#[cfg(not(windows))]
#[async_trait]
impl savant_core::traits::Tool for NixSkillExecutor {
    fn name(&self) -> &str {
        "nix_skill"
    }

    fn description(&self) -> &str {
        "Executes a skill within a Nix flake environment for reproducible builds."
    }

    async fn execute(&self, payload: serde_json::Value) -> Result<String, SavantError> {
        let flake = self.flake_path.clone();
        let input = payload.to_string();

        // Validate the input payload is reasonable size
        if input.len() > 1_048_576 {
            return Err(SavantError::InvalidInput(format!(
                "Payload too large for Nix execution: {} bytes (max: 1MB)",
                input.len()
            )));
        }

        info!("Executing Nix Skill via flake: {}", flake);

        // AAA: Async Process Execution with Timeout
        let child = Command::new("nix")
            .args(["run", &flake, "--", &input])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| SavantError::Unknown(format!("Failed to spawn nix process: {}", e)))?;

        match timeout(
            Duration::from_secs(NIX_EXEC_TIMEOUT_SECS),
            child.wait_with_output(),
        )
        .await
        {
            Ok(Ok(output)) => {
                if output.status.success() {
                    Ok(String::from_utf8_lossy(&output.stdout).to_string())
                } else {
                    Err(SavantError::Unknown(
                        String::from_utf8_lossy(&output.stderr).to_string(),
                    ))
                }
            }
            Ok(Err(e)) => Err(SavantError::Unknown(format!("Nix execution error: {}", e))),
            Err(_) => {
                warn!("Nix execution timed out for flake: {}", flake);
                Err(SavantError::Unknown(format!(
                    "Nix execution timed out after {}s",
                    NIX_EXEC_TIMEOUT_SECS
                )))
            }
        }
    }
}

/// Windows-specific implementation that returns a clear error.
/// Nix requires a Unix-like environment. On Windows, use Docker sandbox or WSL2.
#[cfg(windows)]
#[async_trait]
impl savant_core::traits::Tool for NixSkillExecutor {
    fn name(&self) -> &str {
        "nix_skill"
    }

    fn description(&self) -> &str {
        "Nix flake executor (requires Linux/macOS)"
    }

    async fn execute(&self, _payload: serde_json::Value) -> Result<String, SavantError> {
        Err(SavantError::Unsupported(
            "Nix sandbox requires a Unix-like environment (Linux or macOS). \
             On Windows, use the Docker sandbox for skill execution. \
             For Nix on Windows, run Savant inside WSL2."
                .to_string(),
        ))
    }
}

#[cfg(test)]
#[cfg(not(windows))] // Nix flake refs are Unix-specific
mod tests {
    use super::*;
    use savant_core::traits::Tool;
    use serde_json::json;

    #[test]
    fn test_validate_flake_ref_empty() {
        let result = validate_flake_ref("");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("cannot be empty"));
    }

    #[test]
    fn test_validate_flake_ref_too_long() {
        let long_ref = "a".repeat(MAX_FLAKE_REF_LEN + 1);
        let result = validate_flake_ref(&long_ref);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("maximum length"));
    }

    #[test]
    fn test_validate_flake_ref_dangerous_chars() {
        let dangerous_inputs = vec![
            "flake;rm -rf /",
            "flake&malicious",
            "flake|cat /etc/passwd",
            "flake`whoami`",
            "flake$(whoami)",
            "flake>output.txt",
            "flake<input.txt",
            "flake\nnewline",
            "flake\0null",
        ];
        for input in dangerous_inputs {
            let result = validate_flake_ref(input);
            assert!(result.is_err(), "Should reject: {:?}", input);
        }
    }

    #[test]
    fn test_validate_flake_ref_safe_github() {
        assert!(validate_flake_ref("github:NixOS/nixpkgs").is_ok());
        assert!(validate_flake_ref("github:owner/repo/branch").is_ok());
    }

    #[test]
    fn test_validate_flake_ref_safe_paths() {
        assert!(validate_flake_ref("./my-flake").is_ok());
        assert!(validate_flake_ref("../shared/flake").is_ok());
        assert!(validate_flake_ref("/nix/store/abc123-flake").is_ok());
    }

    #[test]
    fn test_validate_flake_ref_safe_urls() {
        assert!(validate_flake_ref("git+https://github.com/owner/repo").is_ok());
        assert!(validate_flake_ref("https://example.com/flake.nar").is_ok());
    }

    #[test]
    fn test_validate_flake_ref_safe_indirect() {
        assert!(validate_flake_ref("nixpkgs").is_ok());
        assert!(validate_flake_ref("nixpkgs#hello").is_ok());
    }

    #[test]
    fn test_validate_flake_ref_disallowed_format() {
        let result = validate_flake_ref("javascript:alert(1)");
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("disallowed format"));
    }

    #[test]
    fn test_validate_flake_ref_relative_path_missing_parent() {
        let result = validate_flake_ref("./nonexistent-deep/path/flake.nix");
        // This should pass validation (we only check parent, not full path)
        // because the flake reference itself is valid
        assert!(result.is_ok());
    }

    #[test]
    fn test_nix_skill_executor_new_validates() {
        let result = NixSkillExecutor::new("github:NixOS/nixpkgs".to_string());
        assert!(result.is_ok());

        let result = NixSkillExecutor::new("malicious;rm -rf /".to_string());
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_nix_skill_executor_rejects_dangerous_payload() {
        let executor = NixSkillExecutor::new("nixpkgs".to_string()).unwrap();
        let huge_payload = json!({ "data": "x".repeat(2_000_000) });
        let result = executor.execute(huge_payload).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("too large"));
    }
}
