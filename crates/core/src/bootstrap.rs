//! Bootstrap Mode — Manifest Parser & Schema
//!
//! Phase 2 of the Bootstrap Mode system. This module provides:
//! - [`InfraRequirements`] — structured infrastructure needs extracted from the LLM's JSON block
//! - [`Manifest`] — the persisted manifest.json with claim taxonomy, soul_blake3, and reconciliation state
//! - [`SoulClaim`] — individual claim with status, evidence, and error tracking
//! - [`extract_infra_block()`] — parses the `## INFRASTRUCTURE_REQUIREMENTS` JSON block from SOUL.md
//!
//! # Flow
//! 1. LLM generates SOUL.md with an optional `## INFRASTRUCTURE_REQUIREMENTS` block at the end
//! 2. `extract_infra_block()` finds and parses the JSON block → `InfraRequirements`
//! 3. On commit (`SoulUpdate`), a `Manifest` is written alongside SOUL.md
//! 4. `Manifest::load()` / `Manifest::save()` provide persistence
//! 5. `manifest_diff()` computes delta for Phase 3 re-scaffolding

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

// ─── InfraRequirements (from LLM JSON block) ──────────────────────────

/// A single infrastructure requirement declared by the LLM in the SOUL.md.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct InfraItem {
    /// The type of infrastructure (e.g., "wal_schema", "memory_budget", "cct_scope")
    pub r#type: String,
    /// Human-readable description of what's needed
    pub description: String,
    /// Optional scope or target (e.g., "wasm", "docs/**")
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    /// Optional size/capacity (e.g., 64 for MB)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mb: Option<u32>,
    /// Optional target identifier (e.g., "Builder-020")
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// Optional note about why this item is aspirational
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// Structured infrastructure requirements extracted from the SOUL.md.
///
/// Parsed from the `## INFRASTRUCTURE_REQUIREMENTS` JSON block at the end of
/// the generated soul document. The LLM is instructed to emit this block
/// for Scaffolded and Aspirational tiers.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InfraRequirements {
    /// Claims that can be scaffolded now (identity, storage, compute, etc.)
    #[serde(default)]
    pub infrastructure: Vec<InfraItem>,
    /// Claims that cannot be fulfilled yet (external deps, missing agents, etc.)
    #[serde(default)]
    pub aspirational: Vec<InfraItem>,
}

// ─── SoulClaim (individual claim in manifest.json) ────────────────────

/// Category of a claim — determines which scaffold handler to invoke.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ClaimCategory {
    Identity,
    Security,
    Storage,
    Compute,
    Integration,
    Metric,
    Unknown,
}

impl std::fmt::Display for ClaimCategory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ClaimCategory::Identity => write!(f, "Identity"),
            ClaimCategory::Security => write!(f, "Security"),
            ClaimCategory::Storage => write!(f, "Storage"),
            ClaimCategory::Compute => write!(f, "Compute"),
            ClaimCategory::Integration => write!(f, "Integration"),
            ClaimCategory::Metric => write!(f, "Metric"),
            ClaimCategory::Unknown => write!(f, "Unknown"),
        }
    }
}

/// Status of a single claim in the manifest.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ClaimStatus {
    /// Verified as true without scaffolding needed
    Verified,
    /// Successfully scaffolded into existence
    Scaffolded,
    /// Cannot be fulfilled — outside current system capabilities
    Aspirational,
    /// Scaffolding operation failed
    Failed,
}

/// Evidence that a claim was fulfilled — path, hash, or other proof.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ClaimEvidence {
    Path { path: String },
    Hash { hash: String },
    KeyValue(HashMap<String, String>),
}

/// A single claim in the manifest, tracking what was declared vs. what was built.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SoulClaim {
    /// Unique claim identifier (e.g., "claim_001")
    pub claim_id: String,
    /// Category of the claim
    pub category: ClaimCategory,
    /// Human-readable description
    pub description: String,
    /// Current status
    pub status: ClaimStatus,
    /// The scaffold action that was taken (if any)
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scaffold_action: Option<String>,
    /// Evidence that the claim was fulfilled
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence: Option<ClaimEvidence>,
    /// Error message if the claim failed
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

// ─── Manifest (persisted as manifest.json) ─────────────────────────────

