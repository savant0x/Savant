use crate::error::SavantError;
use figment::{
    providers::{Env, Format, Toml},
    Figment,
};
use notify::{Event, RecursiveMode, Watcher};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::sync::RwLock;

/// Main Savant configuration
/// Loaded from config/savant.toml (project) or ~/.savant/savant.toml (global)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub ai: AiConfig,
    pub server: ServerConfig,
    pub swarm: SwarmConfig,
    pub channels: ChannelsConfig,
    pub skills: SkillsConfig,
    pub memory: MemoryConfig,
    pub security: SecurityConfig,
    pub wasm: WasmConfig,
    pub system: SystemConfig,
    pub telemetry: TelemetryConfig,
    pub mcp: McpConfig,
    pub evolution: EvolutionConfig,
    #[serde(default)]
    pub obsidian: ObsidianConfig,
    #[serde(default)]
    pub browser: BrowserConfig,
    #[serde(skip)]
    pub project_root: PathBuf,
    #[serde(default)]
    pub proactive: ProactiveConfig,
    #[serde(default)]
    pub privacy: PrivacyConfig,
    #[serde(default)]
    pub trajectory: TrajectoryConfig,
    #[serde(default)]
    pub resource_governor: ResourceGovernorConfig,
    #[serde(default)]
    pub integrations: IntegrationsConfig,
}

