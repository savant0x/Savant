use savant_core::types::ChatMessage;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

/// E2: Structured checkpoint from context compaction (zot format).
/// Contains 6 sections that preserve critical context during compression.
#[derive(Debug, Clone, Default)]
pub struct StructuredCheckpoint {
    pub goal: String,
    pub constraints: String,
    pub progress: String,
    pub decisions: String,
    pub next_steps: String,
    pub critical_context: String,
}

impl StructuredCheckpoint {
    /// Parse a raw LLM response into a StructuredCheckpoint by section headers.
    pub fn parse(raw: &str) -> Self {
        let mut checkpoint = Self::default();
        let mut current_section = "";
        for line in raw.lines() {
            let trimmed = line.trim();
            if trimmed.to_lowercase().starts_with("[goal") {
                current_section = "goal";
            } else if trimmed.to_lowercase().starts_with("[constraint") {
                current_section = "constraints";
            } else if trimmed.to_lowercase().starts_with("[progress") {
                current_section = "progress";
            } else if trimmed.to_lowercase().starts_with("[decision") {
                current_section = "decisions";
            } else if trimmed.to_lowercase().starts_with("[next") {
                current_section = "next_steps";
            } else if trimmed.to_lowercase().starts_with("[critical") {
                current_section = "critical_context";
            } else if !trimmed.is_empty() && !current_section.is_empty() {
                match current_section {
                    "goal" => {
                        if !checkpoint.goal.is_empty() {
                            checkpoint.goal.push('\n');
                        }
                        checkpoint.goal.push_str(trimmed);
                    }
                    "constraints" => {
                        if !checkpoint.constraints.is_empty() {
                            checkpoint.constraints.push('\n');
                        }
                        checkpoint.constraints.push_str(trimmed);
                    }
                    "progress" => {
                        if !checkpoint.progress.is_empty() {
                            checkpoint.progress.push('\n');
                        }
                        checkpoint.progress.push_str(trimmed);
                    }
                    "decisions" => {
                        if !checkpoint.decisions.is_empty() {
                            checkpoint.decisions.push('\n');
                        }
                        checkpoint.decisions.push_str(trimmed);
                    }
                    "next_steps" => {
                        if !checkpoint.next_steps.is_empty() {
                            checkpoint.next_steps.push('\n');
                        }
                        checkpoint.next_steps.push_str(trimmed);
                    }
                    "critical_context" => {
                        if !checkpoint.critical_context.is_empty() {
                            checkpoint.critical_context.push('\n');
                        }
                        checkpoint.critical_context.push_str(trimmed);
                    }
                    _ => {}
                }
            }
        }
        checkpoint
    }

    /// Render checkpoint back to text for context injection.
    pub fn to_text(&self) -> String {
        let mut out = String::new();
        if !self.goal.is_empty() {
            out.push_str(&format!("[Goal:]\n{}\n\n", self.goal));
        }
        if !self.constraints.is_empty() {
            out.push_str(&format!("[Constraints:]\n{}\n\n", self.constraints));
        }
        if !self.progress.is_empty() {
            out.push_str(&format!("[Progress:]\n{}\n\n", self.progress));
        }
        if !self.decisions.is_empty() {
            out.push_str(&format!("[Decisions:]\n{}\n\n", self.decisions));
        }
        if !self.next_steps.is_empty() {
            out.push_str(&format!("[Next Steps:]\n{}\n\n", self.next_steps));
        }
        if !self.critical_context.is_empty() {
            out.push_str(&format!(
                "[Critical Context:]\n{}\n\n",
                self.critical_context
            ));
        }
        out
    }
}

pub struct ContextCompressor {
    enabled: bool,
    trigger_threshold: f64,
    preserve_head_turns: usize,
    preserve_tail_turns: usize,
    max_summary_tokens: usize,
    cooldown: Duration,
    last_compression: Mutex<Option<Instant>>,
}

impl ContextCompressor {
    pub fn new(
        enabled: bool,
        trigger_threshold: f64,
        preserve_head_turns: usize,
        preserve_tail_turns: usize,
        max_summary_tokens: usize,
        cooldown_seconds: u64,
    ) -> Self {
        ContextCompressor {
            enabled,
            trigger_threshold,
            preserve_head_turns,
            preserve_tail_turns,
            max_summary_tokens,
            cooldown: Duration::from_secs(cooldown_seconds),
            last_compression: Mutex::new(None),
        }
    }

