use crate::config::{AiConfig, ProactiveConfig};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(transparent)]
pub struct SessionId(pub String);

/// Lifecycle phase of a turn.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TurnPhase {
    Processing,
    Completed,
    Failed,
    Interrupted,
    AwaitingApproval,
}

/// Session state — tracks metadata, turn count, and approval settings.
/// Persisted in CortexaDB's "sessions" collection (rkyv) and exposed via serde for API/Wire.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionState {
    pub session_id: String,
    pub created_at: i64,
    pub last_active: i64,
    pub turn_count: u64,
    pub active_turn_id: Option<String>,
    pub auto_approved_tools: Vec<String>,
    pub denied_tools: Vec<String>,
    #[serde(default)]
    pub parent_session_id: Option<String>,
    #[serde(default)]
    pub fork_point_turn_id: Option<String>,
}

impl SessionState {
    /// E3: Add a tool to the auto-approved list.
    pub fn allow_tool(&mut self, tool_name: &str) {
        if !self.auto_approved_tools.iter().any(|t| t == tool_name) {
            self.auto_approved_tools.push(tool_name.to_string());
        }
    }

    /// E3: Check if a tool is auto-approved.
    pub fn is_tool_allowed(&self, tool_name: &str) -> bool {
        self.auto_approved_tools.iter().any(|t| t == tool_name)
    }
}

/// Turn state — tracks lifecycle of a single user/agent turn.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TurnState {
    pub turn_id: String,
    pub session_id: String,
    pub state: TurnPhase,
    pub tool_calls_made: Vec<String>,
    pub started_at: i64,
    pub completed_at: i64,
}

/// Device ID type.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(transparent)]
pub struct DeviceId(pub String);

/// Control Frame for system operations
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", content = "data")]
pub enum ControlFrame {
    HistoryRequest {
        lane_id: String,
        limit: usize,
    },
    InitialSync,
    SoulManifest {
        prompt: String,
        name: Option<String>,
        bootstrap_tier: Option<BootstrapTier>,
    },
    SoulUpdate {
        agent_id: String,
        content: String,
    },
    BulkManifest {
        agents: Vec<AgentManifestPlan>,
    },
    SwarmInsightHistoryRequest {
        limit: usize,
    },
    // Skill management control frames
    SkillsList {
        agent_id: Option<String>,
    },
    SkillInstall {
        source: String, // ClawHub slug or URL
        agent_id: Option<String>,
    },
    SkillUninstall {
        skill_name: String,
        agent_id: Option<String>,
    },
    SkillEnable {
        skill_name: String,
    },
    SkillDisable {
        skill_name: String,
    },
    SkillScan {
        skill_path: String,
    },
    // Configuration control frames
    ConfigGet,
    ConfigSet {
        section: String, // "ai", "server", "skills", "memory", "security", "wasm", "system"
        key: String,
        value: serde_json::Value,
    },
    ModelsList,
    ParameterDescriptors,
    AgentConfigGet {
        agent_id: String,
    },
    AgentConfigSet {
        agent_id: String,
        model: Option<String>,
        model_provider: Option<String>,
        system_prompt: Option<String>,
        temperature: Option<f32>,
        top_p: Option<f32>,
        frequency_penalty: Option<f32>,
        presence_penalty: Option<f32>,
        max_tokens: Option<u32>,
        heartbeat_interval: Option<u64>,
        description: Option<String>,
    },
    /// Natural language command from dashboard
    NLCommand {
        text: String,
    },
    /// Triggers the BootstrapReconciler to process scaffold claims for an agent
    SoulScaffold {
        agent_id: String,
    },
    // ── Evolution System Control Frames ──
    SoulMutationPropose {
        agent_id: String,
        mutation_type: String, // "additive" | "subtractive" | "transformative"
        target_section: String,
        proposed_content: String,
        reasoning: String,
        conversations_triggered: Vec<String>,
        confidence: f32,
    },
    SoulMutationApprove {
        agent_id: String,
        mutation_id: String,
    },
    SoulMutationReject {
        agent_id: String,
        mutation_id: String,
        reason: String,
    },
    SoulMutationRevert {
        agent_id: String,
        target_hash: String,
    },
    EvolutionIdeaSubmit {
        agent_id: String,
        content: String,
        significance: f32,
    },
    EvolutionHistoryRequest {
        agent_id: String,
        limit: usize,
    },
    EvolutionScoreRequest {
        agent_id: String,
    },
    PersonalityExportRequest {
        agent_id: String,
    },
    PersonalityImportRequest {
        agent_id: String,
        payload: serde_json::Value,
    },
}

/// A plan for manifestations of a single agent
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentManifestPlan {
    pub name: String,
    pub soul: String,
    pub identity: Option<String>,
}

/// Request Payload
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum RequestPayload {
    ChatMessage(ChatMessage),
    ControlFrame(ControlFrame),
    Auth(String), // For DASHBOARD_LOGIN and legacy auth
}

/// Request Frame
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RequestFrame {
    #[serde(default)]
    pub request_id: String,
    pub session_id: SessionId,
    pub payload: RequestPayload,
    #[serde(default)]
    pub signature: Option<String>,
    #[serde(default)]
    pub timestamp: Option<i64>,
}

/// Response Frame
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResponseFrame {
    pub request_id: String,
    pub payload: String,
}

/// Event Frame
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventFrame {
    pub event_type: String,
    pub payload: String,
}

/// Chat Event
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatEvent {
    pub message: MessageContent,
}

/// Message Content
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageContent {
    pub text: String,
}

/// Chat roles for LLM interaction
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ChatRole {
    System,
    User,
    Assistant,
    Tool,
}

impl std::fmt::Display for ChatRole {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ChatRole::User => write!(f, "user"),
            ChatRole::Assistant => write!(f, "assistant"),
            ChatRole::System => write!(f, "system"),
            ChatRole::Tool => write!(f, "tool"),
        }
    }
}

impl std::str::FromStr for ChatRole {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "user" => Ok(ChatRole::User),
            "assistant" => Ok(ChatRole::Assistant),
            "system" => Ok(ChatRole::System),
            "tool" => Ok(ChatRole::Tool),
            _ => Err(format!("Invalid ChatRole: {}", s)),
        }
    }
}

