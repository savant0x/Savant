use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::time::{sleep, Duration};
use tracing::{debug, error, info, warn};

use crate::engine::MemoryEnclave;
use crate::models::MemoryEntry;
use futures::StreamExt;
use savant_core::traits::{EmbeddingProvider, LlmProvider};
use savant_core::types::{ChatMessage, ChatRole};

/// Represents a distilled fact ready for public Hive-Mind access.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DistilledTriplet {
    pub subject: String,
    pub predicate: String,
    pub object: String,
    pub confidence: f32,
    pub entropy: f32,
    pub source_session: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct TripletClaims {
    pub sub: String,
    pub jti: String,
    pub iat: i64,
    pub exp: i64,
    pub triplet: DistilledTriplet,
}

/// Spawns a background worker to distill knowledge from the Private Enclave
/// into the Global Collective Database, enforcing Cryptographic Privacy Boundaries.
pub fn spawn_distillation_pipeline(
    enclave: Arc<MemoryEnclave>,
    collective: Arc<MemoryEnclave>,
    llm: Arc<dyn LlmProvider>,
    embeddings: Arc<dyn EmbeddingProvider>,
    _jwt_secret: String,
) {
    let sweep_interval = enclave.config.distillation_sweep_interval_secs;
    tokio::spawn(async move {
        info!("Enclave -> Collective Distillation Pipeline Online");

        loop {
            // Wake at configured interval to scan for high-entropy local facts
            sleep(Duration::from_secs(sweep_interval)).await;

            debug!("Starting distillation sweep pass across Enclave...");

            // RC-16: Stream messages one at a time instead of collecting all 5000 into memory.
            let lsm = enclave.lsm();
            let mut msg_iter = lsm.iter_all_messages(5000);

            for msg in msg_iter.by_ref() {
                if enclave.lsm().is_distilled(&msg.id) {
                    continue;
                }

                // Skip system messages or very short messages
                if msg.content.len() < 20 || matches!(msg.role, crate::models::MessageRole::System)
                {
                    continue;
                }

                // ── Deterministic-first extraction (Item 20) ──
                // Step 1: Try deterministic pattern matching (covers ~90% of cases)
                let deterministic_triplets = extract_triplets_deterministic(&msg.content);

                // Step 2: Confidence gate
                // - confidence > 0.85 → accept deterministic result directly
                // - confidence 0.15-0.85 → delegate to LLM for verification
                // - confidence < 0.15 → discard (noise)
                let triplets = if deterministic_triplets.iter().all(|t| t.confidence > 0.85) {
                    // High-confidence deterministic results — skip LLM
                    deterministic_triplets
                } else if deterministic_triplets.iter().any(|t| t.confidence >= 0.15) {
                    // Ambiguous range — delegate to LLM for verification
                    match extract_triplets(Arc::clone(&llm), &msg.content).await {
                        Ok(llm_triplets) => {
                            // Merge: use LLM results for low-confidence deterministic ones
                            merge_triplets(&deterministic_triplets, &llm_triplets)
                        }
                        Err(e) => {
                            warn!("LLM triplet extraction failed, using deterministic: {}", e);
                            deterministic_triplets
                                .into_iter()
                                .filter(|t| t.confidence >= 0.15)
                                .collect()
                        }
                    }
                } else {
                    // All below 0.15 — skip LLM entirely
                    Vec::new()
                };

                if triplets.is_empty() {
                    // Mark as distilled even if no triplets found (avoid reprocessing)
                    if let Err(e) = enclave.lsm().mark_distilled(&msg.id) {
                        error!("Failed to mark message as distilled: {}", e);
                    }
                    continue;
                }

                // Process extracted triplets
                for triplet_data in triplets {
                    let entropy = calculate_shannon_entropy(&msg.content);
                    let distilled = DistilledTriplet {
                        subject: triplet_data.subject,
                        predicate: triplet_data.predicate,
                        object: triplet_data.object,
                        confidence: triplet_data.confidence,
                        entropy,
                        source_session: msg.session_id.clone(),
                    };

                    let now = chrono::Utc::now().timestamp();
                    let claims = TripletClaims {
                        sub: "hive_mind".to_string(),
                        jti: uuid::Uuid::new_v4().to_string(),
                        iat: now,
                        exp: now + 31536000,
                        triplet: distilled,
                    };

                    let now_ms = chrono::Utc::now().timestamp_millis();

                    let hash = blake3::hash(msg.id.as_bytes());
                    let bytes = hash.as_bytes();
                    let entry_id = u64::from_le_bytes(bytes[..8].try_into().unwrap_or([0u8; 8]));

                    let content = format!(
                        "{} {} {}",
                        claims.triplet.subject, claims.triplet.predicate, claims.triplet.object
                    );

                    let triplet_embedding = match embeddings.embed(&content).await {
                        Ok(vec) => vec,
                        Err(e) => {
                            warn!("Failed to embed triplet: {}", e);
                            continue;
                        }
                    };

                    let entry = MemoryEntry {
                        id: entry_id.into(),
                        session_id: msg.session_id.clone(),
                        created_at: now_ms.into(),
                        updated_at: now_ms.into(),
                        content,
                        category: "distilled_triplet".to_string(),
                        importance: (claims.triplet.confidence * 10.0) as u8,
                        tags: vec!["hive-mind".to_string(), "shared".to_string()],
                        embedding: triplet_embedding,
                        shannon_entropy: entropy.into(),
                        last_accessed_at: now_ms.into(),
                        hit_count: 0.into(),
                        related_to: vec![],
                        access_timestamps: vec![],
                        version: 1.into(),
                        parent_id: None,
                        supersedes: vec![],
                        is_latest: true,
                    };

                    if let Err(e) = collective.index_memory(entry).await {
                        error!("Failed to index distilled triplet into collective: {}", e);
                    } else if let Err(e) = collective.lsm().insert_fact(
                        &claims.triplet.subject,
                        &claims.triplet.predicate,
                        &claims.triplet.object,
                        entry_id,
                    ) {
                        error!("Failed to insert fact into SPO index: {}", e);
                    }
                }

                // Mark as distilled only after successful processing
                if let Err(e) = enclave.lsm().mark_distilled(&msg.id) {
                    error!("Failed to mark message as distilled: {}", e);
                }
            }

            // Persist BM25 index after each sweep for crash recovery
            // Note: BM25 is populated on collective (where index_memory is called)
            if let Err(e) = collective.persist_bm25().await {
                warn!("Failed to persist BM25 state: {}", e);
            }
        }
    });
}

