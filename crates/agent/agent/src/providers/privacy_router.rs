//! Privacy Router — content-aware routing that keeps sensitive data on-device.
//!
//! Scans conversation messages for PII before they reach the LLM provider.
//! When sensitivity exceeds a threshold, routes to a local model instead of cloud.

use savant_core::types::ChatMessage;
use savant_security::pii::PiiDetector;
use serde::{Deserialize, Serialize};

/// Configuration for the privacy router.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrivacyConfig {
    /// Whether privacy-aware routing is enabled.
    pub enabled: bool,
    /// Sensitivity score threshold above which content is forced to local models.
    pub sensitivity_threshold: f32,
    /// Model identifiers for local (on-device) inference.
    pub local_models: Vec<String>,
    /// Model identifiers for cloud inference.
    pub cloud_models: Vec<String>,
    /// Whether to log routing decisions via tracing.
    pub log_decisions: bool,
}

impl Default for PrivacyConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            sensitivity_threshold: 0.7,
            local_models: vec!["gemma4".to_string()],
            cloud_models: Vec::new(),
            log_decisions: true,
        }
    }
}

/// The outcome of a privacy routing decision.
#[derive(Debug, Clone)]
pub enum RoutingDecision {
    /// Route to a local model — content is too sensitive for cloud.
    Local {
        model: String,
        reason: String,
        score: f32,
    },
    /// Route to a cloud model — content is clean.
    Cloud {
        model: String,
        reason: String,
        score: f32,
    },
    /// Ambiguous — present user with both options.
    UserChoice {
        local: String,
        cloud: String,
        reason: String,
        score: f32,
    },
}

impl RoutingDecision {
    /// Returns the selected model regardless of variant.
    pub fn selected_model(&self) -> &str {
        match self {
            Self::Local { model, .. } => model,
            Self::Cloud { model, .. } => model,
            Self::UserChoice { local, .. } => local, // safe default: prefer local
        }
    }

    /// Returns the sensitivity score.
    pub fn score(&self) -> f32 {
        match self {
            Self::Local { score, .. } => *score,
            Self::Cloud { score, .. } => *score,
            Self::UserChoice { score, .. } => *score,
        }
    }
}

/// Content-aware privacy router.
pub struct PrivacyRouter {
    detector: PiiDetector,
    config: PrivacyConfig,
}

impl PrivacyRouter {
    /// Create a new privacy router from configuration.
    pub fn new(config: PrivacyConfig) -> Self {
        Self {
            detector: PiiDetector::new(),
            config,
        }
    }

    /// Route a conversation based on PII content.
    /// Scans all user messages and returns a routing decision.
    pub fn route(&self, messages: &[ChatMessage]) -> RoutingDecision {
        self.route_with_override(messages, None)
    }

    /// Route with an optional forced override.
    pub fn route_with_override(
        &self,
        messages: &[ChatMessage],
        force_local: Option<bool>,
    ) -> RoutingDecision {
        // If forced, respect the override
        if let Some(to_local) = force_local {
            return if to_local {
                RoutingDecision::Local {
                    model: self.local_model().to_string(),
                    reason: "User override: forced local".to_string(),
                    score: 0.0,
                }
            } else {
                RoutingDecision::Cloud {
                    model: self.cloud_model().to_string(),
                    reason: "User override: forced cloud".to_string(),
                    score: 0.0,
                }
            };
        }

        // If not enabled, always route to cloud
        if !self.config.enabled {
            return RoutingDecision::Cloud {
                model: self.cloud_model().to_string(),
                reason: "Privacy routing disabled".to_string(),
                score: 0.0,
            };
        }

        // Scan all user messages for PII
        // Fast-path: use contains_pii() before full scan for performance
        let mut max_score: f32 = 0.0;
        let mut all_types = Vec::new();

        for msg in messages {
            if msg.role == savant_core::types::ChatRole::User {
                // Fast-path boolean check — skip expensive regex scan if no PII patterns
                if !savant_security::pii::contains_pii(&msg.content) {
                    continue;
                }
                let result = self.detector.scan(&msg.content);
                if result.sensitivity_score > max_score {
                    max_score = result.sensitivity_score;
                }
                for pii_type in result.pii_types_found {
                    if !all_types.contains(&pii_type) {
                        all_types.push(pii_type);
                    }
                }
            }
        }

        let decision = if max_score >= self.config.sensitivity_threshold {
            RoutingDecision::Local {
                model: self.local_model().to_string(),
                reason: format!(
                    "PII detected (score={:.2}, types={:?}) — routing to local model",
                    max_score, all_types
                ),
                score: max_score,
            }
        } else if max_score >= 0.3 {
            RoutingDecision::UserChoice {
                local: self.local_model().to_string(),
                cloud: self.cloud_model().to_string(),
                reason: format!(
                    "Moderate sensitivity (score={:.2}, types={:?}) — user should decide",
                    max_score, all_types
                ),
                score: max_score,
            }
        } else {
            RoutingDecision::Cloud {
                model: self.cloud_model().to_string(),
                reason: format!("Clean content (score={:.2}) — routing to cloud", max_score),
                score: max_score,
            }
        };

        if self.config.log_decisions {
            match &decision {
                RoutingDecision::Local { reason, .. } => {
                    tracing::info!(target: "savant::privacy", decision = "local", reason = %reason, score = max_score, "privacy routing decision");
                }
                RoutingDecision::Cloud { reason, .. } => {
                    tracing::info!(target: "savant::privacy", decision = "cloud", reason = %reason, score = max_score, "privacy routing decision");
                }
                RoutingDecision::UserChoice { reason, .. } => {
                    tracing::info!(target: "savant::privacy", decision = "user_choice", reason = %reason, score = max_score, "privacy routing decision");
                }
            }
        }

        decision
    }

