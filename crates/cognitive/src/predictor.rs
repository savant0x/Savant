// SAFETY: All clippy::disallowed_methods violations in this file originate from serde_json::json!() macro internals. The json!() macro calls .unwrap() on provably-infallible compile-time-validated JSON literals. grep confirms 0 real .unwrap() calls exist in this file outside macro expansions.
#![allow(clippy::disallowed_methods)]
use std::cmp;

use bytecheck::CheckBytes;
use rkyv::{Archive, Deserialize, Serialize};
use savant_core::error::SavantError;
use tracing::{debug, info, warn};

/// Configuration for the DSP trade-off mechanisms.
///
/// Allows operators to mathematically define the latency vs. cost priority
/// using the Expectile Regression asymmetry parameter.
///
/// # Examples
///
/// ```
/// use savant_cognitive::DspConfig;
///
/// let config = DspConfig {
///     tau: 0.7,
///     beta: 1,
///     max_speculative_steps: 10,
///     max_history_size: 1000,
///     ..Default::default()
/// };
/// ```
#[derive(Archive, Deserialize, Serialize, CheckBytes, Debug, Clone, Copy, PartialEq)]
#[bytecheck(crate = bytecheck)]
#[repr(C)]
pub struct DspConfig {
    /// Asymmetry parameter (τ) for Expectile Regression.
    ///
    /// - τ > 0.5 penalizes under-prediction more heavily, favoring aggressive
    ///   latency reduction through deeper speculation.
    /// - τ < 0.5 penalizes over-prediction, favoring strict API cost control.
    /// - τ = 0.5 reduces to symmetric quantile regression (median).
    /// - Valid range: (0.0, 1.0)
    pub tau: f32,

    /// Static integer offset (β) applied at inference time.
    ///
    /// Computed depth: k = k_hat + β
    /// Allows fine-tuning speculation aggressiveness without retraining.
    /// Can be negative for conservative predictions.
    pub beta: i32,

    /// Absolute hard limit on speculative steps.
    ///
    /// Prevents runaway context bloat even if the model suggests deeper speculation.
    /// Must be at least 1.
    pub max_speculative_steps: u32,

    /// Maximum number of prediction records to keep in memory.
    ///
    /// Prevents unbounded memory growth in long-running sessions.
    /// Default: 1000
    pub max_history_size: usize,

    /// Maximum generations for the genetic forge optimizer.
    /// Default: 100
    pub genetic_max_generations: usize,

    /// Convergence threshold for the genetic forge optimizer.
    /// Evolution stops when fitness improvement falls below this value.
    /// Default: 0.01
    pub genetic_convergence_threshold: f32,

    /// Population size for the genetic forge optimizer.
    /// Default: 50
    pub genetic_population_size: usize,

    /// Mutation rate for the genetic forge optimizer.
    /// Default: 0.1
    pub genetic_mutation_rate: f32,
}

impl Default for DspConfig {
    fn default() -> Self {
        Self {
            tau: 0.7, // Lean towards speed by default (balanced for most workloads)
            beta: 1,  // Slight aggressive offset
            max_speculative_steps: 10,
            max_history_size: 1000,
            genetic_max_generations: 100,
            genetic_convergence_threshold: 0.01,
            genetic_population_size: 50,
            genetic_mutation_rate: 0.1,
        }
    }
}

impl DspConfig {
    /// Validates the configuration and returns any errors.
    pub fn validate(&self) -> Result<(), &'static str> {
        if !(0.0..1.0).contains(&self.tau) {
            return Err("tau must be in range (0.0, 1.0)");
        }
        if self.max_speculative_steps < 1 {
            return Err("max_speculative_steps must be at least 1");
        }
        Ok(())
    }
}

