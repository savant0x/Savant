//! L2 Context Window Compression
//!
//! Monitors context window utilization and applies semantic compression
//! when thresholds are exceeded. Two-stage: L2 tool eviction → L2 LLM summarization.
//!
//! Thresholds (configurable):
//! - 75%: L2 tool eviction — replace old tool results with 1-line markers
//! - 85%: L2 LLM summarization — summarize middle turns via auxiliary LLM
//! - 95%: Emergency — aggressive truncation
//!
//! Tail protection: minimum 6 conversational turns always preserved.

use tracing::info;

/// Context window utilization threshold for triggering L2 compression.
#[derive(Debug, Clone)]
pub struct L2Thresholds {
    /// Threshold for tool eviction (default: 0.75).
    pub tool_eviction: f32,
    /// Threshold for LLM summarization (default: 0.85).
    pub llm_summarization: f32,
    /// Emergency threshold (default: 0.95).
    pub emergency: f32,
    /// Minimum conversational turns to preserve in tail (default: 6).
    pub min_tail_turns: usize,
}

impl Default for L2Thresholds {
    fn default() -> Self {
        Self {
            tool_eviction: 0.75,
            llm_summarization: 0.85,
            emergency: 0.95,
            min_tail_turns: 6,
        }
    }
}

/// L2 context compression engine.
#[derive(Debug, Clone)]
pub struct L2Compressor {
    /// Thresholds for compression stages.
    pub thresholds: L2Thresholds,
    /// Whether L2 compression is enabled.
    pub enabled: bool,
}

impl L2Compressor {
    /// Creates a new L2 compressor with default thresholds.
    pub fn new() -> Self {
        Self {
            thresholds: L2Thresholds::default(),
            enabled: true,
        }
    }

    /// Creates a new L2 compressor with custom thresholds.
    pub fn with_thresholds(thresholds: L2Thresholds) -> Self {
        Self {
            thresholds,
            enabled: true,
        }
    }

    /// Checks if L2 compression should be triggered.
    /// Returns the compression stage to apply.
    pub fn check_threshold(&self, context_utilization: f32) -> Option<L2Stage> {
        if !self.enabled {
            return None;
        }
        if context_utilization >= self.thresholds.emergency {
            Some(L2Stage::Emergency)
        } else if context_utilization >= self.thresholds.llm_summarization {
            Some(L2Stage::LLMSummarization)
        } else if context_utilization >= self.thresholds.tool_eviction {
            Some(L2Stage::ToolEviction)
        } else {
            None
        }
    }

    /// Evicts old tool results from the context, replacing them with 1-line markers.
    /// Returns the number of evicted results.
    pub fn evict_tool_results(&self, messages: &mut [savant_core::types::ChatMessage]) -> usize {
        let mut evicted = 0;
        let min_tail = self.thresholds.min_tail_turns;
        let total = messages.len();

        // Only evict from the middle (not the tail)
        let evict_end = total.saturating_sub(min_tail);

        for msg in messages.iter_mut().take(evict_end) {
            if msg.role == savant_core::types::ChatRole::Tool {
                let tool_name = "tool";
                let output_preview = msg
                    .content
                    .lines()
                    .next()
                    .unwrap_or("")
                    .chars()
                    .take(80)
                    .collect::<String>();
                *msg = savant_core::types::ChatMessage {
                    role: savant_core::types::ChatRole::Tool,
                    content: format!("[Compact] {} → {}", tool_name, output_preview),
                    images: Vec::new(),
                    ..msg.clone()
                };
                evicted += 1;
            }
        }

        if evicted > 0 {
            info!("[compact L2] Evicted {} tool results", evicted);
        }

        evicted
    }
}

impl Default for L2Compressor {
    fn default() -> Self {
        Self::new()
    }
}

/// L2 compression stage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum L2Stage {
    /// Stage 1: Evict old tool results.
    ToolEviction,
    /// Stage 2: LLM summarization of middle turns.
    LLMSummarization,
    /// Emergency: aggressive truncation.
    Emergency,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_thresholds() {
        let compressor = L2Compressor::new();
        assert_eq!(compressor.check_threshold(0.5), None);
        assert_eq!(
            compressor.check_threshold(0.75),
            Some(L2Stage::ToolEviction)
        );
        assert_eq!(
            compressor.check_threshold(0.85),
            Some(L2Stage::LLMSummarization)
        );
        assert_eq!(compressor.check_threshold(0.95), Some(L2Stage::Emergency));
    }

    #[test]
    fn test_disabled() {
        let mut compressor = L2Compressor::new();
        compressor.enabled = false;
        assert_eq!(compressor.check_threshold(0.99), None);
    }

    #[test]
    fn test_custom_thresholds() {
        let thresholds = L2Thresholds {
            tool_eviction: 0.60,
            llm_summarization: 0.80,
            emergency: 0.90,
            min_tail_turns: 4,
        };
        let compressor = L2Compressor::with_thresholds(thresholds);
        assert_eq!(
            compressor.check_threshold(0.60),
            Some(L2Stage::ToolEviction)
        );
        assert_eq!(
            compressor.check_threshold(0.80),
            Some(L2Stage::LLMSummarization)
        );
    }
}