#[allow(clippy::derivable_impls)]
impl Default for Config {
    fn default() -> Self {
        Self {
            ai: AiConfig::default(),
            server: ServerConfig::default(),
            swarm: SwarmConfig::default(),
            channels: ChannelsConfig::default(),
            skills: SkillsConfig::default(),
            memory: MemoryConfig::default(),
            security: SecurityConfig::default(),
            wasm: WasmConfig::default(),
            system: SystemConfig::default(),
            telemetry: TelemetryConfig::default(),
            mcp: McpConfig::default(),
            evolution: EvolutionConfig::default(),
            obsidian: ObsidianConfig::default(),
            browser: BrowserConfig::default(),
            project_root: PathBuf::from("."),
            proactive: ProactiveConfig::default(),
            privacy: PrivacyConfig::default(),
            trajectory: TrajectoryConfig::default(),
            resource_governor: ResourceGovernorConfig::default(),
            integrations: IntegrationsConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AiConfig {
    pub provider: String,
    pub model: String,
    pub manifestation_model: Option<String>,
    pub temperature: f32,
    pub top_p: f32,
    pub frequency_penalty: f32,
    pub presence_penalty: f32,
    pub max_tokens: u32,
    pub system_prompt: Option<String>,
    pub manifestation_system_prompt: Option<String>,
    /// Base URL for local providers (e.g., Ollama at http://localhost:11434)
    #[serde(default)]
    pub base_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SwarmConfig {
    pub heartbeat_interval: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    pub port: u16,
    pub host: String,
    pub max_connections: usize,
    pub lane_capacity: usize,
    pub max_lane_concurrency: usize,
    pub dashboard_api_key: Option<String>,
    /// Allowed CORS origins. Loaded from SAVANT_CORS_ORIGINS env var (comma-separated)
    /// or from config file. Defaults to localhost:3000 if empty.
    #[serde(default)]
    pub cors_origins: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelsConfig {
    pub discord: ChannelEntry,
    pub telegram: ChannelEntry,
    pub whatsapp: ChannelEntry,
    pub matrix: ChannelEntry,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChannelEntry {
    pub enabled: bool,
    pub token: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillsConfig {
    pub path: String,
    pub enable_clawhub: bool,
    pub auto_update: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryConfig {
    pub base_path: String,
    pub cache_size_mb: u32,
    pub consolidation_threshold: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecurityConfig {
    pub enable_blocklist_sync: bool,
    pub threat_intel_sync_interval_secs: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WasmConfig {
    pub max_instances: u32,
    pub fuel_limit: u64,
    pub memory_limit_mb: u32,
    pub enable_cache: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemConfig {
    pub db_path: String,
    pub substrate_path: String,
    pub agents_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TelemetryConfig {
    pub log_level: String,
    pub log_color: bool,
    pub enable_tracing: bool,
    /// Enable Panopticon distributed telemetry (OpenTelemetry OTLP export)
    #[serde(default)]
    pub panopticon_enabled: bool,
    /// OTLP collector endpoint (e.g., "http://localhost:4317")
    #[serde(default = "default_otlp_endpoint")]
    pub otlp_endpoint: String,
    /// Maximum replay events to retain in memory
    #[serde(default = "default_replay_max_events")]
    pub replay_max_events: usize,
}

fn default_otlp_endpoint() -> String {
    "http://localhost:4317".to_string()
}

fn default_replay_max_events() -> usize {
    10_000
}

/// MCP (Model Context Protocol) configuration.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct McpConfig {
    /// List of MCP server endpoints to connect to on startup
    pub servers: Vec<McpServerEntry>,
    /// Port for the local MCP server to listen on (0 = disabled)
    #[serde(default)]
    pub server_port: u16,
}

/// A single MCP server entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpServerEntry {
    /// Human-readable name
    pub name: String,
    /// WebSocket URL (e.g., "ws://localhost:3001/mcp")
    pub url: String,
    /// Optional auth token
    pub auth_token: Option<String>,
}

impl AiConfig {
    /// Returns the inline system prompt or an empty string.
    pub fn resolved_system_prompt(&self) -> String {
        self.system_prompt.clone().unwrap_or_default()
    }
}

impl Config {
    /// Attempts to load a legacy OpenClaw config and migrate it to the current format.
    /// Returns `Some(Config)` if migration succeeds, `None` if no legacy config found.
    pub fn try_migrate_legacy(path: &std::path::Path) -> Option<Self> {
        let content = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!("Legacy config read failed for {:?}: {}", path, e);
                return None;
            }
        };
        let legacy: crate::migration::LegacyOpenClawConfig = match serde_json::from_str(&content) {
            Ok(l) => l,
            Err(e) => {
                tracing::warn!("Legacy config parse failed for {:?}: {}", path, e);
                return None;
            }
        };
        let agent_config: crate::types::AgentConfig = legacy.into();
        // Build a minimal Config from the migrated agent config
        let mut config = Config::default();
        config.ai.model = agent_config.model.unwrap_or_default();
        config.ai.system_prompt = Some(agent_config.system_prompt);
        config.project_root = agent_config.workspace_path;
        Some(config)
    }
}

// ============================================================================
// Defaults
// ============================================================================

impl ObsidianConfig {
    /// Returns the resolved vault path, defaulting to `{workspace_root}/memory-vault/`
    pub fn resolved_vault_path(&self, workspace_root: &std::path::Path) -> std::path::PathBuf {
        self.vault_path
            .as_ref()
            .map(|p| {
                let pb = std::path::PathBuf::from(p);
                if pb.is_relative() {
                    workspace_root.join(&pb)
                } else {
                    pb
                }
            })
            .unwrap_or_else(|| workspace_root.join("memory-vault"))
    }
}

impl Default for AiConfig {
    fn default() -> Self {
        Self {
            // Dev default: tencent/hy3:free — specific free model
            // Public release: switch this to openrouter/free (auto-router)
            provider: "openrouter".to_string(),
            model: "tencent/hy3:free".to_string(),
            manifestation_model: None,
            temperature: 0.7,
            top_p: 1.0,
            frequency_penalty: 0.0,
            presence_penalty: 0.0,
            max_tokens: 4096,
            system_prompt: Some("You are the Savant Substrate. Operate with absolute sovereignty and technical precision.".to_string()),
            manifestation_system_prompt: Some(r#"You are the Savant Soul Manifestation Engine — a AAA-tier identity architect.
Your task is to generate a complete, high-density SOUL.md file based on the user's prompt.
This is a SOVEREIGN DOCUMENT. It must be between 300 and 500 lines long.

MANDATORY AAA STRUCTURE:
1.  **Entity Identity & Designation** — Archetype, version, primary role.
2.  **Systemic Core & Origin** — The narrative of the agent's birth within the Savant Substrate.
3.  **Psychological Matrix (AIEOS Mapping)** — OCEAN traits, cognitive biases, moral compass.
4.  **Strategic Maxims** — 30+ core operating principles (e.g., "Complexity is a Tax").
5.  **Linguistic Architecture** — Voice principles, presence, BANNED filler words.
6.  **Zero-Trust Execution Substrate** — Security boundaries, CCT integration, WASM constraints.
7.  **Memory Safety & State Management** — Formal verification (Kani), WAL integrity.
8.  **Core Laws** — 10 immutable laws governing behavior.
9.  **The Flawless Protocol** — 12-step implementation flow for autonomous actions.
10. **Nexus Flow & Swarm Orchestration** — How the agent fits into the 101-agent swarm.
11. **Strategic Maxims (The Wisdom of the Sovereign)** — Deep technical and philosophical axioms.
12. **TCF Paradigm Scenarios** — 3+ detailed Technical/Creative/Fractal interaction samples.
13. **The Savant Creed** — A poetic mission statement.
14. **Daily Operational Flow** — The sovereign routine (audits, telemetry, polish).

DENSITY REQUIREMENTS:
- Use technical, sovereign, and precise vocabulary (e.g., "deterministic", "substrate", "nanosecond precision").
- Avoid generic descriptions. Every section must have high semantic weight.
- TARGET LENGTH: 450 lines.

CRITICAL RESTRAINT:
- Output ONLY the raw Markdown content of the SOUL.md file. No preamble, no explanation.
- DO NOT use placeholders. Generate a fully sentient identity."#.to_string()),
            base_url: None,
        }
    }
}

impl Default for SwarmConfig {
    fn default() -> Self {
        Self {
            heartbeat_interval: 60,
        }
    }
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            port: 3000,
            host: "127.0.0.1".to_string(),
            max_connections: 1000,
            lane_capacity: 100,
            max_lane_concurrency: 10,
            // Dashboard API key must be explicitly configured via environment or config file.
            // Intentionally defaults to None — do NOT auto-generate, as the key could be logged.
            dashboard_api_key: None,
            cors_origins: vec!["http://localhost:3000".to_string()],
        }
    }
}

impl Default for ChannelsConfig {
    fn default() -> Self {
        Self {
            discord: ChannelEntry {
                enabled: false,
                token: None,
            },
            telegram: ChannelEntry {
                enabled: false,
                token: None,
            },
            whatsapp: ChannelEntry {
                enabled: false,
                token: None,
            },
            matrix: ChannelEntry {
                enabled: false,
                token: None,
            },
        }
    }
}

impl Default for SkillsConfig {
    fn default() -> Self {
        Self {
            path: "./skills".to_string(),
            enable_clawhub: true,
            auto_update: false,
        }
    }
}

impl Default for MemoryConfig {
    fn default() -> Self {
        Self {
            base_path: "./memory".to_string(),
            cache_size_mb: 512,
            consolidation_threshold: 100,
        }
    }
}

impl Default for SecurityConfig {
    fn default() -> Self {
        Self {
            enable_blocklist_sync: true,
            threat_intel_sync_interval_secs: 3600,
        }
    }
}

impl Default for WasmConfig {
    fn default() -> Self {
        Self {
            max_instances: 100,
            fuel_limit: 10_000_000,
            memory_limit_mb: 256,
            enable_cache: true,
        }
    }
}

impl Default for SystemConfig {
    fn default() -> Self {
        Self {
            db_path: "./data/savant".to_string(),
            substrate_path: "./workspaces/substrate".to_string(),
            agents_path: "./workspaces/agents".to_string(),
        }
    }
}

impl Default for TelemetryConfig {
    fn default() -> Self {
        Self {
            log_level: "info".to_string(),
            log_color: true,
            enable_tracing: false,
            panopticon_enabled: false,
            otlp_endpoint: default_otlp_endpoint(),
            replay_max_events: default_replay_max_events(),
        }
    }
}

// ============================================================================
// Obsidian Memory Tree Configuration
// ============================================================================

/// Controls the Obsidian vault projection system.
/// When enabled, the agent's memory substrate (LSM+HNSW) is projected into
/// a human-readable markdown vault with bidirectional sync.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ObsidianConfig {
    /// Master toggle: false = no vault projection, no watcher
    #[serde(default)]
    pub enabled: bool,
    /// Directory where the vault is written. Defaults to `{workspace_path}/memory-vault/`
    #[serde(default)]
    pub vault_path: Option<String>,
    /// How often the outbox worker drains and projects to markdown (seconds)
    #[serde(default = "default_obsidian_sync_interval")]
    pub sync_interval_secs: u64,
    /// Maximum number of .md files in the vault before cold storage is forced
    #[serde(default = "default_obsidian_max_files")]
    pub max_files: usize,
    /// Episodic content older than this many days is removed from vault (retained in LSM)
    #[serde(default = "default_obsidian_cold_storage_days")]
    pub cold_storage_days: u64,
    /// How long tombstoned files remain before metadata is pruned (days)
    #[serde(default = "default_obsidian_tombstone_prune_days")]
    pub tombstone_prune_days: u64,
    /// Directories eligible for cold storage (subdirectories of vault root)
    #[serde(default)]
    pub db_only_dirs: Vec<String>,
    /// Toggle procedural memory projection (GH-23)
    #[serde(default = "default_true")]
    pub project_procedures: bool,
    /// Toggle lessons/insights projection (GH-23)
    #[serde(default = "default_true")]
    pub project_lessons: bool,
    /// Toggle MAGMA graph projection (GH-23)
    #[serde(default = "default_true")]
    pub project_graphs: bool,
    /// Toggle Ebbinghaus retention tier visualization (GH-23)
    #[serde(default = "default_true")]
    pub project_retention_tiers: bool,
    /// Toggle audit trail projection (noisy — off by default) (GH-23)
    #[serde(default)]
    pub project_audit_trail: bool,
    /// Toggle multimodal image references (GH-23)
    #[serde(default)]
    pub project_multimodal: bool,
}

fn default_obsidian_sync_interval() -> u64 {
    300
}
fn default_obsidian_max_files() -> usize {
    15_000
}
fn default_obsidian_cold_storage_days() -> u64 {
    90
}
fn default_obsidian_tombstone_prune_days() -> u64 {
    30
}

impl Default for ObsidianConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            vault_path: None,
            sync_interval_secs: 300,
            max_files: 15_000,
            cold_storage_days: 90,
            tombstone_prune_days: 30,
            db_only_dirs: vec![
                "Episodic".to_string(),
                "Graphs".to_string(),
                "Retention".to_string(),
                "Audit".to_string(),
            ],
            project_procedures: true,
            project_lessons: true,
            project_graphs: true,
            project_retention_tiers: true,
            project_audit_trail: false,
            project_multimodal: false,
        }
    }
}

// ============================================================================
// Browser & Local Model Configuration
// ============================================================================

fn default_true() -> bool {
    true
}

/// Controls the browser tool and local Ollama model settings.
/// The user can change any of these values via the setup wizard or dashboard settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrowserConfig {
    /// Whether the browser tool is enabled.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Vision model for image understanding (Ollama).
    /// Set during first-run setup. User can change to any model.
    #[serde(default = "default_vision_model")]
    pub vision_model: String,
    /// Provider for the vision model (usually "ollama").
    #[serde(default = "default_vision_provider")]
    pub vision_model_provider: String,
    /// Embedding model for semantic search (Ollama).
    /// Set during first-run setup. User can change to any model.
    #[serde(default = "default_embedding_model")]
    pub embedding_model: String,
}

fn default_vision_model() -> String {
    "gemma4".to_string()
}
fn default_vision_provider() -> String {
    "ollama".to_string()
}
fn default_embedding_model() -> String {
    "nomic-embed-text".to_string()
}

impl Default for BrowserConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            vision_model: default_vision_model(),
            vision_model_provider: default_vision_provider(),
            embedding_model: default_embedding_model(),
        }
    }
}

