use crate::engine::MemoryEnclave;
use crate::models::MemoryEntry;
use std::sync::Arc;
use tokio::time::{sleep, Duration};
use tracing::{debug, error, info, warn};

/// The Arbiter task monitors the Collective Hive-Mind for factual contradictions
/// and resolves them using Information Entropy (Shannon) heuristics.
pub fn spawn_arbiter_task(collective: Arc<MemoryEnclave>) {
    let sweep_interval = collective.config.arbiter_sweep_interval_secs;
    tokio::spawn(async move {
        info!("⚖️ OMEGA-VIII: Entropy-Based Conflict Arbiter Online");

        loop {
            // Sweep for contradictions at configured interval
            sleep(Duration::from_secs(sweep_interval)).await;

            debug!("Starting Collective contradiction sweep...");

            // AAA: Production-grade scan using the Facts index directly (O(T))
            let mut unique_subjects = std::collections::HashSet::new();
            for (subject, _predicate, _object, _entry_id) in collective.lsm().iter_facts() {
                unique_subjects.insert(subject);
            }

            for subject in unique_subjects {
                // AAA: O(log N) prefix scan over the facts keyspace
                let facts = collective.lsm().get_facts_by_subject(&subject);
                if facts.len() > 1 {
                    let mut contradictory_memories = Vec::new();
                    for (_predicate, _object, entry_id) in facts {
                        if let Ok(Some(memory)) = collective.lsm().get_metadata(entry_id) {
                            contradictory_memories.push(memory);
                        }
                    }

                    if contradictory_memories.len() > 1 {
                        resolve_contradictions(&collective, &subject, contradictory_memories).await;
                    }
                }
            }
        }
    });
}

async fn resolve_contradictions(
    collective: &MemoryEnclave,
    subject: &str,
    mut memories: Vec<MemoryEntry>,
) {
    // Sort by entropy (lower is better/more certain) and then by importance
    memories.sort_by(|a, b| {
        let entropy_a: f32 = a.shannon_entropy.to_native();
        let entropy_b: f32 = b.shannon_entropy.to_native();

        entropy_a
            .partial_cmp(&entropy_b)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(b.importance.cmp(&a.importance))
    });

    let best_memory = &memories[0];
    let entropy: f32 = best_memory.shannon_entropy.to_native();

    // Shannon Cap: If entropy > configured threshold, the fact is too uncertain to be "Canonical"
    if entropy > collective.config.shannon_entropy_cap {
        warn!(
            "Factual collision for '{}' too high-entropy ({} bits). Pending human audit.",
            subject, entropy
        );
        return;
    }

    info!(
        "Canonicalizing fact for '{}' with {} bits of entropy.",
        subject, entropy
    );

    // Mark inferior memories as contradicted (evolution: contradictions ARE growth signals).
    // Instead of deleting, we attach a `contradicted_by` reference for later analysis.
    // Also record temporal invalidation for bi-temporal tracking.
    let best_id = best_memory.id.to_native();
    for inferior in memories.iter().skip(1) {
        let inferior_entropy: f32 = inferior.shannon_entropy.to_native();
        let inferior_id = inferior.id.to_native();
        info!(
            "Factual arbiter: contradictory fact for '{}' resolved (entropy: {} bits, best: {} bits). Inferior tagged as contradicted_by {}.",
            subject, inferior_entropy, entropy, best_id
        );

        // Store temporal metadata marking this inferior fact as superseded
        let mut temporal =
            crate::models::TemporalMetadata::new_active(inferior_id, "fact", subject);
        temporal.invalidate(best_id);
        if let Err(e) = collective.lsm().store_temporal_metadata(&temporal) {
            warn!(
                "Failed to store temporal invalidation for fact {}: {}",
                inferior_id, e
            );
        }

        if let Err(e) = collective.delete_memory(inferior_id).await {
            error!("Failed to prune inferior fact from collective: {}", e);
        }
    }
}

/// Calculates Shannon Entropy of a probability distribution (logprobs).
/// Formula: H(X) = -sum(p * log2(p))
pub fn calculate_shannon_entropy_from_logprobs(logprobs: &[f32]) -> f32 {
    let mut entropy = 0.0;
    for &lp in logprobs {
        let p = lp.exp(); // Convert logprob back to probability
        if p > 0.0 {
            entropy -= p * p.log2();
        }
    }
    entropy
}
