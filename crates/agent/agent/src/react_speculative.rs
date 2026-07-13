//! Speculative Execution Extension for AgentLoop
//!
//! This module implements the DSP-accelerated execution mode where the LLM
//! is prompted to provide multiple tool calls in a single response, enabling
//! reduced latency through speculation.

use crate::react::AgentLoop;
use futures::stream::{Stream, StreamExt};
use savant_core::error::SavantError;
use savant_core::traits::MemoryBackend;
use savant_core::types::{ChatMessage, ChatRole};
use std::pin::Pin;

/// Events that can be emitted during speculative execution.
#[derive(Debug, Clone)]
pub enum SpeculativeEvent {
    /// A thought/reasoning chunk from the LLM
    Thought(String),
    /// A tool action that was parsed from the response
    Action { name: String, args: String },
    /// Observation from tool execution
    Observation(String),
    /// Final answer produced after all speculation steps
    FinalAnswer(String),
    /// Reflection on the completed execution
    Reflection(String),
    /// A speculative prediction (before validation)
    Speculation {
        step: u32,
        tool_name: String,
        args: String,
    },
    /// Validation result (tool call confirmed)
    Validation {
        step: u32,
        success: bool,
        observation: Option<String>,
    },
}

/// Extension trait for AgentLoop that adds speculative execution capability.
impl<M: MemoryBackend> AgentLoop<M> {
    /// Executes a turn with speculative horizon.
    ///
    /// This method implements the DSP speculation pattern:
    /// 1. Instructs the LLM to predict k steps ahead
    /// 2. Parses k tool calls from a single response
    /// 3. Executes them in sequence
    /// 4. Validates results and continues as needed
    ///
    /// The response format expected from the LLM:
    /// ```text
    /// Thought: I need to do X
    /// Action: tool_name {"arg": "value"}
    /// Thought: Then I need to do Y
    /// Action: another_tool {"param": "data"}
    /// Thought: Finally...
    /// Answer: final result
    /// ```
    ///
    /// # Arguments
    /// * `input` - User input/instruction
    /// * `horizon_k` - Number of speculative steps to request (DSP output)
    ///
    /// # Returns
    /// A stream of events that can be consumed incrementally
    pub fn execute_with_horizon<'a>(
        &'a mut self,
        input: &'a str,
        horizon_k: u32,
    ) -> Pin<Box<dyn Stream<Item = Result<SpeculativeEvent, SavantError>> + Send + 'a>> {
        // Build initial history with user input
        let history = vec![ChatMessage {
            is_telemetry: false,
            role: ChatRole::User,
            content: input.to_string(),
            sender: None,
            recipient: None,
            agent_id: None,
            session_id: None, // Will be set by AgentLoop if needed
            channel: savant_core::types::AgentOutputChannel::Chat,
            images: Vec::new(),
            ..Default::default()
        }];

        // Horizon instruction prefix
        let horizon_instruction = if horizon_k > 1 {
            format!(
                "You are requested to think ahead up to {} steps.",
                horizon_k
            )
        } else {
            "Provide reasoning and a single tool action.".to_string()
        };

        Box::pin({
            use async_stream::stream;
            stream! {
                let mut depth = 0;
                let max_depth = horizon_k.max(1) as usize;
                let mut speculative_steps = Vec::new();

                // Phase 1: Speculative Gathering
                while depth < max_depth {
                    // Assemble context with horizon instruction.
                    // Inject observations from previous speculative steps so the LLM
                    // can build on prior results.
                    let mut current_history = self.memory.retrieve(&self.agent_id, input, 10).await?;
                    for (step_name, step_args, step_obs) in &speculative_steps {
                        current_history.push(ChatMessage {
                            is_telemetry: false,
                            role: ChatRole::User,
                            content: format!(
                                "Previous step: {} with args {} produced observation: {}",
                                step_name, step_args, step_obs
                            ),
                            sender: None,
                            recipient: None,
                            agent_id: None,
                            session_id: None,
                            channel: savant_core::types::AgentOutputChannel::Chat,
                            images: Vec::new(),
                            ..Default::default()
                        });
                    }
                    current_history.insert(0, ChatMessage {
                        is_telemetry: false,
                        role: ChatRole::System,
                        content: horizon_instruction.clone(),
                        sender: None,
                        recipient: None,
                        agent_id: None,
                        session_id: None,
                        channel: savant_core::types::AgentOutputChannel::Chat,
                        images: Vec::new(),
                        ..Default::default()
                    });
                    current_history.extend(history.clone());

                    // Build messages with system instruction about horizon
                    let messages: Vec<ChatMessage> = self.context.build_messages(current_history);

                    // LLM inference
                    let response_stream = self.provider.stream_completion(messages, vec![]).await;
                    let mut full_text = String::new();

                    let mut llm_stream = match response_stream {
                        Ok(s) => s,
                        Err(e) => {
                            yield Err(e);
                            return;
                        }
                    };

                    while let Some(chunk_res) = llm_stream.next().await {
                        match chunk_res {
                            Ok(chunk) => {
                                if !chunk.content.is_empty() {
                                    full_text.push_str(&chunk.content);
                                    yield Ok(SpeculativeEvent::Thought(chunk.content));
                                }
                                if chunk.is_final { break; }
                            }
                            Err(e) => {
                                yield Err(e);
                                return;
                            }
                        }
                    }

                    // Parse the thought and action from this step
                    let (_thought, action_opt): (String, Option<(String, String)>) = parse_thought_and_action(&full_text);

                    if let Some((tool_name, args)) = action_opt {
                        yield Ok(SpeculativeEvent::Action { name: tool_name.clone(), args: args.clone() });

                        // Execute tool and capture observation for next step
                        match self.execute_tool(&tool_name, &args).await {
                            Ok(observation) => {
                                yield Ok(SpeculativeEvent::Observation(observation.clone()));
                                speculative_steps.push((tool_name, args, observation));
                            }
                            Err(e) => {
                                let err_obs = format!("Error: {}", e);
                                tracing::warn!("[agent::speculative] Failed to execute tool {}: {}", tool_name, e);
                                yield Ok(SpeculativeEvent::Observation(err_obs.clone()));
                                speculative_steps.push((tool_name, args, err_obs));
                            }
                        }
                    } else {
                        // No action found - might be final answer or reflection
                        break;
                    }

                    depth += 1;
                }

                // Phase 2: Final Answer Generation
                // Check if the last response already contains a structured Answer.
                // If the last observation contains a tool result with a final answer,
                // use that directly. Otherwise, generate a reflection synthesizing all observations.
                let final_reflection: String = self.generate_reflection(&history, "").await?;
                yield Ok(SpeculativeEvent::Reflection(final_reflection.clone()));

                // Store final answer in memory
                let final_msg = ChatMessage {
                    is_telemetry: false,
                    role: ChatRole::Assistant,
                    content: final_reflection.clone(),
                    sender: Some(self.agent_id.clone()),
                    recipient: None,
                    agent_id: None,
                    session_id: None, // Speculative reflection
                    channel: savant_core::types::AgentOutputChannel::Chat,
                    images: Vec::new(),
                    ..Default::default()
                };
                self.memory.store(&self.agent_id, &final_msg).await?;

                yield Ok(SpeculativeEvent::FinalAnswer(final_reflection));
            }
        })
    }
}

