//! Natural Language Command Parser
//!
//! Parses natural language commands into structured intents that can be
//! dispatched to the appropriate handler. Supports:
//!
//! - Agent management: "show me all agents", "restart agent X"
//! - Channel control: "restart the discord bot", "enable telegram"
//! - Model switching: "switch to gemma4", "use claude sonnet", "switch to gpt-5"
//! - Diagnostics: "what's using the most memory?", "why did agent X fail?"
//! - Status: "show status", "system health"
//! - Help: "help", "what can you do"

pub mod commands;

use serde::{Deserialize, Serialize};

/// A parsed natural language command intent.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandIntent {
    /// The category of the command.
    pub category: CommandCategory,
    /// The specific action to take.
    pub action: String,
    /// The target (agent name, channel name, model name, etc.)
    pub target: Option<String>,
    /// Additional parameters extracted from the command.
    pub params: std::collections::HashMap<String, String>,
    /// Confidence score (0.0 - 1.0).
    pub confidence: f32,
    /// The original input text.
    pub original: String,
}

/// Command categories for routing.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum CommandCategory {
    /// List or manage agents.
    AgentManagement,
    /// Manage channels (Discord, Telegram, etc.).
    ChannelControl,
    /// Switch model or provider.
    ModelSwitch,
    /// Diagnostics and health checks.
    Diagnostics,
    /// General status information.
    Status,
    /// Help or documentation.
    Help,
    /// Unrecognized command.
    Unknown,
}

/// Parses a natural language string into a CommandIntent.
pub fn parse_command(input: &str) -> CommandIntent {
    let lower = input.to_lowercase().trim().to_string();

    // Try each parser in order of specificity
    if let Some(intent) = parse_agent_command(&lower, input) {
        return intent;
    }
    if let Some(intent) = parse_channel_command(&lower, input) {
        return intent;
    }
    if let Some(intent) = parse_model_command(&lower, input) {
        return intent;
    }
    if let Some(intent) = parse_diagnostics_command(&lower, input) {
        return intent;
    }
    if let Some(intent) = parse_status_command(&lower, input) {
        return intent;
    }
    if let Some(intent) = parse_help_command(&lower, input) {
        return intent;
    }

    CommandIntent {
        category: CommandCategory::Unknown,
        action: "unknown".to_string(),
        target: None,
        params: Default::default(),
        confidence: 0.0,
        original: input.to_string(),
    }
}

fn parse_agent_command(lower: &str, original: &str) -> Option<CommandIntent> {
    if lower.contains("show") && (lower.contains("agent") || lower.contains("all agent")) {
        return Some(CommandIntent {
            category: CommandCategory::AgentManagement,
            action: "list".to_string(),
            target: None,
            params: Default::default(),
            confidence: 0.9,
            original: original.to_string(),
        });
    }

    if lower.contains("restart") && lower.contains("agent") {
        let target = extract_after(lower, "agent");
        return Some(CommandIntent {
            category: CommandCategory::AgentManagement,
            action: "restart".to_string(),
            target,
            params: Default::default(),
            confidence: 0.85,
            original: original.to_string(),
        });
    }

    None
}

fn parse_channel_command(lower: &str, original: &str) -> Option<CommandIntent> {
    let channels = ["discord", "telegram", "whatsapp", "matrix"];

    for channel in &channels {
        if lower.contains(channel) {
            if lower.contains("restart") || lower.contains("enable") || lower.contains("start") {
                return Some(CommandIntent {
                    category: CommandCategory::ChannelControl,
                    action: "restart".to_string(),
                    target: Some(channel.to_string()),
                    params: Default::default(),
                    confidence: 0.9,
                    original: original.to_string(),
                });
            }
            if lower.contains("stop") || lower.contains("disable") {
                return Some(CommandIntent {
                    category: CommandCategory::ChannelControl,
                    action: "stop".to_string(),
                    target: Some(channel.to_string()),
                    params: Default::default(),
                    confidence: 0.9,
                    original: original.to_string(),
                });
            }
        }
    }

    None
}

