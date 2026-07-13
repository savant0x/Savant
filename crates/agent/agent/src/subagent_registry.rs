//! Sub-Agent Registry — lightweight in-memory tracking for active sub-agents.
//!
//! Tracks spawned sub-agents with their profile, role, depth, spawn time,
//! cancellation token, iteration budget, and token consumption.

use dashmap::DashMap;
use savant_core::types::AgentRole;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

/// Entry for a single active sub-agent.
#[derive(Debug)]
pub struct SubAgentEntry {
    pub id: String,
    pub parent_id: String,
    pub profile_name: String,
    pub role: AgentRole,
    pub depth: usize,
    pub spawn_time: chrono::DateTime<chrono::Utc>,
    pub cancellation_token: CancellationToken,
    pub iteration_budget: IterationBudget,
    pub tokens_consumed: AtomicUsize,
}

/// Thread-safe iteration budget with consume/refund semantics.
#[derive(Debug)]
pub struct IterationBudget {
    remaining: AtomicUsize,
    max: usize,
}

impl IterationBudget {
    pub fn new(max: usize) -> Self {
        Self {
            remaining: AtomicUsize::new(max),
            max,
        }
    }

    /// Consume one iteration. Returns false if budget exhausted.
    pub fn consume(&self) -> bool {
        let mut current = self.remaining.load(Ordering::SeqCst);
        loop {
            if current == 0 {
                return false;
            }
            match self.remaining.compare_exchange(
                current,
                current - 1,
                Ordering::SeqCst,
                Ordering::SeqCst,
            ) {
                Ok(_) => return true,
                Err(actual) => current = actual,
            }
        }
    }

    /// Refund n iterations (e.g., on successful completion).
    pub fn refund(&self, n: usize) {
        self.remaining.fetch_add(n, Ordering::SeqCst);
    }

    /// Remaining iterations.
    pub fn remaining(&self) -> usize {
        self.remaining.load(Ordering::SeqCst)
    }

    /// Maximum iterations.
    pub fn max(&self) -> usize {
        self.max
    }
}

/// Thread-safe registry for tracking active sub-agents.
pub struct SubAgentRegistry {
    active: DashMap<String, SubAgentEntry>,
}

impl SubAgentRegistry {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            active: DashMap::new(),
        })
    }

    /// Register a new sub-agent.
    pub fn register(&self, entry: SubAgentEntry) {
        tracing::info!(
            subagent_id = %entry.id,
            parent_id = %entry.parent_id,
            profile = %entry.profile_name,
            depth = entry.depth,
            "Sub-agent registered"
        );
        self.active.insert(entry.id.clone(), entry);
    }

    /// Unregister a sub-agent (called on completion or abort).
    pub fn unregister(&self, id: &str) -> Option<SubAgentEntry> {
        let entry = self.active.remove(id).map(|(_, e)| e);
        if entry.is_some() {
            tracing::info!(subagent_id = %id, "Sub-agent unregistered");
        }
        entry
    }

    /// Get the number of active sub-agents.
    pub fn active_count(&self) -> usize {
        self.active.len()
    }

    /// Get the number of active sub-agents for a specific parent.
    pub fn active_count_for_parent(&self, parent_id: &str) -> usize {
        self.active
            .iter()
            .filter(|e| e.parent_id == parent_id)
            .count()
    }

    /// Check if a sub-agent is active.
    pub fn is_active(&self, id: &str) -> bool {
        self.active.contains_key(id)
    }

    /// Get the cancellation token for a sub-agent.
    pub fn cancellation_token(&self, id: &str) -> Option<CancellationToken> {
        self.active.get(id).map(|e| e.cancellation_token.clone())
    }

    /// Cancel a sub-agent and unregister it.
    pub fn cancel_and_unregister(&self, id: &str) -> bool {
        if let Some((_, entry)) = self.active.remove(id) {
            entry.cancellation_token.cancel();
            tracing::info!(subagent_id = %id, "Sub-agent cancelled and unregistered");
            true
        } else {
            false
        }
    }

    /// Cancel all sub-agents for a specific parent.
    pub fn cancel_all_for_parent(&self, parent_id: &str) -> usize {
        let to_cancel: Vec<String> = self
            .active
            .iter()
            .filter(|e| e.parent_id == parent_id)
            .map(|e| e.id.clone())
            .collect();

        let count = to_cancel.len();
        for id in to_cancel {
            self.cancel_and_unregister(&id);
        }
        count
    }

    /// Get all active sub-agent IDs.
    pub fn active_ids(&self) -> Vec<String> {
        self.active.iter().map(|e| e.id.clone()).collect()
    }
}

impl Default for SubAgentRegistry {
    fn default() -> Self {
        Self {
            active: DashMap::new(),
        }
    }
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)]
mod tests {
    use super::*;

    fn test_entry(id: &str, parent: &str) -> SubAgentEntry {
        SubAgentEntry {
            id: id.to_string(),
            parent_id: parent.to_string(),
            profile_name: "general".to_string(),
            role: AgentRole::Leaf,
            depth: 1,
            spawn_time: chrono::Utc::now(),
            cancellation_token: CancellationToken::new(),
            iteration_budget: IterationBudget::new(50),
            tokens_consumed: AtomicUsize::new(0),
        }
    }

    #[test]
    fn test_register_and_unregister() {
        let registry = SubAgentRegistry::new();
        registry.register(test_entry("sub-1", "parent-1"));
        assert_eq!(registry.active_count(), 1);
        assert!(registry.is_active("sub-1"));

        let entry = registry.unregister("sub-1");
        assert!(entry.is_some());
        assert_eq!(registry.active_count(), 0);
        assert!(!registry.is_active("sub-1"));
    }

    #[test]
    fn test_active_count_for_parent() {
        let registry = SubAgentRegistry::new();
        registry.register(test_entry("sub-1", "parent-1"));
        registry.register(test_entry("sub-2", "parent-1"));
        registry.register(test_entry("sub-3", "parent-2"));

        assert_eq!(registry.active_count_for_parent("parent-1"), 2);
        assert_eq!(registry.active_count_for_parent("parent-2"), 1);
        assert_eq!(registry.active_count_for_parent("parent-3"), 0);
    }

    #[test]
    fn test_cancel_all_for_parent() {
        let registry = SubAgentRegistry::new();
        registry.register(test_entry("sub-1", "parent-1"));
        registry.register(test_entry("sub-2", "parent-1"));
        registry.register(test_entry("sub-3", "parent-2"));

        let cancelled = registry.cancel_all_for_parent("parent-1");
        assert_eq!(cancelled, 2);
        assert_eq!(registry.active_count(), 1);
        assert!(registry.is_active("sub-3"));
    }

    #[test]
    fn test_iteration_budget() {
        let budget = IterationBudget::new(3);
        assert_eq!(budget.remaining(), 3);
        assert!(budget.consume());
        assert!(budget.consume());
        assert!(budget.consume());
        assert!(!budget.consume()); // exhausted
        assert_eq!(budget.remaining(), 0);

        budget.refund(2);
        assert_eq!(budget.remaining(), 2);
        assert!(budget.consume());
    }
}
