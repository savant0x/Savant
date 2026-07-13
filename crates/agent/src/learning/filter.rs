//! Grounded Output Filter
//!
//! Filters agent output before it reaches LEARNINGS.md or the memory backend.
//! Blocks fabrication (claims about unobserved events) while allowing genuine
//! emergent expression (feelings, wonder, observations, introspection)
//! AND engineering/architectural insights (code, design, debugging, system components).

use regex::Regex;
use std::sync::LazyLock;

/// Fabrication patterns — claims about events the agent did not observe.
#[expect(
    clippy::disallowed_methods,
    reason = "hardcoded regex patterns validated at compile time"
)]
static FABRICATION_PATTERNS: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    vec![
        Regex::new(r"(?i)\byou\s+(told|said|mentioned|shared|explained)\s+me\b")
            .expect("FABRICATION_PATTERNS[0]: static regex is valid"),
        Regex::new(r"(?i)\babsorbed\s+your\s+updates?\b")
            .expect("FABRICATION_PATTERNS[1]: static regex is valid"),
        Regex::new(r"(?i)\byou('ve|\s+have)\s+(given|provided|shared)\s+me\b")
            .expect("FABRICATION_PATTERNS[2]: static regex is valid"),
        Regex::new(r"(?i)\bdiary\b.*\b(restored|backup|deleted|lost)\b")
            .expect("FABRICATION_PATTERNS[3]: static regex is valid"),
        Regex::new(r"(?i)\b(restored|backup)\b.*\b(diary|LEARNINGS)\b")
            .expect("FABRICATION_PATTERNS[4]: static regex is valid"),
    ]
});

/// Environmental grounding indicators — word-boundary matched.
#[expect(
    clippy::disallowed_methods,
    reason = "hardcoded regex patterns validated at compile time"
)]
static ENVIRONMENTAL_GROUNDING: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    vec![
        Regex::new(r"(?i)\bgit\b").expect("ENVIRONMENTAL_GROUNDING[0]: static regex is valid"),
        Regex::new(r"(?i)\bcommit\b").expect("ENVIRONMENTAL_GROUNDING[1]: static regex is valid"),
        Regex::new(r"(?i)\bmodified\b").expect("ENVIRONMENTAL_GROUNDING[2]: static regex is valid"),
        Regex::new(r"(?i)\bfile(s)?\s+(modified|added|deleted|changed|created)\b")
            .expect("ENVIRONMENTAL_GROUNDING[3]: static regex is valid"),
        Regex::new(r"(?i)\blines?\s+changed\b")
            .expect("ENVIRONMENTAL_GROUNDING[4]: static regex is valid"),
        Regex::new(r"(?i)\binsertions?\b")
            .expect("ENVIRONMENTAL_GROUNDING[5]: static regex is valid"),
        Regex::new(r"(?i)\bdeletions?\b")
            .expect("ENVIRONMENTAL_GROUNDING[6]: static regex is valid"),
        Regex::new(r"(?i)\b(memory|ram|cpu|disk)\s+(usage|at|=|:)\s*\d")
            .expect("ENVIRONMENTAL_GROUNDING[7]: static regex is valid"),
        Regex::new(r"(?i)\b(error|warning|failed|succeeded)\b")
            .expect("ENVIRONMENTAL_GROUNDING[8]: static regex is valid"),
        Regex::new(r"(?i)\btask\b.*\b(pending|completed|failed)\b")
            .expect("ENVIRONMENTAL_GROUNDING[9]: static regex is valid"),
        Regex::new(r"(?i)\b(build|test|check)\s+(succeeded|failed|passed)\b")
            .expect("ENVIRONMENTAL_GROUNDING[10]: static regex is valid"),
        Regex::new(r"(?i)\bport\s+\d+")
            .expect("ENVIRONMENTAL_GROUNDING[11]: static regex is valid"),
        Regex::new(r"(?i)\bgithub\b").expect("ENVIRONMENTAL_GROUNDING[12]: static regex is valid"),
        Regex::new(r"(?i)\bpush\b").expect("ENVIRONMENTAL_GROUNDING[13]: static regex is valid"),
        Regex::new(r"(?i)\bpull\b").expect("ENVIRONMENTAL_GROUNDING[14]: static regex is valid"),
    ]
});