fn parse_model_command(lower: &str, original: &str) -> Option<CommandIntent> {
    if lower.contains("switch to") || lower.contains("use") || lower.contains("change model") {
        // Try to extract model name
        // Comprehensive model keyword → model ID mappings (OpenRouter API, May 2026).
        // For the full live catalog with context windows, pricing, etc., query /api/models.
        let models = [
            // ── Local (Ollama) — user configures during setup ──
            ("gemma4:e2b", "gemma4:e2b"),
            ("gemma4:e4b", "gemma4:e4b"),
            ("gemma4:26b", "gemma4:26b"),
            ("gemma4:31b", "gemma4:31b"),
            ("gemma4", "gemma4"),
            ("gemma 4", "gemma4"),
            ("gemma", "gemma4"),
            // ── OpenAI ──
            ("gpt-5.5", "openai/gpt-5.5"),
            ("gpt-5.5 pro", "openai/gpt-5.5-pro"),
            ("gpt-5.4", "openai/gpt-5.4"),
            ("gpt-5.4 pro", "openai/gpt-5.4-pro"),
            ("gpt-5.4 mini", "openai/gpt-5.4-mini"),
            ("gpt-5.4 nano", "openai/gpt-5.4-nano"),
            ("gpt-5.4 image", "openai/gpt-5.4-image-2"),
            ("gpt-5.3", "openai/gpt-5.3-chat"),
            ("gpt-5.3 codex", "openai/gpt-5.3-codex"),
            ("gpt-5.2", "openai/gpt-5.2"),
            ("gpt-5.2 pro", "openai/gpt-5.2-pro"),
            ("gpt-5.1", "openai/gpt-5.1"),
            ("gpt-5.1 codex", "openai/gpt-5.1-codex"),
            ("gpt-5.1 codex max", "openai/gpt-5.1-codex-max"),
            ("gpt-5.1 codex mini", "openai/gpt-5.1-codex-mini"),
            ("gpt-5", "openai/gpt-5"),
            ("gpt-5 pro", "openai/gpt-5-pro"),
            ("gpt-5 mini", "openai/gpt-5-mini"),
            ("gpt-5 nano", "openai/gpt-5-nano"),
            ("gpt-5 codex", "openai/gpt-5-codex"),
            ("gpt-5 chat", "openai/gpt-5-chat"),
            ("gpt-5 image", "openai/gpt-5-image"),
            ("gpt-4o", "openai/gpt-4o"),
            ("gpt-4o mini", "openai/gpt-4o-mini"),
            ("gpt-4.1", "openai/gpt-4.1"),
            ("gpt-4.1 mini", "openai/gpt-4.1-mini"),
            ("gpt-4.1 nano", "openai/gpt-4.1-nano"),
            ("gpt-4 turbo", "openai/gpt-4-turbo"),
            ("gpt-4", "openai/gpt-4"),
            ("gpt chat latest", "openai/gpt-chat-latest"),
            ("gpt", "openai/gpt-5.4"),
            ("o1", "openai/o1"),
            ("o1 pro", "openai/o1-pro"),
            ("o3", "openai/o3"),
            ("o3 pro", "openai/o3-pro"),
            ("o3 mini", "openai/o3-mini"),
            ("o4 mini", "openai/o4-mini"),
            ("o4 mini high", "openai/o4-mini-high"),
            ("gpt-oss-120b", "openai/gpt-oss-120b"),
            ("gpt-oss-20b", "openai/gpt-oss-20b"),
            ("gpt oss", "openai/gpt-oss-120b"),
            // ── DeepSeek ──
            ("deepseek v4", "deepseek/deepseek-v4-pro"),
            ("deepseek v4 pro", "deepseek/deepseek-v4-pro"),
            ("deepseek v4 flash", "deepseek/deepseek-v4-flash"),
            ("deepseek v3.2", "deepseek/deepseek-v3.2"),
            ("deepseek v3.2 speciale", "deepseek/deepseek-v3.2-speciale"),
            ("deepseek v3.2 exp", "deepseek/deepseek-v3.2-exp"),
            ("deepseek v3.1", "deepseek/deepseek-chat-v3.1"),
            ("deepseek v3", "deepseek/deepseek-chat-v3-0324"),
            ("deepseek r1 0528", "deepseek/deepseek-r1-0528"),
            (
                "deepseek r1 llama 70b",
                "deepseek/deepseek-r1-distill-llama-70b",
            ),
            (
                "deepseek r1 qwen 32b",
                "deepseek/deepseek-r1-distill-qwen-32b",
            ),
            ("deepseek r1", "deepseek/deepseek-r1"),
            ("deepseek coder", "deepseek/deepseek-coder"),
            ("deepseek chat", "deepseek/deepseek-chat"),
            ("deepseek", "deepseek/deepseek-v4-pro"),
            // ── Anthropic ──
            ("claude opus 4.7", "anthropic/claude-opus-4.7"),
            ("claude opus 4.7 fast", "anthropic/claude-opus-4.7-fast"),
            ("claude opus 4.6", "anthropic/claude-opus-4.6"),
            ("claude opus 4.5", "anthropic/claude-opus-4.5"),
            ("claude opus 4.1", "anthropic/claude-opus-4.1"),
            ("claude opus 4", "anthropic/claude-opus-4"),
            ("claude opus", "anthropic/claude-opus-4.7"),
            ("claude sonnet 4.6", "anthropic/claude-sonnet-4.6"),
            ("claude sonnet 4.5", "anthropic/claude-sonnet-4.5"),
            ("claude sonnet 4", "anthropic/claude-sonnet-4"),
            ("claude sonnet", "anthropic/claude-sonnet-4.6"),
            ("claude haiku 4.5", "anthropic/claude-haiku-4.5"),
            ("claude haiku", "anthropic/claude-haiku-4.5"),
            ("claude 4.7", "anthropic/claude-opus-4.7"),
            ("claude 4.6", "anthropic/claude-sonnet-4.6"),
            ("claude", "anthropic/claude-sonnet-4.6"),
            // ── Google ──
            ("gemini 3.1 pro", "google/gemini-3.1-pro-preview"),
            ("gemini 3.1 flash lite", "google/gemini-3.1-flash-lite"),
            ("gemini 3.1 flash", "google/gemini-3.1-flash-lite"),
            ("gemini 3.1", "google/gemini-3.1-pro-preview"),
            ("gemini 3", "google/gemini-3-flash-preview"),
            ("gemini 2.5 pro", "google/gemini-2.5-pro"),
            ("gemini 2.5 flash", "google/gemini-2.5-flash"),
            ("gemini 2.5 flash lite", "google/gemini-2.5-flash-lite"),
            ("gemini 2.5", "google/gemini-2.5-pro"),
            ("gemini 2.0 flash", "google/gemini-2.0-flash"),
            ("gemini pro", "google/gemini-3.1-pro-preview"),
            ("gemini flash", "google/gemini-3.1-flash-lite"),
            ("gemini", "google/gemini-3.1-pro-preview"),
            // ── xAI ──
            ("grok 4.3", "x-ai/grok-4.3"),
            ("grok 4.20", "x-ai/grok-4.20"),
            ("grok 4.20 multi", "x-ai/grok-4.20-multi-agent"),
            ("grok 4.1 fast", "x-ai/grok-4.1-fast"),
            ("grok 4 fast", "x-ai/grok-4-fast"),
            ("grok 4", "x-ai/grok-4"),
            ("grok 3 mini", "x-ai/grok-3-mini"),
            ("grok 3", "x-ai/grok-3"),
            ("grok code fast", "x-ai/grok-code-fast-1"),
            ("grok", "x-ai/grok-4.3"),
            // ── Meta ──
            ("llama 4 maverick", "meta-llama/llama-4-maverick"),
            ("llama 4 scout", "meta-llama/llama-4-scout"),
            ("llama 4", "meta-llama/llama-4-maverick"),
            ("llama 3.3 70b", "meta-llama/llama-3.3-70b-instruct"),
            ("llama 3.2 3b", "meta-llama/llama-3.2-3b-instruct"),
            ("llama 3.2 1b", "meta-llama/llama-3.2-1b-instruct"),
            ("llama 3.1 405b", "meta-llama/llama-3.1-405b-instruct"),
            ("llama 3.1 70b", "meta-llama/llama-3.1-70b-instruct"),
            ("llama 3.1 8b", "meta-llama/llama-3.1-8b-instruct"),
            ("llama guard 4", "meta-llama/llama-guard-4-12b"),
            ("llama guard", "meta-llama/llama-guard-4-12b"),
            ("llama", "meta-llama/llama-4-maverick"),
            // ── Mistral ──
            ("mistral medium 3.5", "mistralai/mistral-medium-3-5"),
            ("mistral medium 3", "mistralai/mistral-medium-3"),
            ("mistral medium", "mistralai/mistral-medium-3-5"),
            ("mistral small 4", "mistralai/mistral-small-2603"),
            ("mistral small", "mistralai/mistral-small-2603"),
            ("mistral large 3", "mistralai/mistral-large-2512"),
            ("mistral large", "mistralai/mistral-large-2512"),
            ("mistral", "mistralai/mistral-medium-3-5"),
            ("mixtral 8x22b", "mistralai/mixtral-8x22b-instruct"),
            ("mixtral 8x7b", "mistralai/mixtral-8x7b-instruct"),
            ("mixtral", "mistralai/mixtral-8x22b-instruct"),
            ("codestral", "mistralai/codestral-2508"),
            ("devstral", "mistralai/devstral-medium"),
            ("devstral small", "mistralai/devstral-small"),
            ("ministral", "mistralai/ministral-8b-2512"),
            // ── Qwen ──
            ("qwen3.6", "qwen/qwen3.6-flash"),
            ("qwen3.6 flash", "qwen/qwen3.6-flash"),
            ("qwen3.6 35b", "qwen/qwen3.6-35b-a3b"),
            ("qwen3.6 27b", "qwen/qwen3.6-27b"),
            ("qwen3.6 max", "qwen/qwen3.6-max-preview"),
            ("qwen3.6 plus", "qwen/qwen3.6-plus"),
            ("qwen3.5 plus", "qwen/qwen3.5-plus-20260420"),
            ("qwen3.5 397b", "qwen/qwen3.5-397b-a17b"),
            ("qwen3.5 35b", "qwen/qwen3.5-35b-a3b"),
            ("qwen3.5 27b", "qwen/qwen3.5-27b"),
            ("qwen3.5 122b", "qwen/qwen3.5-122b-a10b"),
            ("qwen3.5 9b", "qwen/qwen3.5-9b"),
            ("qwen3.5 flash", "qwen/qwen3.5-flash-02-23"),
            ("qwen3.5", "qwen/qwen3.5-plus-20260420"),
            ("qwen3 coder next", "qwen/qwen3-coder-next"),
            ("qwen3 coder plus", "qwen/qwen3-coder-plus"),
            ("qwen3 coder flash", "qwen/qwen3-coder-flash"),
            ("qwen3 coder", "qwen/qwen3-coder"),
            ("qwen3 max", "qwen/qwen3-max"),
            ("qwen3 max thinking", "qwen/qwen3-max-thinking"),
            ("qwen3 235b", "qwen/qwen3-235b-a22b"),
            ("qwen3 30b", "qwen/qwen3-30b-a3b"),
            ("qwen3 14b", "qwen/qwen3-14b"),
            ("qwen3 8b", "qwen/qwen3-8b"),
            ("qwen3 vl 32b", "qwen/qwen3-vl-32b-instruct"),
            ("qwen3 vl 30b", "qwen/qwen3-vl-30b-a3b-instruct"),
            ("qwen3 vl 235b", "qwen/qwen3-vl-235b-a22b-instruct"),
            ("qwen3 vl", "qwen/qwen3-vl-32b-instruct"),
            ("qwen3 next 80b", "qwen/qwen3-next-80b-a3b-instruct"),
            ("qwen3", "qwen/qwen3.6-flash"),
            ("qwen2.5 coder", "qwen/qwen-2.5-coder-32b-instruct"),
            ("qwen2.5 72b", "qwen/qwen-2.5-72b-instruct"),
            ("qwen2.5", "qwen/qwen-2.5-72b-instruct"),
            ("qwen", "qwen/qwen3.6-flash"),
            // ── Z.ai / GLM ──
            ("glm 5.1", "z-ai/glm-5.1"),
            ("glm 5v turbo", "z-ai/glm-5v-turbo"),
            ("glm 5 turbo", "z-ai/glm-5-turbo"),
            ("glm 5", "z-ai/glm-5"),
            ("glm 4.7", "z-ai/glm-4.7"),
            ("glm 4.7 flash", "z-ai/glm-4.7-flash"),
            ("glm 4.6v", "z-ai/glm-4.6v"),
            ("glm 4.6", "z-ai/glm-4.6"),
            ("glm 4.5v", "z-ai/glm-4.5v"),
            ("glm 4.5", "z-ai/glm-4.5"),
            ("glm", "z-ai/glm-5"),
            // ── MiniMax ──
            ("minimax m2.7", "minimax/minimax-m2.7"),
            ("minimax m2.5", "minimax/minimax-m2.5"),
            ("minimax m2.1", "minimax/minimax-m2.1"),
            ("minimax m2", "minimax/minimax-m2"),
            ("minimax m1", "minimax/minimax-m1"),
            ("minimax", "minimax/minimax-m2.7"),
            // ── Moonshot / Kimi ──
            ("kimi k2.6", "moonshotai/kimi-k2.6"),
            ("kimi k2.5", "moonshotai/kimi-k2.5"),
            ("kimi k2 thinking", "moonshotai/kimi-k2-thinking"),
            ("kimi k2", "moonshotai/kimi-k2"),
            ("kimi", "moonshotai/kimi-k2.6"),
            // ── NVIDIA ──
            ("nemotron 3 super", "nvidia/nemotron-3-super-120b-a12b"),
            ("nemotron 3 nano", "nvidia/nemotron-3-nano-30b-a3b"),
            (
                "nemotron 3 nano omni",
                "nvidia/nemotron-3-nano-omni-30b-a3b-reasoning",
            ),
            ("nemotron nano 12b vl", "nvidia/nemotron-nano-12b-v2-vl"),
            ("nemotron nano 9b", "nvidia/nemotron-nano-9b-v2"),
            ("nemotron", "nvidia/nemotron-3-super-120b-a12b"),
            // ── Cohere ──
            ("command a", "cohere/command-a"),
            ("command r7b", "cohere/command-r7b-12-2024"),
            ("command r plus", "cohere/command-r-plus-08-2024"),
            ("command r", "cohere/command-r-08-2024"),
            ("command", "cohere/command-a"),
            // ── Perplexity ──
            ("sonar pro search", "perplexity/sonar-pro-search"),
            ("sonar reasoning pro", "perplexity/sonar-reasoning-pro"),
            ("sonar pro", "perplexity/sonar-pro"),
            ("sonar deep research", "perplexity/sonar-deep-research"),
            ("sonar", "perplexity/sonar"),
            // ── Nous Research ──
            ("hermes 4 70b", "nousresearch/hermes-4-70b"),
            ("hermes 4 405b", "nousresearch/hermes-4-405b"),
            ("hermes 4", "nousresearch/hermes-4-405b"),
            ("hermes 3 70b", "nousresearch/hermes-3-llama-3.1-70b"),
            ("hermes 3 405b", "nousresearch/hermes-3-llama-3.1-405b"),
            ("hermes", "nousresearch/hermes-4-405b"),
            // ── Arcee AI ──
            ("trinity large thinking", "arcee-ai/trinity-large-thinking"),
            ("trinity large", "arcee-ai/trinity-large-preview"),
            ("trinity mini", "arcee-ai/trinity-mini"),
            ("trinity", "arcee-ai/trinity-large-thinking"),
            ("spotlight", "arcee-ai/spotlight"),
            // ── Amazon Nova ──
            ("nova 2 lite", "amazon/nova-2-lite-v1"),
            ("nova premier", "amazon/nova-premier-v1"),
            ("nova lite", "amazon/nova-lite-v1"),
            ("nova pro", "amazon/nova-pro-v1"),
            ("nova micro", "amazon/nova-micro-v1"),
            ("nova", "amazon/nova-2-lite-v1"),
            // ── Other ──
            ("reka flash", "rekaai/reka-flash-3"),
            ("reka edge", "rekaai/reka-edge"),
            ("reka", "rekaai/reka-flash-3"),
            ("palmyra x5", "writer/palmyra-x5"),
            ("palmyra", "writer/palmyra-x5"),
            ("mercury 2", "inception/mercury-2"),
            ("mercury", "inception/mercury-2"),
            ("cogito v2", "deepcogito/cogito-v2.1-671b"),
            ("cogito", "deepcogito/cogito-v2.1-671b"),
            ("mimo v2.5", "xiaomi/mimo-v2.5"),
            ("mimo", "xiaomi/mimo-v2.5"),
            ("ernie 4.5", "baidu/ernie-4.5-300b-a47b"),
            ("ernie", "baidu/ernie-4.5-300b-a47b"),
            ("cobuddy", "baidu/cobuddy"),
            ("hy3", "tencent/hy3-preview"),
            ("hunyuan", "tencent/hunyuan-a13b-instruct"),
            ("ring", "inclusionai/ring-2.6-1t"),
            ("ling", "inclusionai/ling-2.6-1t"),
            ("laguna", "poolside/laguna-m.1"),
            ("intellect 3", "prime-intellect/intellect-3"),
            ("weaver", "mancer/weaver"),
            ("morph v3", "morph/morph-v3-large"),
            ("morph", "morph/morph-v3-large"),
            ("solar pro", "upstage/solar-pro-3"),
            ("granite 4.1", "ibm-granite/granite-4.1-8b"),
            ("granite", "ibm-granite/granite-4.1-8b"),
            ("jamba large", "ai21/jamba-large-1.7"),
            ("jamba", "ai21/jamba-large-1.7"),
            ("lfm2", "liquid/lfm-2-24b-a2b"),
            ("lfm", "liquid/lfm-2-24b-a2b"),
            ("seed 2", "bytedance-seed/seed-2.0-lite"),
            ("seed", "bytedance-seed/seed-2.0-lite"),
            ("ui tars", "bytedance/ui-tars-1.5-7b"),
            ("olmo 3", "allenai/olmo-3-32b-think"),
            ("olmo", "allenai/olmo-3-32b-think"),
            ("aion 2", "aion-labs/aion-2.0"),
            ("aion", "aion-labs/aion-2.0"),
            // ── OpenRouter Specialty ──
            ("openrouter free", "openrouter/free"),
            ("openrouter/free", "openrouter/free"),
            ("free router", "openrouter/free"),
            ("openrouter auto", "openrouter/auto"),
            ("auto router", "openrouter/auto"),
            ("owl alpha", "openrouter/owl-alpha"),
            ("pareto code", "openrouter/pareto-code"),
            // ── Latest aliases (shortcuts) ──
            ("opus latest", "~anthropic/claude-opus-latest"),
            ("sonnet latest", "~anthropic/claude-sonnet-latest"),
            ("haiku latest", "~anthropic/claude-haiku-latest"),
            ("gpt latest", "~openai/gpt-latest"),
            ("gpt mini latest", "~openai/gpt-mini-latest"),
            ("gemini pro latest", "~google/gemini-pro-latest"),
            ("gemini flash latest", "~google/gemini-flash-latest"),
            ("kimi latest", "~moonshotai/kimi-latest"),
        ];

        for (keyword, model_id) in &models {
            if lower.contains(keyword) {
                return Some(CommandIntent {
                    category: CommandCategory::ModelSwitch,
                    action: "switch".to_string(),
                    target: Some(model_id.to_string()),
                    params: Default::default(),
                    confidence: 0.9,
                    original: original.to_string(),
                });
            }
        }
    }

    None
}