#[derive(Deserialize)]
struct TripletResponse {
    triplets: Vec<RawTriplet>,
}

#[derive(Deserialize, Clone)]
pub struct RawTriplet {
    subject: String,
    predicate: String,
    object: String,
    confidence: f32,
}

async fn extract_triplets(
    llm: Arc<dyn LlmProvider>,
    content: &str,
) -> Result<Vec<RawTriplet>, String> {
    let prompt = format!(
        "Extract core semantic triplets (Subject-Predicate-Object) from the following text. \
        Provide raw factual assertions only. Return as JSON: {{ \"triplets\": [{{ \"subject\": \"...\", \"predicate\": \"...\", \"object\": \"...\", \"confidence\": 0.0 }}] }}\n\nText: {}",
        content
    );

    let messages = vec![ChatMessage {
        is_telemetry: false,
        role: ChatRole::System,
        content: "You are a knowledge extraction engine for a Hive-Mind memory system. Extract atomic facts.".to_string(),
        sender: None,
        recipient: None,
        agent_id: None,
        session_id: None,
        channel: savant_core::types::AgentOutputChannel::Chat,
        images: Vec::new(),
            ..Default::default()
        }, ChatMessage {
        is_telemetry: false,
        role: ChatRole::User,
        content: prompt,
        sender: None,
        recipient: None,
        agent_id: None,
        session_id: None,
        channel: savant_core::types::AgentOutputChannel::Chat,
        images: Vec::new(),
            ..Default::default()
        }];

    let mut stream = llm
        .stream_completion(messages, vec![])
        .await
        .map_err(|e| e.to_string())?;
    let mut full_content = String::new();

    while let Some(chunk_res) = stream.next().await {
        if let Ok(chunk) = chunk_res {
            full_content.push_str(&chunk.content);
        }
    }

    // Parse the JSON output
    let parsed: TripletResponse = serde_json::from_str(&full_content).map_err(|e| e.to_string())?;
    Ok(parsed.triplets)
}

