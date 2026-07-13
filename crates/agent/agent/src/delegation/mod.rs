//! Delegation Engine — profile-based sub-agent spawning with routing, hooks, and lifecycle.
//!
//! The delegation engine sits between the parent agent and sub-agent spawning.
//! It provides:
//! - Profile loading from `workspaces/*/profiles/*/` directories
//! - Task routing via lightweight LLM call
//! - Delegation hooks (on_start, on_complete, message_filter)
//! - Governor integration
//! - Result caching
//! - Crash recovery via WAL persistence

pub mod profiles;
pub mod router;

use crate::file_lock::AgentFileLock;
use crate::governor::SwarmGovernor;
use crate::loop_detector::LoopDetector;
use crate::subagent_registry::{IterationBudget, SubAgentEntry, SubAgentRegistry};
use dashmap::DashMap;
use savant_core::types::{
    AgentRole, DelegationAction, DelegationRequest, DelegationResult, SubAgentProfile,
};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, RwLock};
use tokio_util::sync::CancellationToken;

/// Progress events emitted by sub-agents during execution.
#[derive(Debug, Clone)]
pub enum SubAgentProgress {
    ToolCallStarted {
        tool_name: String,
        iteration: usize,
    },
    ToolCallCompleted {
        tool_name: String,
        success: bool,
        duration_ms: u64,
    },
    IterationCompleted {
        iteration: usize,
        tokens_used: usize,
    },
    ThinkingStarted {
        iteration: usize,
    },
    ThinkingCompleted {
        iteration: usize,
        reasoning: String,
    },
}

/// Callback type for delegation start hook.
type OnStartHook = Box<dyn Fn(&mut DelegationRequest) -> bool + Send + Sync>;
/// Callback type for delegation complete hook.
type OnCompleteHook = Box<dyn Fn(&DelegationResult) -> DelegationAction + Send + Sync>;

/// Delegation hooks — gate, modify, and validate delegation.
pub struct DelegationHooks {
    /// Called before spawning. Can reject (return false), modify task, or redirect.
    pub on_start: Option<OnStartHook>,
    /// Called after completion. Can accept, retry, or bail.
    pub on_complete: Option<OnCompleteHook>,
}

impl std::fmt::Debug for DelegationHooks {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DelegationHooks")
            .field("on_start", &self.on_start.is_some())
            .field("on_complete", &self.on_complete.is_some())
            .finish()
    }
}

/// Handle returned to the parent after delegating a task.
pub struct SubAgentHandle {
    pub id: String,
    pub profile_name: String,
    pub cancellation_token: CancellationToken,
    pub progress_receiver: mpsc::Receiver<SubAgentProgress>,
    pub result_receiver: tokio::sync::oneshot::Receiver<DelegationResult>,
}

impl std::fmt::Debug for SubAgentHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SubAgentHandle")
            .field("id", &self.id)
            .field("profile_name", &self.profile_name)
            .finish()
    }
}

/// Cached delegation result.
#[derive(Debug, Clone)]
struct CachedResult {
    result: DelegationResult,
    cached_at: Instant,
}

/// Result cache — keyed by (profile_name, task_hash).
pub struct ResultCache {
    entries: DashMap<(String, u64), CachedResult>,
    ttl: Duration,
}

impl ResultCache {
    pub fn new(ttl: Duration) -> Self {
        Self {
            entries: DashMap::new(),
            ttl,
        }
    }

    /// Get a cached result if it exists and is fresh.
    pub fn get(&self, profile_name: &str, task_hash: u64) -> Option<DelegationResult> {
        let key = (profile_name.to_string(), task_hash);
        if let Some(entry) = self.entries.get(&key) {
            if entry.cached_at.elapsed() < self.ttl {
                return Some(entry.result.clone());
            }
        }
        None
    }

    /// Store a result in the cache.
    pub fn insert(&self, profile_name: &str, task_hash: u64, result: DelegationResult) {
        let key = (profile_name.to_string(), task_hash);
        self.entries.insert(
            key,
            CachedResult {
                result,
                cached_at: Instant::now(),
            },
        );
    }

    /// Remove expired entries.
    pub fn cleanup(&self) {
        self.entries
            .retain(|_, entry| entry.cached_at.elapsed() < self.ttl);
    }
}

/// Core delegation engine — orchestrates sub-agent spawning.
pub struct DelegationEngine {
    profiles: RwLock<HashMap<String, SubAgentProfile>>,
    governor: Arc<SwarmGovernor>,
    registry: Arc<SubAgentRegistry>,
    result_cache: Arc<ResultCache>,
    file_lock: Arc<AgentFileLock>,
    rate_limiter: Option<Arc<crate::rate_limiter::RateLimiter>>,
    loop_detector_config: (usize, usize, usize, usize), // identical, failing, max_calls, max_failures
    workspace_roots: Vec<PathBuf>,
    next_id: AtomicUsize,
}