fn parse_diagnostics_command(lower: &str, original: &str) -> Option<CommandIntent> {
    if lower.contains("memory") && (lower.contains("using") || lower.contains("most")) {
        return Some(CommandIntent {
            category: CommandCategory::Diagnostics,
            action: "memory_usage".to_string(),
            target: None,
            params: Default::default(),
            confidence: 0.8,
            original: original.to_string(),
        });
    }

    if lower.contains("why") && lower.contains("fail") {
        let target = extract_after_word(lower, &["agent", "fail"]);
        return Some(CommandIntent {
            category: CommandCategory::Diagnostics,
            action: "failure_reason".to_string(),
            target,
            params: Default::default(),
            confidence: 0.8,
            original: original.to_string(),
        });
    }

    None
}

fn parse_status_command(lower: &str, original: &str) -> Option<CommandIntent> {
    if lower.contains("status") || lower.contains("health") || lower.contains("how are you") {
        return Some(CommandIntent {
            category: CommandCategory::Status,
            action: "status".to_string(),
            target: None,
            params: Default::default(),
            confidence: 0.85,
            original: original.to_string(),
        });
    }

    None
}

fn parse_help_command(lower: &str, original: &str) -> Option<CommandIntent> {
    if lower.contains("help") || lower.contains("what can you do") || lower.contains("commands") {
        return Some(CommandIntent {
            category: CommandCategory::Help,
            action: "help".to_string(),
            target: None,
            params: Default::default(),
            confidence: 0.9,
            original: original.to_string(),
        });
    }

    None
}

