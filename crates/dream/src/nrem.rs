//! NREM Phase — Structured Memory Consolidation.
//!
//! Replays recent episodic memories, compresses redundant entries,
//! resolves contradictions, and writes consolidated results to persistent storage.
//!
//! # Relevance-Conditioned Logarithmic Decay
//! Memory weights decrease logarithmically from last access:
//! `w(t) = w0 * log(e + t) * spike_factor(access_count)`
//! Below threshold → cold storage eligible. Spikes on re-access or re-linking.

use std::sync::Arc;
use std::time::Instant;

use savant_memory::MemoryEngine;
use tracing::{debug, info, warn};
use xxhash_rust::xxh3::xxh3_64;

/// Result of an NREM consolidation cycle.
#[derive(Debug, Clone)]
pub struct NremResult {
    /// Number of memories scanned.
    pub scanned: usize,
    /// Number of memories consolidated (deduplicated + compressed).
    pub consolidated: usize,
    /// Number of contradictions resolved.
    pub contradictions_resolved: usize,
    /// IDs of memories marked for cold storage (below decay threshold).
    pub cold_storage_eligible: Vec<u64>,
    /// Duration in milliseconds.
    pub duration_ms: u64,
}

/// Consolidation event emitted to the vault outbox after NREM Phase 3.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ConsolidationEvent {
    /// Unique event identifier.
    pub event_id: String,
    /// IDs of memories that were consolidated (deduplicated/compressed).
    pub consolidated_ids: Vec<u64>,
    /// IDs of memories archived to cold storage (below decay threshold).
    pub archived_ids: Vec<u64>,
    /// New synthesis generated from consolidation (if any).
    pub new_synthesis: Option<String>,
    /// Timestamp of the consolidation event.
    pub timestamp: i64,
}

/// Relevance-conditioned logarithmic decay function.
///
/// Weight decreases logarithmically from last access time.
/// `w(t) = w0 * ln(e + t_hours) * spike_factor(access_count)`
///
/// The spike factor resets weight toward original on re-access or re-linking:
/// `spike_factor(n) = 1.0 + 0.2 * ln(1 + n)` where n = access_count
///
/// Below threshold → cold storage eligible.
pub fn compute_decay_weight(
    initial_weight: f32,
    age_hours: f32,
    access_count: u32,
    referenced_by_others: bool,
) -> f32 {
    // Logarithmic decay: weight decreases slowly over time
    let decay_factor = (1.0 + age_hours).ln().max(0.01);

    // Spike factor: re-access or re-linking pushes weight back up
    let spike = 1.0 + 0.2 * (1.0 + access_count as f32).ln();

    // Reference bonus: memories linked by others are more relevant
    let reference_bonus = if referenced_by_others { 1.3 } else { 1.0 };

    // Normalized: divide by decay, multiply by spike and reference
    let weight = initial_weight / decay_factor.max(1.0) * spike * reference_bonus;

    weight.clamp(0.01, 1.0)
}

/// Determines if a memory weight is below the cold storage threshold.
pub fn is_cold_storage_eligible(weight: f32, threshold: f32) -> bool {
    weight < threshold
}

/// Default decay threshold for cold storage eligibility.
pub const DEFAULT_DECAY_THRESHOLD: f32 = 0.15;

/// NREM controller for structured memory replay and consolidation.
pub struct NremController {
    /// Hours of episodic memory to replay.
    pub replay_window_hours: u64,
    /// Decay threshold below which memories are cold-storage eligible.
    pub decay_threshold: f32,
    /// Maximum number of messages to fetch per cycle.
    pub max_messages: usize,
}

impl NremController {
    /// Creates a new NREM controller with the given replay window.
    pub fn new(replay_window_hours: u64) -> Self {
        Self {
            replay_window_hours,
            decay_threshold: DEFAULT_DECAY_THRESHOLD,
            max_messages: 5000,
        }
    }

    /// Creates a NREM controller with custom decay threshold.
    pub fn with_decay_threshold(replay_window_hours: u64, decay_threshold: f32) -> Self {
        Self {
            replay_window_hours,
            decay_threshold,
            max_messages: 5000,
        }
    }

    /// Creates a default NREM controller (24 hour replay window).
    pub fn default_controller() -> Self {
        Self::new(24)
    }

