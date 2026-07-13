//! A2A (Agent-to-Agent) Communication Layer.
//!
//! Provides typed, structured inter-agent communication for task delegation,
//! capability advertisement, result routing, and memory-aware context passing.
//!
//! All types are `#[repr(C)]` and rkyv-serialized for zero-copy shared memory IPC
//! via iceoryx2. This replaces the fragile text-based `/subagents spawn` pattern
//! with a structured protocol that integrates with Savant's "glass house" memory
//! system (CortexaDB, Obsidian vault, 4-graph reflective memory).

pub mod agent_card;
pub mod context;
pub mod protocol;
pub mod queues;
pub mod result_router;

pub use agent_card::AgentCard;
pub use context::ContextPackage;
pub use protocol::{
    A2AEnvelope, A2AMessageType, Artifact, ArtifactPart, ArtifactPartType, DelegationTask,
    TaskState,
};
pub use queues::{
    AgentTaskQueue, TaskQueueError, DEFAULT_QUEUE_CAPACITY, MAX_QUEUE_RETRIES,
    QUEUE_FULL_BACKOFF_MS,
};
pub use result_router::{
    DelegationResult, RejectionReason, ResultRouter, ResultRouterError, TaskStatusUpdate,
};