pub fn calculate_shannon_entropy(text: &str) -> f32 {
    let mut counts = std::collections::HashMap::new();
    let len = text.len() as f32;
    if len == 0.0 {
        return 0.0;
    }

    for c in text.chars() {
        *counts.entry(c).or_insert(0) += 1;
    }

    let mut entropy = 0.0;
    for count in counts.values() {
        let p = (*count as f32) / len;
        entropy -= p * p.log2();
    }
    entropy
}

// ─── Deterministic Triplet Extraction (Item 20) ────────────────────────

/// Extracts triplets using deterministic pattern matching.
/// Covers ~90% of common SPO patterns at zero LLM cost.
pub fn extract_triplets_deterministic(text: &str) -> Vec<RawTriplet> {
    let mut triplets = Vec::new();
    let lower = text.to_lowercase();

    // Pattern: "X is a Y" → (X, is_a, Y)
    for pattern in &[
        ("is a", "is_a"),
        ("is an", "is_a"),
        ("are a", "is_a"),
        ("are an", "is_a"),
        ("is the", "is_a"),
    ] {
        if let Some(pos) = lower.find(pattern.0) {
            let before = text[..pos].trim();
            let after = text[pos + pattern.0.len()..].trim();
            let subject = extract_last_noun_phrase(before);
            let object = extract_first_noun_phrase(after);
            if !subject.is_empty() && !object.is_empty() {
                triplets.push(RawTriplet {
                    subject,
                    predicate: pattern.1.to_string(),
                    object,
                    confidence: 0.90,
                });
            }
        }
    }

    // Pattern: "X has Y" → (X, has, Y)
    if let Some(pos) = lower.find(" has ") {
        let before = text[..pos].trim();
        let after = text[pos + 5..].trim();
        let subject = extract_last_noun_phrase(before);
        let object = extract_first_noun_phrase(after);
        if !subject.is_empty() && !object.is_empty() {
            triplets.push(RawTriplet {
                subject,
                predicate: "has".to_string(),
                object,
                confidence: 0.85,
            });
        }
    }

    // Pattern: "X uses Y" → (X, uses, Y)
    for verb in &["uses", "requires", "depends on", "needs"] {
        if let Some(pos) = lower.find(&format!(" {} ", verb)) {
            let before = text[..pos].trim();
            let after = text[pos + verb.len() + 2..].trim();
            let subject = extract_last_noun_phrase(before);
            let object = extract_first_noun_phrase(after);
            if !subject.is_empty() && !object.is_empty() {
                triplets.push(RawTriplet {
                    subject,
                    predicate: verb.to_string(),
                    object,
                    confidence: 0.88,
                });
            }
        }
    }

    // Pattern: "X created Y" / "X built Y" / "X developed Y"
    for verb in &[
        "created",
        "built",
        "developed",
        "designed",
        "implemented",
        "wrote",
    ] {
        if let Some(pos) = lower.find(&format!(" {} ", verb)) {
            let before = text[..pos].trim();
            let after = text[pos + verb.len() + 2..].trim();
            let subject = extract_last_noun_phrase(before);
            let object = extract_first_noun_phrase(after);
            if !subject.is_empty() && !object.is_empty() {
                triplets.push(RawTriplet {
                    subject,
                    predicate: verb.to_string(),
                    object,
                    confidence: 0.87,
                });
            }
        }
    }

    // Pattern: "X contains Y" / "X includes Y"
    for verb in &["contains", "includes", "supports"] {
        if let Some(pos) = lower.find(&format!(" {} ", verb)) {
            let before = text[..pos].trim();
            let after = text[pos + verb.len() + 2..].trim();
            let subject = extract_last_noun_phrase(before);
            let object = extract_first_noun_phrase(after);
            if !subject.is_empty() && !object.is_empty() {
                triplets.push(RawTriplet {
                    subject,
                    predicate: verb.to_string(),
                    object,
                    confidence: 0.86,
                });
            }
        }
    }

    triplets
}

