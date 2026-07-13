//! Free Model Router
//!
//! Cloud fallback chain used when no local model is available.
//! The user's primary chat model is configured separately via config/setup.
//! Gemma (user-selected variant) handles vision + embeddings by default.
//!
//! This router is ONLY used as a cloud fallback:
//!   1. `openrouter/free` — OpenRouter picks the best available free model
//!
//! The user can change any model (chat, vision, embedding) at any time
//! via the dashboard settings. This router does not constrain their choices.

use serde::{Deserialize, Serialize};
use tracing::warn;

/// Cloud fallback: OpenRouter free model router.
const FREE_ROUTER: &str = "openrouter/free";

/// Represents a model selection attempt.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelAttempt {
    pub model: String,
    pub attempt_number: u32,
    pub strategy: String,
}

/// Free model router — cloud fallback only.
pub struct FreeModelRouter;

impl FreeModelRouter {
    /// Returns the cloud fallback model.
    pub fn fallback() -> &'static str {
        FREE_ROUTER
    }

    /// Selects the next model based on the rotation strategy.
    /// Currently only has one option: openrouter/free.
    pub fn select_model(attempt: u32) -> ModelAttempt {
        if attempt > 0 {
            warn!(
                "Model selection: attempt {} — only cloud fallback available",
                attempt
            );
        }
        ModelAttempt {
            model: FREE_ROUTER.to_string(),
            attempt_number: attempt,
            strategy: "cloud_fallback".to_string(),
        }
    }

    /// Returns the cloud fallback model info for the dashboard.
    pub fn dashboard_model_list() -> Vec<ModelInfo> {
        vec![ModelInfo {
            name: FREE_ROUTER.to_string(),
            display_name: "OpenRouter Free".to_string(),
            tier: "cloud_fallback".to_string(),
            description: "Cloud fallback. OpenRouter picks the best free model automatically."
                .to_string(),
        }]
    }

    /// Validates that a model name is a known free model.
    pub fn is_free_model(model: &str) -> bool {
        model == FREE_ROUTER
    }
}

/// Model information for dashboard display.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelInfo {
    pub name: String,
    pub display_name: String,
    pub tier: String,
    pub description: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fallback_is_openrouter_free() {
        assert_eq!(FreeModelRouter::fallback(), "openrouter/free");
    }

    #[test]
    fn test_select_model_returns_free_router() {
        let attempt = FreeModelRouter::select_model(0);
        assert_eq!(attempt.model, "openrouter/free");
        assert_eq!(attempt.strategy, "cloud_fallback");
    }

    #[test]
    fn test_is_free_model() {
        assert!(FreeModelRouter::is_free_model("openrouter/free"));
        assert!(!FreeModelRouter::is_free_model("anthropic/claude-opus"));
        assert!(!FreeModelRouter::is_free_model("gpt-4"));
    }

    #[test]
    fn test_dashboard_model_list() {
        let models = FreeModelRouter::dashboard_model_list();
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].name, "openrouter/free");
    }
}
