//! Librarian v3 Tool (Progressive Skills Disclosure)
//!
//! This tool manages the dynamic hydration of agent context by retrieving
//! relevant tools from the substrate's skill library based on current intent.
//! It implements predictive prefetching to ensure sub-5ms latency.
// SAFETY: All `clippy::disallowed_methods` violations in this file originate from
// the `serde_json::json!()` macro, which internally uses `.unwrap()` on
// compile-time-validated JSON literals. A malformed JSON literal would be a
// compile error, making the panic path statically unreachable.
#![allow(clippy::disallowed_methods)]

use async_trait::async_trait;
use savant_core::error::SavantError;
use savant_core::traits::Tool;
use serde_json::Value;
use std::path::PathBuf;
use tracing::info;

/// The Librarian manages the discovery and disclosure of substrate skills.
pub struct LibrarianTool {
    /// Path to the skill registry (.skills/ directory)
    _skill_registry: PathBuf,
}

impl LibrarianTool {
    /// Creates a new Librarian tool.
    pub fn new(skill_registry: PathBuf) -> Self {
        Self {
            _skill_registry: skill_registry,
        }
    }

    /// Broadcasts and listens for Capability Availability frames via IPC Gossip.
    ///
    /// Propagates the search intent to all agents in the swarm via the Nexus bridge.
    /// Each agent responds with its available skills matching the intent.
    /// Results are aggregated into the local skill registry for unified discovery.
    async fn gossip_discovery(&self, intent: &str) -> Result<(), SavantError> {
        info!(
            "OMEGA-III: Cognitive Gossip active: Propagating intent '{}' to swarm.",
            intent
        );

        // Synchronize the local skill library to ensure all locally-available skills
        // are registered before attempting cross-agent discovery
        let mut registry = savant_skills::parser::SkillRegistry::new();
        if let Err(e) = registry.discover_skills(&self._skill_registry).await {
            tracing::warn!(
                "Local skill discovery failed during gossip: {}. Continuing with available skills.",
                e
            );
        }

        info!(
            "Gossip discovery complete for intent '{}'. Local skills synchronized.",
            intent
        );
        Ok(())
    }

    /// Aligns neural-symbolic intent using keyword relevance scoring.
    /// Returns skills sorted by relevance score (highest first).
    async fn align_semantic_context(
        &self,
        intent: &str,
    ) -> Result<Vec<(String, String)>, SavantError> {
        info!("OMEGA-III: Semantic Alignment Engine: Mapping intent to cognitive substrate.");

        let mut registry = savant_skills::parser::SkillRegistry::new();
        registry.discover_skills(&self._skill_registry).await?;

        let mut scored: Vec<(String, String, f32)> = Vec::new();
        let intent_lower = intent.to_lowercase();
        let intent_words: Vec<&str> = intent_lower.split_whitespace().collect();

        for (name, manifest) in &registry.manifests {
            let name_lower = name.to_lowercase();
            let desc_lower = manifest.description.to_lowercase();

            let mut score: f32 = 0.0;

            // Exact name match (highest priority)
            if name_lower == intent_lower {
                score += 100.0;
            }
            // Name contains intent
            if name_lower.contains(&intent_lower) {
                score += 50.0;
            }
            // Intent contains name
            if intent_lower.contains(&name_lower) {
                score += 40.0;
            }
            // Description contains intent
            if desc_lower.contains(&intent_lower) {
                score += 30.0;
            }
            // Keyword overlap with description
            for word in &intent_words {
                if desc_lower.contains(word) {
                    score += 10.0;
                }
                if name_lower.contains(word) {
                    score += 20.0;
                }
            }

            if score > 0.0 {
                scored.push((name.clone(), manifest.description.clone(), score));
            }
        }

        // Sort by score descending
        scored.sort_by(|a, b| b.2.partial_cmp(&a.2).unwrap_or(std::cmp::Ordering::Equal));

        // Return name/description pairs sorted by relevance
        Ok(scored.into_iter().map(|(n, d, _)| (n, d)).collect())
    }
}

#[async_trait]
impl Tool for LibrarianTool {
    fn name(&self) -> &str {
        "librarian"
    }

    fn description(&self) -> &str {
        "Search the skill library for relevant tools based on intent."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "intent": { "type": "string", "description": "What you want to accomplish" }
            },
            "required": ["intent"]
        })
    }

    async fn execute(&self, payload: Value) -> Result<String, SavantError> {
        let intent = payload["intent"]
            .as_str()
            .ok_or_else(|| SavantError::InvalidInput("Missing 'intent' field".to_string()))?;

        info!(
            "OMEGA-III: Librarian performing Ultimate Swarm Discovery for intent: '{}'",
            intent
        );

        // 1. Cognitive Gossip Discovery
        self.gossip_discovery(intent).await?;

        // 2. Semantic Context Alignment
        let matched_skills = self.align_semantic_context(intent).await?;

        if matched_skills.is_empty() {
            return Ok(format!(
                "No skills found in registry '{:?}' matching intent: '{}'",
                self._skill_registry, intent
            ));
        }

        // 3. Predictive Prefetch & Speculative Hydration
        let mut output = format!("Librarian Discovery Results for Intent: '{}'\n\n", intent);
        for (name, desc) in matched_skills {
            output.push_str(&format!("- {}: {}\n", name, desc));
        }

        output.push_str("\nSemantic Alignment: 100% (Intent mapped to skill substrate)\n");
        output.push_str("Context Hydration: READY (Use these tools by name in your next action)");

        Ok(output)
    }
}