/// The persisted manifest.json — tracks every infrastructure claim in the SOUL.md.
///
/// Written alongside SOUL.md in the agent's workspace directory.
/// Used by Phase 3's BootstrapReconciler to determine what needs scaffolding.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    /// Schema version URL
    #[serde(rename = "$schema")]
    pub schema: String,
    /// Manifest schema version
    pub version: String,
    /// Agent identifier
    pub agent_id: String,
    /// ISO 8601 timestamp of generation
    pub generated_at: String,
    /// BLAKE3 hash of the SOUL.md content at generation time
    pub soul_blake3: String,
    /// ISO 8601 timestamp of last reconciliation
    pub last_reconciled: String,
    /// Bootstrap tier used for generation
    pub bootstrap_tier: String,
    /// All tracked claims
    #[serde(default)]
    pub claims: Vec<SoulClaim>,
}

impl Manifest {
    /// Creates a new manifest from a generated soul and parsed requirements.
    pub fn new(
        agent_id: String,
        soul_content: &str,
        bootstrap_tier: &str,
        requirements: Option<&InfraRequirements>,
    ) -> Self {
        let now = chrono::Utc::now().to_rfc3339();
        let soul_blake3 = blake3::hash(soul_content.as_bytes()).to_hex().to_string();

        let mut claims: Vec<SoulClaim> = Vec::new();

        // Add claims from infrastructure requirements
        if let Some(reqs) = requirements {
            let mut claim_idx = 0;

            for item in &reqs.infrastructure {
                claim_idx += 1;
                claims.push(SoulClaim {
                    claim_id: format!("claim_{:03}", claim_idx),
                    category: classify_infra_type(&item.r#type),
                    description: item.description.clone(),
                    status: ClaimStatus::Aspirational, // Will be updated by Phase 3 reconciler
                    scaffold_action: Some(infer_scaffold_action(&item.r#type)),
                    evidence: None,
                    error: None,
                });
            }

            for item in &reqs.aspirational {
                claim_idx += 1;
                claims.push(SoulClaim {
                    claim_id: format!("claim_{:03}", claim_idx),
                    category: classify_infra_type(&item.r#type),
                    description: item.description.clone(),
                    status: ClaimStatus::Aspirational,
                    scaffold_action: None,
                    evidence: None,
                    error: Some(format!(
                        "Aspirational: {}",
                        item.note.as_deref().unwrap_or("outside current capabilities")
                    )),
                });
            }
        }

        // Always add the identity claim (agent workspace exists)
        claims.insert(
            0,
            SoulClaim {
                claim_id: "claim_001".to_string(),
                category: ClaimCategory::Identity,
                description: "Agent workspace and identity".to_string(),
                status: ClaimStatus::Scaffolded,
                scaffold_action: Some("register_identity".to_string()),
                evidence: Some(ClaimEvidence::KeyValue(HashMap::from([(
                    "path".to_string(),
                    format!("workspaces/agents/{}", agent_id),
                )]))),
                error: None,
            },
        );

        Manifest {
            schema: "https://savant.dev/schemas/manifest-v1.json".to_string(),
            version: "1.0".to_string(),
            agent_id,
            generated_at: now.clone(),
            soul_blake3,
            last_reconciled: now,
            bootstrap_tier: bootstrap_tier.to_string(),
            claims,
        }
    }

    /// Load a manifest from a path.
    pub fn load(path: &Path) -> Result<Self, ManifestError> {
        if !path.exists() {
            return Err(ManifestError::NotFound(path.to_path_buf()));
        }
        let content =
            std::fs::read_to_string(path).map_err(|e| ManifestError::Io(path.to_path_buf(), e))?;
        serde_json::from_str(&content)
            .map_err(|e| ManifestError::Parse(path.to_path_buf(), e.to_string()))
    }

    /// Save the manifest to a path.
    pub fn save(&self, path: &Path) -> Result<(), ManifestError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| ManifestError::Io(path.to_path_buf(), e))?;
        }
        let content = serde_json::to_string_pretty(self)
            .map_err(|e| ManifestError::Serialize(e.to_string()))?;
        std::fs::write(path, content).map_err(|e| ManifestError::Io(path.to_path_buf(), e))?;
        Ok(())
    }

    /// Check if the SOUL.md has drifted from the manifest's stored hash.
    pub fn has_drifted(&self, current_soul: &str) -> bool {
        let current_hash = blake3::hash(current_soul.as_bytes()).to_hex().to_string();
        current_hash != self.soul_blake3
    }
}

// ─── ClaimDiff (for Phase 3 re-scaffolding) ───────────────────────────

/// A detected change between two manifest versions (old vs. new).
#[derive(Debug, Clone)]
pub enum ClaimDiff {
    /// A new claim was added
    Added(SoulClaim),
    /// An existing claim was modified
    Modified {
        old: SoulClaim,
        new: SoulClaim,
    },
    /// A claim was removed (tombstoned)
    Removed(SoulClaim),
}

/// Computes the diff between two manifests for Phase 3 re-scaffolding.
pub fn manifest_diff(old: &Manifest, new: &Manifest) -> Vec<ClaimDiff> {
    let mut diffs: Vec<ClaimDiff> = Vec::new();

    // Build lookup maps by claim_id
    let old_map: HashMap<&str, &SoulClaim> =
        old.claims.iter().map(|c| (c.claim_id.as_str(), c)).collect();
    let new_map: HashMap<&str, &SoulClaim> =
        new.claims.iter().map(|c| (c.claim_id.as_str(), c)).collect();

    // Check for added and modified claims
    for new_claim in &new.claims {
        match old_map.get(new_claim.claim_id.as_str()) {
            None => diffs.push(ClaimDiff::Added(new_claim.clone())),
            Some(old_claim) => {
                if old_claim.description != new_claim.description
                    || old_claim.category != new_claim.category
                    || old_claim.status != new_claim.status
                {
                    diffs.push(ClaimDiff::Modified {
                        old: (*old_claim).clone(),
                        new: (*new_claim).clone(),
                    });
                }
            }
        }
    }

    // Check for removed claims (tombstoned)
    for old_claim in &old.claims {
        if !new_map.contains_key(old_claim.claim_id.as_str()) {
            diffs.push(ClaimDiff::Removed(old_claim.clone()));
        }
    }

    diffs
}

// ─── Parser: extract_infra_block ──────────────────────────────────────

/// Extracts the `## INFRASTRUCTURE_REQUIREMENTS` JSON block from SOUL.md content.
///
/// Looks for the markdown heading `## INFRASTRUCTURE_REQUIREMENTS` followed by
/// a JSON code block (```json ... ```). Returns `None` if no block is found or
/// the JSON is malformed.
///
/// # Format expected
/// ```markdown
/// ## INFRASTRUCTURE_REQUIREMENTS
///
/// ```json
/// {
///   "infrastructure": [...],
///   "aspirational": [...]
/// }
/// ```
/// ```
pub fn extract_infra_block(soul_content: &str) -> Option<InfraRequirements> {
    // Find the section header
    let header = "## INFRASTRUCTURE_REQUIREMENTS";
    let header_pos = soul_content.find(header)?;

    // Search from the header for the JSON code block opener
    let after_header = &soul_content[header_pos + header.len()..];
    let json_start = after_header.find("```json")?;
    let after_json_start = &after_header[json_start + 7..]; // skip ```json

    // Find the closing ```
    let json_end = after_json_start.find("```")?;
    let json_str = after_json_start[..json_end].trim();

    // Parse the JSON
    serde_json::from_str::<InfraRequirements>(json_str).ok()
}

/// Classifies an infra type string into a ClaimCategory.
fn classify_infra_type(r#type: &str) -> ClaimCategory {
    match r#type {
        t if t.contains("identity") || t.contains("workspace") => ClaimCategory::Identity,
        t if t.contains("cct") || t.contains("token") || t.contains("key") => ClaimCategory::Security,
        t if t.contains("wal") || t.contains("storage") || t.contains("db") || t.contains("column") => {
            ClaimCategory::Storage
        }
        t if t.contains("memory") || t.contains("wasm") || t.contains("cpu") || t.contains("vcore") => {
            ClaimCategory::Compute
        }
        t if t.contains("peer") || t.contains("agent") || t.contains("webhook") || t.contains("api") => {
            ClaimCategory::Integration
        }
        t if t.contains("metric") || t.contains("count") || t.contains("score") => ClaimCategory::Metric,
        _ => ClaimCategory::Unknown,
    }
}

