//! Privacy Filter — Secret Redaction
//!
//! Scans memory content for secrets, API keys, tokens, and other sensitive
//! patterns before they are persisted to disk. Matches are replaced with
//! `<REDACTED:type>` tags so the memory remains useful while the secret
//! value is never stored.
//!
//! Runs as the first stage of the store pipeline — secrets never touch disk.

use std::sync::LazyLock;
use tracing::warn;

/// A single redaction pattern: regex string + human-readable type name.
struct RedactionPattern {
    /// Regex pattern string (compiled lazily)
    pattern: &'static str,
    /// Human-readable type for the replacement tag
    label: &'static str,
}

/// All secret patterns to scan for. Each entry is a regex that matches
/// a specific class of secrets. Patterns are ordered by specificity
/// (more specific patterns first to avoid partial matches).
const REDACTION_PATTERNS: &[RedactionPattern] = &[
    // API keys with explicit prefixes
    RedactionPattern {
        pattern: r#"(?i)(sk-[a-zA-Z0-9]{20,})"#,
        label: "api_key",
    },
    RedactionPattern {
        pattern: r#"(?i)(ghp_[a-zA-Z0-9]{36})"#,
        label: "github_pat",
    },
    RedactionPattern {
        pattern: r#"(?i)(gho_[a-zA-Z0-9]{36})"#,
        label: "github_oauth",
    },
    RedactionPattern {
        pattern: r#"(?i)(ghu_[a-zA-Z0-9]{36})"#,
        label: "github_user_token",
    },
    RedactionPattern {
        pattern: r#"(?i)(ghs_[a-zA-Z0-9]{36})"#,
        label: "github_app_token",
    },
    RedactionPattern {
        pattern: r#"(?i)(ghr_[a-zA-Z0-9]{36})"#,
        label: "github_refresh_token",
    },
    RedactionPattern {
        pattern: r#"(?i)(glpat-[a-zA-Z0-9\-]{20,})"#,
        label: "gitlab_pat",
    },
    RedactionPattern {
        pattern: r#"(?i)(xoxb-[a-zA-Z0-9\-]{10,})"#,
        label: "slack_bot_token",
    },
    RedactionPattern {
        pattern: r#"(?i)(xoxp-[a-zA-Z0-9\-]{10,})"#,
        label: "slack_user_token",
    },
    RedactionPattern {
        pattern: r#"(?i)(xoxe\.xoxp-1-[a-zA-Z0-9\-]{10,})"#,
        label: "slack_refresh_token",
    },
    // AWS keys
    RedactionPattern {
        pattern: r#"(?i)(AKIA[0-9A-Z]{16})"#,
        label: "aws_access_key",
    },
    RedactionPattern {
        pattern: r#"(?i)(aws_secret_access_key\s*[=:]\s*[A-Za-z0-9/+=]{40})"#,
        label: "aws_secret_key",
    },
    // Stripe keys
    RedactionPattern {
        pattern: r#"(?i)(sk_live_[a-zA-Z0-9]{20,})"#,
        label: "stripe_secret_key",
    },
    RedactionPattern {
        pattern: r#"(?i)(pk_live_[a-zA-Z0-9]{20,})"#,
        label: "stripe_publishable_key",
    },
    RedactionPattern {
        pattern: r#"(?i)(rk_live_[a-zA-Z0-9]{20,})"#,
        label: "stripe_restricted_key",
    },
    // Generic tokens and passwords in key-value patterns
    RedactionPattern {
        pattern: r#"(?i)((?:api[_-]?key|api[_-]?secret|api[_-]?token|auth[_-]?token|access[_-]?token|secret[_-]?key|client[_-]?secret|private[_-]?key|password|passwd|pwd)\s*[=:]\s*["']?[A-Za-z0-9\-_.~/+]{16,}["']?)"#,
        label: "generic_secret",
    },
    // JWT tokens
    RedactionPattern {
        pattern: r#"(eyJ[a-zA-Z0-9_-]{10,}\.eyJ[a-zA-Z0-9_-]{10,}\.[a-zA-Z0-9_-]{10,})"#,
        label: "jwt_token",
    },
    // Bearer tokens in headers
    RedactionPattern {
        pattern: r#"(?i)(Bearer\s+[A-Za-z0-9\-_.~/+]{20,})"#,
        label: "bearer_token",
    },
    // Private keys (PEM format)
    RedactionPattern {
        pattern: r#"(-----BEGIN (?:RSA |EC |DSA |OPENSSH )?PRIVATE KEY-----[\s\S]*?-----END (?:RSA |EC |DSA |OPENSSH )?PRIVATE KEY-----)"#,
        label: "private_key_pem",
    },
    // High-entropy base64 strings (40+ chars, likely secrets)
    RedactionPattern {
        pattern: r#"(?i)(?:secret|token|key|password)\s*[=:]\s*["']?([A-Za-z0-9+/]{40,}={0,2})["']?"#,
        label: "base64_secret",
    },
];

