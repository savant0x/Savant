//! Personality-Driven Memory Promotion
//!
//! Background worker that scans memories, applies OCEAN trait-based decay factors,
//! and promotes high-value memories to canonical storage.
//!
//! # Mechanics
//! - `Conscientiousness` scalar: slows decay for security/constraint memories
//! - `Openness` scalar: lowers entropy threshold for exploratory observations
//! - Memories are scored based on: hit_count, age, entropy, importance, personality fit
//! - High-score memories promote to "canonical" category
//! - Low-score memories are archived

use serde::{Deserialize, Serialize};

pub use savant_core::types::PersonalityDelta;

/// OCEAN personality traits from agent SOUL.md.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersonalityTraits {
    pub openness: f32,          // 0.0 - 1.0
    pub conscientiousness: f32, // 0.0 - 1.0
    pub extraversion: f32,      // 0.0 - 1.0
    pub agreeableness: f32,     // 0.0 - 1.0
    pub neuroticism: f32,       // 0.0 - 1.0
}

impl Default for PersonalityTraits {
    fn default() -> Self {
        Self {
            openness: 0.5,
            conscientiousness: 0.5,
            extraversion: 0.5,
            agreeableness: 0.5,
            neuroticism: 0.5,
        }
    }
}

/// Memory promotion metrics used for scoring.
#[derive(Debug, Clone)]
pub struct PromotionMetrics {
    pub hit_count: u32,
    pub age_hours: f32,
    pub shannon_entropy: f32,
    pub importance: u8,
    pub category: String,
}

/// Promotion scoring engine.
pub struct PromotionEngine {
    personality: PersonalityTraits,
    /// Minimum score for promotion to canonical
    pub promotion_threshold: f32,
    /// Maximum age in hours before aggressive decay
    pub decay_after_hours: f32,
    /// Maximum allowed Euclidean distance from baseline OCEAN before auto-block
    pub personality_drift_limit: f32,
    /// Original baseline personality for drift comparison
    pub baseline_personality: Option<PersonalityTraits>,
    /// Running evolution score (0.0-1.0) updated each promotion cycle
    pub evolution_score: f32,
}

impl PromotionEngine {
    /// Creates a new promotion engine with the given personality traits.
    pub fn new(personality: PersonalityTraits) -> Self {
        Self {
            personality: personality.clone(),
            promotion_threshold: 0.7,
            decay_after_hours: 168.0,
            personality_drift_limit: 0.15,
            baseline_personality: Some(personality),
            evolution_score: 0.0,
        }
    }

    /// Updates the active personality traits for promotion scoring.
    ///
    /// The baseline personality (set at construction) is preserved for drift detection.
    /// Only the active scoring personality is updated.
    pub fn update_traits(&mut self, traits: PersonalityTraits) {
        self.personality = traits;
    }

    /// Updates the evolution score based on the latest promotion cycle results.
    ///
    /// The evolution score represents the ratio of high-value memories to total memories,
    /// providing a health metric for the memory system. A higher score indicates
    /// a greater proportion of valuable, frequently-accessed memories.
    pub fn update_evolution_score(&mut self, score: f32) {
        // Smooth the evolution score with exponential moving average (alpha=0.3)
        // to prevent wild swings from single-cycle anomalies
        let alpha = 0.3;
        self.evolution_score = alpha * score + (1.0 - alpha) * self.evolution_score;
        self.evolution_score = self.evolution_score.clamp(0.0, 1.0);
    }