    /// Returns the configuration.
    pub fn config(&self) -> &PrivacyConfig {
        &self.config
    }

    fn local_model(&self) -> &str {
        self.config
            .local_models
            .first()
            .map(|s| s.as_str())
            .unwrap_or("gemma4")
    }

    fn cloud_model(&self) -> &str {
        self.config
            .cloud_models
            .first()
            .map(|s| s.as_str())
            .unwrap_or("openrouter/free")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use savant_core::types::ChatRole;

    fn user_msg(content: &str) -> ChatMessage {
        ChatMessage {
            role: ChatRole::User,
            content: content.to_string(),
            sender: Some("USER".to_string()),
            recipient: None,
            agent_id: None,
            session_id: None,
            channel: savant_core::types::AgentOutputChannel::Chat,
            is_telemetry: false,
            images: Vec::new(),
            ..Default::default()
        }
    }

    fn enabled_config() -> PrivacyConfig {
        PrivacyConfig {
            enabled: true,
            sensitivity_threshold: 0.7,
            local_models: vec!["gemma4".to_string()],
            cloud_models: vec!["openrouter/claude-sonnet-4".to_string()],
            log_decisions: false,
        }
    }

    #[test]
    fn test_clean_text_routes_cloud() {
        let router = PrivacyRouter::new(enabled_config());
        let messages = vec![user_msg("What is Rust?")];
        let decision = router.route(&messages);
        assert!(matches!(decision, RoutingDecision::Cloud { .. }));
    }

    #[test]
    fn test_ssn_routes_local() {
        let router = PrivacyRouter::new(enabled_config());
        let messages = vec![user_msg("My SSN is 123-45-6789")];
        let decision = router.route(&messages);
        assert!(matches!(decision, RoutingDecision::Local { .. }));
    }

    #[test]
    fn test_email_routes_user_choice() {
        let router = PrivacyRouter::new(enabled_config());
        let messages = vec![user_msg("Email me at alice@example.com")];
        let decision = router.route(&messages);
        // Email weight is 0.5, below 0.7 threshold but above 0.3
        assert!(matches!(decision, RoutingDecision::UserChoice { .. }));
    }

    #[test]
    fn test_disabled_config_routes_cloud() {
        let config = PrivacyConfig {
            enabled: false,
            ..enabled_config()
        };
        let router = PrivacyRouter::new(config);
        let messages = vec![user_msg("SSN: 123-45-6789")];
        let decision = router.route(&messages);
        assert!(matches!(decision, RoutingDecision::Cloud { .. }));
    }

    #[test]
    fn test_force_local_override() {
        let router = PrivacyRouter::new(enabled_config());
        let messages = vec![user_msg("Hello")];
        let decision = router.route_with_override(&messages, Some(true));
        assert!(matches!(decision, RoutingDecision::Local { .. }));
    }

    #[test]
    fn test_force_cloud_override() {
        let router = PrivacyRouter::new(enabled_config());
        let messages = vec![user_msg("SSN: 123-45-6789")];
        let decision = router.route_with_override(&messages, Some(false));
        assert!(matches!(decision, RoutingDecision::Cloud { .. }));
    }

    #[test]
    fn test_selected_model_local() {
        let decision = RoutingDecision::Local {
            model: "gemma4".to_string(),
            reason: "test".to_string(),
            score: 0.9,
        };
        assert_eq!(decision.selected_model(), "gemma4");
    }

    #[test]
    fn test_selected_model_user_choice_prefers_local() {
        let decision = RoutingDecision::UserChoice {
            local: "gemma4".to_string(),
            cloud: "openrouter/claude-sonnet-4".to_string(),
            reason: "test".to_string(),
            score: 0.5,
        };
        assert_eq!(decision.selected_model(), "gemma4");
    }

    #[test]
    fn test_credit_card_routes_local() {
        let router = PrivacyRouter::new(enabled_config());
        let messages = vec![user_msg("Card: 4111111111111111")];
        let decision = router.route(&messages);
        assert!(matches!(decision, RoutingDecision::Local { .. }));
    }
}
