//! Query Expansion & Reformulation (MEM-06)
//!
//! Rule-based query expansion that preprocesses queries before search:
//! - Temporal concretization ("last week" -> date range)
//! - Synonym expansion
//! - Entity extraction from queries

use chrono::{Datelike, Duration, NaiveDate, Utc};

/// Returns the epoch timestamp for midnight of the given date.
/// Since `and_hms_opt(0,0,0)` on a valid NaiveDate always succeeds,
/// we use the NaiveDate API directly.
fn midnight_ts(date: NaiveDate) -> i64 {
    date.and_hms_opt(0, 0, 0)
        .map(|dt| dt.and_utc().timestamp())
        .unwrap_or(0) // unreachable for valid dates, but satisfies clippy
}

/// Expanded query with additional search terms and optional date filters.
#[derive(Debug, Clone)]
pub struct ExpandedQuery {
    /// Original query text.
    pub original: String,
    /// Expanded terms (synonyms, temporal expansions).
    pub expanded_terms: Vec<String>,
    /// Optional date range filter (start, end) as Unix timestamps.
    pub date_range: Option<(i64, i64)>,
}

/// Expands a query with temporal concretization and synonym expansion.
pub fn expand_query(query: &str) -> ExpandedQuery {
    let mut expanded_terms = Vec::new();
    let mut date_range = None;
    let query_lower = query.to_lowercase();

    // Temporal concretization
    if query_lower.contains("today") {
        let now = Utc::now();
        let start = midnight_ts(now.date_naive());
        date_range = Some((start, now.timestamp()));
    } else if query_lower.contains("yesterday") {
        let now = Utc::now();
        let yesterday = now - Duration::days(1);
        let start = midnight_ts(yesterday.date_naive());
        let end = midnight_ts(now.date_naive());
        date_range = Some((start, end));
    } else if query_lower.contains("last week") {
        let now = Utc::now();
        let week_ago = now - Duration::weeks(1);
        date_range = Some((week_ago.timestamp(), now.timestamp()));
    } else if query_lower.contains("last month") {
        let now = Utc::now();
        let month_ago = now - Duration::days(30);
        date_range = Some((month_ago.timestamp(), now.timestamp()));
    } else if query_lower.contains("this week") {
        let now = Utc::now();
        let weekday = now.weekday().num_days_from_monday();
        let start = midnight_ts((now - Duration::days(weekday as i64)).date_naive());
        date_range = Some((start, now.timestamp()));
    }

    // Synonym expansion for common query terms
    let query_words: Vec<&str> = query.split_whitespace().collect();
    for word in &query_words {
        let w = word.to_lowercase();
        if let Some(synonyms) = QUERY_SYNONYMS.get(w.as_str()) {
            for syn in *synonyms {
                expanded_terms.push(syn.to_string());
            }
        }
    }

    // Entity extraction: detect quoted strings, capitalized words, and technical terms
    for word in &query_words {
        let clean = word.trim_matches(|c: char| !c.is_alphanumeric());
        if clean.len() > 2
            && clean
                .chars()
                .next()
                .map(|c| c.is_uppercase())
                .unwrap_or(false)
            && !TEMPORAL_WORDS.contains(&clean.to_lowercase().as_str())
        {
            expanded_terms.push(clean.to_lowercase());
        }
    }

    ExpandedQuery {
        original: query.to_string(),
        expanded_terms,
        date_range,
    }
}

/// Temporal words to exclude from entity extraction.
const TEMPORAL_WORDS: &[&str] = &[
    "today",
    "yesterday",
    "tomorrow",
    "last",
    "this",
    "next",
    "week",
    "month",
    "year",
];

/// Query-specific synonyms (different from BM25 index synonyms).
static QUERY_SYNONYMS: std::sync::LazyLock<
    std::collections::HashMap<&'static str, &'static [&'static str]>,
> = std::sync::LazyLock::new(|| {
    let mut m = std::collections::HashMap::new();
    m.insert("find", &["search", "locate", "get"] as &[&str]);
    m.insert("search", &["find", "locate", "get", "query"] as &[&str]);
    m.insert("show", &["display", "list", "get"] as &[&str]);
    m.insert("get", &["find", "retrieve", "fetch"] as &[&str]);
    m.insert("create", &["make", "add", "new"] as &[&str]);
    m.insert("delete", &["remove", "drop", "destroy"] as &[&str]);
    m.insert("update", &["modify", "change", "edit"] as &[&str]);
    m.insert("bug", &["issue", "error", "defect", "problem"] as &[&str]);
    m.insert("fix", &["resolve", "repair", "patch"] as &[&str]);
    m.insert("help", &["assist", "support", "guide"] as &[&str]);
    m.insert("explain", &["describe", "clarify", "detail"] as &[&str]);
    m.insert("config", &["configuration", "settings", "setup"] as &[&str]);
    m.insert("log", &["logs", "logging", "output"] as &[&str]);
    m.insert("test", &["tests", "testing", "spec"] as &[&str]);
    m.insert("deploy", &["deployment", "release", "publish"] as &[&str]);
    m
});

#[cfg(test)]
#[allow(clippy::disallowed_methods)]
mod tests {
    use super::*;

    #[test]
    fn test_expand_temporal_today() {
        let result = expand_query("memories from today");
        assert!(result.date_range.is_some());
        let (start, end) = result.date_range.unwrap();
        assert!(start <= end);
    }

    #[test]
    fn test_expand_temporal_last_week() {
        let result = expand_query("what happened last week");
        assert!(result.date_range.is_some());
        let (start, end) = result.date_range.unwrap();
        assert!(end - start >= 7 * 24 * 3600 - 1); // ~7 days
    }

    #[test]
    fn test_expand_temporal_yesterday() {
        let result = expand_query("show me yesterday's logs");
        assert!(result.date_range.is_some());
    }

    #[test]
    fn test_expand_no_temporal() {
        let result = expand_query("how does the memory system work");
        assert!(result.date_range.is_none());
    }

    #[test]
    fn test_expand_synonyms() {
        let result = expand_query("find all bugs in the codebase");
        assert!(result
            .expanded_terms
            .iter()
            .any(|t| t == "search" || t == "issue" || t == "error"));
    }

    #[test]
    fn test_expand_entity_extraction() {
        let result = expand_query("the Rust compiler has a bug");
        // "Rust" should be extracted as an entity (capitalized)
        assert!(result.expanded_terms.iter().any(|t| t == "rust"));
    }

    #[test]
    fn test_expand_preserves_original() {
        let result = expand_query("test query");
        assert_eq!(result.original, "test query");
    }
}
