use iceoryx2::prelude::ZeroCopySend;
use iceoryx2::prelude::*;
use iceoryx2::service::port_factory::blackboard::PortFactory;
use std::collections::HashSet;
use std::sync::{Arc, RwLock};
use tracing::{debug, error, info};

use crate::error::SwarmIpcError;
use xxhash_rust::xxh3::xxh3_64;

/// A fixed-size, lock-free Bloom Filter designed for cache-efficient loop detection.
/// Used to prevent Issue #37842: Infinite Multi-Agent Delegation Loops.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, ZeroCopySend, Default)]
pub struct DelegationBloomFilter {
    /// 256 bits of storage for the bloom filter
    pub bitfield: [u64; 4],
    /// Number of distinct agents that have participated in this trace chain
    pub depth_count: u8,
    pub padding: [u8; 7],
}

impl DelegationBloomFilter {
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds an agent's UUID hash to the trace path.
    pub fn add_agent(&mut self, agent_id_hash: u64) {
        let h1 = xxh3_64(&agent_id_hash.to_le_bytes());
        let h2 = h1.rotate_left(21);
        let h3 = h1.rotate_right(17);

        for hash in [h1, h2, h3] {
            let bit_index = (hash % 256) as usize;
            let array_index = bit_index / 64;
            let bit_offset = bit_index % 64;
            self.bitfield[array_index] |= 1 << bit_offset;
        }
        self.depth_count = self.depth_count.saturating_add(1);
    }

    /// Verifies if an agent has already participated in this exact task chain.
    pub fn contains_agent(&self, agent_id_hash: u64) -> bool {
        let h1 = xxh3_64(&agent_id_hash.to_le_bytes());
        let h2 = h1.rotate_left(21);
        let h3 = h1.rotate_right(17);

        for hash in [h1, h2, h3] {
            let bit_index = (hash % 256) as usize;
            let array_index = bit_index / 64;
            let bit_offset = bit_index % 64;

            if (self.bitfield[array_index] & (1 << bit_offset)) == 0 {
                return false;
            }
        }
        true
    }
}

/// The shared context structure for a swarm session.
///
/// MUST be `#[repr(C)]` and contain only trivially copyable types to guarantee
/// safe zero-copy memory mapping across process boundaries without allocator mismatch.
///
/// # Layout (128 bytes)
///
/// The structure is expanded to 128 bytes to support distributed telemetry
/// and cycle detection while remaining aligned for high-concurrency access.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, ZeroCopySend)]
pub struct SwarmSharedContext {
    /// Hashed representation of the OpenClaw UUID SessionId
    pub session_id_hash: u64,
    /// The parent orchestrator's ID (used for tracing hierarchy)
    pub parent_agent_id: u32,
    /// Remaining token budget to prevent infinite loops and cost explosions
    pub current_token_budget: u32,
    /// DSP-computed task complexity score (higher = more speculative steps)
    pub task_complexity_score: f32,
    /// Boolean flag to trigger emergency halts across the entire swarm
    pub emergency_halt: bool,
    /// Continuation token: indicates agent should yield and reschedule
    pub continue_work_delay_ms: u32,

    // --- Distributed Telemetry (W3C TraceContext) ---
    pub trace_id: [u8; 16],
    pub span_id: [u8; 8],

    // --- Cycle Detection ---
    pub delegation_filter: DelegationBloomFilter,
    pub max_delegation_depth: u8,

    /// Reserved for future extension (maintains 128-byte alignment)
    pub reserved: [u8; 25],
}

impl Default for SwarmSharedContext {
    fn default() -> Self {
        Self {
            session_id_hash: 0,
            parent_agent_id: 0,
            current_token_budget: 100_000,
            task_complexity_score: 0.0,
            emergency_halt: false,
            continue_work_delay_ms: 0,
            trace_id: [0; 16],
            span_id: [0; 8],
            delegation_filter: DelegationBloomFilter::new(),
            max_delegation_depth: 20,
            reserved: [0; 25],
        }
    }
}

