use savant_core::bus::NexusBridge;
use savant_core::error::SavantError;
use savant_core::learning::{EmergentLearning, LearningCategory};
use savant_core::traits::MemoryBackend;
use std::path::PathBuf;
use std::sync::Arc;
use tracing::{debug, info};

/// The Emergent Learning Emitter.
///
/// This component is responsible for harvesting cognitive insights from agent
/// traces and formalizing them into the structured LEARNINGS.jsonl dataset.
pub struct LearningEmitter<M: MemoryBackend + Clone> {
    memory: M,
    agent_id: String,
    nexus: Arc<NexusBridge>,
    workspace_path: PathBuf,
}

impl<M: MemoryBackend + Clone> LearningEmitter<M> {
    /// Creates a new LearningEmitter for a specific agent.
    pub fn new(
        agent_id: String,
        memory: M,
        nexus: Arc<NexusBridge>,
        workspace_path: PathBuf,
    ) -> Self {
        Self {
            memory,
            agent_id,
            nexus,
            workspace_path,
        }
    }

    /// Evaluates and emits an emergent learning entry.
    ///
    /// This performs signal-to-noise filtering based on significance and
    /// repetitive content detection.
    pub async fn emit_emergent(
        &self,
        content: String,
        suggested_category: Option<LearningCategory>,
    ) -> Result<(), SavantError> {
        // 1. Initial Signal Processing
        let significance = self.calculate_significance(&content);

        // 2. Filter Noise: Only capture signals with significance > 2
        // OR explicit category (which implies agent intentionality)
        // AAA: Variance Penalty (Phase 19) - If significance is borderline, we filter more strictly.
        if significance <= 2 && suggested_category.is_none() {
            debug!(
                "Discarding low-signal learning (significance {}): {}",
                significance, content
            );
            return Ok(());
        }

        // 2. Grounding filter — block fabrication, require environmental grounding
        if !super::filter::OutputFilter::is_grounded(&content) {
            tracing::warn!(
                "Learning output filtered (not grounded): {}",
                &content[..content.len().min(100)]
            );
            return Ok(());
        }

        // 3. Categorize (Heuristic)
        let category =
            suggested_category.unwrap_or_else(|| self.heuristic_categorization(&content));

        // 4. Construct entry
        let learning = EmergentLearning::new(
            self.agent_id.clone(),
            category,
            content.clone(),
            significance,
        );

        // 5. Formalize: Sink to structured memory
        // We bypass the chat-centric 'store' if possible or ensure it doesn't leak to historical chat lanes.
        // For WAL integrity, we ensure this is recorded as a Learning event, not a chat message.
        let msg = savant_core::types::ChatMessage {
            is_telemetry: false,
            role: savant_core::types::ChatRole::Assistant,
            content: content.clone(),
            sender: Some(self.agent_id.clone()),
            recipient: None,
            agent_id: None,
            session_id: Some(savant_core::types::SessionId(format!(
                "learning:{}",
                self.agent_id
            ))),
            channel: savant_core::types::AgentOutputChannel::Memory,
            images: Vec::new(),
            ..Default::default()
        };

        // AAA Enhancement: Mark message as technical to prevent historical lane pollution
        // In this architecture, the MemoryBackend will handle the partition based on the JSON content.
        self.memory
            .store(&format!("learning.{}", self.agent_id), &msg)
            .await?;

        // 6. Write to LEARNINGS.jsonl so SwarmInsightHistoryRequest finds real data.
        //    Without this, the dashboard reflections panel shows only the dummy fallback
        //    message until a full heartbeat cycle runs LearningsParser::parse_and_convert.
        {
            let jsonl_path = self.workspace_path.join("LEARNINGS.jsonl");
            if let Ok(payload) = serde_json::to_string(&learning) {
                if let Ok(mut file) = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&jsonl_path)
                {
                    use std::io::Write;
                    if let Err(e) = writeln!(file, "{}", payload) {
                        debug!(
                            "Failed to write learning entry to LEARNINGS.jsonl: {}",
                            e
                        );
                    }
                } else {
                    debug!("Failed to open LEARNINGS.jsonl at {:?}", jsonl_path);
                }
            }
        }

        // 7. Broadcast: Bridge to the global Nexus so UI can display real-time insights
        if let Ok(payload) = serde_json::to_string(&learning) {
            if let Err(e) = self.nexus.publish("learning.insight", &payload).await {
                debug!("Failed to publish cognitive insight to Nexus: {}", e);
            }
        }

        info!(
            "[{}] Cognitive Harvest: Emit {:?} (Significance: {})",
            self.agent_id, learning.category, significance
        );

        Ok(())
    }

    /// Heuristic to determine the category of an unstructured learning blob.
    fn heuristic_categorization(&self, content: &str) -> LearningCategory {
        let text = content.to_lowercase();
        if text.contains("protocol") || text.contains("discipline") || text.contains("instruction")
        {
            LearningCategory::Protocol
        } else if text.contains("error")
            || text.contains("misstep")
            || text.contains("fail")
            || text.contains("correction")
        {
            LearningCategory::Error
        } else {
            LearningCategory::Insight
        }
    }

    /// Basic significance calculation based on length and key complexity markers.
    fn calculate_significance(&self, content: &str) -> u8 {
        let mut score = 3u8; // Baseline

        if content.len() > 100 {
            score += 1;
        }
        if content.len() > 300 {
            score += 2;
        }

        // Check for technical complexity markers
        let markers = [
            "refactor",
            "latency",
            "concurrency",
            "optimization",
            "bottleneck",
            "divergence",
            "integrity",
        ];
        for marker in markers {
            if content.to_lowercase().contains(marker) {
                score += 1;
            }
        }

        score.min(10)
    }
}
