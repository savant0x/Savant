// SAFETY: All `clippy::disallowed_methods` violations in this file originate from
// the `serde_json::json!()` macro, which internally uses `.unwrap()` on
// compile-time-validated JSON literals. A malformed JSON literal would be a
// compile error, making the panic path statically unreachable.
#![allow(clippy::disallowed_methods)]

//
//
use crate::manager::AgentManager;
use crate::providers::mgmt::OpenRouterMgmt;
use crate::providers::{
    AnthropicProvider, AzureProvider, CohereProvider, DeepseekProvider, FireworksProvider,
    GoogleProvider, GroqProvider, MistralProvider, NovitaProvider, OllamaProvider, OpenAiProvider,
    OpenRouterProvider, TogetherProvider, XaiProvider,
};
use crate::pulse::HeartbeatPulse;
use crate::react::AgentLoop;
use pqcrypto_dilithium::dilithium2;
use reqwest::Client;
use savant_core::bus::NexusBridge;
use savant_core::db::Storage;
use savant_core::error::SavantError;
use savant_core::traits::{EmbeddingProvider, LlmProvider, MemoryBackend, Tool, VisionProvider};
use savant_core::types::{AgentConfig, ModelProvider};
use savant_core::utils::ollama_embeddings::create_embedding_service;
use savant_core::utils::ollama_vision::create_vision_service;
use savant_core::utils::parsing;
use savant_ipc::{CollectiveBlackboard, SwarmBlackboard};
use savant_memory::{AsyncMemoryBackend, MemoryEngine};
#[cfg(kani)]
use savant_security::{SecurityAuthority, SecurityError};
use std::collections::HashMap;
use std::sync::Arc;

use crate::plugins::WasmToolHost;
use dashmap::DashMap;
use savant_echo::{ComponentMetrics, EchoCompiler, HotSwappableRegistry};
use std::sync::atomic::{AtomicU8, Ordering};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

const WORKSPACE_ROOT_DEFAULT: &str = "./workspaces";
const MEMORY_DB_PATH_DEFAULT: &str = "./data/memory";
const SKILLS_PATH_DEFAULT: &str = "./skills";

/// Configuration for the Swarm Controller.
#[derive(Debug, Clone)]
pub struct SwarmConfig {
    pub workspace_root: std::path::PathBuf,
    pub memory_db_path: std::path::PathBuf,
    pub skills_path: std::path::PathBuf,
    pub blackboard_name: String,
    pub collective_name: String,
    pub config_file: Option<std::path::PathBuf>,
    pub privacy: savant_core::config::PrivacyConfig,
    pub trajectory: savant_core::config::TrajectoryConfig,
    pub embedding_model: String,
}

impl Default for SwarmConfig {
    fn default() -> Self {
        Self {
            workspace_root: std::path::PathBuf::from(WORKSPACE_ROOT_DEFAULT),
            memory_db_path: std::path::PathBuf::from(MEMORY_DB_PATH_DEFAULT),
            skills_path: std::path::PathBuf::from(SKILLS_PATH_DEFAULT),
            blackboard_name: "savant_swarm".to_string(),
            collective_name: "savant_collective".to_string(),
            config_file: None,
            privacy: savant_core::config::PrivacyConfig::default(),
            trajectory: savant_core::config::TrajectoryConfig::default(),
            embedding_model: "nomic-embed-text".to_string(),
        }
    }
}

/// The Swarm Controller: Orchestrates autonomous agents.
pub struct SwarmController {
    config: SwarmConfig,
    nexus: Arc<NexusBridge>,
    storage: Arc<Storage>,
    manager: Arc<AgentManager>,
    agents: Vec<AgentConfig>,
    client: Client,
    handles: DashMap<String, (JoinHandle<()>, CancellationToken)>,
    tools: Arc<HashMap<String, Arc<dyn Tool>>>,
    engine: Arc<MemoryEngine>,
    embedding_service: Arc<dyn EmbeddingProvider>,
    vision_service: Option<Arc<dyn VisionProvider>>,
    blackboard: Arc<SwarmBlackboard>,
    root_authority: ed25519_dalek::VerifyingKey,
    signing_key: ed25519_dalek::SigningKey,
    pqc_authority: dilithium2::PublicKey,
    pqc_signing_key: dilithium2::SecretKey,
    echo_registry: Arc<HotSwappableRegistry>,
    echo_compiler: Arc<EchoCompiler>,
    echo_metrics: Arc<ComponentMetrics>,
    echo_host: Arc<WasmToolHost>,
    collective_blackboard: Arc<CollectiveBlackboard>,
    agent_index_counter: AtomicU8,
    dead_agents: DashMap<String, ()>,
    /// MCP server endpoints to connect on agent spawn
    mcp_servers: Vec<savant_core::config::McpServerEntry>,
    /// Skill lifecycle manager — discovery, installation, approval, security gating
    skill_manager: Arc<tokio::sync::Mutex<savant_skills::parser::SkillManager>>,
    /// Integration sync scheduler shutdown signal
    integrations_shutdown_tx: tokio::sync::watch::Sender<bool>,
    /// Dynamic credential broker for per-task ephemeral token management.
    credential_broker: Arc<savant_security::continuous::credentials::CredentialBroker>,
    /// Graceful shutdown tracker — RAII-based in-flight request tracking.
    shutdown_tracker: Arc<crate::graceful_shutdown::GracefulShutdownTracker>,
    /// Shared CapabilityRegistry — one instance for all agents (not per-agent).
    shared_capability_registry: Arc<savant_ipc::CapabilityRegistry>,
    /// Delegation engine — profile-based sub-agent spawning.
    #[allow(dead_code)]
    delegation_engine: Arc<crate::delegation::DelegationEngine>,
    /// Sub-agent registry — lightweight tracking for active sub-agents.
    #[allow(dead_code)]
    subagent_registry: Arc<crate::subagent_registry::SubAgentRegistry>,
    /// Panopticon replay recorder for agent reasoning trace.
    replay_recorder: Arc<savant_panopticon::replay::ReplayRecorder>,
    /// Schema (code intelligence) index — shared across all agents.
    schema_index: Option<Arc<savant_schema::SchemaIndex>>,
    /// LSP manager — shared across all agents.
    lsp_manager: Option<Arc<crate::lsp::LspManager>>,
    /// Consciousness daemon state handle (shared with gateway for /api/consciousness/status).
    consciousness_state: Arc<AtomicU8>,
    /// CancellationToken for the consciousness daemon — cancelled during shutdown.
    consciousness_shutdown: CancellationToken,
    /// Resource governor — CPU/memory-aware agent spawning with adaptive concurrency.
    governor: Option<Arc<crate::governor::SwarmGovernor>>,
}