/// A standardized chat message for LLM context
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChatMessage {
    pub role: ChatRole,
    pub content: String,
    #[serde(default)]
    pub sender: Option<String>,
    #[serde(default)]
    pub recipient: Option<String>, // None = Broadcast
    #[serde(default)]
    pub agent_id: Option<String>, // Stable ID for tracking
    #[serde(default)]
    pub session_id: Option<SessionId>, // AAA: Unified Context Harmony Anchor
    #[serde(default)]
    pub channel: AgentOutputChannel, // AAA: Consolidated Lane Isolation
    /// Marks non-dialogue messages (heartbeat reflections, system telemetry)
    /// that should go to the insights panel instead of the main chat.
    #[serde(default)]
    pub is_telemetry: bool,
    /// Marks this message as an error response from the agent loop.
    /// When true, the frontend renders error styling and offers RETRY/DISMISS.
    #[serde(default)]
    pub is_error: bool,
    /// Base64-encoded image attachments for multimodal models.
    /// When non-empty, the message is sent as a multimodal request.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<String>,
}

impl ChatMessage {
    /// Creates a new text-only chat message.
    pub fn new(role: ChatRole, content: impl Into<String>) -> Self {
        Self {
            role,
            content: content.into(),
            sender: None,
            recipient: None,
            agent_id: None,
            session_id: None,
            channel: AgentOutputChannel::default(),
            is_telemetry: false,
            is_error: false,
            images: Vec::new(),
        }
    }

    /// Creates a new multimodal chat message with base64-encoded images.
    pub fn with_images(role: ChatRole, content: impl Into<String>, images: Vec<String>) -> Self {
        Self {
            role,
            content: content.into(),
            sender: None,
            recipient: None,
            agent_id: None,
            session_id: None,
            channel: AgentOutputChannel::default(),
            is_telemetry: false,
            is_error: false,
            images,
        }
    }

    /// Attaches base64-encoded images to this message.
    pub fn with_images_mut(mut self, images: Vec<String>) -> Self {
        self.images = images;
        self
    }

    /// Migrates a legacy message format to the current ChatMessage format.
    pub fn from_legacy(legacy: crate::migration::LegacyMessage) -> Self {
        legacy.into()
    }
}

impl Default for ChatMessage {
    fn default() -> Self {
        Self {
            role: ChatRole::System,
            content: String::new(),
            sender: None,
            recipient: None,
            agent_id: None,
            session_id: None,
            channel: AgentOutputChannel::default(),
            is_telemetry: false,
            is_error: false,
            images: Vec::new(),
        }
    }
}
/// Native provider tool call structure
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderToolCall {
    pub id: String,
    pub name: String,
    pub arguments: String,
}

/// A streaming chunk of a chat message
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatChunk {
    pub agent_name: String,
    pub agent_id: String,
    pub content: String,
    pub is_final: bool,
    #[serde(default)]
    pub session_id: Option<SessionId>,
    #[serde(default)]
    pub channel: AgentOutputChannel,
    /// AAA: Shannon Entropy Logprob (Truth Verification)
    #[serde(default)]
    pub logprob: Option<f32>,
    /// Marks non-dialogue chunks (thoughts, telemetry) for dashboard routing
    #[serde(default)]
    pub is_telemetry: bool,
    /// Reasoning/thinking content from models that return it in a separate field
    #[serde(default)]
    pub reasoning: Option<String>,
    /// Native tool calls provided out-of-band
    #[serde(default)]
    pub tool_calls: Option<Vec<ProviderToolCall>>,
}

/// Strict Output Channels for Sovereign Lane Isolation
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum AgentOutputChannel {
    #[default]
    Chat, // User-facing dialogue
    Telemetry, // Internal health/status (heartbeats, logs)
    Memory,    // Distilled insights for long-term recall
}

/// Model Provider Enum
///
/// All string conversions use lowercase canonical form:
/// `"openai"`, `"anthropic"`, `"ollama"`, `"openrouter"`, `"lmstudio"`,
/// `"groq"`, `"perplexity"`, `"local"`, `"google"`, `"mistral"`,
/// `"cohere"`, `"together"`, `"deepseek"`, `"azure"`, `"xai"`,
/// `"fireworks"`, `"novita"`.
///
/// `FromStr` accepts any case. `as_str()` returns lowercase.
/// Serde uses lowercase `rename_all` for consistent TOML/JSON representation.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "lowercase")]
pub enum ModelProvider {
    OpenAi,
    Anthropic,
    Ollama,
    OpenRouter,
    LmStudio,
    Groq,
    Perplexity,
    Local,
    Google,
    Mistral,
    Cohere,
    Together,
    Deepseek,
    Azure,
    Xai,
    Fireworks,
    Novita,
    OpenGateway,
}

impl ModelProvider {
    /// Returns the canonical lowercase string for this provider.
    pub fn as_str(self) -> &'static str {
        match self {
            ModelProvider::OpenAi => "openai",
            ModelProvider::Anthropic => "anthropic",
            ModelProvider::Ollama => "ollama",
            ModelProvider::OpenRouter => "openrouter",
            ModelProvider::LmStudio => "lmstudio",
            ModelProvider::Groq => "groq",
            ModelProvider::Perplexity => "perplexity",
            ModelProvider::Local => "local",
            ModelProvider::Google => "google",
            ModelProvider::Mistral => "mistral",
            ModelProvider::Cohere => "cohere",
            ModelProvider::Together => "together",
            ModelProvider::Deepseek => "deepseek",
            ModelProvider::Azure => "azure",
            ModelProvider::Xai => "xai",
            ModelProvider::Fireworks => "fireworks",
            ModelProvider::Novita => "novita",
            ModelProvider::OpenGateway => "opengateway",
        }
    }
}

impl std::str::FromStr for ModelProvider {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "openai" => Ok(ModelProvider::OpenAi),
            "anthropic" => Ok(ModelProvider::Anthropic),
            "ollama" => Ok(ModelProvider::Ollama),
            "openrouter" => Ok(ModelProvider::OpenRouter),
            "lmstudio" => Ok(ModelProvider::LmStudio),
            "groq" => Ok(ModelProvider::Groq),
            "perplexity" => Ok(ModelProvider::Perplexity),
            "local" => Ok(ModelProvider::Local),
            "google" => Ok(ModelProvider::Google),
            "mistral" => Ok(ModelProvider::Mistral),
            "cohere" => Ok(ModelProvider::Cohere),
            "together" => Ok(ModelProvider::Together),
            "deepseek" => Ok(ModelProvider::Deepseek),
            "azure" => Ok(ModelProvider::Azure),
            "xai" => Ok(ModelProvider::Xai),
            "fireworks" => Ok(ModelProvider::Fireworks),
            "novita" => Ok(ModelProvider::Novita),
            "opengateway" => Ok(ModelProvider::OpenGateway),
            other => Err(format!("Unknown model provider: '{}'", other)),
        }
    }
}

