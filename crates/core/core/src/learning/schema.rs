use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashMap;

/// Categories for emergent cognitive insights.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "PascalCase")]
pub enum LearningCategory {
    /// Insights related to following (or missing) protocol/instructions.
    Protocol,
    /// General insights or "aha" moments during execution.
    Insight,
    /// Spontaneous identification of errors or missteps.
    Error,
    /// Identity mutation proposals from evolution engine.
    Mutation,
}

impl std::fmt::Display for LearningCategory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LearningCategory::Protocol => write!(f, "Protocol"),
            LearningCategory::Insight => write!(f, "Insight"),
            LearningCategory::Error => write!(f, "Error"),
            LearningCategory::Mutation => write!(f, "Mutation"),
        }
    }
}

impl std::str::FromStr for LearningCategory {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "protocol" => Ok(LearningCategory::Protocol),
            "error" => Ok(LearningCategory::Error),
            "insight" => Ok(LearningCategory::Insight),
            "mutation" => Ok(LearningCategory::Mutation),
            _ => Err(()),
        }
    }
}

/// A structured entry for the Emergent Learning Protocol.
///
/// This captures "subconscious" reflections and formalizes them into a
/// queryable signal for swarm evolution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmergentLearning {
    /// RFC3339 formatted timestamp.
    pub timestamp: String,
    /// Stable ID of the agent who originated the insight.
    pub agent_id: String,
    /// The classification of the insight.
    pub category: LearningCategory,
    /// The human-readable insight or critique.
    pub content: String,
    /// Significance rating (1-10) for filtering and prioritization.
    pub significance: u8,
    /// Additional context (e.g., task_id, tool_name, etc.)
    #[serde(default)]
    pub metadata: HashMap<String, Value>,
    /// How much this learning contributes to identity (0.0-1.0)
    #[serde(default)]
    pub identity_weight: f32,
    /// What was promoted to identity, if anything
    #[serde(default)]
    pub promoted_to_identity: Option<String>,
    /// Tag linking learning to a SOUL.md mutation ID
    #[serde(default)]
    pub mutation_tag: Option<String>,
    /// Generation number for lineage tracking
    #[serde(default)]
    pub generation: u32,
}

impl EmergentLearning {
    pub fn new(
        agent_id: String,
        category: LearningCategory,
        content: String,
        significance: u8,
    ) -> Self {
        Self {
            timestamp: chrono::Utc::now().to_rfc3339(),
            agent_id,
            category,
            content,
            significance,
            metadata: HashMap::new(),
            identity_weight: 0.0,
            promoted_to_identity: None,
            mutation_tag: None,
            generation: 0,
        }
    }

    pub fn with_timestamp(
        agent_id: String,
        category: LearningCategory,
        content: String,
        significance: u8,
        timestamp: String,
    ) -> Self {
        Self {
            timestamp,
            agent_id,
            category,
            content,
            significance,
            metadata: HashMap::new(),
            identity_weight: 0.0,
            promoted_to_identity: None,
            mutation_tag: None,
            generation: 0,
        }
    }
}