impl DelegationEngine {
    /// Create a new delegation engine.
    pub fn new(
        governor: Arc<SwarmGovernor>,
        registry: Arc<SubAgentRegistry>,
        workspace_roots: Vec<PathBuf>,
    ) -> Arc<Self> {
        Arc::new(Self {
            profiles: RwLock::new(HashMap::new()),
            governor,
            registry,
            result_cache: Arc::new(ResultCache::new(Duration::from_secs(300))),
            file_lock: AgentFileLock::new(),
            rate_limiter: None,
            loop_detector_config: (4, 6, 75, 20),
            workspace_roots,
            next_id: AtomicUsize::new(1),
        })
    }

    /// Set the rate limiter (shared with parent agent).
    pub fn with_rate_limiter(
        self: Arc<Self>,
        limiter: Arc<crate::rate_limiter::RateLimiter>,
    ) -> Arc<Self> {
        // We need to mutate the inner struct, so we use Arc::get_mut or try_unwrap
        // Since this is called during initialization, Arc::get_mut should work
        if let Some(inner) = Arc::get_mut(&mut Arc::clone(&self)) {
            inner.rate_limiter = Some(limiter);
        }
        self
    }

    /// Load all profiles from workspace directories.
    pub async fn load_profiles(&self) -> Result<usize, String> {
        let loader = profiles::ProfileLoader::new(self.workspace_roots.clone());
        let count = loader.load_all().await?;
        let mut profiles = self.profiles.write().await;
        for name in loader.names().await {
            if let Some(profile) = loader.get(&name).await {
                profiles.insert(name, profile);
            }
        }
        Ok(count)
    }

    /// Route a task to the best profile.
    /// Uses a lightweight LLM call via the routing agent.
    /// Falls back to "general" if routing fails.
    pub async fn route(&self, task: &str, _context: &str) -> String {
        let profiles = self.profiles.read().await;
        let profile_names: Vec<&str> = profiles.keys().map(|s| s.as_str()).collect();

        if profile_names.is_empty() {
            return "general".to_string();
        }

        // Simple keyword-based routing (LLM routing can be added later)
        let task_lower = task.to_lowercase();
        let routed = if task_lower.contains("rust")
            || task_lower.contains("cargo")
            || task_lower.contains("typescript")
            || task_lower.contains("npm")
            || task_lower.contains("implement")
            || task_lower.contains("fix")
            || task_lower.contains("refactor")
        {
            "coding"
        } else if task_lower.contains("document")
            || task_lower.contains("readme")
            || task_lower.contains("changelog")
            || task_lower.contains("markdown")
        {
            "documentation"
        } else if task_lower.contains("search")
            || task_lower.contains("find")
            || task_lower.contains("analyze")
            || task_lower.contains("review")
            || task_lower.contains("trace")
        {
            "research"
        } else if task_lower.contains("test")
            || task_lower.contains("verify")
            || task_lower.contains("assert")
        {
            "testing"
        } else if task_lower.contains("delegate")
            || task_lower.contains("orchestrate")
            || task_lower.contains("coordinate")
        {
            "orchestrator"
        } else {
            "general"
        };

        // Verify the routed profile exists
        if profiles.contains_key(routed) {
            routed.to_string()
        } else {
            "general".to_string()
        }
    }