impl std::fmt::Display for ModelProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// Agent Identity containing personality and metadata (OpenClaw compatible)
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AgentIdentity {
    pub name: String,
    pub soul: String,                 // SOUL.md: Persona & Tone
    pub instructions: Option<String>, // AGENTS.md: Rules & Operating instructions
    pub user_context: Option<String>, // USER.md: Who the user is
    pub metadata: Option<String>,     // IDENTITY.md: Name, vibe, emoji
    pub mission: Option<String>,
    pub expertise: Vec<String>,
    pub ethics: Option<String>,
    pub image: Option<String>, // Base64 or URL to agentimg.png
    pub internal_settings: Option<std::collections::HashMap<String, String>>, // Dynamic session/agent settings
    /// Current OCEAN personality traits (runtime-evolvable)
    #[serde(default)]
    pub personality_traits: Option<PersonalityTraits>,
    /// Blake3 hash of initial SOUL.md — used for drift comparison
    #[serde(default)]
    pub baseline_soul_hash: Option<String>,
}

/// OCEAN (Big Five) personality trait scores normalized 0.0–1.0
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersonalityTraits {
    pub openness: f32,
    pub conscientiousness: f32,
    pub extraversion: f32,
    pub agreeableness: f32,
    pub neuroticism: f32,
}

impl Default for PersonalityTraits {
    fn default() -> Self {
        Self {
            openness: 0.5,
            conscientiousness: 0.5,
            extraversion: 0.5,
            agreeableness: 0.5,
            neuroticism: 0.5,
        }
    }
}

impl PersonalityTraits {
    /// Applies a delta, clamping each trait to [0.0, 1.0].
    pub fn evolve(&self, delta: &PersonalityDelta) -> Self {
        Self {
            openness: (self.openness + delta.openness_delta).clamp(0.0, 1.0),
            conscientiousness: (self.conscientiousness + delta.conscientiousness_delta)
                .clamp(0.0, 1.0),
            extraversion: (self.extraversion + delta.extraversion_delta).clamp(0.0, 1.0),
            agreeableness: (self.agreeableness + delta.agreeableness_delta).clamp(0.0, 1.0),
            neuroticism: (self.neuroticism + delta.neuroticism_delta).clamp(0.0, 1.0),
        }
    }

    /// Computes Euclidean distance between this and another PersonalityTraits.
    pub fn distance(&self, other: &Self) -> f32 {
        ((self.openness - other.openness).powi(2)
            + (self.conscientiousness - other.conscientiousness).powi(2)
            + (self.extraversion - other.extraversion).powi(2)
            + (self.agreeableness - other.agreeableness).powi(2)
            + (self.neuroticism - other.neuroticism).powi(2))
        .sqrt()
    }
}

/// A single SOUL.md mutation proposal
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SoulMutation {
    pub id: String,
    pub agent_id: String,
    pub mutation_type: String, // "additive" | "subtractive" | "transformative"
    pub target_section: String,
    pub before_content: String,
    pub after_content: String,
    pub reasoning: String,
    pub confidence: f32,
    pub status: String, // "pending" | "approved" | "rejected"
    pub proposed_at: i64,
    pub decided_at: Option<i64>,
    pub conversations_triggered: Vec<String>,
    /// Session IDs and episodic evidence that triggered this mutation (for vault WikiLinks)
    #[serde(default)]
    pub source_evidence: Vec<String>,
    /// blake3 hash of before_content for dedup
    #[serde(default)]
    pub before_hash: String,
    /// OCEAN delta applied by this mutation (if personality-related)
    #[serde(default)]
    pub ocean_delta: Option<PersonalityDelta>,
}

/// Evolution state snapshot for tracking growth
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct EvolutionState {
    pub mutation_count: u32,
    pub last_mutation_at: Option<i64>,
    pub evolution_score: f32,
    /// Current stage: "Seedling" | "Growing" | "Mature" | "Sovereign"
    pub stage: String,
    /// Section → last mutation timestamp for digestion cooldown
    #[serde(default)]
    pub section_cooldowns: std::collections::HashMap<String, i64>,
    pub total_conversations: u32,
}

/// Agent Configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentConfig {
    pub agent_id: String,
    pub agent_name: String,
    pub model_provider: ModelProvider,
    // API key is loaded from .env at runtime and used to create derivative keys.
    /// For cloud providers, this is the API key.
    /// For local providers (Ollama, LMStudio), this is the provider URL.
    /// Convention: if the value starts with "http", it's a URL, not a key.
    #[serde(skip_serializing)]
    pub api_key: Option<String>,
    pub env_vars: std::collections::HashMap<String, String>,
    pub system_prompt: String,
    pub model: Option<String>,
    pub heartbeat_interval: u64,
    pub allowed_skills: Vec<String>,
    pub workspace_path: std::path::PathBuf,
    pub identity: Option<AgentIdentity>,
    pub parent_id: Option<String>,
    pub session_id: Option<String>,
    pub proactive: ProactiveConfig,
    /// Per-agent LLM parameters (overridden from agent.config.json)
    pub llm_params: LlmParams,
    /// Per-agent personality traits (evolvable)
    #[serde(default)]
    pub personality_traits: Option<PersonalityTraits>,
    /// Evolution state (mutation count, score, stage, cooldowns)
    #[serde(default)]
    pub evolution_state: Option<EvolutionState>,
    /// Whether the Orchestrator is enabled for this agent (default: true).
    /// When true, the agent uses Orchestrator::execute_turn() which adds
    /// A2A delegation, continuation, handoff validation, and DSP prediction.
    /// When false, the agent uses raw AgentLoop::run().
    #[serde(default = "default_orchestrator_enabled")]
    pub orchestrator_enabled: bool,
    /// Agent tier — Full or SubAgent.
    #[serde(default)]
    pub tier: AgentTier,
}

fn default_orchestrator_enabled() -> bool {
    true
}

/// Delta applied to personality traits when a mutation is approved.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersonalityDelta {
    pub openness_delta: f32,
    pub conscientiousness_delta: f32,
    pub extraversion_delta: f32,
    pub agreeableness_delta: f32,
    pub neuroticism_delta: f32,
    pub reason: String,
    #[serde(default)]
    pub source_interaction_ids: Vec<String>,
}

impl PersonalityDelta {
    pub fn new(reason: String) -> Self {
        Self {
            openness_delta: 0.0,
            conscientiousness_delta: 0.0,
            extraversion_delta: 0.0,
            agreeableness_delta: 0.0,
            neuroticism_delta: 0.0,
            reason,
            source_interaction_ids: Vec::new(),
        }
    }