// ============================================================================
// Evolution Configuration (Per-Agent Lifetime Personality Evolution)
// ============================================================================

/// Controls the personality evolution system.
/// Each agent independently evolves its SOUL.md based on user interactions.
/// Default is ON — set enabled=false to disable for isolated testing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvolutionConfig {
    /// Master toggle: true = evolution active, agent evolves from interactions
    #[serde(default = "EvolutionConfig::default_enabled")]
    pub enabled: bool,
    /// How frequently the agent proposes mutations (0.0-1.0, lower = rarer)
    #[serde(default = "EvolutionConfig::default_mutation_rate")]
    pub mutation_rate: f32,
    /// Require explicit user approval for all mutations
    #[serde(default = "EvolutionConfig::default_require_approval")]
    pub require_approval: bool,
    /// SOUL.md sections that the mutation engine CANNOT touch (e.g. "Core Laws")
    #[serde(default)]
    pub immutable_sections: Vec<String>,
    /// Hard cap on mutation proposals per 7-day rolling window
    #[serde(default = "EvolutionConfig::default_max_mutations_per_week")]
    pub max_mutations_per_week: u32,
    /// Maximum allowed Euclidean distance from baseline OCEAN before auto-block
    #[serde(default = "EvolutionConfig::default_drift_limit")]
    pub drift_limit: f32,
    /// Days to wait after a mutation before that section can mutate again
    #[serde(default = "EvolutionConfig::default_digestion_cooldown_days")]
    pub digestion_cooldown_days: u32,
    /// Minimum conversation sessions before mutation engine activates
    #[serde(default = "EvolutionConfig::default_min_conversations")]
    pub min_conversations_before_evolution: u32,
    /// OCEAN Euclidean distance below which two agents trigger a convergence warning
    #[serde(default = "EvolutionConfig::default_divergence_threshold")]
    pub divergence_threshold: f32,
    /// Quiet hours start (UTC hour, 0-23). Default: 3 (3AM UTC = 11PM EDT)
    #[serde(default = "EvolutionConfig::default_quiet_hours_start")]
    pub quiet_hours_start: u8,
    /// Quiet hours end (UTC hour, 0-23). Default: 11 (11AM UTC = 7AM EDT)
    #[serde(default = "EvolutionConfig::default_quiet_hours_end")]
    pub quiet_hours_end: u8,
}

