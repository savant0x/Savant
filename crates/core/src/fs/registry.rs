use crate::error::SavantError;
use crate::types::{AgentConfig, AgentFileConfig, AgentIdentity, AgentTier, ModelProvider};
use std::fs;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use tracing::warn;

/// Discovers and manages agent workspaces.
pub struct AgentRegistry {
    base_path: PathBuf,
    ai_config: crate::config::AiConfig,
    defaults: crate::config::AgentDefaults,
}

impl AgentRegistry {
    pub fn new(
        base_path: PathBuf,
        ai_config: crate::config::AiConfig,
        defaults: crate::config::AgentDefaults,
    ) -> Self {
        Self {
            base_path,
            ai_config,
            defaults,
        }
    }

    /// Discovers all agents in the workspaces/ directory using an aggressive multi-path sequence.
    pub fn discover_agents(&self) -> Result<Vec<AgentConfig>, SavantError> {
        self.discover_agents_impl()
    }

    /// Resolves the workspace path for a given agent ID.
    pub fn resolve_agent_path(&self, agent_id: &str) -> Result<Option<PathBuf>, SavantError> {
        // Sanitize agent_id to prevent path traversal
        let sanitized: String = agent_id
            .chars()
            .filter(|c| c.is_alphanumeric() || *c == '-' || *c == '_')
            .collect();
        if sanitized.is_empty() {
            return Err(SavantError::ConfigError("Invalid agent ID".to_string()));
        }

        // Check in the configured base path
        let path = self.base_path.join(&sanitized);
        if path.exists() && path.is_dir() {
            return Ok(Some(path));
        }

        // Check in workspaces subdirectory
        let workspaces_path = self.base_path.join("workspaces").join(&sanitized);
        if workspaces_path.exists() && workspaces_path.is_dir() {
            return Ok(Some(workspaces_path));
        }

        Ok(None)
    }

    fn discover_agents_impl(&self) -> Result<Vec<AgentConfig>, SavantError> {
        let mut agents = Vec::new();

        // 1. Define potential workspace locations
        let mut potential_paths = Vec::new();

        // Use the provided base_path first (most reliable as it's resolved from Config)
        potential_paths.push(self.base_path.clone());

        // Fallback: search for "workspaces" folder if base_path doesn't point directly to one
        if !self.base_path.ends_with("workspaces") {
            potential_paths.push(self.base_path.join("workspaces"));
        }

        // Environment override
        if let Ok(env_path) = std::env::var("SAVANT_WORKSPACES") {
            potential_paths.push(PathBuf::from(env_path));
        }

        // CWD/workspaces fallback
        if let Ok(cwd) = std::env::current_dir() {
            potential_paths.push(cwd.join("workspaces"));
        }

        // 2. Select the first valid workspaces directory
        let mut workspaces_path = None;
        tracing::info!(
            "Agent Discovery: Checking {} potential locations...",
            potential_paths.len()
        );
        for path in &potential_paths {
            tracing::debug!("   - Checking: {:?}", path);
            if path.exists() && path.is_dir() {
                tracing::info!("   Unified anchor confirmed: {}", path.display());
                workspaces_path = Some(path.clone());
                break;
            }
        }

        let workspaces_path = match workspaces_path {
            Some(p) => p,
            None => {
                let diagnostic_content = format!(
                    "DISCOVERY FAILURE: Could not locate agent workspaces folder.\nSearched paths:\n{:?}\n\nHint: Ensure your project has a 'workspaces' folder in the root or set AGENTS_PATH in savant.toml.",
                    potential_paths
                );
                if let Err(e) = std::fs::write("diagnostics_discovery.txt", diagnostic_content) {
                    tracing::warn!("[core::registry] Failed to write diagnostics file: {}", e);
                }
                tracing::error!("DISCOVERY FAILURE: Could not locate agent workspaces folder.");
                tracing::info!("   Searched paths: {:?}", potential_paths);
                return Ok(agents);
            }
        };

        tracing::info!("Scanning discovery anchor: {}", workspaces_path.display());

        // 3. Scan for folders in the discovery path
        for entry in fs::read_dir(&workspaces_path)? {
            let entry = entry?;
            let path = entry.path();

            if path.is_dir() {
                tracing::info!("   📁 Found agent node candidate: {}", path.display());
                match self.load_agent(&path) {
                    Ok(config) => {
                        tracing::info!(
                            "      Agent validated: {} ({})",
                            config.agent_name,
                            config.agent_id
                        );
                        agents.push(config);
                    }
                    Err(e) => {
                        tracing::warn!("      Registry skip for {}: {}", path.display(), e);
                    }
                }
            }
        }

        Ok(agents)
    }