    /// Computes Euclidean distance of this delta (for drift checking)
    pub fn euclidean_distance(&self) -> f32 {
        (self.openness_delta.powi(2)
            + self.conscientiousness_delta.powi(2)
            + self.extraversion_delta.powi(2)
            + self.agreeableness_delta.powi(2)
            + self.neuroticism_delta.powi(2))
        .sqrt()
    }
}

/// LLM parameters for fine-tuning agent behavior
/// These can be set per-agent via agent.config.json
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmParams {
    /// Temperature (0.0 - 2.0). Lower = more focused, Higher = more creative
    #[serde(default = "LlmParams::default_temperature")]
    pub temperature: f32,

    /// Top-p nucleus sampling (0.0 - 1.0)
    #[serde(default = "LlmParams::default_top_p")]
    pub top_p: f32,

    /// Frequency penalty (-2.0 - 2.0). Reduces repetition
    #[serde(default = "LlmParams::default_frequency_penalty")]
    pub frequency_penalty: f32,

    /// Presence penalty (-2.0 - 2.0). Encourages new topics
    #[serde(default = "LlmParams::default_presence_penalty")]
    pub presence_penalty: f32,

    /// Max tokens in response
    #[serde(default = "LlmParams::default_max_tokens")]
    pub max_tokens: u32,

    /// Request logprobs for entropy calculation
    #[serde(default)]
    pub logprobs: bool,

    /// Stop sequences
    #[serde(default)]
    pub stop: Vec<String>,

    /// AAA: JWT Secret for knowledge signing (Entropy-Arbiter)
    #[serde(default)]
    pub jwt_secret: Option<String>,
}

impl LlmParams {
    fn default_temperature() -> f32 {
        0.7
    }
    fn default_top_p() -> f32 {
        0.9
    }
    fn default_frequency_penalty() -> f32 {
        0.2
    }
    fn default_presence_penalty() -> f32 {
        0.1
    }
    fn default_max_tokens() -> u32 {
        256000
    }
}

impl Default for LlmParams {
    fn default() -> Self {
        Self {
            temperature: Self::default_temperature(),
            top_p: Self::default_top_p(),
            frequency_penalty: Self::default_frequency_penalty(),
            presence_penalty: Self::default_presence_penalty(),
            max_tokens: Self::default_max_tokens(),
            logprobs: false,
            stop: Vec::new(),
            jwt_secret: None,
        }
    }
}

impl LlmParams {
    /// Creates LLM parameters from a global AI configuration.
    pub fn from_config(config: &AiConfig) -> Self {
        Self {
            temperature: config.temperature,
            top_p: config.top_p,
            frequency_penalty: config.frequency_penalty,
            presence_penalty: config.presence_penalty,
            max_tokens: config.max_tokens,
            logprobs: false,
            stop: Vec::new(),
            jwt_secret: None,
        }
    }
}

/// Descriptor for a parameter that can be configured in the UI
/// Includes human-readable explanations for non-technical users
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParameterDescriptor {
    /// The parameter name (e.g., "temperature")
    pub name: String,
    /// Human-readable label (e.g., "Temperature")
    pub label: String,
    /// Detailed explanation of what this parameter does
    pub description: String,
    /// Simple explanation for non-technical users
    pub simple_description: String,
    /// Recommended range for this parameter
    pub min: f64,
    pub max: f64,
    /// Default value
    pub default: f64,
    /// Step size for UI sliders
    pub step: f64,
    /// Unit label if applicable (e.g., "tokens")
    pub unit: Option<String>,
    /// Tips for common use cases
    pub tips: Vec<String>,
}

