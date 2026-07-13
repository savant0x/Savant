// SAFETY: All `clippy::disallowed_methods` violations in this file originate from
// the `serde_json::json!()` macro, which internally uses `.unwrap()` on
// compile-time-validated JSON literals. A malformed JSON literal would be a
// compile error, making the panic path statically unreachable.
#![allow(clippy::disallowed_methods)]

use anyhow::{Context, Result};
use pqcrypto_dilithium::dilithium2;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::watch;
use tracing::{error, info, warn};

use crate::manager::AgentManager;
use crate::swarm::{SwarmConfig, SwarmController};
use crate::watcher::SwarmWatcher;
use savant_canvas::a2ui::CanvasManager;
use savant_canvas::types::CanvasElement;
use savant_core::bus::NexusBridge;
use savant_core::config::Config;
use savant_core::crypto::AgentKeyPair;
use savant_core::db::Storage;
use savant_core::pulse::watchdog::SovereignWatchdog;
use savant_gateway::server::start_gateway;
use savant_obsidian::{ColdStorageManager, OutboxWorker, VaultWatcher, VaultWriter};

/// Kills any process using the specified port.
/// On Windows, uses PowerShell. On Unix, uses lsof.
/// Best-effort: logs warnings on failure but doesn't block startup.
async fn kill_port_process(port: u16) {
    // Check if port is in use
    if std::net::TcpListener::bind(("127.0.0.1", port)).is_ok() {
        return; // Port is free, nothing to do
    }

    warn!("[ignition] Port {} is in use — attempting to free it", port);

    #[cfg(target_os = "windows")]
    {
        if let Err(e) = tokio::process::Command::new("powershell")
            .args([
                "-Command",
                &format!(
                    "Get-NetTCPConnection -LocalPort {} -ErrorAction SilentlyContinue | ForEach-Object {{ Stop-Process -Id $_.OwningProcess -Force }}",
                    port
                ),
            ])
            .output()
            .await
        {
            warn!("[ignition] Failed to execute port cleanup on Windows: {}", e);
        }
    }

    #[cfg(not(target_os = "windows"))]
    {
        if let Err(e) = tokio::process::Command::new("sh")
            .args([
                "-c",
                &format!("lsof -ti :{} | xargs kill -9 2>/dev/null", port),
            ])
            .output()
            .await
        {
            warn!("[ignition] Failed to execute port cleanup on Unix: {}", e);
        }
    }

    // Wait a moment for the process to release the port
    tokio::time::sleep(std::time::Duration::from_secs(1)).await;

    if std::net::TcpListener::bind(("127.0.0.1", port)).is_ok() {
        info!("[ignition] Port {} freed successfully", port);
    } else {
        warn!(
            "[ignition] Port {} still in use after cleanup attempt",
            port
        );
    }
}

/// 🧬 Savant Ignition Outcome
/// Holds the live handlers for the entire swarm ecosystem.
pub struct SwarmIgnition {
    pub config: Config,
    pub nexus: Arc<NexusBridge>,
    pub storage: Arc<Storage>,
    pub swarm: Arc<SwarmController>,
    pub manager: Arc<AgentManager>,
    /// RC-24: Obsidian shutdown senders — must be kept alive until shutdown
    pub obsidian_shutdown_txs: Vec<tokio::sync::watch::Sender<bool>>,
    /// RC-20/21: Task handles for supervision
    pub gateway_handle: tokio::task::JoinHandle<()>,
    pub swarm_handle: tokio::task::JoinHandle<()>,
    /// BootstrapReconciler shutdown sender and handle
    pub reconciler_shutdown_tx: tokio::sync::watch::Sender<bool>,
    pub reconciler_handle: tokio::task::JoinHandle<()>,
    /// Live scheduler instance for runtime schedule management
    pub scheduler: Arc<savant_core::heartbeat::HeartbeatScheduler>,
    /// Panopticon replay recorder for agent reasoning trace
    pub replay_recorder: Arc<savant_panopticon::replay::ReplayRecorder>,
    /// Sandbox hypervisor backend for agent isolation
    pub hypervisor: Box<dyn savant_sandbox::vmm::AgentHypervisor>,
    /// SovereignWatchdog for substrate health monitoring
    pub watchdog: SovereignWatchdog,
    /// Canvas A2UI manager for real-time agent state visualization
    pub canvas_manager: Arc<CanvasManager>,
}

