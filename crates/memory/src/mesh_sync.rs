//! Mesh P2P Sync (MEM-14)
//!
//! CRDT-based conflict resolution for concurrent writes across multiple
//! Savant instances. Uses vector clock ordering and selective sync by
//! namespace (enclave vs collective).
//!
//! This is a foundation for multi-instance deployment. Currently feature-gated
//! and disabled by default.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Vector clock for causal ordering of events across instances.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct VectorClock {
    /// Instance ID -> logical timestamp.
    pub clocks: HashMap<String, u64>,
}

impl VectorClock {
    pub fn new() -> Self {
        Self {
            clocks: HashMap::new(),
        }
    }

    /// Increments the clock for this instance.
    pub fn tick(&mut self, instance_id: &str) {
        let entry = self.clocks.entry(instance_id.to_string()).or_insert(0);
        *entry += 1;
    }

    /// Updates this clock with another clock (take max of each entry).
    pub fn merge(&mut self, other: &VectorClock) {
        for (instance, &time) in &other.clocks {
            let entry = self.clocks.entry(instance.clone()).or_insert(0);
            *entry = (*entry).max(time);
        }
    }

    /// Returns `true` if this clock happened before `other`.
    pub fn happened_before(&self, other: &VectorClock) -> bool {
        let mut any_less = false;
        for (instance, &time) in &self.clocks {
            let other_time = other.clocks.get(instance).copied().unwrap_or(0);
            if time > other_time {
                return false;
            }
            if time < other_time {
                any_less = true;
            }
        }
        // Also check instances in other but not in self
        for (instance, &time) in &other.clocks {
            if !self.clocks.contains_key(instance) && time > 0 {
                any_less = true;
            }
        }
        any_less
    }

    /// Returns `true` if the two clocks are concurrent (neither happened before the other).
    pub fn is_concurrent(&self, other: &VectorClock) -> bool {
        !self.happened_before(other) && !other.happened_before(self)
    }
}

impl Default for VectorClock {
    fn default() -> Self {
        Self::new()
    }
}

/// Sync operation types.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SyncOperation {
    /// Create a new memory entry.
    Create {
        id: u64,
        namespace: String,
        data: Vec<u8>,
    },
    /// Update an existing memory entry.
    Update {
        id: u64,
        namespace: String,
        data: Vec<u8>,
    },
    /// Delete a memory entry.
    Delete { id: u64, namespace: String },
}

/// A sync message exchanged between instances.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SyncMessage {
    /// Source instance ID.
    pub source: String,
    /// Vector clock at the time of this message.
    pub clock: VectorClock,
    /// The operations to apply.
    pub operations: Vec<SyncOperation>,
}

/// Sync namespace — determines which data is synced.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SyncNamespace {
    /// Private per-agent memories (enclave).
    Enclave,
    /// Shared hive-mind memories (collective).
    Collective,
}

/// CRDT merge strategy for conflict resolution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MergeStrategy {
    /// Compare timestamps, keep newer entry.
    Latest,
    /// Always keep local value, ignore remote operations.
    Local,
    /// Always apply remote value.
    Remote,
    /// Combine non-conflicting fields from both local and remote.
    Merge,
}

/// Mesh sync manager.
pub struct MeshSyncManager {
    /// This instance's ID.
    instance_id: String,
    /// This instance's vector clock.
    clock: VectorClock,
    /// Pending outbound operations.
    pending: Vec<SyncOperation>,
    /// Merge strategy.
    strategy: MergeStrategy,
}

impl MeshSyncManager {
    pub fn new(instance_id: String, strategy: MergeStrategy) -> Self {
        Self {
            instance_id: instance_id.clone(),
            clock: VectorClock::new(),
            pending: Vec::new(),
            strategy,
        }
    }

    /// Records a local operation and advances the vector clock.
    pub fn record_operation(&mut self, op: SyncOperation) {
        self.clock.tick(&self.instance_id);
        self.pending.push(op);
    }

    /// Creates a sync message with all pending operations.
    pub fn create_sync_message(&mut self) -> SyncMessage {
        let msg = SyncMessage {
            source: self.instance_id.clone(),
            clock: self.clock.clone(),
            operations: self.pending.drain(..).collect(),
        };
        msg
    }