impl LlmParams {
    /// Returns descriptors for all configurable parameters
    /// Used by the UI to display helpful explanations
    pub fn get_parameter_descriptors() -> Vec<ParameterDescriptor> {
        vec![
            ParameterDescriptor {
                name: "temperature".to_string(),
                label: "Temperature".to_string(),
                description: "Controls the randomness of the AI's responses. Lower values make the AI more deterministic and focused, while higher values make it more creative and varied. At 0.0, the AI will always choose the most likely next word. At 2.0, it takes much more risks with word choices.".to_string(),
                simple_description: "How creative the AI should be. Lower = more predictable, Higher = more creative".to_string(),
                min: 0.0,
                max: 2.0,
                default: 0.7,
                step: 0.1,
                unit: None,
                tips: vec![
                    "Use 0.0-0.3 for factual answers, coding, or math problems".to_string(),
                    "Use 0.4-0.7 for balanced responses (recommended default)".to_string(),
                    "Use 0.8-1.2 for creative writing or brainstorming".to_string(),
                    "Use 1.3-2.0 for highly creative or experimental outputs (may produce errors)".to_string(),
                ],
            },
            ParameterDescriptor {
                name: "top_p".to_string(),
                label: "Top-P (Nucleus Sampling)".to_string(),
                description: "Controls diversity by limiting which words the AI considers. At 1.0, all words are considered. At 0.5, only the top 50% most likely words are considered. This works with Temperature to fine-tune creativity. Lower values make responses more focused on the most common words.".to_string(),
                simple_description: "How many word choices to consider. Lower = safer words, Higher = more variety".to_string(),
                min: 0.0,
                max: 1.0,
                default: 1.0,
                step: 0.05,
                unit: None,
                tips: vec![
                    "Keep at 1.0 if you're adjusting Temperature (most common setting)".to_string(),
                    "Use 0.5-0.9 for more focused, reliable responses".to_string(),
                    "Use 0.1-0.5 for very conservative, predictable outputs".to_string(),
                    "Usually, adjust Temperature OR Top-P, not both".to_string(),
                ],
            },
            ParameterDescriptor {
                name: "frequency_penalty".to_string(),
                label: "Frequency Penalty".to_string(),
                description: "Reduces how often the AI repeats the same words or phrases. Higher values penalize words that have already appeared in the response, encouraging the AI to use new vocabulary. Negative values encourage repetition, which can be useful for certain tasks.".to_string(),
                simple_description: "Discourages the AI from repeating itself. Higher = less repetition".to_string(),
                min: -2.0,
                max: 2.0,
                default: 0.0,
                step: 0.1,
                unit: None,
                tips: vec![
                    "Use 0.0-0.5 for normal responses (recommended default)".to_string(),
                    "Use 0.5-1.0 if the AI repeats phrases too often".to_string(),
                    "Use 1.0-2.0 for creative writing where variety is important".to_string(),
                    "Rarely needed to go above 1.0 for most use cases".to_string(),
                ],
            },
            ParameterDescriptor {
                name: "presence_penalty".to_string(),
                label: "Presence Penalty".to_string(),
                description: "Encourages the AI to talk about new topics rather than staying on the same subject. Higher values penalize words that have appeared at all (even once), pushing the AI to explore different ideas. This is different from Frequency Penalty, which only penalizes repeated usage.".to_string(),
                simple_description: "Encourages the AI to explore new topics. Higher = more topic variety".to_string(),
                min: -2.0,
                max: 2.0,
                default: 0.0,
                step: 0.1,
                unit: None,
                tips: vec![
                    "Use 0.0 for normal conversations (recommended default)".to_string(),
                    "Use 0.3-0.8 for brainstorming or exploring multiple ideas".to_string(),
                    "Use 0.8-1.5 for creative writing with varied topics".to_string(),
                    "Setting too high may make responses feel disjointed".to_string(),
                ],
            },
            ParameterDescriptor {
                name: "max_tokens".to_string(),
                label: "Maximum Response Length".to_string(),
                description: "The maximum number of tokens (roughly words or parts of words) the AI can generate in a single response. One token is approximately 3/4 of a word in English. Longer responses take more time to generate and cost more to process. Set this based on how long you expect responses to be. Modern models support up to 1 million tokens.".to_string(),
                simple_description: "How long the AI's response can be. Higher = longer answers".to_string(),
                min: 1.0,
                max: 1000000.0,
                default: 4096.0,
                step: 256.0,
                unit: Some("tokens".to_string()),
                tips: vec![
                    "Use 1024-2048 for short answers or chat responses".to_string(),
                    "Use 4096 for balanced responses (recommended default)".to_string(),
                    "Use 8192-16384 for detailed explanations or documents".to_string(),
                    "Use 32768-131072 for long outputs like code files or reports".to_string(),
                    "Use 500000+ for massive outputs (supported by latest models)".to_string(),
                    "Note: Not all AI models support the same maximum length".to_string(),
                ],
            },
            ParameterDescriptor {
                name: "stop".to_string(),
                label: "Stop Sequences".to_string(),
                description: "Words or phrases that will cause the AI to stop generating text. When the AI encounters any of these sequences, it immediately stops its response. This is useful for controlling output format or preventing the AI from continuing past a certain point.".to_string(),
                simple_description: "Words that make the AI stop writing".to_string(),
                min: 0.0,
                max: 0.0,
                default: 0.0,
                step: 0.0,
                unit: None,
                tips: vec![
                    "Leave empty for most use cases (recommended default)".to_string(),
                    "Use for structured outputs where you need a specific endpoint".to_string(),
                    "Common examples: '\\n', 'END', '###'".to_string(),
                    "Can specify multiple stop sequences".to_string(),
                ],
            },
        ]
    }
}

/// On-disk config file format for per-agent settings
/// This is what users edit in agent.config.json
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub struct AgentFileConfig {
    /// Optional legacy agent ID
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,

    /// Optional legacy agent name
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_name: Option<String>,

    /// Override the model (e.g., "anthropic/claude-3-opus", "openai/gpt-4o")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,

    /// Override the model provider (e.g. "ollama", "openrouter", "anthropic")
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_provider: Option<String>,

    /// System prompt override
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system_prompt: Option<String>,

    /// LLM fine-tuning parameters
    #[serde(skip_serializing_if = "Option::is_none")]
    pub llm_params: Option<LlmParams>,

    /// Heartbeat interval in seconds
    #[serde(skip_serializing_if = "Option::is_none")]
    pub heartbeat_interval: Option<u64>,

    /// Allowed skill names (overrides global)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub allowed_skills: Option<Vec<String>>,

    /// Additional environment variables for this agent
    #[serde(skip_serializing_if = "Option::is_none")]
    pub env_vars: Option<std::collections::HashMap<String, String>>,

    /// Agent description for UI display
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,

    /// Agent avatar/icon for UI
    #[serde(skip_serializing_if = "Option::is_none")]
    pub avatar: Option<String>,

    /// OCEAN personality traits (evolvable per-agent)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub personality_traits: Option<PersonalityTraits>,

    /// Evolution state (mutation count, score, stage, cooldowns)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub evolution_state: Option<EvolutionState>,
}

impl AgentFileConfig {
    /// Load config from workspace directory, returning defaults if not found
    pub fn load(workspace_path: &std::path::Path) -> Result<Self, serde_json::Error> {
        let config_path = workspace_path.join("agent.config.json");
        if config_path.exists() {
            let content = std::fs::read_to_string(&config_path).map_err(serde_json::Error::io)?;
            serde_json::from_str(&content)
        } else {
            Ok(Self::default())
        }
    }

    /// Save config to workspace directory
    pub fn save(&self, workspace_path: &std::path::Path) -> Result<(), serde_json::Error> {
        let config_path = workspace_path.join("agent.config.json");
        let content = serde_json::to_string_pretty(self)?;
        std::fs::write(&config_path, content).map_err(serde_json::Error::io)
    }

    /// Apply file config on top of base AgentConfig
    pub fn apply_to(&self, base: &mut AgentConfig) {
        if let Some(ref id) = self.agent_id {
            base.agent_id = id.clone();
        }
        if let Some(ref name) = self.agent_name {
            base.agent_name = name.clone();
        }
        if let Some(ref model) = self.model {
            base.model = Some(model.clone());
        }
        if let Some(ref provider) = self.model_provider {
            match provider.parse::<ModelProvider>() {
                Ok(p) => base.model_provider = p,
                Err(e) => tracing::warn!("{}", e),
            };
        }
        if let Some(ref prompt) = self.system_prompt {
            base.system_prompt = prompt.clone();
        }
        if let Some(ref params) = self.llm_params {
            base.llm_params = params.clone();
        }
        if let Some(interval) = self.heartbeat_interval {
            base.heartbeat_interval = interval;
        }
        if let Some(ref skills) = self.allowed_skills {
            base.allowed_skills = skills.clone();
        }
        if let Some(ref vars) = self.env_vars {
            base.env_vars.extend(vars.clone());
        }
    }
}

/// Memory Category for specialized retrieval
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum MemoryCategory {
    Fact,
    Procedure,
    Correction,
    Preference,
    Observation,
    Reflection,
}