impl SwarmIgnition {
    /// Gracefully shuts down the entire swarm ecosystem.
    ///
    /// Order of operations:
    /// 1. Signal Obsidian vault workers to stop (so they release enclave handles)
    /// 2. Cancel all agents and flush storage via SwarmController
    /// 3. Stop the HeartbeatScheduler
    /// 4. Abort the gateway and swarm watcher tasks
    /// 5. Shut down the memory engine (releases vector database lock)
    pub async fn shutdown(&self) {
        info!("=== SWARM SHUTDOWN STARTED ===");

        // 1. Signal Obsidian vault workers to stop (they hold Arc<MemoryEnclave>)
        for tx in &self.obsidian_shutdown_txs {
            let _ = tx.send(true);
        }
        info!("[shutdown] Obsidian workers signalled");

        // 1.5. Signal BootstrapReconciler to stop
        if self.reconciler_shutdown_tx.send(true).is_err() {
            tracing::warn!("[shutdown] BootstrapReconciler already stopped");
        }

        // 2. Cancel all agents and flush storage via SwarmController
        if let Err(e) = self.swarm.shutdown().await {
            warn!("[shutdown] SwarmController shutdown error: {}", e);
        }
        info!("[shutdown] Swarm controller shut down");

        // 3. Abort gateway and swarm watcher tasks
        self.gateway_handle.abort();
        self.swarm_handle.abort();
        info!("[shutdown] Gateway and swarm tasks aborted");

        // 4. Give background tasks a moment to release their Arc<MemoryEnclave> refs
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;

        info!("=== SWARM SHUTDOWN COMPLETE ===");
    }
}

/// Inject keyring-stored API keys into the process environment.
///
/// # Safety
/// This function calls `std::env::set_var`, which is not thread-safe.
/// It MUST be called exactly once during `ignite()` BEFORE any provider
/// initialization or Tokio task spawning. At this point in the startup
/// sequence no concurrent readers exist — providers read env vars after
/// agent discovery, which happens later in `ignite()`.
///
/// # Why set_var is necessary
/// Some providers (OpenRouter) read API keys from environment variables.
/// Users who store keys in the OS keyring (via `load_secret()`) need this
/// bridge to inject those keys into the runtime. Without this, keyring-stored
/// keys would be invisible to providers that only check `std::env::var()`.
fn inject_keyring_secrets() {
    if let Some(openrouter_key) = savant_core::config::load_secret("OPENROUTER_API_KEY") {
        info!("🔑 OpenRouter API key loaded from keyring");
        // SAFETY: Called once during ignite() before provider init. No concurrent readers.
        std::env::set_var("OPENROUTER_API_KEY", &openrouter_key);
    }
    // Also check for master key
    if let Some(master_key) = savant_core::config::load_secret("OR_MASTER_KEY") {
        info!("🔑 OpenRouter master key loaded from keyring");
        // SAFETY: Called once during ignite() before provider init. No concurrent readers.
        std::env::set_var("OR_MASTER_KEY", &master_key);
    }
}