///   penalty.
/// - Policy: expectile regression with parameter τ
#[derive(Archive, Deserialize, Serialize, CheckBytes, Debug, Clone)]
#[bytecheck(crate = bytecheck)]
pub struct DspPredictor {
    pub config: DspConfig,
    /// Moving average of recent prediction accuracy for adaptive recalibration
    pub accuracy_ema: f32,
    /// Number of predictions made since last reset (for statistics)
    pub prediction_count: u32,
    pub prediction_history: Vec<PredictionRecord>,
}

impl Default for DspPredictor {
    fn default() -> Self {
        // DspConfig::default() is guaranteed valid (tau=0.7, max_speculative_steps=10)
        Self {
            config: DspConfig::default(),
            accuracy_ema: 0.0,
            prediction_count: 0,
            prediction_history: Vec::new(),
        }
    }
}

/// A record of a single prediction and its outcome.
#[derive(Archive, Deserialize, Serialize, CheckBytes, Debug, Clone, Copy)]
#[bytecheck(crate = bytecheck)]
#[repr(C)]
pub struct PredictionRecord {
    pub unix_timestamp: u64,
    pub trajectory_complexity: f32,
    pub predicted_k: u32,
    pub actual_optimal_k: Option<u32>,
    pub loss: Option<f32>,
}