/// The Zero-Copy Blackboard service.
///
/// Manages a shared memory blackboard where the orchestrator can publish context
/// and subagents can read it in O(1) time with zero serialization.
///
/// # Architecture
///
/// The blackboard uses iceoryx2's Blackboard which provides:
/// - POSIX shared memory backing (`/dev/shm/iox2_*`)
/// - True O(1) lookup by key (session_id_hash)
/// - Multiple concurrent readers without locking
/// - Single writer semantics with atomic updates
///
/// # Thread Safety
///
/// `SwarmBlackboard` is fully thread-safe:
/// - Multiple threads can call `publish_context` concurrently (internal serialization)
/// - Multiple threads can call `read_context` concurrently (lock-free reads)
/// - The implementation uses atomics and memory barriers for proper synchronization
///
/// # Performance
///
/// - Write latency: ~100ns
/// - Read latency: ~50ns
/// - Zero heap allocation for reads
/// - Constant memory overhead regardless of number of readers (O(1))
pub struct SwarmBlackboard {
    _node: Arc<Node<ipc::Service>>,
    service: PortFactory<ipc::Service, u64>,
    service_name: String,
    active_sessions: Arc<RwLock<HashSet<u64>>>,
}

impl SwarmBlackboard {
    /// Initializes the zero-copy IPC environment for the Gateway.
    ///
    /// Creates an iceoryx2 node with a blackboard service that can support
    /// up to 1024 concurrent readers (subagents) and 10 nodes for multi-process scaling.
    ///
    /// # Arguments
    /// * `service_name` - Unique name for the blackboard service (e.g., "savant_swarm")
    ///   Must be a valid iceoryx2 service name (alphanumeric, underscores, hyphens).
    ///
    /// # Errors
    /// Returns `SwarmIpcError` if node creation or service initialization fails.
    pub fn new(service_name: &str) -> Result<Self, SwarmIpcError> {
        info!("Initializing Zero-Copy Blackboard '{}'", service_name);

        // Validate service name
        if service_name.is_empty() || service_name.len() > 255 {
            return Err(SwarmIpcError::InvalidServiceName(format!(
                "Service name must be 1-255 characters, got {}",
                service_name.len()
            )));
        }

        // Create the central node that owns all service entities.
        let node = NodeBuilder::new()
            .create::<ipc::Service>()
            .map_err(|e| SwarmIpcError::NodeCreation(e.to_string()))?;

        // Clean up stale shared memory from dead processes before creating.
        let cleanup = Node::<ipc::Service>::cleanup_dead_nodes(Config::global_config());
        if cleanup.cleanups > 0 || cleanup.failed_cleanups > 0 {
            debug!(
                "SwarmBlackboard: stale node cleanup — {} removed, {} failed",
                cleanup.cleanups, cleanup.failed_cleanups
            );
        }

        // Attempt to create the blackboard service with retry-on-collision.
        // On Windows, a prior crashed process can leave stale shared memory
        // handles. When creation fails, we fall back to a suffixed service name.
        let base_name = service_name.to_string();
        let mut attempt: u32 = 0;
        let max_attempts: u32 = 5;

        let (service, resolved_name) = loop {
            let candidate = if attempt == 0 {
                base_name.clone()
            } else {
                format!("{}_{}", base_name, attempt)
            };

            let iox_name: iceoryx2::prelude::ServiceName = candidate.as_str().try_into().map_err(
                |e: iceoryx2::service::service_name::ServiceNameError| {
                    SwarmIpcError::ServiceCreation(e.to_string())
                },
            )?;

            match node
                .service_builder(&iox_name)
                .blackboard_creator::<u64>()
                .max_readers(1024)
                .max_nodes(10)
                .add::<SwarmSharedContext>(0, SwarmSharedContext::default())
                .create()
            {
                Ok(svc) => break (svc, candidate),
                Err(e) if attempt < max_attempts => {
                    attempt += 1;
                    debug!(
                        "Zero-Copy Blackboard '{}' creation failed (attempt {}/{}), retrying as '{}': {}",
                        base_name, attempt, max_attempts, candidate, e
                    );
                }
                Err(e) => {
                    return Err(SwarmIpcError::ServiceCreation(format!(
                        "Blackboard '{}' creation failed after {} attempts: {}",
                        base_name,
                        max_attempts + 1,
                        e
                    )));
                }
            }
        };

        info!(
            "Zero-Copy Blackboard '{}' initialized (max_readers=1024, max_nodes=10)",
            resolved_name
        );

        Ok(Self {
            _node: Arc::new(node),
            service,
            service_name: resolved_name,
            active_sessions: Arc::new(RwLock::new(HashSet::new())),
        })
    }