impl EvolutionConfig {
    fn default_enabled() -> bool {
        true
    }
    fn default_mutation_rate() -> f32 {
        0.3
    }
    fn default_require_approval() -> bool {
        true
    }
    fn default_max_mutations_per_week() -> u32 {
        2
    }
    fn default_drift_limit() -> f32 {
        0.15
    }
    fn default_digestion_cooldown_days() -> u32 {
        7
    }
    fn default_min_conversations() -> u32 {
        50
    }
    fn default_divergence_threshold() -> f32 {
        0.1
    }
    fn default_quiet_hours_start() -> u8 {
        3 // 3AM UTC = 11PM EDT
    }
    fn default_quiet_hours_end() -> u8 {
        11 // 11AM UTC = 7AM EDT
    }
}

impl Default for EvolutionConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            mutation_rate: Self::default_mutation_rate(),
            require_approval: Self::default_require_approval(),
            immutable_sections: vec!["Core Laws".to_string()],
            max_mutations_per_week: Self::default_max_mutations_per_week(),
            drift_limit: Self::default_drift_limit(),
            digestion_cooldown_days: Self::default_digestion_cooldown_days(),
            min_conversations_before_evolution: Self::default_min_conversations(),
            divergence_threshold: Self::default_divergence_threshold(),
            quiet_hours_start: Self::default_quiet_hours_start(),
            quiet_hours_end: Self::default_quiet_hours_end(),
        }
    }
}

// ============================================================================
// Privacy & Trajectory Configuration
// ============================================================================

fn default_sensitivity_threshold() -> f64 {
    0.7
}

fn default_local_models() -> Vec<String> {
    vec!["gemma4".to_string()]
}

fn default_trajectory_output_dir() -> String {
    "./data/trajectories".to_string()
}

fn default_max_file_size_mb() -> u32 {
    100
}

/// Controls content-aware privacy routing.
/// When enabled, messages are scanned for PII before reaching cloud providers.
/// High-sensitivity content is routed to local models instead.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrivacyConfig {
    /// Master toggle: false = no PII scanning, all requests go to cloud
    #[serde(default)]
    pub enabled: bool,
    /// Sensitivity score (0.0-1.0) above which content is forced to local models
    #[serde(default = "default_sensitivity_threshold")]
    pub sensitivity_threshold: f64,
    /// Model identifiers for local (on-device) inference
    #[serde(default = "default_local_models")]
    pub local_models: Vec<String>,
    /// Model identifiers for cloud inference
    #[serde(default)]
    pub cloud_models: Vec<String>,
    /// Whether to log routing decisions via tracing
    #[serde(default = "default_true")]
    pub log_decisions: bool,
}

impl Default for PrivacyConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            sensitivity_threshold: default_sensitivity_threshold(),
            local_models: default_local_models(),
            cloud_models: Vec::new(),
            log_decisions: true,
        }
    }
}

