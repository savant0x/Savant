pub mod context_gatherer;
pub mod perception;
use serde::{Deserialize, Serialize};
use serde_json;
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use tracing::{debug, warn};

/// A single prior heartbeat thought, stored for continuity across pulses.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecentThought {
    pub timestamp: i64,
    pub content: String,
}

/// Protocol C-ATLAS: Proactive Session-State WAL
/// Ensures zero-latency recovery of agent decisions and preferences.
#[derive(Debug, Serialize, Deserialize, Default)]
pub struct WorkingBuffer {
    #[serde(default)]
    pub current_goal: String,
    #[serde(default)]
    pub context_summary: String,
    #[serde(default)]
    pub pending_actions: Vec<String>,
    #[serde(default)]
    pub recent_corrections: Vec<String>,
    #[serde(default)]
    pub agent_preferences: HashMap<String, String>,
    #[serde(default)]
    pub last_pulse_hash: Option<u64>,
    #[serde(default)]
    pub current_lens_index: usize,
    #[serde(default)]
    pub last_reflection_hashes: Vec<u64>,
    #[serde(default)]
    pub ald_watermark: u64,
    #[serde(default)]
    pub recent_thoughts: Vec<RecentThought>,
    #[serde(default)]
    pub last_reflection_time: Option<i64>,
    #[serde(default)]
    pub pending_mutations: Vec<String>,
    #[serde(default)]
    pub current_personality_o: f32,
    #[serde(default)]
    pub current_personality_c: f32,
    #[serde(default)]
    pub current_personality_e: f32,
    #[serde(default)]
    pub current_personality_a: f32,
    #[serde(default)]
    pub current_personality_n: f32,
    #[serde(default)]
    pub total_pulses: u64,
    #[serde(default)]
    pub total_learnings: u64,
    #[serde(default)]
    pub self_modification_count: u64,
    #[serde(default)]
    pub schema_version: u32,
}

