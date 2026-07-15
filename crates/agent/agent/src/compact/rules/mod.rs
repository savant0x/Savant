//! Rule registry with three-layer overlay and hot-reload support.
//!
//! Layers (highest priority first):
//! 1. Project rules: `.compact/rules/` in workspace
//! 2. User rules: `~/.config/savant/rules/`
//! 3. Builtin rules: embedded JSON in binary

use crate::compact::schema::*;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tracing::{info, warn};

/// Registry holding all compiled rules across three layers.
#[derive(Debug, Clone)]
pub struct RuleRegistry {
    /// All compiled rules, ordered by priority (project > user > builtin).
    pub rules: Vec<Arc<CompiledRule>>,
    /// Map from rule ID to index in `rules`.
    pub id_index: HashMap<RuleId, usize>,
    /// Builtin rule JSONs (id, json).
    pub builtins: Vec<(RuleId, String)>,
    /// User rules directory.
    pub user_rules_dir: PathBuf,
    /// Project rules directory.
    pub project_rules_dir: PathBuf,
}

impl RuleRegistry {
    /// Creates a new registry with built-in rules loaded.
    pub fn new(user_rules_dir: PathBuf, project_rules_dir: PathBuf) -> Self {
        let builtins = Self::load_builtin_rules();
        let mut registry = Self {
            rules: Vec::new(),
            id_index: HashMap::new(),
            builtins,
            user_rules_dir,
            project_rules_dir,
        };
        registry.recompile();
        registry
    }

    /// Loads all rules from all three layers and recompiles the registry.
    pub fn recompile(&mut self) {
        self.rules.clear();
        self.id_index.clear();

        // Load in reverse priority order so higher-priority rules overwrite
        let builtins = self.builtins.clone();
        for (id, json) in &builtins {
            self.load_rule_json(id, json);
        }
        let user_dir = self.user_rules_dir.clone();
        self.load_rules_from_dir(&user_dir);
        let project_dir = self.project_rules_dir.clone();
        self.load_rules_from_dir(&project_dir);

        info!(
            "[compact] Registry recompiled: {} rules loaded",
            self.rules.len()
        );
    }

    /// Loads a single rule from JSON.
    fn load_rule_json(&mut self, id: &str, json: &str) {
        let rule: CompactRule = match serde_json::from_str(json) {
            Ok(r) => r,
            Err(e) => {
                warn!("[compact] Failed to parse rule '{}': {}", id, e);
                return;
            }
        };
        let rule_id = rule.id.clone();
        match self.compile_rule(rule) {
            Ok(compiled) => {
                let idx = self.rules.len();
                self.rules.push(Arc::new(compiled));
                self.id_index.insert(rule_id, idx);
            }
            Err(e) => {
                warn!("[compact] Failed to compile rule '{}': {}", id, e);
            }
        }
    }

    /// Loads all `.json` rules from a directory.
    fn load_rules_from_dir(&mut self, dir: &Path) {
        if !dir.exists() {
            return;
        }
        let entries = match std::fs::read_dir(dir) {
            Ok(e) => e,
            Err(e) => {
                warn!("[compact] Failed to read rules dir {:?}: {}", dir, e);
                return;
            }
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            let id = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("unknown")
                .to_string();
            let json = match std::fs::read_to_string(&path) {
                Ok(j) => j,
                Err(e) => {
                    warn!("[compact] Failed to read rule file {:?}: {}", path, e);
                    continue;
                }
            };
            self.load_rule_json(&id, &json);
        }
    }

    /// Compiles a rule's regex patterns.
    fn compile_rule(&self, rule: CompactRule) -> Result<CompiledRule, String> {
        let skip_regexes = rule
            .filters
            .skip_patterns
            .iter()
            .map(|p| regex::Regex::new(p).map_err(|e| format!("skip pattern '{}': {}", p, e)))
            .collect::<Result<Vec<_>, _>>()?;
        let keep_regexes = rule
            .filters
            .keep_patterns
            .iter()
            .map(|p| regex::Regex::new(p).map_err(|e| format!("keep pattern '{}': {}", p, e)))
            .collect::<Result<Vec<_>, _>>()?;
        let heuristic_regexes = rule
            .match_criteria
            .output_heuristics
            .iter()
            .map(|p| regex::Regex::new(p).map_err(|e| format!("heuristic pattern '{}': {}", p, e)))
            .collect::<Result<Vec<_>, _>>()?;
        let counter_regexes = rule
            .counters
            .iter()
            .map(|(name, pattern)| {
                regex::Regex::new(pattern)
                    .map(|r| (name.clone(), r))
                    .map_err(|e| format!("counter '{}': {}", name, e))
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(CompiledRule {
            rule,
            skip_regexes,
            keep_regexes,
            heuristic_regexes,
            counter_regexes,
        })
    }

    /// Returns the number of registered rules.
    pub fn len(&self) -> usize {
        self.rules.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }

    /// Finds a rule by exact ID.
    pub fn get_by_id(&self, id: &str) -> Option<Arc<CompiledRule>> {
        self.id_index
            .get(id)
            .and_then(|&idx| self.rules.get(idx).cloned())
    }

    /// Returns all rules for iteration.
    pub fn all_rules(&self) -> &[Arc<CompiledRule>] {
        &self.rules
    }

    /// Loads built-in rule JSONs.
    fn load_builtin_rules() -> Vec<(RuleId, String)> {
        let mut rules = Vec::new();
        // Builtin rules are embedded at compile time
        for (id, json) in BUILTIN_RULE_JSONS {
            rules.push((id.to_string(), json.to_string()));
        }
        rules
    }
}

/// Embedded built-in rule JSON files.
static BUILTIN_RULE_JSONS: &[(&str, &str)] = &[
    // ── Version Control ──
    ("git/status", include_str!("builtin/git__status.json")),
    ("git/diff", include_str!("builtin/git__diff.json")),
    ("git/log", include_str!("builtin/git__log.json")),
    ("git/branch", include_str!("builtin/git__branch.json")),
    // ── Testing ──
    ("cargo/test", include_str!("builtin/cargo__test.json")),
    ("npm/test", include_str!("builtin/npm__test.json")),
    ("pytest", include_str!("builtin/pytest.json")),
    // ── Build ──
    ("cargo/check", include_str!("builtin/cargo__check.json")),
    ("cargo/build", include_str!("builtin/cargo__build.json")),
    ("cargo/clippy", include_str!("builtin/cargo__clippy.json")),
    // ── Package Manager ──
    ("npm/install", include_str!("builtin/npm__install.json")),
    // ── Infrastructure ──
    ("docker/ps", include_str!("builtin/docker__ps.json")),
    ("docker/build", include_str!("builtin/docker__build.json")),
    ("docker/logs", include_str!("builtin/docker__logs.json")),
    // ── Cloud ──
    ("gh/pr/list", include_str!("builtin/gh__pr_list.json")),
    // ── Filesystem ──
    ("filesystem/ls", include_str!("builtin/filesystem__ls.json")),
    (
        "filesystem/find",
        include_str!("builtin/filesystem__find.json"),
    ),
    // ── Search ──
    ("search/grep", include_str!("builtin/search__grep.json")),
    ("search/rg", include_str!("builtin/search__rg.json")),
    // ── System ──
    ("system/ps", include_str!("builtin/system__ps.json")),
    ("system/df", include_str!("builtin/system__df.json")),
    // ── Observability ──
    (
        "observability/free",
        include_str!("builtin/observability__free.json"),
    ),
    // ── Generic ──
    (
        "generic/fallback",
        include_str!("builtin/generic__fallback.json"),
    ),
];

#[cfg(test)]