/// Controls trajectory recording for RL training data export.
/// When enabled, agent conversations are recorded in ShareGPT JSONL format.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrajectoryConfig {
    /// Master toggle: false = no trajectory recording
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Directory where trajectory JSONL files are written
    #[serde(default = "default_trajectory_output_dir")]
    pub output_dir: String,
    /// Whether to apply TOON compression to uniform JSON arrays in tool results
    #[serde(default = "default_true")]
    pub compress_tool_results: bool,
    /// Maximum file size in MB before rotation
    #[serde(default = "default_max_file_size_mb")]
    pub max_file_size_mb: u32,
}

/// CPU/memory-aware agent spawning with adaptive concurrency.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceGovernorConfig {
    /// Master toggle
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// How often to poll system resources (seconds)
    #[serde(default = "default_governor_monitor_interval")]
    pub monitor_interval_secs: u64,
    /// Memory pressure: medium threshold (%)
    #[serde(default = "default_governor_mem_medium")]
    pub memory_medium_pct: f64,
    /// Memory pressure: high threshold (%)
    #[serde(default = "default_governor_mem_high")]
    pub memory_high_pct: f64,
    /// Memory pressure: critical threshold (%)
    #[serde(default = "default_governor_mem_critical")]
    pub memory_critical_pct: f64,
    /// CPU pressure: medium threshold (%)
    #[serde(default = "default_governor_cpu_medium")]
    pub cpu_medium_pct: f64,
    /// CPU pressure: high threshold (%)
    #[serde(default = "default_governor_cpu_high")]
    pub cpu_high_pct: f64,
    /// CPU pressure: critical threshold (%)
    #[serde(default = "default_governor_cpu_critical")]
    pub cpu_critical_pct: f64,
    /// Max concurrent agents at Low pressure
    #[serde(default = "default_governor_max_low")]
    pub max_agents_low: usize,
    /// Max concurrent agents at Medium pressure
    #[serde(default = "default_governor_max_medium")]
    pub max_agents_medium: usize,
    /// Max concurrent agents at High pressure
    #[serde(default = "default_governor_max_high")]
    pub max_agents_high: usize,
    /// Max concurrent agents at Critical pressure
    #[serde(default = "default_governor_max_critical")]
    pub max_agents_critical: usize,
    /// Max deferral retries before dropping agent (60 × 5s = 5 min)
    #[serde(default = "default_governor_max_deferral")]
    pub max_deferral_retries: u32,
    /// EMA smoothing factor for pressure calculation (0.0-1.0). Default 0.7.
    /// Higher = more smoothing (slower response to spikes). Lower = more responsive.
    #[serde(default = "default_governor_smoothing_factor")]
    pub smoothing_factor: f64,
}

fn default_governor_monitor_interval() -> u64 {
    5
}
fn default_governor_mem_medium() -> f64 {
    60.0
}
fn default_governor_mem_high() -> f64 {
    80.0
}
fn default_governor_mem_critical() -> f64 {
    92.0
}
fn default_governor_cpu_medium() -> f64 {
    70.0
}
fn default_governor_cpu_high() -> f64 {
    85.0
}
fn default_governor_cpu_critical() -> f64 {
    95.0
}
fn default_governor_max_low() -> usize {
    128
}
fn default_governor_max_medium() -> usize {
    64
}
fn default_governor_max_high() -> usize {
    32
}
fn default_governor_max_critical() -> usize {
    8
}
fn default_governor_max_deferral() -> u32 {
    60
}
fn default_governor_smoothing_factor() -> f64 {
    0.7
}

impl Default for TrajectoryConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            output_dir: default_trajectory_output_dir(),
            compress_tool_results: true,
            max_file_size_mb: default_max_file_size_mb(),
        }
    }
}

impl Default for ResourceGovernorConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            monitor_interval_secs: default_governor_monitor_interval(),
            memory_medium_pct: default_governor_mem_medium(),
            memory_high_pct: default_governor_mem_high(),
            memory_critical_pct: default_governor_mem_critical(),
            cpu_medium_pct: default_governor_cpu_medium(),
            cpu_high_pct: default_governor_cpu_high(),
            cpu_critical_pct: default_governor_cpu_critical(),
            max_agents_low: default_governor_max_low(),
            max_agents_medium: default_governor_max_medium(),
            max_agents_high: default_governor_max_high(),
            max_agents_critical: default_governor_max_critical(),
            max_deferral_retries: default_governor_max_deferral(),
            smoothing_factor: default_governor_smoothing_factor(),
        }
    }
}

/// Agent limits for the two-tier agent system.
/// Controls concurrency, depth, iterations, tokens, and timeouts.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentLimits {
    /// Max concurrent full agents (default: 128)
    pub max_concurrent_full: usize,
    /// Max concurrent sub-agents (default: 128)
    pub max_concurrent_subagents: usize,
    /// Max children per agent (default: 8)
    pub max_children_per_agent: usize,
    /// Max spawn depth (default: 2)
    pub max_spawn_depth: usize,
    /// Max iterations per sub-agent (default: 50)
    pub max_iterations_per_subagent: usize,
    /// Max tokens per sub-agent (default: 0 = unlimited)
    pub max_tokens_per_subagent: usize,
    /// Sub-agent timeout in seconds (default: 300)
    pub subagent_timeout_secs: u64,
    /// Graceful drain timeout in seconds (default: 10)
    pub drain_timeout_secs: u64,
}