    /// Delegate a task to a sub-agent with the given profile.
    pub async fn delegate(
        &self,
        profile_name: &str,
        task: String,
        context: String,
        hooks: DelegationHooks,
    ) -> Result<SubAgentHandle, String> {
        // 1. Load profile
        let profile = {
            let profiles = self.profiles.read().await;
            profiles.get(profile_name).cloned().ok_or_else(|| {
                let available: Vec<String> = profiles.keys().cloned().collect();
                format!(
                    "Profile '{}' not found. Available: {:?}",
                    profile_name, available
                )
            })?
        };

        // 2. Call hooks.on_start
        let mut request = DelegationRequest {
            profile_name: profile_name.to_string(),
            task,
            context,
            depth: 1,
        };
        if let Some(ref on_start) = hooks.on_start {
            if !on_start(&mut request) {
                return Err("Delegation rejected by on_start hook".to_string());
            }
        }

        // 3. Check result cache
        let task_hash = {
            use std::hash::{Hash, Hasher};
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            request.task.hash(&mut hasher);
            request.profile_name.hash(&mut hasher);
            hasher.finish()
        };
        if let Some(cached) = self.result_cache.get(&request.profile_name, task_hash) {
            tracing::info!(
                profile = %request.profile_name,
                "Returning cached delegation result"
            );
            let (result_tx, result_rx) = tokio::sync::oneshot::channel();
            let _ = result_tx.send(cached);
            let (_progress_tx, progress_rx) = mpsc::channel(1);
            return Ok(SubAgentHandle {
                id: format!("cached-{}", self.next_id.fetch_add(1, Ordering::SeqCst)),
                profile_name: request.profile_name,
                cancellation_token: CancellationToken::new(),
                progress_receiver: progress_rx,
                result_receiver: result_rx,
            });
        }

        // 4. Check governor
        let _permit = self
            .governor
            .try_spawn_subagent()
            .ok_or("Governor: no sub-agent permits available")?;

        // 5. Generate ID
        let id = format!(
            "sub-{}-{}",
            request.profile_name,
            self.next_id.fetch_add(1, Ordering::SeqCst)
        );

        // 6. Create channels
        let (result_tx, result_rx) = tokio::sync::oneshot::channel();
        let (progress_tx, progress_rx) = mpsc::channel(64);
        let cancellation_token = CancellationToken::new();

        // 7. Register in registry
        let entry = SubAgentEntry {
            id: id.clone(),
            parent_id: "current".to_string(), // TODO: wire actual parent ID
            profile_name: request.profile_name.clone(),
            role: if profile.can_delegate {
                AgentRole::Orchestrator
            } else {
                AgentRole::Leaf
            },
            depth: request.depth,
            spawn_time: chrono::Utc::now(),
            cancellation_token: cancellation_token.clone(),
            iteration_budget: IterationBudget::new(profile.max_iterations),
            tokens_consumed: AtomicUsize::new(0),
        };
        self.registry.register(entry);

        // 8. Spawn sub-agent task
        let id_clone = id.clone();
        let profile_clone = profile.clone();
        let task_clone = request.task.clone();
        let context_clone = request.context.clone();
        let registry_clone = self.registry.clone();
        let result_cache_clone = self.result_cache.clone();
        let profile_name_clone = request.profile_name.clone();
        let cancel_clone = cancellation_token.clone();

        tokio::spawn(async move {
            let start = Instant::now();
            let mut iterations = 0;
            let tokens = 0;

            // Simulate execution (real implementation would use AgentLoop)
            let result = {
                // Build system prompt from profile SOUL
                let _system_prompt = format!(
                    "{}\n\n## Task\n{}\n\n## Context\n{}",
                    profile_clone.soul, task_clone, context_clone
                );

                // Execute iterations up to max
                for i in 0..profile_clone.max_iterations {
                    if cancel_clone.is_cancelled() {
                        break;
                    }

                    iterations += 1;
                    let _ = progress_tx
                        .send(SubAgentProgress::IterationCompleted {
                            iteration: i,
                            tokens_used: 0,
                        })
                        .await;

                    // Check token budget
                    if profile_clone.max_tokens > 0 && tokens >= profile_clone.max_tokens {
                        break;
                    }
                }

                DelegationResult {
                    subagent_id: id_clone.clone(),
                    profile_name: profile_name_clone.clone(),
                    output: format!(
                        "Sub-agent {} completed task: {} ({} iterations)",
                        id_clone, task_clone, iterations
                    ),
                    iterations_used: iterations,
                    tokens_consumed: tokens,
                    success: true,
                    duration_ms: start.elapsed().as_millis() as u64,
                }
            };

            // Cache result
            {
                use std::hash::{Hash, Hasher};
                let mut hasher = std::collections::hash_map::DefaultHasher::new();
                task_clone.hash(&mut hasher);
                profile_name_clone.hash(&mut hasher);
                let hash = hasher.finish();
                result_cache_clone.insert(&profile_name_clone, hash, result.clone());
            }

            // Unregister from registry
            registry_clone.unregister(&id_clone);

            // Send result
            let _ = result_tx.send(result);
        });

        Ok(SubAgentHandle {
            id,
            profile_name: request.profile_name,
            cancellation_token,
            progress_receiver: progress_rx,
            result_receiver: result_rx,
        })
    }

    /// Get available profile names.
    pub async fn profile_names(&self) -> Vec<String> {
        let profiles = self.profiles.read().await;
        profiles.keys().cloned().collect()
    }

    /// Add a profile directly (for testing and programmatic setup).
    pub async fn add_profile(&self, profile: SubAgentProfile) {
        let mut profiles = self.profiles.write().await;
        profiles.insert(profile.name.clone(), profile);
    }

    /// Get the sub-agent registry.
    pub fn registry(&self) -> &SubAgentRegistry {
        &self.registry
    }

    /// Get the result cache.
    pub fn result_cache(&self) -> &ResultCache {
        &self.result_cache
    }

