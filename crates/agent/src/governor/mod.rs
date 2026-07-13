//! Resource Governor — CPU/memory-aware agent spawning with adaptive concurrency.

mod adaptive;
mod monitor;
pub mod pressure;

pub use adaptive::AdaptiveSemaphore;
pub use monitor::ResourceMonitor;
pub use pressure::PressureLevel;

use savant_core::config::ResourceGovernorConfig;
use savant_core::types::AgentConfig;
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex, SemaphorePermit};
use tokio_util::sync::CancellationToken;

/// Orchestrates resource-aware agent spawning with tier-aware concurrency.
pub struct SwarmGovernor {
    pub monitor: Arc<ResourceMonitor>,
    semaphore: AdaptiveSemaphore,
    subagent_semaphore: AdaptiveSemaphore,
    config: ResourceGovernorConfig,
    deferred_agents: Arc<Mutex<Vec<(AgentConfig, u32)>>>,
    shutdown: CancellationToken,
    /// Channel for signaling the SwarmController to retry a deferred agent.
    /// The drain loop sends AgentConfig when a permit becomes available;
    /// the SwarmController receives and calls spawn_agent().
    retry_tx: mpsc::UnboundedSender<AgentConfig>,
    /// Receiver side — taken once by SwarmController via take_retry_rx().
    retry_rx: Mutex<Option<mpsc::UnboundedReceiver<AgentConfig>>>,
}

impl SwarmGovernor {
    pub fn new(config: ResourceGovernorConfig, shutdown: CancellationToken) -> Arc<Self> {
        let monitor = ResourceMonitor::new(config.clone(), shutdown.clone());
        let semaphore = AdaptiveSemaphore::new(monitor.clone(), config.clone());
        let subagent_semaphore = AdaptiveSemaphore::new(monitor.clone(), config.clone());
        let (retry_tx, retry_rx) = mpsc::unbounded_channel();
        Arc::new(Self {
            monitor,
            semaphore,
            subagent_semaphore,
            config,
            deferred_agents: Arc::new(Mutex::new(Vec::new())),
            shutdown,
            retry_tx,
            retry_rx: Mutex::new(Some(retry_rx)),
        })
    }

    /// Take the retry receiver (can only be called once).
    /// The caller (SwarmController) uses this to drain deferred agents that are
    /// ready to be retried.
    pub async fn take_retry_rx(&self) -> Option<mpsc::UnboundedReceiver<AgentConfig>> {
        self.retry_rx.lock().await.take()
    }

    /// Start background tasks (monitor + adaptive adjuster + deferred drain).
    pub fn start(self: &Arc<Self>) -> Vec<tokio::task::JoinHandle<()>> {
        let mut handles = Vec::new();

        // Start resource monitor
        handles.push(self.monitor.start());

        // Start adaptive permit adjuster
        let gov = self.clone();
        handles.push(tokio::spawn(async move {
            let interval = std::time::Duration::from_secs(gov.config.monitor_interval_secs.max(1));
            loop {
                tokio::select! {
                    _ = gov.shutdown.cancelled() => break,
                    _ = tokio::time::sleep(interval) => {
                        gov.semaphore.adjust_permits().await;
                    }
                }
            }
        }));

        // Start deferred agent drain loop
        // When a permit becomes available, sends the deferred agent through the
        // retry channel so the SwarmController can call spawn_agent().
        let gov = self.clone();
        handles.push(tokio::spawn(async move {
            let drain_interval = std::time::Duration::from_secs(5);
            loop {
                tokio::select! {
                    _ = gov.shutdown.cancelled() => break,
                    _ = tokio::time::sleep(drain_interval) => {
                        if let Some(agent) = gov.pop_deferred().await {
                            if gov.try_spawn().is_some() {
                                // Permit acquired — send to SwarmController for actual spawning
                                tracing::info!(
                                    "[governor] Permit available for '{}' — sending retry signal",
                                    agent.agent_name
                                );
                                if gov.retry_tx.send(agent).is_err() {
                                    tracing::warn!("[governor] Retry channel closed — drain loop exiting");
                                    break;
                                }
                            } else {
                                // No permits available — re-defer unchanged
                                gov.defer_agent_with_retries(agent, 0).await;
                            }
                        }
                    }
                }
            }
        }));

        handles
    }

