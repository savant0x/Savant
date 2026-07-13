//! Application-Level Audit Trail (MEM-10)
//!
//! Records all memory operations in a dedicated CortexaDB collection
//! for observability and debugging.

use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;

/// Types of auditable memory operations.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum AuditOperation {
    Store,
    Retrieve,
    Update,
    Delete,
    Consolidate,
    Promote,
    Archive,
    Distill,
    ArbiterResolve,
    VersionCreate,
    VersionSupersede,
    AccessRecord,
    DedupSkip,
    PrivacyRedact,
    CircuitBreakerOpen,
    CircuitBreakerClose,
    EmbeddingGenerated,
    Bm25IndexAdd,
    Bm25IndexRemove,
    RrfFusion,
    QueryExpansion,
    Rerank,
    ForensicSnapshot,
    MeshSync,
    MultimodalEmbed,
}

/// A single audit entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEntry {
    /// Sequential index.
    pub index: u64,
    /// Timestamp (epoch milliseconds).
    pub timestamp: i64,
    /// The operation type.
    pub operation: AuditOperation,
    /// Target memory entry IDs (if applicable).
    pub target_ids: Vec<u64>,
    /// Session ID (if applicable).
    pub session_id: String,
    /// Quality/relevance score (if applicable).
    pub quality_score: Option<f32>,
    /// Human-readable description.
    pub description: String,
}

/// In-memory audit trail with configurable retention.
///
/// Entries are stored in a VecDeque ring buffer for O(1) eviction of oldest
/// entries. The trail can be persisted to a CortexaDB collection for crash recovery.
pub struct AuditTrail {
    entries: VecDeque<AuditEntry>,
    next_index: u64,
    max_entries: usize,
}

impl AuditTrail {
    pub fn new(max_entries: usize) -> Self {
        Self {
            entries: VecDeque::with_capacity(max_entries.min(1000)),
            next_index: 0,
            max_entries,
        }
    }

    /// Records an audit entry.
    pub fn record(
        &mut self,
        operation: AuditOperation,
        target_ids: Vec<u64>,
        session_id: &str,
        quality_score: Option<f32>,
        description: &str,
    ) {
        let entry = AuditEntry {
            index: self.next_index,
            timestamp: Utc::now().timestamp_millis(),
            operation,
            target_ids,
            session_id: session_id.to_string(),
            quality_score,
            description: description.to_string(),
        };

        self.next_index += 1;

        // Ring buffer: evict oldest if full (O(1) with VecDeque::pop_front)
        if self.entries.len() >= self.max_entries {
            self.entries.pop_front();
        }

        self.entries.push_back(entry);
    }

    /// Returns all entries.
    pub fn entries(&self) -> &VecDeque<AuditEntry> {
        &self.entries
    }

    /// Returns entries for a specific operation type.
    pub fn entries_by_operation(&self, op: &AuditOperation) -> Vec<&AuditEntry> {
        self.entries.iter().filter(|e| &e.operation == op).collect()
    }

    /// Returns entries for a specific session.
    pub fn entries_by_session(&self, session_id: &str) -> Vec<&AuditEntry> {
        self.entries
            .iter()
            .filter(|e| e.session_id == session_id)
            .collect()
    }

    /// Returns entries within a time range (epoch milliseconds).
    pub fn entries_in_range(&self, start: i64, end: i64) -> Vec<&AuditEntry> {
        self.entries
            .iter()
            .filter(|e| e.timestamp >= start && e.timestamp <= end)
            .collect()
    }

    /// Returns the total number of recorded entries (including evicted).
    pub fn total_recorded(&self) -> u64 {
        self.next_index
    }

    /// Serializes all entries to JSON.
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(&self.entries)
    }

    /// Clears all entries.
    pub fn clear(&mut self) {
        self.entries.clear();
    }
}

impl Default for AuditTrail {
    fn default() -> Self {
        // Default retention: 90 days worth of entries at ~1000 ops/day
        Self::new(100_000)
    }
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)]
mod tests {
    use super::*;

    #[test]
    fn test_audit_trail_record() {
        let mut trail = AuditTrail::new(100);
        trail.record(
            AuditOperation::Store,
            vec![1],
            "s1",
            Some(0.9),
            "stored memory 1",
        );
        assert_eq!(trail.entries().len(), 1);
        assert_eq!(trail.entries()[0].operation, AuditOperation::Store);
    }

    #[test]
    fn test_audit_trail_ring_buffer() {
        let mut trail = AuditTrail::new(3);
        for i in 0..5 {
            trail.record(
                AuditOperation::Store,
                vec![i],
                "s1",
                None,
                &format!("entry {}", i),
            );
        }
        assert_eq!(trail.entries().len(), 3);
        // Oldest entries (0, 1) should be evicted
        assert_eq!(trail.entries()[0].index, 2);
    }

    #[test]
    fn test_audit_trail_filter_by_operation() {
        let mut trail = AuditTrail::new(100);
        trail.record(AuditOperation::Store, vec![1], "s1", None, "store");
        trail.record(AuditOperation::Retrieve, vec![1], "s1", None, "retrieve");
        trail.record(AuditOperation::Store, vec![2], "s1", None, "store");

        let stores = trail.entries_by_operation(&AuditOperation::Store);
        assert_eq!(stores.len(), 2);
    }

    #[test]
    fn test_audit_trail_filter_by_session() {
        let mut trail = AuditTrail::new(100);
        trail.record(AuditOperation::Store, vec![1], "s1", None, "store");
        trail.record(AuditOperation::Store, vec![2], "s2", None, "store");

        let s1 = trail.entries_by_session("s1");
        assert_eq!(s1.len(), 1);
    }

    #[test]
    fn test_audit_trail_time_range() {
        let mut trail = AuditTrail::new(100);
        trail.record(AuditOperation::Store, vec![1], "s1", None, "store");

        let now = Utc::now().timestamp_millis();
        let entries = trail.entries_in_range(now - 1000, now + 1000);
        assert_eq!(entries.len(), 1);
    }

    #[test]
    fn test_audit_trail_json() {
        let mut trail = AuditTrail::new(100);
        trail.record(AuditOperation::Store, vec![1], "s1", Some(0.8), "test");

        let json = trail.to_json().expect("serialization failed");
        assert!(json.contains("Store"));
        assert!(json.contains("test"));
    }

    #[test]
    fn test_audit_trail_total_recorded() {
        let mut trail = AuditTrail::new(3);
        for i in 0..5 {
            trail.record(AuditOperation::Store, vec![i], "s1", None, "x");
        }
        assert_eq!(trail.total_recorded(), 5); // total includes evicted
    }
}