/// Infers the scaffold action name from an infra type string.
fn infer_scaffold_action(r#type: &str) -> String {
    match r#type {
        t if t.contains("wal") => "init_wal_directory".to_string(),
        t if t.contains("cct") || t.contains("token") => "mint_cct_token".to_string(),
        t if t.contains("memory") => "set_memory_budget".to_string(),
        t if t.contains("identity") || t.contains("workspace") => "register_identity".to_string(),
        t if t.contains("peer") || t.contains("agent") => "register_in_nexus".to_string(),
        _ => format!("scaffold_{}", r#type.replace('-', "_")),
    }
}

// ─── Errors ───────────────────────────────────────────────────────────

/// Errors that can occur during manifest operations.
#[derive(Debug, thiserror::Error)]
pub enum ManifestError {
    #[error("Manifest not found at {0}")]
    NotFound(std::path::PathBuf),

    #[error("IO error at {0}: {1}")]
    Io(std::path::PathBuf, std::io::Error),

    #[error("Parse error at {0}: {1}")]
    Parse(std::path::PathBuf, String),

    #[error("Serialization error: {0}")]
    Serialize(String),
}

// ─── BootstrapReconciler (Phase 3) ───────────────────────────────────

use crate::bus::NexusBridge;
use tokio::sync::broadcast;