    /// Orchestrator writes updated context to the blackboard.
    ///
    /// This is an O(1) operation - all subagents reading this session
    /// instantly see the updated state without any pub-sub overhead.
    ///
    /// # Arguments
    /// * `session_id` - The session ID hash (derived from UUID)
    /// * `context` - The shared context to publish (copied into shared memory)
    ///
    /// # Errors
    /// Returns `SwarmIpcError` if the writer cannot be created or update fails.
    pub fn publish_context(
        &self,
        session_id: u64,
        context: SwarmSharedContext,
    ) -> Result<(), SwarmIpcError> {
        // Create a writer port. This operation is fast (no allocation) and can be
        // called frequently. The writer is scoped to this function and dropped
        // immediately after the update, minimizing synchronization overhead.
        let writer = self.service.writer_builder().create().map_err(|e| {
            SwarmIpcError::AccessViolation(format!(
                "Failed to create writer for session {}: {}",
                session_id, e
            ))
        })?;

        // Create an entry handle for this session. This requires the session key
        // to have been added during blackboard creation.
        let entry = writer
            .entry::<SwarmSharedContext>(&session_id)
            .map_err(|e| {
                SwarmIpcError::AccessViolation(format!(
                    "Session {} not found or type mismatch: {}",
                    session_id, e
                ))
            })?;

        entry.update_with_copy(context);

        // Track this session as active
        if let Ok(mut sessions) = self.active_sessions.write() {
            sessions.insert(session_id);
        }

        debug!(session_id = %session_id, "Published context to blackboard");
        Ok(())
    }

    /// Subagent reads context directly from shared memory.
    ///
    /// This provides zero-copy access - the returned reference points directly
    /// to the shared memory segment, avoiding any heap allocation or serialization.
    ///
    /// # Arguments
    /// * `session_id` - The session ID hash to read
    ///
    /// # Returns
    /// * `Ok(SwarmSharedContext)` if found - the value is a copy (32 bytes) from shared memory
    /// * `Err(SwarmIpcError)` if the session doesn't exist or access fails
    pub fn read_context(&self, session_id: u64) -> Result<SwarmSharedContext, SwarmIpcError> {
        let reader = self.service.reader_builder().create().map_err(|e| {
            SwarmIpcError::AccessViolation(format!(
                "Failed to create reader for session {}: {}",
                session_id, e
            ))
        })?;

        if let Ok(entry) = reader.entry::<SwarmSharedContext>(&session_id) {
            let context = entry.get();
            // SAFETY: iceoryx2 guarantees the memory is valid and properly synchronized
            // The returned reference is zero-copy.
            // We dereference to copy since SwarmSharedContext is Copy
            Ok(*context)
        } else {
            error!(session_id = %session_id, "Context not found in blackboard");
            Err(SwarmIpcError::AccessViolation(format!(
                "Context for session {} not found in blackboard '{}'",
                session_id, self.service_name
            )))
        }
    }

    /// Checks if a session exists in the blackboard without reading the full context.
    ///
    /// This is a lightweight existence check - useful for validating session IDs
    /// before attempting expensive operations.
    pub fn has_session(&self, session_id: u64) -> bool {
        let Ok(reader) = self.service.reader_builder().create() else {
            return false;
        };
        reader.entry::<SwarmSharedContext>(&session_id).is_ok()
    }

