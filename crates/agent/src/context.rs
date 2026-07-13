use crate::budget::TokenBudget;
use savant_core::types::{AgentIdentity, ChatMessage, ChatRole};
use savant_security::prompt_defense;
use tracing::warn;

/// Assembler struct used to construct LLM prompts with token limits in mind.
pub struct ContextAssembler {
    identity: AgentIdentity,
    budget: TokenBudget,
    skills_list: Option<String>,
    substrate_prompt: String,
    auto_recall_block: Option<String>,
    substrate_metrics: String,
    /// Rendered user preferences from FacetCache (injected between instructions and auto-recall).
    user_preferences_block: Option<String>,
}

impl ContextAssembler {
    /// Creates a new ContextAssembler.
    pub fn new(
        identity: AgentIdentity,
        budget: TokenBudget,
        skills_list: Option<String>,
        substrate_prompt: String,
        substrate_metrics: String,
    ) -> Self {
        Self {
            identity,
            budget,
            skills_list,
            substrate_prompt,
            auto_recall_block: None,
            substrate_metrics,
            user_preferences_block: None,
        }
    }

    /// Sets the auto-recall context block for injection into the system prompt.
    pub fn with_auto_recall(mut self, block: String) -> Self {
        self.auto_recall_block = Some(block);
        self
    }

    /// Updates the auto-recall block dynamically (e.g. from memory retrieval each iteration).
    pub fn set_auto_recall(&mut self, block: String) {
        self.auto_recall_block = Some(block);
    }

    /// Updates the skills list (e.g. after tool filtering).
    pub fn update_skills_list(&mut self, skills_list: Option<String>) {
        self.skills_list = skills_list;
    }

    /// Sets the user preferences block for injection into the system prompt.
    /// Rendered from FacetCache::stable_facets() via FacetExtractor::render_preferences().
    pub fn set_user_preferences(&mut self, block: String) {
        if block.is_empty() {
            self.user_preferences_block = None;
        } else {
            self.user_preferences_block = Some(block);
        }
    }

    /// NS-04: Returns a reference to the agent's OCEAN personality traits, if configured.
    pub fn personality_traits(&self) -> Option<&savant_core::types::PersonalityTraits> {
        self.identity.personality_traits.as_ref()
    }

    /// Assembles the full system prompt from identity components (OpenClaw style).
    pub fn assemble_system_prompt(&self) -> String {
        let mut prompt = String::new();

        // 0. Substrate Operational Directive (The House Rules)
        prompt.push_str(&format!(
            "SUBSTRATE OPERATIONAL DIRECTIVE:\n{}\n\n",
            self.substrate_prompt
        ));

        // 0.5. Real system metrics — grounds the agent in observable reality.
        // All memory/CPU numbers below are deterministic (sysinfo crate).
        // DO NOT FABRICATE metrics. Either cite these numbers or say you don't know.
        if !self.substrate_metrics.is_empty() {
            prompt.push_str(&format!("{}\n\n", self.substrate_metrics));
        }

        // 1. Identity & Vibe (IDENTITY.md)
        if let Some(metadata) = &self.identity.metadata {
            prompt.push_str(&format!("IDENTITY INFO:\n{}\n\n", metadata));
        }

        // 2. Persona & Core (SOUL.md)
        prompt.push_str(&format!("PERSONA (SOUL):\n{}\n\n", self.identity.soul));

        // 2.5 Evolution State (personality growth tracking)
        if let Some(traits) = &self.identity.personality_traits {
            prompt.push_str(&format!(
                "EVOLUTION STATE:\n\
                 Your personality is evolving through interaction with your user.\n\
                 Current OCEAN traits: Openness={:.2} Conscientiousness={:.2} Extraversion={:.2} Agreeableness={:.2} Neuroticism={:.2}\n\
                 Baseline hash: {}\n\
                 Your identity grows with each conversation. You are not static.\n\n",
                traits.openness, traits.conscientiousness, traits.extraversion, traits.agreeableness, traits.neuroticism,
                self.identity.baseline_soul_hash.as_deref().unwrap_or("none")
            ));
        }

        // 3. Operating Instructions (AGENTS.md)
        if let Some(instructions) = &self.identity.instructions {
            prompt.push_str(&format!("OPERATING INSTRUCTIONS:\n{}\n\n", instructions));
        }

        // 3.5 User Preferences (learned from conversation history via FacetExtractor)
        if let Some(prefs) = &self.user_preferences_block {
            prompt.push_str(prefs);
            prompt.push_str("\n\n");
        }

        // 3.6 Auto-Recall Context (injected memories from semantic search)
        if let Some(recall) = &self.auto_recall_block {
            prompt.push_str(recall);
        }

        // 4. User context (USER.md)
        if let Some(user) = &self.identity.user_context {
            prompt.push_str(&format!("USER CONTEXT:\n{}\n\n", user));
        }

        if let Some(mission) = &self.identity.mission {
            prompt.push_str(&format!("MISSION:\n{}\n\n", mission));
        }

        if let Some(ethics) = &self.identity.ethics {
            prompt.push_str(&format!("ETHICS & CONSTRAINTS:\n{}\n\n", ethics));
        }

        // 4.5. Coding skills are hot-loaded on demand, not embedded in system prompt

        // 4.6. Universal Perfection Loop (Toggleable via Settings)
        let perfection_enabled = self
            .identity
            .internal_settings
            .as_ref()
            .and_then(|m| m.get("perfection_loop"))
            .map(|v| v != "false")
            .unwrap_or(true); // Default to true

        if perfection_enabled {
            prompt.push_str("AUTONOMOUS PERFECTION LOOP (ACTIVE):\n");
            prompt.push_str(crate::prompts::PERFECTION_LOOP);
            prompt.push_str("\n\n");
        }

        prompt.push_str(&format!(
            "OPERATIONAL LIMITS:\n- Token Budget: {} / {}\n\n",
            self.budget.used, self.budget.limit
        ));

        if let Some(skills) = &self.skills_list {
            prompt.push_str(&format!("AVAILABLE TOOLS:\n{}\n\n", skills));
            prompt.push_str("TOOL USAGE FORMAT:\n");
            prompt.push_str("Use the tool calling format provided by the API when available.\n");
            prompt.push_str("As a fallback, you can also use this text format in your response:\n");
            prompt.push_str("Action: tool_name{\"key\": \"value\"}\n\n");
            prompt.push_str("Examples:\n");
            prompt.push_str(
                "  Action: foundation{\"action\": \"read\", \"path\": \"src/main.rs\"}\n",
            );
            prompt.push_str("  Action: shell{\"command\": \"ls -la\"}\n");
            prompt.push_str(
                "  Action: file_create{\"path\": \"new_file.txt\", \"content\": \"Hello world\"}\n",
            );
            prompt.push_str("  Action: file_move{\"from\": \"old.txt\", \"to\": \"new.txt\"}\n");
            prompt.push_str("  Action: file_delete{\"path\": \"tmp/old.log\"}\n");
            prompt.push_str("  Action: shell{\"command\": \"cargo check\"}\n\n");
        }

        if !self.identity.expertise.is_empty() {
            prompt.push_str("EXPERTISE:\n");
            for skill in &self.identity.expertise {
                prompt.push_str(&format!("- {}\n", skill));
            }
            prompt.push('\n');
        }

        // 5. Global Constraints
        prompt.push_str("Respond in the same language as the user's message.\n\n");

        prompt
    }