/// Check configured provider reachability at startup.
/// If the primary provider is unreachable, falls back to `openrouter/free`
/// and updates the in-memory config.
/// Does NOT block startup — providers are checked on first use regardless.
async fn check_provider_reachability(config: &mut savant_core::config::Config) {
    let provider = &config.ai.provider.clone();
    let model = &config.ai.model.clone();

    // Migrate stale OpenGateway configs to OpenRouter defaults
    // Detect stale OpenGateway or Mimo model configs (both hyphen and space variants)
    let is_stale_provider = provider == "opengateway";
    let is_stale_model = model.to_lowercase().starts_with("mimo-")
        || model.to_lowercase().starts_with("mimo ")
        || model.to_lowercase().starts_with("opengateway/");
    if is_stale_provider || is_stale_model {
        let migration_target = config.ai.model.clone();
        warn!(
            "[ignition] Config has stale provider '{}' / model '{}' — migrating to tencent/hy3:free",
            provider, model
        );
        config.ai.provider = "openrouter".to_string();
        config.ai.model = "tencent/hy3:free".to_string();
        // Also clear manifestation_model if it was set to a stale value
        if let Some(ref mm) = config.ai.manifestation_model {
            let mm_lower = mm.to_lowercase();
            if mm_lower.starts_with("stepfun")
                || mm_lower.starts_with("mimo-")
                || mm_lower.starts_with("mimo ")
                || mm_lower.starts_with("opengateway/")
            {
                let stale_mm = mm.clone();
                config.ai.manifestation_model = None;
                warn!("[ignition] Cleared stale manifestation_model '{}' — will inherit from chat model", stale_mm);
            }
        }
        info!("[ignition] Migrated from stale '{}' to OpenRouter default (tencent/hy3:free)", migration_target);

        // Persist migration to disk so it doesn't re-read the stale model on next restart.
        // Save to primary config path and ~/.savant/savant.toml as global fallback.
        let primary_path = Config::primary_config_path();
        if let Err(e) = config.save(&primary_path) {
            warn!("[ignition] Failed to persist migrated config to {:?}: {}", primary_path, e);
        } else {
            info!("[ignition] Migrated config saved to {:?}", primary_path);
        }
        let home = std::env::var("USERPROFILE")
            .or_else(|_| std::env::var("HOME"))
            .unwrap_or_else(|_| ".".to_string());
        let global_path = PathBuf::from(&home).join(".savant").join("savant.toml");
        if global_path != primary_path {
            if let Err(e) = config.save(&global_path) {
                warn!("[ignition] Failed to sync migrated config to {:?}: {}", global_path, e);
            } else {
                info!("[ignition] Migrated config synced to {:?}", global_path);
            }
        }
    }
    // Re-bind local variables after migration so subsequent logs show current values
    let provider = &config.ai.provider;
    let model = &config.ai.model;

    if config.ai.provider != "openrouter" {
        // Non-OpenRouter providers (e.g., ollama) are checked by the setup wizard
        return;
    }

    // Check OpenRouter reachability
    let client = match savant_core::net::secure_client_with_timeout(5, 5) {
        Ok(c) => c,
        Err(e) => {
            warn!("[ignition] Failed to create HTTP client for provider check: {}", e);
            return;
        }
    };

    let api_key = std::env::var("OPENROUTER_API_KEY")
        .or_else(|_| std::env::var("OR_MASTER_KEY"))
        .unwrap_or_default();

    let mut request = client.get("https://openrouter.ai/api/v1/models");
    if !api_key.is_empty() {
        request = request.header("Authorization", format!("Bearer {}", api_key));
    }

    match request.send().await {
        Ok(resp) if resp.status().is_success() => {
            info!(
                "[ignition] Provider '{}' is reachable (model: {})",
                provider, model
            );
        }
        Ok(resp) => {
            let status = resp.status();
            if status.as_u16() == 401 || status.as_u16() == 403 {
                // Auth failure — bad key or no key.
                // Do NOT overwrite config.ai.model — the default (tencent/hy3:free) is
                // already correct, and overwriting to openrouter/free poisons the
                // manifestation model fallback since manifestation_model defaults to None
                // (inherits from chat model). The user can change models via dashboard settings.
                warn!(
                    "[ignition] Provider '{}' returned HTTP {} — model '{}' may fail. Configure API key via settings.",
                    provider, status, model
                );
            } else {
                warn!(
                    "[ignition] Provider '{}' returned HTTP {} — {} may be degraded",
                    provider, status, model
                );
            }
        }
        Err(e) => {
            warn!(
                "[ignition] Provider '{}' unreachable: {}. Model '{}' may fail.",
                provider, e, model
            );
        }
    }
}

/// 🚀 Swarm Ignition Service
/// Orchestrates the complex startup sequence for the Savant environment.
pub struct IgnitionService;

