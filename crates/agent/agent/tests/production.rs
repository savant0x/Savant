#![allow(clippy::disallowed_methods)]
use async_trait::async_trait;
use futures::stream::{self, Stream};
use pqcrypto_dilithium::dilithium2;
use savant_agent::manager::AgentManager;
use savant_agent::swarm::SwarmController;
use savant_core::bus::NexusBridge;
use savant_core::config::{Config, IntegrationsConfig};
use savant_core::db::Storage;
use savant_core::error::SavantError;
use savant_core::traits::LlmProvider;
use savant_core::types::{
    AgentConfig, AgentIdentity, AgentOutputChannel, ChatChunk, ChatMessage, ModelProvider,
};
use savant_panopticon::replay::ReplayRecorder;
use std::collections::HashMap;
use std::pin::Pin;
use std::sync::Arc;

#[allow(dead_code)]
struct MockLlmProvider;

#[async_trait]
impl LlmProvider for MockLlmProvider {
    async fn stream_completion(
        &self,
        _messages: Vec<ChatMessage>,
        _tools: Vec<serde_json::Value>,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<ChatChunk, SavantError>> + Send>>, SavantError>
    {
        let chunk = ChatChunk {
            agent_name: "Mock".to_string(),
            agent_id: "mock-id".to_string(),
            content: "Mock response".to_string(),
            is_final: true,
            session_id: None,
            channel: AgentOutputChannel::Chat,
            logprob: None,
            is_telemetry: false,
            reasoning: None,
            tool_calls: None,
        };
        Ok(Box::pin(stream::iter(vec![Ok(chunk)])))
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn test_production_swarm_initialization_50_agents() {
    // 1. Setup temp environment
    let base_temp = std::env::temp_dir().join(format!("savant_test_{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&base_temp).expect("Failed to create base temp dir");

    let _storage_path = base_temp.join("test.db");
    let skills_path = base_temp.join("skills");
    let workspace_path = base_temp.join("workspace");
    let memory_path = base_temp.join("data/memory");

    std::fs::create_dir_all(&skills_path).unwrap();
    std::fs::create_dir_all(&workspace_path).unwrap();
    std::fs::create_dir_all(&memory_path).unwrap();

    // 2. Mock keys
    let mut rng = rand::thread_rng();
    let signing_key = ed25519_dalek::SigningKey::generate(&mut rng);
    let root_authority = signing_key.verifying_key();
    let (pqc_authority, pqc_signing_key) = dilithium2::keypair();

    // 3. Create dependencies
    let nexus = Arc::new(NexusBridge::new());
    let storage = Arc::new(
        Storage::new(base_temp.join("storage"), 100_000).expect("Failed to open test storage"),
    );

    let config = Config::default();
    let manager = Arc::new(AgentManager::new(config));

    // 4. Create 50 agents
    let mut agents = Vec::new();
    for i in 0..50 {
        let agent_id = format!("agent_{}", i);
        agents.push(AgentConfig {
            agent_id: agent_id.clone(),
            agent_name: agent_id,
            model_provider: ModelProvider::OpenRouter,
            api_key: Some("mock_key".to_string()),
            env_vars: HashMap::new(),
            system_prompt: "You are a test agent.".to_string(),
            model: Some("anthropic/claude-3-sonnet".to_string()),
            heartbeat_interval: 10,
            allowed_skills: Vec::new(),
            workspace_path: workspace_path.join(format!("agent_{}", i)),
            identity: Some(AgentIdentity::default()),
            parent_id: None,
            session_id: Some("test-session".to_string()),
            proactive: Default::default(),
            llm_params: Default::default(),
            personality_traits: None,
            evolution_state: None,
            orchestrator_enabled: true,
            tier: savant_core::types::AgentTier::Full,
        });
    }

    // 5. Initialize Controller
    let swarm_config = savant_agent::swarm::SwarmConfig {
        workspace_root: workspace_path.clone(),
        memory_db_path: base_temp.join("memory"),
        skills_path: skills_path.clone(),
        blackboard_name: format!("test_blackboard_{}", uuid::Uuid::new_v4()),
        collective_name: format!("test_collective_{}", uuid::Uuid::new_v4()),
        config_file: None,
        ..Default::default()
    };

    let controller = SwarmController::new(
        swarm_config,
        agents,
        storage,
        manager,
        nexus,
        root_authority,
        signing_key,
        pqc_authority,
        pqc_signing_key,
        vec![], // No MCP servers in test
        Arc::new(ReplayRecorder::new(1000)),
        IntegrationsConfig::default(),
        None, // No SchemaIndex in test
        None, // No LspManager in test
    )
    .await
    .expect("Failed to create SwarmController");

    // 6. Ignite
    controller.ignite().await;

    // 7. Verify health (Wait for agents to boot)
    tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;

    let dead_agents = controller.check_swarm_health().await;
    assert!(
        dead_agents.is_empty(),
        "Dead agents detected: {:?}",
        dead_agents
    );

    // 8. Verify IPC (Blackboard existence)
    // In a real scenario, we'd check if the agents are writing to the blackboard
}

#[tokio::test]
async fn test_agent_panic_recovery_logic() {
    // This test would verify that the SwarmController handles agent task completion/failure
    // Since SwarmController current doesn't auto-restart, we verify evacuation works.

    let base_temp =
        std::env::temp_dir().join(format!("savant_panic_test_{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&base_temp).unwrap();

    let signing_key = ed25519_dalek::SigningKey::generate(&mut rand::thread_rng());
    let root_authority = signing_key.verifying_key();
    let (pqc_authority, pqc_signing_key) = dilithium2::keypair();

    let swarm_config = savant_agent::swarm::SwarmConfig {
        workspace_root: base_temp.join("unstable_ws"),
        memory_db_path: base_temp.join("panic_memory"),
        skills_path: base_temp.join("skills"),
        blackboard_name: format!("panic_blackboard_{}", uuid::Uuid::new_v4()),
        collective_name: format!("panic_collective_{}", uuid::Uuid::new_v4()),
        config_file: None,
        ..Default::default()
    };

    let controller = SwarmController::new(
        swarm_config,
        vec![AgentConfig {
            agent_id: "unstable_agent".to_string(),
            agent_name: "Unstable".to_string(),
            model_provider: ModelProvider::OpenRouter,
            api_key: Some("mock".to_string()),
            env_vars: HashMap::new(),
            system_prompt: "test".to_string(),
            model: None,
            heartbeat_interval: 5,
            allowed_skills: Vec::new(),
            workspace_path: base_temp.join("unstable"),
            identity: None,
            parent_id: None,
            session_id: None,
            proactive: Default::default(),
            llm_params: Default::default(),
            personality_traits: None,
            evolution_state: None,
            orchestrator_enabled: true,
            tier: savant_core::types::AgentTier::Full,
        }],
        Arc::new(
            Storage::new(base_temp.join("panic_storage"), 100_000)
                .expect("Failed to open panic storage"),
        ),
        Arc::new(AgentManager::new(Config::default())),
        Arc::new(NexusBridge::new()),
        root_authority,
        signing_key,
        pqc_authority,
        pqc_signing_key,
        vec![], // No MCP servers in test
        Arc::new(ReplayRecorder::new(1000)),
        IntegrationsConfig::default(),
        None, // No SchemaIndex in test
        None, // No LspManager in test
    )
    .await
    .unwrap();

    controller.ignite().await;
    tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;

    controller.evacuate_agent("unstable_agent").await;

    // AAA: Allow time for the evacuation task to complete and reflect in state
    tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;

    let dead = controller.check_swarm_health().await;
    assert!(dead.contains(&"unstable_agent".to_string()));
}

#[tokio::test(flavor = "multi_thread")]
async fn test_500_agent_initialization_scaling() {
    // Audit-grade scaling verification
    let base_temp =
        std::env::temp_dir().join(format!("savant_scale_test_{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&base_temp).unwrap();

    let _storage_path = base_temp.join("scale.db");

    let mut rng = rand::thread_rng();
    let signing_key = ed25519_dalek::SigningKey::generate(&mut rng);
    let root_authority = signing_key.verifying_key();
    let (pqc_authority, pqc_signing_key) = dilithium2::keypair();

    let storage = Arc::new(
        Storage::new(base_temp.join("scale_storage"), 100_000)
            .expect("Failed to open scale storage"),
    );

    let nexus = Arc::new(NexusBridge::new());
    let manager = Arc::new(AgentManager::new(Config::default()));

    let mut agents = Vec::new();
    for i in 0..500 {
        let agent_id = format!("scale_agent_{}", i);
        agents.push(AgentConfig {
            agent_id: agent_id.clone(),
            agent_name: agent_id,
            model_provider: ModelProvider::OpenRouter,
            api_key: Some("mock".to_string()),
            env_vars: HashMap::new(),
            system_prompt: "Scale Test".to_string(),
            model: None,
            heartbeat_interval: 10,
            allowed_skills: Vec::new(),
            workspace_path: base_temp.join(format!("agent_{}", i)),
            identity: None,
            parent_id: None,
            session_id: Some("scale-session".to_string()),
            proactive: Default::default(),
            llm_params: Default::default(),
            personality_traits: None,
            evolution_state: None,
            orchestrator_enabled: true,
            tier: savant_core::types::AgentTier::Full,
        });
    }

    let swarm_config = savant_agent::swarm::SwarmConfig {
        workspace_root: base_temp.join("scale_ws"),
        memory_db_path: base_temp.join("scale_memory"),
        skills_path: base_temp.join("skills"),
        blackboard_name: format!("scale_blackboard_{}", uuid::Uuid::new_v4()),
        collective_name: format!("scale_collective_{}", uuid::Uuid::new_v4()),
        config_file: None,
        ..Default::default()
    };

    let controller = SwarmController::new(
        swarm_config,
        agents,
        storage,
        manager,
        nexus,
        root_authority,
        signing_key,
        pqc_authority,
        pqc_signing_key,
        vec![], // No MCP servers in test
        Arc::new(ReplayRecorder::new(1000)),
        IntegrationsConfig::default(),
        None, // No SchemaIndex in test
        None, // No LspManager in test
    )
    .await
    .expect("Failed to create Scale Controller");

    controller.ignite().await;

    // Scaling target: <5s for 500 agents on standard SSD
    tokio::time::sleep(tokio::time::Duration::from_secs(4)).await;

    let dead = controller.check_swarm_health().await;
    assert!(
        dead.is_empty(),
        "Scaling failure: agents failed to ignite at 500 count: {:?}",
        dead
    );
}
