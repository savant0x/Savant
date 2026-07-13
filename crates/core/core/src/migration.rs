use crate::types::{AgentConfig, AgentTier, ChatMessage, ChatRole, ModelProvider};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::str::FromStr;

/// Legacy OpenClaw Agent Configuration (JSON Shape)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LegacyOpenClawConfig {
    pub id: String,
    pub name: String,
    pub model: String,
    pub provider: String,
    pub api_key: Option<String>,
    pub skills: Vec<String>,
    pub workspace: String,
}

impl From<LegacyOpenClawConfig> for AgentConfig {
    fn from(legacy: LegacyOpenClawConfig) -> Self {
        let provider = ModelProvider::from_str(&legacy.provider).unwrap_or(ModelProvider::Local);

        AgentConfig {
            agent_id: legacy.id,
            agent_name: legacy.name,
            model_provider: provider,
            api_key: legacy.api_key,
            env_vars: HashMap::new(),
            system_prompt: "You are a migrated OpenClaw agent.".to_string(),
            model: Some(legacy.model),
            heartbeat_interval: 600, // Legacy default was 10 mins
            allowed_skills: legacy.skills,
            workspace_path: std::path::PathBuf::from(legacy.workspace),
            identity: None,
            parent_id: None,
            session_id: None,
            proactive: crate::config::ProactiveConfig::default(),
            llm_params: crate::types::LlmParams::default(),
            personality_traits: None,
            evolution_state: None,
            orchestrator_enabled: true,
            tier: AgentTier::Full,
        }
    }
}

/// Legacy OpenClaw Message Shape
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LegacyMessage {
    pub role: String,
    pub content: String,
    pub sender: String,
}

impl From<LegacyMessage> for ChatMessage {
    fn from(legacy: LegacyMessage) -> Self {
        let role = match legacy.role.as_str() {
            "user" => ChatRole::User,
            "system" => ChatRole::System,
            _ => ChatRole::Assistant,
        };

        ChatMessage {
            is_telemetry: false,
            role,
            content: legacy.content,
            sender: Some(legacy.sender),
            recipient: None,
            agent_id: None,
            session_id: None,
            channel: crate::types::AgentOutputChannel::Chat,
            images: Vec::new(),
            is_error: false,
        }
    }
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)]
mod tests {
    use super::*;

    #[test]
    fn test_legacy_config_migration() {
        let legacy = LegacyOpenClawConfig {
            id: "test-1".to_string(),
            name: "Tester".to_string(),
            model: "gpt-4".to_string(),
            provider: "openai".to_string(),
            api_key: Some("sk-123".to_string()),
            skills: vec!["fs".to_string()],
            workspace: "/tmp".to_string(),
        };

        let savant: AgentConfig = legacy.into();
        assert_eq!(savant.agent_id, "test-1");
        assert_eq!(savant.model_provider, ModelProvider::OpenAi);
        assert_eq!(savant.heartbeat_interval, 600);
    }
}
