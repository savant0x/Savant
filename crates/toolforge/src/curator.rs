use std::sync::Arc;
use tracing::info;

use crate::provenance::ProvenanceTracker;
use crate::registry::SharedToolRegistry;

pub struct CollectiveCurator {
    registry: Arc<SharedToolRegistry>,
    provenance: Arc<ProvenanceTracker>,
    /// Number of days of inactivity before a tool is auto-archived (default: 30)
    inactivity_threshold_days: u64,
}

impl CollectiveCurator {
    pub fn new(registry: Arc<SharedToolRegistry>, provenance: Arc<ProvenanceTracker>) -> Self {
        CollectiveCurator {
            registry,
            provenance,
            inactivity_threshold_days: 30,
        }
    }

    /// Sets a custom inactivity threshold in days.
    pub fn with_inactivity_threshold(mut self, days: u64) -> Self {
        self.inactivity_threshold_days = days;
        self
    }

    pub async fn run_auto_transitions(&self) {
        info!("[toolforge::curator] Running auto-transition scan");
        let entries = self.provenance.replay();
        let now = chrono::Utc::now();

        let mut tool_last_action: std::collections::HashMap<
            String,
            (String, chrono::DateTime<chrono::Utc>),
        > = std::collections::HashMap::new();
        let mut tool_pinned: std::collections::HashSet<String> = std::collections::HashSet::new();

        for entry in &entries {
            if let Ok(ts) = chrono::DateTime::parse_from_rfc3339(&entry.timestamp) {
                let ts_utc = ts.with_timezone(&chrono::Utc);
                tool_last_action.insert(entry.name.clone(), (entry.action.clone(), ts_utc));
            }
            if entry.action == "pin" && entry.pinned == Some(true) {
                tool_pinned.insert(entry.name.clone());
            }
        }

        for (name, (action, last_ts)) in &tool_last_action {
            if tool_pinned.contains(name) {
                continue;
            }
            if action == "archive" {
                continue;
            }
            let days_inactive = (now - *last_ts).num_days();
            if days_inactive > self.inactivity_threshold_days as i64 {
                self.registry.remove(name);
                info!("[toolforge::curator] Auto-archived stale tool: {name} ({days_inactive} days inactive)");
            }
        }
    }
}
