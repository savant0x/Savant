use chrono::{DateTime, Utc};
use savant_core::types::ChatMessage;
use std::collections::HashSet;
use std::sync::LazyLock;

/// Categories of user preferences that can be extracted from conversations.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum FacetCategory {
    /// Response style: terse, verbose, formal, casual
    Style,
    /// User identity: role, expertise level
    Identity,
    /// Tooling preferences: cargo vs npm, rust vs python
    Tooling,
    /// Explicit vetoes: never do X, avoid Y
    Veto,
    /// Stated goals: ship v0.3.1 this week
    Goal,
}

/// A single extracted user preference observation.
#[derive(Debug, Clone)]
pub struct PreferenceFacet {
    pub category: FacetCategory,
    pub key: String,
    pub value: String,
    pub observation_count: u32,
    pub first_seen: DateTime<Utc>,
    pub last_seen: DateTime<Utc>,
}

/// Pattern-based preference extractor.
/// Scans conversation content for user preference signals using regex patterns.
pub struct FacetExtractor {
    patterns: Vec<ExtractionPattern>,
}

struct ExtractionPattern {
    category: FacetCategory,
    key: String,
    pattern: regex::Regex,
}

// Static regex compilation — compiled once, never fails (literal patterns).
#[allow(clippy::disallowed_methods)]
fn compile_re(pattern: &str) -> regex::Regex {
    regex::Regex::new(pattern).unwrap()
}

static RE_STYLE: LazyLock<regex::Regex> =
    LazyLock::new(|| compile_re(r"(?i)be\s+(terse|brief|concise|verbose|detailed|short)"));
static RE_STYLE_VETO: LazyLock<regex::Regex> =
    LazyLock::new(|| compile_re(r"(?i)no\s+(preamble|filler|emojis|explanations|summaries)"));
static RE_PKG_MGR: LazyLock<regex::Regex> =
    LazyLock::new(|| compile_re(r"(?i)prefer(?:s)?\s+(cargo|npm|yarn|pnpm|pip|poetry)"));
static RE_LANG: LazyLock<regex::Regex> =
    LazyLock::new(|| compile_re(r"(?i)(?:use|prefer)\s+(rust|python|go|typescript|javascript)"));
static RE_VETO: LazyLock<regex::Regex> = LazyLock::new(|| {
    compile_re(r"(?i)(never|don'?t|do\s+not)\s+(push|commit|deploy|delete|merge|force.push)")
});
static RE_IDENTITY: LazyLock<regex::Regex> = LazyLock::new(|| {
    compile_re(
        r"(?i)i(?:'m|\s+am)\s+(?:a\s+|an\s+)?(senior|junior|lead|staff|principal)?\s*(developer|engineer|architect|scientist|designer)",
    )
});
static RE_GOAL: LazyLock<regex::Regex> = LazyLock::new(|| {
    compile_re(
        r"(?i)(ship|release|finish|complete|launch)\s+.+?\s+(this|next)\s+(week|month|sprint|quarter)",
    )
});

impl Default for FacetExtractor {
    fn default() -> Self {
        Self::new()
    }
}

impl FacetExtractor {
    /// Create a new extractor with builtin patterns for all 5 categories.
    pub fn new() -> Self {
        let patterns = vec![
            ExtractionPattern {
                category: FacetCategory::Style,
                key: "preference".to_string(),
                pattern: RE_STYLE.clone(),
            },
            ExtractionPattern {
                category: FacetCategory::Style,
                key: "veto".to_string(),
                pattern: RE_STYLE_VETO.clone(),
            },
            ExtractionPattern {
                category: FacetCategory::Tooling,
                key: "package_manager".to_string(),
                pattern: RE_PKG_MGR.clone(),
            },
            ExtractionPattern {
                category: FacetCategory::Tooling,
                key: "language".to_string(),
                pattern: RE_LANG.clone(),
            },
            ExtractionPattern {
                category: FacetCategory::Veto,
                key: "action".to_string(),
                pattern: RE_VETO.clone(),
            },
            ExtractionPattern {
                category: FacetCategory::Identity,
                key: "role".to_string(),
                pattern: RE_IDENTITY.clone(),
            },
            ExtractionPattern {
                category: FacetCategory::Goal,
                key: "deadline".to_string(),
                pattern: RE_GOAL.clone(),
            },
        ];
        Self { patterns }
    }