/// Memory Entry with Elite metadata
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryEntry {
    pub id: i64,
    pub timestamp: i64,
    pub content: String,
    pub category: MemoryCategory,
    pub importance: u8,            // 1-10 ranking for consolidation
    pub associations: Vec<String>, // Tags or linked concept IDs
    pub embedding: Option<Vec<f32>>,
}

/// Agent Reflection for self-improvement
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentReflection {
    pub task_id: String,
    pub success: bool,
    pub critique: String,
    pub learning: String,
    pub action_items: Vec<String>,
    pub importance: u8,
}

/// Heartbeat Task
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HeartbeatTask {
    pub id: String,
    pub schedule: String,
    pub command: String,
    pub last_run: Option<i64>,
    pub next_run: Option<i64>,
}

/// Represents the execution mode defined in the skill's manifest
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", content = "target")]
pub enum ExecutionMode {
    /// Modern, high-performance OCI WebAssembly Component
    WasmComponent(String),
    /// Legacy OpenClaw bash/python script requiring Landlock fallback
    LegacyNative(String),
    /// Docker container execution with full isolation
    DockerContainer(String),
    /// Nix flake execution with reproducible environment
    NixFlake(String),
    /// AWS Lambda function execution
    Lambda(String),
    /// Standalone WASM execution (fallback when WassetteExecutor unavailable)
    StandaloneWasm(Vec<u8>),
    /// Loaded as context, not executed
    Reference,
}

/// Explicit permission declarations to prevent silent data exfiltration
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct CapabilityGrants {
    #[serde(default)]
    pub fs_read: std::collections::HashSet<std::path::PathBuf>,
    #[serde(default)]
    pub fs_write: std::collections::HashSet<std::path::PathBuf>,
    #[serde(default)]
    pub network_allow: std::collections::HashSet<String>,
    #[serde(default)]
    pub requires_env: Vec<String>,
}

/// The parsed representation of an OpenClaw SKILL.md file
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillManifest {
    pub name: String,
    pub description: String,
    pub version: String,
    pub execution_mode: ExecutionMode,
    #[serde(default)]
    pub capabilities: CapabilityGrants,
    /// The raw markdown instructions to be injected into the LLM context
    #[serde(skip_deserializing, default)]
    pub instructions: String,
    /// Skills that must run before this skill (dependency resolution)
    #[serde(default)]
    pub depends_on: Vec<String>,
    /// Skills this skill can invoke (chaining)
    #[serde(default)]
    pub chain_with: Vec<String>,
}

/// A chain of skill invocations with conditional execution and data passing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillChain {
    pub name: String,
    pub steps: Vec<SkillChainStep>,
}

/// A single step in a skill chain.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillChainStep {
    pub skill_name: String,
    /// Optional condition for conditional execution (e.g., "podcast-guest-today")
    #[serde(default)]
    pub condition: Option<String>,
    /// How to pass output to the next step (e.g., "meeting-list")
    #[serde(default)]
    pub pass_output_as: Option<String>,
}

/// Bootstrap Tier — controls how aggressively the system grounds LLM-generated
/// agent souls in real infrastructure vs. allowing creative fabrication.
///
/// - `PureGeneration`: No grounding. LLM hallucinates freely. No manifest.json.
/// - `Grounded`: Injected system context. LLM must match reality exactly, no scaffolding.
/// - `Scaffolded`: Context + LLM can declare needs via structured requirements block.
///   System auto-creates the declared artifacts.
/// - `Aspirational`: All Tier 3 logic + unfulfillable claims classified as aspirational backlog.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum BootstrapTier {
    /// Pure Generation — No grounding. LLM hallucinates freely. No manifest.json.
    PureGeneration,
    /// Grounded — Injected system context. LLM must match reality exactly.
    Grounded,
    /// Scaffolded — Context + LLM can request scaffolding via structured infra block.
    Scaffolded,
    /// Aspirational — All Tier 3 + unfulfillable claims classified as backlog.
    Aspirational,
}

impl Default for BootstrapTier {
    fn default() -> Self {
        Self::Scaffolded
    }
}

/// Bootstrap Status — tracks the lifecycle state of an agent's infrastructure scaffolding.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash, Default)]
#[serde(rename_all = "snake_case")]
pub enum BootstrapStatus {
    /// Agent is fully bootstrapped and ready (default for existing agents).
    #[default]
    Ready,
    /// Soul generated but scaffolding not yet complete.
    Pending,
    /// Scaffolding partially failed; agent may run in degraded mode.
    Degraded,
}

/// Agent tier — distinguishes full workspace agents from ephemeral sub-agents.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum AgentTier {
    #[default]
    Full,
    SubAgent,
}

/// Agent role — determines delegation permissions within the tier hierarchy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum AgentRole {
    #[default]
    Main,
    Orchestrator,
    Leaf,
}

/// Resolves the role of a child agent based on parent role and depth.
pub fn resolve_agent_role(parent_role: AgentRole, depth: usize, max_depth: usize) -> AgentRole {
    if depth >= max_depth {
        AgentRole::Leaf
    } else {
        match parent_role {
            AgentRole::Main | AgentRole::Orchestrator => AgentRole::Orchestrator,
            AgentRole::Leaf => AgentRole::Leaf,
        }
    }
}

/// Sub-agent profile — defines identity, capabilities, and constraints.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubAgentProfile {
    pub name: String,
    pub soul: String,
    pub allowed_tools: Vec<String>,
    pub max_iterations: usize,
    pub timeout_secs: u64,
    pub can_delegate: bool,
    pub preferred_model: Option<String>,
    pub max_concurrent: usize,
    pub max_tokens: usize,
}

impl Default for SubAgentProfile {
    fn default() -> Self {
        Self {
            name: "general".into(),
            soul: "You are a versatile general-purpose agent.".into(),
            allowed_tools: Vec::new(),
            max_iterations: 50,
            timeout_secs: 300,
            can_delegate: false,
            preferred_model: None,
            max_concurrent: 8,
            max_tokens: 0,
        }
    }
}

/// Delegation request — passed to hooks before spawning.
#[derive(Debug, Clone)]
pub struct DelegationRequest {
    pub profile_name: String,
    pub task: String,
    pub context: String,
    pub depth: usize,
}

/// Delegation result — returned after sub-agent completion.
#[derive(Debug, Clone)]
pub struct DelegationResult {
    pub subagent_id: String,
    pub profile_name: String,
    pub output: String,
    pub iterations_used: usize,
    pub tokens_consumed: usize,
    pub success: bool,
    pub duration_ms: u64,
}

