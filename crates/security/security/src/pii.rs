//! PII Detection — deterministic content-sensitive scanning for privacy-aware routing.
//!
//! Detects emails, phone numbers, SSNs, credit cards, IP addresses, government IDs,
//! and generic secret patterns. Computes a sensitivity score used by the PrivacyRouter
//! to decide whether content should stay on-device.

use regex_lite::Regex;
use std::sync::LazyLock;

/// Types of personally identifiable information.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PiiType {
    Email,
    Phone,
    Ssn,
    CreditCard,
    IpAddress,
    GovernmentId,
    Name,
    Address,
}

/// A detected PII match with location and confidence.
#[derive(Debug, Clone)]
pub struct PiiMatch {
    pub pii_type: PiiType,
    pub start: usize,
    pub end: usize,
    pub confidence: f32,
}

/// A compiled PII detection pattern.
struct PiiPattern {
    pii_type: PiiType,
    regex: Regex,
    confidence: f32,
    weight: f32,
}

/// Result of a PII scan.
#[derive(Debug, Clone)]
pub struct PiiScanResult {
    pub matches: Vec<PiiMatch>,
    pub sensitivity_score: f32,
    pub pii_types_found: Vec<PiiType>,
}

/// Pre-compiled PII detection patterns (one-time compilation via LazyLock).
/// Helper to compile regex with descriptive panic on failure.
/// Panicking is correct here because the regex patterns are hardcoded
/// and their validity is a startup invariant — if a regex is invalid,
/// the process should fail immediately with a clear message.
fn compile_regex(pattern: &str) -> Regex {
    Regex::new(pattern).unwrap_or_else(|e| panic!("Invalid PII regex pattern '{}': {}", pattern, e))
}

/// Pre-compiled PII detection patterns (one-time compilation via LazyLock).
static PII_PATTERNS: LazyLock<Vec<PiiPattern>> = LazyLock::new(|| {
    vec![
        // Email — high confidence
        PiiPattern {
            pii_type: PiiType::Email,
            regex: compile_regex(r"[a-zA-Z0-9._%+\-]+@[a-zA-Z0-9.\-]+\.[a-zA-Z]{2,}"),
            confidence: 0.95,
            weight: 0.5,
        },
        // SSN — requires separator between groups (SEC-04: prevents matching plain 9-digit numbers)
        PiiPattern {
            pii_type: PiiType::Ssn,
            regex: compile_regex(r"\b\d{3}[-\s]\d{2}[-\s]\d{4}\b"),
            confidence: 0.85,
            weight: 1.0,
        },
        // Credit card — 13-19 digit sequences (with optional separators)
        PiiPattern {
            pii_type: PiiType::CreditCard,
            regex: compile_regex(r"\b(?:\d[ -]*?){13,19}\b"),
            confidence: 0.70,
            weight: 1.0,
        },
        // US phone numbers
        PiiPattern {
            pii_type: PiiType::Phone,
            regex: compile_regex(r"\b(?:\+?1[-.\s]?)?\(?\d{3}\)?[-.\s]?\d{3}[-.\s]?\d{4}\b"),
            confidence: 0.80,
            weight: 0.5,
        },
        // IPv4
        PiiPattern {
            pii_type: PiiType::IpAddress,
            regex: compile_regex(
                r"\b(?:(?:25[0-5]|2[0-4]\d|[01]?\d\d?)\.){3}(?:25[0-5]|2[0-4]\d|[01]?\d\d?)\b",
            ),
            confidence: 0.90,
            weight: 0.2,
        },
        // IPv6 (simplified)
        PiiPattern {
            pii_type: PiiType::IpAddress,
            regex: compile_regex(r"\b(?:[0-9a-fA-F]{1,4}:){7}[0-9a-fA-F]{1,4}\b"),
            confidence: 0.90,
            weight: 0.2,
        },
        // US passport
        PiiPattern {
            pii_type: PiiType::GovernmentId,
            regex: compile_regex(r"\b[A-Z]\d{8}\b"),
            confidence: 0.60,
            weight: 0.9,
        },
        // US driver's license (state-specific, generic pattern)
        PiiPattern {
            pii_type: PiiType::GovernmentId,
            regex: compile_regex(r"\b[A-Z]\d{7,14}\b"),
            confidence: 0.40,
            weight: 0.9,
        },
        // Name — heuristic: "my name is <Name>" or "I'm <Name>" or "I am <Name>"
        PiiPattern {
            pii_type: PiiType::Name,
            regex: compile_regex(r"(?i)(?:my name is|I'm|I am)\s+([A-Z][a-z]+(?:\s+[A-Z][a-z]+)?)"),
            confidence: 0.60,
            weight: 0.4,
        },
        // Address — heuristic: US street address pattern
        PiiPattern {
            pii_type: PiiType::Address,
            regex: compile_regex(
                r"\b\d{1,5}\s+[A-Z][a-z]+(?:\s+[A-Z][a-z]+)*\s+(?:St|Street|Ave|Avenue|Blvd|Boulevard|Dr|Drive|Ln|Lane|Rd|Road|Way|Ct|Court)\b",
            ),
            confidence: 0.70,
            weight: 0.6,
        },
    ]
});

