//! Reflective/Semantic Memory Layer
//!
//! A multi-graph store for synthesized rules, core identity constraints,
//! and generalized concepts derived from episodic memory consolidation.
//!
//! # MAGMA Multi-Graph Architecture
//! Four orthogonal graphs with intent-aware routing:
//! - **SemanticGraph**: Concept nodes, is_a/part_of/supports/contradicts edges
//! - **TemporalGraph**: Event nodes, superseded_by/evolved_into/prior_state edges
//! - **CausalGraph**: Action/outcome nodes, requires/generates/modifies edges
//! - **EntityGraph**: Person/project/service nodes, works_for/knows/founded/advises edges
//!
//! Written by the background consolidation thread, read by the workspace
//! executive monitor.

use serde::{Deserialize, Serialize};

/// A concept match with relevance score from multi-tier search.
#[derive(Debug)]
pub struct ConceptMatch<'a> {
    pub concept: &'a Concept,
    pub relevance: f32,
}

/// A concept node in the reflective memory graph.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Concept {
    /// Unique identifier.
    pub id: String,
    /// Human-readable label.
    pub label: String,
    /// Source memory entry IDs that contributed to this concept.
    pub source_entries: Vec<u64>,
    /// Concept type for decay and promotion behavior.
    #[serde(default)]
    pub concept_type: ConceptType,
    /// Creation timestamp.
    pub created_at: i64,
    /// Last access timestamp.
    pub last_accessed: i64,
}

/// Classification of concept for memory lifecycle management.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub enum ConceptType {
    /// Ephemeral observation — normal decay
    #[default]
    Episodic,
    /// Derived knowledge — slower decay
    Semantic,
    /// Core identity concept — immortal, never decays
    Identity,
}

/// A relation between two concepts.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Relation {
    /// Type of relation (e.g., "is_a", "part_of", "contradicts", "evolved_into", "superseded_by").
    pub relation_type: String,
    /// Strength of the relation [0.0, 1.0].
    pub weight: f32,
    /// Source concept ID.
    pub source_concept: String,
    /// Target concept ID.
    pub target_concept: String,
}

/// Graph namespace for MAGMA multi-graph routing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum GraphNamespace {
    /// Concept nodes with hierarchical/epistemic relations.
    Semantic,
    /// Event nodes with temporal ordering relations.
    Temporal,
    /// Action/outcome nodes with causal relations.
    Causal,
    /// Person/project/service nodes with social relations.
    Entity,
}

impl GraphNamespace {
    /// Returns the canonical relation types for this namespace.
    pub fn relation_types(&self) -> &'static [&'static str] {
        match self {
            GraphNamespace::Semantic => &[
                "is_a",
                "part_of",
                "subclass_of",
                "supports",
                "contradicts",
                "derived_from",
            ],
            GraphNamespace::Temporal => &[
                "superseded_by",
                "evolved_into",
                "prior_state",
                "follows",
                "precedes",
            ],
            GraphNamespace::Causal => &["requires", "generates", "modifies", "enables", "prevents"],
            GraphNamespace::Entity => &[
                "works_for",
                "knows",
                "founded",
                "advises",
                "invested_in",
                "attended",
                "collaborates_with",
                "reports_to",
            ],
        }
    }

    /// Checks whether a relation type belongs to this namespace.
    pub fn contains_relation(&self, relation_type: &str) -> bool {
        self.relation_types().contains(&relation_type)
    }
}

/// Intent-aware query router for MAGMA multi-graph architecture.
///
/// Classifies incoming queries by intent and routes to the appropriate
/// graph namespace, achieving 95%+ token reduction vs dense retrieval.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum QueryIntent {
    /// "What is X?", "How does X relate to Y?" → SemanticGraph
    Semantic,
    /// "When did X happen?", "What came before X?" → TemporalGraph
    Temporal,
    /// "What caused X?", "What does X require?" → CausalGraph
    Causal,
    /// "Who works for X?", "Who founded Y?" → EntityGraph
    Entity,
    /// Ambiguous — search all graphs and merge results
    Hybrid,
}