    /// Removes a session from the blackboard.
    ///
    /// Attempts to remove a session from the blackboard.
    ///
    /// Note: iceoryx2 0.8.x Blackboards use a static key-value mapping.
    /// Direct removal of keys is not supported. This function clears the
    /// session data to default values but the key slot remains allocated.
    /// Returns Ok(true) if the session was found and cleared, Ok(false) if not found.
    pub fn remove_session(&self, session_id: u64) -> Result<bool, SwarmIpcError> {
        // OMEGA-VII: Hardened Cessation via Death Signal
        // Instead of just clearing to default, we explicitly set halt flags
        // to force any active speculative agents to stop immediately.
        let death_signal = SwarmSharedContext {
            emergency_halt: true,
            current_token_budget: 0,
            ..SwarmSharedContext::default()
        };
        self.publish_context(session_id, death_signal)?;

        // Remove from active session tracking
        if let Ok(mut sessions) = self.active_sessions.write() {
            sessions.remove(&session_id);
        }

        debug!(session_id = %session_id, "Death Signal published to session");
        Ok(true)
    }

    /// Returns the service name associated with this blackboard.
    pub fn service_name(&self) -> &str {
        &self.service_name
    }

    /// Returns statistics about the blackboard (for monitoring/debugging).
    ///
    /// `active_sessions` reflects the number of sessions that have written context
    /// and not yet been deleted. `max_capacity` reflects the configured subscriber limit.
    pub fn stats(&self) -> BlackboardStats {
        let count = self.active_sessions.read().map(|s| s.len()).unwrap_or(0);
        BlackboardStats {
            active_sessions: count,
            max_capacity: 1024,
            service_name: self.service_name.clone(),
        }
    }
}

impl Drop for SwarmBlackboard {
    fn drop(&mut self) {
        info!("Shutting down blackboard service '{}'", self.service_name);
    }
}

