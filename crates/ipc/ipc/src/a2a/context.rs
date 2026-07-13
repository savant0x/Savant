//! Context Package — Memory-aware context passing between agents.
//!
//! Instead of passing raw text between agents, Savant passes CortexaDB
//! collection keys into its 4-graph reflective memory system. A subagent
//! hydrates context by reading from CortexaDB collections via zero-copy
//! rkyv deserialization.
//!
//! The ContextPackage also references Obsidian vault file paths for large
//! payloads that don't fit in shared memory.

use rkyv::{Archive, Deserialize, Serialize};

/// Memory-aware context package for inter-agent delegation.
///
/// Contains CortexaDB collection keys for all 4 graph types, shared memory
/// offsets for recent tool outputs, and Obsidian vault references for large
/// payloads.
///
/// Size: 448 bytes (7 cache lines)
#[repr(C)]
#[derive(Debug, Copy, Clone, PartialEq, Eq, Archive, Serialize, Deserialize)]
pub struct ContextPackage {
    // CortexaDB collection keys for context hydration
    pub session_collection: [u8; 64],
    pub semantic_collection: [u8; 64],
    pub episodic_collection: [u8; 64],
    pub entity_collection: [u8; 64],
    pub causal_collection: [u8; 64],
    pub temporal_collection: [u8; 64],
    // Shared memory offsets for recent tool outputs (fixed array)
    pub tool_output_offsets: [u32; 8],
    pub tool_output_count: u8,
    // Security and scope
    pub namespace_scope: u64,
    pub max_token_budget: u32,
    // Obsidian vault rich path (for large payloads)
    pub obsidian_path_offset: u32,
    pub obsidian_path_len: u16,
    pub _padding: [u8; 2],
}

impl ContextPackage {
    pub fn new() -> Self {
        Self {
            session_collection: [0u8; 64],
            semantic_collection: [0u8; 64],
            episodic_collection: [0u8; 64],
            entity_collection: [0u8; 64],
            causal_collection: [0u8; 64],
            temporal_collection: [0u8; 64],
            tool_output_offsets: [0u32; 8],
            tool_output_count: 0,
            namespace_scope: 0,
            max_token_budget: 4096,
            obsidian_path_offset: 0,
            obsidian_path_len: 0,
            _padding: [0u8; 2],
        }
    }

    /// Sets the session collection key (conversation history).
    pub fn with_session_collection(mut self, key: &str) -> Self {
        let len = key.len().min(64);
        self.session_collection[..len].copy_from_slice(&key.as_bytes()[..len]);
        self
    }

    /// Sets the semantic collection key (concepts, relations).
    pub fn with_semantic_collection(mut self, key: &str) -> Self {
        let len = key.len().min(64);
        self.semantic_collection[..len].copy_from_slice(&key.as_bytes()[..len]);
        self
    }

    /// Sets the episodic collection key (events, experiences).
    pub fn with_episodic_collection(mut self, key: &str) -> Self {
        let len = key.len().min(64);
        self.episodic_collection[..len].copy_from_slice(&key.as_bytes()[..len]);
        self
    }

    /// Sets the entity collection key (people, projects, services).
    pub fn with_entity_collection(mut self, key: &str) -> Self {
        let len = key.len().min(64);
        self.entity_collection[..len].copy_from_slice(&key.as_bytes()[..len]);
        self
    }

    /// Sets the causal collection key (actions, outcomes).
    pub fn with_causal_collection(mut self, key: &str) -> Self {
        let len = key.len().min(64);
        self.causal_collection[..len].copy_from_slice(&key.as_bytes()[..len]);
        self
    }

    /// Sets the temporal collection key (ordering, evolution).
    pub fn with_temporal_collection(mut self, key: &str) -> Self {
        let len = key.len().min(64);
        self.temporal_collection[..len].copy_from_slice(&key.as_bytes()[..len]);
        self
    }

    /// Adds a tool output offset.
    pub fn with_tool_output(mut self, offset: u32) -> Self {
        if (self.tool_output_count as usize) < 8 {
            self.tool_output_offsets[self.tool_output_count as usize] = offset;
            self.tool_output_count += 1;
        }
        self
    }

    /// Sets the namespace scope for memory isolation.
    pub fn with_namespace_scope(mut self, scope: u64) -> Self {
        self.namespace_scope = scope;
        self
    }

    /// Sets the maximum token budget for context hydration.
    pub fn with_token_budget(mut self, budget: u32) -> Self {
        self.max_token_budget = budget;
        self
    }

    /// Sets the Obsidian vault path for large payloads.
    pub fn with_obsidian_path(mut self, offset: u32, len: u16) -> Self {
        self.obsidian_path_offset = offset;
        self.obsidian_path_len = len;
        self
    }

    /// Returns true if this package has any collection keys set.
    pub fn has_collections(&self) -> bool {
        self.session_collection != [0u8; 64]
            || self.semantic_collection != [0u8; 64]
            || self.episodic_collection != [0u8; 64]
            || self.entity_collection != [0u8; 64]
            || self.causal_collection != [0u8; 64]
            || self.temporal_collection != [0u8; 64]
    }

    /// Returns the number of tool output references.
    pub fn tool_output_len(&self) -> usize {
        self.tool_output_count as usize
    }

    /// Returns true if this package references an Obsidian vault path.
    pub fn has_obsidian_ref(&self) -> bool {
        self.obsidian_path_len > 0
    }
}

impl Default for ContextPackage {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_context_package_size() {
        assert_eq!(std::mem::size_of::<ContextPackage>(), 448);
    }

    #[test]
    fn test_context_package_new() {
        let pkg = ContextPackage::new();
        assert_eq!(pkg.tool_output_count, 0);
        assert_eq!(pkg.max_token_budget, 4096);
        assert!(!pkg.has_collections());
        assert!(!pkg.has_obsidian_ref());
    }

    #[test]
    fn test_context_package_builder() {
        let pkg = ContextPackage::new()
            .with_session_collection("transcript.session-123")
            .with_semantic_collection("semantic.concepts")
            .with_episodic_collection("episodic.events")
            .with_namespace_scope(42)
            .with_token_budget(8192)
            .with_tool_output(100)
            .with_tool_output(200)
            .with_obsidian_path(500, 256);
        assert!(pkg.has_collections());
        assert_eq!(pkg.tool_output_count, 2);
        assert_eq!(pkg.tool_output_offsets[0], 100);
        assert_eq!(pkg.tool_output_offsets[1], 200);
        assert_eq!(pkg.namespace_scope, 42);
        assert_eq!(pkg.max_token_budget, 8192);
        assert!(pkg.has_obsidian_ref());
        assert_eq!(pkg.obsidian_path_offset, 500);
        assert_eq!(pkg.obsidian_path_len, 256);
    }

    #[test]
    fn test_context_package_collection_keys() {
        let pkg = ContextPackage::new().with_session_collection("test.session");
        assert!(pkg.has_collections());
        assert_eq!(&pkg.session_collection[..12], b"test.session");
    }

    #[test]
    fn test_context_package_tool_output_overflow() {
        let mut pkg = ContextPackage::new();
        for i in 0..10 {
            pkg = pkg.with_tool_output(i * 100);
        }
        // Should cap at 8
        assert_eq!(pkg.tool_output_count, 8);
    }
}