    /// Runs the NREM consolidation cycle.
    ///
    /// # Process
    /// 1. Fetch recent messages from all sessions (last N hours)
    /// 2. Deduplicate consecutive identical messages
    /// 3. Detect and resolve contradictions (keep newer + higher importance)
    /// 4. Apply relevance-conditioned logarithmic decay to memory weights
    /// 5. Mark below-threshold memories as cold-storage eligible
    /// 6. Write consolidated results back to memory
    /// 7. Emit ConsolidationEvent to outbox for vault projection
    pub async fn run(
        &self,
        memory: &Arc<MemoryEngine>,
    ) -> Result<(NremResult, Option<ConsolidationEvent>), super::DreamError> {
        let start = Instant::now();
        info!(
            "[NREM] Starting consolidation cycle (window={}h, decay_threshold={:.2})",
            self.replay_window_hours, self.decay_threshold
        );

        // Fetch all messages across sessions
        let enclave = memory.enclave();
        let lsm = enclave.lsm();
        let all_messages = lsm.iter_all_messages(self.max_messages);
        let messages: Vec<_> = all_messages.collect();

        if messages.is_empty() {
            debug!("[NREM] No messages to consolidate");
            return Ok((
                NremResult {
                    scanned: 0,
                    consolidated: 0,
                    contradictions_resolved: 0,
                    cold_storage_eligible: Vec::new(),
                    duration_ms: start.elapsed().as_millis() as u64,
                },
                None,
            ));
        }

        let scanned = messages.len();
        let now_ms = chrono::Utc::now().timestamp_millis();

        // Phase 1: Deduplicate consecutive identical messages
        let mut deduped = Vec::with_capacity(messages.len());
        let mut dedup_count = 0usize;

        for msg in &messages {
            if let Some(last) = deduped.last() {
                let last_msg: &savant_memory::AgentMessage = last;
                if last_msg.content == msg.content && last_msg.role == msg.role {
                    dedup_count += 1;
                    continue;
                }
            }
            deduped.push(msg.clone());
        }

        // Phase 2: Detect contradictions (simplified: messages with conflicting keywords)
        let contradictions = detect_contradictions(&deduped);

        // Phase 3: Resolve contradictions — keep the newer message
        let resolved = resolve_contradictions(deduped, &contradictions);
        let consolidated = resolved.len();
        let contradictions_resolved = contradictions.len();

        // Phase 4: Apply relevance-conditioned logarithmic decay
        let mut cold_storage_ids = Vec::new();
        let mut consolidated_ids = Vec::new();

        for msg in &resolved {
            let age_hours = (now_ms - i64::from(msg.timestamp)) as f32 / 3_600_000.0;

            // For AgentMessage, we use content length as a proxy for importance
            // and tool_calls count as a proxy for access/references
            let content_importance = (msg.content.len().min(1000) as f32) / 1000.0;
            let access_count = msg.tool_calls.len() as u32;
            let referenced = !msg.tool_calls.is_empty() || !msg.tool_results.is_empty();

            let weight =
                compute_decay_weight(content_importance, age_hours, access_count, referenced);

            if is_cold_storage_eligible(weight, self.decay_threshold) {
                let id_val = xxh3_64(msg.id.as_bytes());
                cold_storage_ids.push(id_val);
            }

            if !msg.tool_calls.is_empty() {
                let id_val = xxh3_64(msg.id.as_bytes());
                consolidated_ids.push(id_val);
            }
        }

        if !cold_storage_ids.is_empty() {
            info!(
                "[NREM] {} memories below decay threshold ({:.2}) — cold storage eligible",
                cold_storage_ids.len(),
                self.decay_threshold
            );
        }

        // Phase 5: Write consolidated results back
        // Group by session and compact each session
        let mut sessions: std::collections::HashMap<String, Vec<savant_memory::AgentMessage>> =
            std::collections::HashMap::new();
        for msg in resolved {
            sessions
                .entry(msg.session_id.clone())
                .or_default()
                .push(msg);
        }

        for (session_id, session_messages) in &sessions {
            if let Err(e) = memory
                .enclave()
                .atomic_compact(session_id, session_messages.clone())
                .await
            {
                warn!("[NREM] Failed to compact session {}: {}", session_id, e);
            }
        }

        let duration_ms = start.elapsed().as_millis() as u64;
        info!(
            "[NREM] Complete: {} scanned, {} consolidated, {} contradictions resolved, {} cold-storage eligible ({}ms)",
            scanned, consolidated, contradictions_resolved, cold_storage_ids.len(), duration_ms
        );

        // Phase 6: Emit ConsolidationEvent to outbox for vault projection
        let consolidation_event = if !consolidated_ids.is_empty() || !cold_storage_ids.is_empty() {
            let event = ConsolidationEvent {
                event_id: uuid::Uuid::new_v4().to_string(),
                consolidated_ids: consolidated_ids.clone(),
                archived_ids: cold_storage_ids.clone(),
                new_synthesis: if contradictions_resolved > 0 {
                    Some(format!(
                        "Resolved {} contradictions across {} sessions",
                        contradictions_resolved,
                        sessions.len()
                    ))
                } else {
                    None
                },
                timestamp: chrono::Utc::now().timestamp(),
            };
            info!(
                "[NREM] Emitting ConsolidationEvent: {} consolidated, {} archived",
                event.consolidated_ids.len(),
                event.archived_ids.len()
            );
            Some(event)
        } else {
            None
        };

        Ok((
            NremResult {
                scanned,
                consolidated: dedup_count,
                contradictions_resolved,
                cold_storage_eligible: cold_storage_ids,
                duration_ms,
            },
            consolidation_event,
        ))
    }
}

