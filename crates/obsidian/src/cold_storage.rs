use std::collections::HashSet;
use std::fs;
use std::path::Path;

use chrono::{Duration, NaiveDate, Utc};
use tracing::{debug, info, warn};

use crate::config::ObsidianConfig;
use crate::error::VaultError;
use crate::writer::VaultWriter;

/// Manages the vault file ceiling and cold storage migration.
///
/// Obsidian becomes sluggish past ~15K files and Graph View crashes at ~100K.
/// This manager enforces a configurable ceiling by migrating eligible episodic
/// content older than `cold_storage_days` out of the vault and into db_only
/// status (retained immutably in the LSM, invisible in Obsidian).
pub struct ColdStorageManager {
    vault_path: std::path::PathBuf,
    config: ObsidianConfig,
}

impl ColdStorageManager {
    pub fn new(vault_path: std::path::PathBuf, config: ObsidianConfig) -> Self {
        Self { vault_path, config }
    }

    /// Runs the cold storage check: removes eligible files and enforces ceiling.
    ///
    /// Uses the provided `writer` to ensure vault structure exists before
    /// writing tombstones, so that the `.stale/` directory is present.
    pub async fn run(&self, writer: &VaultWriter) -> Result<(), VaultError> {
        // Ensure vault structure (including .stale/ directory) exists
        writer.ensure_structure().await?;

        let max = self.config.max_files.max(100);
        let cold_days = self.config.cold_storage_days;
        let db_only_dirs: HashSet<String> = self
            .config
            .db_only_dirs
            .iter()
            .map(|d| d.trim_end_matches('/').to_string())
            .collect();

        // Phase 1: count current files
        let vault_path = self.vault_path.clone();
        let current_count = tokio::task::spawn_blocking(move || count_all_md(&vault_path))
            .await
            .map_err(|e| VaultError::Config(e.to_string()))?;
        debug!(
            "[obsidian] Cold storage: {current_count} files (max {max}, cold after {cold_days}d)"
        );

        // Phase 2: remove episodic files older than cold_storage_days
        let cutoff = Utc::now().date_naive() - chrono::Duration::days(cold_days as i64);

        let episodic_dir = self.vault_path.join("Episodic");
        let stale_dir = self.vault_path.join(".stale");
        tokio::task::spawn_blocking(move || {
            if let Ok(entries) = fs::read_dir(&episodic_dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if !path.is_file() || path.extension().is_none_or(|e| e != "md") {
                        continue;
                    }

                    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
                    if let Ok(date) = NaiveDate::parse_from_str(stem, "%Y-%m-%d") {
                        if date < cutoff {
                            let tombstone = stale_dir.join(format!("{stem}.tombstone"));
                            if !tombstone.exists() {
                                if let Ok(mut f) = fs::File::create(&tombstone) {
                                    use std::io::Write;
                                    if let Err(e) = f.write_all(
                                        format!(
                                            "Cold storage — removed {date} (>{cold_days}d old).\n\
                                             Retained in LSM+HNSW.\n"
                                        )
                                        .as_bytes(),
                                    ) {
                                        warn!("[obsidian] Failed to write tombstone: {}", e);
                                    }
                                }
                            }
                            if let Err(e) = fs::remove_file(&path) {
                                warn!("[obsidian] Failed to remove stale file {stem}: {e}");
                            } else {
                                info!(
                                    "[obsidian] Cold storage: archived Episodic/{stem}.md \
                                     (>{cold_days}d old)"
                                );
                            }
                        }
                    }
                }
            }
        })
        .await
        .map_err(|e| VaultError::Config(e.to_string()))?;

        // Phase 3: move non-episodic db_only files to cold storage
        for dir_name in &db_only_dirs {
            let dir_path = self.vault_path.join(dir_name);
            if !dir_path.exists() || dir_name == "Episodic" {
                continue; // Episodic already handled above
            }
            self.archive_old_files_async(&dir_path, &cutoff).await?;
        }

        // Phase 4: enforce file ceiling — if still over limit, archive oldest episodic
        let vault_path = self.vault_path.clone();
        let new_count = tokio::task::spawn_blocking(move || count_all_md(&vault_path))
            .await
            .map_err(|e| VaultError::Config(e.to_string()))?;
        let overage = new_count.saturating_sub(max);
        if overage > 0 {
            info!(
                "[obsidian] Vault at {new_count} files ({overage} over limit of {max}); \
                 force-archiving oldest episodic"
            );
            self.archive_oldest_episodic_async(overage + 10).await?;
        }