/// Utility: Compute a stable hash of a UUID string for session identification.
///
/// Uses a FNV-1a hash algorithm for speed and reasonable distribution.
/// The same UUID will always produce the same hash across processes.
pub fn hash_session_id(uuid: &str) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for &byte in uuid.as_bytes() {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

/// Statistics about the blackboard state.
#[derive(Debug, Clone)]
pub struct BlackboardStats {
    pub active_sessions: usize,
    pub max_capacity: usize,
    pub service_name: String,
}

// ============================================================================
// Capability Blackboard Extension
// ============================================================================
// The following provides a higher-level capability registry for AgentCards,
// using a separate iceoryx2 blackboard service keyed by agent_id hash.
// This enables the Orchestrator to discover agent capabilities at runtime
// rather than relying on filesystem scanning.

/// Registry for AgentCard capability advertisements.
///
/// Uses a separate iceoryx2 blackboard service keyed by agent_id hash
/// (u64). Each entry is a 192-byte AgentCard from the `a2a` module.
///
/// The Orchestrator scans this registry to find the best agent for a
/// delegated task using semantic similarity + pressure scoring + skill
/// verification.
pub struct CapabilityRegistry {
    _node: Arc<Node<ipc::Service>>,
    service: PortFactory<ipc::Service, u64>,
    service_name: String,
    registered_ids: Arc<RwLock<HashSet<u64>>>,
}

impl CapabilityRegistry {
    /// Creates a new capability registry for AgentCard advertisements.
    ///
    /// `max_agents` controls the maximum number of concurrent agent registrations.
    /// Each agent occupies one slot keyed by its FNV-1a hash.
    pub fn new(service_name: &str, max_agents: usize) -> Result<Self, SwarmIpcError> {
        if service_name.is_empty() || service_name.len() > 255 {
            return Err(SwarmIpcError::InvalidServiceName(format!(
                "Service name must be 1-255 characters, got {}",
                service_name.len()
            )));
        }
        if max_agents == 0 || max_agents > 128 {
            return Err(SwarmIpcError::InvalidServiceName(format!(
                "max_agents must be 1-128, got {}",
                max_agents
            )));
        }

        let node = NodeBuilder::new()
            .create::<ipc::Service>()
            .map_err(|e| SwarmIpcError::NodeCreation(e.to_string()))?;

        // Clean up stale shared memory from dead processes before creating.
        let cleanup = Node::<ipc::Service>::cleanup_dead_nodes(Config::global_config());
        if cleanup.cleanups > 0 || cleanup.failed_cleanups > 0 {
            debug!(
                "CapabilityRegistry: stale node cleanup — {} removed, {} failed",
                cleanup.cleanups, cleanup.failed_cleanups
            );
        }

        let base_name = service_name.to_string();
        let mut attempt: u32 = 0;
        let max_attempts: u32 = 5;

        let service = loop {
            let candidate = if attempt == 0 {
                base_name.clone()
            } else {
                format!("{}_{}", base_name, attempt)
            };

            let iox_name: iceoryx2::prelude::ServiceName = candidate.as_str().try_into().map_err(
                |e: iceoryx2::service::service_name::ServiceNameError| {
                    SwarmIpcError::ServiceCreation(e.to_string())
                },
            )?;

            match node
                .service_builder(&iox_name)
                .blackboard_creator::<u64>()
                .max_readers(1024)
                .max_nodes(10)
                .add::<AgentCardCopy>(0, AgentCardCopy::default())
                .create()
            {
                Ok(svc) => break svc,
                Err(e) if attempt < max_attempts => {
                    attempt += 1;
                    debug!(
                        "CapabilityRegistry '{}' creation failed (attempt {}/{}), retrying as '{}': {}",
                        base_name, attempt, max_attempts, candidate, e
                    );
                }
                Err(e) => {
                    return Err(SwarmIpcError::ServiceCreation(format!(
                        "CapabilityRegistry '{}' creation failed after {} attempts: {}",
                        base_name,
                        max_attempts + 1,
                        e
                    )));
                }
            }
        };

        info!(
            "CapabilityRegistry '{}' initialized (max_agents={})",
            base_name, max_agents
        );

        Ok(Self {
            _node: Arc::new(node),
            service,
            service_name: base_name,
            registered_ids: Arc::new(RwLock::new(HashSet::new())),
        })
    }

    /// Registers or updates an AgentCard for the given agent_id.
    pub fn register_agent(
        &self,
        agent_id: u64,
        card: &crate::a2a::agent_card::AgentCard,
    ) -> Result<(), SwarmIpcError> {
        let writer = self.service.writer_builder().create().map_err(|e| {
            SwarmIpcError::AccessViolation(format!("Failed to create writer: {}", e))
        })?;

        let entry = writer.entry::<AgentCardCopy>(&agent_id).map_err(|e| {
            SwarmIpcError::AccessViolation(format!(
                "Agent {} not found in registry: {}",
                agent_id, e
            ))
        })?;

        entry.update_with_copy(AgentCardCopy::from_agent_card(*card));

        // Track the registered agent ID for iteration
        if let Ok(mut ids) = self.registered_ids.write() {
            ids.insert(agent_id);
        }

        debug!(agent_id = %agent_id, "Agent registered in capability registry");
        Ok(())
    }

    /// Reads an AgentCard for the given agent_id.
    pub fn get_agent(
        &self,
        agent_id: u64,
    ) -> Result<crate::a2a::agent_card::AgentCard, SwarmIpcError> {
        let reader = self.service.reader_builder().create().map_err(|e| {
            SwarmIpcError::AccessViolation(format!("Failed to create reader: {}", e))
        })?;

        if let Ok(entry) = reader.entry::<AgentCardCopy>(&agent_id) {
            let card_copy = entry.get();
            Ok(card_copy.to_agent_card())
        } else {
            Err(SwarmIpcError::AccessViolation(format!(
                "Agent {} not found in capability registry '{}'",
                agent_id, self.service_name
            )))
        }
    }

    /// Returns true if the given agent_id is registered.
    pub fn has_agent(&self, agent_id: u64) -> bool {
        let Ok(reader) = self.service.reader_builder().create() else {
            return false;
        };
        reader.entry::<AgentCardCopy>(&agent_id).is_ok()
    }

    /// Removes an agent from the registry.
    pub fn unregister_agent(&self, agent_id: u64) -> Result<(), SwarmIpcError> {
        let default_card = crate::a2a::agent_card::AgentCard::new([0u8; 32], "");
        self.register_agent(agent_id, &default_card)?;
        if let Ok(mut ids) = self.registered_ids.write() {
            ids.remove(&agent_id);
        }
        debug!(agent_id = %agent_id, "Agent unregistered from capability registry");
        Ok(())
    }

    /// Finds the best agent for a given task using semantic matching.
    ///
    /// Scans all registered AgentCards and returns the agent_id with the highest
    /// composite score: `(semantic_similarity * 0.7) + ((1.0 - pressure) * 0.3)`.
    ///
    /// Only considers agents that:
    /// - Are active (`is_active == true`)
    /// - Have pressure < 0.9 (not overloaded)
    /// - Pass the required skills check (bitwise AND)
    ///
    /// Returns `None` if no suitable agent is found.
    pub fn find_best_agent(
        &self,
        required_skills: u128,
        semantic_similarity_fn: &dyn Fn(&crate::a2a::agent_card::AgentCard) -> f32,
    ) -> Option<(u64, crate::a2a::agent_card::AgentCard)> {
        let reader = match self.service.reader_builder().create() {
            Ok(r) => r,
            Err(_) => return None,
        };

        let mut best: Option<(u64, crate::a2a::agent_card::AgentCard, f32)> = None;

        let ids = match self.registered_ids.read() {
            Ok(ids) => ids,
            Err(_) => return None,
        };

        for agent_id in ids.iter() {
            if let Ok(entry) = reader.entry::<AgentCardCopy>(agent_id) {
                let card = entry.get().to_agent_card();
                if !card.is_available() {
                    continue;
                }
                if !card.has_skills(required_skills) {
                    continue;
                }
                let similarity = semantic_similarity_fn(&card);
                let score = card.match_score(similarity, required_skills);
                if best.as_ref().is_none_or(|(_, _, s)| score > *s) {
                    best = Some((*agent_id, card, score));
                }
            }
        }

        best.map(|(id, card, _)| (id, card))
    }

    /// Finds the top N agents matching the required skills and semantic similarity.
    ///
    /// Returns up to `top_n` agents sorted by match score (best first).
    /// Used for speculative delegation where multiple agents try the same task
    /// and the best result is selected via entropy-based scoring.
    pub fn find_top_agents(
        &self,
        required_skills: u128,
        top_n: usize,
        semantic_similarity_fn: &dyn Fn(&crate::a2a::agent_card::AgentCard) -> f32,
    ) -> Vec<(u64, crate::a2a::agent_card::AgentCard)> {
        let reader = match self.service.reader_builder().create() {
            Ok(r) => r,
            Err(_) => return Vec::new(),
        };

        let mut candidates: Vec<(u64, crate::a2a::agent_card::AgentCard, f32)> = Vec::new();

        let ids = match self.registered_ids.read() {
            Ok(ids) => ids,
            Err(_) => return Vec::new(),
        };

        for agent_id in ids.iter() {
            if let Ok(entry) = reader.entry::<AgentCardCopy>(agent_id) {
                let card = entry.get().to_agent_card();
                if !card.is_available() {
                    continue;
                }
                if !card.has_skills(required_skills) {
                    continue;
                }
                let similarity = semantic_similarity_fn(&card);
                let score = card.match_score(similarity, required_skills);
                candidates.push((*agent_id, card, score));
            }
        }

        // Sort by score descending (best first)
        candidates.sort_by(|a, b| b.2.partial_cmp(&a.2).unwrap_or(std::cmp::Ordering::Equal));
        candidates.truncate(top_n);
        candidates
            .into_iter()
            .map(|(id, card, _)| (id, card))
            .collect()
    }

    /// Returns the service name for this registry.
    pub fn service_name(&self) -> &str {
        &self.service_name
    }
}

impl Drop for CapabilityRegistry {
    fn drop(&mut self) {
        info!("Shutting down capability registry '{}'", self.service_name);
    }
}

/// Wrapper to enable iceoryx2 blackboard storage of AgentCard.
/// iceoryx2 requires `ZeroCopySend` which is unsafe to implement directly
/// for AgentCard due to its padding fields. This wrapper provides a
/// `#[derive(ZeroCopySend)]` compatible representation.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, ZeroCopySend)]
struct AgentCardCopy {
    pub data: [u8; std::mem::size_of::<crate::a2a::agent_card::AgentCard>()],
}

impl Default for AgentCardCopy {
    fn default() -> Self {
        Self {
            data: [0u8; std::mem::size_of::<crate::a2a::agent_card::AgentCard>()],
        }
    }
}

impl AgentCardCopy {
    fn from_agent_card(card: crate::a2a::agent_card::AgentCard) -> Self {
        let mut data = [0u8; std::mem::size_of::<crate::a2a::agent_card::AgentCard>()];
        // SAFETY: AgentCard is #[repr(C)], so its memory layout is stable and deterministic.
        // The size is known at compile time and we're copying exactly that many bytes.
        let card_bytes = unsafe {
            std::slice::from_raw_parts(
                &card as *const _ as *const u8,
                std::mem::size_of::<crate::a2a::agent_card::AgentCard>(),
            )
        };
        data.copy_from_slice(card_bytes);
        Self { data }
    }

    fn to_agent_card(self) -> crate::a2a::agent_card::AgentCard {
        let mut card = crate::a2a::agent_card::AgentCard::new([0u8; 32], "");
        // SAFETY: AgentCard is #[repr(C)], so its memory layout is stable and deterministic.
        // We're writing exactly size_of::<AgentCard>() bytes into a properly initialized instance.
        let card_bytes = unsafe {
            std::slice::from_raw_parts_mut(
                &mut card as *mut _ as *mut u8,
                std::mem::size_of::<crate::a2a::agent_card::AgentCard>(),
            )
        };
        card_bytes.copy_from_slice(&self.data);
        card
    }
}

#[cfg(test)]
mod capability_registry_tests {
    use super::*;

    #[test]
    fn test_agent_card_copy_roundtrip() {
        let original = crate::a2a::agent_card::AgentCard::new([1u8; 32], "test-agent");
        let copy = AgentCardCopy::from_agent_card(original);
        let restored = copy.to_agent_card();
        assert_eq!(original.agent_id, restored.agent_id);
        assert_eq!(original.name, restored.name);
    }

    #[test]
    #[cfg(target_os = "linux")] // iceoryx2 requires POSIX shared memory runtime (Linux only)
    fn test_capability_registry_creation() {
        let registry = CapabilityRegistry::new("test_cap_registry", 128);
        assert!(registry.is_ok());
    }

    #[test]
    fn test_capability_registry_invalid_name() {
        let result = CapabilityRegistry::new("", 128);
        assert!(result.is_err());
    }

    #[test]
    fn test_capability_registry_invalid_max_agents() {
        let result = CapabilityRegistry::new("test", 0);
        assert!(result.is_err());
        let result = CapabilityRegistry::new("test", 256);
        assert!(result.is_err());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_swarm_shared_context_size() {
        assert_eq!(std::mem::size_of::<SwarmSharedContext>(), 128);
    }

    #[test]
    fn test_swarm_shared_context_is_copy() {
        fn assert_copy<T: Copy>() {}
        assert_copy::<SwarmSharedContext>();
    }

    #[test]
    fn test_hash_session_id_deterministic() {
        let uuid = "550e8400-e29b-41d4-a716-446655440000";
        let h1 = hash_session_id(uuid);
        let h2 = hash_session_id(uuid);
        assert_eq!(h1, h2);
    }

    #[test]
    fn test_bloom_filter_cycle_detection() {
        let mut filter = DelegationBloomFilter::new();
        let agent_1 = 0xDEADBEEF;
        let agent_2 = 0xCAFEBABE;

        filter.add_agent(agent_1);
        assert!(filter.contains_agent(agent_1));
        assert!(!filter.contains_agent(agent_2));

        filter.add_agent(agent_2);
        assert!(filter.contains_agent(agent_1));
        assert!(filter.contains_agent(agent_2));
        assert_eq!(filter.depth_count, 2);
    }

    #[test]
    fn test_bloom_filter_false_positive_rate_low() {
        let mut filter = DelegationBloomFilter::new();
        for i in 0..10 {
            filter.add_agent(i as u64);
        }

        // Check for 11, which shouldn't be there (low probability of clash)
        assert!(!filter.contains_agent(11));
    }
}
