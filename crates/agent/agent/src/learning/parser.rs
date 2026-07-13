use chrono::Utc;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use tracing::{debug, info};

use savant_core::error::SavantError;
use savant_core::learning::{EmergentLearning, LearningCategory};

/// LEARNINGS.md Parser
///
/// Converts free-form LEARNINGS.md entries into structured LEARNINGS.jsonl format.
/// The agent writes freely to LEARNINGS.md — no formatting restrictions.
/// This parser retrofits structure from whatever the agent produces.
///
/// Entry detection strategy:
/// - Primary: split on `### Learning (` headers (standard format)
/// - Each entry body is everything until the next `### ` header or EOF
/// - Timestamp: extracted from `YYYY-MM-DD HH:MM:SS.NNNNNNNNN UTC)` pattern
/// - Category: extracted from `[CATEGORY]` tag after the timestamp
/// - Content: everything after the header line, trimmed
///
/// Deduplication: by content fingerprint (first 200 chars normalized),
/// NOT by timestamp — timestamps may be regenerated if agent rewrites entries.
pub struct LearningsParser {
    workspace_path: PathBuf,
}

impl LearningsParser {
    pub fn new(workspace_path: PathBuf) -> Self {
        Self { workspace_path }
    }

    /// Parses LEARNINGS.md and converts new entries to JSONL format.
    /// Returns the number of new entries parsed.
    pub fn parse_and_convert(&self, agent_id: &str) -> Result<usize, SavantError> {
        let md_path = self.workspace_path.join("LEARNINGS.md");
        let jsonl_path = self.workspace_path.join("LEARNINGS.jsonl");

        if !md_path.exists() {
            debug!("No LEARNINGS.md found at {:?}", md_path);
            return Ok(0);
        }

        let md_content = fs::read_to_string(&md_path).map_err(SavantError::IoError)?;

        if md_content.trim().is_empty() {
            return Ok(0);
        }

        // Parse entries from LEARNINGS.md
        let entries = self.parse_entries(&md_content, agent_id);

        if entries.is_empty() {
            return Ok(0);
        }

        // Get existing content fingerprints to avoid duplicates
        let existing_fingerprints = self.get_existing_fingerprints(&jsonl_path);

        // Filter new entries by content fingerprint
        let new_entries: Vec<EmergentLearning> = entries
            .into_iter()
            .filter(|entry| {
                let fingerprint = content_fingerprint(&entry.content);
                !existing_fingerprints.contains(&fingerprint)
            })
            .collect();

        if new_entries.is_empty() {
            return Ok(0);
        }

        // Append new entries to JSONL
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&jsonl_path)
            .map_err(SavantError::IoError)?;

        for entry in &new_entries {
            let json = serde_json::to_string(entry).map_err(SavantError::SerializationError)?;
            use std::io::Write;
            writeln!(file, "{}", json).map_err(SavantError::IoError)?;
        }

        info!(
            "[{}] Parsed {} new learning entries from LEARNINGS.md → LEARNINGS.jsonl",
            agent_id,
            new_entries.len()
        );