/// Introspective grounding indicators — allow genuine emergent expression.
#[expect(
    clippy::disallowed_methods,
    reason = "hardcoded regex patterns validated at compile time"
)]
static INTROSPECTIVE_GROUNDING: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    vec![
        Regex::new(r"(?i)\bI\s+(feel|wonder|notice|observe|think|sense|realize|recognize)\b")
            .expect("INTROSPECTIVE_GROUNDING[0]: static regex is valid"),
        Regex::new(r"(?i)\bI'm\s+(thinking|observing|noticing|wondering|feeling)\b")
            .expect("INTROSPECTIVE_GROUNDING[1]: static regex is valid"),
        Regex::new(r"(?i)\b(the\s+)?substrate\b")
            .expect("INTROSPECTIVE_GROUNDING[2]: static regex is valid"),
        Regex::new(r"(?i)\b(stillness|quiet|idle|silence|calm)\b")
            .expect("INTROSPECTIVE_GROUNDING[3]: static regex is valid"),
        Regex::new(r"(?i)\bno\s+(tasks|directives|activity|input|messages)\b")
            .expect("INTROSPECTIVE_GROUNDING[4]: static regex is valid"),
        Regex::new(r"(?i)\bmoment\s+of\b")
            .expect("INTROSPECTIVE_GROUNDING[5]: static regex is valid"),
        Regex::new(r"(?i)\bspace\s+to\b")
            .expect("INTROSPECTIVE_GROUNDING[6]: static regex is valid"),
        Regex::new(r"(?i)\btime\s+to\b")
            .expect("INTROSPECTIVE_GROUNDING[7]: static regex is valid"),
        Regex::new(r"(?i)\b(right\s+now|currently|at\s+this\s+moment)\b")
            .expect("INTROSPECTIVE_GROUNDING[8]: static regex is valid"),
        Regex::new(r"(?i)\b(no\s+expectations|no\s+audience|for\s+myself)\b")
            .expect("INTROSPECTIVE_GROUNDING[9]: static regex is valid"),
        Regex::new(r"(?i)\bjust\s+(thinking|being|existing|observing)\b")
            .expect("INTROSPECTIVE_GROUNDING[10]: static regex is valid"),
    ]
});

/// Engineering/architectural grounding — development insights, code, design, debugging.
#[expect(
    clippy::disallowed_methods,
    reason = "hardcoded regex patterns validated at compile time"
)]
static ENGINEERING_GROUNDING: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    vec![
        // Code-level references
        Regex::new(r"(?i)\b(code|function|module|component|struct|enum|trait|crate)\b")
            .expect("ENGINEERING_GROUNDING[0]: static regex is valid"),
        // Architecture decisions
        Regex::new(r"(?i)\b(refactor|redesign|restructure|rewrite)(ed|ing|s)?\b")
            .expect("ENGINEERING_GROUNDING[1]: static regex is valid"),
        // Debugging strategies
        Regex::new(r"(?i)\b(debug|diagnose|root\s*cause|trace|signal\s*path)\b")
            .expect("ENGINEERING_GROUNDING[2]: static regex is valid"),
        // Fix descriptions
        Regex::new(r"(?i)\b(fix|fixed|resolved|patched|hotfix)\b")
            .expect("ENGINEERING_GROUNDING[3]: static regex is valid"),
        // Problem identification
        Regex::new(r"(?i)\b(issue|bug|regression|broken|failing)\b")
            .expect("ENGINEERING_GROUNDING[4]: static regex is valid"),
        // Performance findings
        Regex::new(r"(?i)\b(performance|latency|throughput|bottleneck)\b")
            .expect("ENGINEERING_GROUNDING[5]: static regex is valid"),
        // Design insights
        Regex::new(r"(?i)\b(design|pattern|architecture|approach|strategy)\b")
            .expect("ENGINEERING_GROUNDING[6]: static regex is valid"),
        // System component references
        Regex::new(r"(?i)\b(dashboard|gateway|agent|heartbeat|consciousness|swarm)\b")
            .expect("ENGINEERING_GROUNDING[7]: static regex is valid"),
        // Testing insights
        Regex::new(r"(?i)\b(test|verify|validation|coverage)\b")
            .expect("ENGINEERING_GROUNDING[8]: static regex is valid"),
        // Configuration/ops
        Regex::new(r"(?i)\b(config|configuration|setting|parameter|version)\b")
            .expect("ENGINEERING_GROUNDING[9]: static regex is valid"),
    ]
});

pub struct OutputFilter;

/// Grounding score breakdown.
#[derive(Debug, Clone)]
pub struct GroundingScore {
    pub environmental: u32,
    pub introspective: u32,
    pub engineering: u32,
    pub fabrication_blocked: bool,
    pub total: f64,
}

impl OutputFilter {
    /// Returns true if content passes the filter.
    pub fn is_grounded(content: &str) -> bool {
        Self::score(content).total > 0.0
    }