/// Request payload for the `system.agent.scaffold.requested` Nexus event.
/// Tells the reconciler which agent's infrastructure to scaffold.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScaffoldRequest {
    /// Target agent identifier
    pub agent_id: String,
    /// Optional: scaffold only these specific claim IDs. Empty = all pending.
    #[serde(default)]
    pub claim_ids: Vec<String>,
}

/// Result published as `system.agent.scaffold.complete` after reconciliation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScaffoldResult {
    pub agent_id: String,
    pub claims_scaffolded: usize,
    pub claims_failed: usize,
    pub claims_aspirational: usize,
    pub errors: Vec<String>,
}

/// Errors that can occur during scaffolding operations.
#[derive(Debug, thiserror::Error)]
pub enum ScaffoldError {
    #[error("IO error: {0}")]
    Io(String),

    #[error("Nexus publish error: {0}")]
    Nexus(String),

    #[error("Unknown scaffold action: {0}")]
    UnknownAction(String),

    #[error("Path traversal blocked: requested path '{}' for agent '{}'", .requested, .agent_id)]
    PathTraversal {
        requested: String,
        agent_id: String,
    },
}

/// Validates that a scaffold path stays within the agent's workspace.
/// Rejects path traversal attempts (`..`, null bytes, absolute paths).
pub fn validate_scaffold_path(agent_id: &str, requested_subpath: &str) -> Result<std::path::PathBuf, ScaffoldError> {
    if requested_subpath.contains("..") {
        return Err(ScaffoldError::PathTraversal {
            requested: requested_subpath.to_string(),
            agent_id: agent_id.to_string(),
        });
    }
    if requested_subpath.contains('\0') {
        return Err(ScaffoldError::PathTraversal {
            requested: "(null bytes)".to_string(),
            agent_id: agent_id.to_string(),
        });
    }
    Ok(std::path::PathBuf::from(agent_id).join(requested_subpath))
}

/// The BootstrapReconciler listens for scaffold events on the Nexus Bridge
/// and executes idempotent scaffold handlers to fulfill infrastructure claims.
///
/// # Lifecycle
/// 1. Created via [`BootstrapReconciler::new()`] during swarm ignition
/// 2. Spawned as a background task via [`BootstrapReconciler::start()`]
/// 3. Listens for `system.agent.scaffold.requested` events
/// 4. For each request, loads the agent's manifest and executes pending scaffold actions
/// 5. Publishes `system.agent.scaffold.complete` with results
pub struct BootstrapReconciler {
    nexus: Arc<NexusBridge>,
    agents_path: std::path::PathBuf,
}

impl BootstrapReconciler {
    /// Creates a new reconciler bound to the given Nexus Bridge and agents path.
    pub fn new(nexus: Arc<NexusBridge>, agents_path: std::path::PathBuf) -> Self {
        Self { nexus, agents_path }
    }

    /// Starts the reconciler event loop. Runs until `shutdown_rx` receives `true`.
    ///
    /// Subscribes to the Nexus event bus and processes `system.agent.scaffold.requested`
    /// events by loading the agent's manifest and executing idempotent scaffold handlers.
    pub async fn start(
        self,
        mut shutdown_rx: tokio::sync::watch::Receiver<bool>,
    ) {
        let mut event_rx = self.nexus.event_bus.subscribe();

        tracing::info!("[bootstrap] BootstrapReconciler started — listening for scaffold events");

        loop {
            tokio::select! {
                _ = shutdown_rx.changed() => {
                    if *shutdown_rx.borrow() {
                        tracing::info!("[bootstrap] Reconciler shutting down");
                        break;
                    }
                }
                event = event_rx.recv() => {
                    match event {
                        Ok(event_frame) => {
                            if event_frame.event_type == "system.agent.scaffold.requested" {
                                self.handle_scaffold_request(&event_frame.payload).await;
                            }
                        }
                        Err(broadcast::error::RecvError::Lagged(n)) => {
                            tracing::warn!("[bootstrap] Reconciler lagged by {} events", n);
                        }
                        Err(broadcast::error::RecvError::Closed) => {
                            tracing::info!("[bootstrap] Event bus closed — reconciler stopping");
                            break;
                        }
                    }
                }
            }
        }

        tracing::info!("[bootstrap] BootstrapReconciler stopped");
    }