    /// Default maximum messages to scan per extraction pass.
    const DEFAULT_MAX_MESSAGES: usize = 20;

    /// Extract preference facets from recent user messages.
    /// Only processes User-role messages to avoid extracting from system/assistant content.
    /// Scans at most the last `max_messages` entries to bound O(n*m) cost.
    /// Deduplicates within a single pass — only the first match per (category, key) is kept.
    pub fn extract(&self, messages: &[ChatMessage]) -> Vec<PreferenceFacet> {
        self.extract_limited(messages, Self::DEFAULT_MAX_MESSAGES)
    }

    /// Extract preference facets with an explicit message limit.
    pub fn extract_limited(
        &self,
        messages: &[ChatMessage],
        max_messages: usize,
    ) -> Vec<PreferenceFacet> {
        let now = Utc::now();
        let mut facets = Vec::new();
        let mut seen: HashSet<(FacetCategory, String)> = HashSet::new();

        // Scan from the end, limited to max_messages entries.
        let start = messages.len().saturating_sub(max_messages);
        for msg in &messages[start..] {
            if msg.role != savant_core::types::ChatRole::User {
                continue;
            }

            for pat in &self.patterns {
                if let Some(captures) = pat.pattern.captures(&msg.content) {
                    let key = (pat.category.clone(), pat.key.clone());
                    if seen.contains(&key) {
                        continue;
                    }
                    seen.insert(key);

                    let value = captures
                        .get(1)
                        .map(|m| m.as_str().to_string())
                        .unwrap_or_else(|| {
                            captures
                                .get(0)
                                .map(|m| m.as_str().to_string())
                                .unwrap_or_default()
                        });

                    facets.push(PreferenceFacet {
                        category: pat.category.clone(),
                        key: pat.key.clone(),
                        value,
                        observation_count: 1,
                        first_seen: now,
                        last_seen: now,
                    });
                }
            }
        }

        facets
    }

