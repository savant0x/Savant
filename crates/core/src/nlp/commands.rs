//! Command execution — dispatches parsed intents to handlers.

use super::{CommandCategory, CommandIntent};
use crate::error::SavantError;

/// Executes a parsed command intent and returns a human-readable response.
pub async fn execute_command(intent: &CommandIntent) -> Result<String, SavantError> {
    match intent.category {
        CommandCategory::AgentManagement => execute_agent_command(intent).await,
        CommandCategory::ChannelControl => execute_channel_command(intent).await,
        CommandCategory::ModelSwitch => execute_model_command(intent).await,
        CommandCategory::Diagnostics => execute_diagnostics_command(intent).await,
        CommandCategory::Status => execute_status_command().await,
        CommandCategory::Help => Ok(help_text()),
        CommandCategory::Unknown => Ok(format!(
            "I don't understand: \"{}\"\n\nTry: \"show me all agents\", \"restart the discord bot\", \"switch to gemma4\", or \"help\"",
            intent.original
        )),
    }
}

async fn execute_agent_command(intent: &CommandIntent) -> Result<String, SavantError> {
    match intent.action.as_str() {
        "list" => Ok(
            "Agent listing requires swarm controller access. Query via WebSocket: send ControlFrame::AgentList to the gateway."
                .to_string(),
        ),
        "restart" => {
            if let Some(agent) = &intent.target {
                Ok(format!("Agent restart requires swarm controller access. Send ControlFrame::AgentRestart(\"{}\") via WebSocket to the gateway.", agent))
            } else {
                Ok("Which agent would you like to restart?".to_string())
            }
        }
        _ => Ok("Unknown agent command".to_string()),
    }
}

async fn execute_channel_command(intent: &CommandIntent) -> Result<String, SavantError> {
    if let Some(channel) = &intent.target {
        match intent.action.as_str() {
            "restart" => Ok(format!(
                "Channel '{}' restart requires channel pool access. Send ControlFrame::ChannelRestart(\"{}\") via WebSocket.",
                channel, channel
            )),
            "stop" => Ok(format!(
                "Channel '{}' disable requires channel pool access. Send ControlFrame::ChannelStop(\"{}\") via WebSocket.",
                channel, channel
            )),
            _ => Ok(format!("Unknown channel action: {}", intent.action)),
        }
    } else {
        Ok(
            "Which channel would you like to manage? (discord, telegram, whatsapp, matrix)"
                .to_string(),
        )
    }
}

async fn execute_model_command(intent: &CommandIntent) -> Result<String, SavantError> {
    if let Some(model) = &intent.target {
        Ok(format!(
            "Model switch to '{}' requires config access. Send ControlFrame::ConfigSet {{ section: \"model\", key: \"name\", value: \"{}\" }} via WebSocket.",
            model, model
        ))
    } else {
        Ok("Which model would you like to switch to? Try: gemma4, claude sonnet, gpt-5, deepseek v4, grok 4, or query /api/models for the full catalog.".to_string())
    }
}

async fn execute_diagnostics_command(intent: &CommandIntent) -> Result<String, SavantError> {
    match intent.action.as_str() {
        "memory_usage" => Ok(
            "Memory diagnostics: Query via WebSocket ControlFrame::MemoryStats or run `cargo test -p savant_memory` to verify engine health.".to_string()
        ),
        "failure_reason" => {
            if let Some(agent) = &intent.target {
                Ok(format!(
                    "Failure analysis for agent '{}': Query agent heartbeat logs via the dashboard timeline or WebSocket subscription.",
                    agent
                ))
            } else {
                Ok("Which agent failed? Provide the agent name for failure analysis.".to_string())
            }
        }
        _ => Ok("Unknown diagnostics command".to_string()),
    }
}

async fn execute_status_command() -> Result<String, SavantError> {
    Ok(
        "System status: Health check available via WebSocket ControlFrame::SystemStatus or dashboard connection indicator.".to_string()
    )
}

fn help_text() -> String {
    r#"Available commands:

  Agent Management:
    "show me all agents"          — List all agents
    "restart agent [name]"        — Restart a specific agent

  Channel Control:
    "restart the discord bot"     — Restart Discord channel
    "disable telegram"            — Disable Telegram channel
    "enable whatsapp"             — Enable WhatsApp channel

  Model Switching:
    "switch to gemma4"            — Change to local Gemma 4 (default)
    "switch to claude sonnet"     — Change to Claude Sonnet
    "switch to gpt-5"             — Change to GPT-5
    "switch to deepseek v4"       — Change to DeepSeek V4
    "use openrouter free"         — Use free cloud models
    See /api/models for the full catalog.

  Diagnostics:
    "what's using the most memory" — Memory diagnostics
    "why did agent [name] fail"    — Failure analysis

  Other:
    "status"                      — System health check
    "help"                        — This help text"#
        .to_string()
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)]
mod tests {
    use super::*;
    use crate::nlp::parse_command;

    #[tokio::test]
    async fn test_execute_list_agents() {
        let intent = parse_command("show me all agents");
        let result = execute_command(&intent).await;
        assert!(result.is_ok());
        assert!(result.unwrap().contains("Agent listing"));
    }

    #[tokio::test]
    async fn test_execute_restart_discord() {
        let intent = parse_command("restart the discord bot");
        let result = execute_command(&intent).await;
        assert!(result.is_ok());
        assert!(result.unwrap().contains("discord"));
    }

    #[tokio::test]
    async fn test_execute_help() {
        let intent = parse_command("help");
        let result = execute_command(&intent).await;
        assert!(result.is_ok());
        assert!(result.unwrap().contains("Available commands"));
    }

    #[tokio::test]
    async fn test_execute_unknown() {
        let intent = parse_command("do the flargle thing");
        let result = execute_command(&intent).await;
        assert!(result.is_ok());
        assert!(result.unwrap().contains("don't understand"));
    }

    #[tokio::test]
    async fn test_execute_switch_model() {
        let intent = parse_command("switch to gemma4");
        let result = execute_command(&intent).await;
        assert!(result.is_ok());
        assert!(result.unwrap().contains("gemma4"));
    }
}