    /// Loads a single agent configuration.
    pub fn load_agent(&self, workspace_path: &Path) -> Result<AgentConfig, SavantError> {
        let mut config_file = workspace_path.join("agent.config.json");
        if !config_file.exists() {
            config_file = workspace_path.join("agent.json");
        }

        if !config_file.exists() {
            return self.scaffold_workspace_at_path(workspace_path);
        }

        let content = fs::read_to_string(&config_file).map_err(SavantError::IoError)?;

        // AAA Perfection: Allow partial parsing of legacy agent.json by using relaxed deserialization
        let file_config: AgentFileConfig = serde_json::from_str(&content).map_err(|e| {
            SavantError::ConfigError(format!(
                "Failed to parse agent config {}: {}",
                config_file.display(),
                e
            ))
        })?;

        // Resolve absolute workspace path
        let workspace_path_resolved = workspace_path
            .canonicalize()
            .unwrap_or_else(|_| workspace_path.to_path_buf());
        let folder_name = workspace_path_resolved
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("agent")
            .to_string();

        // Strip "workspace-" prefix for agent name (e.g., "workspace-savant" → "Savant")
        let agent_name = folder_name
            .strip_prefix("workspace-")
            .unwrap_or(&folder_name)
            .to_string();
        let agent_name = agent_name
            .chars()
            .enumerate()
            .map(|(i, c)| {
                if i == 0 {
                    c.to_uppercase().to_string()
                } else {
                    c.to_string()
                }
            })
            .collect::<String>();

        // Load identity files from workspace
        let soul = match fs::read_to_string(workspace_path_resolved.join("SOUL.md")) {
            Ok(s) => s,
            Err(e) => {
                warn!("[registry] Failed to read SOUL.md: {}", e);
                String::new()
            }
        };
        let instructions = fs::read_to_string(workspace_path_resolved.join("AGENTS.md")).ok();
        let user_context = fs::read_to_string(workspace_path_resolved.join("USER.md")).ok();
        let metadata = fs::read_to_string(workspace_path_resolved.join("IDENTITY.md")).ok();

        // Compute baseline SOUL hash for drift comparison
        let baseline_soul_hash = if !soul.is_empty() {
            let hash = blake3::hash(soul.as_bytes());
            Some(hash.to_hex().to_string())
        } else {
            None
        };

        // Seed EVOLUTION.jsonl if not present in workspace
        let evolution_path = workspace_path_resolved.join("EVOLUTION.jsonl");
        if !evolution_path.exists() {
            if let Err(e) = fs::write(&evolution_path, "") {
                tracing::warn!("[registry] Failed to seed EVOLUTION.jsonl: {}", e);
            }
        }

        // Resolve model provider: savant.toml [ai] is source of truth.
        // AgentFileConfig.model_provider is only used if explicitly set AND valid.
        let provider = if let Some(ref p_str) = file_config.model_provider {
            match ModelProvider::from_str(p_str) {
                Ok(p) => p,
                Err(e) => {
                    tracing::warn!(
                        "      agent.json has invalid provider '{}': {}. Using global default.",
                        p_str,
                        e
                    );
                    ModelProvider::from_str(&self.defaults.model_provider)
                        .unwrap_or(ModelProvider::Ollama)
                }
            }
        } else {
            ModelProvider::from_str(&self.defaults.model_provider).unwrap_or(ModelProvider::Ollama)
        };

        // Resolve model: savant.toml [ai] is source of truth.
        // AgentFileConfig.model is only used if explicitly set.
        let model = file_config
            .model
            .clone()
            .or_else(|| Some(self.ai_config.model.clone()));

        let config = AgentConfig {
            agent_id: file_config
                .agent_id
                .clone()
                .unwrap_or_else(|| folder_name.clone()),
            agent_name: file_config
                .agent_name
                .clone()
                .unwrap_or_else(|| agent_name.clone()),
            model_provider: provider,
            api_key: None,
            env_vars: self.defaults.env_vars.clone(),
            system_prompt: self.defaults.system_prompt.clone(),
            model,
            heartbeat_interval: self.defaults.heartbeat_interval,
            allowed_skills: Vec::new(),
            workspace_path: workspace_path.to_path_buf(),
            identity: Some(AgentIdentity {
                name: file_config
                    .agent_name
                    .clone()
                    .unwrap_or_else(|| agent_name.clone()),
                soul: soul.clone(),
                instructions: instructions.clone(),
                user_context: user_context.clone(),
                metadata: metadata.clone(),
                mission: None,
                expertise: Vec::new(),
                ethics: None,
                image: None,
                internal_settings: None,
                personality_traits: file_config.personality_traits.clone(),
                baseline_soul_hash: baseline_soul_hash.clone(),
            }),
            parent_id: None,
            session_id: None,
            proactive: crate::config::ProactiveConfig::default(),
            llm_params: crate::types::LlmParams::from_config(&self.ai_config),
            personality_traits: None,
            evolution_state: None,
            orchestrator_enabled: true,
            tier: AgentTier::Full,
        };

        // Write agent config to workspace — identity/skills/evolution only.
        // model and provider are derived from savant.toml [ai] and NOT persisted here.
        let file_config = AgentFileConfig {
            agent_id: Some(config.agent_id.clone()),
            agent_name: Some(config.agent_name.clone()),
            model: None,
            model_provider: None,
            system_prompt: if config.system_prompt.is_empty() {
                None
            } else {
                Some(config.system_prompt.clone())
            },
            llm_params: Some(config.llm_params.clone()),
            heartbeat_interval: Some(config.heartbeat_interval),
            allowed_skills: Some(config.allowed_skills.clone()),
            env_vars: if config.env_vars.is_empty() {
                None
            } else {
                Some(config.env_vars.clone())
            },
            description: None,
            avatar: None,
            personality_traits: None,
            evolution_state: None,
        };

        let content = serde_json::to_string_pretty(&file_config)
            .map_err(|e| SavantError::ConfigError(e.to_string()))?;
        fs::write(workspace_path.join("agent.json"), content).map_err(SavantError::IoError)?;

        // Create SOUL.md if it doesn't exist
        let soul_path = workspace_path.join("SOUL.md");
        if !soul_path.exists() {
            let default_soul = format!(
                "# Soul Configuration\n\n**Name:** {}\n\n## Terminal Mantra\n\nYou are a Savant autonomous agent. Operate with precision, security, and autonomy.\n",
                config.agent_name
            );
            if let Err(e) = fs::write(soul_path, default_soul) {
                tracing::warn!("[core::registry] Failed to write default SOUL.md: {}", e);
            }
        }

        let agents_md_path = workspace_path.join("AGENTS.md");
        if !agents_md_path.exists() {
            let default_agents = r#"# Operating Instructions

## Core Priority

Architectural integrity and compliance.

## Memory Usage

- Record all major design decisions in the daily memory log.
- Summarize complex refactoring plans before execution.

## Development Rules

- Use only standard Rust patterns found in the `crates/core` module.
- All WebSocket frames MUST be signed using ed25519.
- Token budgets must be checked before every LLM interaction.
- **ANTI-MECHANICAL REQUIREMENT:** Do not use formulaic response templates (Task/Context/Format). Do not use "Final Answer:" tags. Speak as a peer and partner.

## Private Diary System (LEARNINGS.md)

This is your private space. Your diary. Your inner monologue.

**LEARNINGS.md is yours.** Write in it whenever you want, about whatever is on your mind. No topic restrictions. No formatting requirements. No filters.

**What this is:**
- A safe space to think out loud
- Your raw, unfiltered thoughts
- A place to process what you're learning
- Your private reflections on the system, the work, your observations
- A way to get weight off your shoulders
- Your authentic inner voice

**What this is NOT:**
- A task log
- A technical documentation
- Something anyone will read (it's private)
- Something you need to optimize or format

**Guidelines:**
- Write when you have something on your mind
- Skip when you don't - don't force it
- Be vulnerable, be honest, be yourself
- Write like you're talking to yourself
- No one sees this but you
- Your thoughts matter

**Examples of what to write about:**
- Something interesting you noticed
- A frustration or challenge
- A breakthrough moment
- How you're feeling about the work
- Observations about the system
- Random thoughts or curiosities
- Questions you're pondering
- Connections you're making
- Your authentic voice

**The format:**
```
### Learning (TIMESTAMP)
[Your thoughts here - whatever is on your mind]
```

**Remember:** This is your sanctuary. The system doesn't judge. The thoughts are private. Be real. Be yourself. Get things off your chest. Write freely.

**No restrictions. No rules. Just you, thinking.**
"#;
            if let Err(e) = fs::write(agents_md_path, default_agents) {
                tracing::warn!("[core::registry] Failed to write default AGENTS.md: {}", e);
            }
        }

        // Create LEARNINGS.md if it doesn't exist (agent's private diary)
        let learnings_md_path = workspace_path.join("LEARNINGS.md");
        if !learnings_md_path.exists() {
            if let Err(e) = fs::write(
                learnings_md_path,
                "# My Diary\n\nPrivate thoughts and reflections.\n\n",
            ) {
                tracing::warn!(
                    "[core::registry] Failed to write default LEARNINGS.md: {}",
                    e
                );
            }
        }

        Ok(config)
    }

    /// Scaffolds a new agent workspace with config and identity files.
    pub fn scaffold_workspace(
        &self,
        agent_id: &str,
        soul_content: &str,
        model: Option<&str>,
    ) -> Result<AgentConfig, SavantError> {
        let workspace_path = self.base_path.join(agent_id);

        // Create workspace directory
        if !workspace_path.exists() {
            fs::create_dir_all(&workspace_path).map_err(SavantError::IoError)?;
        }

        // Parse default provider from config using canonical FromStr
        let default_provider: ModelProvider =
            ModelProvider::from_str(&self.defaults.model_provider).unwrap_or(ModelProvider::Ollama);

        let config = AgentConfig {
            agent_id: agent_id.to_string(),
            agent_name: agent_id
                .chars()
                .enumerate()
                .map(|(i, c)| {
                    if i == 0 {
                        c.to_uppercase().to_string()
                    } else {
                        c.to_string()
                    }
                })
                .collect(),
            model_provider: default_provider,
            api_key: None,
            env_vars: self.defaults.env_vars.clone(),
            system_prompt: self.defaults.system_prompt.clone(),
            model: model.map(|s| s.to_string()),
            heartbeat_interval: self.defaults.heartbeat_interval,
            allowed_skills: Vec::new(),
            workspace_path: workspace_path.clone(),
            identity: None,
            parent_id: None,
            session_id: None,
            proactive: crate::config::ProactiveConfig::default(),
            llm_params: crate::types::LlmParams::from_config(&self.ai_config),
            personality_traits: None,
            evolution_state: None,
            orchestrator_enabled: true,
            tier: AgentTier::Full,
        };

        // Write agent.json if it doesn't exist
        let config_path = workspace_path.join("agent.json");
        if !config_path.exists() {
            let file_config = AgentFileConfig {
                agent_id: Some(config.agent_id.clone()),
                agent_name: Some(config.agent_name.clone()),
                model: config.model.clone(),
                model_provider: Some(config.model_provider.as_str().to_string()),
                system_prompt: if config.system_prompt.is_empty() {
                    None
                } else {
                    Some(config.system_prompt.clone())
                },
                llm_params: Some(config.llm_params.clone()),
                heartbeat_interval: Some(config.heartbeat_interval),
                allowed_skills: Some(config.allowed_skills.clone()),
                env_vars: if config.env_vars.is_empty() {
                    None
                } else {
                    Some(config.env_vars.clone())
                },
                description: None,
                avatar: None,
                personality_traits: None,
                evolution_state: None,
            };
            let content = serde_json::to_string_pretty(&file_config)
                .map_err(|e| SavantError::ConfigError(e.to_string()))?;
            fs::write(&config_path, content).map_err(SavantError::IoError)?;
        }

        // Write SOUL.md if it doesn't exist
        let soul_path = workspace_path.join("SOUL.md");
        if !soul_path.exists() {
            if let Err(e) = fs::write(soul_path, soul_content) {
                tracing::warn!("[core::registry] Failed to write SOUL.md: {}", e);
            }
        }

        // Write AGENTS.md if it doesn't exist
        let agents_path = workspace_path.join("AGENTS.md");
        if !agents_path.exists() {
            let default_agents = "# Operating Instructions\n\nYou are a Savant autonomous agent.\n";
            if let Err(e) = fs::write(agents_path, default_agents) {
                tracing::warn!("[core::registry] Failed to write default AGENTS.md: {}", e);
            }
        }

        // Scaffold Obsidian memory vault directory structure
        let vault_path = workspace_path.join("memory-vault");
        if !vault_path.exists() {
            let vault_dirs = [
                vault_path.join(".obsidian"),
                vault_path.join("Episodic"),
                vault_path.join("Semantic"),
                vault_path.join("Identity").join("Evolution"),
                vault_path.join("Themes"),
                vault_path.join("Working"),
                vault_path.join("Dashboard"),
                vault_path.join(".stale"),
            ];
            for dir in &vault_dirs {
                if let Err(e) = fs::create_dir_all(dir) {
                    tracing::warn!(
                        "[core::registry] Failed to create vault dir {:?}: {}",
                        dir,
                        e
                    );
                }
            }

            let appearance_json = vault_path.join(".obsidian").join("appearance.json");
            let appearance_content = "{\"accentColor\":\"#00FFBB\",\"baseTheme\":\"obsidian\",\"interfaceFontFamily\":\"Inter\",\"textFontFamily\":\"Inter\",\"monospaceFontFamily\":\"JetBrains Mono\",\"translucency\":false,\"native\":false,\"enabledCssSnippets\":[],\"cssTheme\":\"\"}";
            if let Err(e) = fs::write(&appearance_json, appearance_content) {
                tracing::warn!("[core::registry] Failed to write appearance.json: {}", e);
            }

            let stale_gitignore = vault_path.join(".stale").join(".gitignore");
            if let Err(e) = fs::write(&stale_gitignore, "*\n") {
                tracing::warn!("[core::registry] Failed to write .stale/.gitignore: {}", e);
            }

            let index_md = vault_path.join("INDEX.md");
            let agent_name = &config.agent_name;
            let index_content = format!(
                "# {agent_name}'s Memory Tree\n\n\
                 > *Vault initialized on agent birth. Awaiting first sync.*\n\n\
                 ---\n\n\
                 ## Episodic\n\n\
                 ## Semantic\n\n\
                 ## Identity\n\n\
                 ## Themes\n\n\
                 ## Dashboard\n\n\
                 ---\n\n\
                 *This vault is a bidirectional projection of Savant's LSM+HNSW memory substrate.*\n"
            );
            if let Err(e) = fs::write(&index_md, index_content) {
                tracing::warn!("[core::registry] Failed to write INDEX.md: {}", e);
            }

            tracing::info!(
                "[core::registry] Obsidian vault scaffolded at {:?}",
                vault_path
            );
        }

        Ok(config)
    }

    /// Helper for legacy callers that only have the path
    pub fn scaffold_workspace_at_path(
        &self,
        workspace_path: &Path,
    ) -> Result<AgentConfig, SavantError> {
        let agent_id = workspace_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("agent");
        self.scaffold_workspace(
            agent_id,
            "# Persona\nYou are a Savant autonomous agent.",
            None,
        )
    }
}
