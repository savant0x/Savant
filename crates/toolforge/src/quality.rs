use regex::Regex;
use serde::Serialize;
use std::collections::HashSet;
use std::sync::LazyLock;

#[derive(Debug, Clone, Serialize)]
pub struct QualityFailure {
    pub code: String,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct QualityResult {
    pub passed: bool,
    pub failures: Vec<QualityFailure>,
}

#[allow(clippy::disallowed_methods)] // .expect() on hardcoded regex in LazyLock — one-time init, cannot fail
static STUB_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)(todo!\(\)|unimplemented!\(\)|//\s*todo|FIXME|placeholder|\[STUB\]|__STUB__|TBD)",
    )
    .expect("Hardcoded stub detection regex is valid")
});

#[allow(clippy::disallowed_methods)] // .expect() on hardcoded regex in LazyLock — one-time init, cannot fail
static NAMING_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[a-z][a-z0-9]*(-[a-z0-9]+)*$").expect("Valid naming regex"));

#[allow(clippy::disallowed_methods)] // .expect() on hardcoded regex in LazyLock — one-time init, cannot fail
static ACTIONABLE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?m)^\s*(\d+\.\s|[-*]\s|```)").expect("Valid actionable regex"));

#[allow(clippy::disallowed_methods)] // .expect() on hardcoded regex in LazyLock — one-time init, cannot fail
static VERSION_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\d+\.\d+\.\d+").expect("Valid semver regex"));

pub struct QualityGate;

impl QualityGate {
    pub fn validate(
        name: &str,
        description: &str,
        version: &str,
        body: &str,
        existing_tools: &std::collections::HashSet<String>,
    ) -> QualityResult {
        let mut failures = Vec::new();

        if name.trim().is_empty() {
            failures.push(QualityFailure {
                code: String::from("E_NAME_REQUIRED"),
                detail: String::from("Skill name is empty"),
            });
        } else if name.len() > 64 || !NAMING_RE.is_match(name) {
            failures.push(QualityFailure {
                code: String::from("E_NAME_FORMAT"),
                detail: format!("'{name}' must be kebab-case, start with a letter, max 64 chars"),
            });
        }

        let name_lower = name.to_lowercase();
        if existing_tools.contains(&name_lower) {
            failures.push(QualityFailure {
                code: String::from("E_DUPLICATE_NAME"),
                detail: format!("A tool named '{name}' already exists"),
            });
        }

        for existing in existing_tools.iter() {
            let overlap = keyword_overlap(name, description, existing);
            if overlap > 0.8 {
                failures.push(QualityFailure {
                    code: String::from("E_DUPLICATE_SIMILAR"),
                    detail: format!(
                        "Tool is {:.0}% similar to existing tool '{existing}'. Consider patching that tool instead.",
                        overlap * 100.0
                    ),
                });
                break;
            }
        }

        if description.trim().len() < 10 {
            failures.push(QualityFailure {
                code: String::from("E_DESC_REQUIRED"),
                detail: format!(
                    "Description is {} chars, minimum is 10",
                    description.trim().len()
                ),
            });
        }

        if version.trim().is_empty() || !VERSION_RE.is_match(version) {
            failures.push(QualityFailure {
                code: String::from("E_VERSION_REQUIRED"),
                detail: format!("Version '{version}' must match X.Y.Z (semver)"),
            });
        }

        if let Some(m) = STUB_RE.find(body) {
            failures.push(QualityFailure {
                code: String::from("E_STUBS_FOUND"),
                detail: format!("Body contains a stub: '{}'", m.as_str()),
            });
        }

        let clean_body = strip_frontmatter(body);
        if clean_body.len() < 200 {
            failures.push(QualityFailure {
                code: String::from("E_BODY_TOO_SHORT"),
                detail: format!(
                    "Body is {} chars (excluding frontmatter), minimum is 200",
                    clean_body.len()
                ),
            });
        }

        if !ACTIONABLE_RE.is_match(&clean_body) {
            failures.push(QualityFailure {
                code: String::from("E_NO_ACTIONABLE"),
                detail: String::from("No numbered list, bullet list, or code block found in body"),
            });
        }

        QualityResult {
            passed: failures.is_empty(),
            failures,
        }
    }
}

fn strip_frontmatter(body: &str) -> String {
    let trimmed = body.trim();
    if let Some(stripped) = trimmed.strip_prefix("---") {
        if let Some(end) = stripped.find("---") {
            return stripped[end + 3..].trim().to_string();
        }
    }
    trimmed.to_string()
}

fn keyword_overlap(name: &str, description: &str, existing: &str) -> f64 {
    let combined = format!("{name} {description}").to_lowercase();
    let words_a: HashSet<String> = combined
        .split_whitespace()
        .filter(|w| w.len() > 2)
        .map(|w| w.to_string())
        .collect();
    let words_b: HashSet<String> = existing
        .split('-')
        .chain(existing.split(' '))
        .filter(|w| w.len() > 2)
        .map(|w| w.to_lowercase().to_string())
        .collect();

    if words_a.is_empty() || words_b.is_empty() {
        return 0.0;
    }

    let intersection = words_a
        .iter()
        .filter(|w| words_b.contains(w.as_str()))
        .count();
    let union = words_a.len().min(words_b.len());
    intersection as f64 / union as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_name_rejected() {
        let result = QualityGate::validate("", "a good description", "0.1.0", "# Usage\n\n1. First step\n2. Second step\n\nSome more text to reach the minimum character count. Let me add some more words here to ensure we pass the 200 character minimum threshold for the quality gate validation check.", &std::collections::HashSet::new());
        assert!(!result.passed);
        assert!(result.failures.iter().any(|f| f.code == "E_NAME_REQUIRED"));
    }

    #[test]
    fn test_stub_detection() {
        let body = "# Test\n\n1. Do something\n\n// TODO: implement this\n\nMore text needed to reach minimum length for quality gate validation of body content which must be at least two hundred characters long so we add some padding here to make sure the test passes correctly without hitting the body length check.";
        let result = QualityGate::validate(
            "my-tool",
            "a good description",
            "0.1.0",
            body,
            &std::collections::HashSet::new(),
        );
        assert!(!result.passed);
        assert!(result.failures.iter().any(|f| f.code == "E_STUBS_FOUND"));
    }

    #[test]
    fn test_short_body_rejected() {
        let result = QualityGate::validate(
            "my-tool",
            "a good description",
            "0.1.0",
            "short",
            &std::collections::HashSet::new(),
        );
        assert!(!result.passed);
        assert!(result.failures.iter().any(|f| f.code == "E_BODY_TOO_SHORT"));
    }

    #[test]
    fn test_valid_tool_passes() {
        let body = "# Web Scraper\n\n## Usage\n\nExtracts structured data from web pages.\n\n1. Navigate to the target URL using the browser tool\n2. Call `browser.get_content()` to extract the page HTML\n3. Parse the content using the steps below\n4. Return the structured data to the user\n\n## Error Handling\n\n- If navigation fails, retry up to 3 times with exponential backoff\n- If the page is empty, report the error to the user\n\nThis skill provides a standardized way to scrape web content using the collective browser tool. It handles common edge cases and provides structured output.";
        let result = QualityGate::validate(
            "web-scraper",
            "Scrapes web pages and extracts structured data",
            "0.1.0",
            body,
            &std::collections::HashSet::new(),
        );
        assert!(
            result.passed,
            "Expected PASS but got failures: {:?}",
            result.failures
        );
    }

    #[test]
    fn test_duplicate_name_rejected() {
        let mut existing = std::collections::HashSet::new();
        existing.insert(String::from("my-tool"));
        let body = "# Test\n\n1. First step\n2. Second step\n\nExtra text to reach the minimum character count for quality gate validation. Adding more words to ensure we pass the body length threshold of two hundred characters with room to spare for the test case.";
        let result =
            QualityGate::validate("my-tool", "a good description", "0.1.0", body, &existing);
        assert!(!result.passed);
        assert!(result.failures.iter().any(|f| f.code == "E_DUPLICATE_NAME"));
    }
}