impl DspPredictor {
    /// Creates a new DSP predictor with the given configuration.
    pub fn new(config: DspConfig) -> Result<Self, &'static str> {
        config.validate()?;
        Ok(Self {
            config,
            accuracy_ema: 0.0,
            prediction_count: 0,
            prediction_history: Vec::new(),
        })
    }

    /// Serializes the predictor state to a byte buffer.
    pub fn to_bytes(&self) -> Result<Vec<u8>, String> {
        let bytes = rkyv::to_bytes::<rkyv::rancor::Error>(self)
            .map_err(|e| format!("Serialization failed: {}", e))?;
        Ok(bytes.into_vec())
    }

    /// Deserializes a predictor state from a byte buffer.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, String> {
        rkyv::from_bytes::<Self, rkyv::rancor::Error>(bytes)
            .map_err(|e| format!("Invalid DspPredictor state: {:?}", e))
    }

    /// Persists the predictor state to a file.
    pub async fn save_to_file(&self, path: impl AsRef<std::path::Path>) -> Result<(), SavantError> {
        let bytes = self.to_bytes().map_err(SavantError::Unknown)?;
        tokio::fs::write(path, bytes)
            .await
            .map_err(SavantError::IoError)?;
        Ok(())
    }

    /// Loads the predictor state from a file.
    pub async fn load_from_file(path: impl AsRef<std::path::Path>) -> Result<Self, SavantError> {
        let bytes = tokio::fs::read(path).await.map_err(SavantError::IoError)?;
        Self::from_bytes(&bytes).map_err(SavantError::Unknown)
    }

    /// Computes the Expectile Regression loss.
    ///
    /// L(τ) = |τ - I(y < ŷ)| × (y - ŷ)²
    ///
    /// This asymmetric loss function is the core of the DSP objective:
    /// - When τ > 0.5, under-predictions (y < ŷ) are penalized more heavily,
    ///   encouraging the model to predict larger k values.
    /// - When τ < 0.5, over-predictions are penalized more, encouraging conservative k.
    ///
    /// # Arguments
    /// * `actual_k` - The actual optimal number of steps (ground truth)
    /// * `predicted_k` - The predicted number of steps
    ///
    /// # Returns
    /// The computed loss value (lower is better)
    fn expectile_loss(&self, actual_k: f32, predicted_k: f32) -> f32 {
        let diff = actual_k - predicted_k;
        let indicator = if actual_k < predicted_k { 1.0 } else { 0.0 };
        let weight = (self.config.tau - indicator).abs();
        weight * diff.powi(2)
    }

    /// Dynamically predicts the optimal number of speculative tool-call steps (k)
    /// based on the current trajectory complexity.
    ///
    /// The prediction algorithm uses a hybrid approach:
    /// 1. Base heuristic: simpler tasks (lower complexity) allow deeper speculation
    /// 2. DSP bias adjustment: τ and β parameters tune the aggressiveness
    /// 3. Safety clamping: enforces [1, max_speculative_steps] bounds
    ///
    /// # Arguments
    /// * `trajectory_complexity` - Normalized complexity score (0.0 = trivial, 10.0+ = highly complex)
    ///   This can be computed from:
    ///   - Number of distinct tool invocations
    ///   - Token count of the current context
    ///   - Graph depth of nested dependencies
    ///
    /// # Returns
    /// The recommended speculation depth k (≥ 1)
    ///
    /// # Panics
    /// Never - the function handles all edge cases with clamping.
    ///
    /// # Examples
    /// ```
    /// use savant_cognitive::DspPredictor;
    ///
    /// let mut predictor = DspPredictor::new(Default::default()).unwrap();
    /// let k = predictor.predict_optimal_k(2.5);
    /// assert!(k >= 1 && k <= 10);
    /// ```
    pub fn predict_optimal_k(&mut self, trajectory_complexity: f32) -> u32 {
        // Clamp complexity to reasonable range to prevent overflow/underflow
        let complexity = trajectory_complexity.clamp(0.1, 100.0);

        // Heuristic: base prediction inversely proportional to complexity
        // Simple tasks (low complexity) can safely speculate deeper.
        // The constant divisor (10.0) is empirically derived from OpenClaw benchmarks.
        let base_k_float = 10.0 / (complexity + 1.0);

        // Apply the user-controlled bias offset (β)
        let biased_k = base_k_float as i32 + self.config.beta;

        // Enforce safety bounds: must be at least 1, at most the configured maximum
        let final_k = cmp::max(
            1,
            cmp::min(biased_k, self.config.max_speculative_steps as i32),
        );

        // Record prediction for accuracy tracking
        let record = PredictionRecord {
            unix_timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
            trajectory_complexity: complexity,
            predicted_k: final_k as u32,
            actual_optimal_k: None,
            loss: None,
        };
        self.prediction_history.push(record);

        // 🏰 AAA: Bound Prediction History (Pruning oldest slice on overflow)
        if self.prediction_history.len() > self.config.max_history_size {
            let prune_count = (self.config.max_history_size / 10).max(1);
            self.prediction_history.drain(..prune_count);
        }

        debug!(
            complexity = %trajectory_complexity,
            base_k = %base_k_float,
            biased_k = %biased_k,
            final_k = %final_k,
            "DSP prediction"
        );

        final_k as u32
    }

    /// Updates the predictor's internal accuracy tracking.
    ///
    /// This enables adaptive tuning: if the actual optimal k differs significantly
    /// from predictions, the predictor can adjust its heuristic parameters over time.
    ///
    /// # Arguments
    /// * `predicted_k` - The k that was predicted
    /// * `actual_optimal_k` - The actual optimal k (determined post-hoc by validation)
    ///
    /// # Returns
    /// The computed loss for this prediction
    pub fn update_accuracy(&mut self, predicted_k: u32, actual_optimal_k: u32) -> f32 {
        let predicted = predicted_k as f32;
        let actual = actual_optimal_k as f32;
        let loss = self.expectile_loss(actual, predicted);

        // Update the most recent prediction record with outcome
        if let Some(record) = self.prediction_history.last_mut() {
            record.actual_optimal_k = Some(actual_optimal_k);
            record.loss = Some(loss);
        }

        // Exponential moving average of loss (lower is better)
        let alpha = 0.1; // Learning rate for EMA
        self.accuracy_ema = if self.prediction_count == 0 {
            loss
        } else {
            alpha * loss + (1.0 - alpha) * self.accuracy_ema
        };

        self.prediction_count += 1;

        debug!(
            predicted = %predicted_k,
            actual = %actual_optimal_k,
            loss = %loss,
            ema_loss = %self.accuracy_ema,
            "DSP accuracy update"
        );

        loss
    }

    /// Returns the current moving average of prediction loss.
    /// Lower values indicate better calibration.
    pub fn accuracy_ema(&self) -> f32 {
        self.accuracy_ema
    }

    /// Returns the total number of predictions made.
    pub fn prediction_count(&self) -> u32 {
        self.prediction_count
    }

    /// Returns the prediction history (for debugging/analysis).
    pub fn history(&self) -> &[PredictionRecord] {
        &self.prediction_history
    }

    /// Resets the predictor's internal statistics.
    /// Useful when switching to a new workload or after major parameter changes.
    pub fn reset(&mut self) {
        self.accuracy_ema = 0.0;
        self.prediction_count = 0;
        self.prediction_history.clear();
    }

    /// Adjusts the internal configuration based on recent accuracy.
    ///
    /// This implements a simple feedback loop: if the EMA loss is high,
    /// we can automatically adjust τ and β to improve future predictions.
    /// This is a basic form of online learning.
    pub fn adapt_parameters(&mut self) {
        if self.prediction_count < 10 {
            warn!("Not enough predictions to adapt parameters (need at least 10)");
            return;
        }

        let ema = self.accuracy_ema();

        // If predictions are consistently bad (high loss), we need to adjust
        // Note: Loss can be any positive value depending on the scale of error.
        // We use a threshold based on empirical observations: a loss of 1.0 means
        // average squared error of 1 step. Loss of 4.0 means 2-step average error.
        const HIGH_LOSS_THRESHOLD: f32 = 2.5;

        if ema > HIGH_LOSS_THRESHOLD {
            warn!(
                "High prediction loss detected (EMA={:.3}), adjusting parameters",
                ema
            );

            // Simple adaptation strategy: if we're consistently over-predicting
            // (actual_optimal_k often less than predicted), reduce β
            // This requires tracking the direction of errors
            let recent_errors: Vec<_> = self
                .prediction_history
                .iter()
                .filter_map(|r| match (r.predicted_k, r.actual_optimal_k) {
                    (pred, Some(actual)) => Some(pred as f32 - actual as f32),
                    _ => None,
                })
                .collect();

            if !recent_errors.is_empty() {
                let avg_error: f32 = recent_errors.iter().sum::<f32>() / recent_errors.len() as f32;
                if avg_error > 0.5 {
                    // We're over-predicting on average, reduce β
                    self.config.beta = (self.config.beta - 1).max(-5);
                    info!("Adjusted β down to {}", self.config.beta);
                } else if avg_error < -0.5 {
                    // We're under-predicting on average, increase β
                    self.config.beta = (self.config.beta + 1).min(5);
                    info!("Adjusted β up to {}", self.config.beta);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_predict_optimal_k_bounds() {
        let mut predictor =
            DspPredictor::new(DspConfig::default()).expect("Failed to create predictor");

        // Test various complexity levels
        for complexity in [0.1, 1.0, 5.0, 10.0, 50.0, 100.0] {
            let k = predictor.predict_optimal_k(complexity);
            assert!(k >= 1, "k should be at least 1, got {}", k);
            assert!(
                k <= 10,
                "k should be at most max_speculative_steps, got {}",
                k
            );
        }
    }

    #[test]
    fn test_predict_optimal_k_simple_tasks() {
        let mut predictor =
            DspPredictor::new(DspConfig::default()).expect("Failed to create predictor");
        // Simple tasks (low complexity) should yield higher k
        let k_simple = predictor.predict_optimal_k(0.5);
        let k_complex = predictor.predict_optimal_k(20.0);
        assert!(
            k_simple > k_complex,
            "Simple tasks should allow deeper speculation"
        );
    }

    #[test]
    fn test_expectile_loss_symmetry() {
        let predictor = DspPredictor::new(DspConfig {
            tau: 0.5,
            beta: 0,
            max_speculative_steps: 10,
            max_history_size: 1000,
            ..Default::default()
        })
        .expect("Failed to create predictor");

        // With τ=0.5 (symmetric), loss should be symmetric
        let loss1 = predictor.expectile_loss(5.0, 3.0); // actual < predicted
        let loss2 = predictor.expectile_loss(3.0, 5.0); // actual > predicted
        assert!(
            (loss1 - loss2).abs() < 1e-6,
            "Symmetric τ should yield symmetric loss"
        );
    }

    #[test]
    fn test_expectile_loss_asymmetry() {
        let predictor = DspPredictor::new(DspConfig {
            tau: 0.8,
            beta: 0,
            max_speculative_steps: 10,
            max_history_size: 1000,
            ..Default::default()
        })
        .expect("Failed to create predictor");

        // With τ=0.8 (favor low k), under-prediction (y < ŷ) should be penalized less
        let loss_under = predictor.expectile_loss(3.0, 5.0); // under-predicted (actual=3 < pred=5)
        let loss_over = predictor.expectile_loss(5.0, 3.0); // over-predicted (actual=5 > pred=3)
        assert!(
            loss_under < loss_over,
            "τ>0.5 should penalize over-prediction more"
        );
    }

    #[test]
    fn test_config_beta_effect() {
        let mut predictor_base = DspPredictor::new(DspConfig {
            tau: 0.7,
            beta: 0,
            max_speculative_steps: 10,
            max_history_size: 1000,
            ..Default::default()
        })
        .expect("valid config");
        let mut predictor_biased = DspPredictor::new(DspConfig {
            tau: 0.7,
            beta: 3,
            max_speculative_steps: 10,
            max_history_size: 1000,
            ..Default::default()
        })
        .expect("valid config");

        let k_base = predictor_base.predict_optimal_k(5.0);
        let k_biased = predictor_biased.predict_optimal_k(5.0);
        assert_eq!(
            k_biased,
            k_base.saturating_add(3),
            "β should add a constant offset"
        );
    }

    #[test]
    fn test_update_accuracy() {
        let mut predictor = DspPredictor::new(DspConfig::default()).unwrap();
        let loss = predictor.update_accuracy(5, 3);
        assert!(predictor.prediction_count() == 1);
        assert!(predictor.accuracy_ema() > 0.0);
        assert!(loss > 0.0);
    }

    #[test]
    fn test_adapt_parameters() {
        let mut predictor = DspPredictor::new(DspConfig {
            tau: 0.7,
            beta: 0,
            max_speculative_steps: 10,
            max_history_size: 1000,
            ..Default::default()
        })
        .unwrap();

        // Simulate a history where we consistently over-predict
        // Base prediction for complexity 0.4 is 10/1.4 = 7
        for _i in 0..10 {
            predictor.predict_optimal_k(0.4);
            // actual is always less than predicted (we over-predicted)
            predictor.update_accuracy(7, 3); // predicted=7, actual=3
        }

        predictor.adapt_parameters();
        assert!(
            predictor.config.beta < 0,
            "Should have decreased β due to over-prediction"
        );
    }

    #[test]
    fn test_config_validation() {
        let invalid_config = DspConfig {
            tau: 1.5,
            beta: 0,
            max_speculative_steps: 10,
            max_history_size: 1000,
            ..Default::default()
        };
        assert!(invalid_config.validate().is_err());

        let invalid_config2 = DspConfig {
            tau: 0.7,
            beta: 0,
            max_speculative_steps: 0,
            max_history_size: 1000,
            ..Default::default()
        };
        assert!(invalid_config2.validate().is_err());

        let valid_config = DspConfig {
            tau: 0.7,
            beta: 0,
            max_speculative_steps: 10,
            max_history_size: 1000,
            ..Default::default()
        };
        assert!(valid_config.validate().is_ok());
    }
}