/// Delegation action — what to do after sub-agent completion.
#[derive(Debug, Clone)]
pub enum DelegationAction {
    Accept,
    Retry(String),
    Bail,
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)]
mod tests {
    use super::*;

    // ===== SessionId =====
    #[test]
    fn session_id_display() {
        let id = SessionId("abc-123".into());
        let json = serde_json::to_string(&id).unwrap();
        assert_eq!(json, r#""abc-123""#);
    }

    #[test]
    fn session_id_equality() {
        assert_eq!(SessionId("a".into()), SessionId("a".into()));
        assert_ne!(SessionId("a".into()), SessionId("b".into()));
    }

    #[test]
    fn session_id_in_hashset() {
        use std::collections::HashSet;
        let mut set = HashSet::new();
        set.insert(SessionId("dup".into()));
        set.insert(SessionId("dup".into()));
        set.insert(SessionId("other".into()));
        assert_eq!(set.len(), 2);
    }

    // ===== ChatRole =====
    #[test]
    fn chat_role_serialization() {
        assert_eq!(serde_json::to_string(&ChatRole::User).unwrap(), r#""user""#);
        assert_eq!(
            serde_json::to_string(&ChatRole::Assistant).unwrap(),
            r#""assistant""#
        );
        assert_eq!(
            serde_json::to_string(&ChatRole::System).unwrap(),
            r#""system""#
        );
    }

    #[test]
    fn chat_role_display() {
        assert_eq!(ChatRole::User.to_string(), "user");
        assert_eq!(ChatRole::Assistant.to_string(), "assistant");
        assert_eq!(ChatRole::System.to_string(), "system");
    }

    #[test]
    fn chat_role_from_str() {
        assert_eq!("user".parse::<ChatRole>().unwrap(), ChatRole::User);
        assert_eq!(
            "assistant".parse::<ChatRole>().unwrap(),
            ChatRole::Assistant
        );
        assert_eq!("system".parse::<ChatRole>().unwrap(), ChatRole::System);
        assert!("invalid".parse::<ChatRole>().is_err());
    }

    #[test]
    fn chat_role_from_str_case_insensitive() {
        assert_eq!("USER".parse::<ChatRole>().unwrap(), ChatRole::User);
        assert_eq!(
            "Assistant".parse::<ChatRole>().unwrap(),
            ChatRole::Assistant
        );
    }

    // ===== AgentOutputChannel =====
    #[test]
    fn agent_output_channel_default() {
        assert_eq!(AgentOutputChannel::default(), AgentOutputChannel::Chat);
    }

    #[test]
    fn agent_output_channel_serialization() {
        let json = serde_json::to_string(&AgentOutputChannel::Telemetry).unwrap();
        assert_eq!(json, r#""telemetry""#);
        let deserialized: AgentOutputChannel = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, AgentOutputChannel::Telemetry);
    }

    // ===== ChatMessage =====
    #[test]
    fn chat_message_defaults() {
        let msg = ChatMessage {
            is_telemetry: false,
            role: ChatRole::User,
            content: "hi".into(),
            sender: None,
            recipient: None,
            agent_id: None,
            session_id: None,
            channel: AgentOutputChannel::default(),
            images: Vec::new(),
            ..Default::default()
        };
        assert!(msg.sender.is_none());
        assert!(msg.recipient.is_none());
        assert!(msg.agent_id.is_none());
        assert_eq!(msg.channel, AgentOutputChannel::Chat);
        assert!(msg.images.is_empty());
    }

    #[test]
    fn chat_message_roundtrip() {
        let msg = ChatMessage {
            is_telemetry: false,
            role: ChatRole::Assistant,
            content: "Response".into(),
            sender: Some("agent-1".into()),
            recipient: Some("global".into()),
            agent_id: Some("id-1".into()),
            session_id: Some(SessionId("sess-1".into())),
            channel: AgentOutputChannel::Memory,
            images: Vec::new(),
            ..Default::default()
        };
        let json = serde_json::to_string(&msg).unwrap();
        let deserialized: ChatMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.role, ChatRole::Assistant);
        assert_eq!(deserialized.sender, Some("agent-1".into()));
        assert_eq!(deserialized.channel, AgentOutputChannel::Memory);
    }

    // ===== ControlFrame =====
    #[test]
    fn control_frame_all_variants_roundtrip() {
        let variants = vec![
            ControlFrame::InitialSync,
            ControlFrame::HistoryRequest {
                lane_id: "global".into(),
                limit: 10,
            },
            ControlFrame::SoulManifest {
                prompt: "test".into(),
                name: Some("A".into()),
                bootstrap_tier: None,
            },
            ControlFrame::SoulManifest {
                prompt: "test".into(),
                name: None,
                bootstrap_tier: None,
            },
            ControlFrame::SoulUpdate {
                agent_id: "a".into(),
                content: "c".into(),
            },
            ControlFrame::BulkManifest {
                agents: vec![AgentManifestPlan {
                    name: "X".into(),
                    soul: "s".into(),
                    identity: None,
                }],
            },
            ControlFrame::SwarmInsightHistoryRequest { limit: 50 },
        ];

        for variant in &variants {
            let json = serde_json::to_string(variant).unwrap();
            // Should not panic on deserialization
            let _value: serde_json::Value = serde_json::from_str(&json).unwrap();
        }
    }

    // ===== RequestPayload =====
    #[test]
    fn request_payload_chat_dispatch() {
        let json = r#"{"role":"user","content":"test"}"#;
        let payload: RequestPayload = serde_json::from_str(json).unwrap();
        match payload {
            RequestPayload::ChatMessage(msg) => assert_eq!(msg.content, "test"),
            _ => panic!("Expected ChatMessage"),
        }
    }

    #[test]
    fn request_payload_control_dispatch() {
        let json = r#"{"type":"InitialSync"}"#;
        let payload: RequestPayload = serde_json::from_str(json).unwrap();
        assert!(matches!(
            payload,
            RequestPayload::ControlFrame(ControlFrame::InitialSync)
        ));
    }

    #[test]
    fn request_payload_auth_dispatch() {
        let json = r#""bearer-token""#;
        let payload: RequestPayload = serde_json::from_str(json).unwrap();
        match payload {
            RequestPayload::Auth(token) => assert_eq!(token, "bearer-token"),
            _ => panic!("Expected Auth"),
        }
    }

    // ===== RequestFrame =====
    #[test]
    fn request_frame_defaults() {
        let json = serde_json::json!({
            "session_id": "s",
            "payload": {"role": "user", "content": "hi"}
        });
        let frame: RequestFrame = serde_json::from_value(json).unwrap();
        assert_eq!(frame.request_id, "");
        assert!(frame.signature.is_none());
        assert!(frame.timestamp.is_none());
    }

    #[test]
    fn request_frame_full_roundtrip() {
        let frame = RequestFrame {
            request_id: "r1".into(),
            session_id: SessionId("s1".into()),
            payload: RequestPayload::Auth("token".into()),
            signature: Some("sig".into()),
            timestamp: Some(1710000000),
        };
        let json = serde_json::to_string(&frame).unwrap();
        let deserialized: RequestFrame = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.request_id, "r1");
        assert_eq!(deserialized.signature, Some("sig".into()));
        assert_eq!(deserialized.timestamp, Some(1710000000));
    }