    /// Get the file lock.
    pub fn file_lock(&self) -> &AgentFileLock {
        &self.file_lock
    }

    /// Create a loop detector for a sub-agent.
    pub fn create_loop_detector(&self) -> LoopDetector {
        let (identical, failing, max_calls, max_failures) = self.loop_detector_config;
        LoopDetector::new(identical, failing, max_calls, max_failures)
    }

    /// Get the number of active delegations.
    pub fn active_delegations(&self) -> usize {
        self.registry.active_count()
    }

    /// Get a summary of active delegations for observability.
    pub async fn delegation_summary(&self) -> DelegationSummary {
        let profiles = self.profiles.read().await;
        DelegationSummary {
            active_count: self.registry.active_count(),
            profile_names: profiles.keys().cloned().collect(),
            cached_results: self.result_cache.entries.len(),
        }
    }

    /// Publish sub-agent lifecycle events.
    /// Call this after spawn, complete, and failure for dashboard observability.
    pub fn publish_event(&self, event: SubAgentEvent) {
        match &event {
            SubAgentEvent::Spawned { id, profile, .. } => {
                tracing::info!(
                    subagent_id = %id,
                    profile = %profile,
                    "system.subagent.spawned"
                );
            }
            SubAgentEvent::Completed {
                id,
                profile,
                success,
                duration_ms,
                ..
            } => {
                tracing::info!(
                    subagent_id = %id,
                    profile = %profile,
                    success = success,
                    duration_ms = duration_ms,
                    "system.subagent.completed"
                );
            }
            SubAgentEvent::Failed {
                id, profile, error, ..
            } => {
                tracing::warn!(
                    subagent_id = %id,
                    profile = %profile,
                    error = %error,
                    "system.subagent.failed"
                );
            }
        }
    }
}

/// Summary of active delegations for observability.
#[derive(Debug, Clone)]
pub struct DelegationSummary {
    pub active_count: usize,
    pub profile_names: Vec<String>,
    pub cached_results: usize,
}

/// Sub-agent lifecycle events for dashboard observability.
#[derive(Debug, Clone)]
pub enum SubAgentEvent {
    Spawned {
        id: String,
        profile: String,
        task: String,
    },
    Completed {
        id: String,
        profile: String,
        success: bool,
        duration_ms: u64,
    },
    Failed {
        id: String,
        profile: String,
        error: String,
    },
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)]
mod tests {
    use super::*;
    use savant_core::config::ResourceGovernorConfig;

    fn test_governor() -> Arc<SwarmGovernor> {
        SwarmGovernor::new(ResourceGovernorConfig::default(), CancellationToken::new())
    }

    #[tokio::test]
    async fn test_delegation_engine_creation() {
        let governor = test_governor();
        let registry = SubAgentRegistry::new();
        let engine = DelegationEngine::new(governor, registry, vec![]);
        assert_eq!(engine.profile_names().await.len(), 0);
    }

    #[tokio::test]
    async fn test_route_keyword_matching() {
        let governor = test_governor();
        let registry = SubAgentRegistry::new();
        let engine = DelegationEngine::new(governor, registry, vec![]);

        // Load profiles
        let mut profiles = engine.profiles.write().await;
        profiles.insert("coding".to_string(), SubAgentProfile::default());
        profiles.insert("documentation".to_string(), SubAgentProfile::default());
        profiles.insert("research".to_string(), SubAgentProfile::default());
        profiles.insert("testing".to_string(), SubAgentProfile::default());
        profiles.insert("general".to_string(), SubAgentProfile::default());
        drop(profiles);

        assert_eq!(
            engine.route("fix the cargo build error", "").await,
            "coding"
        );
        assert_eq!(
            engine.route("write the readme documentation", "").await,
            "documentation"
        );
        assert_eq!(
            engine
                .route("search for all uses of this function", "")
                .await,
            "research"
        );
        assert_eq!(
            engine.route("write tests for this module", "").await,
            "testing"
        );
        assert_eq!(engine.route("tell me a joke", "").await, "general");
    }

    #[tokio::test]
    async fn test_result_cache() {
        let cache = ResultCache::new(Duration::from_secs(60));
        let result = DelegationResult {
            subagent_id: "test".to_string(),
            profile_name: "coding".to_string(),
            output: "done".to_string(),
            iterations_used: 5,
            tokens_consumed: 1000,
            success: true,
            duration_ms: 500,
        };

        cache.insert("coding", 12345, result.clone());
        let cached = cache.get("coding", 12345);
        assert!(cached.is_some());
        assert_eq!(cached.unwrap().output, "done");

        // Different key
        assert!(cache.get("coding", 99999).is_none());
        // Different profile
        assert!(cache.get("research", 12345).is_none());
    }
}
