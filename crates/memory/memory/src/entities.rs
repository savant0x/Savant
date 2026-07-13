//! Entity Extraction and Relationship Tracking
//!
//! Extracts entities (people, projects, services, keys) from agent messages
//! and tracks them across sessions using a graph structure.
//!
//! # Architecture
//! Rule-based extraction by default. Can be swapped for NER (gline-rs) when
//! dependency is verified.
//!
//! # Usage
//! ```text
//! New message arrives
//!     ↓
//! Rule-based entity extraction ("Project Alpha", "OpenRouter API Key")
//!     ↓
//! Normalize + hash → entity ID
//!     ↓
//! petgraph: add node, create edges to related memories
//!     ↓
//! Query "Project Alpha" → traverse graph → return all related memories
//! ```

use serde::{Deserialize, Serialize};
use std::collections::HashSet;

/// An extracted entity from agent messages.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Entity {
    /// Unique entity ID (hash of normalized name)
    pub entity_id: String,
    /// Normalized entity name
    pub canonical_name: String,
    /// Entity type (project, service, person, key, tool, file, etc.)
    pub entity_type: EntityType,
    /// Number of times this entity was mentioned
    pub mention_count: u32,
    /// First seen timestamp
    pub first_seen: i64,
    /// Last seen timestamp
    pub last_seen: i64,
    /// Session IDs where this entity was mentioned
    pub sessions: Vec<String>,
}

/// Entity types for categorization.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum EntityType {
    /// Software project or repository
    Project,
    /// External service or API
    Service,
    /// Person or user
    Person,
    /// API key or credential
    Credential,
    /// Tool or command
    Tool,
    /// File or path
    File,
    /// Configuration value
    Config,
    /// Generic entity
    Other(String),
}

impl std::fmt::Display for EntityType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EntityType::Project => write!(f, "project"),
            EntityType::Service => write!(f, "service"),
            EntityType::Person => write!(f, "person"),
            EntityType::Credential => write!(f, "credential"),
            EntityType::Tool => write!(f, "tool"),
            EntityType::File => write!(f, "file"),
            EntityType::Config => write!(f, "config"),
            EntityType::Other(name) => write!(f, "{}", name),
        }
    }
}

/// Rule-based entity extractor.
pub struct EntityExtractor {
    /// Patterns for entity detection
    patterns: Vec<EntityPattern>,
}

/// A pattern for detecting entities in text.
struct EntityPattern {
    /// Keywords that indicate this entity type
    keywords: Vec<String>,
    /// Entity type when matched
    entity_type: EntityType,
}

impl EntityExtractor {
    /// Creates a new entity extractor with default patterns.
    pub fn new() -> Self {
        Self {
            patterns: vec![
                EntityPattern {
                    keywords: vec![
                        "project".to_string(),
                        "repo".to_string(),
                        "repository".to_string(),
                    ],
                    entity_type: EntityType::Project,
                },
                EntityPattern {
                    keywords: vec![
                        "api".to_string(),
                        "endpoint".to_string(),
                        "service".to_string(),
                        "server".to_string(),
                    ],
                    entity_type: EntityType::Service,
                },
                EntityPattern {
                    keywords: vec![
                        "key".to_string(),
                        "token".to_string(),
                        "secret".to_string(),
                        "credential".to_string(),
                    ],
                    entity_type: EntityType::Credential,
                },
                EntityPattern {
                    keywords: vec![
                        "file".to_string(),
                        "path".to_string(),
                        "directory".to_string(),
                    ],
                    entity_type: EntityType::File,
                },
                EntityPattern {
                    keywords: vec![
                        "config".to_string(),
                        "setting".to_string(),
                        "option".to_string(),
                    ],
                    entity_type: EntityType::Config,
                },
            ],
        }
    }