    // ===== EventFrame / ResponseFrame =====
    #[test]
    fn event_frame_roundtrip() {
        let frame = EventFrame {
            event_type: "chat.message".into(),
            payload: r#"{"c":"hi"}"#.into(),
        };
        let json = serde_json::to_string(&frame).unwrap();
        let d: EventFrame = serde_json::from_str(&json).unwrap();
        assert_eq!(d.event_type, "chat.message");
    }

    #[test]
    fn response_frame_roundtrip() {
        let frame = ResponseFrame {
            request_id: "r1".into(),
            payload: "ok".into(),
        };
        let json = serde_json::to_string(&frame).unwrap();
        let d: ResponseFrame = serde_json::from_str(&json).unwrap();
        assert_eq!(d.request_id, "r1");
        assert_eq!(d.payload, "ok");
    }

    // ===== MemoryCategory =====
    #[test]
    fn memory_category_all_variants() {
        let categories = [
            MemoryCategory::Fact,
            MemoryCategory::Procedure,
            MemoryCategory::Correction,
            MemoryCategory::Preference,
            MemoryCategory::Observation,
            MemoryCategory::Reflection,
        ];
        for cat in &categories {
            let json = serde_json::to_string(cat).unwrap();
            let deserialized: MemoryCategory = serde_json::from_str(&json).unwrap();
            assert_eq!(&deserialized, cat);
        }
    }

    // ===== ModelProvider =====
    #[test]
    fn model_provider_all_variants() {
        let providers = [
            ModelProvider::OpenAi,
            ModelProvider::Anthropic,
            ModelProvider::Ollama,
            ModelProvider::OpenRouter,
            ModelProvider::LmStudio,
            ModelProvider::Groq,
            ModelProvider::Perplexity,
            ModelProvider::Local,
        ];
        for p in &providers {
            let json = serde_json::to_string(p).unwrap();
            let d: ModelProvider = serde_json::from_str(&json).unwrap();
            assert_eq!(&d, p);
        }
    }

    // ===== ExecutionMode =====
    #[test]
    fn execution_mode_wasm_roundtrip() {
        let mode = ExecutionMode::WasmComponent("test.wasm".into());
        let json = serde_json::to_string(&mode).unwrap();
        let d: ExecutionMode = serde_json::from_str(&json).unwrap();
        match d {
            ExecutionMode::WasmComponent(t) => assert_eq!(t, "test.wasm"),
            _ => panic!("Expected WasmComponent"),
        }
    }

    #[test]
    fn execution_mode_legacy_roundtrip() {
        let mode = ExecutionMode::LegacyNative("script.py".into());
        let json = serde_json::to_string(&mode).unwrap();
        let d: ExecutionMode = serde_json::from_str(&json).unwrap();
        match d {
            ExecutionMode::LegacyNative(t) => assert_eq!(t, "script.py"),
            _ => panic!("Expected LegacyNative"),
        }
    }

    // ===== CapabilityGrants =====
    #[test]
    fn capability_grants_default_empty() {
        let grants = CapabilityGrants::default();
        assert!(grants.fs_read.is_empty());
        assert!(grants.fs_write.is_empty());
        assert!(grants.network_allow.is_empty());
        assert!(grants.requires_env.is_empty());
    }

    // ===== AgentManifestPlan =====
    #[test]
    fn agent_manifest_plan_roundtrip() {
        let plan = AgentManifestPlan {
            name: "Strategist".into(),
            soul: "# SOUL".into(),
            identity: Some("id-1".into()),
        };
        let json = serde_json::to_string(&plan).unwrap();
        let d: AgentManifestPlan = serde_json::from_str(&json).unwrap();
        assert_eq!(d.name, "Strategist");
        assert_eq!(d.identity, Some("id-1".into()));
    }

    // ===== DeviceId =====
    #[test]
    fn device_id_roundtrip() {
        let id = DeviceId("dev-1".into());
        let json = serde_json::to_string(&id).unwrap();
        let d: DeviceId = serde_json::from_str(&json).unwrap();
        assert_eq!(id, d);
    }

    // ===== HeartbeatTask =====
    #[test]
    fn heartbeat_task_roundtrip() {
        let task = HeartbeatTask {
            id: "hb-1".into(),
            schedule: "*/5 * * * *".into(),
            command: "health".into(),
            last_run: Some(1000),
            next_run: Some(1300),
        };
        let json = serde_json::to_string(&task).unwrap();
        let d: HeartbeatTask = serde_json::from_str(&json).unwrap();
        assert_eq!(d.id, "hb-1");
        assert_eq!(d.last_run, Some(1000));
    }

    // ===== AgentReflection =====
    #[test]
    fn agent_reflection_roundtrip() {
        let reflection = AgentReflection {
            task_id: "t1".into(),
            success: true,
            critique: "good".into(),
            learning: "keep going".into(),
            action_items: vec!["doc".into()],
            importance: 5,
        };
        let json = serde_json::to_string(&reflection).unwrap();
        let d: AgentReflection = serde_json::from_str(&json).unwrap();
        assert!(d.success);
        assert_eq!(d.importance, 5);
        assert_eq!(d.action_items.len(), 1);
    }

    // ===== MemoryEntry =====
    #[test]
    fn memory_entry_roundtrip() {
        let entry = MemoryEntry {
            id: 1,
            timestamp: 1710000000,
            content: "test".into(),
            category: MemoryCategory::Fact,
            importance: 7,
            associations: vec!["tag1".into()],
            embedding: Some(vec![0.1, 0.2, 0.3]),
        };
        let json = serde_json::to_string(&entry).unwrap();
        let d: MemoryEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(d.id, 1);
        assert_eq!(d.category, MemoryCategory::Fact);
        assert_eq!(d.importance, 7);
    }

    // ===== session_key / message_key helpers =====
    // Note: session_key() and message_key() are in savant_memory::models
}