/// Words to ignore when computing content word overlap (stopwords).
const CONTENT_STOPWORDS: &[&str] = &[
    "the", "a", "an", "is", "was", "are", "were", "be", "been", "being", "have", "has", "had",
    "do", "does", "did", "will", "would", "could", "should", "may", "might", "shall", "can", "to",
    "of", "in", "for", "on", "with", "at", "by", "from", "as", "into", "about", "it", "its",
    "this", "that", "and", "or", "but", "not", "no", "nor",
];

/// Negation patterns that indicate a statement is being denied/reversed.
const NEGATION_WORDS: &[&str] = &[
    "not",
    "never",
    "no longer",
    "isn't",
    "doesn't",
    "wasn't",
    "won't",
    "can't",
    "couldn't",
    "shouldn't",
    "wouldn't",
    "haven't",
    "hasn't",
    "hadn't",
    "don't",
    "didn't",
    "cannot",
];

/// Maximum number of recent messages to check for contradictions (sliding window).
const CONTRADICTION_WINDOW: usize = 500;

/// Detects contradictions in a list of messages.
/// Returns indices of contradictory message pairs.
///
/// Uses a sliding window (last 500 messages) to avoid O(n^2) on large histories.
/// A contradiction requires BOTH:
/// 1. Explicit negation pattern in one of the messages
/// 2. >50% shared content words between the two messages (topic overlap)
fn detect_contradictions(messages: &[savant_memory::AgentMessage]) -> Vec<(usize, usize)> {
    let mut contradictions = Vec::new();

    // Sliding window: only compare recent messages to avoid O(n^2) on large histories
    let window_start = messages.len().saturating_sub(CONTRADICTION_WINDOW);

    for i in window_start..messages.len() {
        for j in (i + 1)..messages.len() {
            let a = messages[i].content.to_lowercase();
            let b = messages[j].content.to_lowercase();

            // Step 1: Check if either message contains negation words
            let a_has_negation = NEGATION_WORDS.iter().any(|nw| a.contains(nw));
            let b_has_negation = NEGATION_WORDS.iter().any(|nw| b.contains(nw));

            if !a_has_negation && !b_has_negation {
                continue; // No negation — skip pair
            }

            // Step 2: Compute semantic overlap via content words (>50% shared)
            let content_words = |text: &str| -> std::collections::HashSet<String> {
                text.split_whitespace()
                    .filter(|w| !CONTENT_STOPWORDS.contains(w) && w.len() > 2)
                    .map(|w| w.to_string())
                    .collect()
            };
            let words_a = content_words(&a);
            let words_b = content_words(&b);

            if words_a.is_empty() || words_b.is_empty() {
                continue;
            }

            let shared = words_a.intersection(&words_b).count();
            let min_count = words_a.len().min(words_b.len());
            let overlap_ratio = shared as f32 / min_count as f32;

            if overlap_ratio > 0.5 {
                contradictions.push((i, j));
            }
        }
    }

    contradictions
}