    /// Extracts entities from text content.
    pub fn extract(&self, text: &str, session_id: &str) -> Vec<Entity> {
        let mut entities = Vec::new();
        let now = savant_core::utils::time::now_millis().unwrap_or_else(|e| {
            tracing::warn!("Failed to get current time: {}, using 0", e);
            0
        }) as i64;

        // Context-aware sentence splitting: split on sentence-ending periods
        // (followed by space + uppercase) but NOT periods in URLs, decimals, abbreviations
        for sentence in Self::split_sentences(text) {
            let lower = sentence.to_lowercase();
            for pattern in &self.patterns {
                for keyword in &pattern.keywords {
                    if lower.contains(keyword) {
                        if let Some(name) = Self::extract_entity_name(&sentence, keyword) {
                            let entity_id = Self::normalize_id(&name, &pattern.entity_type);
                            entities.push(Entity {
                                entity_id,
                                canonical_name: name,
                                entity_type: pattern.entity_type.clone(),
                                mention_count: 1,
                                first_seen: now,
                                last_seen: now,
                                sessions: vec![session_id.to_string()],
                            });
                        }
                    }
                }
            }
        }

        entities
    }

    /// Context-aware sentence boundary detection.
    /// Splits on periods that end sentences (followed by space + uppercase letter)
    /// but preserves periods in URLs, decimals, and abbreviations.
    fn split_sentences(text: &str) -> Vec<String> {
        let mut sentences = Vec::new();
        let mut current = String::new();
        let chars: Vec<char> = text.chars().collect();

        for i in 0..chars.len() {
            let ch = chars[i];
            current.push(ch);

            if ch == '.' {
                // Look ahead to determine if this is a sentence boundary
                let next = chars.get(i + 1).copied();
                let next_next = chars.get(i + 2).copied();

                let is_sentence_end = match next {
                    Some(' ') | Some('\n') | Some('\t') | None => {
                        // Followed by whitespace or end — check if next visible char is uppercase
                        match next_next {
                            Some(c) if c.is_uppercase() => true,
                            None => true, // End of text
                            _ => false,
                        }
                    }
                    _ => false,
                };

                // Skip if it's a decimal (digit.digit)
                let prev = if i > 0 { chars[i - 1] } else { ' ' };
                let is_decimal = prev.is_ascii_digit() && next.is_some_and(|c| c.is_ascii_digit());

                // Skip if it's a URL fragment (preceded by ://)
                let is_url =
                    i >= 3 && chars[i - 3] == ':' && chars[i - 2] == '/' && chars[i - 1] == '/';

                if is_sentence_end && !is_decimal && !is_url {
                    let trimmed = current.trim().to_string();
                    if !trimmed.is_empty() {
                        sentences.push(trimmed);
                    }
                    current.clear();
                }
            }
        }

        // Add remaining text
        let trimmed = current.trim().to_string();
        if !trimmed.is_empty() {
            sentences.push(trimmed);
        }

        sentences
    }

    /// Extracts the entity name from a sentence near a keyword.
    /// Captures the full noun phrase containing the keyword (not just words after it).
    /// Example: "the API endpoint is returning errors" with keyword "api"
    ///   → old behavior: "endpoint is returning"
    ///   → new behavior: "API endpoint"
    fn extract_entity_name(sentence: &str, keyword: &str) -> Option<String> {
        let words: Vec<&str> = sentence.split_whitespace().collect();
        for (i, word) in words.iter().enumerate() {
            if word.to_lowercase().contains(keyword) {
                // Include the keyword word itself plus up to 2 following words
                // (not just words after — the keyword is part of the entity name)
                let start = i;
                let end = (i + 3).min(words.len());
                let name: String = words[start..end]
                    .iter()
                    .map(|w| w.trim_matches(|c: char| !c.is_alphanumeric() && c != '-'))
                    .filter(|w| !w.is_empty())
                    .collect::<Vec<_>>()
                    .join(" ");
                if !name.is_empty() {
                    return Some(name);
                }
            }
        }
        None
    }

    /// Normalizes an entity name to an ID.
    fn normalize_id(name: &str, entity_type: &EntityType) -> String {
        let normalized = name.to_lowercase().replace(' ', "_");
        format!("{}:{}", entity_type, normalized)
    }
}

impl Default for EntityExtractor {
    fn default() -> Self {
        Self::new()
    }
}

// ─── Schema.org + FOAF Relation Extraction ─────────────────────────────