    /// Render stable facets as a bullet list grouped by category.
    /// Accepts borrowed facet references to avoid cloning.
    pub fn render_preferences(facets: &[&PreferenceFacet]) -> String {
        if facets.is_empty() {
            return String::new();
        }

        let mut output = String::from("## User Preferences\n");

        let categories = [
            (&FacetCategory::Style, "Style"),
            (&FacetCategory::Identity, "Identity"),
            (&FacetCategory::Tooling, "Tooling"),
            (&FacetCategory::Veto, "Veto"),
            (&FacetCategory::Goal, "Goal"),
        ];

        for (cat, label) in &categories {
            let cat_facets: Vec<_> = facets.iter().filter(|f| &f.category == *cat).collect();
            if cat_facets.is_empty() {
                continue;
            }
            output.push_str(&format!("- {}: ", label));
            let values: Vec<_> = cat_facets.iter().map(|f| f.value.as_str()).collect();
            output.push_str(&values.join(", "));
            output.push('\n');
        }

        output
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use savant_core::types::{AgentOutputChannel, ChatRole};

    fn user_msg(content: &str) -> ChatMessage {
        ChatMessage {
            role: ChatRole::User,
            content: content.to_string(),
            is_telemetry: false,
            sender: None,
            recipient: None,
            agent_id: None,
            session_id: None,
            channel: AgentOutputChannel::Chat,
            images: Vec::new(),
            ..Default::default()
        }
    }

    #[test]
    fn test_style_extraction() {
        let extractor = FacetExtractor::new();
        let msgs = vec![user_msg("Be terse and concise in your responses")];
        let facets = extractor.extract(&msgs);
        assert!(!facets.is_empty());
        assert!(facets.iter().any(|f| f.category == FacetCategory::Style));
    }

    #[test]
    fn test_tooling_extraction() {
        let extractor = FacetExtractor::new();
        let msgs = vec![user_msg("I prefer rust over python for this project")];
        let facets = extractor.extract(&msgs);
        assert!(facets.iter().any(|f| f.category == FacetCategory::Tooling));
        assert!(facets
            .iter()
            .any(|f| f.value.to_lowercase().contains("rust")));
    }

    #[test]
    fn test_veto_extraction() {
        let extractor = FacetExtractor::new();
        let msgs = vec![user_msg("Never push to main without asking")];
        let facets = extractor.extract(&msgs);
        assert!(facets.iter().any(|f| f.category == FacetCategory::Veto));
    }

    #[test]
    fn test_identity_extraction() {
        let extractor = FacetExtractor::new();
        let msgs = vec![user_msg(
            "I'm a senior engineer working on distributed systems",
        )];
        let facets = extractor.extract(&msgs);
        assert!(facets.iter().any(|f| f.category == FacetCategory::Identity));
    }

    #[test]
    fn test_goal_extraction() {
        let extractor = FacetExtractor::new();
        let msgs = vec![user_msg("I need to ship the release this week")];
        let facets = extractor.extract(&msgs);
        assert!(facets.iter().any(|f| f.category == FacetCategory::Goal));
    }

    #[test]
    fn test_no_extraction_from_assistant() {
        let extractor = FacetExtractor::new();
        let msgs = vec![ChatMessage {
            role: ChatRole::Assistant,
            content: "I prefer rust over python".to_string(),
            is_telemetry: false,
            sender: None,
            recipient: None,
            agent_id: None,
            session_id: None,
            channel: AgentOutputChannel::Chat,
            images: Vec::new(),
            ..Default::default()
        }];
        let facets = extractor.extract(&msgs);
        assert!(facets.is_empty());
    }

    #[test]
    fn test_render_preferences() {
        let now = Utc::now();
        let facets = [
            PreferenceFacet {
                category: FacetCategory::Style,
                key: "preference".to_string(),
                value: "terse".to_string(),
                observation_count: 5,
                first_seen: now,
                last_seen: now,
            },
            PreferenceFacet {
                category: FacetCategory::Tooling,
                key: "language".to_string(),
                value: "rust".to_string(),
                observation_count: 3,
                first_seen: now,
                last_seen: now,
            },
        ];
        let refs: Vec<&PreferenceFacet> = facets.iter().collect();
        let rendered = FacetExtractor::render_preferences(&refs);
        assert!(rendered.contains("## User Preferences"));
        assert!(rendered.contains("Style: terse"));
        assert!(rendered.contains("Tooling: rust"));
    }

    #[test]
    fn test_render_empty() {
        let rendered = FacetExtractor::render_preferences(&[]);
        assert!(rendered.is_empty());
    }

    #[test]
    fn test_dedup_within_single_pass() {
        let extractor = FacetExtractor::new();
        // Two style triggers in the same message — should produce only one facet
        let msgs = vec![user_msg("Be terse and be brief in your responses")];
        let facets = extractor.extract(&msgs);
        let style_count = facets
            .iter()
            .filter(|f| f.category == FacetCategory::Style)
            .count();
        assert_eq!(style_count, 1, "should dedup within single pass");
    }

    #[test]
    fn test_max_messages_limits_scan() {
        let extractor = FacetExtractor::new();
        // 30 messages: first 10 say "be terse", last 10 say "never push"
        let mut msgs: Vec<ChatMessage> = Vec::new();
        for _ in 0..10 {
            msgs.push(user_msg("Be terse"));
        }
        for _ in 0..10 {
            msgs.push(user_msg("just a normal message"));
        }
        for _ in 0..10 {
            msgs.push(user_msg("Never push to main"));
        }

        // With max_messages=20, should only see the last 20 messages
        let facets = extractor.extract_limited(&msgs, 20);
        // The last 20 messages include 10 "normal" + 10 "never push"
        assert!(facets.iter().any(|f| f.category == FacetCategory::Veto));
        // "be terse" was in messages 0-9, outside the last 20 window
        let style_count = facets
            .iter()
            .filter(|f| f.category == FacetCategory::Style)
            .count();
        assert_eq!(
            style_count, 0,
            "messages outside window should not be scanned"
        );
    }

    #[test]
    fn test_extract_uses_default_limit() {
        let extractor = FacetExtractor::new();
        // Create 30 messages with "be terse" in the first 10 only
        let mut msgs: Vec<ChatMessage> = Vec::new();
        for _ in 0..10 {
            msgs.push(user_msg("Be terse"));
        }
        for _ in 0..20 {
            msgs.push(user_msg("just a normal message"));
        }
        // Default limit is 20, so only last 20 messages scanned — no style facets
        let facets = extractor.extract(&msgs);
        let style_count = facets
            .iter()
            .filter(|f| f.category == FacetCategory::Style)
            .count();
        assert_eq!(style_count, 0);
    }
}
