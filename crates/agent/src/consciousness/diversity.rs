//! Anti-Echo-Chamber — detects and breaks reasoning convergence across agents.

/// Convergence status of agent outputs.
#[derive(Debug, Clone)]
pub enum ConvergenceStatus {
    /// Agent outputs are sufficiently diverse.
    Healthy,
    /// Agent outputs have converged — reasoning is repetitive.
    Converged {
        /// How severe the convergence is (0.0-1.0).
        severity: f64,
        /// Recommended action to break convergence.
        recommendation: DiversityAction,
    },
}

/// Actions to break reasoning convergence.
#[derive(Debug, Clone)]
pub enum DiversityAction {
    /// Spike temperature to encourage divergent thinking.
    SpikeTemperature,
    /// Inject contradictory parameters.
    InjectContradiction,
    /// Force re-evaluation of assumptions.
    ReEvaluate,
}

/// Detects and prevents reasoning convergence across agents.
pub struct AntiEchoChamber {
    convergence_threshold: f64,
}

impl Default for AntiEchoChamber {
    fn default() -> Self {
        Self::new()
    }
}

impl AntiEchoChamber {
    pub fn new() -> Self {
        Self {
            convergence_threshold: 0.7,
        }
    }

    /// Check if agent outputs have converged (are too similar).
    pub fn check_convergence(&self, agent_outputs: &[String]) -> ConvergenceStatus {
        if agent_outputs.len() < 2 {
            return ConvergenceStatus::Healthy;
        }

        let avg_similarity = self.average_similarity(agent_outputs);

        if avg_similarity > self.convergence_threshold {
            ConvergenceStatus::Converged {
                severity: avg_similarity,
                recommendation: DiversityAction::SpikeTemperature,
            }
        } else {
            ConvergenceStatus::Healthy
        }
    }

    /// Compute average pairwise similarity between outputs.
    fn average_similarity(&self, outputs: &[String]) -> f64 {
        let mut total = 0.0;
        let mut count = 0;

        for i in 0..outputs.len() {
            for j in (i + 1)..outputs.len() {
                total += self.simple_similarity(&outputs[i], &outputs[j]);
                count += 1;
            }
        }

        if count > 0 {
            total / count as f64
        } else {
            0.0
        }
    }

    /// Simple word-overlap similarity (Jaccard index).
    /// Case-insensitive, uses char count for word length filter.
    fn simple_similarity(&self, a: &str, b: &str) -> f64 {
        use std::collections::HashSet;
        let words_a: HashSet<String> = a
            .split_whitespace()
            .filter(|w| w.chars().count() > 3)
            .map(|w| w.to_lowercase())
            .collect();
        let words_b: HashSet<String> = b
            .split_whitespace()
            .filter(|w| w.chars().count() > 3)
            .map(|w| w.to_lowercase())
            .collect();

        if words_a.is_empty() || words_b.is_empty() {
            return 0.0;
        }

        let intersection = words_a.intersection(&words_b).count();
        let union = words_a.union(&words_b).count();

        intersection as f64 / union as f64
    }
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)]
mod tests {
    use super::*;

    #[test]
    fn test_healthy_diversity() {
        let anti = AntiEchoChamber::new();
        let outputs = vec![
            "The system is running well with no issues.".to_string(),
            "I noticed the memory engine could be optimized.".to_string(),
            "The build completed successfully after refactoring.".to_string(),
        ];
        match anti.check_convergence(&outputs) {
            ConvergenceStatus::Healthy => {}
            _ => panic!("Expected healthy diversity"),
        }
    }

    #[test]
    fn test_convergence_detected() {
        let anti = AntiEchoChamber::new();
        let outputs = vec![
            "The system is running well and everything is fine.".to_string(),
            "The system is running well and everything is working fine.".to_string(),
            "The system is running well and everything is operating fine.".to_string(),
        ];
        match anti.check_convergence(&outputs) {
            ConvergenceStatus::Converged { .. } => {}
            _ => panic!("Expected convergence to be detected"),
        }
    }

    #[test]
    fn test_insufficient_outputs() {
        let anti = AntiEchoChamber::new();
        let outputs = vec!["single output".to_string()];
        match anti.check_convergence(&outputs) {
            ConvergenceStatus::Healthy => {}
            _ => panic!("Expected healthy with single output"),
        }
    }
}