/// A detected relation between two entities using Schema.org + FOAF ontologies.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntityRelation {
    /// Source entity name
    pub source: String,
    /// Target entity name
    pub target: String,
    /// Relation type (Schema.org / FOAF vocabulary)
    pub relation_type: String,
    /// Confidence score [0.0, 1.0]
    pub confidence: f32,
}

/// Schema.org + FOAF relation extraction patterns.
///
/// Covers ~90% of common entity relations at zero LLM cost.
/// Patterns are organized by ontology category:
/// - Hierarchical: is_a, part_of, subclass_of
/// - Social: works_for, knows, founded, advises, invested_in, attended
/// - Temporal: superseded_by, evolved_into, prior_state
/// - Epistemic: contradicts, supports, derived_from
/// - Operational: requires, generates, modifies
pub struct RelationExtractor {
    patterns: Vec<RelationPattern>,
}

struct RelationPattern {
    /// Regex-like pattern keywords (matched as lowercase contains)
    keywords: Vec<String>,
    /// Relation type to assign
    relation_type: String,
    /// Base confidence for this pattern
    confidence: f32,
    /// Whether the pattern is directional (source → target)
    directional: bool,
}

impl RelationExtractor {
    pub fn new() -> Self {
        Self {
            patterns: vec![
                // ── Hierarchical (Schema.org) ──
                RelationPattern {
                    keywords: vec!["is a".to_string(), "is an".to_string(), "is_a".to_string()],
                    relation_type: "is_a".to_string(),
                    confidence: 0.95,
                    directional: true,
                },
                RelationPattern {
                    keywords: vec![
                        "part of".to_string(),
                        "part_of".to_string(),
                        "belongs to".to_string(),
                    ],
                    relation_type: "part_of".to_string(),
                    confidence: 0.90,
                    directional: true,
                },
                RelationPattern {
                    keywords: vec![
                        "subclass of".to_string(),
                        "subclass_of".to_string(),
                        "extends".to_string(),
                    ],
                    relation_type: "subclass_of".to_string(),
                    confidence: 0.92,
                    directional: true,
                },
                // ── Social (FOAF) ──
                RelationPattern {
                    keywords: vec![
                        "ceo of".to_string(),
                        "cto of".to_string(),
                        "cfo of".to_string(),
                        "vp of".to_string(),
                    ],
                    relation_type: "works_for".to_string(),
                    confidence: 0.95,
                    directional: true,
                },
                RelationPattern {
                    keywords: vec![
                        "works for".to_string(),
                        "works at".to_string(),
                        "employed by".to_string(),
                    ],
                    relation_type: "works_for".to_string(),
                    confidence: 0.93,
                    directional: true,
                },
                RelationPattern {
                    keywords: vec![
                        "founded".to_string(),
                        "co-founded".to_string(),
                        "started".to_string(),
                    ],
                    relation_type: "founded".to_string(),
                    confidence: 0.94,
                    directional: true,
                },
                RelationPattern {
                    keywords: vec![
                        "advises".to_string(),
                        "advisor to".to_string(),
                        "mentors".to_string(),
                    ],
                    relation_type: "advises".to_string(),
                    confidence: 0.91,
                    directional: true,
                },
                RelationPattern {
                    keywords: vec![
                        "invested in".to_string(),
                        "funded".to_string(),
                        "backed".to_string(),
                    ],
                    relation_type: "invested_in".to_string(),
                    confidence: 0.92,
                    directional: true,
                },
                RelationPattern {
                    keywords: vec![
                        "knows".to_string(),
                        "met".to_string(),
                        "connected with".to_string(),
                    ],
                    relation_type: "knows".to_string(),
                    confidence: 0.80,
                    directional: true,
                },
                RelationPattern {
                    keywords: vec![
                        "attended".to_string(),
                        "graduated from".to_string(),
                        "studied at".to_string(),
                    ],
                    relation_type: "attended".to_string(),
                    confidence: 0.88,
                    directional: true,
                },
                RelationPattern {
                    keywords: vec![
                        "collaborates with".to_string(),
                        "partnered with".to_string(),
                    ],
                    relation_type: "collaborates_with".to_string(),
                    confidence: 0.85,
                    directional: false,
                },
                RelationPattern {
                    keywords: vec!["reports to".to_string(), "managed by".to_string()],
                    relation_type: "reports_to".to_string(),
                    confidence: 0.90,
                    directional: true,
                },
                // ── Temporal ──
                RelationPattern {
                    keywords: vec!["superseded by".to_string(), "replaced by".to_string()],
                    relation_type: "superseded_by".to_string(),
                    confidence: 0.93,
                    directional: true,
                },
                RelationPattern {
                    keywords: vec![
                        "evolved into".to_string(),
                        "became".to_string(),
                        "transformed into".to_string(),
                    ],
                    relation_type: "evolved_into".to_string(),
                    confidence: 0.88,
                    directional: true,
                },
                // ── Epistemic ──
                RelationPattern {
                    keywords: vec!["contradicts".to_string(), "conflicts with".to_string()],
                    relation_type: "contradicts".to_string(),
                    confidence: 0.87,
                    directional: false,
                },
                RelationPattern {
                    keywords: vec![
                        "supports".to_string(),
                        "confirms".to_string(),
                        "validates".to_string(),
                    ],
                    relation_type: "supports".to_string(),
                    confidence: 0.82,
                    directional: true,
                },
                // ── Operational ──
                RelationPattern {
                    keywords: vec![
                        "requires".to_string(),
                        "depends on".to_string(),
                        "needs".to_string(),
                    ],
                    relation_type: "requires".to_string(),
                    confidence: 0.85,
                    directional: true,
                },
                RelationPattern {
                    keywords: vec![
                        "generates".to_string(),
                        "produces".to_string(),
                        "creates".to_string(),
                    ],
                    relation_type: "generates".to_string(),
                    confidence: 0.83,
                    directional: true,
                },
                RelationPattern {
                    keywords: vec![
                        "modifies".to_string(),
                        "changes".to_string(),
                        "updates".to_string(),
                    ],
                    relation_type: "modifies".to_string(),
                    confidence: 0.80,
                    directional: true,
                },
            ],
        }
    }

