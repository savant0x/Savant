//! Taint Tracing — Tracks data provenance through the system.
//!
//! All external data ingestion is tagged with taint metadata.
//! During memory consolidation and dreaming, taint provenance is traced.
//! Heavily tainted memories require human-in-the-loop verification.

use serde::{Deserialize, Serialize};

/// Taint tag for tracking data provenance.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaintTag {
    /// Source of the data.
    pub source: String,
    /// Timestamp of ingestion.
    pub timestamp: i64,
    /// Trust level (0.0 = untrusted, 1.0 = fully trusted).
    pub trust_level: f32,
    /// Chain of transformations applied to this data.
    pub provenance_chain: Vec<String>,
}

impl TaintTag {
    /// Creates a new taint tag from a source with the given trust level.
    pub fn new(source: &str, trust_level: f32) -> Self {
        Self {
            source: source.to_string(),
            timestamp: chrono::Utc::now().timestamp(),
            trust_level,
            provenance_chain: vec![source.to_string()],
        }
    }

    /// External web data — low trust.
    pub fn external_web() -> Self {
        Self::new("external_web", 0.2)
    }

    /// User-provided file — medium trust.
    pub fn user_file() -> Self {
        Self::new("user_file", 0.5)
    }

    /// System-generated — full trust.
    pub fn system() -> Self {
        Self::new("system", 1.0)
    }

    /// NREM replay output — medium-high trust (grounded in real memories).
    pub fn nrem_replay() -> Self {
        Self {
            source: "nrem_replay".to_string(),
            timestamp: chrono::Utc::now().timestamp(),
            trust_level: 0.7,
            provenance_chain: vec![
                "memory_replay".to_string(),
                "nrem_consolidation".to_string(),
            ],
        }
    }

    /// Dream engine output — medium trust (speculative exploration).
    pub fn dream() -> Self {
        Self {
            source: "dream".to_string(),
            timestamp: chrono::Utc::now().timestamp(),
            trust_level: 0.5,
            provenance_chain: vec![
                "latent_exploration".to_string(),
                "cross_domain_recombination".to_string(),
            ],
        }
    }

    /// Adds a transformation step to the provenance chain.
    pub fn add_transformation(&mut self, step: &str) {
        self.provenance_chain.push(step.to_string());
    }

    /// Compounds two taint tags (used during memory consolidation).
    /// Trust level becomes the minimum of the two sources.
    pub fn compound(&self, other: &TaintTag) -> Self {
        Self {
            source: format!("{}+{}", self.source, other.source),
            timestamp: chrono::Utc::now().timestamp(),
            trust_level: self.trust_level.min(other.trust_level),
            provenance_chain: {
                let mut chain = self.provenance_chain.clone();
                chain.extend(other.provenance_chain.clone());
                chain
            },
        }
    }

    /// Returns true if this data requires human-in-the-loop verification.
    pub fn requires_human_verification(&self) -> bool {
        self.trust_level < 0.3
    }
}

/// Tracks taint tags for data flowing through the system.
///
/// Use this to tag external data on ingestion and check trust levels
/// before allowing data to reach sensitive tools.
pub struct TaintTracker {
    /// Active taint tags indexed by data identifier.
    tags: std::sync::RwLock<std::collections::HashMap<String, TaintTag>>,
}

impl TaintTracker {
    /// Creates a new taint tracker.
    pub fn new() -> Self {
        Self {
            tags: std::sync::RwLock::new(std::collections::HashMap::new()),
        }
    }

    /// Tags data with a taint tag.
    pub fn tag(&self, data_id: &str, tag: TaintTag) {
        let mut tags = self.tags.write().unwrap_or_else(|e| e.into_inner());
        tags.insert(data_id.to_string(), tag);
    }

    /// Gets the taint tag for data, if any.
    pub fn get_tag(&self, data_id: &str) -> Option<TaintTag> {
        let tags = self.tags.read().unwrap_or_else(|e| e.into_inner());
        tags.get(data_id).cloned()
    }

    /// Checks if data is trusted (trust_level >= threshold).
    pub fn is_trusted(&self, data_id: &str, threshold: f32) -> bool {
        let tags = self.tags.read().unwrap_or_else(|e| e.into_inner());
        tags.get(data_id)
            .map(|tag| tag.trust_level >= threshold)
            .unwrap_or(true) // Untagged data is considered trusted
    }

    /// Checks if data requires human verification.
    pub fn requires_verification(&self, data_id: &str) -> bool {
        let tags = self.tags.read().unwrap_or_else(|e| e.into_inner());
        tags.get(data_id)
            .map(|tag| tag.requires_human_verification())
            .unwrap_or(false)
    }

    /// Returns the number of tracked data items.
    pub fn count(&self) -> usize {
        let tags = self.tags.read().unwrap_or_else(|e| e.into_inner());
        tags.len()
    }

    /// Removes taint tag for data (e.g., after verification).
    pub fn clear(&self, data_id: &str) {
        let mut tags = self.tags.write().unwrap_or_else(|e| e.into_inner());
        tags.remove(data_id);
    }
}

impl Default for TaintTracker {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_external_web_low_trust() {
        let tag = TaintTag::external_web();
        assert!(tag.trust_level < 0.3);
        assert!(tag.requires_human_verification());
    }

    #[test]
    fn test_system_full_trust() {
        let tag = TaintTag::system();
        assert_eq!(tag.trust_level, 1.0);
        assert!(!tag.requires_human_verification());
    }

    #[test]
    fn test_compound_takes_minimum() {
        let a = TaintTag::external_web();
        let b = TaintTag::system();
        let compounded = a.compound(&b);
        assert_eq!(compounded.trust_level, 0.2);
    }

    #[test]
    fn test_provenance_chain_grows() {
        let mut tag = TaintTag::external_web();
        tag.add_transformation("distillation");
        assert_eq!(tag.provenance_chain.len(), 2);
    }

    #[test]
    fn test_nrem_replay_trust() {
        let tag = TaintTag::nrem_replay();
        assert_eq!(tag.trust_level, 0.7);
        assert!(!tag.requires_human_verification());
    }

    #[test]
    fn test_dream_trust() {
        let tag = TaintTag::dream();
        assert_eq!(tag.trust_level, 0.5);
        assert!(!tag.requires_human_verification());
    }
}
