//! Agent Card — Capability advertisement and semantic matching.
//!
//! Each agent publishes an AgentCard to the iceoryx2 Capability Blackboard.
//! The Orchestrator scans these cards to find the best agent for a delegated task
//! using semantic similarity + pressure scoring + skill verification.

use rkyv::{Archive, Deserialize, Serialize};

/// Capability advertisement for a single agent.
///
/// Stored in the iceoryx2 Capability Blackboard (key-value store).
/// The key is the agent's FNV-1a hash. The value is this struct.
///
/// Size: 176 bytes (aligned for iceoryx2)
#[repr(C)]
#[derive(Debug, Copy, Clone, PartialEq, Archive, Serialize, Deserialize)]
pub struct AgentCard {
    pub agent_id: [u8; 32],
    pub name: [u8; 64],
    pub description_vector_id: u64,
    pub allowed_skills_mask: u128,
    pub input_modes: u8,
    pub output_modes: u8,
    pub pressure: f32,
    pub total_successes: u32,
    pub total_failures: u32,
    pub protocol_version: u16,
    pub is_active: bool,
    pub memory_enclave_id: u64,
    pub max_concurrent_tasks: u8,
    pub avg_task_duration_ms: u32,
    pub _padding: [u8; 7],
}

impl AgentCard {
    pub fn new(agent_id: [u8; 32], name: &str) -> Self {
        let mut name_bytes = [0u8; 64];
        let len = name.len().min(64);
        name_bytes[..len].copy_from_slice(&name.as_bytes()[..len]);
        Self {
            agent_id,
            name: name_bytes,
            description_vector_id: 0,
            allowed_skills_mask: 0,
            input_modes: 0x01,  // Text by default
            output_modes: 0x01, // Text by default
            pressure: 0.0,
            total_successes: 0,
            total_failures: 0,
            protocol_version: 0x0100,
            is_active: false,
            memory_enclave_id: 0,
            max_concurrent_tasks: 1,
            avg_task_duration_ms: 0,
            _padding: [0u8; 7],
        }
    }

    /// Returns true if this agent can handle the given input mode.
    pub fn accepts_input(&self, mode: u8) -> bool {
        self.input_modes & mode != 0
    }

    /// Returns true if this agent can produce the given output mode.
    pub fn produces_output(&self, mode: u8) -> bool {
        self.output_modes & mode != 0
    }

    /// Returns true if this agent has the required skills (bitmask check).
    pub fn has_skills(&self, required_mask: u128) -> bool {
        self.allowed_skills_mask & required_mask == required_mask
    }

    /// Returns true if this agent is available for new tasks.
    pub fn is_available(&self) -> bool {
        self.is_active && self.pressure < 0.9
    }

    /// Computes a composite score for task-agent matching.
    /// Higher is better. Range: [0.0, 1.0]
    pub fn match_score(&self, semantic_similarity: f32, required_skills: u128) -> f32 {
        if !self.is_available() {
            return 0.0;
        }
        if !self.has_skills(required_skills) {
            return 0.0;
        }
        let pressure_factor = 1.0 - self.pressure;
        // 70% semantic match, 30% availability
        (semantic_similarity * 0.7) + (pressure_factor * 0.3)
    }

    /// Updates the agent's pressure metric (0.0 = idle, 1.0 = overloaded).
    pub fn update_pressure(&mut self, active_tasks: u8) {
        self.pressure = if self.max_concurrent_tasks == 0 {
            0.0
        } else {
            (active_tasks as f32 / self.max_concurrent_tasks as f32).min(1.0)
        };
    }

    /// Records a task completion (success or failure).
    pub fn record_completion(&mut self, success: bool, duration_ms: u32) {
        if success {
            self.total_successes = self.total_successes.saturating_add(1);
        } else {
            self.total_failures = self.total_failures.saturating_add(1);
        }
        // Rolling average of task duration
        let total = self.total_successes + self.total_failures;
        if total > 0 {
            self.avg_task_duration_ms =
                (self.avg_task_duration_ms * (total - 1) + duration_ms) / total;
        }
    }
}

/// Input mode flags for AgentCard.
pub mod input_modes {
    pub const TEXT: u8 = 0x01;
    pub const MEMORY_GRAPH: u8 = 0x02;
    pub const TOOL_OUTPUT: u8 = 0x04;
}

/// Output mode flags for AgentCard.
pub mod output_modes {
    pub const TEXT: u8 = 0x01;
    pub const JSON: u8 = 0x02;
    pub const ARTIFACT: u8 = 0x04;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_agent_card_size() {
        assert_eq!(std::mem::size_of::<AgentCard>(), 176);
    }

    #[test]
    fn test_agent_card_new() {
        let card = AgentCard::new([1u8; 32], "test-agent");
        assert_eq!(card.agent_id, [1u8; 32]);
        assert_eq!(&card.name[..10], b"test-agent");
        assert!(!card.is_active);
        assert_eq!(card.pressure, 0.0);
    }

    #[test]
    fn test_agent_card_availability() {
        let mut card = AgentCard::new([1u8; 32], "test");
        assert!(!card.is_available()); // Not active
        card.is_active = true;
        assert!(card.is_available());
        card.pressure = 0.95;
        assert!(!card.is_available()); // Overloaded
    }

    #[test]
    fn test_agent_card_skills() {
        let mut card = AgentCard::new([1u8; 32], "test");
        card.allowed_skills_mask = 0b1010;
        assert!(card.has_skills(0b0010));
        assert!(card.has_skills(0b1000));
        assert!(card.has_skills(0b1010));
        assert!(!card.has_skills(0b1111));
    }

    #[test]
    fn test_agent_card_match_score() {
        let mut card = AgentCard::new([1u8; 32], "test");
        card.is_active = true;
        card.allowed_skills_mask = 0b1111;
        card.max_concurrent_tasks = 4;
        card.update_pressure(1);
        let score = card.match_score(0.8, 0b0101);
        assert!(score > 0.0 && score <= 1.0);
    }

    #[test]
    fn test_agent_card_pressure() {
        let mut card = AgentCard::new([1u8; 32], "test");
        card.max_concurrent_tasks = 4;
        card.update_pressure(0);
        assert_eq!(card.pressure, 0.0);
        card.update_pressure(2);
        assert_eq!(card.pressure, 0.5);
        card.update_pressure(4);
        assert_eq!(card.pressure, 1.0);
        card.update_pressure(8); // Should cap at 1.0
        assert_eq!(card.pressure, 1.0);
    }

    #[test]
    fn test_agent_card_completion_tracking() {
        let mut card = AgentCard::new([1u8; 32], "test");
        card.record_completion(true, 1000);
        card.record_completion(true, 2000);
        card.record_completion(false, 500);
        assert_eq!(card.total_successes, 2);
        assert_eq!(card.total_failures, 1);
        assert_eq!(card.avg_task_duration_ms, 1166);
    }

    #[test]
    fn test_input_output_modes() {
        let mut card = AgentCard::new([1u8; 32], "test");
        card.input_modes = input_modes::TEXT | input_modes::MEMORY_GRAPH;
        card.output_modes = output_modes::TEXT | output_modes::JSON;
        assert!(card.accepts_input(input_modes::TEXT));
        assert!(card.accepts_input(input_modes::MEMORY_GRAPH));
        assert!(!card.accepts_input(input_modes::TOOL_OUTPUT));
        assert!(card.produces_output(output_modes::TEXT));
        assert!(card.produces_output(output_modes::JSON));
        assert!(!card.produces_output(output_modes::ARTIFACT));
    }
}
