//! OCEAN personality-driven compression scaling.
//!
//! Adjusts compression aggressiveness based on the agent's OCEAN personality:
//! - High Openness: reduce compression 20%, preserve exploration output
//! - High Conscientiousness: increase compression, strip all boilerplate
//! - High Neuroticism: trigger L2 earlier (60% instead of 75%)

use crate::compact::l2::L2Thresholds;
use savant_core::types::PersonalityTraits;

/// OCEAN-aware compression scaler.
#[derive(Debug, Clone)]
pub struct OceanScaler;

impl OceanScaler {
    /// Scales L2 thresholds based on OCEAN personality.
    /// Returns adjusted thresholds.
    pub fn scale_thresholds(base: &L2Thresholds, ocean: &PersonalityTraits) -> L2Thresholds {
        let mut adjusted = base.clone();

        // High Openness → reduce compression (preserve more)
        if ocean.openness > 0.7 {
            adjusted.tool_eviction = (base.tool_eviction + 0.10).min(0.90);
            adjusted.llm_summarization = (base.llm_summarization + 0.05).min(0.95);
        }

        // High Conscientiousness → increase compression (strip more)
        if ocean.conscientiousness > 0.7 {
            adjusted.tool_eviction = (base.tool_eviction - 0.10).max(0.50);
            adjusted.llm_summarization = (base.llm_summarization - 0.05).max(0.70);
        }

        // High Neuroticism → trigger L2 earlier (prioritize stability)
        if ocean.neuroticism > 0.7 {
            adjusted.tool_eviction = 0.60;
            adjusted.llm_summarization = 0.75;
        }

        adjusted
    }

    /// Evolves personality traits based on interaction deltas and returns
    /// the distance from the previous state (0.0 = identical, higher = more change).
    pub fn evolve_and_measure(
        current: &PersonalityTraits,
        delta: &savant_core::types::PersonalityDelta,
    ) -> (PersonalityTraits, f32) {
        let evolved = current.evolve(delta);
        let distance = current.distance(&evolved);
        (evolved, distance)
    }

    /// Returns a compression aggressiveness multiplier based on OCEAN.
    /// 1.0 = normal, <1.0 = less aggressive, >1.0 = more aggressive.
    pub fn aggressiveness_multiplier(ocean: &PersonalityTraits) -> f32 {
        let mut multiplier = 1.0;

        // Openness reduces compression
        if ocean.openness > 0.7 {
            multiplier -= 0.2;
        } else if ocean.openness < 0.3 {
            multiplier += 0.1;
        }

        // Conscientiousness increases compression
        if ocean.conscientiousness > 0.7 {
            multiplier += 0.15;
        }

        // Neuroticism increases compression (earlier triggering)
        if ocean.neuroticism > 0.7 {
            multiplier += 0.1;
        }

        f32::clamp(multiplier, 0.5, 1.5)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_high_openness_reduces_compression() {
        let base = L2Thresholds::default();
        let ocean = PersonalityTraits {
            openness: 0.9,
            conscientiousness: 0.5,
            extraversion: 0.5,
            agreeableness: 0.5,
            neuroticism: 0.5,
        };
        let adjusted = OceanScaler::scale_thresholds(&base, &ocean);
        assert!(adjusted.tool_eviction > base.tool_eviction);
    }

    #[test]
    fn test_high_conscientiousness_increases_compression() {
        let base = L2Thresholds::default();
        let ocean = PersonalityTraits {
            openness: 0.5,
            conscientiousness: 0.9,
            extraversion: 0.5,
            agreeableness: 0.5,
            neuroticism: 0.5,
        };
        let adjusted = OceanScaler::scale_thresholds(&base, &ocean);
        assert!(adjusted.tool_eviction < base.tool_eviction);
    }

    #[test]
    fn test_high_neuroticism_triggers_earlier() {
        let base = L2Thresholds::default();
        let ocean = PersonalityTraits {
            openness: 0.5,
            conscientiousness: 0.5,
            extraversion: 0.5,
            agreeableness: 0.5,
            neuroticism: 0.9,
        };
        let adjusted = OceanScaler::scale_thresholds(&base, &ocean);
        assert_eq!(adjusted.tool_eviction, 0.60);
    }

    #[test]
    fn test_aggressiveness_multiplier() {
        let ocean = PersonalityTraits {
            openness: 0.5,
            conscientiousness: 0.5,
            extraversion: 0.5,
            agreeableness: 0.5,
            neuroticism: 0.5,
        };
        let mult = OceanScaler::aggressiveness_multiplier(&ocean);
        assert!((mult - 1.0).abs() < 0.01);
    }
}