/// YAML-frontmatter machine-parseable fields.
#[derive(Debug, Serialize, Deserialize)]
struct FrontmatterData {
    #[serde(default)]
    schema_version: u32,
    #[serde(default)]
    current_goal: String,
    #[serde(default)]
    current_lens_index: usize,
    #[serde(default)]
    last_pulse_hash: Option<u64>,
    #[serde(default)]
    ald_watermark: u64,
    #[serde(default)]
    last_reflection_time: Option<i64>,
    #[serde(default)]
    personality: PersonalityBlock,
    #[serde(default)]
    growth: GrowthBlock,
    #[serde(default)]
    recent_reflection_hashes: Vec<u64>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct PersonalityBlock {
    #[serde(default)]
    o: f32,
    #[serde(default)]
    c: f32,
    #[serde(default)]
    e: f32,
    #[serde(default)]
    a: f32,
    #[serde(default)]
    n: f32,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct GrowthBlock {
    #[serde(default)]
    total_pulses: u64,
    #[serde(default)]
    total_learnings: u64,
    #[serde(default)]
    self_modification_count: u64,
}

pub struct ProactivePartner {
    state_path: PathBuf,
    context_path: PathBuf,
}

impl ProactivePartner {
    pub fn new(root: PathBuf, config: &savant_core::config::ProactiveConfig) -> Self {
        Self {
            state_path: root.join(&config.session_state_file),
            context_path: root.join(&config.workspace_context_file),
        }
    }

    /// Commit the current working buffer to the sovereign WAL.
    /// Produces YAML frontmatter for machine-parseable fields and structured
    /// markdown sections for human-readable content.
    pub fn commit_state(&self, buffer: &WorkingBuffer) -> Result<(), Box<dyn std::error::Error>> {
        let frontmatter = FrontmatterData {
            schema_version: 1,
            current_goal: buffer.current_goal.clone(),
            current_lens_index: buffer.current_lens_index,
            last_pulse_hash: buffer.last_pulse_hash,
            ald_watermark: buffer.ald_watermark,
            last_reflection_time: buffer.last_reflection_time,
            personality: PersonalityBlock {
                o: buffer.current_personality_o,
                c: buffer.current_personality_c,
                e: buffer.current_personality_e,
                a: buffer.current_personality_a,
                n: buffer.current_personality_n,
            },
            growth: GrowthBlock {
                total_pulses: buffer.total_pulses,
                total_learnings: buffer.total_learnings,
                self_modification_count: buffer.self_modification_count,
            },
            recent_reflection_hashes: buffer.last_reflection_hashes.clone(),
        };

        let yaml = serde_yaml::to_string(&frontmatter)?;

        let now = chrono::Utc::now().to_rfc3339();
        let mut md = String::new();

        md.push_str("---\n");
        md.push_str(yaml.trim_end());
        md.push_str("\n---\n\n");
        md.push_str("# Sovereign Session State (WAL)\n\n");
        md.push_str(&format!(
            "> **Last Pulse:** {}\n> **Schema:** v{}\n> **Pulse Count:** {}\n\n",
            now, buffer.schema_version, buffer.total_pulses
        ));
        md.push_str("---\n\n");

        // Current Goal
        md.push_str("## Current Goal\n\n");
        md.push_str(&format!("{}\n\n", buffer.current_goal));
        md.push_str("---\n\n");

        // Context Summary
        md.push_str("## Context Summary\n\n");
        if buffer.context_summary.is_empty() {
            md.push_str("(empty)\n\n");
        } else {
            md.push_str(&format!("{}\n\n", buffer.context_summary));
        }
        md.push_str("---\n\n");

        // Recent Thoughts
        md.push_str("## Recent Thoughts\n\n");
        if buffer.recent_thoughts.is_empty() {
            md.push_str("(none)\n\n");
        } else {
            md.push_str("| Time | Content |\n");
            md.push_str("|------|---------|\n");
            for rt in &buffer.recent_thoughts {
                let time_str = chrono::DateTime::from_timestamp(rt.timestamp, 0).map_or_else(
                    || format!("t:{}", rt.timestamp),
                    |dt| dt.format("%Y-%m-%d %H:%M:%S UTC").to_string(),
                );
                let content = rt.content.replace('\n', " ").replace('|', "\\|");
                md.push_str(&format!("| {} | {} |\n", time_str, content));
            }
            md.push('\n');
        }
        md.push_str("---\n\n");

        // Pending Actions
        md.push_str("## Pending Actions\n\n");
        if buffer.pending_actions.is_empty() {
            md.push_str("(none)\n\n");
        } else {
            for (i, action) in buffer.pending_actions.iter().enumerate() {
                md.push_str(&format!("{}. {}\n", i + 1, action));
            }
            md.push('\n');
        }
        md.push_str("---\n\n");

        // Recent Corrections
        md.push_str("## Recent Corrections\n\n");
        if buffer.recent_corrections.is_empty() {
            md.push_str("(none)\n\n");
        } else {
            for correction in &buffer.recent_corrections {
                md.push_str(&format!("- {}\n", correction));
            }
            md.push('\n');
        }
        md.push_str("---\n\n");

        // Agent Preferences
        md.push_str("## Agent Preferences\n\n");
        if buffer.agent_preferences.is_empty() {
            md.push_str("(none)\n\n");
        } else {
            md.push_str("| Key | Value |\n");
            md.push_str("|-----|-------|\n");
            for (k, v) in &buffer.agent_preferences {
                md.push_str(&format!("| {} | {} |\n", k, v));
            }
            md.push('\n');
        }
        md.push_str("---\n\n");

        // Pending Mutations
        md.push_str("## Pending Mutations\n\n");
        if buffer.pending_mutations.is_empty() {
            md.push_str("(none)\n\n");
        } else {
            for mutation in &buffer.pending_mutations {
                md.push_str(&format!("- {}\n", mutation));
            }
            md.push('\n');
        }

        fs::write(&self.state_path, md)?;
        debug!("C-ATLAS: Session-State committed to WAL (frontmatter v1).");
        Ok(())
    }

    /// Materialize the working buffer from the WAL.
    /// Supports both the new frontmatter+markdown format (v1) and the legacy
    /// JSON format (v0). Detection: new format has YAML key-value pairs after
    /// the opening `---`; legacy format has a `#` heading.
    pub fn restore_state(&self) -> Result<WorkingBuffer, Box<dyn std::error::Error>> {
        if !self.state_path.exists() {
            return Ok(WorkingBuffer::default());
        }
        let content = fs::read_to_string(&self.state_path)?;

        if Self::is_frontmatter_format(&content) {
            self.parse_frontmatter_wal(&content)
        } else if content.contains('{') {
            warn!("C-ATLAS: Legacy JSON WAL detected. Consider re-saving to migrate to frontmatter format.");
            self.parse_legacy_json_wal(&content)
        } else {
            Err("WAL: unrecognized format".into())
        }
    }

    /// Detect frontmatter format by checking if the first non-empty line after
    /// the opening `---` is a YAML key-value pair (contains `:`) rather than
    /// a markdown heading (`#`).
    fn is_frontmatter_format(content: &str) -> bool {
        let trimmed = content.trim_start();
        if !trimmed.starts_with("---") {
            return false;
        }
        let after_delim = &trimmed[3..];
        let second_line = after_delim
            .lines()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("")
            .trim();
        second_line.contains(':') && !second_line.starts_with('#')
    }

    /// Parse the new frontmatter + markdown WAL format.
    fn parse_frontmatter_wal(
        &self,
        content: &str,
    ) -> Result<WorkingBuffer, Box<dyn std::error::Error>> {
        let stripped = content.trim_start();
        if !stripped.starts_with("---") {
            return Err("WAL missing frontmatter opening delimiter".into());
        }

        let after_first = &stripped[3..];
        let second_delim = after_first
            .find("\n---")
            .ok_or("WAL missing frontmatter closing delimiter")?;
        let yaml_str = &after_first[..second_delim];
        let body = &after_first[second_delim + 4..];

        let fm: FrontmatterData = serde_yaml::from_str(yaml_str)
            .map_err(|e| format!("WAL frontmatter parse error: {}", e))?;

        let mut buffer = WorkingBuffer {
            current_goal: fm.current_goal,
            current_lens_index: fm.current_lens_index,
            last_pulse_hash: fm.last_pulse_hash,
            ald_watermark: fm.ald_watermark,
            last_reflection_time: fm.last_reflection_time,
            current_personality_o: fm.personality.o,
            current_personality_c: fm.personality.c,
            current_personality_e: fm.personality.e,
            current_personality_a: fm.personality.a,
            current_personality_n: fm.personality.n,
            total_pulses: fm.growth.total_pulses,
            total_learnings: fm.growth.total_learnings,
            self_modification_count: fm.growth.self_modification_count,
            last_reflection_hashes: fm.recent_reflection_hashes,
            schema_version: fm.schema_version,
            ..Default::default()
        };

        // Parse markdown sections by heading
        for (heading, section_body) in Self::extract_md_sections(body) {
            match heading.as_str() {
                "Context Summary" => {
                    let trimmed = section_body.trim();
                    if trimmed != "(empty)" {
                        buffer.context_summary = trimmed.to_string();
                    }
                }
                "Recent Thoughts" => {
                    buffer.recent_thoughts = Self::parse_thoughts_table(section_body.trim());
                }
                "Pending Actions" => {
                    buffer.pending_actions = Self::parse_numbered_list(section_body.trim());
                }
                "Recent Corrections" => {
                    buffer.recent_corrections = Self::parse_bullet_list(section_body.trim());
                }
                "Agent Preferences" => {
                    buffer.agent_preferences = Self::parse_kv_table(section_body.trim());
                }
                "Pending Mutations" => {
                    buffer.pending_mutations = Self::parse_bullet_list(section_body.trim());
                }
                _ => {}
            }
        }

        Ok(buffer)
    }

    /// Parse the legacy JSON WAL format (v0).
    fn parse_legacy_json_wal(
        &self,
        content: &str,
    ) -> Result<WorkingBuffer, Box<dyn std::error::Error>> {
        let json_start = content
            .find('{')
            .ok_or("Invalid WAL: No JSON start found")?;
        let json_end = content.rfind('}').ok_or("Invalid WAL: No JSON end found")?;
        let json_str = &content[json_start..=json_end];
        let mut buffer: WorkingBuffer = serde_json::from_str(json_str)?;
        buffer.schema_version = 0;
        Ok(buffer)
    }

    /// Split markdown body by `## ` headings into (heading_text, body) pairs.
    /// Strips trailing `---` separator lines from each section body.
    fn extract_md_sections(body: &str) -> Vec<(String, String)> {
        let mut sections = Vec::new();
        let mut current_heading: Option<String> = None;
        let mut current_body = String::new();

        for line in body.lines() {
            if let Some(rest) = line.strip_prefix("## ") {
                if let Some(h) = current_heading.take() {
                    sections.push((h, Self::strip_trailing_separators(&current_body)));
                    current_body.clear();
                }
                current_heading = Some(rest.trim().to_string());
            } else if current_heading.is_some() {
                current_body.push_str(line);
                current_body.push('\n');
            }
        }
        if let Some(h) = current_heading {
            sections.push((h, Self::strip_trailing_separators(&current_body)));
        }

        sections
    }

    /// Remove trailing `---` lines and whitespace from section content.
    fn strip_trailing_separators(s: &str) -> String {
        let trimmed = s.trim_end();
        trimmed.trim_end_matches("---").trim_end().to_string()
    }

    /// Parse a markdown table with `Time | Content` columns into `RecentThought` vec.
    fn parse_thoughts_table(text: &str) -> Vec<RecentThought> {
        let mut thoughts = Vec::new();
        for line in text.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with('|')
                && !trimmed.starts_with("| Time")
                && !trimmed.starts_with("|---")
            {
                let parts: Vec<&str> = trimmed.splitn(3, '|').collect();
                if parts.len() >= 3 {
                    let time_str = parts[1].trim();
                    let content = parts[2].trim();
                    // Remove trailing table delimiter
                    let content = content.trim_end_matches('|').trim().replace("\\|", "|");
                    let timestamp = Self::parse_timestamp(time_str).unwrap_or(0);
                    thoughts.push(RecentThought { timestamp, content });
                }
            }
        }
        thoughts
    }

    /// Parse a numbered list (`1. item`) into a `Vec<String>`.
    fn parse_numbered_list(text: &str) -> Vec<String> {
        text.lines()
            .filter_map(|line| {
                let trimmed = line.trim();
                if let Some(pos) = trimmed.find(". ") {
                    if trimmed[..pos].chars().all(|c| c.is_ascii_digit()) {
                        return Some(trimmed[pos + 2..].to_string());
                    }
                }
                None
            })
            .collect()
    }

    /// Parse a bullet list (`- item`) into a `Vec<String>`.
    fn parse_bullet_list(text: &str) -> Vec<String> {
        text.lines()
            .filter_map(|line| {
                let trimmed = line.trim();
                trimmed.strip_prefix("- ").map(|s| s.to_string())
            })
            .collect()
    }

    /// Parse a key-value table (`| key | value |`) into a `HashMap`.
    fn parse_kv_table(text: &str) -> HashMap<String, String> {
        let mut map = HashMap::new();
        for line in text.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with('|')
                && !trimmed.starts_with("| Key")
                && !trimmed.starts_with("|---")
            {
                let parts: Vec<&str> = trimmed.splitn(3, '|').collect();
                if parts.len() >= 3 {
                    let key = parts[1].trim().to_string();
                    let value = parts[2].trim().trim_end_matches('|').trim().to_string();
                    if !key.is_empty() {
                        map.insert(key, value);
                    }
                }
            }
        }
        map
    }