/// Routes a query to the appropriate graph namespace(s).
///
/// Uses keyword-based intent classification. For ambiguous queries,
/// returns `QueryIntent::Hybrid` which searches all graphs.
pub fn resolve_graph_intent(query: &str) -> QueryIntent {
    let lower = query.to_lowercase();

    // Temporal indicators
    if lower.contains("when")
        || lower.contains("before")
        || lower.contains("after")
        || lower.contains("timeline")
        || lower.contains("history")
        || lower.contains("evolved")
        || lower.contains("superseded")
        || lower.contains("previously")
    {
        return QueryIntent::Temporal;
    }

    // Causal indicators
    if lower.contains("why")
        || lower.contains("cause")
        || lower.contains("because")
        || lower.contains("requires")
        || lower.contains("generates")
        || lower.contains("leads to")
        || lower.contains("results in")
    {
        return QueryIntent::Causal;
    }

    // Entity indicators
    if lower.contains("who")
        || lower.contains("works for")
        || lower.contains("founded")
        || lower.contains("knows")
        || lower.contains("advises")
        || lower.contains("person")
        || lower.contains("team")
    {
        return QueryIntent::Entity;
    }

    // Semantic indicators
    if lower.contains("what is")
        || lower.contains("how does")
        || lower.contains("relate")
        || lower.contains("type of")
        || lower.contains("kind of")
        || lower.contains("similar")
    {
        return QueryIntent::Semantic;
    }

    QueryIntent::Hybrid
}

/// Converts a `QueryIntent` to the primary `GraphNamespace`.
pub fn intent_to_namespace(intent: &QueryIntent) -> Option<GraphNamespace> {
    match intent {
        QueryIntent::Semantic => Some(GraphNamespace::Semantic),
        QueryIntent::Temporal => Some(GraphNamespace::Temporal),
        QueryIntent::Causal => Some(GraphNamespace::Causal),
        QueryIntent::Entity => Some(GraphNamespace::Entity),
        QueryIntent::Hybrid => None,
    }
}

/// Maximum number of concepts per namespace graph.
const MAX_CONCEPTS_PER_NAMESPACE: usize = 10_000;
/// Maximum number of relations per namespace graph.
const MAX_RELATIONS_PER_NAMESPACE: usize = 50_000;

/// Per-namespace graph storage.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NamespaceGraph {
    pub namespace: GraphNamespace,
    pub concepts: Vec<Concept>,
    pub relations: Vec<Relation>,
}

impl NamespaceGraph {
    pub fn new(namespace: GraphNamespace) -> Self {
        Self {
            namespace,
            concepts: Vec::new(),
            relations: Vec::new(),
        }
    }

    pub fn add_concept(&mut self, concept: Concept) {
        self.concepts.retain(|c| c.id != concept.id);
        // RC-06: Evict concept with fewest source entries if at capacity
        if self.concepts.len() >= MAX_CONCEPTS_PER_NAMESPACE {
            if let Some(min_idx) = self
                .concepts
                .iter()
                .enumerate()
                .min_by_key(|(_, c)| c.source_entries.len())
                .map(|(i, _)| i)
            {
                if concept.source_entries.len() > self.concepts[min_idx].source_entries.len() {
                    self.concepts.remove(min_idx);
                } else {
                    return;
                }
            }
        }
        self.concepts.push(concept);
    }

    pub fn add_relation(&mut self, relation: Relation) {
        // RC-06: Dedup relations by source+target+type
        let already_exists = self.relations.iter().any(|r| {
            r.source_concept == relation.source_concept
                && r.target_concept == relation.target_concept
                && r.relation_type == relation.relation_type
        });
        if already_exists {
            return;
        }
        // RC-06: Evict oldest relation if at capacity
        if self.relations.len() >= MAX_RELATIONS_PER_NAMESPACE {
            self.relations.remove(0);
        }
        self.relations.push(relation);
    }

