//! Semantic Window — Sliding context window with eviction.
//!
//! Maintains a window of the most relevant message turns.
//! When the window exceeds the threshold, evicts the lowest-scoring
//! non-pinned entries. Evicted entries are written to episodic memory.

use savant_core::types::ChatMessage;
use tracing::info;

use super::scoring::{score_messages, ContextScore};

/// Configuration for the semantic window.
#[derive(Debug, Clone)]
pub struct WindowConfig {
    /// Maximum number of message turns in the window.
    pub max_turns: usize,
    /// Percentage of non-pinned entries to evict when exceeding threshold (0-100).
    pub eviction_pct: usize,
    /// Session duration in minutes before activating streaming mode.
    pub session_threshold_mins: u64,
}

impl Default for WindowConfig {
    fn default() -> Self {
        Self {
            max_turns: 50,
            eviction_pct: 20,
            session_threshold_mins: 30,
        }
    }
}

/// Result of a window management operation.
#[derive(Debug, Clone)]
pub struct WindowResult {
    /// Messages retained in the window.
    pub retained: Vec<ChatMessage>,
    /// Messages evicted from the window.
    pub evicted: Vec<ChatMessage>,
    /// Number of pinned messages (never evicted).
    pub pinned_count: usize,
}

/// Semantic window manager for context selection.
pub struct SemanticWindow {
    config: WindowConfig,
}

impl SemanticWindow {
    /// Creates a new semantic window with the given config.
    pub fn new(config: WindowConfig) -> Self {
        Self { config }
    }

    /// Creates a default semantic window.
    pub fn default_window() -> Self {
        Self::new(WindowConfig::default())
    }

    /// Manages the context window for a conversation.
    ///
    /// # Process
    /// 1. Score all messages for relevance
    /// 2. If under threshold, return all messages
    /// 3. If over threshold, evict lowest-scoring non-pinned entries
    /// 4. Return retained + evicted lists
    pub fn manage(&self, messages: &[ChatMessage], current_query: &str) -> WindowResult {
        let mut scores = score_messages(messages, current_query);

        if scores.len() <= self.config.max_turns {
            return WindowResult {
                retained: messages.to_vec(),
                evicted: vec![],
                pinned_count: scores.iter().filter(|s| s.pinned).count(),
            };
        }

        // Sort by relevance (ascending) — lowest first for eviction
        scores.sort_by(|a, b| {
            a.relevance
                .partial_cmp(&b.relevance)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        // Calculate how many to evict (only non-pinned)
        let non_pinned: Vec<&ContextScore> = scores.iter().filter(|s| !s.pinned).collect();
        let evict_count = (non_pinned.len() * self.config.eviction_pct / 100).max(1);
        let evict_count = evict_count.min(scores.len().saturating_sub(self.config.max_turns));

        // Collect indices to evict (from lowest-scoring non-pinned)
        let mut evict_indices: Vec<usize> = Vec::new();
        for score in &scores {
            if !score.pinned && evict_indices.len() < evict_count {
                evict_indices.push(score.index);
            }
        }

        // Split messages into retained and evicted
        let mut retained = Vec::with_capacity(messages.len() - evict_indices.len());
        let mut evicted = Vec::with_capacity(evict_indices.len());

        for (i, msg) in messages.iter().enumerate() {
            if evict_indices.contains(&i) {
                evicted.push(msg.clone());
            } else {
                retained.push(msg.clone());
            }
        }

        let pinned_count = scores.iter().filter(|s| s.pinned).count();

        info!(
            "[SemanticWindow] Evicted {} messages, retained {} ({} pinned)",
            evicted.len(),
            retained.len(),
            pinned_count,
        );

        WindowResult {
            retained,
            evicted,
            pinned_count,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use savant_core::types::ChatRole;

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
    fn test_under_threshold_no_eviction() {
        let window = SemanticWindow::default_window();
        let messages = vec![
            make_msg(ChatRole::System, "system"),
            make_msg(ChatRole::User, "hello"),
        ];
        let result = window.manage(&messages, "");
        assert_eq!(result.retained.len(), 2);
        assert!(result.evicted.is_empty());
    }

    #[test]
    fn test_pinned_messages_never_evicted() {
        let config = WindowConfig {
            max_turns: 3,
            eviction_pct: 50,
            session_threshold_mins: 30,
        };
        let window = SemanticWindow::new(config);

        let messages = vec![
            make_msg(
                ChatRole::System,
                "SUBSTRATE OPERATIONAL DIRECTIVE: Always be helpful",
            ),
            make_msg(ChatRole::User, "msg1"),
            make_msg(ChatRole::Assistant, "reply1"),
            make_msg(ChatRole::User, "msg2"),
            make_msg(ChatRole::Assistant, "reply2"),
        ];

        let result = window.manage(&messages, "");

        // System message should be pinned and retained
        assert!(result.retained.iter().any(|m| m.role == ChatRole::System));
        // Pinned count should be at least 1
        assert!(result.pinned_count >= 1);
    }

    #[test]
    fn test_default_config() {
        let config = WindowConfig::default();
        assert_eq!(config.max_turns, 50);
        assert_eq!(config.eviction_pct, 20);
        assert_eq!(config.session_threshold_mins, 30);
    }
}