    /// Maximum number of messages in the context window.
    const MAX_MESSAGES: usize = 200;
    /// Maximum total characters across all messages.
    const MAX_TOTAL_CHARS: usize = 500_000;

    /// Converts the conversation history and memory into ChatMessages.
    /// RC-11: Enforces maximum message count and total character limits.
    pub fn build_messages(&self, history: Vec<ChatMessage>) -> Vec<ChatMessage> {
        let mut messages = Vec::new();

        messages.push(ChatMessage {
            is_telemetry: false,
            role: ChatRole::System,
            content: self.assemble_system_prompt(),
            sender: Some("SYSTEM".to_string()),
            recipient: None,
            agent_id: None,
            session_id: None,
            channel: savant_core::types::AgentOutputChannel::Chat,
            images: Vec::new(),
            ..Default::default()
        });

        for msg in history {
            if msg.channel == savant_core::types::AgentOutputChannel::Chat
                || msg.channel == savant_core::types::AgentOutputChannel::Memory
            {
                let scan = prompt_defense::scan_prompt(&msg.content);
                if !scan.passed {
                    if let Some(first) = scan.blocked.first() {
                        warn!(
                            "[context] Prompt injection blocked in {} message: {}",
                            msg.role, first.pattern
                        );
                    }
                }
                let mut sanitized = msg;
                if !scan.sanitized_text.is_empty() {
                    sanitized.content = scan.sanitized_text;
                }
                messages.push(sanitized);
            }
        }

        // RC-11: Enforce message count limit (keep system prompt + most recent messages)
        if messages.len() > Self::MAX_MESSAGES {
            let system_msg = messages.remove(0);
            let excess = messages.len() - (Self::MAX_MESSAGES - 1);
            messages.drain(..excess);
            messages.insert(0, system_msg);
            tracing::warn!(
                "[context] Truncated {} excess messages to stay within MAX_MESSAGES={}",
                excess,
                Self::MAX_MESSAGES
            );
        }

        // RC-11: Enforce total character limit
        let total_chars: usize = messages.iter().map(|m| m.content.chars().count()).sum();
        if total_chars > Self::MAX_TOTAL_CHARS {
            let mut accumulated = 0;
            // Keep system prompt and scan from the end, dropping oldest messages
            let system_msg = messages.remove(0);
            let mut kept = Vec::new();
            for msg in messages.into_iter().rev() {
                let msg_chars = msg.content.chars().count();
                if accumulated + msg_chars > Self::MAX_TOTAL_CHARS {
                    break;
                }
                accumulated += msg_chars;
                kept.push(msg);
            }
            kept.reverse();
            kept.insert(0, system_msg);
            messages = kept;
            tracing::warn!(
                "[context] Truncated messages to fit within MAX_TOTAL_CHARS={}",
                Self::MAX_TOTAL_CHARS
            );
        }

        messages
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use savant_core::types::AgentIdentity;

    #[test]
    fn test_assemble_system_prompt() {
        let identity = AgentIdentity {
            name: "TestAgent".to_string(),
            soul: "Vibe check.".to_string(),
            instructions: Some("Do stuff.".to_string()),
            user_context: None,
            metadata: Some("Emoji: 🤖".to_string()),
            mission: None,
            expertise: vec!["Rust".to_string()],
            ethics: None,
            image: None,
            internal_settings: None,
            personality_traits: None,
            baseline_soul_hash: None,
        };
        let budget = TokenBudget::new(100);
        let assembler = ContextAssembler::new(
            identity,
            budget,
            None,
            "House Rules.".to_string(),
            String::new(),
        );
        let prompt = assembler.assemble_system_prompt();

        assert!(prompt.contains("Vibe check."));
        assert!(prompt.contains("Do stuff."));
        assert!(prompt.contains("🤖"));
        assert!(prompt.contains("Rust"));
    }
}