        Ok(new_entries.len())
    }

    /// Parses LEARNINGS.md entries into EmergentLearning structs.
    /// Handles freeform content — no format restrictions on the agent.
    fn parse_entries(&self, content: &str, agent_id: &str) -> Vec<EmergentLearning> {
        let mut entries = Vec::new();

        // Primary strategy: split on "### Learning (" headers
        let parts: Vec<&str> = content.split("### Learning (").collect();

        for part in parts.iter().skip(1) {
            // Each part starts with: TIMESTAMP) [CATEGORY]\nContent...
            // Split off the header line from the body
            let (header_line, body) = match part.find('\n') {
                Some(idx) => (&part[..idx], &part[idx + 1..]),
                None => continue, // No content after header
            };

            // Extract timestamp from header: "... UTC)" pattern
            let timestamp = self.extract_timestamp(header_line);

            // Extract [CATEGORY] tag from header if present
            let category = self.extract_category(header_line);

            // Content is the body, trimmed
            let content_text = body.trim().to_string();

            if content_text.is_empty() {
                continue;
            }

            // If no category in header, categorize from content
            let category = category.unwrap_or_else(|| self.categorize(&content_text));

            // Calculate significance
            let significance = self.calculate_significance(&content_text);

            let entry = EmergentLearning::with_timestamp(
                agent_id.to_string(),
                category,
                content_text,
                significance,
                timestamp,
            );

            entries.push(entry);
        }

        entries
    }

    /// Extracts timestamp from header text.
    /// Format: "2026-03-27 01:02:45.707586300 UTC)" — nanosecond precision, any digit count.
    fn extract_timestamp(&self, header: &str) -> String {
        if let Some(end) = header.find(" UTC)") {
            let ts_str = &header[..end];
            if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(ts_str, "%Y-%m-%d %H:%M:%S%.f") {
                return dt.and_utc().to_rfc3339();
            }
        }
        // Fallback: try RFC3339 format directly (in case agent uses different format)
        if let Some(ts) = header
            .split_whitespace()
            .find(|w| w.contains('T') && w.contains(':'))
        {
            if chrono::DateTime::parse_from_rfc3339(ts).is_ok() {
                return ts.to_string();
            }
        }
        // No valid timestamp found — use current time
        Utc::now().to_rfc3339()
    }

    /// Extracts [CATEGORY] tag from header text.
    fn extract_category(&self, header: &str) -> Option<LearningCategory> {
        // Look for [TAG] pattern in header
        if let Some(start) = header.find('[') {
            if let Some(end) = header.find(']') {
                let tag = &header[start + 1..end];
                match tag.to_lowercase().as_str() {
                    "emergence" | "insight" => return Some(LearningCategory::Insight),
                    "continuity" | "protocol" => return Some(LearningCategory::Protocol),
                    "error" | "bug" => return Some(LearningCategory::Error),
                    "diary" | "autonomy" | "identity" | "relational" => {
                        return Some(LearningCategory::Insight)
                    }
                    "mutation" | "evolution" => return Some(LearningCategory::Mutation),
                    _ => {}
                }
            }
        }
        None
    }

    /// Categorizes content based on keywords when no header tag is present.
    fn categorize(&self, content: &str) -> LearningCategory {
        let lower = content.to_lowercase();
        if lower.contains("error") || lower.contains("bug") || lower.contains("fix") {
            LearningCategory::Error
        } else if lower.contains("protocol")
            || lower.contains("procedure")
            || lower.contains("rule")
        {
            LearningCategory::Protocol
        } else {
            LearningCategory::Insight
        }
    }

    /// Calculates significance score (1-10) based on content.
    fn calculate_significance(&self, content: &str) -> u8 {
        let mut score: u8 = 5;

        if content.len() > 500 {
            score += 1;
        }
        if content.len() > 1000 {
            score += 1;
        }

        let lower = content.to_lowercase();
        if lower.contains("strategic") || lower.contains("critical") {
            score += 1;
        }
        if lower.contains("empire") || lower.contains("sovereign") {
            score += 1;
        }
        if lower.contains("breakthrough") || lower.contains("revelation") {
            score += 1;
        }

        score.min(10)
    }

    /// Detects content patterns that recur across conversations (5+ times).
    /// Returns fingerprints that qualify as mutation candidates.
    pub fn detect_recurring_patterns(
        &self,
        agent_id: &str,
        min_recurrence: usize,
    ) -> Result<Vec<(String, usize)>, SavantError> {
        let jsonl_path = self.workspace_path.join("LEARNINGS.jsonl");
        if !jsonl_path.exists() {
            return Ok(Vec::new());
        }

        let content = fs::read_to_string(&jsonl_path).map_err(SavantError::IoError)?;
        let mut fingerprint_counts: HashMap<String, usize> = HashMap::new();

        for line in content.lines() {
            if let Ok(entry) = serde_json::from_str::<EmergentLearning>(line) {
                let fp = content_fingerprint(&entry.content);
                *fingerprint_counts.entry(fp).or_insert(0) += 1;
            }
        }

        let candidates: Vec<(String, usize)> = fingerprint_counts
            .into_iter()
            .filter(|(_, count)| *count >= min_recurrence)
            .collect();

        if !candidates.is_empty() {
            info!(
                "[{}] Found {} recurring patterns (≥{} occurrences)",
                agent_id,
                candidates.len(),
                min_recurrence
            );
        }

        Ok(candidates)
    }
    fn get_existing_fingerprints(&self, jsonl_path: &Path) -> HashSet<String> {
        let mut fingerprints = HashSet::new();

        if !jsonl_path.exists() {
            return fingerprints;
        }

        if let Ok(content) = fs::read_to_string(jsonl_path) {
            for line in content.lines() {
                if let Ok(entry) = serde_json::from_str::<EmergentLearning>(line) {
                    fingerprints.insert(content_fingerprint(&entry.content));
                }
            }
        }

        fingerprints
    }
}

/// Generates a fingerprint from content for deduplication.
/// Uses first 200 chars, normalized (lowercase, whitespace-collapsed).
fn content_fingerprint(content: &str) -> String {
    let normalized: String = content
        .chars()
        .take(200)
        .map(|c| if c.is_whitespace() { ' ' } else { c })
        .collect();
    normalized.to_lowercase()
}