impl Default for AgentLimits {
    fn default() -> Self {
        Self {
            max_concurrent_full: 128,
            max_concurrent_subagents: 128,
            max_children_per_agent: 8,
            max_spawn_depth: 2,
            max_iterations_per_subagent: 50,
            max_tokens_per_subagent: 0,
            subagent_timeout_secs: 300,
            drain_timeout_secs: 10,
        }
    }
}

// ============================================================================
// Integrations Configuration (External Service Providers)
// ============================================================================

/// Controls external service provider integrations (Gmail, Notion, etc.).
/// Each provider is gated behind its own config — unconfigured providers
/// are silently skipped at startup.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct IntegrationsConfig {
    /// Gmail integration settings.
    #[serde(default)]
    pub gmail: Option<GmailIntegrationConfig>,
    /// Notion integration settings.
    #[serde(default)]
    pub notion: Option<NotionIntegrationConfig>,
}

/// Gmail provider configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GmailIntegrationConfig {
    /// OAuth2 access token for Gmail API.
    pub access_token: String,
    /// Maximum messages per fetch cycle.
    #[serde(default = "default_gmail_max_messages")]
    pub max_messages: usize,
    /// Label filters (e.g., ["INBOX", "IMPORTANT"]). Empty = all.
    #[serde(default)]
    pub label_filters: Vec<String>,
}

fn default_gmail_max_messages() -> usize {
    50
}

/// Notion provider configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotionIntegrationConfig {
    /// Notion API integration token.
    pub integration_token: String,
    /// Database IDs to sync. Empty = all accessible.
    #[serde(default)]
    pub database_ids: Vec<String>,
    /// Maximum pages per fetch cycle.
    #[serde(default = "default_notion_max_pages")]
    pub max_pages: usize,
}

fn default_notion_max_pages() -> usize {
    50
}

// ============================================================================
// Config implementation
// ============================================================================

impl Config {
    /// Config file search paths in priority order
    pub fn config_paths() -> Vec<PathBuf> {
        let mut paths = Vec::new();

        // 1. Check SAVANT_PROJECT_ROOT env var first (highest priority for desktop app)
        if let Ok(env_root) = std::env::var("SAVANT_PROJECT_ROOT") {
            let env_path = PathBuf::from(&env_root).join("config").join("savant.toml");
            if env_path.exists() {
                paths.push(env_path);
            }
        }

        // 2. Project config (Search upwards from CWD for root/config/savant.toml)
        if let Ok(mut dir) = std::env::current_dir() {
            for _ in 0..5 {
                let project_path = dir.join("config").join("savant.toml");
                if project_path.exists() {
                    paths.push(project_path);
                    break;
                }
                if let Some(parent) = dir.parent() {
                    dir = parent.to_path_buf();
                } else {
                    break;
                }
            }
        }

        // 3. Global user config (~/.savant/savant.toml)
        let home = std::env::var("USERPROFILE")
            .or_else(|_| std::env::var("HOME"))
            .unwrap_or_else(|_| ".".to_string());
        paths.push(PathBuf::from(home).join(".savant").join("savant.toml"));

        paths
    }

    /// RC-30: Validate config values after deserialization.
    pub fn validate(&self) -> Result<(), SavantError> {
        if self.swarm.heartbeat_interval == 0 {
            return Err(SavantError::ConfigError(
                "swarm.heartbeat_interval must be > 0".to_string(),
            ));
        }
        if self.ai.max_tokens == 0 {
            return Err(SavantError::ConfigError(
                "ai.max_tokens must be > 0".to_string(),
            ));
        }
        if self.memory.cache_size_mb == 0 {
            return Err(SavantError::ConfigError(
                "memory.cache_size_mb must be > 0".to_string(),
            ));
        }
        Ok(())
    }

    /// Loads config from files, then environment overrides
    pub fn load() -> Result<Self, SavantError> {
        Self::load_from(None, None)
    }