/// Deterministic PII detector using regex patterns.
pub struct PiiDetector {
    patterns: &'static Vec<PiiPattern>,
}

impl PiiDetector {
    /// Creates a new detector with all built-in patterns.
    pub fn new() -> Self {
        Self {
            patterns: &*PII_PATTERNS,
        }
    }

    /// Scan text for PII matches.
    pub fn detect(&self, text: &str) -> Vec<PiiMatch> {
        let mut matches = Vec::new();

        for pattern in self.patterns.iter() {
            for mat in pattern.regex.find_iter(text) {
                // For credit cards, validate with Luhn
                if pattern.pii_type == PiiType::CreditCard {
                    let digits: String = text[mat.start()..mat.end()]
                        .chars()
                        .filter(|c: &char| c.is_ascii_digit())
                        .collect();
                    if !luhn_check(&digits) {
                        continue;
                    }
                }

                // For SSN, reject if all digits are zero or the area number is 000
                if pattern.pii_type == PiiType::Ssn {
                    let digits: String = text[mat.start()..mat.end()]
                        .chars()
                        .filter(|c: &char| c.is_ascii_digit())
                        .collect();
                    if digits.len() == 9 {
                        let area = &digits[0..3];
                        let group = &digits[3..5];
                        let serial = &digits[5..9];
                        if area == "000" || group == "00" || serial == "0000" || area == "666" {
                            continue;
                        }
                        // Reject ITIN range (9xx)
                        if area.starts_with('9') {
                            continue;
                        }
                    }
                }

                matches.push(PiiMatch {
                    pii_type: pattern.pii_type,
                    start: mat.start(),
                    end: mat.end(),
                    confidence: pattern.confidence,
                });
            }
        }

        matches
    }

    /// Compute a sensitivity score for the given text.
    /// Returns 0.0 for clean text, up to 1.0 for highly sensitive content.
    pub fn sensitivity_score(&self, text: &str) -> f32 {
        let matches = self.detect(text);
        if matches.is_empty() {
            return 0.0;
        }

        // Group by type, compute per-type weighted score
        let mut type_counts = std::collections::HashMap::<PiiType, (u32, f32)>::new();
        for m in &matches {
            let entry = type_counts.entry(m.pii_type).or_insert((0, 0.0));
            entry.0 += 1;
            // Find the weight for this type (use the max weight if multiple patterns)
            for pattern in self.patterns.iter() {
                if pattern.pii_type == m.pii_type && pattern.weight > entry.1 {
                    entry.1 = pattern.weight;
                }
            }
        }

        let mut max_score: f32 = 0.0;
        for (count, weight) in type_counts.values() {
            let count_factor = match *count {
                1 => 1.0,
                2..=5 => 1.2,
                _ => 1.5,
            };
            let score = weight * count_factor;
            if score > max_score {
                max_score = score;
            }
        }

        max_score.clamp(0.0, 1.0)
    }

    /// Scan text and return a full result with matches and score.
    pub fn scan(&self, text: &str) -> PiiScanResult {
        let matches = self.detect(text);
        let sensitivity_score = self.sensitivity_score(text);

        let mut pii_types_found: Vec<PiiType> = matches.iter().map(|m| m.pii_type).collect();
        pii_types_found.sort_by_key(|t| format!("{:?}", t));
        pii_types_found.dedup();

        PiiScanResult {
            matches,
            sensitivity_score,
            pii_types_found,
        }
    }
}

