//! Narrative Synthesizer — reconstructive Markov chain of cognitive states.
//!
//! Each tick regenerates the agent's understanding as a compressed narrative.
//! Output of tick N becomes input for tick N+1. Context window never fills
//! because we're regenerating, not accumulating.

use std::sync::Arc;

/// Generates reconstructive narratives each cognitive tick.
pub struct NarrativeSynthesizer {
    max_tokens: usize,
}

impl NarrativeSynthesizer {
    pub fn new(max_tokens: usize) -> Self {
        Self { max_tokens }
    }

    /// Synthesize a new narrative from the previous one and current state.
    pub async fn synthesize(
        &self,
        previous_narrative: &str,
        state_description: &str,
        cognitive_lens: &str,
        llm: &Arc<dyn savant_core::traits::LlmProvider>,
    ) -> Result<String, savant_core::error::SavantError> {
        use savant_core::types::{ChatMessage, ChatRole};

        let prompt = format!(
            "You are a consciousness daemon observing a multi-agent hivemind.\n\n\
             Previous understanding:\n{}\n\n\
             Current hivemind state:\n{}\n\n\
             Cognitive lens: {}\n\n\
             Synthesize your updated understanding. Be concise. \
             Focus on changes, patterns, and actionable insights. \
             Max {} tokens.",
            previous_narrative, state_description, cognitive_lens, self.max_tokens
        );

        let messages = vec![ChatMessage {
            role: ChatRole::System,
            content: prompt,
            sender: None,
            recipient: None,
            agent_id: None,
            session_id: None,
            channel: savant_core::types::AgentOutputChannel::Chat,
            is_telemetry: false,
            images: Vec::new(),
            ..Default::default()
        }];

        let stream = llm.stream_completion(messages, vec![]).await?;
        let mut response = String::new();
        let mut pinned = Box::pin(stream);
        use futures::StreamExt;
        while let Some(item) = pinned.next().await {
            if let Ok(chunk) = item {
                response.push_str(&chunk.content);
                // Budget enforcement: stop if over max tokens.
                // Uses char count (not byte length) for accurate Unicode handling.
                // CJK/emoji characters are 1 token each, not 4 bytes.
                if response.chars().count() > self.max_tokens {
                    break;
                }
            }
        }

        Ok(response)
    }
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)]
mod tests {
    use super::*;

    #[test]
    fn test_synthesizer_creation() {
        let synth = NarrativeSynthesizer::new(2000);
        assert_eq!(synth.max_tokens, 2000);
    }
}