    /// Parse a human-readable timestamp string back to unix seconds.
    /// Accepts `YYYY-MM-DD HH:MM:SS UTC` or `t:NNNNN` format.
    fn parse_timestamp(s: &str) -> Option<i64> {
        if let Some(t_str) = s.strip_prefix("t:") {
            return t_str.parse().ok();
        }
        chrono::NaiveDateTime::parse_from_str(s.trim_end_matches(" UTC"), "%Y-%m-%d %H:%M:%S")
            .ok()
            .and_then(|ndt| ndt.and_utc().timestamp_millis().checked_div(1000))
    }

    /// OMEGA-VIII: Distill raw cognition into Layer 2 Workspace Context.
    pub fn distill_context(&self, summary: &str) -> Result<(), Box<dyn std::error::Error>> {
        let formatted = format!(
            "# Sovereign Workspace Context (Layer 2)\n\n\
             > **Last Distillation:** {}\n\n\
             ## Current Knowledge Synthesis\n\n{}\n\n\
             ---\n\n\
             *Autonomous distillation performed by Savant Pulse.*\n",
            chrono::Utc::now().to_rfc3339(),
            summary
        );
        fs::write(&self.context_path, formatted)?;
        Ok(())
    }
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn sample_buffer() -> WorkingBuffer {
        let mut prefs = HashMap::new();
        prefs.insert("communication_style".to_string(), "direct".to_string());
        WorkingBuffer {
            current_goal: "Test Goal".to_string(),
            context_summary: "Test context with } braces { and special chars".to_string(),
            pending_actions: vec!["Action 1".to_string(), "Action 2".to_string()],
            recent_corrections: vec!["Fixed X".to_string()],
            agent_preferences: prefs,
            last_pulse_hash: Some(12345),
            current_lens_index: 3,
            last_reflection_hashes: vec![111, 222],
            ald_watermark: 42,
            recent_thoughts: vec![RecentThought {
                timestamp: 1717036800,
                content: "Thought about | pipes and } braces".to_string(),
            }],
            last_reflection_time: Some(1717036800),
            pending_mutations: vec!["SOUL.md section 3: update".to_string()],
            current_personality_o: 0.72,
            current_personality_c: 0.65,
            current_personality_e: 0.58,
            current_personality_a: 0.81,
            current_personality_n: 0.33,
            total_pulses: 142,
            total_learnings: 87,
            self_modification_count: 3,
            schema_version: 1,
        }
    }