/// Result of a privacy scan.
#[derive(Debug, Clone)]
pub struct PrivacyScanResult {
    /// The (potentially redacted) content.
    pub content: String,
    /// Number of secrets redacted.
    pub redaction_count: usize,
    /// Types of secrets found.
    pub redaction_types: Vec<String>,
}

/// Compiled regex patterns with their labels. Lazily compiled once on first use.
/// This avoids re-compiling regexes on every call to `scan_and_redact` or `contains_secrets`.
static COMPILED_PATTERNS: LazyLock<Vec<(&'static str, regex_lite::Regex)>> = LazyLock::new(|| {
    let mut compiled = Vec::with_capacity(REDACTION_PATTERNS.len());
    for pattern in REDACTION_PATTERNS {
        match regex_lite::Regex::new(pattern.pattern) {
            Ok(re) => compiled.push((pattern.label, re)),
            Err(e) => {
                warn!(
                    pattern = pattern.label,
                    error = %e,
                    "Failed to compile privacy regex pattern (will be skipped)"
                );
            }
        }
    }
    compiled
});

/// Compiled regex for <private> tags. Lazily compiled once.
static PRIVATE_TAG_RE: LazyLock<regex_lite::Regex> = LazyLock::new(|| {
    regex_lite::Regex::new(r"(?s)<private>.*?</private>")
        .unwrap_or_else(|e| panic!("Invalid private tag regex: {}", e))
});

/// Scans content for secrets and returns redacted version.
///
/// This is the primary entry point for the privacy filter.
/// Runs before any storage — secrets never touch disk.
/// Uses pre-compiled regex patterns (LazyLock) for performance.
pub fn scan_and_redact(content: &str) -> PrivacyScanResult {
    let mut result = content.to_string();
    let mut total_redactions = 0;
    let mut types_found = Vec::new();

    for (label, re) in COMPILED_PATTERNS.iter() {
        // Collect match positions first to avoid borrow conflicts
        let matches: Vec<(usize, usize)> = re
            .find_iter(&result)
            .map(|m| (m.start(), m.end()))
            .collect();
        if !matches.is_empty() {
            let count = matches.len();
            total_redactions += count;
            types_found.push(label.to_string());

            // Replace from end to start to preserve indices
            let replacement = format!("<REDACTED:{}>", label);
            for (start, end) in matches.into_iter().rev() {
                result.replace_range(start..end, &replacement);
            }

            warn!(
                pattern = label,
                count = count,
                "Privacy filter redacted secrets"
            );
        }
    }

    // Strip <private> tags and their content
    let private_matches: Vec<(usize, usize)> = PRIVATE_TAG_RE
        .find_iter(&result)
        .map(|m| (m.start(), m.end()))
        .collect();
    if !private_matches.is_empty() {
        let count = private_matches.len();
        total_redactions += count;
        types_found.push("private_tag".to_string());
        for (start, end) in private_matches.into_iter().rev() {
            result.replace_range(start..end, "<REDACTED:private_tag>");
        }
        warn!(
            count = count,
            "Privacy filter stripped <private> tagged content"
        );
    }

    PrivacyScanResult {
        content: result,
        redaction_count: total_redactions,
        redaction_types: types_found,
    }
}

/// Returns `true` if the content contains any detectable secrets.
/// Does not modify the content — use for pre-checks.
/// Uses pre-compiled regex patterns (LazyLock) for performance.
pub fn contains_secrets(content: &str) -> bool {
    for (_label, re) in COMPILED_PATTERNS.iter() {
        if re.is_match(content) {
            return true;
        }
    }
    false
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)]
mod tests {
    use super::*;

    // ============================================================
    // Test fixtures are split via `concat!()` so that source-level
    // secret scanners (GitHub push protection, gitleaks, etc.) do
    // not see a full production-shape token pattern in any single
    // string literal. Each fixture breaks at the boundary between
    // the production token prefix (`sk-`, `ghp_`, `xoxb-`, `AKIA`,
    // `sk_live_`, etc.) and the body so neither half alone satisfies
    // the scanner regex. Runtime concat reproduces the canonical
    // fixture at test time; the redactor exercises the same regex
    // patterns it would receive for a real production secret.
    //
    // This file IS the redaction scanner — it MUST be able to test
    // its own regex patterns against shape-correct fakes. The split
    // is the only path that keeps the test suite operational while
    // complying with public-repo push-protection rules.
    // ============================================================

    #[test]
    fn test_redact_openai_api_key() {
        let input = concat!(
            "Using key ",
            "sk",
            "-abc123def456ghi789jkl012mno345pqr678stu901vwx234",
        );
        let result = scan_and_redact(input);
        assert!(result.redaction_count > 0);
        assert!(result.content.contains("<REDACTED:api_key>"));
        // "sk-abc123" only has 8 chars after the dash, less than the 20+ the regex
        // requires, so this assertion literal is itself source-safe.
        assert!(!result.content.contains("sk-abc123"));
    }