    /// Calculates the promotion score for a memory.
    ///
    /// Score is 0.0 - 1.0. Higher = more likely to be promoted.
    ///
    /// Factors:
    /// - Hit count (access frequency)
    /// - Age decay (older memories score lower unless frequently accessed)
    /// - Shannon entropy (lower entropy = more deterministic = higher score)
    /// - Importance (direct multiplier)
    /// - Personality adjustment (Conscientiousness slows decay for security memories)
    pub fn calculate_score(&self, metrics: &PromotionMetrics) -> f32 {
        let mut score = 0.0;

        // Hit count contribution (0.0 - 0.3)
        let hit_score = (metrics.hit_count as f32 / 100.0).min(0.3);
        score += hit_score;

        // Age decay (0.0 - 0.3 penalty)
        let age_decay = if metrics.age_hours > self.decay_after_hours {
            ((metrics.age_hours - self.decay_after_hours) / self.decay_after_hours).min(0.3)
        } else {
            0.0
        };
        score -= age_decay;

        // Entropy bonus (lower entropy = more deterministic = higher score)
        let entropy_score = (1.0 - metrics.shannon_entropy).max(0.0) * 0.2;
        score += entropy_score;

        // Importance multiplier (1-10 scale)
        let importance_factor = metrics.importance as f32 / 10.0;
        score *= 1.0 + importance_factor;

        // Personality adjustment
        // High conscientiousness: slow decay for security/constraint memories
        if metrics.category.contains("security") || metrics.category.contains("config") {
            let conscientiousness_bonus = self.personality.conscientiousness * 0.2;
            score += conscientiousness_bonus;
        }

        // High openness: boost exploratory/observation memories
        if metrics.category.contains("observation") || metrics.category.contains("exploration") {
            let openness_bonus = self.personality.openness * 0.15;
            score += openness_bonus;
        }

        // High extraversion: boost social/collaboration/communication memories
        if metrics.category.contains("social")
            || metrics.category.contains("collaboration")
            || metrics.category.contains("communication")
        {
            let extraversion_bonus = self.personality.extraversion * 0.2;
            score += extraversion_bonus;
        }

        // High agreeableness: boost consensus/harmony/agreement memories
        if metrics.category.contains("consensus")
            || metrics.category.contains("harmony")
            || metrics.category.contains("agreement")
        {
            let agreeableness_bonus = self.personality.agreeableness * 0.15;
            score += agreeableness_bonus;
        }

        // High neuroticism: conservative decay for threat/error memories (hyper-vigilance)
        if (metrics.category.contains("threat") || metrics.category.contains("error"))
            && self.personality.neuroticism > 0.6
        {
            // Conservative: keep threat memories longer
            score += 0.1;
        }

        score.clamp(0.0, 1.0)
    }

    /// Determines if a memory should be promoted to canonical.
    pub fn should_promote(&self, metrics: &PromotionMetrics) -> bool {
        self.calculate_score(metrics) >= self.promotion_threshold
    }

    /// Determines if a memory should be archived.
    pub fn should_archive(&self, metrics: &PromotionMetrics) -> bool {
        self.calculate_score(metrics) < 0.2 && metrics.age_hours > self.decay_after_hours
    }

    /// Checks if a learning should be promoted to agent identity (SOUL.md mutation).
    /// Requires 5+ recurrences AND high significance (≥7) AND personality alignment.
    pub fn should_promote_to_identity(
        &self,
        metrics: &PromotionMetrics,
        recurrence_count: usize,
    ) -> bool {
        recurrence_count >= 5
            && metrics.importance >= 7
            && self.calculate_score(metrics) >= self.promotion_threshold
    }

    /// Checks whether a proposed personality delta stays within the drift limit.
    /// Returns Ok(()) if within bounds, Err with distance if exceeded.
    pub fn check_drift_guard(&self, delta: &PersonalityDelta) -> Result<(), f32> {
        let distance = delta.euclidean_distance();
        if distance > self.personality_drift_limit {
            Err(distance)
        } else {
            Ok(())
        }
    }

    /// Computes Euclidean distance from the baseline personality.
    pub fn distance_from_baseline(&self) -> f32 {
        let baseline = match &self.baseline_personality {
            Some(b) => b,
            None => return 0.0,
        };
        let p = &self.personality;
        ((p.openness - baseline.openness).powi(2)
            + (p.conscientiousness - baseline.conscientiousness).powi(2)
            + (p.extraversion - baseline.extraversion).powi(2)
            + (p.agreeableness - baseline.agreeableness).powi(2)
            + (p.neuroticism - baseline.neuroticism).powi(2))
        .sqrt()
    }
}

impl Default for PromotionEngine {
    fn default() -> Self {
        Self::new(PersonalityTraits::default())
    }
}

/// Retention scoring mode.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum RetentionMode {
    /// OCEAN personality-weighted scoring (existing).
    Ocean,
    /// Ebbinghaus forgetting curve scoring (MEM-09).
    Ebbinghaus,
}

/// Ebbinghaus retention scoring configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EbbinghausConfig {
    /// Decay rate (lambda). Higher = faster forgetting.
    pub lambda: f32,
    /// Access reinforcement weight (sigma).
    pub sigma: f32,
    /// Tier thresholds.
    pub hot_threshold: f32,
    pub warm_threshold: f32,
    pub cold_threshold: f32,
}