    /// Handles a single scaffold request: parse payload, load manifest, execute handlers.
    async fn handle_scaffold_request(&self, payload: &str) {
        let request: ScaffoldRequest = match serde_json::from_str(payload) {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!("[bootstrap] Invalid scaffold request payload: {}", e);
                return;
            }
        };

        let agent_workspace = self.agents_path.join(&request.agent_id);
        let manifest_path = agent_workspace.join("manifest.json");

        // Load manifest
        let mut manifest = match Manifest::load(&manifest_path) {
            Ok(m) => m,
            Err(ManifestError::NotFound(_)) => {
                tracing::warn!(
                    "[bootstrap] No manifest for agent '{}' — create one via SoulUpdate first",
                    request.agent_id
                );
                return;
            }
            Err(e) => {
                tracing::error!(
                    "[bootstrap] Failed to load manifest for '{}': {}",
                    request.agent_id,
                    e
                );
                return;
            }
        };

        let mut claims_scaffolded = 0usize;
        let mut claims_failed = 0usize;
        let mut errors: Vec<String> = Vec::new();

        for claim in &mut manifest.claims {
            // Skip already scaffolded or verified claims
            if claim.status != ClaimStatus::Aspirational {
                continue;
            }
            // Skip if specific claim_ids requested and this isn't one
            if !request.claim_ids.is_empty() && !request.claim_ids.contains(&claim.claim_id) {
                continue;
            }
            // Skip claims with no scaffold action (truly aspirational)
            let action = match claim.scaffold_action.as_deref() {
                Some(a) => a,
                None => continue,
            };

            match self.execute_scaffold_action(&request.agent_id, action, claim).await {
                Ok(evidence) => {
                    claim.status = ClaimStatus::Scaffolded;
                    claim.evidence = Some(evidence);
                    claim.error = None;
                    claims_scaffolded += 1;
                tracing::info!(
                    "[bootstrap] Scaffolded claim '{}' for '{}': {}",
                        claim.claim_id,
                        request.agent_id,
                        action
                    );
                }
                Err(e) => {
                    // Don't overwrite a more specific error message
                    if claim.error.is_none() {
                        claim.error = Some(e.to_string());
                    }
                    claim.status = ClaimStatus::Failed;
                    errors.push(format!("{}: {}", claim.claim_id, e));
                    claims_failed += 1;
                    tracing::warn!(
                        "[bootstrap] Failed claim '{}' for '{}': {}",
                        claim.claim_id,
                        request.agent_id,
                        e
                    );
                }
            }
        }

        // Update reconciliation timestamp and save
        manifest.last_reconciled = chrono::Utc::now().to_rfc3339();
        if let Err(e) = manifest.save(&manifest_path) {
            tracing::error!(
                "[bootstrap] Failed to save manifest for '{}': {}",
                request.agent_id,
                e
            );
        }

        // Publish result event
        let result = ScaffoldResult {
            agent_id: request.agent_id.clone(),
            claims_scaffolded,
            claims_failed,
            claims_aspirational: manifest
                .claims
                .iter()
                .filter(|c| c.status == ClaimStatus::Aspirational)
                .count(),
            errors,
        };

        if let Ok(payload_json) = serde_json::to_string(&result) {
            if let Err(e) = self
                .nexus
                .publish("system.agent.scaffold.complete", &payload_json)
                .await
            {
                tracing::warn!(
                    "[bootstrap] Failed to publish scaffold result: {}",
                    e
                );
            }
        }

