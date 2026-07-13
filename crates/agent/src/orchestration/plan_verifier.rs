//! Skill Verification — validates generated skills before deployment.
//!
//! Implements Component 6 of FID-20260525-LLM-INTERACTION-QUALITY:
//! Verifies generated skill code for syntax, structure, and size before
//! returning from SovereignSynthesizer.

use std::path::Path;

/// Maximum file size for a single skill file (100KB default).
const MAX_FILE_SIZE_BYTES: u64 = 100_000;
/// Files required in every skill crate.
const REQUIRED_FILES: &[&str] = &["Cargo.toml", "src/lib.rs"];

/// Result of a skill verification pass.
#[derive(Debug)]
pub enum VerificationResult {
    /// All checks passed.
    Pass,
    /// One or more issues found.
    Fail(Vec<VerificationIssue>),
}

/// A single verification issue.
#[derive(Debug)]
pub enum VerificationIssue {
    /// File exceeds maximum size.
    FileTooLarge(String, u64),
    /// Required file is missing.
    MissingRequiredFile(String),
    /// Syntax error from cargo check.
    SyntaxError(String, String),
}

/// Verifies generated skill code before deployment.
pub struct SkillVerifier {
    max_file_size: u64,
    run_syntax_check: bool,
}

impl Default for SkillVerifier {
    fn default() -> Self {
        Self::new()
    }
}

impl SkillVerifier {
    /// Create a new verifier with default settings.
    pub fn new() -> Self {
        Self {
            max_file_size: MAX_FILE_SIZE_BYTES,
            run_syntax_check: true,
        }
    }

    /// Create a verifier with syntax check disabled (for tests or constrained environments).
    pub fn without_syntax_check() -> Self {
        Self {
            max_file_size: MAX_FILE_SIZE_BYTES,
            run_syntax_check: false,
        }
    }

    /// Verify a skill directory. Returns Pass or Fail with issues.
    pub async fn verify(&self, skill_dir: &Path) -> VerificationResult {
        let mut issues = Vec::new();

        // Check required files exist
        for required in REQUIRED_FILES {
            if !skill_dir.join(required).exists() {
                issues.push(VerificationIssue::MissingRequiredFile(required.to_string()));
            }
        }

        // Check file sizes
        if let Ok(entries) = std::fs::read_dir(skill_dir) {
            for entry in entries.flatten() {
                self.check_file_size(&entry.path(), &mut issues);
            }
        }

        // Recursively check subdirectories
        if let Ok(entries) = std::fs::read_dir(skill_dir.join("src")) {
            for entry in entries.flatten() {
                self.check_file_size(&entry.path(), &mut issues);
            }
        }

        // Run cargo check if enabled
        if self.run_syntax_check {
            self.run_cargo_check(skill_dir, &mut issues).await;
        }

        if issues.is_empty() {
            VerificationResult::Pass
        } else {
            VerificationResult::Fail(issues)
        }
    }

    fn check_file_size(&self, path: &Path, issues: &mut Vec<VerificationIssue>) {
        if path.is_file() {
            if let Ok(metadata) = std::fs::metadata(path) {
                if metadata.len() > self.max_file_size {
                    issues.push(VerificationIssue::FileTooLarge(
                        path.display().to_string(),
                        metadata.len(),
                    ));
                }
            }
        }
    }

    async fn run_cargo_check(&self, skill_dir: &Path, issues: &mut Vec<VerificationIssue>) {
        let manifest = skill_dir.join("Cargo.toml");
        if !manifest.exists() {
            return;
        }

        match tokio::process::Command::new("cargo")
            .arg("check")
            .arg("--manifest-path")
            .arg(&manifest)
            .arg("--quiet")
            .output()
            .await
        {
            Ok(output) if !output.status.success() => {
                let stderr = String::from_utf8_lossy(&output.stderr);
                issues.push(VerificationIssue::SyntaxError(
                    manifest.display().to_string(),
                    stderr.chars().take(500).collect(),
                ));
            }
            Err(e) => {
                issues.push(VerificationIssue::SyntaxError(
                    manifest.display().to_string(),
                    format!("Failed to run cargo check: {}", e),
                ));
            }
            _ => {}
        }
    }
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_missing_required_files() {
        let dir = tempdir().expect("dir");
        let verifier = SkillVerifier::without_syntax_check();

        match verifier.verify(dir.path()).await {
            VerificationResult::Fail(issues) => {
                assert!(issues
                    .iter()
                    .any(|i| matches!(i, VerificationIssue::MissingRequiredFile(_))));
            }
            _ => panic!("Expected Fail for empty skill dir"),
        }
    }

    #[tokio::test]
    async fn test_valid_skill_passes() {
        let dir = tempdir().expect("dir");
        fs::create_dir_all(dir.path().join("src")).expect("src dir");
        fs::write(
            dir.path().join("Cargo.toml"),
            "[package]\nname = \"test\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        )
        .expect("Cargo.toml");
        fs::write(dir.path().join("src/lib.rs"), "pub fn test() {}").expect("lib.rs");

        let verifier = SkillVerifier::without_syntax_check();
        match verifier.verify(dir.path()).await {
            VerificationResult::Pass => {}
            _ => panic!("Expected Pass for valid skill"),
        }
    }

    #[tokio::test]
    async fn test_oversized_file_detected() {
        let dir = tempdir().expect("dir");
        fs::create_dir_all(dir.path().join("src")).expect("src dir");
        fs::write(
            dir.path().join("Cargo.toml"),
            "[package]\nname = \"test\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
        )
        .expect("Cargo.toml");
        // Write a large file (> 100KB)
        fs::write(dir.path().join("src/lib.rs"), "x".repeat(200_000)).expect("lib.rs");

        let verifier = SkillVerifier::without_syntax_check();
        match verifier.verify(dir.path()).await {
            VerificationResult::Fail(issues) => {
                assert!(issues
                    .iter()
                    .any(|i| matches!(i, VerificationIssue::FileTooLarge(_, _))));
            }
            _ => panic!("Expected Fail for oversized file"),
        }
    }
}
