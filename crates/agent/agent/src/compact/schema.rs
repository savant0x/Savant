//! Compact Rule Schema
//!
//! Defines the `CompactRule` struct and all supporting types for the
//! deterministic tool output compression engine. Rules are loaded from
//! JSON files and compiled into a trie-indexed registry.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Unique identifier for a compaction rule (e.g., "git/status", "cargo/test").
pub type RuleId = String;

/// Family classification for grouping related rules.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash, Default)]
pub enum RuleFamily {
    VersionControl,
    Testing,
    Build,
    PackageManager,
    Infrastructure,
    Cloud,
    FileSystem,
    Search,
    Network,
    Observability,
    Media,
    Archive,
    Database,
    Service,
    System,
    Lint,
    Install,
    Transfer,
    Task,
    #[default]
    Generic,
}

impl std::fmt::Display for RuleFamily {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RuleFamily::VersionControl => write!(f, "version-control"),
            RuleFamily::Testing => write!(f, "testing"),
            RuleFamily::Build => write!(f, "build"),
            RuleFamily::PackageManager => write!(f, "package-manager"),
            RuleFamily::Infrastructure => write!(f, "infrastructure"),
            RuleFamily::Cloud => write!(f, "cloud"),
            RuleFamily::FileSystem => write!(f, "filesystem"),
            RuleFamily::Search => write!(f, "search"),
            RuleFamily::Network => write!(f, "network"),
            RuleFamily::Observability => write!(f, "observability"),
            RuleFamily::Media => write!(f, "media"),
            RuleFamily::Archive => write!(f, "archive"),
            RuleFamily::Database => write!(f, "database"),
            RuleFamily::Service => write!(f, "service"),
            RuleFamily::System => write!(f, "system"),
            RuleFamily::Lint => write!(f, "lint"),
            RuleFamily::Install => write!(f, "install"),
            RuleFamily::Transfer => write!(f, "transfer"),
            RuleFamily::Task => write!(f, "task"),
            RuleFamily::Generic => write!(f, "generic"),
        }
    }
}

/// Match criteria for rule classification.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MatchCriteria {
    /// Tool names that trigger this rule (e.g., ["git", "cargo"]).
    #[serde(default)]
    pub tool_names: Vec<String>,
    /// Argv[0] patterns to match.
    #[serde(default)]
    pub argv0: Vec<String>,
    /// Substrings that must appear in the full argv.
    #[serde(default)]
    pub argv_includes: Vec<String>,
    /// Substrings that must appear in the full command string.
    #[serde(default)]
    pub command_includes: Vec<String>,
    /// Heuristic patterns to match against the first 512 bytes of output.
    #[serde(default)]
    pub output_heuristics: Vec<String>,
}

/// Filter patterns for line-level inclusion/exclusion.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Filters {
    /// Regex patterns for lines to skip (noise).
    #[serde(default)]
    pub skip_patterns: Vec<String>,
    /// Regex patterns for lines to keep (signal). If non-empty, only matching lines are retained.
    #[serde(default)]
    pub keep_patterns: Vec<String>,
}

/// Text transformation pipeline configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Transforms {
    /// Strip ANSI escape sequences.
    #[serde(default = "default_true")]
    pub strip_ansi: bool,
    /// Normalize whitespace (collapse multiple spaces, trim lines).
    #[serde(default)]
    pub normalize_whitespace: bool,
    /// Deduplicate adjacent identical lines.
    #[serde(default)]
    pub dedupe_adjacent_lines: bool,
    /// Extract/minify JSON structure.
    #[serde(default)]
    pub extract_json: bool,
    /// Trim empty lines from start and end.
    #[serde(default = "default_true")]
    pub trim_empty_edges: bool,
}

fn default_true() -> bool {
    true
}

impl Default for Transforms {
    fn default() -> Self {
        Self {
            strip_ansi: true,
            normalize_whitespace: false,
            dedupe_adjacent_lines: false,
            extract_json: false,
            trim_empty_edges: true,
        }
    }
}

/// Summarization strategy for preserving head/tail of output.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SummarizeStrategy {
    /// Number of lines to preserve from the head.
    #[serde(default = "default_head_lines")]
    pub head_lines: usize,
    /// Number of lines to preserve from the tail.
    #[serde(default = "default_tail_lines")]
    pub tail_lines: usize,
    /// Maximum characters for the summarized output.
    #[serde(default = "default_max_chars")]
    pub max_chars: usize,
}

fn default_head_lines() -> usize {
    50
}
fn default_tail_lines() -> usize {
    20
}
fn default_max_chars() -> usize {
    8_000
}

impl Default for SummarizeStrategy {
    fn default() -> Self {
        Self {
            head_lines: 50,
            tail_lines: 20,
            max_chars: 8_000,
        }
    }
}