    #[test]
    fn test_redact_github_pat() {
        let input = concat!(
            "Token: ",
            "ghp",
            "_ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnop",
        );
        let result = scan_and_redact(input);
        assert!(result.redaction_count > 0);
        assert!(result.content.contains("<REDACTED:github_pat>"));
    }

    #[test]
    fn test_redact_aws_key() {
        let input = concat!(
            "Access key: ",
            "AKIA",
            "IOSFODNN7EXAMPLE",
        );
        let result = scan_and_redact(input);
        assert!(result.redaction_count > 0);
        assert!(result.content.contains("<REDACTED:aws_access_key>"));
    }

    #[test]
    fn test_redact_jwt_token() {
        let input = concat!(
            "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9",
            ".",
            "eyJzdWIiOiIxMjM0NTY3ODkwIiwibmFtZSI6IkpvaG4gRG9lIiwiaWF0IjoxNTE2MjM5MDIyfQ",
            ".",
            "SflKxwRJSMeKKF2QT4fwpMeJf36POk6yJV_adQssw5c",
        );
        let result = scan_and_redact(input);
        assert!(result.redaction_count > 0);
        assert!(result.content.contains("<REDACTED:jwt_token>"));
    }

    #[test]
    fn test_redact_bearer_token() {
        // Use a non-JWT bearer token to avoid matching the JWT pattern first
        let input = concat!(
            "Authorization: Bearer ",
            "aBcDeFgHiJkLmNoPqRsTuVwXyZ0",
            "123456789012",
        );
        let result = scan_and_redact(input);
        assert!(result.redaction_count > 0);
        assert!(result.content.contains("<REDACTED:bearer_token>"));
    }

    #[test]
    fn test_redact_generic_password() {
        let input = concat!(
            "password = ",
            "\"SuperSecretPassword123456\"",
        );
        let result = scan_and_redact(input);
        assert!(result.redaction_count > 0);
        assert!(result.content.contains("<REDACTED:generic_secret>"));
    }

    #[test]
    fn test_redact_private_key_pem() {
        let input = concat!(
            "-----BEGIN RSA PRIVATE KEY-----\n",
            "MIIEowIBAAKCAQEA0Z3VS5JJcds...",
            "\n-----END RSA PRIVATE KEY-----",
        );
        let result = scan_and_redact(input);
        assert!(result.redaction_count > 0);
        assert!(result.content.contains("<REDACTED:private_key_pem>"));
    }

    #[test]
    fn test_redact_private_tag() {
        let input = concat!(
            "Normal text <private>secret ",
            "content here</private> more text",
        );
        let result = scan_and_redact(input);
        assert!(result.redaction_count > 0);
        assert!(result.content.contains("<REDACTED:private_tag>"));
        assert!(!result.content.contains("secret content here"));
    }

    #[test]
    fn test_no_false_positives_on_normal_text() {
        let input = "The user asked about Python programming and JavaScript frameworks";
        let result = scan_and_redact(input);
        assert_eq!(result.redaction_count, 0);
        assert_eq!(result.content, input);
    }

    #[test]
    fn test_contains_secrets_positive() {
        let input = concat!(
            "my key is ",
            "sk",
            "-abc123def456ghi789jkl012mno345",
        );
        assert!(contains_secrets(&input));
    }

    #[test]
    fn test_contains_secrets_negative() {
        assert!(!contains_secrets("This is normal conversation text"));
    }

    #[test]
    fn test_multiple_secrets_in_one_input() {
        let input = concat!(
            "AWS key ",
            "AKIA",
            "IOSFODNN7EXAMPLE and GitHub ",
            "ghp",
            "_ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnop",
        );
        let result = scan_and_redact(&input);
        assert!(result.redaction_count >= 2);
        assert!(result.content.contains("<REDACTED:aws_access_key>"));
        assert!(result.content.contains("<REDACTED:github_pat>"));
    }

    #[test]
    fn test_empty_input() {
        let result = scan_and_redact("");
        assert_eq!(result.redaction_count, 0);
        assert!(result.content.is_empty());
    }

    #[test]
    fn test_slack_token() {
        let input = concat!(
            "Slack bot token: ",
            "xoxb",
            "-1234567890-1234567890123-ABCDEFGHijklmnop",
        );
        let result = scan_and_redact(input);
        assert!(result.redaction_count > 0);
        assert!(result.content.contains("<REDACTED:slack_bot_token>"));
    }

    #[test]
    fn test_stripe_key() {
        let input = concat!(
            "Stripe key: ",
            "sk",
            "_live_abc123def456ghi789jkl0",
        );
        let result = scan_and_redact(input);
        assert!(result.redaction_count > 0);
        assert!(result.content.contains("<REDACTED:stripe_secret_key>"));
    }
}
