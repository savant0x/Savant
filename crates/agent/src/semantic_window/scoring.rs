//! Context Scoring — Evaluates message turns for semantic importance.

use savant_core::types::{ChatMessage, ChatRole};

/// Score for a single message turn.
#[derive(Debug, Clone)]
pub struct ContextScore {
    /// Index in the original message list.
    pub index: usize,
    /// The message.
    pub message: ChatMessage,
    /// Relevance score [0.0, 1.0].
    pub relevance: f32,
    /// Whether this message is pinned (never evicted).
    pub pinned: bool,
}

/// Scores message turns for semantic importance.
///
/// Scoring factors (multi-head):
/// - Role weight: System=1.0, Tool=0.9, User=0.7, Assistant=0.5
/// - Recency: exponential decay favoring recent messages
/// - Keyword relevance: case-insensitive token overlap with query
/// - Causal preservation: assistant responses near user queries get a boost
pub fn score_messages(messages: &[ChatMessage], current_query: &str) -> Vec<ContextScore> {
    let total = messages.len();
    let mut scores = Vec::with_capacity(total);

    let query_lower = current_query.to_lowercase();
    let query_tokens: std::collections::HashSet<String> = query_lower
        .split_whitespace()
        .filter(|w| w.len() > 3)
        .map(|w| w.to_string())
        .collect();

    for (i, msg) in messages.iter().enumerate() {
        let role_weight = match msg.role {
            ChatRole::System => 1.0,    // System messages are always important
            ChatRole::User => 0.7,      // User messages are usually important
            ChatRole::Assistant => 0.5, // Assistant responses are less critical
            _ => 0.6,                   // Tool and other roles
        };

        // Recency: exponential decay — recent messages matter more
        let recency = if total > 1 {
            let linear = i as f32 / (total - 1) as f32;
            // Exponential boost for recent messages
            linear.powf(0.5) // sqrt gives moderate boost to recency
        } else {
            1.0
        };

        // Keyword relevance: case-insensitive token overlap with current query
        let keyword_relevance = if query_tokens.is_empty() {
            0.5
        } else {
            improved_keyword_overlap(&msg.content, &query_tokens)
        };

        // Causal preservation: assistant responses immediately following user queries
        // should be kept together (prevent breaking Q→A pairs)
        let causal_boost = if msg.role == ChatRole::Assistant && i > 0 {
            if messages[i - 1].role == ChatRole::User {
                0.15 // Boost assistant responses that follow user queries
            } else {
                0.0
            }
        } else {
            0.0
        };

        // Pin status: system messages and messages containing SOUL.md references are pinned
        let pinned = msg.role == ChatRole::System
            || msg.content.contains("SOUL.md")
            || msg.content.contains("PERSONA (SOUL)")
            || msg.content.contains("SUBSTRATE OPERATIONAL DIRECTIVE");

        let relevance = if pinned {
            1.0 // Pinned content always has max relevance
        } else {
            (role_weight * 0.25 + recency * 0.30 + keyword_relevance * 0.30 + causal_boost)
                .clamp(0.0, 1.0)
        };

        scores.push(ContextScore {
            index: i,
            message: msg.clone(),
            relevance,
            pinned,
        });
    }

    scores
}

/// Improved keyword overlap with case-insensitive matching.
fn improved_keyword_overlap(
    content: &str,
    query_tokens: &std::collections::HashSet<String>,
) -> f32 {
    let content_lower = content.to_lowercase();
    let content_tokens: std::collections::HashSet<&str> = content_lower
        .split_whitespace()
        .filter(|w| w.len() > 3)
        .collect();

    if content_tokens.is_empty() || query_tokens.is_empty() {
        return 0.0;
    }

    let query_refs: std::collections::HashSet<&str> =
        query_tokens.iter().map(|s| s.as_str()).collect();
    let intersection = content_tokens.intersection(&query_refs).count();
    let min_size = content_tokens.len().min(query_refs.len());

    (intersection as f32 / min_size as f32).min(1.0)
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
    fn test_system_messages_pinned() {
        let messages = vec![
            make_msg(ChatRole::System, "You are an assistant"),
            make_msg(ChatRole::User, "hello"),
        ];
        let scores = score_messages(&messages, "");
        assert!(scores[0].pinned);
        assert!(!scores[1].pinned);
    }

    #[test]
    fn test_soul_reference_pinned() {
        let messages = vec![make_msg(ChatRole::User, "PERSONA (SOUL): You are loyal")];
        let scores = score_messages(&messages, "");
        assert!(scores[0].pinned);
    }

    #[test]
    fn test_relevance_ordering() {
        let messages = vec![
            make_msg(ChatRole::System, "system prompt"),
            make_msg(ChatRole::User, "first question"),
            make_msg(ChatRole::Assistant, "first answer"),
            make_msg(ChatRole::User, "second question"),
        ];
        let scores = score_messages(&messages, "second question");
        // The most recent user message with keyword match should score high
        assert!(scores[3].relevance > scores[2].relevance);
    }

    #[test]
    fn test_keyword_overlap() {
        let query_tokens: std::collections::HashSet<String> =
            ["build", "errors"].iter().map(|s| s.to_string()).collect();
        let overlap = improved_keyword_overlap("the build failed with errors", &query_tokens);
        assert!(overlap > 0.0);
    }
}
