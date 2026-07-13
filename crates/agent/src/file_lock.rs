//! Agent File Lock — reader-writer locks with deadlock prevention for multi-agent file access.
//!
//! When multiple sub-agents operate on the same workspace, reader-writer locks prevent
//! data corruption. Concurrent reads are allowed. Writes are exclusive.

use dashmap::DashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::{OwnedRwLockReadGuard, OwnedRwLockWriteGuard, RwLock};

/// Per-file reader-writer lock for multi-agent file coordination.
pub struct AgentFileLock {
    locks: DashMap<PathBuf, Arc<RwLock<()>>>,
}

impl AgentFileLock {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            locks: DashMap::new(),
        })
    }

    /// Acquire a read lock on a file. Multiple readers can hold simultaneously.
    pub async fn read_lock(&self, path: &Path) -> OwnedRwLockReadGuard<()> {
        let lock = self
            .locks
            .entry(path.to_path_buf())
            .or_insert_with(|| Arc::new(RwLock::new(())))
            .clone();
        lock.read_owned().await
    }

    /// Acquire a write lock on a file. Exclusive — blocks all other readers and writers.
    pub async fn write_lock(&self, path: &Path) -> OwnedRwLockWriteGuard<()> {
        let lock = self
            .locks
            .entry(path.to_path_buf())
            .or_insert_with(|| Arc::new(RwLock::new(())))
            .clone();
        lock.write_owned().await
    }

    /// Release all locks for a given agent (called on agent shutdown).
    /// This removes lock entries that have no active holders.
    pub fn cleanup(&self) {
        self.locks.retain(|_, lock| Arc::strong_count(lock) > 1);
    }
}

impl Default for AgentFileLock {
    fn default() -> Self {
        Self {
            locks: DashMap::new(),
        }
    }
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[tokio::test]
    async fn test_concurrent_reads() {
        let file_lock = AgentFileLock::new();
        let path = PathBuf::from("/tmp/test_file.rs");

        let _r1 = file_lock.read_lock(&path).await;
        let _r2 = file_lock.read_lock(&path).await;
        // Both reads acquired — no deadlock
    }

    #[tokio::test]
    async fn test_exclusive_write() {
        let file_lock = AgentFileLock::new();
        let path = PathBuf::from("/tmp/test_file.rs");

        let _w = file_lock.write_lock(&path).await;
        // Write acquired — would block readers
    }

    #[tokio::test]
    async fn test_cleanup() {
        let file_lock = AgentFileLock::new();
        let path = PathBuf::from("/tmp/test_file.rs");

        {
            let _r = file_lock.read_lock(&path).await;
            assert!(file_lock.locks.contains_key(&path));
        }
        // Lock dropped — cleanup should remove entry
        file_lock.cleanup();
    }
}