        // CP-28: Prune old tombstones
        let tombstone_days = self.config.tombstone_prune_days;
        if tombstone_days > 0 {
            let stale_dir = self.vault_path.join(".stale");
            if stale_dir.exists() {
                let tombstone_cutoff =
                    Utc::now().date_naive() - Duration::days(tombstone_days as i64);
                tokio::task::spawn_blocking(move || {
                    for entry in fs::read_dir(&stale_dir).into_iter().flatten() {
                        let path = match entry {
                            Ok(e) => e.path(),
                            Err(_) => continue,
                        };
                        if !path.is_file() {
                            continue;
                        }
                        let should_prune = path
                            .metadata()
                            .and_then(|m| m.modified())
                            .ok()
                            .and_then(|t| {
                                let duration = t.duration_since(std::time::UNIX_EPOCH).ok()?;
                                let datetime =
                                    chrono::DateTime::from_timestamp(duration.as_secs() as i64, 0)?;
                                Some(datetime.date_naive() < tombstone_cutoff)
                            })
                            .unwrap_or(false);
                        if should_prune {
                            if let Err(e) = fs::remove_file(&path) {
                                warn!("[obsidian] Failed to prune tombstone: {}", e);
                            }
                        }
                    }
                })
                .await
                .map_err(|e| VaultError::Config(e.to_string()))?;
            }
        }

        Ok(())
    }

    async fn archive_old_files_async(
        &self,
        dir: &Path,
        cutoff: &NaiveDate,
    ) -> Result<(), VaultError> {
        let dir = dir.to_path_buf();
        let cutoff = *cutoff;
        let vault_path = self.vault_path.clone();
        tokio::task::spawn_blocking(move || archive_old_files_sync(&vault_path, &dir, &cutoff))
            .await
            .map_err(|e| VaultError::Config(e.to_string()))?
    }

    async fn archive_oldest_episodic_async(&self, count: usize) -> Result<(), VaultError> {
        let vault_path = self.vault_path.clone();
        tokio::task::spawn_blocking(move || archive_oldest_episodic_sync(&vault_path, count))
            .await
            .map_err(|e| VaultError::Config(e.to_string()))?
    }
}

fn archive_old_files_sync(
    vault_path: &Path,
    dir: &Path,
    cutoff: &NaiveDate,
) -> Result<(), VaultError> {
    if !dir.is_dir() {
        return Ok(());
    }
    for entry in fs::read_dir(dir)? {
        let path = entry?.path();
        if !path.is_file() || path.extension().is_none_or(|e| e != "md") {
            continue;
        }
        let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
        // CP-26: Try filename-as-date first, fall back to file modification time
        let should_archive = if let Ok(date) = NaiveDate::parse_from_str(stem, "%Y-%m-%d") {
            date < *cutoff
        } else {
            // Use file modification time for non-date-named files
            path.metadata()
                .and_then(|m| m.modified())
                .ok()
                .and_then(|t| {
                    let duration = t.duration_since(std::time::UNIX_EPOCH).ok()?;
                    let datetime = chrono::DateTime::from_timestamp(duration.as_secs() as i64, 0)?;
                    Some(datetime.date_naive() < *cutoff)
                })
                .unwrap_or(false)
        };
        if should_archive {
            let tombstone = vault_path.join(".stale").join(format!("{stem}.tombstone"));
            if !tombstone.exists() {
                if let Err(e) = fs::write(&tombstone, "Cold storage archive.\n") {
                    warn!("[obsidian] Failed to write tombstone: {}", e);
                }
            }
            if let Err(e) = fs::remove_file(&path) {
                warn!("[obsidian] Failed to remove archived file: {}", e);
            }
        }
    }
    Ok(())
}

fn archive_oldest_episodic_sync(vault_path: &Path, count: usize) -> Result<(), VaultError> {
    let episodic = vault_path.join("Episodic");
    if !episodic.is_dir() {
        return Ok(());
    }
    let mut dated: Vec<_> = Vec::new();
    for entry in fs::read_dir(&episodic)? {
        let path = entry?.path();
        if !path.is_file() || path.extension().is_none_or(|e| e != "md") {
            continue;
        }
        let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
        if let Ok(date) = NaiveDate::parse_from_str(stem, "%Y-%m-%d") {
            dated.push((date, path));
        }
    }
    dated.sort_by_key(|(d, _)| *d);
    for (_date, path) in dated.iter().take(count) {
        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("unknown");
        let tombstone = vault_path.join(".stale").join(format!("{stem}.tombstone"));
        if let Err(e) = fs::write(&tombstone, "Force-archived (file ceiling enforcement).\n") {
            warn!("[obsidian] Failed to write tombstone: {}", e);
        }
        if let Err(e) = fs::remove_file(path) {
            warn!("[obsidian] Failed to remove force-archived file: {}", e);
        }
        debug!("[obsidian] Force-archived Episodic/{stem}.md");
    }
    Ok(())
}

fn count_all_md(path: &Path) -> usize {
    let mut count = 0;
    if let Ok(entries) = fs::read_dir(path) {
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_dir() {
                count += count_all_md(&p);
            } else if p.extension().is_some_and(|e| e == "md") {
                count += 1;
            }
        }
    }
    count
}