impl SwarmController {
    #[allow(clippy::too_many_arguments)]
    pub async fn new(
        config: SwarmConfig,
        agents: Vec<AgentConfig>,
        storage: Arc<Storage>,
        manager: Arc<AgentManager>,
        nexus: Arc<NexusBridge>,
        root_authority: ed25519_dalek::VerifyingKey,
        signing_key: ed25519_dalek::SigningKey,
        pqc_authority: dilithium2::PublicKey,
        pqc_signing_key: dilithium2::SecretKey,
        mcp_servers: Vec<savant_core::config::McpServerEntry>,
        replay_recorder: Arc<savant_panopticon::replay::ReplayRecorder>,
        integrations_config: savant_core::config::IntegrationsConfig,
        schema_index: Option<Arc<savant_schema::SchemaIndex>>,
        lsp_manager: Option<Arc<crate::lsp::LspManager>>,
    ) -> Result<Self, savant_core::error::SavantError> {
        // 1. Discover all available tools (skills) once for the swarm
        let skill_path = config.skills_path.clone();
        let mut registry = savant_skills::parser::SkillRegistry::new();

        if let Err(e) = registry.discover_skills(&skill_path).await {
            tracing::error!("Failed to discover skills: {}", e);
        }

        let tools = Arc::new(registry.tools);

        // 2. Initialize Embedding Service FIRST (required by memory engine)
        let embedding_service: Arc<dyn savant_core::traits::EmbeddingProvider> =
            create_embedding_service(Some(&config.embedding_model))
                .await
                .map_err(|e| {
                    savant_core::error::SavantError::Unknown(format!(
                        "Embedding service is required: {}",
                        e
                    ))
                })
                .map(Arc::from)?;
        tracing::info!(
            "Embedding service initialized ({} dims)",
            embedding_service.dimensions()
        );

        // 2.5. Initialize Memory Engine (Fjall LSM + ruvector)
        let engine = MemoryEngine::with_defaults(&config.memory_db_path, embedding_service.clone())
            .map_err(|e| {
                savant_core::error::SavantError::Unknown(format!(
                    "Failed to init memory engine: {}",
                    e
                ))
            })?;

        // 2.5a. Initialize Compact Engine (L1 tool output compression)
        let compact_user_rules = config.workspace_root.join("config/compact-rules");
        let compact_project_rules = config.workspace_root.join(".savant/compact-rules");
        crate::compact::integration::init(compact_user_rules, compact_project_rules).await;

        // 2.6. Initialize Vision Service (Ollama Gemma)
        let vision_service = match create_vision_service().await {
            Some(svc) => {
                tracing::info!("Vision service initialized (Gemma)");
                Some(Arc::from(svc))
            }
            None => {
                tracing::warn!("Vision service unavailable. Image understanding disabled.");
                None
            }
        };

        // 3. Initialize Shared Blackboard (Zero-Copy IPC)
        let blackboard = Arc::new(SwarmBlackboard::new(&config.blackboard_name).map_err(|e| {
            savant_core::error::SavantError::Unknown(format!("Failed to init blackboard: {}", e))
        })?);

        // 4. Initialize ECHO Substrate
        let wasm_config = wasmtime::Config::new();
        let wasm_engine = wasmtime::Engine::new(&wasm_config).map_err(|e| {
            savant_core::error::SavantError::Unknown(format!("Failed to init wasm engine: {}", e))
        })?;
        let echo_registry = Arc::new(HotSwappableRegistry::new(wasm_engine));
        let echo_compiler = Arc::new(EchoCompiler::new(config.workspace_root.clone()));
        let echo_metrics = Arc::new(ComponentMetrics::new(0.05, 100));
        let echo_host = Arc::new(WasmToolHost::new().map_err(|e| {
            savant_core::error::SavantError::Unknown(format!(
                "Failed to init WASM tool host: {}",
                e
            ))
        })?);
        let collective_blackboard = Arc::new(
            CollectiveBlackboard::new(&config.collective_name).map_err(|e| {
                savant_core::error::SavantError::Unknown(format!(
                    "Failed to init collective blackboard: {}",
                    e
                ))
            })?,
        );

        // --- Solo Authority Fallback ---
        // If only one agent is present, set quorum to 1 to prevent deadlock.
        if agents.len() == 1 {
            tracing::info!("Solo agent detected. Setting collective quorum threshold to 1.");
            if let Err(e) = collective_blackboard.set_quorum_threshold(1) {
                tracing::warn!("Failed to set solo quorum threshold: {}", e);
            }
        }

        // Initialize integration provider registry and sync scheduler
        let provider_registry = Arc::new(savant_integrations::registry::ProviderRegistry::new());

        // Register configured providers (gated behind config — unconfigured providers skipped)
        if let Some(ref gmail_cfg) = integrations_config.gmail {
            use savant_integrations::provider::{ProviderConfig, ProviderKind};
            let mut settings = std::collections::HashMap::new();
            settings.insert("access_token".to_string(), gmail_cfg.access_token.clone());
            settings.insert(
                "max_messages".to_string(),
                gmail_cfg.max_messages.to_string(),
            );
            settings.insert(
                "label_filters".to_string(),
                serde_json::to_string(&gmail_cfg.label_filters).unwrap_or_default(),
            );
            let provider_config = ProviderConfig {
                kind: ProviderKind::Gmail,
                settings,
                enabled: true,
                sync_interval_secs: 3600,
            };
            let gmail_provider: Arc<dyn savant_integrations::Provider> =
                Arc::new(savant_integrations::GmailProvider::new(
                    provider_config.clone(),
                    savant_integrations::GmailConfig {
                        access_token: Some(gmail_cfg.access_token.clone()),
                        max_messages: gmail_cfg.max_messages,
                        label_filters: gmail_cfg.label_filters.clone(),
                        ..Default::default()
                    },
                ));
            provider_registry.register(provider_config, gmail_provider);
            tracing::info!("[swarm] Gmail provider registered");
        }

        if let Some(ref notion_cfg) = integrations_config.notion {
            use savant_integrations::provider::{ProviderConfig, ProviderKind};
            let mut settings = std::collections::HashMap::new();
            settings.insert(
                "integration_token".to_string(),
                notion_cfg.integration_token.clone(),
            );
            settings.insert(
                "database_ids".to_string(),
                serde_json::to_string(&notion_cfg.database_ids).unwrap_or_default(),
            );
            settings.insert("max_pages".to_string(), notion_cfg.max_pages.to_string());
            let provider_config = ProviderConfig {
                kind: ProviderKind::Notion,
                settings,
                enabled: true,
                sync_interval_secs: 3600,
            };
            let notion_provider: Arc<dyn savant_integrations::Provider> =
                Arc::new(savant_integrations::NotionProvider::new(
                    provider_config.clone(),
                    savant_integrations::NotionConfig {
                        integration_token: Some(notion_cfg.integration_token.clone()),
                        database_ids: notion_cfg.database_ids.clone(),
                        max_pages: notion_cfg.max_pages,
                        ..Default::default()
                    },
                ));
            provider_registry.register(provider_config, notion_provider);
            tracing::info!("[swarm] Notion provider registered");
        }

        let (integrations_shutdown_tx, integrations_shutdown_rx) =
            tokio::sync::watch::channel(false);
        let sync_state_path = config
            .workspace_root
            .join(".savant")
            .join("sync_state.json");
        match savant_integrations::scheduler::SyncScheduler::new(
            provider_registry.clone(),
            sync_state_path,
            3600, // 1 hour default sync interval
            integrations_shutdown_rx,
        )
        .await
        {
            Ok(scheduler) => {
                tokio::spawn(async move {
                    scheduler.run().await;
                });
                tracing::info!("[swarm] Integration sync scheduler started (interval: 3600s)");
            }
            Err(e) => tracing::warn!("[swarm] SyncScheduler init failed: {}", e),
        }

        // Initialize skill lifecycle manager
        let mut skill_manager = savant_skills::parser::SkillManager::new(skill_path.clone());
        if let Err(e) = skill_manager.discover_all_skills(None).await {
            tracing::warn!("[swarm] SkillManager discovery failed: {}", e);
        }
        let skill_manager = Arc::new(tokio::sync::Mutex::new(skill_manager));

        // E5: Wire skill hot reload — watches skills directory for changes
        let hot_reload = savant_skills::hot_reload::SkillHotReload::new(
            skill_path.clone(),
            Arc::new(tokio::sync::Mutex::new(
                savant_skills::parser::SkillRegistry::new(),
            )),
        );
        if let Err(e) = hot_reload.start() {
            tracing::warn!("[swarm] Skill hot reload failed to start: {}", e);
        } else {
            tracing::info!("[swarm] Skill hot reload active for {:?}", skill_path);
        }

        // Initialize credential broker for per-task ephemeral token management
        let credential_broker =
            Arc::new(savant_security::continuous::credentials::CredentialBroker::new());

        // Clone workspace root before config is moved into struct
        let workspace_root_for_delegation = config.workspace_root.clone();

        Ok(Self {
            config,
            nexus,
            storage,
            manager,
            agents,
            client: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(12))
                .connect_timeout(std::time::Duration::from_secs(5))
                .pool_max_idle_per_host(4)
                .redirect(reqwest::redirect::Policy::limited(10))
                .build()
                .map_err(|e| {
                    savant_core::error::SavantError::Unknown(format!(
                        "CRITICAL: Failed to build secure HTTP client: {}",
                        e
                    ))
                })?,
            handles: DashMap::new(),
            tools,
            engine,
            embedding_service,
            vision_service,
            blackboard,
            root_authority,
            signing_key,
            pqc_authority,
            pqc_signing_key,
            echo_registry,
            echo_compiler,
            echo_metrics,
            echo_host,
            collective_blackboard,
            agent_index_counter: AtomicU8::new(1),
            dead_agents: DashMap::new(),
            mcp_servers,
            skill_manager,
            integrations_shutdown_tx,
            credential_broker,
            shutdown_tracker: Arc::new(crate::graceful_shutdown::GracefulShutdownTracker::new()),
            replay_recorder,
            schema_index,
            lsp_manager,
            consciousness_state: Arc::new(AtomicU8::new(1)), // Idle
            consciousness_shutdown: CancellationToken::new(),
            governor: {
                let gov_config = savant_core::config::ResourceGovernorConfig::default();
                if gov_config.enabled {
                    Some(crate::governor::SwarmGovernor::new(
                        gov_config,
                        CancellationToken::new(),
                    ))
                } else {
                    None
                }
            },
            shared_capability_registry: Arc::new(
                savant_ipc::CapabilityRegistry::new("shared_swarm_caps", 128).unwrap_or_else(|e| {
                    tracing::warn!(
                        "Failed to create shared CapabilityRegistry: {}. Using fallback.",
                        e
                    );
                    savant_ipc::CapabilityRegistry::new("fallback_shared_caps", 128)
                        .expect("CRITICAL: Cannot create fallback CapabilityRegistry")
                }),
            ),
            subagent_registry: crate::subagent_registry::SubAgentRegistry::new(),
            delegation_engine: {
                let del_gov_config = savant_core::config::ResourceGovernorConfig::default();
                let governor =
                    crate::governor::SwarmGovernor::new(del_gov_config, CancellationToken::new());
                let registry = crate::subagent_registry::SubAgentRegistry::new();
                crate::delegation::DelegationEngine::new(
                    governor,
                    registry,
                    vec![workspace_root_for_delegation],
                )
            },
        })
    }

    /// Launches the entire swarm into autonomous pulse mode.
    pub async fn ignite(&self) {
        tracing::info!("Igniting Savant swarm with {} agents...", self.agents.len());

        // Spawn ECHO Watcher
        if let Err(e) = savant_echo::watcher::spawn_echo_watcher(
            self.config.workspace_root.clone(),
            self.echo_registry.clone(),
            self.echo_compiler.clone(),
        )
        .await
        {
            tracing::error!("Failed to start ECHO watcher: {}", e);
        }

        // Start Resource Governor background tasks (monitor + adaptive adjuster)
        if let Some(ref governor) = self.governor {
            let _handles = governor.start();
            tracing::info!(
                "[governor] Started — {} permits at {} pressure",
                governor.available_permits(),
                governor.current_pressure()
            );
        }

        // Spawn Executive Monitor (Global Workspace Theory — selection-broadcast cycle)
        let (_ws_delta_tx, ws_delta_rx) = tokio::sync::watch::channel(0.0f32);
        let exec_monitor = Arc::new(crate::workspace::ExecutiveMonitor::new(ws_delta_rx));
        exec_monitor.register_listener(
            "swarm_broadcast",
            Arc::new(move |event| {
                tracing::debug!(
                    "[gwt] Broadcast: slot={}, salience={:.2}, source={:?}",
                    event.slot_id,
                    event.salience,
                    event.source
                );
            }),
        );
        let monitor_clone = exec_monitor.clone();
        tokio::spawn(async move {
            monitor_clone.run().await;
        });

        // Spawn Consciousness Daemon (continuous thinking, entropy-based cadence)
        let consciousness_llm = self.create_consciousness_provider().await;
        if let Some(llm) = consciousness_llm {
            let shared_state = self.consciousness_state.clone();
            let daemon = crate::consciousness::ConsciousnessDaemon::with_state_handle(
                llm,
                self.config.workspace_root.clone(),
                self.consciousness_shutdown.clone(),
                shared_state,
            );
            tokio::spawn(async move {
                daemon.run().await;
            });
            tracing::info!("[swarm] Consciousness daemon started");
        } else {
            tracing::warn!("[swarm] No LLM provider available for consciousness daemon — disabled");
        }

        // Spawn credential broker cleanup task (runs every 5 minutes)
        let broker_for_cleanup = self.credential_broker.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(300));
            interval.tick().await; // skip first immediate tick
            loop {
                interval.tick().await;
                broker_for_cleanup.cleanup_expired().await;
            }
        });

        // Spawn compact rules reload listener (reloads on CONFIG_SET_RESULT events)
        let nexus_for_compact = self.nexus.clone();
        tokio::spawn(async move {
            let (mut rx, _) = nexus_for_compact.subscribe().await;
            while let Ok(event) = rx.recv().await {
                if event.event_type == "CONFIG_SET_RESULT" {
                    tracing::info!("[compact] Config changed — reloading compact rules");
                    crate::compact::integration::reload_rules().await;
                }
            }
        });

        for agent in &self.agents {
            self.spawn_agent(agent.clone()).await;
        }

        // Drain deferred agents from the governor retry channel.
        // When the governor's background drain loop finds a permit for a
        // deferred agent, it sends the AgentConfig through the retry channel.
        // We log and publish a Nexus event for observability. The actual
        // re-spawn will happen on the next app restart or manual trigger,
        // since we can't call &self.spawn_agent() from a spawned task.
        if let Some(ref governor) = self.governor {
            if let Some(mut retry_rx) = governor.take_retry_rx().await {
                let nexus_for_retry = self.nexus.clone();
                tokio::spawn(async move {
                    while let Some(agent_cfg) = retry_rx.recv().await {
                        tracing::info!(
                            "[swarm] Deferred agent '{}' ready for retry — publishing event",
                            agent_cfg.agent_name
                        );
                        let payload = serde_json::json!({
                            "agent_id": agent_cfg.agent_id,
                            "agent_name": agent_cfg.agent_name,
                        });
                        if let Err(e) = nexus_for_retry
                            .publish("system.agent.retry", &payload.to_string())
                            .await
                        {
                            tracing::warn!(
                                "[swarm] Failed to publish agent.retry event: {}",
                                e
                            );
                        }
                    }
                });
            }
        }
    }

    /// Spawns a single agent into the swarm.
    pub async fn spawn_agent(&self, agent_cfg: AgentConfig) {
        let agent_id = agent_cfg.agent_id.clone();
        let agent_name = agent_cfg.agent_name.clone();

        // Resource governor check — defer if system under pressure
        if let Some(ref governor) = self.governor {
            if governor.try_spawn().is_none() {
                tracing::warn!(
                    "[governor] Deferring '{}' — {} pressure, no permits available",
                    agent_name,
                    governor.current_pressure()
                );
                governor.defer_agent(agent_cfg).await;
                return;
            }
            tracing::debug!(
                "[governor] Permit granted for '{}' (pressure={})",
                agent_name,
                governor.current_pressure()
            );
        }

        // Sign the agent config for authenticity verification by the spawned agent
        let config_json = serde_json::to_string(&agent_cfg).unwrap_or_default();
        match self.sign_message(&config_json) {
            Ok(signature) => {
                tracing::debug!(
                    "[swarm] Agent config signed for '{}' (sig_len={})",
                    agent_id,
                    signature.len()
                );
            }
            Err(e) => {
                tracing::warn!(
                    "[swarm] Failed to sign agent config for '{}': {}",
                    agent_id,
                    e
                );
            }
        }

        self.evacuate_agent(&agent_id).await;

        // Background workspace file indexing for the agent
        let ws_path = agent_cfg.workspace_path.clone();
        let agent_id_for_index = agent_id.clone();
        tokio::spawn(async move {
            let db_path = ws_path.join("file_index.db");
            let indexer = savant_core::fs::FileIndexer::new(db_path);
            if let Err(e) = indexer.init_db() {
                tracing::warn!(
                    "[swarm] FileIndexer DB init failed for '{}': {}",
                    agent_id_for_index,
                    e
                );
                return;
            }
            match indexer.index_directory(&agent_id_for_index, &ws_path).await {
                Ok(()) => {
                    tracing::info!(
                        "[swarm] FileIndexer for '{}': workspace indexed",
                        agent_id_for_index,
                    );
                }
                Err(e) => {
                    tracing::warn!(
                        "[swarm] FileIndexer failed for '{}': {}",
                        agent_id_for_index,
                        e
                    );
                }
            }
        });

        let nexus = self.nexus.clone();
        let storage = self.storage.clone();
        let manager = self.manager.clone();
        let client = self.client.clone();
        let tools = self.tools.clone();
        let engine = Arc::clone(&self.engine);
        let embedding_service = self.embedding_service.clone();
        let vision_service = self.vision_service.clone();
        let root_authority = self.root_authority; // VerifyingKey is Copy, so this is a copy.
        let signing_key = self.signing_key.clone();
        let pqc_authority = self.pqc_authority;
        let blackboard = self.blackboard.clone();
        let pqc_signing_key = self.pqc_signing_key;
        let echo_registry = self.echo_registry.clone();
        let echo_metrics = self.echo_metrics.clone();
        let echo_host = self.echo_host.clone();
        let collective = self.collective_blackboard.clone();
        let mcp_servers = self.mcp_servers.clone();
        let credential_broker = self.credential_broker.clone();
        let skill_manager = self.skill_manager.clone();
        let schema_index = self.schema_index.clone();
        let lsp_manager = self.lsp_manager.clone();
        let browser_config = self
            .config
            .config_file
            .as_deref()
            .and_then(savant_browser::BrowserConfig::from_config_file)
            .unwrap_or_default();

        // Assign a unique index for consensus voting (sequential 1-128)
        // Uses compare_exchange loop to prevent race condition on wrap-around.
        let agent_index = loop {
            let current = self.agent_index_counter.load(Ordering::SeqCst);
            let next = if current >= 128 { 1 } else { current + 1 };
            if self
                .agent_index_counter
                .compare_exchange(current, next, Ordering::SeqCst, Ordering::SeqCst)
                .is_ok()
            {
                break next;
            }
        };

        let shutdown_token = CancellationToken::new();
        let shutdown_task_token = shutdown_token.clone();
        let dream_engine = self.engine.clone();
        let replay_recorder = self.replay_recorder.clone();
        let shared_capability_registry = self.shared_capability_registry.clone();
        let delegation_engine = self.delegation_engine.clone();

        let handle = tokio::spawn(async move {
            let mut agent_cfg = agent_cfg;
            // Master key resolution - reads from process env, not agent config
            if agent_cfg.api_key.is_none() {
                if let Ok(master_key) = std::env::var("OR_MASTER_KEY") {
                    tracing::info!(
                        "[{}] OR_MASTER_KEY found, creating derivative key",
                        agent_name
                    );
                    match OpenRouterMgmt::new(master_key.clone())
                        .create_key(&agent_name)
                        .await
                    {
                        Ok(derivative_key) => {
                            tracing::info!("[{}] Derivative key created successfully", agent_name);
                            agent_cfg.api_key = Some(derivative_key);
                        }
                        Err(e) => {
                            tracing::error!(
                                "[{}] CRITICAL: Key creation failed: {}. Not falling back to master key.",
                                agent_name, e
                            );
                        }
                    }
                } else if let Ok(direct_key) = std::env::var("OPENROUTER_API_KEY") {
                    tracing::info!(
                        "[{}] OPENROUTER_API_KEY found, using directly",
                        agent_name
                    );
                    agent_cfg.api_key = Some(direct_key);
                } else {
                    tracing::warn!(
                        "[{}] No API key found in environment (checked OR_MASTER_KEY and OPENROUTER_API_KEY)",
                        agent_name
                    );
                }
            }

            agent_cfg = match manager.boot_agent(agent_cfg).await {
                Ok(cfg) => cfg,
                Err(e) => {
                    parsing::log_agent_error(&agent_name, "Failed to boot agent", e);
                    tracing::error!(
                        "[swarm] CRITICAL: Agent '{}' exiting before heartbeat start — command bus will have 0 receivers",
                        agent_name
                    );
                    return;
                }
            };

            // 2. Fetch model info from OpenRouter (universal model database)
            //    Only fetch when provider is OpenRouter — other providers don't have
            //    their models listed on OpenRouter, so the fetch always returns empty.
            let model_info = if agent_cfg.model_provider == ModelProvider::OpenRouter {
                let model_id_for_info = agent_cfg
                    .model
                    .clone()
                    .unwrap_or_else(|| "anthropic/claude-3-sonnet".to_string());

                let or_master_key = std::env::var("OR_MASTER_KEY")
                    .or_else(|_| std::env::var("OPENROUTER_API_KEY"))
                    .unwrap_or_default();

                let info = crate::providers::fetch_openrouter_model_info(
                    &client,
                    &or_master_key,
                    &model_id_for_info,
                )
                .await;

                if let Some(ref i) = info {
                    tracing::info!(
                        "[{}] Model info loaded: context={}, max_completion={}, safe_max_tokens={}",
                        agent_name,
                        i.context_length.unwrap_or(0),
                        i.max_completion_tokens.unwrap_or(0),
                        i.safe_max_tokens()
                    );
                } else {
                    tracing::warn!(
                        "[{}] Could not fetch model info for '{}' — using defaults",
                        agent_name,
                        agent_cfg.model.as_deref().unwrap_or("unknown")
                    );
                }
                info
            } else {
                tracing::info!(
                    "[{}] Skipping OpenRouter model info fetch (provider: {:?})",
                    agent_name,
                    agent_cfg.model_provider
                );
                None
            };

            // 3. Select LLM Provider
            let base_provider: Arc<dyn LlmProvider> = match agent_cfg.model_provider {
                ModelProvider::OpenRouter => {
                    let model_id = agent_cfg
                        .model
                        .clone()
                        .unwrap_or_else(|| "anthropic/claude-3-sonnet".to_string());
                    let or_api_key = agent_cfg.api_key.clone().unwrap_or_default();
                    Arc::new(OpenRouterProvider {
                        client: client.clone(),
                        api_key: or_api_key,
                        model: model_id,
                        agent_id: agent_cfg.agent_id.clone(),
                        agent_name: agent_cfg.agent_name.clone(),
                        llm_params: Some(agent_cfg.llm_params.clone()),
                        context_window: model_info.as_ref().and_then(|m| m.context_length),
                        max_completion_tokens: model_info.as_ref().map(|m| m.safe_max_tokens()),
                    })
                }
                ModelProvider::OpenAi => Arc::new(OpenAiProvider {
                    client: client.clone(),
                    api_key: agent_cfg.api_key.clone().unwrap_or_default(),
                    model: agent_cfg
                        .model
                        .clone()
                        .unwrap_or_else(|| "gpt-4".to_string()),
                    agent_id: agent_cfg.agent_id.clone(),
                    agent_name: agent_cfg.agent_name.clone(),
                    llm_params: Some(agent_cfg.llm_params.clone()),
                    max_completion_tokens: model_info.as_ref().map(|m| m.safe_max_tokens()),
                    base_url: "https://api.openai.com/v1".to_string(),
                }),
                ModelProvider::OpenGateway => {
                    // OpenGateway is deprecated — route through OpenRouter
                    tracing::warn!(
                        "[{}] OpenGateway provider deprecated — routing through OpenRouter",
                        agent_cfg.agent_name
                    );
                    let model_id = agent_cfg
                        .model
                        .clone()
                        .unwrap_or_else(|| "tencent/hy3:free".to_string());
                    let or_api_key = agent_cfg.api_key.clone().unwrap_or_default();
                    Arc::new(OpenRouterProvider {
                        client: client.clone(),
                        api_key: or_api_key,
                        model: model_id,
                        agent_id: agent_cfg.agent_id.clone(),
                        agent_name: agent_cfg.agent_name.clone(),
                        llm_params: Some(agent_cfg.llm_params.clone()),
                        context_window: model_info.as_ref().and_then(|m| m.context_length),
                        max_completion_tokens: model_info.as_ref().map(|m| m.safe_max_tokens()),
                    })
                }
                ModelProvider::Anthropic => Arc::new(AnthropicProvider {
                    client: client.clone(),
                    api_key: agent_cfg.api_key.clone().unwrap_or_default(),
                    model: agent_cfg
                        .model
                        .clone()
                        .unwrap_or_else(|| "claude-3-sonnet-20240229".to_string()),
                    agent_id: agent_cfg.agent_id.clone(),
                    agent_name: agent_cfg.agent_name.clone(),
                    llm_params: Some(agent_cfg.llm_params.clone()),
                    max_completion_tokens: model_info.as_ref().map(|m| m.safe_max_tokens()),
                }),
                ModelProvider::Ollama => Arc::new(OllamaProvider {
                    client: client.clone(),
                    // NOTE: For local providers (Ollama, LMStudio), the `api_key` config field
                    // stores the provider URL, not an actual API key. This is a documented
                    // convention — local providers don't need authentication.
                    url: agent_cfg
                        .api_key
                        .clone()
                        .unwrap_or_else(|| "http://localhost:11434".to_string()),
                    model: agent_cfg
                        .model
                        .clone()
                        .unwrap_or_else(|| "llama2".to_string()),
                    agent_id: agent_cfg.agent_id.clone(),
                    agent_name: agent_cfg.agent_name.clone(),
                }),
                ModelProvider::Groq => Arc::new(GroqProvider {
                    client: client.clone(),
                    api_key: agent_cfg.api_key.clone().unwrap_or_default(),
                    model: agent_cfg
                        .model
                        .clone()
                        .unwrap_or_else(|| "llama2-70b-4096".to_string()),
                    agent_id: agent_cfg.agent_id.clone(),
                    agent_name: agent_cfg.agent_name.clone(),
                    llm_params: Some(agent_cfg.llm_params.clone()),
                    max_completion_tokens: model_info.as_ref().map(|m| m.safe_max_tokens()),
                }),
                ModelProvider::Google => Arc::new(GoogleProvider {
                    client: client.clone(),
                    api_key: agent_cfg.api_key.clone().unwrap_or_default(),
                    model: agent_cfg
                        .model
                        .clone()
                        .unwrap_or_else(|| "gemini-pro".to_string()),
                    agent_id: agent_cfg.agent_id.clone(),
                    agent_name: agent_cfg.agent_name.clone(),
                    llm_params: Some(agent_cfg.llm_params.clone()),
                    max_completion_tokens: model_info.as_ref().map(|m| m.safe_max_tokens()),
                }),
                ModelProvider::Mistral => Arc::new(MistralProvider {
                    client: client.clone(),
                    api_key: agent_cfg.api_key.clone().unwrap_or_default(),
                    model: agent_cfg
                        .model
                        .clone()
                        .unwrap_or_else(|| "mistral-large-latest".to_string()),
                    agent_id: agent_cfg.agent_id.clone(),
                    agent_name: agent_cfg.agent_name.clone(),
                    llm_params: Some(agent_cfg.llm_params.clone()),
                    max_completion_tokens: model_info.as_ref().map(|m| m.safe_max_tokens()),
                }),
                ModelProvider::Together => Arc::new(TogetherProvider {
                    client: client.clone(),
                    api_key: agent_cfg.api_key.clone().unwrap_or_default(),
                    model: agent_cfg
                        .model
                        .clone()
                        .unwrap_or_else(|| "togethercomputer/llama-2-70b-chat".to_string()),
                    agent_id: agent_cfg.agent_id.clone(),
                    agent_name: agent_cfg.agent_name.clone(),
                    llm_params: Some(agent_cfg.llm_params.clone()),
                    max_completion_tokens: model_info.as_ref().map(|m| m.safe_max_tokens()),
                }),
                ModelProvider::Deepseek => Arc::new(DeepseekProvider {
                    client: client.clone(),
                    api_key: agent_cfg.api_key.clone().unwrap_or_default(),
                    model: agent_cfg
                        .model
                        .clone()
                        .unwrap_or_else(|| "deepseek-chat".to_string()),
                    agent_id: agent_cfg.agent_id.clone(),
                    agent_name: agent_cfg.agent_name.clone(),
                    llm_params: Some(agent_cfg.llm_params.clone()),
                    max_completion_tokens: model_info.as_ref().map(|m| m.safe_max_tokens()),
                }),
                ModelProvider::Cohere => Arc::new(CohereProvider {
                    client: client.clone(),
                    api_key: agent_cfg.api_key.clone().unwrap_or_default(),
                    model: agent_cfg
                        .model
                        .clone()
                        .unwrap_or_else(|| "command-r-plus".to_string()),
                    agent_id: agent_cfg.agent_id.clone(),
                    agent_name: agent_cfg.agent_name.clone(),
                    llm_params: Some(agent_cfg.llm_params.clone()),
                    max_completion_tokens: model_info.as_ref().map(|m| m.safe_max_tokens()),
                }),
                ModelProvider::Azure => Arc::new(AzureProvider {
                    client: client.clone(),
                    api_key: agent_cfg.api_key.clone().unwrap_or_default(),
                    endpoint: std::env::var("AZURE_OPENAI_ENDPOINT").unwrap_or_default(),
                    deployment: agent_cfg
                        .model
                        .clone()
                        .unwrap_or_else(|| "gpt-4".to_string()),
                    api_version: std::env::var("AZURE_OPENAI_API_VERSION")
                        .unwrap_or_else(|_| "2024-02-01".to_string()),
                    agent_id: agent_cfg.agent_id.clone(),
                    agent_name: agent_cfg.agent_name.clone(),
                    llm_params: Some(agent_cfg.llm_params.clone()),
                    max_completion_tokens: model_info.as_ref().map(|m| m.safe_max_tokens()),
                }),
                ModelProvider::Xai => Arc::new(XaiProvider {
                    client: client.clone(),
                    api_key: agent_cfg.api_key.clone().unwrap_or_default(),
                    model: agent_cfg
                        .model
                        .clone()
                        .unwrap_or_else(|| "grok-2".to_string()),
                    agent_id: agent_cfg.agent_id.clone(),
                    agent_name: agent_cfg.agent_name.clone(),
                    llm_params: Some(agent_cfg.llm_params.clone()),
                    max_completion_tokens: model_info.as_ref().map(|m| m.safe_max_tokens()),
                }),
                ModelProvider::Fireworks => Arc::new(FireworksProvider {
                    client: client.clone(),
                    api_key: agent_cfg.api_key.clone().unwrap_or_default(),
                    model: agent_cfg.model.clone().unwrap_or_else(|| {
                        "accounts/fireworks/models/llama-v2-70b-chat".to_string()
                    }),
                    agent_id: agent_cfg.agent_id.clone(),
                    agent_name: agent_cfg.agent_name.clone(),
                    llm_params: Some(agent_cfg.llm_params.clone()),
                    max_completion_tokens: model_info.as_ref().map(|m| m.safe_max_tokens()),
                }),
                ModelProvider::Novita => Arc::new(NovitaProvider {
                    client: client.clone(),
                    api_key: agent_cfg.api_key.clone().unwrap_or_default(),
                    model: agent_cfg
                        .model
                        .clone()
                        .unwrap_or_else(|| "meta-llama/llama-2-70b-chat".to_string()),
                    agent_id: agent_cfg.agent_id.clone(),
                    agent_name: agent_cfg.agent_name.clone(),
                    llm_params: Some(agent_cfg.llm_params.clone()),
                    max_completion_tokens: model_info.as_ref().map(|m| m.safe_max_tokens()),
                }),
                ModelProvider::LmStudio | ModelProvider::Perplexity | ModelProvider::Local => {
                    // NOTE: These providers currently route through OpenRouter.
                    // LMStudio/Local should use local endpoints in a future update.
                    // Perplexity uses OpenRouter as proxy.
                    tracing::warn!(
                        "[{}] Provider {:?} routing through OpenRouter (local endpoint not yet wired)",
                        agent_cfg.agent_id,
                        agent_cfg.model_provider
                    );
                    let model_id = agent_cfg
                        .model
                        .clone()
                        .unwrap_or_else(|| "anthropic/claude-3-sonnet".to_string());
                    let or_api_key = agent_cfg.api_key.clone().unwrap_or_default();
                    Arc::new(OpenRouterProvider {
                        client: client.clone(),
                        api_key: or_api_key,
                        model: model_id,
                        agent_id: agent_cfg.agent_id.clone(),
                        agent_name: agent_cfg.agent_name.clone(),
                        llm_params: Some(agent_cfg.llm_params.clone()),
                        context_window: model_info.as_ref().and_then(|m| m.context_length),
                        max_completion_tokens: model_info.as_ref().map(|m| m.safe_max_tokens()),
                    })
                }
            };

            let provider: Arc<dyn LlmProvider> = base_provider;

            // Wrap provider in ProviderChain for resilience (circuit breaker, timeout, rate limiter)
            let chain_config = crate::providers::chain::ChainConfig::default();
            let provider_chain = crate::providers::chain::ProviderChain::new(
                provider.clone(),
                agent_cfg.agent_id.clone(),
                chain_config,
            );
            // The chain implements LlmProvider — use it as the agent's provider
            let provider: Arc<dyn LlmProvider> = Arc::new(provider_chain);

            // 3. Filter Tools for this agent
            let mut agent_tools: Vec<Arc<dyn Tool>> = agent_cfg
                .allowed_skills
                .iter()
                .filter_map(|name| tools.get(&name.to_lowercase()).cloned())
                .collect();

            // Create async memory backend from the shared engine
            // W1a: Extract enclave before engine is moved (FID-20260529-MEMORY-ENCLAVE)
            let memory_enclave = engine.enclave();
            let inner_backend = Arc::new(AsyncMemoryBackend::with_embeddings(
                engine,
                embedding_service.clone(),
            ));

            // Wrap in FileLoggingMemoryBackend to fulfill Perfection Loop requirements
            let memory_backend: Arc<dyn MemoryBackend> =
                Arc::new(crate::memory::FileLoggingMemoryBackend::new(
                    inner_backend,
                    agent_cfg.workspace_path.clone(),
                ));

            // Inject System-Level Atomic Memory Tools (using the wrapped backend)
            agent_tools.push(Arc::new(crate::tools::MemoryAppendTool::new(
                memory_backend.clone(),
                agent_cfg.agent_id.clone(),
            )));
            agent_tools.push(Arc::new(crate::tools::MemorySearchTool::new(
                memory_backend.clone(),
                agent_cfg.agent_id.clone(),
            )));

            // 🌌 Universal Autonomy Protocol: All agents are granted Foundation Sovereignty
            agent_tools.push(Arc::new(crate::tools::FoundationTool::new(
                agent_cfg.workspace_path.clone(),
            )));
            // C4: Shared security scanner for file tools
            let file_scanner = Arc::new(savant_skills::security::SecurityScanner::new());
            agent_tools.push(Arc::new(
                crate::tools::FileMoveTool::new(agent_cfg.workspace_path.clone())
                    .with_scanner(file_scanner.clone()),
            ));
            agent_tools.push(Arc::new(
                crate::tools::FileDeleteTool::new(agent_cfg.workspace_path.clone())
                    .with_scanner(file_scanner.clone()),
            ));
            agent_tools.push(Arc::new(
                crate::tools::FileAtomicEditTool::new(agent_cfg.workspace_path.clone())
                    .with_scanner(file_scanner.clone()),
            ));
            agent_tools.push(Arc::new(
                crate::tools::FileCreateTool::new(agent_cfg.workspace_path.clone())
                    .with_scanner(file_scanner),
            ));
            // NA-09: Wire SettingsTool with workspace path for sandbox-safe resolution
            agent_tools.push(Arc::new(crate::tools::SettingsTool::with_workspace(
                &agent_cfg.workspace_path,
            )));
            agent_tools.push(Arc::new(crate::tools::SovereignShell::new(
                agent_cfg.workspace_path.clone(),
                Arc::new(savant_skills::security::SecurityScanner::new()),
            )));
            agent_tools.push(Arc::new(crate::tools::TaskMatrixTool::new(
                agent_cfg.workspace_path.clone(),
                agent_cfg.proactive.clone(),
            )));
            let browser_config = browser_config.clone();
            agent_tools.push(Arc::new(crate::tools::BrowserTool::new(browser_config)));
            agent_tools.push(Arc::new(crate::tools::SkillManagerTool::new(
                skill_manager.clone(),
            )));
            agent_tools.push(Arc::new(crate::tools::skill_lookup::SkillLookupTool::new(
                skill_manager.clone(),
            )));
            if let Ok(tracker) = savant_toolforge::ProvenanceTracker::new(
                &std::path::PathBuf::from("skills/forge/.provenance.jsonl"),
            ) {
                agent_tools.push(Arc::new(crate::tools::ToolForgeTool::new(
                    std::path::PathBuf::from("skills/forge"),
                    savant_toolforge::SharedToolRegistry::new(),
                    std::sync::Arc::new(tracker),
                )));
            } else {
                tracing::warn!(
                    "[{}] ProvenanceTracker unavailable — ToolForgeTool skipped",
                    agent_name
                );
            }

            // Register generation tools (SVG zero-VRAM + image generation)
            // provider is already Arc<dyn LlmProvider> — clone for shared ownership
            let provider_arc: Arc<dyn savant_core::traits::LlmProvider> = provider.clone();
            let svg_backend = Arc::new(savant_generation::backends::svg::SvgBackend::new(Some(
                provider_arc.clone(),
            )));
            let cache_dir = std::path::PathBuf::from(".savant/generated/images");
            let image_cache = match savant_generation::cache::ImageCache::new(cache_dir, 1024) {
                Ok(cache) => cache,
                Err(e) => {
                    tracing::warn!("Failed to create image cache: {}", e);
                    // Use a cross-platform temp directory fallback (not /tmp which doesn't exist on Windows)
                    let fallback_dir = std::env::temp_dir().join("savant_cache");
                    match savant_generation::cache::ImageCache::new(
                        fallback_dir,
                        100,
                    ) {
                        Ok(fallback) => fallback,
                        Err(fb_err) => {
                            tracing::error!(
                                "[{}] Failed to create fallback image cache: {}. Image generation disabled.",
                                agent_name,
                                fb_err
                            );
                            // Non-fatal: create a no-op cache that accepts but discards entries
                            // instead of killing the entire agent task
                            match savant_generation::cache::ImageCache::new(
                                std::env::temp_dir().join("savant_cache_fallback"),
                                10,
                            ) {
                                Ok(last_resort) => last_resort,
                                Err(_) => {
                            tracing::error!(
                                "[{}] All image cache paths failed — agent cannot continue",
                                agent_name
                            );
                            tracing::error!(
                                "[swarm] CRITICAL: Agent '{}' exiting before heartbeat start — command bus will have 0 receivers",
                                agent_name
                            );
                            return;
                                }
                            }
                        }
                    }
                }
            };
            let expander =
                savant_generation::prompt::PromptExpander::new(Some(provider_arc.clone()));
            let generation_config = savant_generation::GenerationConfig::default();
            let generation_backend: Arc<dyn savant_generation::backends::GenerationBackend> =
                svg_backend.clone();
            let orchestrator = Arc::new(
                savant_generation::orchestrator::GenerationOrchestrator::new(
                    generation_config,
                    vec![generation_backend],
                    expander,
                    image_cache,
                ),
            );
            agent_tools.push(Arc::new(crate::tools::GenerateSvgTool::new(svg_backend)));
            agent_tools.push(Arc::new(crate::tools::GenerateImageTool::new(orchestrator)));

            // Code intelligence tools (SchemaIndex-backed)
            if let Some(ref idx) = schema_index {
                // Background index — don't block agent startup
                let idx_bg = idx.clone();
                let name_bg = agent_name.clone();
                tokio::task::spawn_blocking(move || {
                    let stats = idx_bg.index_all();
                    tracing::info!(
                        "[{}] SchemaIndex: indexed {} files, {} symbols",
                        name_bg,
                        stats.files_indexed,
                        stats.symbols_found
                    );
                });
                agent_tools.push(Arc::new(crate::tools::CodeSearchTool::new(idx.clone())));
                agent_tools.push(Arc::new(crate::tools::GetCallersTool::new(idx.clone())));
                agent_tools.push(Arc::new(crate::tools::GetImpactTool::new(idx.clone())));
                agent_tools.push(Arc::new(crate::tools::GetSymbolsTool::new(idx.clone())));
                tracing::info!("[{}] 4 schema tools registered", agent_name);
            }

            // LSP tools (LspManager-backed)
            if let Some(ref mgr) = lsp_manager {
                agent_tools.push(Arc::new(crate::lsp::LspHoverTool::new(mgr.clone())));
                agent_tools.push(Arc::new(crate::lsp::LspGotoDefinitionTool::new(
                    mgr.clone(),
                )));
                agent_tools.push(Arc::new(crate::lsp::LspFindReferencesTool::new(
                    mgr.clone(),
                )));
                agent_tools.push(Arc::new(crate::lsp::LspDiagnosticsTool::new(mgr.clone())));
                tracing::info!("[{}] 4 LSP tools registered", agent_name);
            }

            // Shell intelligence tools (stateless)
            agent_tools.push(Arc::new(crate::shell_intel::ExplainCommandTool));
            tracing::info!("[{}] ExplainCommandTool registered", agent_name);

            // Discover and register MCP tools from configured servers via McpClientPool
            let mcp_pool = savant_mcp::client::McpClientPool::new();
            for server in &mcp_servers {
                let result = if let Some(ref auth_token) = server.auth_token {
                    mcp_pool
                        .connect_server_with_auth(server.url.as_str(), auth_token.as_str())
                        .await
                } else {
                    mcp_pool.connect_server(server.url.as_str()).await
                };
                match result {
                    Ok(count) => {
                        tracing::info!(
                            "[{}] Discovered {} MCP tools from {}",
                            agent_name,
                            count,
                            server.name
                        );
                    }
                    Err(e) => {
                        tracing::warn!(
                            "[{}] Failed to connect to MCP server {}: {}",
                            agent_name,
                            server.name,
                            e
                        );
                    }
                }
            }
            let mcp_tools = mcp_pool.get_tools().await;
            tracing::info!(
                "[{}] Total MCP tools available: {}",
                agent_name,
                mcp_tools.len()
            );
            agent_tools.extend(mcp_tools);

            // G-2: Validate all tool schemas before building AgentLoop
            for tool in &agent_tools {
                let schema = tool.parameters_schema();
                if let Err(e) = crate::tools::schema_validator::validate_strict_schema(&schema) {
                    tracing::warn!(
                        "[{}] Tool '{}' has non-compliant schema: {:?}",
                        agent_name,
                        tool.name(),
                        e
                    );
                }
            }

            // 5. Build Agent Loop with the async backend and secure WASM host
            // OMEGA-VIII: Issue a workspace-scoped CCT (Cognitive Capability Token)
            // Convert Arc<HashMap> to HashMap by cloning the inner map for the plugin host
            let tools_for_host: HashMap<String, Arc<dyn Tool>> =
                tools.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
            let plugin_host = match crate::plugins::WasmPluginHost::new(
                root_authority,
                Some(pqc_authority),
                tools_for_host,
            ) {
                Ok(h) => Arc::new(h),
                Err(e) => {
                    parsing::log_agent_error(
                        &agent_name,
                        "Failed to init WASM host",
                        SavantError::Unknown(e.to_string()),
                    );
                    tracing::error!(
                        "[swarm] CRITICAL: Agent '{}' exiting before heartbeat start — command bus will have 0 receivers",
                        agent_name
                    );
                    return;
                }
            };

            // Mint CCT token: 24h duration, scoped to workspace
            // We use a derivation of the agent name as cadence entropy for this bootstrap session
            let token = match savant_security::SecurityAuthority::mint_quantum_token(
                &signing_key,
                &pqc_signing_key,
                agent_index as u64,
                &agent_cfg.workspace_path.to_string_lossy(),
                "execute",
                86400, // 24 hours
                agent_name.as_bytes(),
            ) {
                Ok(t) => Some(t),
                Err(e) => {
                    tracing::warn!(
                        "[swarm] Failed to mint CCT token for agent {}: {}",
                        agent_name,
                        e
                    );
                    None
                }
            };

            if token.is_some() {
                tracing::info!(
                    "CCT Token issued for agent: {} (ECHO-Absolute boundary active)",
                    agent_name
                );
            } else {
                tracing::warn!(
                    "Failed to mint CCT token for agent: {}. Running in restricted mode.",
                    agent_name
                );
            }

            // NA-12: Construct SecurityAuthority from root keys for AgentLoop
            let security_authority = Arc::new(savant_security::SecurityAuthority::new(
                root_authority,
                Some(pqc_authority),
            ));

            // Load API key into credential broker for this agent
            if let Some(ref api_key) = agent_cfg.api_key {
                credential_broker
                    .load_credential("openrouter", api_key)
                    .await;
                credential_broker
                    .load_credential("anthropic", api_key)
                    .await;
                credential_broker.load_credential("openai", api_key).await;
            }

            let rate_limiter = std::sync::Arc::new(crate::rate_limiter::RateLimiter::new(
                crate::rate_limiter::RateLimiterConfig::default(),
            ));

            let agent_loop = AgentLoop::new(
                agent_cfg.agent_id.clone(),
                provider_arc,
                memory_backend,
                agent_tools,
                agent_cfg.identity.clone().unwrap_or_default(),
                agent_cfg.system_prompt.clone(),
            )
            .with_echo(echo_registry, echo_metrics, echo_host)
            .with_collective(collective, agent_index)
            .with_plugins(plugin_host, Vec::new(), token)
            .with_security_authority(security_authority)
            .with_credential_broker(credential_broker.clone())
            .with_replay_recorder(replay_recorder)
            .with_rate_limiter(rate_limiter)
            .with_delegate(Box::new(crate::react::HeartbeatDelegate::new()));

            // Apply tool filter if agent has allowed_skills restriction
            let agent_loop = if !agent_cfg.allowed_skills.is_empty() {
                agent_loop.with_tool_filter(agent_cfg.allowed_skills.clone())
            } else {
                agent_loop
            };

            let agent_loop = if let Some(vision) = vision_service {
                agent_loop.with_vision(vision)
            } else {
                agent_loop
            };

            tracing::info!("Agent {} background pulse ignited.", agent_cfg.agent_name);

            // 6. Create delta channel for dream scheduler coordination
            let (delta_tx, delta_rx) = tokio::sync::watch::channel(0.0f32);

            // 7. Spawn the Dream Scheduler with delta receiver
            let dream_config = savant_dream::DreamConfig::default();
            let shutdown_token = CancellationToken::new();
            let dream_scheduler = savant_dream::scheduler::DreamScheduler::new(
                dream_config,
                dream_engine,
                delta_rx,
                shutdown_token,
            );
            tokio::spawn(async move {
                dream_scheduler.run().await;
            });

            // 7a. Spawn the CollectiveCurator for automatic tool lifecycle transitions
            {
                let curator_registry = savant_toolforge::SharedToolRegistry::new();
                let curator_provenance = std::sync::Arc::new(
                    savant_toolforge::ProvenanceTracker::new(&std::path::PathBuf::from(
                        "skills/forge/.provenance.jsonl",
                    ))
                    .unwrap_or_else(|_| {
                        // SAFETY: /dev/null path always succeed on all platforms
                        #[allow(clippy::disallowed_methods)]
                        savant_toolforge::ProvenanceTracker::new(&std::path::PathBuf::from(
                            "/dev/null",
                        ))
                        .expect("provenance fallback: /dev/null path is always valid")
                    }),
                );
                let curator = std::sync::Arc::new(
                    savant_toolforge::CollectiveCurator::new(curator_registry, curator_provenance)
                        .with_inactivity_threshold(30),
                );
                tokio::spawn(async move {
                    let mut interval = tokio::time::interval(std::time::Duration::from_secs(3600));
                    interval.tick().await;
                    loop {
                        interval.tick().await;
                        curator.run_auto_transitions().await;
                    }
                });
            }

            // 8. Register default hooks on the agent loop (fire-and-forget lifecycle hooks)
            agent_loop.register_default_hooks().await;

            // 9. Build Orchestrator from the pre-built AgentLoop (enterprise-grade wiring)
            // Uses shared CapabilityRegistry for cross-agent delegation.
            let capability_registry = shared_capability_registry;

            let mut orchestrator = crate::orchestration::Orchestrator::from_agent_loop(
                agent_loop,
                agent_cfg.agent_id.clone(),
                agent_cfg
                    .session_id
                    .clone()
                    .unwrap_or_else(|| agent_cfg.agent_id.clone()),
                blackboard.clone(),
                capability_registry,
                Some(memory_enclave),
            );

            // Wire DelegationEngine into Orchestrator for profile-based sub-agent spawning
            orchestrator.set_delegation_engine(delegation_engine.clone());

            // C1: Publish agent ready event after boot (FID-20260529)
            let ready_event = savant_core::types::EventFrame {
                event_type: "system.agent.ready".to_string(),
                payload: serde_json::json!({
                    "agent_id": agent_cfg.agent_id,
                    "agent_name": agent_cfg.agent_name,
                    "timestamp": std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs()
                })
                .to_string(),
            };
            if let Err(e) = nexus
                .publish(&ready_event.event_type, &ready_event.payload)
                .await
            {
                tracing::warn!(
                    "[swarm] Failed to publish agent.ready for {}: {}",
                    agent_cfg.agent_name,
                    e
                );
            }

            // 10. Start the Heartbeat Pulse with the Orchestrator
            let pulse =
                HeartbeatPulse::new(agent_cfg, nexus, storage, shutdown_task_token, delta_tx);
            pulse.start_with_orchestrator(orchestrator).await;
        });

        self.handles.insert(agent_id, (handle, shutdown_token));
    }

    pub async fn evacuate_agent(&self, agent_id: &str) {
        if let Some((_, (handle, token))) = self.handles.remove(agent_id) {
            tracing::info!(
                "Evacuating agent: {} (triggering graceful shutdown)",
                agent_id
            );
            token.cancel();

            // Revoke all ephemeral tokens for this agent's tasks
            self.credential_broker.revoke_task_tokens(agent_id).await;

            // Wait for in-flight requests to complete (graceful shutdown)
            let _guard = self.shutdown_tracker.register();
            let tracker = self.shutdown_tracker.clone();
            let agent_id_owned = agent_id.to_string();
            tokio::spawn(async move {
                if !tracker
                    .wait_for_all_timeout(std::time::Duration::from_secs(5))
                    .await
                {
                    tracing::warn!(
                        "Agent {} shutdown: {} in-flight requests still pending after timeout",
                        agent_id_owned,
                        tracker.in_flight()
                    );
                }
            });

            // Give it 12s to shut down gracefully before aborting
            match tokio::time::timeout(std::time::Duration::from_secs(12), handle).await {
                Ok(_) => {
                    tracing::info!("Agent {} shut down gracefully.", agent_id);
                }
                Err(_) => {
                    tracing::warn!("Agent {} timed out during shutdown.", agent_id);
                }
            }

            self.dead_agents.insert(agent_id.to_string(), ());
        }
    }

    pub async fn check_swarm_health(&self) -> Vec<String> {
        // NA-23: Verify core subsystems are accessible during health checks
        let nexus_ref = self.nexus();
        let blackboard_ref = self.blackboard();

        // Verify crypto subsystem is operational via sign/verify round-trip
        if let Ok(sig) = self.sign_message("health-check") {
            if let Err(e) = self.verify_message("health-check", &sig) {
                tracing::warn!("[swarm] Crypto health check FAILED: {}", e);
            }
        }
        tracing::debug!(
            "Swarm health check: nexus_refs={}, blackboard_refs={}",
            Arc::strong_count(&nexus_ref),
            Arc::strong_count(&blackboard_ref),
        );

        let mut dead_agents: Vec<String> =
            self.dead_agents.iter().map(|r| r.key().clone()).collect();
        for entry in self.handles.iter() {
            let (id, (handle, _)) = entry.pair();
            if handle.is_finished() && !dead_agents.contains(id) {
                dead_agents.push(id.clone());
            }
        }
        dead_agents
    }

    /// Returns system diagnostics including compact rule count.
    pub async fn diagnostics(&self) -> serde_json::Value {
        let compact_rules = crate::compact::integration::rule_count().await;
        let dead_agents = self.check_swarm_health().await;
        let active_agents = self.handles.len().saturating_sub(dead_agents.len());
        let (lsm_stats, vector_count) = self.engine.stats();
        serde_json::json!({
            "active_agents": active_agents,
            "dead_agents": dead_agents.len(),
            "compact_rules_loaded": compact_rules,
            "memory_messages": lsm_stats.total_messages,
            "memory_sessions": lsm_stats.total_sessions,
            "vector_count": vector_count,
            "continuation_limit": 10, // ContinuationConfig::default().max_continuations
        })
    }

    pub fn nexus(&self) -> Arc<NexusBridge> {
        self.nexus.clone()
    }

    pub fn engine(&self) -> Arc<MemoryEngine> {
        self.engine.clone()
    }

    pub fn blackboard(&self) -> Arc<SwarmBlackboard> {
        self.blackboard.clone()
    }

    pub async fn active_agents_count(&self) -> usize {
        self.handles.len()
    }

    /// Signs a message using the swarm's Ed25519 signing key.
    /// Used for IPC/A2A message authentication.
    pub fn sign_message(&self, message: &str) -> Result<String, savant_core::crypto::CryptoError> {
        let keypair = savant_core::crypto::AgentKeyPair {
            public_key: hex::encode(self.root_authority.to_bytes()),
            secret_key: hex::encode(self.signing_key.to_bytes()),
            key_id: "swarm-signing-key".to_string(),
            created_at: 0,
        };
        keypair.sign_message(message)
    }

    /// Verifies a message signature using the swarm's Ed25519 verifying key.
    /// Used for IPC/A2A message authentication.
    pub fn verify_message(
        &self,
        message: &str,
        signature: &str,
    ) -> Result<bool, savant_core::crypto::CryptoError> {
        let keypair = savant_core::crypto::AgentKeyPair {
            public_key: hex::encode(self.root_authority.to_bytes()),
            secret_key: hex::encode(self.signing_key.to_bytes()),
            key_id: "swarm-signing-key".to_string(),
            created_at: 0,
        };
        keypair.verify_message(message, signature)
    }

    /// Create an LLM provider for the consciousness daemon.
    /// Uses the first agent's config or defaults to OpenRouter.
    /// Creates its own derivative key from OR_MASTER_KEY (not the agent's config key)
    /// to avoid the timing issue where the daemon starts before spawn_agent() creates keys.
    async fn create_consciousness_provider(&self) -> Option<Arc<dyn LlmProvider>> {
        let agent_cfg = self.agents.first()?;
        let model_id = agent_cfg
            .model
            .clone()
            .unwrap_or_else(|| "tencent/hy3:free".to_string());

        // Create a derivative key from OR_MASTER_KEY for the daemon
        // Must happen BEFORE the daemon starts, otherwise it will fall back to
        // OR_MASTER_KEY which cannot do chat completions (management-only).
        let daemon_key = if let Ok(master_key) = std::env::var("OR_MASTER_KEY") {
            match OpenRouterMgmt::new(master_key)
                .create_key("consciousness-daemon")
                .await
            {
                Ok(key) => {
                    tracing::info!("[swarm] Consciousness daemon derivative key created");
                    key
                }
                Err(e) => {
                    tracing::error!(
                        "[swarm] Failed to create consciousness daemon key: {}. Daemon will be disabled.",
                        e
                    );
                    return None;
                }
            }
        } else {
            tracing::warn!(
                "[swarm] OR_MASTER_KEY not available — consciousness daemon requires it for derivative key creation"
            );
            return None;
        };

        match agent_cfg.model_provider {
            ModelProvider::OpenRouter => {
                Some(Arc::new(OpenRouterProvider {
                    client: self.client.clone(),
                    api_key: daemon_key,
                    model: model_id,
                    agent_id: "consciousness-daemon".to_string(),
                    agent_name: "Consciousness".to_string(),
                    llm_params: None,
                    context_window: Some(1_000_000),
                    max_completion_tokens: Some(4096),
                }))
            }
            ModelProvider::OpenGateway => {
                // OpenGateway is deprecated — route through OpenRouter
                tracing::warn!(
                    "[swarm] OpenGateway provider deprecated — consciousness daemon routing through OpenRouter"
                );
                Some(Arc::new(OpenRouterProvider {
                    client: self.client.clone(),
                    api_key: daemon_key,
                    model: model_id,
                    agent_id: "consciousness-daemon".to_string(),
                    agent_name: "Consciousness".to_string(),
                    llm_params: None,
                    context_window: Some(1_000_000),
                    max_completion_tokens: Some(4096),
                }))
            }
            ModelProvider::Ollama => Some(Arc::new(OllamaProvider {
                client: self.client.clone(),
                url: agent_cfg
                    .api_key
                    .clone()
                    .unwrap_or_else(|| "http://localhost:11434".to_string()),
                model: model_id,
                agent_id: "consciousness-daemon".to_string(),
                agent_name: "Consciousness".to_string(),
            })),
            _ => {
                tracing::warn!(
                    "[swarm] Consciousness daemon: unsupported provider {:?}, using default",
                    agent_cfg.model_provider
                );
                None
            }
        }
    }

    /// Get the consciousness daemon state handle (for gateway API).
    pub fn consciousness_state_handle(&self) -> Arc<AtomicU8> {
        self.consciousness_state.clone()
    }

    /// Gracefully shuts down the swarm, flushing all storage and cancelling agents.
    pub async fn shutdown(&self) -> Result<(), savant_core::error::SavantError> {
        tracing::info!("Swarm: Initiating graceful shutdown...");

        // Cancel consciousness daemon first (it holds an LLM connection)
        self.consciousness_shutdown.cancel();

        // Cancel all agents
        for entry in self.handles.iter() {
            let (id, (_, token)) = entry.pair();
            tracing::info!("Cancelling agent: {}", id);
            token.cancel();
        }

        // Abort all agent tasks (tokens already cancelled above)
        for entry in self.handles.iter() {
            let (id, (handle, _)) = entry.pair();
            handle.abort();
            tracing::info!("Agent {} abort signalled", id);
        }

        // Flush storage
        self.storage.shutdown()?;

        // Signal integrations shutdown
        let _ = self.integrations_shutdown_tx.send(true);

        // Shut down memory engine — cancels the background consolidation scheduler
        // so its Arc<MemoryEnclave> is dropped and vector database locks are released.
        self.engine.shutdown();

        tracing::info!("Swarm: Graceful shutdown complete");
        Ok(())
    }
}