    /// Finds concepts matching the query using three-tier matching (ranked).
    /// Returns concepts with relevance scores for use in hybrid search fusion.
    pub fn find_concepts_ranked(&self, query: &str) -> Vec<ConceptMatch<'_>> {
        let query_lower = query.to_lowercase();
        let query_words: Vec<&str> = query_lower.split_whitespace().collect();
        let mut matches = Vec::new();

        for concept in &self.concepts {
            let label_lower = concept.label.to_lowercase();

            // Tier 1: Exact match
            if label_lower == query_lower {
                matches.push(ConceptMatch {
                    concept,
                    relevance: 1.0,
                });
                continue;
            }

            // Tier 2: Substring match (bidirectional)
            if label_lower.contains(&query_lower) || query_lower.contains(&label_lower) {
                matches.push(ConceptMatch {
                    concept,
                    relevance: 0.8,
                });
                continue;
            }

            // Tier 3: Word-level overlap
            if !query_words.is_empty() {
                let label_words: Vec<&str> = label_lower.split_whitespace().collect();
                let overlap = query_words
                    .iter()
                    .filter(|qw| {
                        label_words
                            .iter()
                            .any(|lw| lw.contains(*qw) || qw.contains(lw))
                    })
                    .count();
                if overlap > 0 {
                    let relevance = 0.3 + (overlap as f32 / query_words.len() as f32) * 0.4;
                    matches.push(ConceptMatch { concept, relevance });
                }
            }
        }

        matches.sort_by(|a, b| {
            b.relevance
                .partial_cmp(&a.relevance)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        matches
    }

    /// Finds concepts matching the query (simple substring, backward-compatible).
    pub fn find_concepts(&self, query: &str) -> Vec<&Concept> {
        let query_lower = query.to_lowercase();
        self.concepts
            .iter()
            .filter(|c| c.label.to_lowercase().contains(&query_lower))
            .collect()
    }

    pub fn find_relations(&self, concept_id: &str) -> Vec<&Relation> {
        self.relations
            .iter()
            .filter(|r| r.source_concept == concept_id || r.target_concept == concept_id)
            .collect()
    }

    pub fn concept_count(&self) -> usize {
        self.concepts.len()
    }

    pub fn relation_count(&self) -> usize {
        self.relations.len()
    }
}

/// Reflective memory graph storing generalized concepts and relations.
///
/// This is NOT the same as the episodic memory (LSM/vector). This layer
/// stores high-level abstractions derived during memory consolidation.
///
/// # MAGMA Multi-Graph Architecture
/// Maintains four separate graph namespaces with intent-aware routing:
/// - SemanticGraph: Concept nodes, is_a/part_of/supports/contradicts edges
/// - TemporalGraph: Event nodes, superseded_by/evolved_into/prior_state edges
/// - CausalGraph: Action/outcome nodes, requires/generates/modifies edges
/// - EntityGraph: Person/project/service nodes, works_for/knows/founded/advises edges
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReflectiveMemory {
    /// Semantic graph: concept nodes with hierarchical/epistemic relations.
    pub semantic: NamespaceGraph,
    /// Temporal graph: event nodes with temporal ordering relations.
    pub temporal: NamespaceGraph,
    /// Causal graph: action/outcome nodes with causal relations.
    pub causal: NamespaceGraph,
    /// Entity graph: person/project/service nodes with social relations.
    pub entity: NamespaceGraph,
    /// Last consolidation timestamp.
    pub last_consolidation: i64,
}

impl ReflectiveMemory {
    /// Creates an empty reflective memory with all four graph namespaces.
    pub fn new() -> Self {
        Self {
            semantic: NamespaceGraph::new(GraphNamespace::Semantic),
            temporal: NamespaceGraph::new(GraphNamespace::Temporal),
            causal: NamespaceGraph::new(GraphNamespace::Causal),
            entity: NamespaceGraph::new(GraphNamespace::Entity),
            last_consolidation: chrono::Utc::now().timestamp(),
        }
    }