/// Extract text after a keyword.
fn extract_after(input: &str, keyword: &str) -> Option<String> {
    if let Some(pos) = input.find(keyword) {
        let after = &input[pos + keyword.len()..].trim();
        if !after.is_empty() {
            return Some(after.to_string());
        }
    }
    None
}

/// Extract text after any of the given keywords.
fn extract_after_word(input: &str, keywords: &[&str]) -> Option<String> {
    for kw in keywords {
        if let Some(result) = extract_after(input, kw) {
            return Some(result);
        }
    }
    None
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_list_agents() {
        let intent = parse_command("show me all agents");
        assert_eq!(intent.category, CommandCategory::AgentManagement);
        assert_eq!(intent.action, "list");
        assert!(intent.confidence > 0.8);
    }

    #[test]
    fn test_parse_restart_discord() {
        let intent = parse_command("restart the discord bot");
        assert_eq!(intent.category, CommandCategory::ChannelControl);
        assert_eq!(intent.action, "restart");
        assert_eq!(intent.target, Some("discord".to_string()));
    }

    #[test]
    fn test_parse_switch_model() {
        let intent = parse_command("switch to gemma4");
        assert_eq!(intent.category, CommandCategory::ModelSwitch);
        assert_eq!(intent.action, "switch");
        assert_eq!(intent.target, Some("gemma4".to_string()));
    }

    #[test]
    fn test_parse_switch_model_openrouter() {
        let intent = parse_command("switch to gpt-5.4");
        assert_eq!(intent.category, CommandCategory::ModelSwitch);
        assert_eq!(intent.target, Some("openai/gpt-5.4".to_string()));
    }

    #[test]
    fn test_parse_stop_telegram() {
        let intent = parse_command("disable telegram");
        assert_eq!(intent.category, CommandCategory::ChannelControl);
        assert_eq!(intent.action, "stop");
        assert_eq!(intent.target, Some("telegram".to_string()));
    }

    #[test]
    fn test_parse_status() {
        let intent = parse_command("show status");
        assert_eq!(intent.category, CommandCategory::Status);
        assert_eq!(intent.action, "status");
    }

    #[test]
    fn test_parse_unknown() {
        let intent = parse_command("do something random with the flargle");
        assert_eq!(intent.category, CommandCategory::Unknown);
        assert!(intent.confidence < 0.1);
    }

    #[test]
    fn test_parse_help() {
        let intent = parse_command("what can you do");
        assert_eq!(intent.category, CommandCategory::Help);
        assert_eq!(intent.action, "help");
    }

    #[test]
    fn test_parse_memory() {
        let intent = parse_command("what's using the most memory");
        assert_eq!(intent.category, CommandCategory::Diagnostics);
        assert_eq!(intent.action, "memory_usage");
    }

    #[test]
    fn test_parse_restart_agent() {
        let intent = parse_command("restart agent alpha");
        assert_eq!(intent.category, CommandCategory::AgentManagement);
        assert_eq!(intent.action, "restart");
        assert_eq!(intent.target, Some("alpha".to_string()));
    }
}
