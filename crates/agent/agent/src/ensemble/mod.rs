//! Multi-Model Ensemble
//!
//! Routes queries to multiple LLM providers simultaneously and selects
//! the best response using configurable strategies.
//!
//! # Strategies
//! - `BestOfN` — Run N providers, return the first successful response
//! - `Consensus` — Run N providers, return response with highest agreement
//! - `Fallback` — Try providers in order, return first success
//!
//! # Usage
//! ```ignore
//! let ensemble = EnsembleRouter::new(vec![
//!     ("openrouter/hunter-alpha", 0.7),
//!     ("openrouter/free", 0.5),
//! ]);
//! let result = ensemble.query("Explain recursion").await?;
//! ```

use serde::{Deserialize, Serialize};

/// A single provider response with metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderResponse {
    pub provider: String,
    pub model: String,
    pub content: String,
    pub latency_ms: u64,
    pub token_count: usize,
}

/// Ensemble selection strategy.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum EnsembleStrategy {
    /// First successful response wins.
    BestOfN,
    /// All providers run, pick most common/quality response.
    Consensus,
    /// Try providers in order, stop at first success.
    Fallback,
}

/// A provider in the ensemble.
#[derive(Debug, Clone)]
pub struct EnsembleProvider {
    pub model: String,
    pub temperature: f32,
}

/// Multi-model ensemble router.
pub struct EnsembleRouter {
    providers: Vec<EnsembleProvider>,
    strategy: EnsembleStrategy,
}

impl EnsembleRouter {
    /// Creates a new ensemble router with the given providers and strategy.
    pub fn new(providers: Vec<EnsembleProvider>, strategy: EnsembleStrategy) -> Self {
        Self {
            providers,
            strategy,
        }
    }

    /// Creates a fallback router with the default model chain.
    pub fn with_fallback_chain() -> Self {
        Self {
            providers: vec![
                EnsembleProvider {
                    model: "gemma4".to_string(),
                    temperature: 0.7,
                },
                EnsembleProvider {
                    model: "openrouter/free".to_string(),
                    temperature: 0.7,
                },
            ],
            strategy: EnsembleStrategy::Fallback,
        }
    }

    /// Returns the list of configured providers.
    pub fn providers(&self) -> &[EnsembleProvider] {
        &self.providers
    }

    /// Returns the configured strategy.
    pub fn strategy(&self) -> &EnsembleStrategy {
        &self.strategy
    }

    /// Selects the best model based on strategy and attempt number.
    pub fn select_model(&self, attempt: u32) -> Option<&EnsembleProvider> {
        match self.strategy {
            EnsembleStrategy::Fallback => self.providers.get(attempt as usize),
            EnsembleStrategy::BestOfN => self.providers.get(attempt as usize),
            EnsembleStrategy::Consensus => self.providers.first(),
        }
    }

    /// Adds a provider to the ensemble.
    pub fn add_provider(&mut self, model: String, temperature: f32) {
        self.providers.push(EnsembleProvider { model, temperature });
    }

    /// Evaluates response quality using heuristic scoring.
    ///
    /// Scoring factors:
    /// - Length (longer is generally more complete)
    /// - Presence of code blocks (if expected)
    /// - Absence of error phrases
    /// - Token efficiency (content per token)
    pub fn score_response(response: &ProviderResponse) -> f32 {
        let mut score: f32 = 0.0;

        // Length score (up to 0.3)
        let len_score = (response.content.len() as f32 / 2000.0).min(0.3);
        score += len_score;

        // Latency penalty (up to -0.2)
        if response.latency_ms > 10000 {
            score -= 0.2;
        } else if response.latency_ms > 5000 {
            score -= 0.1;
        }

        // Error phrase penalty (-0.5)
        let lower = response.content.to_lowercase();
        if lower.contains("error") || lower.contains("failed") || lower.contains("cannot") {
            score -= 0.5;
        }

        // Code block bonus (+0.1)
        if response.content.contains("```") {
            score += 0.1;
        }

        score.clamp(0.0, 1.0)
    }

    /// Picks the best response from a set of responses.
    pub fn pick_best(responses: &[ProviderResponse]) -> Option<&ProviderResponse> {
        responses.iter().max_by(|a, b| {
            Self::score_response(a)
                .partial_cmp(&Self::score_response(b))
                .unwrap_or(std::cmp::Ordering::Equal)
        })
    }

    /// Returns all providers for Consensus strategy — caller runs them in parallel.
    pub fn all_providers(&self) -> &[EnsembleProvider] {
        &self.providers
    }

    /// Selects the best response from multiple provider responses (Consensus strategy).
    /// Returns the provider index and response with the highest quality score.
    pub fn select_consensus(responses: &[ProviderResponse]) -> Option<(usize, &ProviderResponse)> {
        responses.iter().enumerate().max_by(|(_, a), (_, b)| {
            Self::score_response(a)
                .partial_cmp(&Self::score_response(b))
                .unwrap_or(std::cmp::Ordering::Equal)
        })
    }
}

impl Default for EnsembleRouter {
    fn default() -> Self {
        Self::with_fallback_chain()
    }
}

#[cfg(test)]
