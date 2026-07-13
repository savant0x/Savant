use savant_core::config::Config;
use savant_core::error::SavantError;
use savant_core::fs::registry::AgentRegistry;
use savant_core::types::AgentConfig;

pub struct AgentManager {
    pub config: Config,
    pub registry: AgentRegistry,
}

impl AgentManager {
    pub fn new(config: Config) -> Self {
        let agents_path = config.resolve_path(&config.system.agents_path);
        tracing::info!(
            "AgentManager: Initializing with agents path: {:?}",
            agents_path
        );
        Self {
            config: config.clone(),
            registry: AgentRegistry::new(
                agents_path,
                config.ai.clone(),
                savant_core::config::AgentDefaults::default(),
            ),
        }
    }

    /// Boots an agent, performing setup if necessary.
    pub async fn boot_agent(&self, agent: AgentConfig) -> Result<AgentConfig, SavantError> {
        tracing::info!("Booting agent: {}", agent.agent_name);

        // Automatically scaffold uniform workspace subdirectories
        let skills_dir = agent.workspace_path.join("skills");
        if let Err(e) = tokio::fs::create_dir_all(&skills_dir).await {
            return Err(SavantError::Unknown(format!(
                "Failed to scaffold skills directory for agent {}: {}",
                agent.agent_name, e
            )));
        }

        Ok(agent)
    }

    /// Discovers all agents using the unified registry.
    pub async fn discover_agents(&self) -> Result<Vec<AgentConfig>, SavantError> {
        self.registry.discover_agents()
    }
}