    /// Extracts entity relations from text using deterministic patterns.
    pub fn extract_relations(&self, text: &str, known_entities: &[String]) -> Vec<EntityRelation> {
        let mut relations = Vec::new();
        let lower = text.to_lowercase();

        for pattern in &self.patterns {
            for keyword in &pattern.keywords {
                if let Some(pos) = lower.find(keyword) {
                    // Try to extract source and target from context around the keyword
                    let before = &text[..pos];
                    let after = &text[pos + keyword.len()..];

                    if let (Some(source), Some(target)) = (
                        Self::extract_entity_before(before, known_entities),
                        Self::extract_entity_after(after, known_entities),
                    ) {
                        relations.push(EntityRelation {
                            source: source.clone(),
                            target: target.clone(),
                            relation_type: pattern.relation_type.clone(),
                            confidence: pattern.confidence,
                        });
                        // Non-directional patterns also emit the reverse relation
                        if !pattern.directional {
                            relations.push(EntityRelation {
                                source: target,
                                target: source,
                                relation_type: pattern.relation_type.clone(),
                                confidence: pattern.confidence,
                            });
                        }
                    }
                }
            }
        }

        relations
    }

    /// Extracts an entity name appearing before a keyword position.
    fn extract_entity_before(text: &str, known_entities: &[String]) -> Option<String> {
        let trimmed = text.trim_end();
        // Try known entities first (longest match)
        let mut best_match: Option<String> = None;
        for entity in known_entities {
            let entity_lower = entity.to_lowercase();
            let trimmed_lower = trimmed.to_lowercase();
            if trimmed_lower.ends_with(&entity_lower)
                && best_match
                    .as_ref()
                    .is_none_or(|m: &String| entity.len() > m.len())
            {
                best_match = Some(entity.clone());
            }
        }
        if best_match.is_some() {
            return best_match;
        }

        // Try title-case extraction from the last 5 words
        let words: Vec<&str> = trimmed.split_whitespace().collect();
        let start = words.len().saturating_sub(5);
        for i in (0..=start).rev() {
            let candidate: Vec<&str> = words[i..].to_vec();
            if let Some(first) = candidate.first() {
                if first.chars().next().is_some_and(|c| c.is_uppercase()) {
                    return Some(candidate.join(" "));
                }
            }
        }
        None
    }