impl Default for EbbinghausConfig {
    fn default() -> Self {
        Self {
            lambda: 0.1,
            sigma: 0.5,
            hot_threshold: 0.7,
            warm_threshold: 0.4,
            cold_threshold: 0.15,
        }
    }
}

/// Retention tier based on Ebbinghaus score.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum RetentionTier {
    /// High retention — full detail in vault.
    Hot,
    /// Medium retention — summary in vault.
    Warm,
    /// Low retention — archive reference only.
    Cold,
    /// Below threshold — eligible for eviction.
    Dead,
}

/// Ebbinghaus retention scoring engine (MEM-09).
///
/// Formula: `score = salience * exp(-λ * Δt) + σ * Σ(1/days_since_access)`
///
/// Where:
/// - `salience` is type-based (architecture=0.9, bug=0.7, pattern=0.8, etc.)
/// - `Δt` is time since last access in days
/// - `Σ(1/days_since_access)` is the sum of recency-weighted access scores
/// - `λ` is the decay rate
/// - `σ` is the access reinforcement weight
pub struct EbbinghausScorer {
    config: EbbinghausConfig,
}

impl EbbinghausScorer {
    pub fn new(config: EbbinghausConfig) -> Self {
        Self { config }
    }

    /// Computes the Ebbinghaus retention score for a memory.
    ///
    /// # Arguments
    /// - `category`: memory category (used for salience lookup)
    /// - `days_since_access`: days since last access
    /// - `access_timestamps`: ring buffer of access timestamps (epoch seconds)
    /// - `now`: current timestamp (epoch seconds)
    pub fn score(
        &self,
        category: &str,
        days_since_access: f32,
        access_timestamps: &[i64],
        now: i64,
    ) -> f32 {
        let salience = self.type_salience(category);

        // Exponential decay: salience * exp(-λ * Δt)
        let decay = salience * (-self.config.lambda * days_since_access).exp();

        // Access reinforcement: σ * Σ(1/days_since_access_i)
        let reinforcement: f32 = access_timestamps
            .iter()
            .map(|&ts| {
                let days = (now - ts).max(1) as f32 / 86400.0;
                1.0 / days
            })
            .sum();

        (decay + self.config.sigma * reinforcement).clamp(0.0, 1.0)
    }

    /// Returns the retention tier for a score.
    pub fn tier(&self, score: f32) -> RetentionTier {
        if score >= self.config.hot_threshold {
            RetentionTier::Hot
        } else if score >= self.config.warm_threshold {
            RetentionTier::Warm
        } else if score >= self.config.cold_threshold {
            RetentionTier::Cold
        } else {
            RetentionTier::Dead
        }
    }

    /// Returns the salience weight for a memory category.
    fn type_salience(&self, category: &str) -> f32 {
        match category {
            "architecture" | "design" => 0.9,
            "bug" | "error" | "regression" => 0.7,
            "pattern" | "convention" => 0.8,
            "preference" | "setting" => 0.85,
            "workflow" | "procedure" => 0.6,
            "fact" | "knowledge" => 0.5,
            "observation" | "exploration" => 0.4,
            "transcript" | "message" => 0.3,
            _ => 0.5, // default salience
        }
    }
}

impl Default for EbbinghausScorer {
    fn default() -> Self {
        Self::new(EbbinghausConfig::default())
    }
}

#[cfg(test)]
mod ebbinghaus_tests {
    use super::*;

    #[test]
    fn test_ebbinghaus_fresh_memory() {
        let scorer = EbbinghausScorer::default();
        let now = 1700000000;
        let score = scorer.score("architecture", 0.0, &[], now);
        assert!(score > 0.8); // fresh + high salience
    }

    #[test]
    fn test_ebbinghaus_old_memory_no_access() {
        let scorer = EbbinghausScorer::default();
        let now = 1700000000;
        let score = scorer.score("transcript", 30.0, &[], now);
        assert!(score < 0.2); // old + low salience + no access
    }

    #[test]
    fn test_ebbinghaus_access_reinforcement() {
        let scorer = EbbinghausScorer::default();
        let now = 1700000000;
        let recent_access = vec![now - 3600, now - 7200, now - 86400]; // 1h, 2h, 1d ago
        let score_with_access = scorer.score("fact", 7.0, &recent_access, now);
        let score_without = scorer.score("fact", 7.0, &[], now);
        assert!(score_with_access > score_without);
    }