impl IgnitionService {
    /// Ignites the swarm using the provided configuration.
    pub async fn ignite(config_path: Option<&str>) -> Result<SwarmIgnition> {
        info!("🧬 Initializing Savant Swarm Substrate...");

        // 1. Configuration
        let mut config =
            Config::load_from(config_path, None).context("Failed to load configuration")?;
        info!(
            "✅ Configuration substrate successfully initialized at {}",
            config.project_root.display()
        );

        // 2. Crypto
        let master_key = AgentKeyPair::ensure_master_key().context("Master key failure")?;
        let root_authority = master_key
            .get_verifying_key()
            .context("Failed to derive root authority")?;
        let signing_key = master_key
            .get_signing_key()
            .context("Failed to derive signing key")?;
        let (pqc_authority, pqc_signing_key) = dilithium2::keypair();
        info!("🔐 Cryptographic identity established");

        // 2.5. Tri-Enclave Attestation
        let attestation_manager = savant_security::attestation::AttestationManager;
        let state_hash = {
            use sha2::Digest;
            let mut hasher = sha2::Sha256::new();
            hasher.update(root_authority.as_bytes());
            // Include the ed25519 signing key as entropy for attestation
            hasher.update(signing_key.to_bytes());
            let result = hasher.finalize();
            let mut hash = [0u8; 32];
            hash.copy_from_slice(&result);
            hash
        };
        match attestation_manager.attest_state(state_hash).await {
            Ok(result) => {
                if result.has_consensus() {
                    info!("✅ Tri-Enclave Attestation: Consensus REACHED");
                } else {
                    warn!("⚠️ Tri-Enclave Attestation: Consensus NOT reached. Running in degraded mode.");
                }
            }
            Err(e) => {
                warn!(
                    "⚠️ Attestation check failed: {}. Continuing without attestation.",
                    e
                );
            }
        }

        // 2.6. Sync Threat Intelligence (background — non-blocking)
        let _intel_handle = tokio::spawn(async {
            let intel_result = savant_skills::security::sync_threat_intelligence().await;
            if intel_result.success {
                info!(
                    "🛡️ Threat intelligence synced: {} hashes, {} names, {} domains",
                    intel_result.hashes_synced,
                    intel_result.names_synced,
                    intel_result.domains_synced
                );
            } else if let Some(err) = &intel_result.error {
                if err.contains("401") || err.contains("Unauthorized") {
                    tracing::debug!(
                        "Threat intelligence sync skipped (auth not configured): {}",
                        err
                    );
                } else {
                    tracing::warn!("Threat intelligence sync failed: {}", err);
                }
            }
        });

        // 3+4. Event Bus + Storage (parallel initialization)
        let db_path = config.resolve_path(&config.system.db_path);
        let (nexus_result, storage_result) = tokio::join!(
            async {
                let nexus = Arc::new(NexusBridge::new());
                info!("🌐 Nexus event bus operational");
                nexus
            },
            async {
                info!("💾 Synchronizing storage at: {}", db_path.display());
                Storage::with_defaults(db_path).context("Storage initialization failed")
            }
        );
        let nexus = nexus_result;
        let storage = Arc::new(storage_result?);

        // 4.5. Ghost-Restore: Database integrity check and recovery
        if let Err(e) = storage.ghost_restore() {
            warn!(
                "[ignition] Ghost-restore failed (non-fatal): {}. Continuing with potentially degraded storage.",
                e
            );
        } else {
            info!("✅ Storage ghost-restore completed successfully");
        }

        // 4.6. Load swarm history for coordination context
        match storage.get_swarm_history(100) {
            Ok(history) => {
                info!(
                    "📜 Loaded {} swarm history entries for coordination context",
                    history.len()
                );
            }
            Err(e) => {
                warn!("[ignition] Failed to load swarm history: {}", e);
            }
        }

        // 4.7. Load secrets from keyring for API key resolution
        inject_keyring_secrets();

        // 4.8. Provider reachability check — ping the configured provider
        // If unreachable, fall back to openrouter/free in memory
        check_provider_reachability(&mut config).await;

        // 5. Agent Discovery
        let manager = Arc::new(AgentManager::new(config.clone()));
        info!("🔍 Starting agent discovery sequence...");
        let discovered_agents = manager
            .discover_agents()
            .await
            .context("Agent discovery failed")?;

        let mut agent_metadata = Vec::new();
        if discovered_agents.is_empty() {
            warn!("🔍 No agents found in workspace clusters. Check your workspaces directory.");
        } else {
            info!(
                "✅ Discovered {} agents for deployment",
                discovered_agents.len()
            );
            for a in &discovered_agents {
                info!("   - Agent: {} ({})", a.agent_name, a.agent_id);
                agent_metadata.push(serde_json::json!({
                    "id": a.agent_id,
                    "name": a.agent_name,
                    "status": "Active",
                    "role": "Agent",
                    "image": a.identity.as_ref().and_then(|i| i.image.clone())
                }));
            }
        }

        // 5.1. Canvas A2UI — push discovered agents for real-time visualization
        let canvas_manager = Arc::new(CanvasManager::new(1000));
        {
            let canvas_elements: Vec<CanvasElement> = discovered_agents
                .iter()
                .map(|a| {
                    let mut properties = std::collections::HashMap::new();
                    properties.insert("name".to_string(), a.agent_name.clone());
                    properties.insert("status".to_string(), "Active".to_string());
                    properties.insert("role".to_string(), "Agent".to_string());
                    if let Some(image) = a.identity.as_ref().and_then(|i| i.image.as_ref()) {
                        properties.insert("image".to_string(), image.to_string());
                    }
                    CanvasElement {
                        id: a.agent_id.clone(),
                        element_type: "agent".to_string(),
                        properties,
                    }
                })
                .collect();
            if let Err(e) = canvas_manager.update_elements(canvas_elements).await {
                warn!("[ignition] Failed to push agents to CanvasManager: {}", e);
            } else {
                info!(
                    "[ignition] CanvasManager hydrated with {} agents",
                    discovered_agents.len()
                );
            }
        }

        // Sync initial discovery state to bus
        let discovery_event = serde_json::json!({
            "status": "SWARM_IGNITED",
            "agents": agent_metadata
        });

        // Populate system.agents in shared memory and publish event
        nexus
            .update_state("system.agents".to_string(), discovery_event.to_string())
            .await;
        if let Err(e) = nexus
            .publish("agents.discovered", &discovery_event.to_string())
            .await
        {
            tracing::debug!(
                "[agent::ignition] No subscribers for agents.discovered event (expected during init): {}",
                e
            );
        }

        // 5.5. Create replay recorder early so SwarmController can use it
        let replay_recorder = Arc::new(savant_panopticon::replay::ReplayRecorder::new(
            config.telemetry.replay_max_events,
        ));

        // 6. Swarm Controller
        let project_root = config.resolve_path(&config.system.agents_path);

        // 6a. Initialize SchemaIndex (code intelligence) — non-fatal
        //     SchemaIndex::open() creates .savant/schema/code.db internally
        let schema_index = match savant_schema::SchemaIndex::open(project_root.clone()) {
            Ok(idx) => {
                info!("📊 SchemaIndex opened — code intelligence available");
                Some(Arc::new(idx))
            }
            Err(e) => {
                warn!(
                    "[ignition] SchemaIndex init failed (code intelligence disabled): {}",
                    e
                );
                None
            }
        };

        // 6b. Initialize LSP Manager — non-fatal (logs discovery count internally)
        let lsp_manager = Some(Arc::new(crate::lsp::LspManager::new(project_root.clone())));

        let swarm_config = SwarmConfig {
            workspace_root: project_root,
            memory_db_path: config.resolve_path("./data/memory"),
            skills_path: config.resolve_path("./skills"),
            blackboard_name: "savant_swarm".into(),
            collective_name: "savant_collective".into(),
            config_file: Some(
                config_path
                    .map(std::path::PathBuf::from)
                    .unwrap_or_else(Config::primary_config_path),
            ),
            privacy: config.privacy.clone(),
            trajectory: config.trajectory.clone(),
            embedding_model: config.browser.embedding_model.clone(),
        };

        let swarm = SwarmController::new(
            swarm_config,
            discovered_agents,
            storage.clone(),
            manager.clone(),
            nexus.clone(),
            root_authority,
            signing_key,
            pqc_authority,
            pqc_signing_key,
            config.mcp.servers.clone(),
            replay_recorder.clone(),
            config.integrations.clone(),
            schema_index,
            lsp_manager,
        )
        .await
        .context("Swarm Controller ignition failed")?;

        let swarm = Arc::new(swarm);
        info!("🚀 Swarm Controller online and synchronized");

        // 6.5. Initialize CompactEngine (L1/L2/OCEAN/HNSW compression)
        let compact_user_rules = config.resolve_path("./config/compact-rules");
        let compact_project_rules = config.resolve_path("./.savant/compact-rules");
        crate::compact::integration::init(
            compact_user_rules.clone(),
            compact_project_rules.clone(),
        )
        .await;
        info!("📦 CompactEngine initialized (L1 rules + L2 semantic + OCEAN + HNSW)");

        // NA-21: Watch compact rules directories for hot-reload
        {
            use notify_debouncer_mini::{new_debouncer, notify::*};
            let (reload_tx, mut reload_rx) = tokio::sync::mpsc::channel::<()>(4);
            let mut debouncer = match new_debouncer(
                std::time::Duration::from_millis(2000),
                move |res: notify_debouncer_mini::DebounceEventResult| {
                    if let Ok(events) = res {
                        if !events.is_empty() {
                            let _ = reload_tx.blocking_send(());
                        }
                    }
                },
            ) {
                Ok(d) => d,
                Err(e) => {
                    tracing::warn!("[ignition] Failed to create compact rules debouncer: {}", e);
                    // Continue without hot-reload — non-fatal.
                    // Fallback debouncer with a no-op callback; this constructor is infallible
                    // for its minimal configuration (static duration + empty callback).
                    #[allow(clippy::disallowed_methods)]
                    {
                        new_debouncer(
                            std::time::Duration::from_millis(2000),
                            move |_: notify_debouncer_mini::DebounceEventResult| {},
                        )
                        .expect("fallback debouncer: static config is always valid")
                    }
                }
            };
            for dir in &[&compact_user_rules, &compact_project_rules] {
                if dir.exists() {
                    if let Err(e) = debouncer.watcher().watch(dir, RecursiveMode::Recursive) {
                        tracing::warn!(
                            "[ignition] Failed to watch compact rules dir {}: {}",
                            dir.display(),
                            e
                        );
                    }
                }
            }
            // Keep debouncer alive for the duration of the process
            let _debouncer = debouncer;
            tokio::spawn(async move {
                while reload_rx.recv().await.is_some() {
                    tracing::info!("[compact] Rules directory changed — reloading");
                    crate::compact::integration::reload_rules().await;
                }
            });
        }

        // 7. Obsidian Vault Projection Worker (always active by default)
        let mut obsidian_shutdown_txs = Vec::new();
        if config.obsidian.enabled {
            let obsidian_config = config.obsidian.clone();
            let obsidian_enclave = swarm.engine().enclave();
            let obsidian_nexus = nexus.clone();
            let agents_path = config.resolve_path(&config.system.agents_path);

            let vault_path = obsidian_config.resolved_vault_path(&agents_path);

            // Outbox worker owns VaultWriter + ColdStorageManager
            let writer = VaultWriter::new(
                vault_path.clone(),
                Some(Arc::clone(&obsidian_enclave)),
                obsidian_config.clone(),
                "savant".to_string(),
            );
            let cold_storage = ColdStorageManager::new(vault_path.clone(), obsidian_config.clone());
            let (outbox_shutdown_tx, outbox_shutdown_rx) = watch::channel(false);
            let outbox = OutboxWorker::new(
                vault_path.clone(),
                writer,
                cold_storage,
                obsidian_config.clone(),
                Some(Arc::clone(&obsidian_enclave)),
                agents_path.clone(),
                outbox_shutdown_rx,
            );
            let outbox_handle = tokio::spawn({
                let vault_path = vault_path.clone();
                async move {
                    info!(
                        "[obsidian] Vault projection worker starting at {}",
                        vault_path.display()
                    );
                    outbox.run().await;
                }
            });

            // Vault watcher owns its own reference to enclave + nexus
            let (watcher_shutdown_tx, watcher_shutdown_rx) = watch::channel(false);
            let watcher = VaultWatcher::new(
                vault_path.clone(),
                obsidian_config,
                Some(Arc::clone(&obsidian_nexus)),
                Some(Arc::clone(&obsidian_enclave)),
                watcher_shutdown_rx,
            );
            let watcher_handle = tokio::spawn({
                let vault_path = vault_path.clone();
                async move {
                    info!(
                        "[obsidian] Vault watcher starting at {}",
                        vault_path.display()
                    );
                    if let Err(e) = watcher.run().await {
                        warn!("[obsidian] Vault watcher error: {e}");
                    }
                }
            });

            // RC-24: Store shutdown senders in the struct for proper lifecycle management
            obsidian_shutdown_txs.push(outbox_shutdown_tx);
            obsidian_shutdown_txs.push(watcher_shutdown_tx);
            let _outbox_handle = outbox_handle;
            let _watcher_handle = watcher_handle;

            info!(
                "[obsidian] Vault projection workers enabled at {}",
                vault_path.display()
            );
        }

        // 7b. HeartbeatScheduler — cron-based scheduling
        let scheduler = {
            let sched = savant_core::heartbeat::HeartbeatScheduler::new().await?;
            sched.register_evolve_pulse().await?;
            sched.register_weekly_digest().await?;
            // Register all persisted user-defined schedules
            let persisted_count = sched.register_all_persisted().await?;
            sched.start().await?;
            let mut rx = sched.subscribe();
            let nexus_for_scheduler = nexus.clone();
            tokio::spawn(async move {
                while let Ok(cmd) = rx.recv().await {
                    tracing::info!("[scheduler] Cron event: {}", cmd);
                    if let Err(e) = nexus_for_scheduler.publish("pulse.trigger", &cmd).await {
                        tracing::warn!("[scheduler] Failed to publish cron event: {}", e);
                    }
                }
            });
            info!(
                "⏰ HeartbeatScheduler started (evolve_pulse + weekly_digest + {} persisted)",
                persisted_count
            );
            Arc::new(sched)
        };

        // 7b.1. SovereignWatchdog — substrate health monitoring
        let mut watchdog = SovereignWatchdog::new();
        let _watchdog_handle = watchdog.attach(&scheduler).await;
        info!("🐕 SovereignWatchdog attached to HeartbeatScheduler");

        // 7b.2. Config hot-reload watcher
        let config_lock = Arc::new(tokio::sync::RwLock::new(config.clone()));
        let config_path = Config::primary_config_path();
        if let Err(e) = Config::watch(config_lock.clone(), config_path) {
            warn!(
                "[ignition] Config watcher failed to start (non-fatal): {}",
                e
            );
        } else {
            info!("👁️ Config hot-reload watcher active");
        }

        // 7c. BootstrapReconciler — listens for scaffold events and executes idempotent handlers
        let (reconciler_shutdown_tx, reconciler_shutdown_rx) = tokio::sync::watch::channel(false);
        let reconciler = savant_core::bootstrap::BootstrapReconciler::new(
            nexus.clone(),
            config.resolve_path(&config.system.agents_path),
        );
        let reconciler_handle = tokio::spawn(async move {
            reconciler.start(reconciler_shutdown_rx).await;
        });
        info!("🔧 BootstrapReconciler active — listening for scaffold events");

        // 7d. Initialize gateway scheduler handler so REST endpoints work
        savant_gateway::handlers::schedules::init_scheduler(scheduler.clone()).await;

        // 7d. Panopticon — distributed telemetry (replay recorder created in step 5.5)
        if config.telemetry.panopticon_enabled {
            match savant_panopticon::init_panopticon("savant", &config.telemetry.otlp_endpoint) {
                Ok(_) => info!(
                    "🔭 Panopticon telemetry initialized → {}",
                    config.telemetry.otlp_endpoint
                ),
                Err(e) => warn!(
                    "⚠️ Panopticon init failed (non-fatal): {}. Continuing without OTLP export.",
                    e
                ),
            }
        } else {
            info!("🔭 Panopticon disabled (set telemetry.panopticon_enabled=true to enable)");
        }

        // 7e. Sandbox — select best available hypervisor backend
        let hypervisor = savant_sandbox::vmm::select_backend().await;
        info!(
            "🔒 Sandbox hypervisor selected: {}",
            hypervisor.backend_name()
        );

        // 8. Gateway (Async Background)
        // Kill any stale process on the gateway port before starting
        kill_port_process(config.server.port).await;
        let g_config = config.clone();
        let g_nexus = nexus.clone();
        let g_storage = storage.clone();
        let g_canvas = canvas_manager.clone();
        let g_echo_metrics = Arc::new(savant_echo::ComponentMetrics::new(0.05, 100));
        // RC-20: Store gateway JoinHandle for supervision
        let gateway_handle = tokio::spawn(async move {
            if let Err(e) =
                start_gateway(g_config, g_nexus, g_storage, g_echo_metrics, g_canvas).await
            {
                error!("❌ Gateway crash: {}", e);
            }
        });

        // 8a. MCP Server (exposes local tools via Model Context Protocol)
        if config.mcp.server_port > 0 {
            let mcp_port = config.mcp.server_port;
            let skills_path = config
                .resolve_path(&config.system.agents_path)
                .parent()
                .map(|p| p.join("skills"))
                .unwrap_or_else(|| std::path::PathBuf::from("skills"));
            let mcp_registry = Arc::new(tokio::sync::RwLock::new({
                let mut reg = savant_skills::parser::SkillRegistry::new();
                if let Err(e) = reg.discover_skills(&skills_path).await {
                    tracing::warn!("[ignition] MCP server skill discovery failed: {}", e);
                }
                reg
            }));
            // Start MCP server — try auth first, fall back to no-auth
            let mcp_server = match std::env::var("SAVANT_MCP_AUTH_TOKEN") {
                Ok(token) => {
                    let mut mcp_tokens = std::collections::HashMap::new();
                    let token_hash = blake3::hash(token.as_bytes()).to_hex().to_string();
                    mcp_tokens.insert(token_hash, "env-configured".to_string());
                    Arc::new(savant_mcp::server::McpServer::with_auth(
                        mcp_registry,
                        mcp_tokens,
                    ))
                }
                Err(_) => Arc::new(savant_mcp::server::McpServer::new(mcp_registry)),
            };
            let mcp_addr = format!("127.0.0.1:{}", mcp_port);
            let mcp_addr_display = mcp_addr.clone();
            tokio::spawn(async move {
                if let Err(e) = mcp_server.start(&mcp_addr).await {
                    tracing::error!("❌ MCP Server crash: {}", e);
                }
            });
            info!("🔌 MCP Server starting on {}", mcp_addr_display);
        }

        // 8b. Swarm Ignition & Watcher
        let s_swarm = swarm.clone();
        let s_manager = manager.clone();
        let s_nexus = nexus.clone();
        // RC-21: Store swarm JoinHandle for supervision
        let swarm_handle = tokio::spawn(async move {
            s_swarm.ignite().await;
            let watcher = Arc::new(SwarmWatcher::new(s_swarm, s_manager, s_nexus));
            if let Err(e) = watcher.start().await {
                error!("🔭 SwarmWatcher error: {}", e);
            }
        });

        Ok(SwarmIgnition {
            config,
            nexus,
            storage,
            swarm,
            manager,
            obsidian_shutdown_txs,
            gateway_handle,
            swarm_handle,
            reconciler_shutdown_tx,
            reconciler_handle,
            scheduler,
            replay_recorder,
            hypervisor,
            watchdog,
            canvas_manager,
        })
    }
}