/// Behavior when the tool exits with a non-zero code.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub enum FailureMode {
    /// Preserve the full raw output on failure.
    #[serde(rename = "preserve_raw")]
    #[default]
    PreserveRaw,
    /// Aggressively truncate on failure, keeping only head+tail.
    #[serde(rename = "aggressive_truncate")]
    AggressiveTruncate {
        head_lines: usize,
        tail_lines: usize,
    },
    /// Emit an error marker with the exit code.
    #[serde(rename = "emit_error_marker")]
    EmitErrorMarker,
}

/// Hint about the expected output type for structured data handling.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub enum OutputHint {
    #[serde(rename = "text")]
    #[default]
    PlainText,
    #[serde(rename = "json")]
    Json,
    #[serde(rename = "yaml")]
    Yaml,
    #[serde(rename = "binary")]
    Binary,
    #[serde(rename = "table")]
    Table,
}

/// A single compaction rule.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompactRule {
    /// Unique identifier (e.g., "git/status").
    pub id: RuleId,
    /// Family classification.
    #[serde(default)]
    pub family: RuleFamily,
    /// Inherit from another rule's configuration.
    #[serde(default)]
    pub extends: Option<RuleId>,
    /// Match criteria for classification.
    #[serde(default)]
    pub match_criteria: MatchCriteria,
    /// Line-level filters.
    #[serde(default)]
    pub filters: Filters,
    /// Text transformations.
    #[serde(default)]
    pub transforms: Transforms,
    /// Summarization strategy.
    #[serde(default)]
    pub summarize: SummarizeStrategy,
    /// Behavior on tool failure.
    #[serde(default)]
    pub failure_mode: FailureMode,
    /// Regex DoS protection timeout in milliseconds.
    #[serde(default = "default_budget_ms")]
    pub budget_ms: u32,
    /// Expected output type hint.
    #[serde(default)]
    pub output_hint: OutputHint,
    /// Alternate rule IDs to try if this rule achieves < target compression.
    #[serde(default)]
    pub fallback_chain: Vec<RuleId>,
    /// Minimum compression ratio (0.0-1.0) to consider this rule successful.
    #[serde(default = "default_min_ratio")]
    pub min_compression_ratio: f32,
    /// Human-readable description.
    #[serde(default)]
    pub description: String,
    /// Named regex counters (pattern name -> regex).
    #[serde(default)]
    pub counters: HashMap<String, String>,
}

fn default_budget_ms() -> u32 {
    5
}
fn default_min_ratio() -> f32 {
    0.05
}

impl Default for CompactRule {
    fn default() -> Self {
        Self {
            id: String::new(),
            family: RuleFamily::Generic,
            extends: None,
            match_criteria: MatchCriteria::default(),
            filters: Filters::default(),
            transforms: Transforms::default(),
            summarize: SummarizeStrategy::default(),
            failure_mode: FailureMode::default(),
            budget_ms: 5,
            output_hint: OutputHint::PlainText,
            fallback_chain: Vec::new(),
            min_compression_ratio: 0.05,
            description: String::new(),
            counters: HashMap::new(),
        }
    }
}

/// Compiled rule with pre-built regex patterns.
#[derive(Debug, Clone)]
pub struct CompiledRule {
    pub rule: CompactRule,
    pub skip_regexes: Vec<regex::Regex>,
    pub keep_regexes: Vec<regex::Regex>,
    pub heuristic_regexes: Vec<regex::Regex>,
    pub counter_regexes: Vec<(String, regex::Regex)>,
}

/// Input to the compaction engine from a tool execution.
#[derive(Debug, Clone)]
pub struct ToolOutput {
    /// Tool name (e.g., "git").
    pub tool_name: String,
    /// Full argument vector.
    pub argv: Vec<String>,
    /// Exit code (0 = success).
    pub exit_code: i32,
    /// Raw stdout+stderr combined.
    pub raw_output: String,
    /// Working directory.
    pub working_dir: Option<String>,
}

/// Result of the L1 compaction pipeline.
#[derive(Debug, Clone)]
pub struct CompactionResult {
    /// The compacted output string.
    pub output: String,
    /// Rule ID that was applied (or "passthrough" / "fallback").
    pub rule_id: String,
    /// Original byte count.
    pub original_bytes: usize,
    /// Compressed byte count.
    pub compressed_bytes: usize,
    /// Compression ratio (compressed / original).
    pub ratio: f32,
    /// Named counter values extracted from output.
    pub counters: HashMap<String, usize>,
    /// Whether the output was truncated.
    pub was_truncated: bool,
    /// Processing time in microseconds.
    pub processing_us: u64,
}

impl CompactionResult {
    pub fn passthrough(output: &str) -> Self {
        let len = output.len();
        Self {
            output: output.to_string(),
            rule_id: "passthrough".to_string(),
            original_bytes: len,
            compressed_bytes: len,
            ratio: 1.0,
            counters: HashMap::new(),
            was_truncated: false,
            processing_us: 0,
        }
    }
}

#[cfg(test)]