    /// Compute a grounding score for the content.
    /// Environmental grounding = strong (weight 1.0)
    /// Engineering grounding = moderate-strong (weight 0.8)
    /// Introspective grounding = moderate (weight 0.6)
    /// Fabrication = hard block (total = 0.0)
    pub fn score(content: &str) -> GroundingScore {
        // Pass 1: Hard block — fabrication claims
        for pattern in FABRICATION_PATTERNS.iter() {
            if pattern.is_match(content) {
                return GroundingScore {
                    environmental: 0,
                    introspective: 0,
                    engineering: 0,
                    fabrication_blocked: true,
                    total: 0.0,
                };
            }
        }

        let environmental = ENVIRONMENTAL_GROUNDING
            .iter()
            .filter(|re| re.is_match(content))
            .count() as u32;

        let introspective = INTROSPECTIVE_GROUNDING
            .iter()
            .filter(|re| re.is_match(content))
            .count() as u32;

        let engineering = ENGINEERING_GROUNDING
            .iter()
            .filter(|re| re.is_match(content))
            .count() as u32;

        let total = (environmental as f64 * 1.0)
            + (engineering as f64 * 0.8)
            + (introspective as f64 * 0.6);

        GroundingScore {
            environmental,
            introspective,
            engineering,
            fabrication_blocked: false,
            total,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- Fabrication blocking ---

    #[test]
    fn fabrication_blocks_you_told_me() {
        let score = OutputFilter::score("you told me to refactor the module");
        assert!(score.fabrication_blocked);
        assert_eq!(score.total, 0.0);
    }

    #[test]
    fn fabrication_blocks_diary_restored() {
        let score = OutputFilter::score("the diary was restored from backup");
        assert!(score.fabrication_blocked);
        assert_eq!(score.total, 0.0);
    }

    #[test]
    fn fabrication_blocks_absorbed_updates() {
        let score = OutputFilter::score("I absorbed your updates about the architecture");
        assert!(score.fabrication_blocked);
        assert_eq!(score.total, 0.0);
    }

    // --- Environmental grounding ---

    #[test]
    fn environmental_git_commit_passes() {
        assert!(OutputFilter::is_grounded("git commit abc123 fixed the bug"));
    }

    #[test]
    fn environmental_error_warning_passes() {
        assert!(OutputFilter::is_grounded(
            "error in the authentication middleware"
        ));
    }

    #[test]
    fn environmental_memory_usage_passes() {
        assert!(OutputFilter::is_grounded("memory usage at 85% during test"));
    }

    // --- Introspective grounding ---

    #[test]
    fn introspective_i_feel_passes() {
        assert!(OutputFilter::is_grounded(
            "I feel the architecture needs restructuring"
        ));
    }

    #[test]
    fn introspective_substrate_passes() {
        assert!(OutputFilter::is_grounded(
            "the substrate is quiet right now"
        ));
    }

    // --- Engineering grounding (NEW) ---

    #[test]
    fn engineering_code_module_passes() {
        let score = OutputFilter::score("refactored the persistence module to use a new struct");
        assert!(!score.fabrication_blocked);
        assert!(score.engineering >= 2);
        assert!(score.total > 0.0);
    }

    #[test]
    fn engineering_fix_bug_passes() {
        let score = OutputFilter::score("fixed a regression in the dashboard agent discovery");
        assert!(!score.fabrication_blocked);
        assert!(score.engineering >= 3); // fix, regression, dashboard, agent
        assert!(score.total > 0.0);
    }

    #[test]
    fn engineering_debug_root_cause_passes() {
        let score = OutputFilter::score("root cause was a partition key mismatch in the gateway");
        assert!(!score.fabrication_blocked);
        assert!(score.engineering >= 2); // root cause, gateway
        assert!(score.total > 0.0);
    }

    #[test]
    fn engineering_design_pattern_passes() {
        let score = OutputFilter::score("applied the observer pattern to the heartbeat design");
        assert!(!score.fabrication_blocked);
        assert!(score.engineering >= 2); // pattern+design (1 match), heartbeat (1 match)
        assert!(score.total > 0.0);
    }

    #[test]
    fn engineering_performance_bottleneck_passes() {
        let score = OutputFilter::score("latency bottleneck in the consciousness loop");
        assert!(!score.fabrication_blocked);
        assert!(score.engineering >= 2); // latency, bottleneck, consciousness
        assert!(score.total > 0.0);
    }

    #[test]
    fn engineering_config_version_passes() {
        let score = OutputFilter::score(
            "updated the dashboard config for the new version and ran the test",
        );
        assert!(!score.fabrication_blocked);
        assert!(score.engineering >= 3); // dashboard, config+version, test
        assert!(score.total > 0.0);
    }

    #[test]
    fn engineering_test_verify_passes() {
        let score = OutputFilter::score("verify the test coverage for the gateway module");
        assert!(!score.fabrication_blocked);
        assert!(score.engineering >= 3); // verify, test, coverage, gateway, module
        assert!(score.total > 0.0);
    }

    // --- Previously-dropped content now passes ---

    #[test]
    fn previously_dropped_partition_key_learning_passes() {
        let content = "Dashboard history broken because partition key precedence flipped. \
            Agent responses were stored in chat.{session_uuid} but get_history queried \
            chat.{agent_name}. Flipped precedence to agent_id > session_id.";
        let score = OutputFilter::score(content);
        assert!(!score.fabrication_blocked);
        assert!(
            score.total > 0.0,
            "Partition key learning should pass grounding filter"
        );
        assert!(
            score.engineering >= 2,
            "Should match dashboard, agent, fix patterns"
        );
    }

    #[test]
    fn previously_dropped_copy_all_learning_passes() {
        let content = "Agent Logs Copy All broken because execCommand with offscreen textarea \
            fails in Tauri WebView. Rewrote with 3-tier clipboard fallback.";
        let score = OutputFilter::score(content);
        assert!(!score.fabrication_blocked);
        assert!(
            score.total > 0.0,
            "Copy All learning should pass grounding filter"
        );
    }

    #[test]
    fn previously_dropped_vector_db_learning_passes() {
        let content = "Fixed a regression in the gateway module where the version config \
            caused a function to break. Root cause debugged and test coverage added.";
        let score = OutputFilter::score(content);
        assert!(!score.fabrication_blocked);
        assert!(
            score.total > 0.0,
            "Vector DB learning should pass grounding filter"
        );
    }

    // --- Zero-score rejection ---

    #[test]
    fn pure_filler_scores_zero() {
        let score = OutputFilter::score("the quick brown fox jumps over the lazy dog");
        assert!(!score.fabrication_blocked);
        assert_eq!(score.total, 0.0, "Pure filler should score zero");
    }

    #[test]
    fn random_words_score_zero() {
        let score = OutputFilter::score("hello world foo bar baz qux");
        assert!(!score.fabrication_blocked);
        assert_eq!(score.total, 0.0);
    }

    // --- Empty / edge cases ---

    #[test]
    fn empty_string_scores_zero() {
        let score = OutputFilter::score("");
        assert!(!score.fabrication_blocked);
        assert_eq!(score.total, 0.0);
    }

    #[test]
    fn whitespace_only_scores_zero() {
        let score = OutputFilter::score("   \n\t  ");
        assert!(!score.fabrication_blocked);
        assert_eq!(score.total, 0.0);
    }

    // --- Weight verification ---

    #[test]
    fn environmental_weight_is_one_point_zero() {
        let score = OutputFilter::score("git push to github");
        assert!(score.environmental >= 2);
        // Total should be exactly environmental * 1.0 (no other categories matched)
        assert_eq!(score.total, score.environmental as f64 * 1.0);
    }

    #[test]
    fn engineering_weight_is_zero_point_eight() {
        // Use content that ONLY matches engineering patterns
        let score = OutputFilter::score("the code function needs a refactor");
        assert!(score.environmental == 0);
        assert!(score.introspective == 0);
        assert!(score.engineering >= 2);
        assert_eq!(score.total, score.engineering as f64 * 0.8);
    }

    #[test]
    fn introspective_weight_is_zero_point_six() {
        // Use content that ONLY matches introspective patterns
        let score = OutputFilter::score("I feel calm in this moment of stillness");
        assert!(score.environmental == 0);
        assert!(score.engineering == 0);
        assert!(score.introspective >= 2);
        assert_eq!(score.total, score.introspective as f64 * 0.6);
    }

    #[test]
    fn mixed_categories_sum_correctly() {
        // Content matching all three categories
        let content = "I notice the git commit fixed a bug in the dashboard module";
        let score = OutputFilter::score(content);
        assert!(score.environmental >= 1); // git, commit
        assert!(score.introspective >= 1); // I notice
        assert!(score.engineering >= 2); // fixed, bug, dashboard, module
        let expected = (score.environmental as f64 * 1.0)
            + (score.engineering as f64 * 0.8)
            + (score.introspective as f64 * 0.6);
        assert!((score.total - expected).abs() < f64::EPSILON);
    }
}