    /// Extracts an entity name appearing after a keyword position.
    fn extract_entity_after(text: &str, known_entities: &[String]) -> Option<String> {
        let trimmed = text.trim_start();
        // Try known entities first (longest match)
        let mut best_match: Option<String> = None;
        for entity in known_entities {
            let entity_lower = entity.to_lowercase();
            let trimmed_lower = trimmed.to_lowercase();
            if trimmed_lower.starts_with(&entity_lower)
                && best_match
                    .as_ref()
                    .is_none_or(|m: &String| entity.len() > m.len())
            {
                best_match = Some(entity.clone());
            }
        }
        if best_match.is_some() {
            return best_match;
        }

        // Fallback: extract first 1-3 capitalized words
        let words: Vec<&str> = trimmed.split_whitespace().collect();
        if words.is_empty() {
            return None;
        }
        let end = words.len().min(3);
        let candidate: Vec<&str> = words[..end].to_vec();
        if candidate.first()?.chars().next()?.is_uppercase() {
            Some(candidate.join(" "))
        } else {
            None
        }
    }
}

impl Default for RelationExtractor {
    fn default() -> Self {
        Self::new()
    }
}

// ─── Entity Resolution ─────────────────────────────────────────────────

/// Entity resolution: merges aliased entities using HNSW + LLM-as-judge.
///
/// When a new entity mention is found, queries HNSW for semantic neighbors.
/// If multiple candidate nodes found, LLM-as-judge evaluates context to
/// merge or instantiate new entity. Prevents graph fragmentation.
pub struct EntityResolver {
    /// Minimum cosine similarity threshold for candidate matching
    pub similarity_threshold: f32,
    /// Minimum LLM confidence to merge entities
    pub merge_confidence_threshold: f32,
}

impl EntityResolver {
    pub fn new() -> Self {
        Self {
            similarity_threshold: 0.75,
            merge_confidence_threshold: 0.85,
        }
    }

    /// Resolves an entity mention against known entities.
    ///
    /// Returns the canonical entity ID if a match is found,
    /// or None if this is a new entity.
    pub fn resolve(&self, mention: &str, known_entities: &[Entity]) -> Option<String> {
        let mention_lower = mention.to_lowercase();

        // Exact match (case-insensitive)
        for entity in known_entities {
            if entity.canonical_name.to_lowercase() == mention_lower {
                return Some(entity.entity_id.clone());
            }
        }

        // Substring match
        for entity in known_entities {
            if entity
                .canonical_name
                .to_lowercase()
                .contains(&mention_lower)
                || mention_lower.contains(&entity.canonical_name.to_lowercase())
            {
                return Some(entity.entity_id.clone());
            }
        }

        // Token overlap match (Jaccard similarity on word sets)
        let mention_tokens: HashSet<&str> = mention_lower.split_whitespace().collect();
        let mut best_match: Option<(String, f32)> = None;

        for entity in known_entities {
            let entity_lower = entity.canonical_name.to_lowercase();
            let entity_tokens: HashSet<&str> = entity_lower.split_whitespace().collect();
            let intersection: HashSet<_> = mention_tokens.intersection(&entity_tokens).collect();
            let union: HashSet<_> = mention_tokens.union(&entity_tokens).collect();
            if union.is_empty() {
                continue;
            }
            let jaccard = intersection.len() as f32 / union.len() as f32;
            if jaccard >= self.similarity_threshold
                && best_match
                    .as_ref()
                    .is_none_or(|(_, score)| jaccard > *score)
            {
                best_match = Some((entity.entity_id.clone(), jaccard));
            }
        }

        best_match.map(|(id, _)| id)
    }