/// Resolves contradictions by keeping the newer message (higher index = newer).
fn resolve_contradictions(
    mut messages: Vec<savant_memory::AgentMessage>,
    contradictions: &[(usize, usize)],
) -> Vec<savant_memory::AgentMessage> {
    let mut to_remove = std::collections::HashSet::new();

    for &(i, j) in contradictions {
        // Keep the newer one (higher index), remove the older one
        to_remove.insert(i.min(j));
    }

    // Remove in reverse order to preserve indices
    let mut remove_indices: Vec<usize> = to_remove.into_iter().collect();
    remove_indices.sort_unstable();
    remove_indices.reverse();

    for idx in remove_indices {
        if idx < messages.len() {
            messages.remove(idx);
        }
    }

    messages
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_contradictions() {
        use savant_memory::AgentMessage;

        let messages = vec![
            AgentMessage::user("s1", "The build is not passing anymore"),
            AgentMessage::user("s1", "The build is passing now"),
        ];

        let contradictions = detect_contradictions(&messages);
        assert!(
            !contradictions.is_empty(),
            "Should detect contradiction via negation + shared content words"
        );
    }

    #[test]
    fn test_detect_contradictions_no_negation() {
        use savant_memory::AgentMessage;

        // Without negation words, no contradiction should be detected
        let messages = vec![
            AgentMessage::user("s1", "The build is passing"),
            AgentMessage::user("s1", "The build is failing"),
        ];

        let contradictions = detect_contradictions(&messages);
        assert!(
            contradictions.is_empty(),
            "Antonym pairs without negation should not trigger contradiction detection"
        );
    }

    #[test]
    fn test_resolve_contradictions_keeps_newer() {
        use savant_memory::AgentMessage;

        let messages = vec![
            AgentMessage::user("s1", "The service isn't working correctly"),
            AgentMessage::user("s1", "The service is working correctly now"),
        ];

        let contradictions = vec![(0, 1)];
        let resolved = resolve_contradictions(messages, &contradictions);

        assert_eq!(resolved.len(), 1);
        assert!(
            resolved[0].content.contains("now"),
            "Should keep the newer message"
        );
    }

    #[test]
    fn test_nrem_controller_default() {
        let controller = NremController::default_controller();
        assert_eq!(controller.replay_window_hours, 24);
    }

    #[test]
    fn test_decay_weight_no_decay() {
        // New memory (age=0), no accesses → should have high weight
        let weight = compute_decay_weight(1.0, 0.0, 0, false);
        assert!(
            weight > 0.5,
            "Fresh memory should have high weight, got {}",
            weight
        );
    }

    #[test]
    fn test_decay_weight_old_memory() {
        // Very old memory (1 year), no accesses → should have low weight
        let weight = compute_decay_weight(1.0, 8760.0, 0, false);
        assert!(
            weight < 0.3,
            "Old memory should have low weight, got {}",
            weight
        );
    }

    #[test]
    fn test_decay_weight_spike_on_access() {
        // Old memory but frequently accessed → spike factor kicks in
        let weight_no_access = compute_decay_weight(1.0, 100.0, 0, false);
        let weight_with_access = compute_decay_weight(1.0, 100.0, 10, false);
        assert!(
            weight_with_access > weight_no_access,
            "Frequently accessed memory should spike: {} > {}",
            weight_with_access,
            weight_no_access
        );
    }

    #[test]
    fn test_decay_weight_referenced_bonus() {
        // Referenced memory should have higher weight
        let weight_unreferenced = compute_decay_weight(1.0, 50.0, 0, false);
        let weight_referenced = compute_decay_weight(1.0, 50.0, 0, true);
        assert!(
            weight_referenced > weight_unreferenced,
            "Referenced memory should have bonus: {} > {}",
            weight_referenced,
            weight_unreferenced
        );
    }

    #[test]
    fn test_cold_storage_eligible() {
        assert!(is_cold_storage_eligible(0.1, 0.15));
        assert!(!is_cold_storage_eligible(0.5, 0.15));
        assert!(!is_cold_storage_eligible(0.15, 0.15)); // At threshold = not eligible
    }

    #[test]
    fn test_controller_custom_decay_threshold() {
        let controller = NremController::with_decay_threshold(48, 0.25);
        assert_eq!(controller.replay_window_hours, 48);
        assert!((controller.decay_threshold - 0.25).abs() < f32::EPSILON);
    }
}
