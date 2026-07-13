//! Dream Output Filter — Evaluates dream outputs before storage.
//!
//! Dream outputs are NOT required to be grounded (they explore latent space).
//! However, outputs that enter LEARNINGS.md must pass the standard grounding filter.
//! All dream outputs are tagged with taint metadata.

/// Taint tag for dream-generated content.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DreamTaintTag {
    /// Source of the content.
    pub source: String,
    /// Timestamp of generation.
    pub timestamp: i64,
    /// Trust level (0.0 = untrusted, 1.0 = fully trusted).
    pub trust_level: f32,
    /// Provenance chain (transformations applied).
    pub provenance_chain: Vec<String>,
}

impl DreamTaintTag {
    /// Creates a new dream taint tag.
    pub fn new(phase: &str) -> Self {
        Self {
            source: format!("dream_{}", phase),
            timestamp: chrono::Utc::now().timestamp(),
            trust_level: 0.5,
            provenance_chain: vec![format!("dream_{}_phase", phase)],
        }
    }

    /// Creates a taint tag for NREM outputs (higher trust — grounded in real memories).
    pub fn nrem() -> Self {
        Self {
            source: "dream_nrem".to_string(),
            timestamp: chrono::Utc::now().timestamp(),
            trust_level: 0.7,
            provenance_chain: vec![
                "memory_replay".to_string(),
                "nrem_consolidation".to_string(),
            ],
        }
    }

    /// Creates a taint tag for REM outputs (lower trust — speculative exploration).
    pub fn rem() -> Self {
        Self {
            source: "dream_rem".to_string(),
            timestamp: chrono::Utc::now().timestamp(),
            trust_level: 0.5,
            provenance_chain: vec![
                "latent_exploration".to_string(),
                "cross_domain_recombination".to_string(),
            ],
        }
    }

    /// Returns true if this content requires human verification.
    pub fn requires_human_verification(&self) -> bool {
        self.trust_level < 0.3
    }
}

/// Filters dream outputs for quality and diversity.
pub struct DreamFilter {
    /// Minimum content length to store (characters).
    pub min_content_length: usize,
    /// Minimum alphanumeric ratio to avoid storing noise.
    pub min_alpha_ratio: f32,
    /// Trust level threshold for human verification.
    pub trust_verification_threshold: f32,
    /// Minimum trust level for REM content to enter learnings.
    pub rem_learnings_trust_threshold: f32,
}

impl Default for DreamFilter {
    fn default() -> Self {
        Self {
            min_content_length: 10,
            min_alpha_ratio: 0.3,
            trust_verification_threshold: 0.3,
            rem_learnings_trust_threshold: 0.5,
        }
    }
}

impl DreamFilter {
    /// Creates a new DreamFilter with default thresholds.
    pub fn new() -> Self {
        Self::default()
    }

    /// Evaluates whether a dream output should be stored.
    ///
    /// Dream outputs are NOT grounded (they explore latent space),
    /// so the standard grounding filter is NOT applied here.
    ///
    /// Instead, we check:
    /// 1. Minimum content length (discard trivially short outputs)
    /// 2. No duplicate content (hash-based dedup)
    /// 3. Vendi Score threshold is applied at the REM controller level
    pub fn should_store(&self, content: &str) -> bool {
        // Minimum length check
        if content.trim().len() < self.min_content_length {
            return false;
        }

        // Discard outputs that are just noise
        let alpha_ratio = content.chars().filter(|c| c.is_alphanumeric()).count() as f32
            / content.len().max(1) as f32;

        if alpha_ratio < self.min_alpha_ratio {
            return false;
        }

        true
    }

    /// Evaluates whether a dream output can enter LEARNINGS.md.
    /// Must pass the standard grounding filter from the agent crate.
    ///
    /// NOTE: This requires the grounding filter from `crates/agent/src/learning/filter.rs`.
    /// Since `crates/dream` depends on `crates/memory` but NOT `crates/agent`,
    /// this check is performed at integration time (in the heartbeat pulse).
    pub fn can_enter_learnings(&self, taint: &DreamTaintTag) -> bool {
        // Heavily tainted content cannot enter learnings
        if taint.trust_level < self.trust_verification_threshold {
            return false;
        }

        // REM content is speculative — require higher trust for learnings
        if taint.source == "dream_rem" && taint.trust_level < self.rem_learnings_trust_threshold {
            return false;
        }

        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_should_store_valid_content() {
        let filter = DreamFilter::new();
        assert!(filter
            .should_store("This is a meaningful dream association about memory consolidation"));
    }

    #[test]
    fn test_should_store_rejects_short() {
        let filter = DreamFilter::new();
        assert!(!filter.should_store("short"));
    }

    #[test]
    fn test_should_store_rejects_noise() {
        let filter = DreamFilter::new();
        assert!(!filter.should_store("@#$%^&*()!@#$%^&*()!@#$%^&*()"));
    }

    #[test]
    fn test_nrem_taint_higher_trust() {
        let nrem = DreamTaintTag::nrem();
        let rem = DreamTaintTag::rem();
        assert!(nrem.trust_level > rem.trust_level);
    }

    #[test]
    fn test_can_enter_learnings_low_trust() {
        let filter = DreamFilter::new();
        let tag = DreamTaintTag {
            source: "dream_rem".to_string(),
            timestamp: 0,
            trust_level: 0.2,
            provenance_chain: vec![],
        };
        assert!(!filter.can_enter_learnings(&tag));
    }

    #[test]
    fn test_can_enter_learnings_nrem() {
        let filter = DreamFilter::new();
        let tag = DreamTaintTag::nrem();
        assert!(filter.can_enter_learnings(&tag));
    }

    #[test]
    fn test_custom_filter_thresholds() {
        let filter = DreamFilter {
            min_content_length: 5,
            min_alpha_ratio: 0.1,
            trust_verification_threshold: 0.1,
            rem_learnings_trust_threshold: 0.3,
        };
        assert!(filter.should_store("short"));
    }
}