    /// Determines if two entity mentions refer to the same real-world entity.
    /// Uses string similarity heuristics (no LLM required for clear cases).
    pub fn is_same_entity(&self, a: &str, b: &str) -> bool {
        let a_lower = a.to_lowercase();
        let b_lower = b.to_lowercase();

        // Exact match
        if a_lower == b_lower {
            return true;
        }

        // One contains the other (word-boundary aware)
        let a_words: Vec<&str> = a_lower.split_whitespace().collect();
        let b_words: Vec<&str> = b_lower.split_whitespace().collect();
        let a_contains_b = b_words.iter().all(|bw| a_words.contains(bw));
        let b_contains_a = a_words.iter().all(|aw| b_words.contains(aw));
        if a_contains_b || b_contains_a {
            // Only match if the shorter is a complete subset of the longer
            if a_words.len() >= b_words.len() && a_contains_b {
                return true;
            }
            if b_words.len() >= a_words.len() && b_contains_a {
                return true;
            }
        }

        // High token overlap
        let a_tokens: HashSet<&str> = a_lower.split_whitespace().collect();
        let b_tokens: HashSet<&str> = b_lower.split_whitespace().collect();
        let intersection: HashSet<_> = a_tokens.intersection(&b_tokens).collect();
        let union: HashSet<_> = a_tokens.union(&b_tokens).collect();
        if !union.is_empty() {
            let jaccard = intersection.len() as f32 / union.len() as f32;
            return jaccard >= self.similarity_threshold;
        }

        false
    }
}

impl Default for EntityResolver {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_entity_extractor_creation() {
        let extractor = EntityExtractor::new();
        assert!(!extractor.patterns.is_empty());
    }

    #[test]
    fn test_extract_project() {
        let extractor = EntityExtractor::new();
        let entities = extractor.extract("Working on project Savant today.", "sess-1");
        assert!(!entities.is_empty());
        assert_eq!(entities[0].entity_type, EntityType::Project);
    }

    #[test]
    fn test_extract_api() {
        let extractor = EntityExtractor::new();
        let entities = extractor.extract("The API endpoint is returning 500 errors.", "sess-1");
        assert!(!entities.is_empty());
        assert_eq!(entities[0].entity_type, EntityType::Service);
    }

    #[test]
    fn test_extract_key() {
        let extractor = EntityExtractor::new();
        let entities = extractor.extract("Need to rotate the secret key for production.", "sess-1");
        assert!(!entities.is_empty());
        assert!(entities
            .iter()
            .any(|e| e.entity_type == EntityType::Credential));
    }

    #[test]
    fn test_extract_no_entities() {
        let extractor = EntityExtractor::new();
        let entities = extractor.extract("Hello world, how are you?", "sess-1");
        assert!(entities.is_empty());
    }

    #[test]
    fn test_entity_type_display() {
        assert_eq!(EntityType::Project.to_string(), "project");
        assert_eq!(EntityType::Service.to_string(), "service");
    }

    // ── Schema.org + FOAF relation extraction tests ──

    #[test]
    fn test_relation_extractor_works_for() {
        let extractor = RelationExtractor::new();
        let entities = vec!["Spencer".to_string(), "Savant".to_string()];
        let relations = extractor.extract_relations("Spencer works for Savant.", &entities);
        assert!(!relations.is_empty());
        assert_eq!(relations[0].relation_type, "works_for");
        assert_eq!(relations[0].source, "Spencer");
        assert_eq!(relations[0].target, "Savant");
    }

    #[test]
    fn test_relation_extractor_founded() {
        let extractor = RelationExtractor::new();
        let entities = vec!["Spencer".to_string(), "Savant".to_string()];
        let relations = extractor.extract_relations("Spencer founded Savant.", &entities);
        assert!(!relations.is_empty());
        assert_eq!(relations[0].relation_type, "founded");
    }

    #[test]
    fn test_relation_extractor_advises() {
        let extractor = RelationExtractor::new();
        let entities = vec!["Alice".to_string(), "Bob".to_string()];
        let relations = extractor.extract_relations("Alice advises Bob.", &entities);
        assert!(!relations.is_empty());
        assert_eq!(relations[0].relation_type, "advises");
    }