    /// Returns the appropriate graph namespace for a query intent.
    pub fn graph_for_intent(&self, intent: &QueryIntent) -> Option<&NamespaceGraph> {
        match intent {
            QueryIntent::Semantic => Some(&self.semantic),
            QueryIntent::Temporal => Some(&self.temporal),
            QueryIntent::Causal => Some(&self.causal),
            QueryIntent::Entity => Some(&self.entity),
            QueryIntent::Hybrid => None,
        }
    }

    /// Returns the appropriate mutable graph namespace for a query intent.
    pub fn graph_for_intent_mut(&mut self, intent: &QueryIntent) -> Option<&mut NamespaceGraph> {
        match intent {
            QueryIntent::Semantic => Some(&mut self.semantic),
            QueryIntent::Temporal => Some(&mut self.temporal),
            QueryIntent::Causal => Some(&mut self.causal),
            QueryIntent::Entity => Some(&mut self.entity),
            QueryIntent::Hybrid => None,
        }
    }

    /// Routes a query to the appropriate namespace and returns matching concepts.
    pub fn resolve(&self, query: &str) -> Vec<&Concept> {
        let intent = resolve_graph_intent(query);
        match intent {
            QueryIntent::Hybrid => {
                let mut results = Vec::new();
                results.extend(self.semantic.find_concepts(query));
                results.extend(self.temporal.find_concepts(query));
                results.extend(self.causal.find_concepts(query));
                results.extend(self.entity.find_concepts(query));
                results
            }
            _ => self
                .graph_for_intent(&intent)
                .map(|g| g.find_concepts(query))
                .unwrap_or_default(),
        }
    }

    /// Routes a query to the appropriate namespace and returns ranked concept matches.
    /// Uses three-tier matching: exact > substring > word overlap.
    pub fn resolve_ranked(&self, query: &str) -> Vec<ConceptMatch<'_>> {
        let intent = resolve_graph_intent(query);
        match intent {
            QueryIntent::Hybrid => {
                let mut results = Vec::new();
                results.extend(self.semantic.find_concepts_ranked(query));
                results.extend(self.temporal.find_concepts_ranked(query));
                results.extend(self.causal.find_concepts_ranked(query));
                results.extend(self.entity.find_concepts_ranked(query));
                results.sort_by(|a, b| {
                    b.relevance
                        .partial_cmp(&a.relevance)
                        .unwrap_or(std::cmp::Ordering::Equal)
                });
                results
            }
            _ => self
                .graph_for_intent(&intent)
                .map(|g| g.find_concepts_ranked(query))
                .unwrap_or_default(),
        }
    }

    /// Adds a concept to the appropriate namespace based on the relation type.
    pub fn add_concept_to_namespace(&mut self, concept: Concept, namespace: GraphNamespace) {
        match namespace {
            GraphNamespace::Semantic => self.semantic.add_concept(concept),
            GraphNamespace::Temporal => self.temporal.add_concept(concept),
            GraphNamespace::Causal => self.causal.add_concept(concept),
            GraphNamespace::Entity => self.entity.add_concept(concept),
        }
    }

    /// Adds a relation to the appropriate namespace based on the relation type.
    pub fn add_relation_to_namespace(&mut self, relation: Relation) -> Result<(), String> {
        // Determine namespace from relation type
        for ns in [
            GraphNamespace::Semantic,
            GraphNamespace::Temporal,
            GraphNamespace::Causal,
            GraphNamespace::Entity,
        ] {
            if ns.contains_relation(&relation.relation_type) {
                match ns {
                    GraphNamespace::Semantic => self.semantic.add_relation(relation),
                    GraphNamespace::Temporal => self.temporal.add_relation(relation),
                    GraphNamespace::Causal => self.causal.add_relation(relation),
                    GraphNamespace::Entity => self.entity.add_relation(relation),
                }
                return Ok(());
            }
        }
        // Default to semantic for unknown relation types
        self.semantic.add_relation(relation);
        Ok(())
    }

