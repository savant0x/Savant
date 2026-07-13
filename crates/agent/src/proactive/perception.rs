//! OMEGA-VIII: Perception Engine (Context Hydration)
//!
//! Provides high-fidelity awareness of environment variance
//! (Git changes, FS activity, real system metrics) to the proactive heartbeat.
//!
//! All metrics are sourced from deterministic sysinfo crate — the agent
//! cannot hallucinate system state because it receives exact numbers.

use std::path::Path;
use std::process::Command;
use sysinfo::{CpuRefreshKind, MemoryRefreshKind, RefreshKind, System};

/// Perception configuration with tunable thresholds.
pub struct PerceptionConfig {
    /// How many seconds back to check for file modifications.
    pub fs_activity_window_secs: u64,
    /// How many files to list at most in activity report.
    pub max_activity_entries: usize,
}

impl Default for PerceptionConfig {
    fn default() -> Self {
        Self {
            fs_activity_window_secs: 60,
            max_activity_entries: 20,
        }
    }
}

/// Perception engine with configurable thresholds.
pub struct PerceptionEngine {
    config: PerceptionConfig,
}

impl PerceptionEngine {
    pub fn new(config: PerceptionConfig) -> Self {
        Self { config }
    }

    pub fn default_engine() -> Self {
        Self::new(PerceptionConfig::default())
    }

    /// Captures a high-level summary of Git changes in the workspace.
    pub fn get_git_status(path: &Path) -> String {
        let output = Command::new("git")
            .args(["status", "--short"])
            .current_dir(path)
            .output();

        match output {
            Ok(out) if out.status.success() => {
                let s = String::from_utf8_lossy(&out.stdout).to_string();
                if s.is_empty() {
                    "No pending git changes.".to_string()
                } else {
                    format!("Git Status:\n{}", s)
                }
            }
            _ => "Git status unavailable.".to_string(),
        }
    }

    /// Captures a brief diff of the most recent changes.
    pub fn get_git_diff(path: &Path) -> String {
        let output = Command::new("git")
            .args(["diff", "--stat"])
            .current_dir(path)
            .output();

        match output {
            Ok(out) if out.status.success() => {
                let s = String::from_utf8_lossy(&out.stdout).to_string();
                if s.is_empty() {
                    "".to_string()
                } else {
                    format!("Git Diff Summary:\n{}", s)
                }
            }
            _ => "".to_string(),
        }
    }

    /// Checks for recent file system activity within the configured time window.
    pub fn get_fs_activity(&self, path: &Path) -> String {
        let window_secs = self.config.fs_activity_window_secs;
        let max_entries = self.config.max_activity_entries;
        let mut activity = Vec::new();

        if let Ok(entries) = std::fs::read_dir(path) {
            for entry in entries.flatten() {
                if activity.len() >= max_entries {
                    break;
                }
                if let Ok(metadata) = entry.metadata() {
                    if let Ok(modified) = metadata.modified() {
                        if let Ok(elapsed) = modified.elapsed() {
                            if elapsed.as_secs() < window_secs {
                                activity.push(format!(
                                    "- {} (modified {}s ago)",
                                    entry.file_name().to_string_lossy(),
                                    elapsed.as_secs()
                                ));
                            }
                        }
                    }
                }
            }
        }

        if activity.is_empty() {
            format!("No recent FS activity in last {}s.", window_secs)
        } else {
            format!(
                "Recent FS Activity (last {}s):\n{}",
                window_secs,
                activity.join("\n")
            )
        }
    }

    /// Deterministic substrate metrics via sysinfo crate.
    /// Returns exact memory and CPU values — the agent cannot hallucinate.
    /// Uses block_in_place to avoid blocking the tokio runtime.
    pub fn get_substrate_metrics() -> String {
        tokio::task::block_in_place(|| {
            let mut sys = System::new_with_specifics(
                RefreshKind::nothing()
                    .with_memory(MemoryRefreshKind::everything())
                    .with_cpu(CpuRefreshKind::everything()),
            );

            // Refresh CPU to get accurate usage (needs a small interval between refreshes).
            // Use std::thread::sleep inside block_in_place since we're already in a blocking context.
            sys.refresh_cpu_all();
            std::thread::sleep(std::time::Duration::from_millis(200));
            sys.refresh_cpu_all();
            let total_mem = sys.total_memory();
            let used_mem = sys.used_memory();
            let mem_pct = if total_mem > 0 {
                (used_mem as f64 / total_mem as f64) * 100.0
            } else {
                0.0
            };

            let cpu_usage = sys.global_cpu_usage();

            let total_mb = total_mem / (1024 * 1024);
            let used_mb = used_mem / (1024 * 1024);

            format!(
                "Substrate Metrics (deterministic):\n\
                - Memory: {}MB / {}MB ({:.1}%)\n\
                - CPU: {:.1}%",
                used_mb, total_mb, mem_pct, cpu_usage
            )
        })
    }
}