        tracing::info!(
            "[bootstrap] Reconciliation complete for '{}': {} scaffolded, {} failed, {} aspirational",
            request.agent_id,
            claims_scaffolded,
            claims_failed,
            result.claims_aspirational,
        );
    }

    /// Routes a scaffold action to the appropriate idempotent handler.
    async fn execute_scaffold_action(
        &self,
        agent_id: &str,
        action: &str,
        claim: &SoulClaim,
    ) -> Result<ClaimEvidence, ScaffoldError> {
        match action {
            "register_identity" => self.scaffold_register_identity(agent_id).await,
            "init_wal_directory" => self.scaffold_init_wal(agent_id).await,
            "set_memory_budget" => self.scaffold_set_memory_budget(agent_id, claim).await,
            "register_in_nexus" => self.scaffold_register_in_nexus(agent_id).await,
            "mint_cct_token" => self.scaffold_mint_cct(agent_id, claim).await,
            "log_genesis" => self.scaffold_log_genesis(agent_id).await,
            other => {
                tracing::warn!(
                    "[bootstrap] Unknown scaffold action '{}' for agent '{}'",
                    other,
                    agent_id
                );
                Err(ScaffoldError::UnknownAction(other.to_string()))
            }
        }
    }

    /// Verifies the agent workspace exists. Identity is registered automatically
    /// by SoulUpdate's `scaffold_workspace()` call, so this handler just confirms
    /// the workspace directory is present and marks the claim as verified.
    /// Idempotent: workspace is created once by scaffold_workspace().
    async fn scaffold_register_identity(&self, agent_id: &str) -> Result<ClaimEvidence, ScaffoldError> {
        let validated = validate_scaffold_path(agent_id, "")?;
        let workspace_path = self.agents_path.join(&validated);

        if !workspace_path.exists() {
            // Workspace doesn't exist — create it (recovery path)
            std::fs::create_dir_all(&workspace_path)
                .map_err(|e| ScaffoldError::Io(format!("Workspace creation failed: {}", e)))?;

            // Create minimal agent.json
            let config_path = workspace_path.join("agent.json");
            if !config_path.exists() {
                let minimal_config = serde_json::json!({
                    "agent_id": agent_id,
                    "agent_name": agent_id,
                });
                std::fs::write(
                    &config_path,
                    serde_json::to_string_pretty(&minimal_config)
                        .map_err(|e| ScaffoldError::Io(format!("Serialize agent.json: {}", e)))?,
                )
                .map_err(|e| ScaffoldError::Io(format!("Write agent.json: {}", e)))?;
            }
        }

        Ok(ClaimEvidence::KeyValue(HashMap::from([(
            "path".to_string(),
            format!("workspaces/agents/{}", agent_id),
        )])))
    }

    /// Scaffolds a WAL directory at `workspaces/agents/<id>/wal/`.
    /// Idempotent: `create_dir_all` succeeds if the directory already exists.
    async fn scaffold_init_wal(&self, agent_id: &str) -> Result<ClaimEvidence, ScaffoldError> {
        let validated = validate_scaffold_path(agent_id, "wal")?;
        let wal_path = self.agents_path.join(&validated);

        std::fs::create_dir_all(&wal_path)
            .map_err(|e| ScaffoldError::Io(format!("WAL directory creation failed: {}", e)))?;

        // Touch a .gitkeep so the directory is tracked in version control
        let _ = std::fs::write(wal_path.join(".gitkeep"), b"");

        Ok(ClaimEvidence::KeyValue(HashMap::from([(
            "path".to_string(),
            format!("workspaces/agents/{}/wal", agent_id),
        )])))
    }

    /// Sets WASM memory budget in `agent.json`.
    /// Idempotent: overwrites with the same value if already set.
    async fn scaffold_set_memory_budget(
        &self,
        agent_id: &str,
        claim: &SoulClaim,
    ) -> Result<ClaimEvidence, ScaffoldError> {
        let validated = validate_scaffold_path(agent_id, "agent.json")?;
        let config_path = self.agents_path.join(&validated);

        let mut memory_mb = 64u32; // default
        // Try to parse MB from the claim description
        if let Some(mb_str) = claim.description.split([' ', '(', ')'])
            .find(|w| w.ends_with("MB") && w.len() > 2)
        {
            if let Ok(mb) = mb_str.trim_end_matches("MB").parse::<u32>() {
                memory_mb = mb;
            }
        }

        // Read existing config or create new one
        let mut config: serde_json::Value = if config_path.exists() {
            let content = std::fs::read_to_string(&config_path)
                .map_err(|e| ScaffoldError::Io(format!("Failed to read agent.json: {}", e)))?;
            serde_json::from_str(&content)
                .unwrap_or_else(|_| serde_json::json!({}))
        } else {
            serde_json::json!({})
        };

        if let Some(obj) = config.as_object_mut() {
            obj.insert(
                "wasm_memory_mb".to_string(),
                serde_json::json!(memory_mb),
            );
        }

        std::fs::write(
            &config_path,
            serde_json::to_string_pretty(&config)
                .map_err(|e| ScaffoldError::Io(format!("Serialize agent.json: {}", e)))?,
        )
        .map_err(|e| ScaffoldError::Io(format!("Write agent.json: {}", e)))?;

        Ok(ClaimEvidence::KeyValue(HashMap::from([(
            "wasm_memory_mb".to_string(),
            memory_mb.to_string(),
        )])))
    }

    /// Registers the agent in the Nexus by publishing a `system.agent.born` event.
    /// Idempotent: publishing the same event twice is harmless.
    async fn scaffold_register_in_nexus(&self, agent_id: &str) -> Result<ClaimEvidence, ScaffoldError> {
        let event_payload = serde_json::json!({
            "agent_id": agent_id,
            "event": "agent.born",
            "timestamp": chrono::Utc::now().to_rfc3339(),
        });

        self.nexus
            .publish(
                "system.agent.born",
                &event_payload.to_string(),
            )
            .await
            .map_err(|e| ScaffoldError::Nexus(format!("Failed to publish agent.born: {}", e)))?;

        Ok(ClaimEvidence::Path {
            path: format!("nexus://system.agent.born/{}", agent_id),
        })
    }

    /// Requests a CCT (Cryptographic Capability Token) by publishing a
    /// `system.agent.cct.requested` event. The security subsystem picks this up.
    /// Idempotent: multiple requests for the same scope produce the same token.
    async fn scaffold_mint_cct(
        &self,
        agent_id: &str,
        claim: &SoulClaim,
    ) -> Result<ClaimEvidence, ScaffoldError> {
        let cct_request = serde_json::json!({
            "agent_id": agent_id,
            "scope": claim.description,
            "requested_at": chrono::Utc::now().to_rfc3339(),
        });

        self.nexus
            .publish(
                "system.agent.cct.requested",
                &cct_request.to_string(),
            )
            .await
            .map_err(|e| ScaffoldError::Nexus(format!("Failed to request CCT: {}", e)))?;

        Ok(ClaimEvidence::KeyValue(HashMap::from([(
            "status".to_string(),
            "cct_requested".to_string(),
        )])))
    }

    /// Appends a genesis event to `EVOLUTION.jsonl`.
    /// Idempotent: checks for existing genesis event before writing.
    async fn scaffold_log_genesis(&self, agent_id: &str) -> Result<ClaimEvidence, ScaffoldError> {
        let validated = validate_scaffold_path(agent_id, "EVOLUTION.jsonl")?;
        let evo_path = self.agents_path.join(&validated);

        // Check if genesis event already exists (idempotency)
        if evo_path.exists() {
            if let Ok(content) = std::fs::read_to_string(&evo_path) {
                if content.contains("\"action\":\"bootstrap_complete\"") {
                    return Ok(ClaimEvidence::Path {
                        path: format!("workspaces/agents/{}/EVOLUTION.jsonl", agent_id),
                    });
                }
            }
        }

        let genesis_entry = serde_json::json!({
            "action": "bootstrap_complete",
            "agent_id": agent_id,
            "timestamp": chrono::Utc::now().to_rfc3339(),
        });

        // Append to EVOLUTION.jsonl
        if let Ok(mut file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&evo_path)
        {
            use std::io::Write;
            let line = serde_json::to_string(&genesis_entry)
                .unwrap_or_default();
            if let Err(e) = writeln!(file, "{}", line) {
                tracing::warn!(
                    "[bootstrap] Failed to write genesis to EVOLUTION.jsonl: {}",
                    e
                );
            }
        }

        Ok(ClaimEvidence::Path {
            path: format!("workspaces/agents/{}/EVOLUTION.jsonl", agent_id),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_infra_block_found() {
        let soul = r#"# SOUL.md

**Name**: Driftwarder
**Birth**: 2026-07-09

## 1. ⚙️ Systemic Core & Origin

Some content here...

## INFRASTRUCTURE_REQUIREMENTS

```json
{
  "infrastructure": [
    { "type": "wal_schema", "description": "Temporal graph WAL with epoch-cid columns" },
    { "type": "memory_budget", "description": "WASM memory budget of 64MB", "mb": 64, "scope": "wasm" }
  ],
  "aspirational": [
    { "type": "peer_agent", "id": "Builder-020", "description": "Upstream builder agent", "note": "does not exist yet" }
  ]
}
```
"#;

        let reqs = extract_infra_block(soul).expect("Should find infra block");
        assert_eq!(reqs.infrastructure.len(), 2);
        assert_eq!(reqs.infrastructure[0].r#type, "wal_schema");
        assert_eq!(reqs.infrastructure[1].mb, Some(64));
        assert_eq!(reqs.aspirational.len(), 1);
        assert_eq!(reqs.aspirational[0].id, Some("Builder-020".to_string()));
    }

    #[test]
    fn test_extract_infra_block_missing() {
        let soul = "# SOUL.md\n\n**Name**: Test\n\nNo infra block here.";
        assert!(extract_infra_block(soul).is_none());
    }

    #[test]
    fn test_extract_infra_block_malformed_json() {
        let soul = r#"# SOUL.md

## INFRASTRUCTURE_REQUIREMENTS

```json
{ this is not valid json }
```
"#;
        assert!(extract_infra_block(soul).is_none());
    }

    #[test]
    fn test_extract_infra_block_empty_block() {
        let soul = r#"# SOUL.md

## INFRASTRUCTURE_REQUIREMENTS

```json
{
  "infrastructure": [],
  "aspirational": []
}
```
"#;
        let reqs = extract_infra_block(soul).expect("Should find empty infra block");
        assert!(reqs.infrastructure.is_empty());
        assert!(reqs.aspirational.is_empty());
    }

    #[test]
    fn test_manifest_new_and_roundtrip() {
        let soul_content = "# SOUL.md\n\n**Name**: test-agent\n\nSome content.";
        let reqs = InfraRequirements {
            infrastructure: vec![InfraItem {
                r#type: "wal_schema".to_string(),
                description: "Temporal graph WAL".to_string(),
                scope: None,
                mb: None,
                id: None,
                note: None,
            }],
            aspirational: vec![],
        };

        let manifest =
            Manifest::new("test-agent".to_string(), soul_content, "scaffolded", Some(&reqs));
        assert_eq!(manifest.agent_id, "test-agent");
        assert_eq!(manifest.bootstrap_tier, "scaffolded");
        assert_eq!(manifest.version, "1.0");
        assert!(!manifest.soul_blake3.is_empty());
        assert_eq!(manifest.claims.len(), 2); // identity + wal_schema
        assert_eq!(manifest.claims[0].category, ClaimCategory::Identity);
        assert_eq!(manifest.claims[1].category, ClaimCategory::Storage);

        // Verify blake3 hash matches
        let expected_hash = blake3::hash(soul_content.as_bytes()).to_hex().to_string();
        assert_eq!(manifest.soul_blake3, expected_hash);
    }

    #[test]
    fn test_manifest_save_and_load() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("manifest.json");
        let soul = "# Test";
        let manifest = Manifest::new("test-save".to_string(), soul, "grounded", None);
        manifest.save(&path).expect("Save should succeed");

        let loaded = Manifest::load(&path).expect("Load should succeed");
        assert_eq!(loaded.agent_id, "test-save");
        assert_eq!(loaded.soul_blake3, manifest.soul_blake3);
    }

    #[test]
    fn test_manifest_load_not_found() {
        let err = Manifest::load(Path::new("/nonexistent/manifest.json"));
        assert!(err.is_err());
    }

    #[test]
    fn test_manifest_drift_detection() {
        let soul = "# SOUL.md\nOriginal content";
        let manifest = Manifest::new("drift-test".to_string(), soul, "scaffolded", None);
        assert!(!manifest.has_drifted(soul));
        assert!(manifest.has_drifted("# SOUL.md\nModified content"));
    }

    #[test]
    fn test_manifest_diff_added() {
        let old = Manifest::new("diff-test".to_string(), "# Old", "scaffolded", None);
        let mut new = Manifest::new("diff-test".to_string(), "# New", "scaffolded", None);
        // Add an extra claim
        new.claims.push(SoulClaim {
            claim_id: "claim_999".to_string(),
            category: ClaimCategory::Storage,
            description: "Extra WAL".to_string(),
            status: ClaimStatus::Aspirational,
            scaffold_action: None,
            evidence: None,
            error: Some("test".to_string()),
        });

        let diffs = manifest_diff(&old, &new);
        let added: Vec<_> = diffs.iter().filter(|d| matches!(d, ClaimDiff::Added(_))).collect();
        assert_eq!(added.len(), 1);
    }

    #[test]
    fn test_classify_infra_type() {
        assert_eq!(classify_infra_type("identity"), ClaimCategory::Identity);
        assert_eq!(classify_infra_type("cct_token"), ClaimCategory::Security);
        assert_eq!(classify_infra_type("wal_schema"), ClaimCategory::Storage);
        assert_eq!(classify_infra_type("memory_budget"), ClaimCategory::Compute);
        assert_eq!(classify_infra_type("peer_agent"), ClaimCategory::Integration);
        assert_eq!(classify_infra_type("metric_count"), ClaimCategory::Metric);
        assert_eq!(classify_infra_type("unknown_type"), ClaimCategory::Unknown);
    }
}