    /// Returns the total concept count across all namespaces.
    pub fn total_concept_count(&self) -> usize {
        self.semantic.concept_count()
            + self.temporal.concept_count()
            + self.causal.concept_count()
            + self.entity.concept_count()
    }

    /// Returns the total relation count across all namespaces.
    pub fn total_relation_count(&self) -> usize {
        self.semantic.relation_count()
            + self.temporal.relation_count()
            + self.causal.relation_count()
            + self.entity.relation_count()
    }

    /// Finds all relations for a concept across all namespaces.
    pub fn find_all_relations(&self, concept_id: &str) -> Vec<&Relation> {
        let mut results = Vec::new();
        results.extend(self.semantic.find_relations(concept_id));
        results.extend(self.temporal.find_relations(concept_id));
        results.extend(self.causal.find_relations(concept_id));
        results.extend(self.entity.find_relations(concept_id));
        results
    }

    // ─── Backward-compatible API ─────────────────────────────────────────

    /// Adds a concept to the semantic namespace (backward-compatible).
    pub fn add_concept(&mut self, concept: Concept) {
        self.semantic.add_concept(concept);
    }

    /// Adds a relation to the appropriate namespace (backward-compatible).
    pub fn add_relation(&mut self, relation: Relation) {
        if let Err(e) = self.add_relation_to_namespace(relation) {
            tracing::warn!("[reflective] Failed to add relation: {}", e);
        }
    }

    /// Finds concepts by label across all namespaces (backward-compatible).
    pub fn find_concepts(&self, query: &str) -> Vec<&Concept> {
        self.resolve(query)
    }

    /// Finds all relations for a concept across all namespaces (backward-compatible).
    pub fn find_relations(&self, concept_id: &str) -> Vec<&Relation> {
        self.find_all_relations(concept_id)
    }

    /// Returns the total concept count (backward-compatible).
    pub fn concept_count(&self) -> usize {
        self.total_concept_count()
    }

    /// Returns the total relation count (backward-compatible).
    pub fn relation_count(&self) -> usize {
        self.total_relation_count()
    }
}

impl Default for ReflectiveMemory {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
#[cfg(test)]
#[allow(clippy::disallowed_methods)]
mod tests {
    use super::*;

