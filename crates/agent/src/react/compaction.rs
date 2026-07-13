//! Context Compaction — prevents context overflow on long conversations.
//!
//! Three strategies selected by usage ratio (configurable via L2Thresholds):
//! - tool_eviction (default 75%): MoveToWorkspace — archive old messages to daily log
//! - llm_summarization (default 85%): Summarize — LLM bullet-point summary, keep recent
//! - emergency (default 95%): Truncate — aggressive, keep only recent turns
//!
//! NS-02: Thresholds are configurable via `L2Compressor`.
//! NS-04: Thresholds are personality-adjusted via `OceanScaler`.

use savant_core::types::{ChatMessage, ChatRole};

/// Configurable L2 thresholds for context compaction.
/// Mirrors `compact::l2::L2Thresholds` but defined here to avoid circular deps.
#[derive(Debug, Clone)]
pub struct L2CompactionThresholds {
    /// Threshold for tool eviction / move-to-workspace (default: 0.75).
    pub tool_eviction: f32,
    /// Threshold for LLM summarization (default: 0.85).
    pub llm_summarization: f32,
    /// Emergency threshold for aggressive truncation (default: 0.95).
    pub emergency: f32,
}

impl Default for L2CompactionThresholds {
    fn default() -> Self {
        Self {
            tool_eviction: 0.75,
            llm_summarization: 0.85,
            emergency: 0.95,
        }
    }
}

impl From<crate::compact::l2::L2Thresholds> for L2CompactionThresholds {
    fn from(t: crate::compact::l2::L2Thresholds) -> Self {
        Self {
            tool_eviction: t.tool_eviction,
            llm_summarization: t.llm_summarization,
            emergency: t.emergency,
        }
    }
}

/// Compaction strategy selected based on context usage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompactionStrategy {
    /// Archive old messages to workspace daily log
    MoveToWorkspace,
    /// LLM-based summarization of old messages
    Summarize,
    /// Aggressive truncation — keep only recent turns
    Truncate,
}

/// Estimate token count for a message.
/// Uses char_count / 4 which is more accurate for code, JSON, and structured output
/// than the word-count-based formula. This approximates tiktoken-rs behavior
/// without the dependency overhead.
pub fn estimate_message_tokens(msg: &ChatMessage) -> usize {
    let char_count = msg.content.chars().count();
    // Each token is roughly 4 characters for English text, code, and JSON.
    // Add 4 for message overhead (role, separators).
    (char_count / 4) + 4
}

/// Estimate total tokens across all messages.
pub fn estimate_total_tokens(messages: &[ChatMessage]) -> usize {
    messages.iter().map(estimate_message_tokens).sum()
}

/// Context monitor — decides when and how to compact.
/// NS-02: Uses configurable L2 thresholds instead of hardcoded values.
pub struct ContextMonitor {
    /// Model's context window in tokens
    context_limit: usize,
    /// NS-02: Configurable thresholds for staged compression.
    thresholds: L2CompactionThresholds,
}

impl ContextMonitor {
    pub fn new(context_limit: usize) -> Self {
        Self {
            context_limit,
            thresholds: L2CompactionThresholds::default(),
        }
    }

    /// NS-02: Creates a monitor with custom L2 thresholds (e.g., OceanScaler-adjusted).
    pub fn with_thresholds(context_limit: usize, thresholds: L2CompactionThresholds) -> Self {
        Self {
            context_limit,
            thresholds,
        }
    }

    /// Current usage ratio (0.0 = empty, 1.0 = full).
    pub fn usage_ratio(&self, messages: &[ChatMessage]) -> f64 {
        if self.context_limit == 0 {
            return 1.0;
        }
        estimate_total_tokens(messages) as f64 / self.context_limit as f64
    }

    /// Suggest a compaction strategy based on current usage.
    /// NS-02: Uses configurable thresholds from L2Compressor.
    pub fn suggest(&self, messages: &[ChatMessage]) -> Option<CompactionStrategy> {
        let usage = self.usage_ratio(messages);
        let t = &self.thresholds;
        match usage {
            u if u < t.tool_eviction as f64 => None,
            u if u < t.llm_summarization as f64 => Some(CompactionStrategy::MoveToWorkspace),
            u if u < t.emergency as f64 => Some(CompactionStrategy::Summarize),
            _ => Some(CompactionStrategy::Truncate),
        }
    }
}

/// Compactor — executes compaction strategies.
pub struct Compactor;

impl Compactor {
    /// Truncate: keep only the most recent messages.
    pub fn truncate(messages: Vec<ChatMessage>, keep_recent: usize) -> Vec<ChatMessage> {
        if messages.len() <= keep_recent {
            return messages;
        }
        messages[messages.len() - keep_recent..].to_vec()
    }

    /// Move to workspace: archive old messages, keep recent.
    /// Returns (archived_text, recent_messages).
    pub fn partition(messages: Vec<ChatMessage>, keep_recent: usize) -> (String, Vec<ChatMessage>) {
        if messages.len() <= keep_recent {
            return (String::new(), messages);
        }

        let split_idx = messages.len() - keep_recent;
        let archived_text = messages[..split_idx]
            .iter()
            .map(|m| format!("[{:?}] {}", m.role, m.content))
            .collect::<Vec<_>>()
            .join("\n\n");

        let recent = messages[split_idx..].to_vec();
        (archived_text, recent)
    }