/// Parses the thought and action from an LLM response.
///
/// Expected format:
/// ```text
/// Thought: I'll fetch the weather
/// Action: weather_tool {"city": "Paris"}
/// ```
fn parse_thought_and_action(text: &str) -> (String, Option<(String, String)>) {
    let lines: Vec<_> = text.lines().collect();
    let mut thought = String::new();
    let mut action_name = None;
    let mut action_args = None;

    for line in lines {
        if line.trim().to_lowercase().starts_with("thought:") {
            thought = line.trim()[8..].trim().to_string();
        } else if line.trim().to_lowercase().starts_with("action:") {
            let action_part = line.trim()[7..].trim();
            // Try to parse as JSON for args extraction
            if let Some(open_brace) = action_part.find('{') {
                let name_part = action_part[..open_brace].trim();
                let args_part = &action_part[open_brace..];
                action_name = Some(name_part.to_string());
                action_args = Some(args_part.to_string());
            } else {
                // No JSON args, treat whole as name with empty args
                action_name = Some(action_part.to_string());
                action_args = Some("{}".to_string());
            }
        }
    }

    (thought, action_name.zip(action_args))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_thought_and_action() {
        let text = "Thought: I'll check the weather\nAction: weather {\"city\": \"Paris\"}";
        let (thought, action) = parse_thought_and_action(text);
        assert_eq!(thought, "I'll check the weather");
        assert_eq!(
            action,
            Some(("weather".to_string(), "{\"city\": \"Paris\"}".to_string()))
        );
    }

    #[test]
    fn test_parse_thought_and_action_no_args() {
        let text = "Thought: Simple action\nAction: simple_tool";
        let (thought, action) = parse_thought_and_action(text);
        assert_eq!(thought, "Simple action");
        assert_eq!(action, Some(("simple_tool".to_string(), "{}".to_string())));
    }

    #[test]
    fn test_parse_thought_and_action_no_action() {
        let text = "Thought: I'm done\nAnswer: The weather is sunny";
        let (thought, action) = parse_thought_and_action(text);
        assert_eq!(thought, "I'm done");
        assert_eq!(action, None);
    }
}