    pub async fn should_compress(
        &self,
        messages: &[ChatMessage],
        current_token_count: usize,
        max_tokens: usize,
    ) -> bool {
        if !self.enabled {
            return false;
        }
        let threshold = (max_tokens as f64 * self.trigger_threshold) as usize;
        if current_token_count < threshold {
            return false;
        }
        if messages.len() <= self.preserve_head_turns + self.preserve_tail_turns {
            return false;
        }
        let mut last = self.last_compression.lock().await;
        if let Some(prev) = *last {
            if prev.elapsed() < self.cooldown {
                return false;
            }
        }
        *last = Some(Instant::now());
        true
    }

    pub fn partition<'a>(
        &self,
        messages: &'a [ChatMessage],
    ) -> (
        Vec<&'a ChatMessage>,
        Vec<&'a ChatMessage>,
        Vec<&'a ChatMessage>,
    ) {
        let head: Vec<&ChatMessage> = messages.iter().take(self.preserve_head_turns).collect();
        let tail: Vec<&ChatMessage> = messages
            .iter()
            .rev()
            .take(self.preserve_tail_turns)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        let middle: Vec<&ChatMessage> = messages
            .iter()
            .skip(self.preserve_head_turns)
            .take(
                messages
                    .len()
                    .saturating_sub(self.preserve_head_turns + self.preserve_tail_turns),
            )
            .collect();
        (head, middle, tail)
    }

    pub fn build_compression_prompt(middle_messages: &[&ChatMessage]) -> String {
        let conversation: String = middle_messages
            .iter()
            .map(|m| {
                format!(
                    "[{}] {}",
                    match m.role {
                        savant_core::types::ChatRole::User => "USER",
                        savant_core::types::ChatRole::Assistant => "ASSISTANT",
                        _ => "SYSTEM",
                    },
                    m.content
                )
            })
            .collect::<Vec<_>>()
            .join("\n");

        format!(
            "Compress this conversation into a structured checkpoint. Output ONLY the checkpoint, no preamble.\n\
            Use EXACTLY these section headers:\n\
            [Goal:]\n\
            [Constraints:]\n\
            [Progress:]\n\
            [Decisions:]\n\
            [Next Steps:]\n\
            [Critical Context:]\n\n\
            Each section should contain concise bullet points.\n\
            Conversation:\n{conversation}"
        )
    }

    /// Estimate token count. Uses div_ceil for consistency with budget.rs.
    pub fn estimate_tokens(text: &str) -> usize {
        text.len().div_ceil(4)
    }

    /// Returns the maximum token count for a compressed summary.
    /// Callers should truncate LLM-generated summaries to this length.
    pub fn max_summary_tokens(&self) -> usize {
        self.max_summary_tokens
    }

    /// D4: Compresses the middle messages by calling the LLM with a structured
    /// summary prompt. Returns the compressed summary text.
    /// Uses checkpoint format: Resolved/Pending/Key Decisions/Context.
    pub async fn compress(
        &self,
        middle_messages: &[&ChatMessage],
        provider: &dyn savant_core::traits::LlmProvider,
    ) -> Result<String, savant_core::error::SavantError> {
        if middle_messages.is_empty() {
            return Ok(String::new());
        }

        let prompt = Self::build_compression_prompt(middle_messages);
        let mut summary = String::new();
        let mut stream = provider
            .stream_completion(
                vec![savant_core::types::ChatMessage {
                    role: savant_core::types::ChatRole::User,
                    content: prompt,
                    ..Default::default()
                }],
                vec![],
            )
            .await?;

        use futures::StreamExt;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            summary.push_str(&chunk.content);
            // Truncate to max summary tokens (approximate)
            if summary.len() > self.max_summary_tokens * 4 {
                break;
            }
        }

        // Update cooldown
        let mut last = self.last_compression.lock().await;
        *last = Some(Instant::now());

        Ok(summary)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_token_estimation() {
        let text = "This is a test of token estimation.";
        let tokens = ContextCompressor::estimate_tokens(text);
        assert!(tokens > 0);
        assert!(tokens < text.len());
    }

    #[test]
    fn test_partition() {
        let compressor = ContextCompressor::new(true, 0.8, 2, 3, 2000, 600);
        let messages: Vec<ChatMessage> = (0..10)
            .map(|i| ChatMessage {
                is_telemetry: false,
                role: savant_core::types::ChatRole::User,
                content: format!("message {i}"),
                sender: None,
                recipient: None,
                agent_id: None,
                session_id: None,
                channel: savant_core::types::AgentOutputChannel::Chat,
                images: Vec::new(),
                ..Default::default()
            })
            .collect();
        let (head, middle, tail) = compressor.partition(&messages);
        assert_eq!(head.len(), 2);
        assert_eq!(tail.len(), 3);
        assert_eq!(middle.len(), 5);
    }
}