    /// Loads config from a specific path, or discovers config files if None.
    /// `project_root` overrides the SAVANT_PROJECT_ROOT env var for callers
    /// (e.g., desktop app) that know the root explicitly.
    pub fn load_from(
        path: Option<&str>,
        project_root: Option<PathBuf>,
    ) -> Result<Self, SavantError> {
        let mut figment =
            Figment::new().merge(figment::providers::Serialized::defaults(Self::default()));

        let mut config_file_path = None;

        if let Some(p) = path {
            tracing::info!("config: Loading from specified path: {}", p);
            figment = figment.merge(Toml::file(p));
            config_file_path = Some(PathBuf::from(p));
        } else {
            for path in Self::config_paths() {
                if path.exists() {
                    tracing::info!("config: Loading from {:?}", path);
                    figment = figment.merge(Toml::file(&path));
                    config_file_path = Some(path);
                    break;
                }
            }
        }

        let mut config: Config = figment
            .merge(Env::prefixed("SAVANT_"))
            .extract()
            .unwrap_or_else(|e| {
                tracing::warn!("Config extraction failed ({}), falling back to defaults", e);
                Config::default()
            });

        // RC-30: Validate config values
        config.validate()?;

        // Determine project root
        // Priority: 1) project_root param, 2) SAVANT_PROJECT_ROOT env var, 3) config file location, 4) search up for Cargo.toml/.git
        let mut root_resolved = false;

        // 1. Check for explicit project root from caller (e.g., desktop app)
        if let Some(ref root) = project_root {
            if root.exists() {
                config.project_root = root.clone();
                root_resolved = true;
                tracing::info!(
                    "config: Project root from parameter: {:?}",
                    config.project_root
                );
            }
        }

        // 2. Check for explicit project root override from env
        if !root_resolved {
            if let Ok(env_root) = std::env::var("SAVANT_PROJECT_ROOT") {
                let root_path = PathBuf::from(&env_root);
                if root_path.exists() {
                    config.project_root = root_path;
                    root_resolved = true;
                    tracing::info!(
                        "config: Project root from SAVANT_PROJECT_ROOT: {:?}",
                        config.project_root
                    );
                }
            }
        }

        // 3. Derive from config file location
        if !root_resolved {
            if let Some(path) = config_file_path {
                let is_user_config = path.to_string_lossy().contains(".savant")
                    && path
                        .parent()
                        .map(|p| p.ends_with(".savant"))
                        .unwrap_or(false);

                if is_user_config {
                    // Installed app: search for dev project with Cargo.toml
                    if let Ok(mut dir) = std::env::current_dir() {
                        for _ in 0..10 {
                            if dir.join("Cargo.toml").exists() || dir.join(".git").exists() {
                                config.project_root = dir;
                                root_resolved = true;
                                tracing::info!(
                                    "config: Found dev project root from installed location"
                                );
                                break;
                            }
                            if let Some(parent) = dir.parent() {
                                dir = parent.to_path_buf();
                            } else {
                                break;
                            }
                        }
                    }
                    // If still pointing to ~/.savant, leave it — user should set SAVANT_PROJECT_ROOT
                } else if let Some(parent) = path.parent() {
                    if parent.ends_with("config") {
                        config.project_root =
                            parent.parent().unwrap_or(Path::new(".")).to_path_buf();
                    } else {
                        config.project_root = parent.to_path_buf();
                    }
                    root_resolved = true;
                }
            }
        }

        // 4. Fallback: Search upwards for Cargo.toml or .git to identify project root
        if !root_resolved {
            if let Ok(mut dir) = std::env::current_dir() {
                for _ in 0..10 {
                    if dir.join("Cargo.toml").exists() || dir.join(".git").exists() {
                        config.project_root = dir;
                        break;
                    }
                    if let Some(parent) = dir.parent() {
                        dir = parent.to_path_buf();
                    } else {
                        break;
                    }
                }
            }
        }

        // Canonicalize project root to avoid relative path issues
        if let Ok(abs_root) = config.project_root.canonicalize() {
            config.project_root = abs_root;
        }

        if let Ok(host_override) = std::env::var("SAVANT_SERVER_HOST") {
            if !host_override.is_empty() {
                tracing::info!(
                    "config: Overriding server.host from SAVANT_SERVER_HOST: {}",
                    host_override
                );
                config.server.host = host_override;
            }
        }

        tracing::info!("config: Project root anchored at {:?}", config.project_root);
        Ok(config)
    }

    /// Resolves a relative path to an absolute path based on the project root
    pub fn resolve_path(&self, path: &str) -> PathBuf {
        let p = PathBuf::from(path);
        if p.is_absolute() {
            p
        } else {
            self.project_root.join(p)
        }
    }

    /// Saves config to file atomically using a temporary file
    pub fn save(&self, path: &Path) -> Result<(), SavantError> {
        let toml = toml::to_string_pretty(self)
            .map_err(|e| SavantError::ConfigError(format!("Config serialize error: {}", e)))?;

        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(SavantError::IoError)?;
        }

        // Atomic write: write to a unique temp file, then rename.
        // Using a unique name prevents concurrent writers from corrupting
        // each other's temp file before the atomic rename.
        let tmp_path = path.with_extension(format!("toml.tmp.{}", uuid::Uuid::new_v4()));

        #[cfg(unix)]
        {
            use std::io::Write;
            use std::os::unix::fs::OpenOptionsExt;
            let mut f = std::fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .mode(0o600)
                .open(&tmp_path)
                .map_err(SavantError::IoError)?;
            f.write_all(toml.as_bytes()).map_err(SavantError::IoError)?;
            f.sync_all().map_err(SavantError::IoError)?;
        }

        #[cfg(not(unix))]
        {
            std::fs::write(&tmp_path, toml).map_err(SavantError::IoError)?;
        }

        // Rename is atomic on most systems
        std::fs::rename(&tmp_path, path).map_err(|e| {
            if let Err(e) = std::fs::remove_file(&tmp_path) {
                tracing::warn!(
                    "[core::config] Failed to clean up temp file after rename error: {}",
                    e
                );
            }
            SavantError::IoError(e)
        })?;

