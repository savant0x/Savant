use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProvenanceEntry {
    pub name: String,
    #[serde(default)]
    pub creator_agent_id: String,
    #[serde(default)]
    pub creator_agent_name: String,
    pub action: String,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub category: Option<String>,
    #[serde(default)]
    pub rating: Option<String>,
    #[serde(default)]
    pub rating_agent: Option<String>,
    #[serde(default)]
    pub comment: Option<String>,
    #[serde(default)]
    pub pinned: Option<bool>,
    #[serde(default)]
    pub reason: Option<String>,
    #[serde(default)]
    pub superseded_by: Option<String>,
    #[serde(default)]
    pub audit_result: Option<String>,
    #[serde(default)]
    pub audit_iterations: Option<u32>,
    #[serde(default)]
    pub audit_findings: Option<Vec<String>>,
    #[serde(default)]
    pub from_version: Option<String>,
    #[serde(default)]
    pub to_version: Option<String>,
    pub timestamp: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolStats {
    pub use_count: u32,
    pub unique_agents: u32,
    pub thumbs_up: u32,
    pub thumbs_down: u32,
    pub last_used_at: Option<DateTime<Utc>>,
}

impl ToolStats {
    pub fn success_rate(&self) -> f32 {
        let total = self.thumbs_up + self.thumbs_down;
        if total == 0 {
            return 1.0;
        }
        self.thumbs_up as f32 / total as f32
    }
}

pub struct ProvenanceTracker {
    path: PathBuf,
}

impl ProvenanceTracker {
    pub fn new(log_path: &Path) -> Result<Self, std::io::Error> {
        if let Some(parent) = log_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        // Touch the file to ensure it exists
        OpenOptions::new()
            .create(true)
            .append(true)
            .open(log_path)?;
        Ok(ProvenanceTracker {
            path: log_path.to_path_buf(),
        })
    }

    /// RC-18: Async append using spawn_blocking to avoid blocking the tokio worker.
    pub async fn append(&self, entry: &ProvenanceEntry) {
        let line = serde_json::to_string(entry).unwrap_or_default();
        let path = self.path.clone();
        if let Err(e) = tokio::task::spawn_blocking(move || {
            if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(&path) {
                if let Err(write_err) = writeln!(file, "{line}") {
                    tracing::warn!("[provenance] failed to write entry: {}", write_err);
                }
            }
        })
        .await
        {
            tracing::warn!("[provenance] spawn_blocking failed: {}", e);
        }
    }

    pub fn replay(&self) -> Vec<ProvenanceEntry> {
        match File::open(&self.path) {
            Ok(f) => {
                let reader = BufReader::new(f);
                reader
                    .lines()
                    .filter_map(|line| {
                        line.ok()
                            .and_then(|l| serde_json::from_str::<ProvenanceEntry>(&l).ok())
                    })
                    .collect()
            }
            Err(_) => Vec::new(),
        }
    }

    pub fn compute_stats(&self, name: &str) -> ToolStats {
        let entries = self.replay();
        let mut use_count = 0u32;
        let mut thumbs_up = 0u32;
        let mut thumbs_down = 0u32;
        let mut unique_agents = std::collections::HashSet::new();
        let mut last_used_at = None;

        for entry in &entries {
            if entry.name != name {
                continue;
            }
            if entry.action.as_str() == "rate" {
                use_count += 1;
                if let Some(ref agent) = entry.rating_agent {
                    unique_agents.insert(agent.clone());
                }
                match entry.rating.as_deref() {
                    Some("thumbs_up") => thumbs_up += 1,
                    Some("thumbs_down") => thumbs_down += 1,
                    _ => {}
                }
            }
            if let Ok(ts) = DateTime::parse_from_rfc3339(&entry.timestamp) {
                last_used_at = Some(ts.with_timezone(&Utc));
            }
        }

        ToolStats {
            use_count,
            unique_agents: unique_agents.len() as u32,
            thumbs_up,
            thumbs_down,
            last_used_at,
        }
    }
}
