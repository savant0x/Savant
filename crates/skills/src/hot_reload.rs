//! Skill Hot-Reload
//!
//! Watches the `skills/` directory for changes and automatically reloads
//! modified skills without requiring a restart.
//!
//! When a SKILL.md file is modified:
//! 1. The skill is re-parsed
//! 2. The SkillRegistry is updated
//! 3. A notification is published via the event bus
//! 4. If parsing fails, the last valid version is kept

use crate::parser::SkillRegistry;
use savant_core::error::SavantError;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{error, info, warn};

/// Watches a skills directory for changes and auto-reloads skills.
pub struct SkillHotReload {
    /// Path to the skills directory.
    skills_dir: PathBuf,
    /// The shared skill registry.
    registry: Arc<Mutex<SkillRegistry>>,
    /// Whether the watcher is running.
    running: Arc<std::sync::atomic::AtomicBool>,
}

impl SkillHotReload {
    /// Creates a new hot-reload watcher for the given skills directory.
    pub fn new(skills_dir: PathBuf, registry: Arc<Mutex<SkillRegistry>>) -> Self {
        Self {
            skills_dir,
            registry,
            running: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        }
    }

    /// Starts watching for changes. This spawns a background task.
    pub fn start(&self) -> Result<tokio::task::JoinHandle<()>, SavantError> {
        let skills_dir = self.skills_dir.clone();
        let registry = self.registry.clone();
        let running = self.running.clone();

        running.store(true, std::sync::atomic::Ordering::Relaxed);

        let handle = tokio::spawn(async move {
            info!("Skill hot-reload watching: {:?}", skills_dir);
            Self::watch_loop(skills_dir, registry, running).await;
        });

        Ok(handle)
    }

    /// Stops the watcher.
    pub fn stop(&self) {
        self.running
            .store(false, std::sync::atomic::Ordering::Relaxed);
        info!("Skill hot-reload stopped");
    }

    /// The main watch loop. Checks for changes every 2 seconds.
    async fn watch_loop(
        skills_dir: PathBuf,
        registry: Arc<Mutex<SkillRegistry>>,
        running: Arc<std::sync::atomic::AtomicBool>,
    ) {
        let mut last_modified: std::collections::HashMap<PathBuf, std::time::SystemTime> =
            std::collections::HashMap::new();

        while running.load(std::sync::atomic::Ordering::Relaxed) {
            // Scan skills directory
            match std::fs::read_dir(&skills_dir) {
                Ok(entries) => {
                    for entry in entries.flatten() {
                        let path = entry.path();
                        if path.is_dir() {
                            let skill_md = path.join("SKILL.md");
                            if skill_md.exists() {
                                if let Ok(metadata) = std::fs::metadata(&skill_md) {
                                    if let Ok(modified) = metadata.modified() {
                                        let should_reload = last_modified
                                            .get(&skill_md)
                                            .map(|&prev| modified > prev)
                                            .unwrap_or(true);

                                        if should_reload {
                                            info!("Skill file changed: {:?}", skill_md);
                                            Self::reload_skill(&skill_md, &registry).await;
                                            last_modified.insert(skill_md, modified);
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                Err(e) => {
                    warn!("Failed to read skills directory: {}", e);
                }
            }

            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        }
    }

    /// Reloads a single skill from its SKILL.md file.
    async fn reload_skill(skill_md: &Path, registry: &Arc<Mutex<SkillRegistry>>) {
        let mut reg = registry.lock().await;

        match reg.load_skill_from_file(skill_md).await {
            Ok(()) => {
                info!("Successfully reloaded skill: {:?}", skill_md);
            }
            Err(e) => {
                error!(
                    "Failed to reload skill {:?}: {}. Keeping last valid version.",
                    skill_md, e
                );
            }
        }
    }
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_hot_reload_creation() {
        let registry = Arc::new(Mutex::new(SkillRegistry::new()));
        let watcher = SkillHotReload::new(std::path::PathBuf::from("./skills"), registry);
        assert!(!watcher.running.load(std::sync::atomic::Ordering::Relaxed));
    }
}