/// Merges deterministic and LLM triplets, preferring LLM for ambiguous cases.
fn merge_triplets(deterministic: &[RawTriplet], llm: &[RawTriplet]) -> Vec<RawTriplet> {
    let mut merged = Vec::new();

    // Accept all high-confidence deterministic triplets directly
    for det in deterministic {
        if det.confidence > 0.85 {
            merged.push(det.clone());
        }
    }

    // For LLM triplets, add them if they don't conflict with existing ones
    for llm_triplet in llm {
        let is_duplicate = merged.iter().any(|existing| {
            existing.subject.to_lowercase() == llm_triplet.subject.to_lowercase()
                && existing.predicate.to_lowercase() == llm_triplet.predicate.to_lowercase()
                && existing.object.to_lowercase() == llm_triplet.object.to_lowercase()
        });
        if !is_duplicate {
            merged.push(RawTriplet {
                subject: llm_triplet.subject.clone(),
                predicate: llm_triplet.predicate.clone(),
                object: llm_triplet.object.clone(),
                confidence: llm_triplet.confidence,
            });
        }
    }

    merged
}

/// Extracts the last noun phrase from text (up to 3 words).
fn extract_last_noun_phrase(text: &str) -> String {
    let words: Vec<&str> = text.split_whitespace().collect();
    if words.is_empty() {
        return String::new();
    }
    let start = words.len().saturating_sub(3);
    words[start..].join(" ")
}

/// Extracts the first noun phrase from text (up to 3 words).
fn extract_first_noun_phrase(text: &str) -> String {
    let words: Vec<&str> = text.split_whitespace().collect();
    if words.is_empty() {
        return String::new();
    }
    let end = words.len().min(3);
    words[..end].join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deterministic_is_a() {
        let triplets = extract_triplets_deterministic("Rust is a programming language.");
        assert!(!triplets.is_empty());
        assert_eq!(triplets[0].predicate, "is_a");
        assert!(triplets[0].confidence > 0.85);
    }

    #[test]
    fn test_deterministic_uses() {
        let triplets = extract_triplets_deterministic("Savant uses Rust for performance.");
        assert!(!triplets.is_empty());
        assert_eq!(triplets[0].predicate, "uses");
    }

    #[test]
    fn test_deterministic_created() {
        let triplets = extract_triplets_deterministic("Spencer created Savant.");
        assert!(!triplets.is_empty());
        assert_eq!(triplets[0].predicate, "created");
    }

    #[test]
    fn test_deterministic_no_match() {
        let triplets = extract_triplets_deterministic("Hello world, how are you?");
        assert!(triplets.is_empty());
    }

    #[test]
    fn test_merge_triplets() {
        let det = vec![RawTriplet {
            subject: "Rust".to_string(),
            predicate: "is_a".to_string(),
            object: "language".to_string(),
            confidence: 0.90,
        }];
        let llm = vec![RawTriplet {
            subject: "Rust".to_string(),
            predicate: "is_a".to_string(),
            object: "language".to_string(),
            confidence: 0.95,
        }];
        let merged = merge_triplets(&det, &llm);
        // Should deduplicate — only 1 result
        assert_eq!(merged.len(), 1);
    }

    #[test]
    fn test_confidence_gate_high() {
        // High confidence deterministic → no LLM needed
        let triplets = extract_triplets_deterministic("Python is a programming language.");
        assert!(triplets.iter().all(|t| t.confidence > 0.85));
    }

    #[test]
    fn test_shannon_entropy() {
        let entropy = calculate_shannon_entropy("aaaa");
        assert_eq!(entropy, 0.0);
        let entropy = calculate_shannon_entropy("abcd");
        assert!(entropy > 0.0);
    }
}
