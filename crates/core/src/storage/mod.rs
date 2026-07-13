//! Storage engine abstraction layer.
//!
//! Defines the `StorageEngine` trait for chat/message storage and provides
//! `CortexaDbAdapter` wrapping the existing `crate::db::Storage`.
//!
//! Reserved for: read replicas, sharded storage, cold tier.
//! Current implementation via CortexaDbAdapter wrapping crate::db (CortexaDB).

use crate::error::SavantError;
use crate::types::ChatMessage;

/// Abstraction over chat/message storage backends.
///
/// Implementors provide storage for agent chat history, swarm messages,
/// and related data. Future backends (read replicas, sharded, cold tier)
/// implement this trait to swap storage without changing call sites.
pub trait StorageEngine: Send + Sync {
    /// Append a chat message for the given agent.
    fn append(&self, agent_id: &str, msg: &ChatMessage) -> Result<(), SavantError>;

    /// Retrieve chat history for an agent, most recent first.
    fn get_history(&self, agent_id: &str, limit: usize) -> Result<Vec<ChatMessage>, SavantError>;

    /// Retrieve swarm-wide chat history.
    fn get_swarm_history(&self, limit: usize) -> Result<Vec<ChatMessage>, SavantError>;

    /// Prune history for an agent, keeping only the most recent entries.
    fn prune(&self, agent_id: &str, keep_last: usize) -> Result<(), SavantError>;

    /// Run database integrity check and recovery.
    fn ghost_restore(&self) -> Result<(), SavantError>;

    /// Graceful shutdown — flush pending writes.
    fn shutdown(&self) -> Result<(), SavantError>;
}

/// Adapter wrapping `crate::db::Storage` to implement `StorageEngine`.
pub struct CortexaDbAdapter {
    inner: crate::db::Storage,
}

impl CortexaDbAdapter {
    pub fn new(inner: crate::db::Storage) -> Self {
        Self { inner }
    }
}

impl StorageEngine for CortexaDbAdapter {
    fn append(&self, agent_id: &str, msg: &ChatMessage) -> Result<(), SavantError> {
        self.inner.append_chat(agent_id, msg)
    }

    fn get_history(&self, agent_id: &str, limit: usize) -> Result<Vec<ChatMessage>, SavantError> {
        self.inner.get_history(agent_id, limit)
    }

    fn get_swarm_history(&self, limit: usize) -> Result<Vec<ChatMessage>, SavantError> {
        self.inner.get_swarm_history(limit)
    }

    fn prune(&self, agent_id: &str, keep_last: usize) -> Result<(), SavantError> {
        self.inner.prune_history(agent_id, keep_last)
    }

    fn ghost_restore(&self) -> Result<(), SavantError> {
        self.inner.ghost_restore()
    }

    fn shutdown(&self) -> Result<(), SavantError> {
        self.inner.shutdown()
    }
}