impl Default for PiiDetector {
    fn default() -> Self {
        Self::new()
    }
}

/// Luhn algorithm check for credit card validation.
fn luhn_check(digits: &str) -> bool {
    if digits.len() < 13 || digits.len() > 19 {
        return false;
    }
    let mut sum = 0;
    let mut alternate = false;
    for ch in digits.chars().rev() {
        if let Some(d) = ch.to_digit(10) {
            let mut n = d;
            if alternate {
                n *= 2;
                if n > 9 {
                    n -= 9;
                }
            }
            sum += n;
            alternate = !alternate;
        } else {
            return false;
        }
    }
    sum % 10 == 0
}

/// Convenience: check if text contains any secrets (fast path, stops at first match).
pub fn contains_pii(text: &str) -> bool {
    let detector = PiiDetector::new();
    !detector.detect(text).is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_email() {
        let detector = PiiDetector::new();
        let matches = detector.detect("Contact alice@example.com for details");
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].pii_type, PiiType::Email);
    }

    #[test]
    fn test_detect_ssn() {
        let detector = PiiDetector::new();
        let matches = detector.detect("SSN: 123-45-6789");
        assert!(matches.iter().any(|m| m.pii_type == PiiType::Ssn));
    }

    #[test]
    fn test_detect_ssn_invalid_area() {
        let detector = PiiDetector::new();
        // 000 area number should be rejected
        let matches = detector.detect("SSN: 000-12-3456");
        assert!(!matches.iter().any(|m| m.pii_type == PiiType::Ssn));
    }

    #[test]
    fn test_detect_ssn_itin_rejected() {
        let detector = PiiDetector::new();
        // 9xx area is ITIN, not SSN
        let matches = detector.detect("SSN: 987-65-4321");
        assert!(!matches.iter().any(|m| m.pii_type == PiiType::Ssn));
    }

    #[test]
    fn test_detect_credit_card_valid_luhn() {
        let detector = PiiDetector::new();
        // 4111111111111111 is a valid test Visa number (passes Luhn)
        let matches = detector.detect("Card: 4111111111111111");
        assert!(matches.iter().any(|m| m.pii_type == PiiType::CreditCard));
    }

    #[test]
    fn test_detect_credit_card_invalid_luhn() {
        let detector = PiiDetector::new();
        // 1234567890123456 does NOT pass Luhn
        let matches = detector.detect("Card: 1234567890123456");
        assert!(!matches.iter().any(|m| m.pii_type == PiiType::CreditCard));
    }

    #[test]
    fn test_detect_phone() {
        let detector = PiiDetector::new();
        let matches = detector.detect("Call (555) 123-4567");
        assert!(matches.iter().any(|m| m.pii_type == PiiType::Phone));
    }

    #[test]
    fn test_detect_ipv4() {
        let detector = PiiDetector::new();
        let matches = detector.detect("Server at 192.168.1.1");
        assert!(matches.iter().any(|m| m.pii_type == PiiType::IpAddress));
    }

    #[test]
    fn test_clean_text_scores_zero() {
        let detector = PiiDetector::new();
        let score = detector.sensitivity_score("Hello, this is a normal message.");
        assert_eq!(score, 0.0);
    }

    #[test]
    fn test_high_sensitivity_text() {
        let detector = PiiDetector::new();
        let text = "SSN: 123-45-6789, Card: 4111111111111111";
        let score = detector.sensitivity_score(text);
        assert!(score >= 0.7, "Expected high sensitivity, got {}", score);
    }

    #[test]
    fn test_scan_returns_types() {
        let detector = PiiDetector::new();
        let result = detector.scan("Email: test@test.com, IP: 10.0.0.1");
        assert!(result.pii_types_found.contains(&PiiType::Email));
        assert!(result.pii_types_found.contains(&PiiType::IpAddress));
    }

    #[test]
    fn test_contains_pii_convenience() {
        assert!(contains_pii("my email is foo@bar.com"));
        assert!(!contains_pii("no secrets here"));
    }

    #[test]
    fn test_no_pii_empty_text() {
        let detector = PiiDetector::new();
        let result = detector.scan("");
        assert!(result.matches.is_empty());
        assert_eq!(result.sensitivity_score, 0.0);
    }
}