    #[test]
    fn test_ebbinghaus_tier_assignment() {
        let scorer = EbbinghausScorer::default();
        assert_eq!(scorer.tier(0.9), RetentionTier::Hot);
        assert_eq!(scorer.tier(0.5), RetentionTier::Warm);
        assert_eq!(scorer.tier(0.2), RetentionTier::Cold);
        assert_eq!(scorer.tier(0.05), RetentionTier::Dead);
    }

    #[test]
    fn test_ebbinghaus_type_salience() {
        let scorer = EbbinghausScorer::default();
        assert!(scorer.type_salience("architecture") > scorer.type_salience("transcript"));
        assert!(scorer.type_salience("preference") > scorer.type_salience("observation"));
    }

    #[test]
    fn test_ebbinghaus_score_clamped() {
        let scorer = EbbinghausScorer::default();
        let now = 1700000000;
        let many_access: Vec<i64> = (0..100).map(|i| now - i * 60).collect();
        let score = scorer.score("architecture", 0.0, &many_access, now);
        assert!(score <= 1.0); // clamped
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_traits() {
        let traits = PersonalityTraits::default();
        assert_eq!(traits.openness, 0.5);
        assert_eq!(traits.conscientiousness, 0.5);
    }

    #[test]
    fn test_promotion_score_high_importance() {
        let engine = PromotionEngine::default();
        let metrics = PromotionMetrics {
            hit_count: 50,
            age_hours: 24.0,
            shannon_entropy: 0.3,
            importance: 9,
            category: "fact".to_string(),
        };
        let score = engine.calculate_score(&metrics);
        assert!(score > 0.5);
    }

    #[test]
    fn test_promotion_score_low_importance() {
        let engine = PromotionEngine::default();
        let metrics = PromotionMetrics {
            hit_count: 1,
            age_hours: 200.0,
            shannon_entropy: 0.9,
            importance: 2,
            category: "observation".to_string(),
        };
        let score = engine.calculate_score(&metrics);
        assert!(score < 0.5);
    }

    #[test]
    fn test_conscientiousness_bonus() {
        let traits = PersonalityTraits {
            conscientiousness: 1.0,
            ..Default::default()
        };
        let engine = PromotionEngine::new(traits);

        let security_metrics = PromotionMetrics {
            hit_count: 10,
            age_hours: 24.0,
            shannon_entropy: 0.5,
            importance: 5,
            category: "security".to_string(),
        };

        let neutral_metrics = PromotionMetrics {
            hit_count: 10,
            age_hours: 24.0,
            shannon_entropy: 0.5,
            importance: 5,
            category: "general".to_string(),
        };

        let security_score = engine.calculate_score(&security_metrics);
        let neutral_score = engine.calculate_score(&neutral_metrics);
        assert!(security_score > neutral_score);
    }

    #[test]
    fn test_openness_bonus() {
        let traits = PersonalityTraits {
            openness: 1.0,
            ..Default::default()
        };
        let engine = PromotionEngine::new(traits);

        let exploration_metrics = PromotionMetrics {
            hit_count: 5,
            age_hours: 24.0,
            shannon_entropy: 0.5,
            importance: 5,
            category: "observation".to_string(),
        };

        let fact_metrics = PromotionMetrics {
            hit_count: 5,
            age_hours: 24.0,
            shannon_entropy: 0.5,
            importance: 5,
            category: "fact".to_string(),
        };

        let explore_score = engine.calculate_score(&exploration_metrics);
        let fact_score = engine.calculate_score(&fact_metrics);
        assert!(explore_score > fact_score);
    }

    #[test]
    fn test_should_promote_threshold() {
        let engine = PromotionEngine::default();
        let high_metrics = PromotionMetrics {
            hit_count: 100,
            age_hours: 24.0,
            shannon_entropy: 0.1,
            importance: 9,
            category: "fact".to_string(),
        };
        assert!(engine.should_promote(&high_metrics));
    }

    #[test]
    fn test_should_archive_old_low_value() {
        let engine = PromotionEngine::default();
        let old_metrics = PromotionMetrics {
            hit_count: 0,
            age_hours: 500.0,
            shannon_entropy: 0.95,
            importance: 1,
            category: "observation".to_string(),
        };
        assert!(engine.should_archive(&old_metrics));
    }
}
