// SAFETY: All `clippy::disallowed_methods` violations in this file originate from
// the `serde_json::json!()` macro, which internally uses `.unwrap()` on
// compile-time-validated JSON literals. A malformed JSON literal would be a
// compile error, making the panic path statically unreachable.
#![allow(clippy::disallowed_methods)]

use async_trait::async_trait;
use savant_core::error::SavantError;
use savant_core::traits::{MemoryBackend, Tool};
use savant_core::types::{ChatMessage, ChatRole};
use serde_json::Value;
use std::sync::Arc;

/// Tool for appending long-term observations to an agent's memory.
/// Prevents raw file clobbering seen in legacy frameworks.
pub struct MemoryAppendTool {
    memory: Arc<dyn MemoryBackend>,
    agent_id: String,
}

impl MemoryAppendTool {
    pub fn new(memory: Arc<dyn MemoryBackend>, agent_id: String) -> Self {
        Self { memory, agent_id }
    }
}

#[async_trait]
impl Tool for MemoryAppendTool {
    fn name(&self) -> &str {
        "memory_append"
    }

    fn description(&self) -> &str {
        "Store a fact, observation, or learning in long-term memory for future reference."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "content": { "type": "string", "description": "The observation or fact to remember" }
            },
            "required": ["content"]
        })
    }

    async fn execute(&self, payload: Value) -> Result<String, SavantError> {
        let content = payload["content"].as_str().ok_or_else(|| {
            SavantError::Unknown("Missing 'content' field in payload".to_string())
        })?;

        let msg = ChatMessage {
            is_telemetry: false,
            role: ChatRole::Assistant,
            content: content.to_string(),
            sender: Some(self.agent_id.clone()),
            recipient: None,
            agent_id: None,
            session_id: None, // Will be prioritized by Backend if None
            channel: savant_core::types::AgentOutputChannel::Memory,
            images: Vec::new(),
            ..Default::default()
        };

        self.memory.store(&self.agent_id, &msg).await?;

        Ok(format!(
            "Successfully archived observation for agent {}.",
            self.agent_id
        ))
    }

    fn capabilities(&self) -> savant_core::types::CapabilityGrants {
        savant_core::types::CapabilityGrants {
            fs_read: std::collections::HashSet::new(),
            fs_write: [std::path::PathBuf::from("memory")].into_iter().collect(),
            ..Default::default()
        }
    }
}

/// Tool for semantic search across archived session history and memories.
pub struct MemorySearchTool {
    memory: Arc<dyn MemoryBackend>,
    agent_id: String,
}

impl MemorySearchTool {
    pub fn new(memory: Arc<dyn MemoryBackend>, agent_id: String) -> Self {
        Self { memory, agent_id }
    }
}

#[async_trait]
impl Tool for MemorySearchTool {
    fn name(&self) -> &str {
        "memory_search"
    }

    fn description(&self) -> &str {
        "Search past conversations and stored memories to find relevant context."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "query": { "type": "string", "description": "Search query to find relevant memories" }
            },
            "required": ["query"]
        })
    }

    async fn execute(&self, payload: Value) -> Result<String, SavantError> {
        let query = payload["query"]
            .as_str()
            .ok_or_else(|| SavantError::Unknown("Missing 'query' field in payload".to_string()))?;

        // Semantic search using the backend
        let messages = self.memory.retrieve(&self.agent_id, query, 10).await?;
        let mut response = String::from("Relevant historical entries:\n");
        for m in messages {
            response.push_str(&format!(
                "[{}] {}: {}\n",
                m.role,
                m.sender.unwrap_or_default(),
                m.content
            ));
        }

        Ok(response)
    }

    fn capabilities(&self) -> savant_core::types::CapabilityGrants {
        savant_core::types::CapabilityGrants {
            fs_read: [std::path::PathBuf::from("memory")].into_iter().collect(),
            fs_write: std::collections::HashSet::new(),
            ..Default::default()
        }
    }
}