    /// Try to acquire a spawn permit for a full agent. Returns None if pressure is too high.
    pub fn try_spawn(&self) -> Option<SemaphorePermit<'_>> {
        self.semaphore.try_acquire()
    }

    /// Try to acquire a spawn permit for a sub-agent. Returns None if pressure is too high.
    pub fn try_spawn_subagent(&self) -> Option<SemaphorePermit<'_>> {
        self.subagent_semaphore.try_acquire()
    }

    /// Queue an agent for deferred spawning.
    /// Uses `.lock().await` for backpressure — never silently drops.
    pub async fn defer_agent(&self, agent: AgentConfig) {
        self.defer_agent_with_retries(agent, 0).await;
    }

    /// Queue an agent for deferred spawning with a specific retry count.
    pub async fn defer_agent_with_retries(&self, agent: AgentConfig, retries: u32) {
        tracing::warn!(
            "[governor] Deferring agent '{}' — {} pressure, {} permits available, retry {}/{}",
            agent.agent_name,
            self.current_pressure(),
            self.available_permits(),
            retries,
            self.config.max_deferral_retries
        );
        let mut deferred = self.deferred_agents.lock().await;
        deferred.push((agent, retries));
    }

    /// Pop next deferred agent if retries not exhausted.
    /// The agent is removed from the queue. The caller must re-defer
    /// via `defer_agent()` if spawning fails.
    pub async fn pop_deferred(&self) -> Option<AgentConfig> {
        let mut deferred = self.deferred_agents.lock().await;
        if deferred.is_empty() {
            return None;
        }
        let (agent, retries) = deferred.remove(0);
        if retries >= self.config.max_deferral_retries {
            tracing::error!(
                "[governor] Dropping deferred agent '{}' — max retries ({}) exceeded",
                agent.agent_name,
                self.config.max_deferral_retries
            );
            None
        } else {
            Some(agent)
        }
    }

    /// Current pressure level.
    pub fn current_pressure(&self) -> PressureLevel {
        self.monitor.current_pressure()
    }

    /// Current CPU and memory percentages.
    pub fn current_metrics(&self) -> (f64, f64) {
        self.monitor.current_metrics()
    }

    /// Available permits.
    pub fn available_permits(&self) -> usize {
        self.semaphore.available_permits()
    }

    /// Whether governor is enabled.
    pub fn is_enabled(&self) -> bool {
        self.config.enabled
    }
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)]
mod tests {
    use super::*;

    fn test_config() -> ResourceGovernorConfig {
        ResourceGovernorConfig {
            enabled: true,
            monitor_interval_secs: 5,
            memory_medium_pct: 60.0,
            memory_high_pct: 80.0,
            memory_critical_pct: 92.0,
            cpu_medium_pct: 70.0,
            cpu_high_pct: 85.0,
            cpu_critical_pct: 95.0,
            max_agents_low: 128,
            max_agents_medium: 64,
            max_agents_high: 32,
            max_agents_critical: 8,
            max_deferral_retries: 3,
            smoothing_factor: 0.7,
        }
    }

    #[test]
    fn test_pressure_ordering() {
        assert!(PressureLevel::Low < PressureLevel::Medium);
        assert!(PressureLevel::Medium < PressureLevel::High);
        assert!(PressureLevel::High < PressureLevel::Critical);
    }

    #[test]
    fn test_governor_creation() {
        let shutdown = CancellationToken::new();
        let gov = SwarmGovernor::new(test_config(), shutdown);
        assert!(gov.is_enabled());
        assert_eq!(gov.current_pressure(), PressureLevel::Low);
    }

    #[tokio::test]
    async fn test_deferred_agent_queue() {
        let shutdown = CancellationToken::new();
        let gov = SwarmGovernor::new(test_config(), shutdown);

        let agent = AgentConfig {
            agent_id: "test".into(),
            agent_name: "test".into(),
            model_provider: savant_core::types::ModelProvider::Ollama,
            api_key: None,
            env_vars: Default::default(),
            system_prompt: String::new(),
            model: None,
            heartbeat_interval: 60,
            allowed_skills: Vec::new(),
            workspace_path: std::path::PathBuf::new(),
            identity: None,
            parent_id: None,
            session_id: None,
            proactive: savant_core::config::ProactiveConfig::default(),
            llm_params: savant_core::types::LlmParams::default(),
            personality_traits: None,
            evolution_state: None,
            orchestrator_enabled: true,
            tier: savant_core::types::AgentTier::Full,
        };

        gov.defer_agent(agent).await;
        let popped = gov.pop_deferred().await;
        assert!(popped.is_some());
        assert_eq!(popped.unwrap().agent_name, "test");
    }

    #[tokio::test]
    async fn test_pop_deferred_no_duplicate() {
        let shutdown = CancellationToken::new();
        let gov = SwarmGovernor::new(test_config(), shutdown);

        let agent = AgentConfig {
            agent_id: "test".into(),
            agent_name: "test".into(),
            model_provider: savant_core::types::ModelProvider::Ollama,
            api_key: None,
            env_vars: Default::default(),
            system_prompt: String::new(),
            model: None,
            heartbeat_interval: 60,
            allowed_skills: Vec::new(),
            workspace_path: std::path::PathBuf::new(),
            identity: None,
            parent_id: None,
            session_id: None,
            proactive: savant_core::config::ProactiveConfig::default(),
            llm_params: savant_core::types::LlmParams::default(),
            personality_traits: None,
            evolution_state: None,
            orchestrator_enabled: true,
            tier: savant_core::types::AgentTier::Full,
        };

        gov.defer_agent(agent).await;

        // Pop the agent — it should be removed from the queue
        let popped = gov.pop_deferred().await;
        assert!(popped.is_some());

        // Second pop should return None — no duplicate
        let second = gov.pop_deferred().await;
        assert!(
            second.is_none(),
            "pop_deferred() should not return the same agent twice"
        );
    }

    #[tokio::test]
    async fn test_adaptive_semaphore_permits() {
        let shutdown = CancellationToken::new();
        let gov = SwarmGovernor::new(test_config(), shutdown);
        let _handles = gov.start();

        assert!(gov.available_permits() >= 1);
    }
}