    /// Apply compaction strategy to messages.
    ///
    /// For MoveToWorkspace and Summarize: archives old content, returns recent messages.
    /// A system message is injected to inform the LLM that context was compacted.
    pub fn compact(
        messages: Vec<ChatMessage>,
        strategy: CompactionStrategy,
        keep_recent: usize,
    ) -> Vec<ChatMessage> {
        match strategy {
            CompactionStrategy::Truncate => Self::truncate(messages, keep_recent),
            CompactionStrategy::MoveToWorkspace | CompactionStrategy::Summarize => {
                let (archived, mut recent) = Self::partition(messages, keep_recent);

                if !archived.is_empty() {
                    let summary_msg = ChatMessage {
                        is_telemetry: false,
                        role: ChatRole::System,
                        content: format!(
                            "[Context compacted: {} older messages archived. The conversation continues below.]",
                            archived.lines().count().max(1)
                        ),
                        sender: Some("SYSTEM".to_string()),
                        recipient: None,
                        agent_id: None,
                        session_id: None,
                        channel: savant_core::types::AgentOutputChannel::Chat,
                        images: Vec::new(),
                        ..Default::default()
                    };
                    recent.insert(0, summary_msg);
                }

                recent
            }
        }
    }

    /// D5: Like compact() but also returns the archived text for persistence.
    /// Callers should save archived_text to LSM or daily log.
    pub fn compact_with_archive(
        messages: Vec<ChatMessage>,
        strategy: CompactionStrategy,
        keep_recent: usize,
    ) -> (Vec<ChatMessage>, String) {
        match strategy {
            CompactionStrategy::Truncate => (Self::truncate(messages, keep_recent), String::new()),
            CompactionStrategy::MoveToWorkspace | CompactionStrategy::Summarize => {
                let (archived, mut recent) = Self::partition(messages, keep_recent);

                if !archived.is_empty() {
                    let summary_msg = ChatMessage {
                        is_telemetry: false,
                        role: ChatRole::System,
                        content: format!(
                            "[Context compacted: {} older messages archived. The conversation continues below.]",
                            archived.lines().count().max(1)
                        ),
                        sender: Some("SYSTEM".to_string()),
                        recipient: None,
                        agent_id: None,
                        session_id: None,
                        channel: savant_core::types::AgentOutputChannel::Chat,
                        images: Vec::new(),
                        ..Default::default()
                    };
                    recent.insert(0, summary_msg);
                }

                (recent, archived)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_msg(role: ChatRole, content: &str) -> ChatMessage {
        ChatMessage {
            is_telemetry: false,
            role,
            content: content.to_string(),
            sender: None,
            recipient: None,
            agent_id: None,
            session_id: None,
            channel: savant_core::types::AgentOutputChannel::Chat,
            images: Vec::new(),
            ..Default::default()
        }
    }

    #[test]
    fn test_estimate_message_tokens() {
        let msg = make_msg(ChatRole::User, "hello world this is a test");
        let tokens = estimate_message_tokens(&msg);
        assert!(tokens > 0);
        assert!(tokens < 20); // 6 words * 1.3 + 4 ≈ 12
    }

    #[test]
    fn test_estimate_total_tokens() {
        let messages = vec![
            make_msg(ChatRole::User, "hello"),
            make_msg(ChatRole::Assistant, "hi there"),
        ];
        let total = estimate_total_tokens(&messages);
        assert!(total > 0);
    }

    #[test]
    fn test_monitor_no_compaction_needed() {
        let monitor = ContextMonitor::new(100_000);
        let messages = vec![make_msg(ChatRole::User, "short message")];
        assert!(monitor.suggest(&messages).is_none());
    }

    #[test]
    fn test_monitor_suggests_archive() {
        let monitor = ContextMonitor::new(100);
        let messages: Vec<ChatMessage> = (0..30)
            .map(|_| {
                make_msg(
                    ChatRole::User,
                    "this is a moderately long message with several words",
                )
            })
            .collect();
        let strategy = monitor.suggest(&messages);
        assert!(strategy.is_some());
    }

    #[test]
    fn test_truncate() {
        let messages: Vec<ChatMessage> = (0..10)
            .map(|i| make_msg(ChatRole::User, &format!("msg {}", i)))
            .collect();
        let result = Compactor::truncate(messages, 3);
        assert_eq!(result.len(), 3);
        assert_eq!(result[0].content, "msg 7");
    }

    #[test]
    fn test_truncate_no_op() {
        let messages = vec![make_msg(ChatRole::User, "only message")];
        let result = Compactor::truncate(messages, 10);
        assert_eq!(result.len(), 1);
    }

    #[test]
    fn test_partition() {
        let messages: Vec<ChatMessage> = (0..5)
            .map(|i| make_msg(ChatRole::User, &format!("msg {}", i)))
            .collect();
        let (archived, recent) = Compactor::partition(messages, 2);
        assert!(archived.contains("msg 0"));
        assert_eq!(recent.len(), 2);
        assert_eq!(recent[0].content, "msg 3");
    }

    #[test]
    fn test_compact_with_summary_injection() {
        let messages: Vec<ChatMessage> = (0..5)
            .map(|i| make_msg(ChatRole::User, &format!("msg {}", i)))
            .collect();
        let result = Compactor::compact(messages, CompactionStrategy::MoveToWorkspace, 2);
        assert_eq!(result.len(), 3); // summary + 2 recent
        assert_eq!(result[0].role, ChatRole::System);
        assert!(result[0].content.contains("compacted"));
    }

    #[test]
    fn test_compact_truncate() {
        let messages: Vec<ChatMessage> = (0..10)
            .map(|i| make_msg(ChatRole::User, &format!("msg {}", i)))
            .collect();
        let result = Compactor::compact(messages, CompactionStrategy::Truncate, 3);
        assert_eq!(result.len(), 3);
    }
}