    #[test]
    fn test_add_and_find_concept() {
        let mut memory = ReflectiveMemory::new();
        memory.add_concept(Concept {
            id: "c1".to_string(),
            label: "Build System".to_string(),
            source_entries: vec![1, 2, 3],
            concept_type: ConceptType::Semantic,
            created_at: 0,
            last_accessed: 0,
        });

        let found = memory.find_concepts("build");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].label, "Build System");
    }

    #[test]
    fn test_relations() {
        let mut memory = ReflectiveMemory::new();
        memory.add_relation(Relation {
            relation_type: "is_a".to_string(),
            weight: 0.9,
            source_concept: "c1".to_string(),
            target_concept: "c2".to_string(),
        });

        let relations = memory.find_relations("c1");
        assert_eq!(relations.len(), 1);
    }

    #[test]
    fn test_deduplicate_concepts() {
        let mut memory = ReflectiveMemory::new();
        memory.add_concept(Concept {
            id: "c1".to_string(),
            label: "Original".to_string(),
            source_entries: vec![],
            concept_type: ConceptType::Semantic,
            created_at: 0,
            last_accessed: 0,
        });
        memory.add_concept(Concept {
            id: "c1".to_string(),
            label: "Updated".to_string(),
            source_entries: vec![1],
            concept_type: ConceptType::Semantic,
            created_at: 1,
            last_accessed: 1,
        });

        assert_eq!(memory.concept_count(), 1);
        // Find the concept in any namespace
        let found = memory.find_concepts("Updated");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].label, "Updated");
    }

    #[test]
    fn test_namespace_routing() {
        let mut memory = ReflectiveMemory::new();

        // Add concepts to different namespaces
        memory.add_concept_to_namespace(
            Concept {
                id: "s1".to_string(),
                label: "Rust Language".to_string(),
                source_entries: vec![],
                concept_type: ConceptType::Semantic,
                created_at: 0,
                last_accessed: 0,
            },
            GraphNamespace::Semantic,
        );
        memory.add_concept_to_namespace(
            Concept {
                id: "e1".to_string(),
                label: "Spencer".to_string(),
                source_entries: vec![],
                concept_type: ConceptType::Identity,
                created_at: 0,
                last_accessed: 0,
            },
            GraphNamespace::Entity,
        );

        assert_eq!(memory.semantic.concept_count(), 1);
        assert_eq!(memory.entity.concept_count(), 1);
        assert_eq!(memory.temporal.concept_count(), 0);
    }

    #[test]
    fn test_intent_routing() {
        assert_eq!(
            resolve_graph_intent("What is a build system?"),
            QueryIntent::Semantic
        );
        assert_eq!(
            resolve_graph_intent("When did the project start?"),
            QueryIntent::Temporal
        );
        assert_eq!(
            resolve_graph_intent("What caused the failure?"),
            QueryIntent::Causal
        );
        assert_eq!(
            resolve_graph_intent("Who founded the company?"),
            QueryIntent::Entity
        );
        assert_eq!(resolve_graph_intent("Tell me about X"), QueryIntent::Hybrid);
    }

    #[test]
    fn test_namespace_relation_types() {
        assert!(GraphNamespace::Semantic.contains_relation("is_a"));
        assert!(GraphNamespace::Semantic.contains_relation("supports"));
        assert!(GraphNamespace::Temporal.contains_relation("evolved_into"));
        assert!(GraphNamespace::Causal.contains_relation("requires"));
        assert!(GraphNamespace::Entity.contains_relation("founded"));
        assert!(GraphNamespace::Entity.contains_relation("works_for"));
    }

    #[test]
    fn test_relation_auto_routing() {
        let mut memory = ReflectiveMemory::new();

        // is_a → Semantic
        memory
            .add_relation_to_namespace(Relation {
                relation_type: "is_a".to_string(),
                weight: 0.9,
                source_concept: "c1".to_string(),
                target_concept: "c2".to_string(),
            })
            .unwrap();
        assert_eq!(memory.semantic.relation_count(), 1);

        // founded → Entity
        memory
            .add_relation_to_namespace(Relation {
                relation_type: "founded".to_string(),
                weight: 1.0,
                source_concept: "p1".to_string(),
                target_concept: "c1".to_string(),
            })
            .unwrap();
        assert_eq!(memory.entity.relation_count(), 1);

        // requires → Causal
        memory
            .add_relation_to_namespace(Relation {
                relation_type: "requires".to_string(),
                weight: 0.8,
                source_concept: "a1".to_string(),
                target_concept: "a2".to_string(),
            })
            .unwrap();
        assert_eq!(memory.causal.relation_count(), 1);
    }

    #[test]
    fn test_hybrid_query() {
        let mut memory = ReflectiveMemory::new();

        memory.add_concept_to_namespace(
            Concept {
                id: "s1".to_string(),
                label: "Savant Project".to_string(),
                source_entries: vec![],
                concept_type: ConceptType::Semantic,
                created_at: 0,
                last_accessed: 0,
            },
            GraphNamespace::Semantic,
        );
        memory.add_concept_to_namespace(
            Concept {
                id: "e1".to_string(),
                label: "Savant Project".to_string(),
                source_entries: vec![],
                concept_type: ConceptType::Identity,
                created_at: 0,
                last_accessed: 0,
            },
            GraphNamespace::Entity,
        );

        // Hybrid query should find both
        let results = memory.resolve("Savant Project");
        assert_eq!(results.len(), 2);
    }
}