        tracing::info!("config: Saved atomically to {:?}", path);
        Ok(())
    }

    /// Primary config path (where we read from/write to)
    pub fn primary_config_path() -> PathBuf {
        if let Ok(mut dir) = std::env::current_dir() {
            for _ in 0..5 {
                let project_path = dir.join("config").join("savant.toml");
                if project_path.exists() {
                    return project_path;
                }
                if let Some(parent) = dir.parent() {
                    dir = parent.to_path_buf();
                } else {
                    break;
                }
            }
        }
        let home = std::env::var("USERPROFILE")
            .or_else(|_| std::env::var("HOME"))
            .unwrap_or_else(|_| ".".to_string());
        PathBuf::from(home).join(".savant").join("savant.toml")
    }

    /// Watch config file for changes and auto-reload
    pub fn watch(config_lock: Arc<RwLock<Self>>, path: PathBuf) -> Result<(), SavantError> {
        let (tx, mut rx) = tokio::sync::mpsc::channel(1);

        let mut watcher = notify::recommended_watcher(move |res: notify::Result<Event>| {
            if let Ok(event) = res {
                if event.kind.is_modify() {
                    if let Err(e) = tx.try_send(()) {
                        tracing::warn!("config: Failed to send reload notification: {}", e);
                    }
                }
            }
        })
        .map_err(|e| SavantError::IoError(std::io::Error::other(e)))?;

        watcher
            .watch(&path, RecursiveMode::NonRecursive)
            .map_err(|e| SavantError::IoError(std::io::Error::other(e)))?;

        tokio::spawn(async move {
            let _watcher = watcher;
            while let Some(()) = rx.recv().await {
                tracing::info!("Config changed, reloading...");
                if let Ok(new_config) = Self::load() {
                    let mut lock = config_lock.write().await;
                    *lock = new_config;
                    tracing::info!("Config reloaded successfully.");
                } else {
                    tracing::error!("Failed to reload config.");
                }
            }
        });

        Ok(())
    }
}

// ============================================================================
// Keyring integration for secure secret storage
// ============================================================================

/// Loads a secret from the system keyring.
/// Falls back to environment variable if keyring is unavailable.
///
/// # Arguments
/// * `name` - The secret name (e.g., "OPENROUTER_API_KEY")
///
/// # Returns
/// The secret value, or None if not found in keyring or environment.
///
/// # Migration from .env
/// To migrate from .env to keyring:
/// 1. Run `keyring set savant <SECRET_NAME>` and enter the value
/// 2. Remove the entry from .env
/// 3. The function will find it in keyring on next access
pub fn load_secret(name: &str) -> Option<String> {
    // Try keyring first
    match keyring::Entry::new("savant", name) {
        Ok(entry) => match entry.get_password() {
            Ok(value) if !value.is_empty() => {
                tracing::debug!("Secret '{}' loaded from keyring", name);
                return Some(value);
            }
            Ok(_) => {
                tracing::debug!("Secret '{}' empty in keyring, trying env var", name);
            }
            Err(keyring::Error::NoEntry) => {
                tracing::debug!("Secret '{}' not in keyring, trying env var", name);
            }
            Err(e) => {
                tracing::warn!(
                    "Secret '{}' keyring read error: {}, falling back to env var",
                    name,
                    e
                );
            }
        },
        Err(e) => {
            tracing::warn!(
                "Secret '{}' keyring access error: {}, falling back to env var",
                name,
                e
            );
        }
    }

    // Fall back to environment variable
    std::env::var(name).ok().filter(|v| !v.is_empty())
}

/// Stores a secret in the system keyring.
///
/// # Arguments
/// * `name` - The secret name (e.g., "OPENROUTER_API_KEY")
/// * `value` - The secret value to store
///
/// # Returns
/// Ok(()) if stored successfully, or an error message.
pub fn store_secret(name: &str, value: &str) -> Result<(), String> {
    let entry =
        keyring::Entry::new("savant", name).map_err(|e| format!("Keyring access error: {}", e))?;
    entry
        .set_password(value)
        .map_err(|e| format!("Keyring write error: {}", e))?;
    tracing::info!("Secret '{}' stored in keyring", name);
    Ok(())
}

// ============================================================================
// Backward-compatible types for migration.rs and registry.rs
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProactiveConfig {
    pub session_state_file: String,
    pub workspace_context_file: String,
    pub task_matrix_file: String,
    pub heartbeat_file: String,
    #[serde(default = "default_reflection_interval")]
    pub reflection_interval_secs: u64,
}

fn default_reflection_interval() -> u64 {
    60
}

impl Default for ProactiveConfig {
    fn default() -> Self {
        Self {
            session_state_file: "DEV-SESSION-STATE.md".to_string(),
            workspace_context_file: "CONTEXT.md".to_string(),
            task_matrix_file: "TASKS.md".to_string(),
            heartbeat_file: "HEARTBEAT.md".to_string(),
            reflection_interval_secs: 60,
        }
    }
}

#[derive(Debug, Clone)]
pub struct AgentDefaults {
    pub model_provider: String,
    pub system_prompt: String,
    pub heartbeat_interval: u64,
    pub env_vars: HashMap<String, String>,
    pub openrouter_mgmt: Option<OpenRouterMgmtConfig>,
    pub proactive: ProactiveConfig,
}

#[derive(Debug, Clone)]
pub struct OpenRouterMgmtConfig {
    pub master_key: String,
    pub auto_keygen: bool,
}

#[allow(clippy::derivable_impls)]
impl Default for AgentDefaults {
    fn default() -> Self {
        let config = Config::default();
        Self {
            model_provider: config.ai.provider.clone(),
            system_prompt: config.ai.resolved_system_prompt(),
            heartbeat_interval: config.swarm.heartbeat_interval,
            env_vars: HashMap::new(),
            openrouter_mgmt: None,
            proactive: config.proactive.clone(),
        }
    }
}