    #[test]
    fn test_commit_state_produces_frontmatter() {
        let dir = tempfile::tempdir().unwrap();
        let state_path = dir.path().join("TEST-STATE.md");
        let partner = ProactivePartner {
            state_path: state_path.clone(),
            context_path: dir.path().join("CONTEXT.md"),
        };

        let buffer = sample_buffer();
        partner.commit_state(&buffer).unwrap();

        let content = std::fs::read_to_string(&state_path).unwrap();
        assert!(
            content.starts_with("---"),
            "Should start with frontmatter delimiter"
        );
        assert!(
            content.contains("schema_version: 1"),
            "Should contain schema version"
        );
        assert!(
            content.contains("## Current Goal"),
            "Should contain heading"
        );
        assert!(
            content.contains("## Context Summary"),
            "Should contain context heading"
        );
    }

    #[test]
    fn test_restore_state_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let state_path = dir.path().join("TEST-STATE.md");
        let partner = ProactivePartner {
            state_path: state_path.clone(),
            context_path: dir.path().join("CONTEXT.md"),
        };

        let original = sample_buffer();
        partner.commit_state(&original).unwrap();
        let restored = partner.restore_state().unwrap();

        assert_eq!(restored.current_goal, original.current_goal);
        assert_eq!(restored.context_summary, original.context_summary);
        assert_eq!(restored.pending_actions, original.pending_actions);
        assert_eq!(restored.recent_corrections, original.recent_corrections);
        assert_eq!(restored.last_pulse_hash, original.last_pulse_hash);
        assert_eq!(restored.current_lens_index, original.current_lens_index);
        assert_eq!(
            restored.last_reflection_hashes,
            original.last_reflection_hashes
        );
        assert_eq!(restored.ald_watermark, original.ald_watermark);
        assert_eq!(restored.pending_mutations, original.pending_mutations);
        assert_eq!(
            restored.current_personality_o,
            original.current_personality_o
        );
        assert_eq!(restored.total_pulses, original.total_pulses);
        assert_eq!(restored.schema_version, 1);
        assert_eq!(restored.recent_thoughts.len(), 1);
        assert_eq!(
            restored.recent_thoughts[0].content,
            original.recent_thoughts[0].content
        );
        assert_eq!(restored.agent_preferences, original.agent_preferences);
    }

    #[test]
    fn test_restore_state_legacy_json() {
        let dir = tempfile::tempdir().unwrap();
        let state_path = dir.path().join("TEST-STATE.md");
        let partner = ProactivePartner {
            state_path: state_path.clone(),
            context_path: dir.path().join("CONTEXT.md"),
        };

        let original = sample_buffer();
        let json = serde_json::to_string(&original).unwrap();
        let legacy = format!("--- \n# Sovereign Session State (WAL)\n---\n\n{}", json);
        std::fs::write(&state_path, legacy).unwrap();

        let restored = partner.restore_state().unwrap();
        assert_eq!(restored.current_goal, original.current_goal);
        assert_eq!(restored.context_summary, original.context_summary);
        assert_eq!(restored.schema_version, 0);
    }

    #[test]
    fn test_restore_state_context_with_braces() {
        let dir = tempfile::tempdir().unwrap();
        let state_path = dir.path().join("TEST-STATE.md");
        let partner = ProactivePartner {
            state_path: state_path.clone(),
            context_path: dir.path().join("CONTEXT.md"),
        };

        let mut buffer = sample_buffer();
        buffer.context_summary = "The system crashed. } end { middle } more".to_string();
        partner.commit_state(&buffer).unwrap();
        let restored = partner.restore_state().unwrap();
        assert_eq!(restored.context_summary, buffer.context_summary);
    }

    #[test]
    fn test_restore_state_missing_file() {
        let dir = tempfile::tempdir().unwrap();
        let partner = ProactivePartner {
            state_path: dir.path().join("NONEXISTENT.md"),
            context_path: dir.path().join("CONTEXT.md"),
        };

        let restored = partner.restore_state().unwrap();
        assert_eq!(restored.current_goal, "");
        assert_eq!(restored.schema_version, 0);
    }

    #[test]
    fn test_schema_version_default() {
        let json = r#"{"current_goal":"test"}"#;
        let buffer: WorkingBuffer = serde_json::from_str(json).unwrap();
        assert_eq!(buffer.schema_version, 0);
    }
}