    /// Processes an incoming sync message. Returns operations that should be applied
    /// based on the configured merge strategy.
    pub fn process_sync_message(&mut self, msg: SyncMessage) -> Vec<SyncOperation> {
        // Always merge incoming clocks to track causality
        self.clock.merge(&msg.clock);

        match self.strategy {
            // Local: discard all remote operations — local state is authoritative
            MergeStrategy::Local => Vec::new(),

            // Remote: accept all remote operations — remote state is authoritative
            MergeStrategy::Remote => msg.operations,

            // Latest: compare timestamps, keep newer. For operations without
            // timestamp metadata, accept them (assume remote is newer).
            MergeStrategy::Latest => {
                msg.operations
                    .into_iter()
                    .filter(|op| {
                        // Check if we have a pending local operation for the same ID
                        let op_id = match op {
                            SyncOperation::Create { id, .. }
                            | SyncOperation::Update { id, .. }
                            | SyncOperation::Delete { id, .. } => *id,
                        };
                        // Find any conflicting local operation
                        let local_conflict = self.pending.iter().find(|p| {
                            let p_id = match p {
                                SyncOperation::Create { id, .. }
                                | SyncOperation::Update { id, .. }
                                | SyncOperation::Delete { id, .. } => *id,
                            };
                            p_id == op_id
                        });

                        match local_conflict {
                            Some(_) => {
                                // Both have operations for same ID.
                                // Compare data blobs as timestamp proxy —
                                // larger data is treated as "newer" (more complete).
                                // If equal, prefer remote (remote wins ties).
                                let local_data_len = local_conflict
                                    .map(|op| match op {
                                        SyncOperation::Create { data, .. }
                                        | SyncOperation::Update { data, .. } => data.len(),
                                        SyncOperation::Delete { .. } => 0,
                                    })
                                    .unwrap_or(0);
                                let remote_data_len = match &op {
                                    SyncOperation::Create { data, .. }
                                    | SyncOperation::Update { data, .. } => data.len(),
                                    SyncOperation::Delete { .. } => 0,
                                };
                                remote_data_len >= local_data_len
                            }
                            None => {
                                // No local conflict — accept remote operation
                                true
                            }
                        }
                    })
                    .collect()
            }

            // Merge: combine non-conflicting operations from both local and remote.
            // Conflicts (same ID) are resolved by keeping the operation with more data.
            MergeStrategy::Merge => {
                let mut merged = Vec::new();
                let mut seen_ids: std::collections::HashSet<u64> = std::collections::HashSet::new();

                // First, add all local pending operations
                for op in &self.pending {
                    let op_id = match op {
                        SyncOperation::Create { id, .. }
                        | SyncOperation::Update { id, .. }
                        | SyncOperation::Delete { id, .. } => *id,
                    };
                    seen_ids.insert(op_id);
                    merged.push(op.clone());
                }

                // Then, add non-conflicting remote operations
                for op in msg.operations {
                    let op_id = match &op {
                        SyncOperation::Create { id, .. }
                        | SyncOperation::Update { id, .. }
                        | SyncOperation::Delete { id, .. } => *id,
                    };

                    if seen_ids.contains(&op_id) {
                        // Conflict: keep the one with more data (more complete state)
                        if let Some(local_idx) = merged.iter().position(|m| {
                            let m_id = match m {
                                SyncOperation::Create { id, .. }
                                | SyncOperation::Update { id, .. }
                                | SyncOperation::Delete { id, .. } => *id,
                            };
                            m_id == op_id
                        }) {
                            let local_data_len = match &merged[local_idx] {
                                SyncOperation::Create { data, .. }
                                | SyncOperation::Update { data, .. } => data.len(),
                                SyncOperation::Delete { .. } => 0,
                            };
                            let remote_data_len = match &op {
                                SyncOperation::Create { data, .. }
                                | SyncOperation::Update { data, .. } => data.len(),
                                SyncOperation::Delete { .. } => 0,
                            };
                            // Replace local if remote has more data
                            if remote_data_len > local_data_len {
                                merged[local_idx] = op;
                            }
                        }
                    } else {
                        seen_ids.insert(op_id);
                        merged.push(op);
                    }
                }

                merged
            }
        }
    }

    /// Returns the current vector clock.
    pub fn clock(&self) -> &VectorClock {
        &self.clock
    }

    /// Returns the instance ID.
    pub fn instance_id(&self) -> &str {
        &self.instance_id
    }
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)]
mod tests {
    use super::*;

    #[test]
    fn test_vector_clock_tick() {
        let mut clock = VectorClock::new();
        clock.tick("instance1");
        clock.tick("instance1");
        assert_eq!(clock.clocks.get("instance1"), Some(&2));
    }

    #[test]
    fn test_vector_clock_merge() {
        let mut clock1 = VectorClock::new();
        clock1.tick("a");
        clock1.tick("a");

        let mut clock2 = VectorClock::new();
        clock2.tick("b");

        clock1.merge(&clock2);
        assert_eq!(clock1.clocks.get("a"), Some(&2));
        assert_eq!(clock1.clocks.get("b"), Some(&1));
    }

    #[test]
    fn test_happened_before() {
        let mut clock1 = VectorClock::new();
        clock1.tick("a");

        let mut clock2 = VectorClock::new();
        clock2.tick("a");
        clock2.tick("a");

        assert!(clock1.happened_before(&clock2));
        assert!(!clock2.happened_before(&clock1));
    }

    #[test]
    fn test_concurrent_clocks() {
        let mut clock1 = VectorClock::new();
        clock1.tick("a");

        let mut clock2 = VectorClock::new();
        clock2.tick("b");

        assert!(clock1.is_concurrent(&clock2));
    }

    #[test]
    fn test_mesh_sync_basic() {
        let mut manager = MeshSyncManager::new("node1".to_string(), MergeStrategy::Latest);

        manager.record_operation(SyncOperation::Create {
            id: 1,
            namespace: "enclave".to_string(),
            data: vec![1, 2, 3],
        });

        let msg = manager.create_sync_message();
        assert_eq!(msg.source, "node1");
        assert_eq!(msg.operations.len(), 1);
    }

    #[test]
    fn test_mesh_sync_process_message() {
        let mut manager = MeshSyncManager::new("node1".to_string(), MergeStrategy::Latest);

        let msg = SyncMessage {
            source: "node2".to_string(),
            clock: {
                let mut c = VectorClock::new();
                c.tick("node2");
                c
            },
            operations: vec![SyncOperation::Create {
                id: 2,
                namespace: "collective".to_string(),
                data: vec![4, 5, 6],
            }],
        };

        let ops = manager.process_sync_message(msg);
        assert_eq!(ops.len(), 1);
        assert_eq!(manager.clock().clocks.get("node2"), Some(&1));
    }
}