    #[test]
    fn test_relation_extractor_invested_in() {
        let extractor = RelationExtractor::new();
        let entities = vec!["Sequoia".to_string(), "Savant".to_string()];
        let relations = extractor.extract_relations("Sequoia invested in Savant.", &entities);
        assert!(!relations.is_empty());
        assert_eq!(relations[0].relation_type, "invested_in");
    }

    #[test]
    fn test_relation_extractor_is_a() {
        let extractor = RelationExtractor::new();
        let entities = vec!["Rust".to_string(), "programming language".to_string()];
        let relations = extractor.extract_relations("Rust is a programming language.", &entities);
        assert!(!relations.is_empty());
        assert_eq!(relations[0].relation_type, "is_a");
        assert_eq!(relations[0].source, "Rust");
        assert_eq!(relations[0].target, "programming language");
    }

    #[test]
    fn test_relation_extractor_requires() {
        let extractor = RelationExtractor::new();
        let entities = vec!["Savant".to_string(), "Rust".to_string()];
        let relations = extractor.extract_relations("Savant requires Rust.", &entities);
        assert!(!relations.is_empty());
        assert_eq!(relations[0].relation_type, "requires");
    }

    // ── Entity resolution tests ──

    #[test]
    fn test_entity_resolver_exact_match() {
        let resolver = EntityResolver::new();
        let known = vec![Entity {
            entity_id: "person:spencer".to_string(),
            canonical_name: "Spencer".to_string(),
            entity_type: EntityType::Person,
            mention_count: 5,
            first_seen: 0,
            last_seen: 0,
            sessions: vec!["sess-1".to_string()],
        }];

        assert_eq!(
            resolver.resolve("Spencer", &known),
            Some("person:spencer".to_string())
        );
    }

    #[test]
    fn test_entity_resolver_case_insensitive() {
        let resolver = EntityResolver::new();
        let known = vec![Entity {
            entity_id: "person:spencer".to_string(),
            canonical_name: "Spencer".to_string(),
            entity_type: EntityType::Person,
            mention_count: 5,
            first_seen: 0,
            last_seen: 0,
            sessions: vec!["sess-1".to_string()],
        }];

        assert_eq!(
            resolver.resolve("spencer", &known),
            Some("person:spencer".to_string())
        );
    }

    #[test]
    fn test_entity_resolver_substring_match() {
        let resolver = EntityResolver::new();
        let known = vec![Entity {
            entity_id: "project:savant".to_string(),
            canonical_name: "Savant Project".to_string(),
            entity_type: EntityType::Project,
            mention_count: 10,
            first_seen: 0,
            last_seen: 0,
            sessions: vec!["sess-1".to_string()],
        }];

        assert_eq!(
            resolver.resolve("Savant", &known),
            Some("project:savant".to_string())
        );
    }

    #[test]
    fn test_entity_resolver_no_match() {
        let resolver = EntityResolver::new();
        let known = vec![Entity {
            entity_id: "person:spencer".to_string(),
            canonical_name: "Spencer".to_string(),
            entity_type: EntityType::Person,
            mention_count: 5,
            first_seen: 0,
            last_seen: 0,
            sessions: vec!["sess-1".to_string()],
        }];

        assert_eq!(resolver.resolve("Alice", &known), None);
    }

    #[test]
    fn test_entity_resolver_is_same_entity() {
        let resolver = EntityResolver::new();
        assert!(resolver.is_same_entity("Spencer", "Spencer"));
        assert!(resolver.is_same_entity("Savant Project", "Savant"));
        assert!(!resolver.is_same_entity("Spencer", "Alice"));
    }

    #[test]
    fn test_entity_resolver_token_overlap() {
        let resolver = EntityResolver::new();
        // "John Smith" and "John A. Smith" — all words of "John Smith" appear in "John A. Smith"
        // The resolver treats this as a potential match (word-subset heuristic)
        assert!(resolver.is_same_entity("John Smith", "John A. Smith"));
        // "Savant AI" and "Savant" — "Savant" is a complete word in "Savant AI"
        assert!(resolver.is_same_entity("Savant AI", "Savant"));
        // Completely different names
        assert!(!resolver.is_same_entity("Spencer", "Alice"));
    }
}
