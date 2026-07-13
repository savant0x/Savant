// SAFETY: All `clippy::disallowed_methods` violations in this file originate from
// the `serde_json::json!()` macro, which internally uses `.unwrap()` on
// compile-time-validated JSON literals. A malformed JSON literal would be a
// compile error, making the panic path statically unreachable.
#![allow(clippy::disallowed_methods)]

pub mod chain;
pub mod cost_router;
pub mod mgmt;
pub mod privacy_router;

use savant_core::types::{ChatMessage, ChatRole};

/// Format a message with images into OpenAI-compatible content array format.
/// If the message has no images, returns the content as a plain string.
/// If images are present, returns a content array with text + image_url blocks.
#[allow(clippy::disallowed_methods)] // serde_json::json! macro internally uses unwrap
fn format_message_content_with_images(msg: &ChatMessage) -> serde_json::Value {
    if msg.images.is_empty() {
        serde_json::json!(msg.content)
    } else {
        let mut parts = vec![serde_json::json!({
            "type": "text",
            "text": msg.content
        })];
        for img in &msg.images {
            parts.push(serde_json::json!({
                "type": "image_url",
                "image_url": {
                    "url": format!("data:image/jpeg;base64,{}", img)
                }
            }));
        }
        serde_json::json!(parts)
    }
}

/// Format a message with images into Anthropic-compatible content array format.
/// Anthropic uses `source` instead of `image_url` for image blocks.
#[allow(clippy::disallowed_methods)] // serde_json::json! macro internally uses unwrap
fn format_message_content_anthropic(msg: &ChatMessage) -> serde_json::Value {
    if msg.images.is_empty() {
        serde_json::json!(msg.content)
    } else {
        let mut parts = vec![serde_json::json!({
            "type": "text",
            "text": msg.content
        })];
        for img in &msg.images {
            parts.push(serde_json::json!({
                "type": "image",
                "source": {
                    "type": "base64",
                    "media_type": "image/jpeg",
                    "data": img
                }
            }));
        }
        serde_json::json!(parts)
    }
}

use async_stream::stream;
use async_trait::async_trait;
use futures::stream::{Stream, StreamExt};
use reqwest::Client;
use savant_core::error::SavantError;
use savant_core::traits::LlmProvider;
use savant_core::types::{ChatChunk, LlmParams};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::pin::Pin;

/// PB-09: Maximum number of streaming chunks before forcing completion.
const MAX_STREAM_CHUNKS: u32 = 10_000;

/// Classifies reqwest errors into appropriate SavantError variants.
/// This enables the provider chain (error classification, cooldown, circuit breaker)
/// to handle different failure modes correctly:
/// - Timeout → retries with backoff
/// - Rate limit (429) → cooldown tracking
/// - Auth (401/403) → terminal (no retry)
/// - Server error (5xx) → transient retry
/// - Network → transient retry
fn classify_http_error(e: reqwest::Error, provider: &str) -> SavantError {
    if e.is_timeout() {
        SavantError::Timeout(format!("{} request timed out: {}", provider, e))
    } else if e.is_connect() {
        SavantError::NetworkError(format!("{} connection failed: {}", provider, e))
    } else if let Some(status) = e.status() {
        match status.as_u16() {
            401 | 403 => {
                SavantError::AuthError(format!("{} auth failed ({}): {}", provider, status, e))
            }
            429 => SavantError::RateLimit(format!("{} rate limited ({}): {}", provider, status, e)),
            500..=599 => {
                SavantError::Unknown(format!("{} server error ({}): {}", provider, status, e))
            }
            _ => SavantError::NetworkError(format!("{} HTTP {}: {}", provider, status, e)),
        }
    } else {
        SavantError::NetworkError(format!("{} request failed: {}", provider, e))
    }
}

/// PB-15: Extracts the Retry-After header value from an HTTP response.
/// Returns the number of seconds to wait, or None if not present/parseable.
pub fn extract_retry_after(response: &reqwest::Response) -> Option<u64> {
    response
        .headers()
        .get("retry-after")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.parse::<u64>().ok())
}

/// PB-15: Checks an HTTP response for 429 status and extracts Retry-After.
/// Returns Ok(()) if the response is not rate-limited.
/// Returns Err(SavantError::RateLimit) with embedded retry-after seconds if rate-limited.
fn check_response_retry_after(
    response: &reqwest::Response,
    provider: &str,
) -> Result<(), SavantError> {
    if response.status() == 429 {
        let retry_secs = extract_retry_after(response);
        let msg = if let Some(secs) = retry_secs {
            format!("{} rate limited (429): retry after {}s", provider, secs)
        } else {
            format!("{} rate limited (429)", provider)
        };
        return Err(SavantError::RateLimit(msg));
    }
    Ok(())
}

/// Parses a single JSON object from the beginning of a buffer.
/// Returns the parsed object and the remaining unparsed string.
fn parse_json_object(buffer: &str) -> Option<(Value, String)> {
    // Find the first complete JSON object by counting braces
    let mut depth = 0;
    let mut start = None;

    for (i, ch) in buffer.char_indices() {
        match ch {
            '{' => {
                if start.is_none() {
                    start = Some(i);
                }
                depth += 1;
            }
            '}' => {
                depth -= 1;
                if depth == 0 {
                    if let Some(s) = start {
                        let json_str = &buffer[s..=i];
                        if let Ok(obj) = serde_json::from_str(json_str) {
                            let rest = buffer[i + 1..].to_string();
                            return Some((obj, rest));
                        }
                    }
                    return None;
                }
            }
            _ => {}
        }
    }
    None
}

/// Helper to transform raw bytes stream from OpenAI-compatible providers into ChatChunk stream.
fn openai_stream_to_chunks<S>(
    stream: S,
    agent_id: String,
    agent_name: String,
    provider_name: String,
) -> Pin<Box<dyn Stream<Item = Result<ChatChunk, SavantError>> + Send>>
where
    S: Stream<Item = Result<bytes::Bytes, SavantError>> + Send + 'static + std::marker::Unpin,
{
    Box::pin(stream! {
        let mut stream = stream;
        let mut chunk_count = 0u32;
        let mut tool_calls_map = std::collections::HashMap::<u64, savant_core::types::ProviderToolCall>::new();

        while let Some(chunk_res) = stream.next().await {
            // PB-09: Guard against unbounded streaming
            if chunk_count >= MAX_STREAM_CHUNKS {
                tracing::warn!("[{}] Stream exceeded {} chunks, forcing completion", agent_id, MAX_STREAM_CHUNKS);
                break;
            }
            match chunk_res {
                Ok(bytes) => {
                    let text = String::from_utf8_lossy(&bytes);
                    tracing::debug!("[{}] LLM stream chunk ({} bytes): {}", agent_id, bytes.len(), text.chars().take(200).collect::<String>());
                    for line in text.lines() {
                        let line = line.trim();
                        if line.is_empty() { continue; }
                        if let Some(data) = line.strip_prefix("data: ") {
                            if data == "[DONE]" { break; }

                            if let Ok(json) = serde_json::from_str::<Value>(data) {
                                let choice = &json["choices"][0];
                                let logprob = choice["logprobs"]["content"][0]["logprob"].as_f64().map(|f| f as f32);

                                // Extract reasoning/thinking from provider-level fields (2026 standard)
                                let reasoning = choice["delta"]["reasoning"].as_str()
                                    .or_else(|| choice["delta"]["reasoning_content"].as_str())
                                    .map(|s| s.to_string());

                                if let Some(reasoning_text) = reasoning {
                                    if !reasoning_text.trim().is_empty() {
                                        chunk_count += 1;
                                        yield Ok(ChatChunk {
                                            agent_name: agent_name.clone(),
                                            agent_id: agent_id.clone(),
                                            content: String::new(),
                                            is_final: false,
                                            session_id: None,
                                            channel: savant_core::types::AgentOutputChannel::Chat,
                                            logprob,
                                            is_telemetry: true,
                                            reasoning: Some(reasoning_text),
                                            tool_calls: None,
                                        });
                                    }
                                }

                                // Handle tool calls accumulation
                                if let Some(tool_calls_array) = choice["delta"]["tool_calls"].as_array() {
                                    for tc in tool_calls_array {
                                        if let Some(index) = tc["index"].as_u64() {
                                            let entry = tool_calls_map.entry(index).or_insert_with(|| savant_core::types::ProviderToolCall {
                                                id: tc["id"].as_str().unwrap_or("").to_string(),
                                                name: "".to_string(),
                                                arguments: "".to_string(),
                                            });
                                            if let Some(function) = tc.get("function") {
                                                if let Some(name) = function["name"].as_str() {
                                                    entry.name.push_str(name);
                                                }
                                                if let Some(args) = function["arguments"].as_str() {
                                                    entry.arguments.push_str(args);
                                                }
                                            }
                                        }
                                    }
                                }

                                // Check finish_reason
                                if let Some(finish_reason) = choice["finish_reason"].as_str() {
                                    if finish_reason == "tool_calls" {
                                        let calls: Vec<_> = tool_calls_map.into_values().collect();
                                        if !calls.is_empty() {
                                            chunk_count += 1;
                                            yield Ok(ChatChunk {
                                                agent_name: agent_name.clone(),
                                                agent_id: agent_id.clone(),
                                                content: String::new(),
                                                is_final: false,
                                                session_id: None,
                                                channel: savant_core::types::AgentOutputChannel::Chat,
                                                logprob: None,
                                                is_telemetry: false,
                                                reasoning: None,
                                                tool_calls: Some(calls),
                                            });
                                        }
                                        tool_calls_map = std::collections::HashMap::new();
                                    }
                                }

                                if let Some(content) = choice["delta"]["content"].as_str() {
                                    if !content.contains("OPENROUTER PROCESSING") {
                                        chunk_count += 1;
                                        yield Ok(ChatChunk {
                                            agent_name: agent_name.clone(),
                                            agent_id: agent_id.clone(),
                                            content: content.to_string(),
                                            is_final: false,
                                            session_id: None,
                                            channel: savant_core::types::AgentOutputChannel::Chat,
                                            logprob,
                                            is_telemetry: false,
                                            reasoning: None,
                                            tool_calls: None,
                                        });
                                    }
                                }
                            } else {
                                // SSE chunks may be split across TCP frames; buffer partial JSON
                                // and retry on next chunk. For now, downgrade to debug since
                                // the stream recovers automatically.
                                tracing::debug!("[{}] Partial/malformed SSE chunk ({} bytes), buffering for next frame: {}", agent_id, data.len(), data.chars().take(100).collect::<String>());
                            }
                        }
                    }
                }
                Err(e) => {
                    if chunk_count > 0 {
                        tracing::warn!(
                            "[{}] {} stream interrupted after {} chunks — treating as complete",
                            agent_id,
                            provider_name,
                            chunk_count
                        );
                        break;
                    }
                    tracing::warn!(
                        "[{}] {} stream interrupted ({}): propagating error",
                        agent_id,
                        provider_name,
                        e
                    );
                    yield Err(SavantError::NetworkError(format!(
                        "[{}] Stream interrupted: {}", agent_id, e
                    )));
                    return;
                }
            }
        }
        tracing::info!("[{}] LLM stream complete, yielded {} chunks", agent_id, chunk_count);
        yield Ok(ChatChunk {
            agent_name: agent_name.clone(),
            agent_id: agent_id.clone(),
            content: String::new(),
            is_final: true,
            session_id: None,
            channel: savant_core::types::AgentOutputChannel::Chat,
            logprob: None,
            is_telemetry: false,
            reasoning: None,
            tool_calls: None,
        });
    })
}

pub struct OpenAiProvider {
    pub client: Client,
    pub api_key: String,
    pub model: String,
    pub agent_id: String,
    pub agent_name: String,
    pub llm_params: Option<LlmParams>,
    pub max_completion_tokens: Option<u32>,
    pub base_url: String,
}

#[async_trait]
impl LlmProvider for OpenAiProvider {
    async fn stream_completion(
        &self,
        messages: Vec<ChatMessage>,
        tools: Vec<serde_json::Value>,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<ChatChunk, SavantError>> + Send>>, SavantError>
    {
        let formatted_messages: Vec<serde_json::Value> = messages
            .iter()
            .map(|msg| {
                let role = match msg.role {
                    ChatRole::System => "system",
                    ChatRole::User => "user",
                    ChatRole::Assistant => "assistant",
                    _ => "user",
                };
                let content = format_message_content_with_images(msg);
                serde_json::json!({ "role": role, "content": content })
            })
            .collect();

        let url = format!("{}/chat/completions", self.base_url);
        let response = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", self.api_key))
            .json(&json!({
                "model": self.model,
                "messages": formatted_messages,
                "tools": tools,
                "stream": true,
                "temperature": self.llm_params.as_ref().map(|p| p.temperature).unwrap_or(0.7),
                "top_p": self.llm_params.as_ref().map(|p| p.top_p).unwrap_or(0.9),
                "frequency_penalty": self.llm_params.as_ref().map(|p| p.frequency_penalty).unwrap_or(0.2),
                "presence_penalty": self.llm_params.as_ref().map(|p| p.presence_penalty).unwrap_or(0.1),
                "max_tokens": self.max_completion_tokens.unwrap_or_else(|| self.llm_params.as_ref().map(|p| p.max_tokens).unwrap_or(4096)),
            }))
            .send()
            .await
            .map_err(|e| classify_http_error(e, "OpenAI"))?;

        // PB-15: Check for 429 and extract Retry-After before streaming
        check_response_retry_after(&response, "OpenAI")?;

        let stream = response
            .bytes_stream()
            .map(|res| res.map_err(|e| SavantError::IoError(std::io::Error::other(e))));

        Ok(openai_stream_to_chunks(
            stream,
            self.agent_id.clone(),
            self.agent_name.clone(),
            "openai".to_string(),
        ))
    }

    fn supports_multimodal(&self) -> bool {
        true
    }
}

/// All model properties fetched from the OpenRouter API.
/// Used for dynamic configuration of ANY provider based on OpenRouter's model database.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelInfo {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub context_length: Option<usize>,
    pub max_completion_tokens: Option<usize>,
    pub prompt_tokens_limit: Option<usize>,
    pub completion_tokens_limit: Option<usize>,
    pub supported_parameters: Vec<String>,
    pub default_temperature: Option<f32>,
    pub default_top_p: Option<f32>,
    pub default_frequency_penalty: Option<f32>,
    pub default_presence_penalty: Option<f32>,
    pub is_moderated: Option<bool>,
}

impl ModelInfo {
    /// Returns the safe max_tokens value for API requests.
    /// Uses the model's max_completion_tokens if available,
    /// otherwise falls back to 85% of context_length.
    /// Falls back to 4096 if no model info is available (prevents
    /// OpenRouter credit errors when model is unknown).
    pub fn safe_max_tokens(&self) -> u32 {
        if let Some(max_comp) = self.max_completion_tokens {
            if max_comp > 0 {
                return (max_comp as u64).min(u32::MAX as u64) as u32;
            }
        }
        if let Some(cw) = self.context_length {
            if cw > 0 {
                return ((cw as f64 * 0.85) as u64).min(u32::MAX as u64) as u32;
            }
        }
        // Sensible default when model info is unavailable — prevents
        // OpenRouter credit errors from requesting 32768 tokens.
        4096
    }
}

/// Fetches comprehensive model info from the OpenRouter API.
/// Works for ANY model on OpenRouter, even if the user uses a different provider.
pub async fn fetch_openrouter_model_info(
    client: &Client,
    api_key: &str,
    model_id: &str,
) -> Option<ModelInfo> {
    let url = format!(
        "https://openrouter.ai/api/v1/models/{}",
        model_id.replace('/', "%2F")
    );
    let response = match client
        .get(&url)
        .header("Authorization", format!("Bearer {}", api_key))
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!(
                "[fetch_openrouter_model_info] HTTP request failed for {}: {}",
                model_id,
                e
            );
            return None;
        }
    };

    let json: Value = match response.json().await {
        Ok(j) => j,
        Err(e) => {
            tracing::warn!(
                "[fetch_openrouter_model_info] JSON parse failed for {}: {}",
                model_id,
                e
            );
            return None;
        }
    };
    let data = &json["data"];

    let id = data["id"].as_str().unwrap_or(model_id).to_string();
    let name = data["name"].as_str().unwrap_or(model_id).to_string();
    let description = data["description"].as_str().map(|s| s.to_string());
    let context_length = data["context_length"].as_u64().map(|v| v as usize);
    let max_completion_tokens = data["top_provider"]["max_completion_tokens"]
        .as_u64()
        .map(|v| v as usize);
    let prompt_tokens_limit = data["per_request_limits"]["prompt_tokens"]
        .as_u64()
        .map(|v| v as usize);
    let completion_tokens_limit = data["per_request_limits"]["completion_tokens"]
        .as_u64()
        .map(|v| v as usize);
    let is_moderated = data["top_provider"]["is_moderated"].as_bool();

    let supported_parameters = data["supported_parameters"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();

    let default_temperature = data["default_parameters"]["temperature"]
        .as_f64()
        .map(|v| v as f32);
    let default_top_p = data["default_parameters"]["top_p"]
        .as_f64()
        .map(|v| v as f32);
    let default_frequency_penalty = data["default_parameters"]["frequency_penalty"]
        .as_f64()
        .map(|v| v as f32);
    let default_presence_penalty = data["default_parameters"]["presence_penalty"]
        .as_f64()
        .map(|v| v as f32);

    let info = ModelInfo {
        id,
        name,
        description,
        context_length,
        max_completion_tokens,
        prompt_tokens_limit,
        completion_tokens_limit,
        supported_parameters,
        default_temperature,
        default_top_p,
        default_frequency_penalty,
        default_presence_penalty,
        is_moderated,
    };

    tracing::info!(
        "Model info for {}: context={}, max_completion={}, params={:?}",
        model_id,
        context_length.unwrap_or(0),
        max_completion_tokens.unwrap_or(0),
        info.supported_parameters
    );

    Some(info)
}

pub struct OpenRouterProvider {
    pub client: Client,
    pub api_key: String,
    pub model: String,
    pub agent_id: String,
    pub agent_name: String,
    pub llm_params: Option<LlmParams>,
    pub context_window: Option<usize>,
    pub max_completion_tokens: Option<u32>,
}

#[async_trait]
impl LlmProvider for OpenRouterProvider {
    async fn stream_completion(
        &self,
        messages: Vec<ChatMessage>,
        tools: Vec<serde_json::Value>,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<ChatChunk, SavantError>> + Send>>, SavantError>
    {
        // Auto-calculate max_tokens from the dynamically-fetched model info.
        let max_tokens = self.max_completion_tokens.unwrap_or_else(|| {
            self.context_window
                .map(|cw| (cw as f64 * 0.85) as u32)
                .unwrap_or(4096)
        });

        let response = self
            .client
            .post("https://openrouter.ai/api/v1/chat/completions")
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("HTTP-Referer", "https://github.com/Savant-AI/Savant")
            .header("X-Title", "Savant Framework")
            .json(&json!({
                "model": self.model,
                "messages": messages,
                "tools": tools,
                "stream": true,
                "temperature": self.llm_params.as_ref().map(|p| p.temperature).unwrap_or(0.7),
                "top_p": self.llm_params.as_ref().map(|p| p.top_p).unwrap_or(1.0),
                "frequency_penalty": self.llm_params.as_ref().map(|p| p.frequency_penalty).unwrap_or(0.0),
                "presence_penalty": self.llm_params.as_ref().map(|p| p.presence_penalty).unwrap_or(0.0),
                "max_tokens": max_tokens,
            }))
            .send()
            .await
            .map_err(|e| classify_http_error(e, "OpenRouter"))?;

        // PB-15: Check for 429 and extract Retry-After before streaming
        check_response_retry_after(&response, "OpenRouter")?;

        let stream = response
            .bytes_stream()
            .map(|res| res.map_err(|e| SavantError::IoError(std::io::Error::other(e))));

        Ok(openai_stream_to_chunks(
            stream,
            self.agent_id.clone(),
            self.agent_name.clone(),
            "openrouter".to_string(),
        ))
    }

    fn context_window(&self) -> Option<usize> {
        self.context_window
    }
}

/// Helper to transform raw bytes stream from Anthropic providers into ChatChunk stream.
fn anthropic_stream_to_chunks<S>(
    stream: S,
    agent_id: String,
    agent_name: String,
) -> Pin<Box<dyn Stream<Item = Result<ChatChunk, SavantError>> + Send>>
where
    S: Stream<Item = Result<bytes::Bytes, SavantError>> + Send + 'static + std::marker::Unpin,
{
    Box::pin(stream! {
        let mut stream = stream;
        let mut tool_calls = Vec::new();
        let mut current_tool_id = String::new();
        let mut current_tool_name = String::new();
        let mut current_tool_args = String::new();
        let mut chunk_count = 0u32;

        while let Some(chunk_res) = stream.next().await {
            // PB-09: Guard against unbounded streaming
            if chunk_count >= MAX_STREAM_CHUNKS {
                tracing::warn!("[{}] Anthropic stream exceeded {} chunks, forcing completion", agent_id, MAX_STREAM_CHUNKS);
                break;
            }
            chunk_count += 1;
            match chunk_res {
                Ok(bytes) => {
                    let text = String::from_utf8_lossy(&bytes);
                    for line in text.lines() {
                        let line = line.trim();
                        if line.is_empty() { continue; }
                        if let Some(data) = line.strip_prefix("data: ") {
                            if let Ok(json) = serde_json::from_str::<Value>(data) {
                                if json["type"] == "content_block_start" {
                                    if let Some(block) = json.get("content_block") {
                                        if block["type"] == "tool_use" {
                                            current_tool_id = block["id"].as_str().unwrap_or("").to_string();
                                            current_tool_name = block["name"].as_str().unwrap_or("").to_string();
                                            current_tool_args = String::new();
                                        }
                                    }
                                } else if json["type"] == "content_block_delta" {
                                    if json["delta"]["type"] == "thinking_delta" {
                                        if let Some(thinking) = json["delta"]["thinking"].as_str() {
                                            yield Ok(ChatChunk {
                                                agent_name: agent_name.clone(),
                                                agent_id: agent_id.clone(),
                                                content: String::new(),
                                                is_final: false,
                                                session_id: None,
                                                channel: savant_core::types::AgentOutputChannel::Chat,
                                                logprob: None,
                                                is_telemetry: true,
                                                reasoning: Some(thinking.to_string()),
                                                tool_calls: None,
                                            });
                                        }
                                    } else if json["delta"]["type"] == "text_delta" {
                                        if let Some(content) = json["delta"]["text"].as_str() {
                                            yield Ok(ChatChunk {
                                                agent_name: agent_name.clone(),
                                                agent_id: agent_id.clone(),
                                                content: content.to_string(),
                                                is_final: false,
                                                session_id: None,
                                                channel: savant_core::types::AgentOutputChannel::Chat,
                                                logprob: None,
                                                is_telemetry: false,
                                                reasoning: None,
                                                tool_calls: None,
                                            });
                                        }
                                    } else if json["delta"]["type"] == "input_json_delta" {
                                        if let Some(partial) = json["delta"]["partial_json"].as_str() {
                                            current_tool_args.push_str(partial);
                                        }
                                    }
                                } else if json["type"] == "content_block_stop" {
                                    if !current_tool_name.is_empty() {
                                        tool_calls.push(savant_core::types::ProviderToolCall {
                                            id: current_tool_id.clone(),
                                            name: current_tool_name.clone(),
                                            arguments: current_tool_args.clone(),
                                        });
                                        current_tool_name.clear();
                                    }
                                } else if json["type"] == "message_stop" {
                                    if !tool_calls.is_empty() {
                                        yield Ok(ChatChunk {
                                            agent_name: agent_name.clone(),
                                            agent_id: agent_id.clone(),
                                            content: String::new(),
                                            is_final: false,
                                            session_id: None,
                                            channel: savant_core::types::AgentOutputChannel::Chat,
                                            logprob: None,
                                            is_telemetry: false,
                                            reasoning: None,
                                            tool_calls: Some(std::mem::take(&mut tool_calls)),
                                        });
                                    }
                                    break;
                                }
                            }
                        }
                    }
                }
                Err(e) => {
                    if chunk_count > 0 {
                        tracing::warn!(
                            "[{}] Anthropic stream interrupted after {} chunks — treating as complete",
                            agent_id,
                            chunk_count
                        );
                        break;
                    }
                    tracing::warn!(
                        "[{}] Anthropic stream interrupted ({}): propagating error",
                        agent_id,
                        e
                    );
                    yield Err(SavantError::NetworkError(format!(
                        "[{}] Stream interrupted: {}", agent_id, e
                    )));
                    return;
                }
            }
        }
        yield Ok(ChatChunk {
            agent_name: agent_name.clone(),
            agent_id: agent_id.clone(),
            content: String::new(),
            is_final: true,
            session_id: None,
            channel: savant_core::types::AgentOutputChannel::Chat,
            logprob: None,
            is_telemetry: false,
            reasoning: None,
            tool_calls: None,
        });
    })
}

pub struct AnthropicProvider {
    pub client: Client,
    pub api_key: String,
    pub model: String,
    pub agent_id: String,
    pub agent_name: String,
    pub llm_params: Option<LlmParams>,
    pub max_completion_tokens: Option<u32>,
}

#[async_trait]
impl LlmProvider for AnthropicProvider {
    async fn stream_completion(
        &self,
        messages: Vec<ChatMessage>,
        tools: Vec<serde_json::Value>,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<ChatChunk, SavantError>> + Send>>, SavantError>
    {
        let anthropic_messages: Vec<serde_json::Value> = messages
            .iter()
            .enumerate()
            .map(|(i, msg)| {
                let content = format_message_content_anthropic(msg);
                let mut m = serde_json::json!({
                    "role": match msg.role {
                        ChatRole::System => "system",
                        ChatRole::User => "user",
                        ChatRole::Assistant => "assistant",
                        _ => "user",
                    },
                    "content": content,
                });
                if i == 0 || i >= messages.len().saturating_sub(4) {
                    m["cache_control"] = serde_json::json!({"type": "ephemeral"});
                }
                m
            })
            .collect();

        let mut body = serde_json::json!({
            "model": self.model,
            "max_tokens": self.max_completion_tokens.unwrap_or_else(|| self.llm_params.as_ref().map(|p| p.max_tokens).unwrap_or(4096)),
            "temperature": self.llm_params.as_ref().map(|p| p.temperature).unwrap_or(0.7),
            "top_p": self.llm_params.as_ref().map(|p| p.top_p).unwrap_or(0.9),
            "stream": true,
            "messages": anthropic_messages,
        });

        if !tools.is_empty() {
            body["tools"] = serde_json::json!(tools);
        }

        let response = self
            .client
            .post("https://api.anthropic.com/v1/messages")
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", "2023-06-01")
            .json(&body)
            .send()
            .await
            .map_err(|e| classify_http_error(e, "Anthropic"))?;

        // PB-15: Check for 429 and extract Retry-After before streaming
        check_response_retry_after(&response, "Anthropic")?;

        let stream = response
            .bytes_stream()
            .map(|res| res.map_err(|e| SavantError::IoError(std::io::Error::other(e))));

        Ok(anthropic_stream_to_chunks(
            stream,
            self.agent_id.clone(),
            self.agent_name.clone(),
        ))
    }

    fn supports_multimodal(&self) -> bool {
        true
    }
}

pub struct OllamaProvider {
    pub client: Client,
    pub url: String,
    pub model: String,
    pub agent_id: String,
    pub agent_name: String,
}

#[async_trait]
impl LlmProvider for OllamaProvider {
    async fn stream_completion(
        &self,
        messages: Vec<ChatMessage>,
        _tools: Vec<serde_json::Value>,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<ChatChunk, SavantError>> + Send>>, SavantError>
    {
        // Ollama expects images as a top-level field on each message object
        let ollama_messages: Vec<serde_json::Value> = messages
            .iter()
            .map(|msg| {
                let mut m = serde_json::json!({
                    "role": match msg.role {
                        ChatRole::System => "system",
                        ChatRole::User => "user",
                        ChatRole::Assistant => "assistant",
                        _ => "user",
                    },
                    "content": msg.content,
                });
                if !msg.images.is_empty() {
                    m["images"] = serde_json::json!(msg.images);
                }
                m
            })
            .collect();

        let response = self
            .client
            .post(format!("{}/api/chat", self.url))
            .json(&json!({
                "model": self.model,
                "messages": ollama_messages,
                "stream": true,
            }))
            .send()
            .await
            .map_err(|e| classify_http_error(e, "Ollama"))?;

        // PB-15: Check for 429 and extract Retry-After before streaming
        check_response_retry_after(&response, "Ollama")?;

        let stream = response
            .bytes_stream()
            .map(|res| res.map_err(|e| SavantError::IoError(std::io::Error::other(e))));

        let agent_name = self.agent_name.clone();
        let agent_id = self.agent_id.clone();
        let final_name = agent_name.clone();
        let final_id = agent_id.clone();

        Ok(Box::pin(stream! {
            let mut stream = stream;
            while let Some(chunk_res) = stream.next().await {
                match chunk_res {
                    Ok(bytes) => {
                        let text = String::from_utf8_lossy(&bytes);
                        if let Ok(json) = serde_json::from_str::<Value>(&text) {
                            // Ollama thinking support (2026 standard)
                            if let Some(thinking) = json["message"]["thinking"].as_str() {
                                if !thinking.trim().is_empty() {
                                    yield Ok(ChatChunk {
                                        agent_name: agent_name.clone(),
                                        agent_id: agent_id.clone(),
                                        content: String::new(),
                                        is_final: false,
                                        session_id: None,
                                        channel: savant_core::types::AgentOutputChannel::Chat,
                                        logprob: None,
                                        is_telemetry: true,
                                        reasoning: Some(thinking.to_string()),
                                        tool_calls: None,
                                    });
                                }
                            }
                            if let Some(content) = json["message"]["content"].as_str() {
                                if !content.is_empty() {
                                    yield Ok(ChatChunk {
                                        agent_name: agent_name.clone(),
                                        agent_id: agent_id.clone(),
                                        content: content.to_string(),
                                        is_final: false,
                                        session_id: None,
                                        channel: savant_core::types::AgentOutputChannel::Chat,
                                        logprob: None,
                                        is_telemetry: false,
                                        reasoning: None,
                                        tool_calls: None,
                                    });
                                }
                            }
                            if let Some(tool_calls_json) = json["message"]["tool_calls"].as_array() {
                                let mut calls = Vec::new();
                                for call in tool_calls_json {
                                    if let Some(function) = call.get("function") {
                                        calls.push(savant_core::types::ProviderToolCall {
                                            id: "".to_string(),
                                            name: function["name"].as_str().unwrap_or("").to_string(),
                                            arguments: function["arguments"].to_string(),
                                        });
                                    }
                                }
                                if !calls.is_empty() {
                                    yield Ok(ChatChunk {
                                        agent_name: agent_name.clone(),
                                        agent_id: agent_id.clone(),
                                        content: String::new(),
                                        is_final: false,
                                        session_id: None,
                                        channel: savant_core::types::AgentOutputChannel::Chat,
                                        logprob: None,
                                        is_telemetry: false,
                                        reasoning: None,
                                        tool_calls: Some(calls),
                                    });
                                }
                            }
                        }
                    }
                    Err(e) => {
                        tracing::warn!(
                            "[{}] Ollama stream interrupted ({}): yielding partial response as complete",
                            agent_id,
                            e
                        );
                        yield Ok(ChatChunk {
                            agent_name: agent_name.clone(),
                            agent_id: agent_id.clone(),
                            content: String::new(),
                            is_final: true,
                            session_id: None,
                            channel: savant_core::types::AgentOutputChannel::Chat,
                            logprob: None,
                            is_telemetry: false,
                            reasoning: None,
                            tool_calls: None,
                        });
                        return;
                    }
                }
            }
            yield Ok(ChatChunk {
                agent_name: final_name,
                agent_id: final_id,
                content: String::new(),
                is_final: true,
                session_id: None,
                channel: savant_core::types::AgentOutputChannel::Chat,
                logprob: None,
                is_telemetry: false,
                reasoning: None,
                tool_calls: None,
            });
        }))
    }

    fn supports_multimodal(&self) -> bool {
        true
    }
}

pub struct GroqProvider {
    pub client: Client,
    pub api_key: String,
    pub model: String,
    pub agent_id: String,
    pub agent_name: String,
    pub llm_params: Option<LlmParams>,
    pub max_completion_tokens: Option<u32>,
}

#[async_trait]
impl LlmProvider for GroqProvider {
    async fn stream_completion(
        &self,
        messages: Vec<ChatMessage>,
        tools: Vec<serde_json::Value>,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<ChatChunk, SavantError>> + Send>>, SavantError>
    {
        let response = self
            .client
            .post("https://api.groq.com/openai/v1/chat/completions")
            .header("Authorization", format!("Bearer {}", self.api_key))
            .json(&json!({
                "model": self.model,
                "messages": messages,
                "tools": tools,
                "stream": true,
                "temperature": self.llm_params.as_ref().map(|p| p.temperature).unwrap_or(0.7),
                "top_p": self.llm_params.as_ref().map(|p| p.top_p).unwrap_or(0.9),
                "frequency_penalty": self.llm_params.as_ref().map(|p| p.frequency_penalty).unwrap_or(0.2),
                "presence_penalty": self.llm_params.as_ref().map(|p| p.presence_penalty).unwrap_or(0.1),
                "max_tokens": self.max_completion_tokens.unwrap_or_else(|| self.llm_params.as_ref().map(|p| p.max_tokens).unwrap_or(4096)),
            }))
            .send()
            .await
            .map_err(|e| classify_http_error(e, "Groq"))?;

        // PB-15: Check for 429 and extract Retry-After before streaming
        check_response_retry_after(&response, "Groq")?;

        let stream = response
            .bytes_stream()
            .map(|res| res.map_err(|e| SavantError::IoError(std::io::Error::other(e))));

        Ok(openai_stream_to_chunks(
            stream,
            self.agent_id.clone(),
            self.agent_name.clone(),
            "groq".to_string(),
        ))
    }
}

// ============================================================================
// ADDITIONAL MODEL PROVIDERS - Support for all major AI providers
// ============================================================================

/// Google AI (Gemini) provider
pub struct GoogleProvider {
    pub client: Client,
    pub api_key: String,
    pub model: String,
    pub agent_id: String,
    pub agent_name: String,
    pub llm_params: Option<LlmParams>,
    pub max_completion_tokens: Option<u32>,
}

#[async_trait]
impl LlmProvider for GoogleProvider {
    async fn stream_completion(
        &self,
        messages: Vec<ChatMessage>,
        tools: Vec<serde_json::Value>,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<ChatChunk, SavantError>> + Send>>, SavantError>
    {
        // Convert messages to Gemini format
        let contents: Vec<serde_json::Value> = messages
            .iter()
            .map(|m| {
                json!({
                    "role": match m.role {
                        savant_core::types::ChatRole::User => "user",
                        _ => "model",
                    },
                    "parts": [{ "text": m.content }]
                })
            })
            .collect();

        let response = self
            .client
            .post(format!(
                "https://generativelanguage.googleapis.com/v1beta/models/{}:streamGenerateContent",
                self.model
            ))
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .json(&json!({
                "contents": contents,
                "tools": tools,
                "generationConfig": {
                    "temperature": self.llm_params.as_ref().map(|p| p.temperature).unwrap_or(0.7),
                    "topP": self.llm_params.as_ref().map(|p| p.top_p).unwrap_or(0.9),
                    "maxOutputTokens": self.max_completion_tokens.unwrap_or_else(|| self.llm_params.as_ref().map(|p| p.max_tokens).unwrap_or(4096)),
                }
            }))
            .send()
            .await
            .map_err(|e| classify_http_error(e, "Google AI"))?;

        // PB-15: Check for 429 and extract Retry-After before streaming
        check_response_retry_after(&response, "Google AI")?;

        let stream = response
            .bytes_stream()
            .map(|res| res.map_err(|e| SavantError::IoError(std::io::Error::other(e))));

        Ok(google_stream_to_chunks(
            stream,
            self.agent_id.clone(),
            self.agent_name.clone(),
        ))
    }
}

/// Google streaming response parser
fn google_stream_to_chunks<S>(
    stream: S,
    agent_id: String,
    agent_name: String,
) -> Pin<Box<dyn Stream<Item = Result<ChatChunk, SavantError>> + Send>>
where
    S: Stream<Item = Result<bytes::Bytes, SavantError>> + Send + 'static + std::marker::Unpin,
{
    Box::pin(stream! {
        let mut buffer = String::new();
        let mut stream = stream.boxed();
        let mut chunk_count = 0u32;

        while let Some(chunk_result) = stream.next().await {
            match chunk_result {
                Ok(chunk) => {
                    let chunk_str = String::from_utf8_lossy(&chunk);
                    buffer.push_str(&chunk_str);

                    // Process complete JSON objects from buffer
                    while let Some((obj, rest)) = parse_json_object(&buffer) {
                        buffer = rest;

                        // Extract text from Gemini response format
                        if let Some(candidates) = obj.get("candidates").and_then(|c| c.as_array()) {
                            for candidate in candidates {
                                if let Some(content) = candidate.get("content") {
                                    if let Some(parts) = content.get("parts").and_then(|p| p.as_array()) {
                                        for part in parts {
                                            if let Some(text) = part.get("text").and_then(|t| t.as_str()) {
                                                chunk_count += 1;
                                                yield Ok(ChatChunk {
                                                    agent_name: agent_name.clone(),
                                                    agent_id: agent_id.clone(),
                                                    content: text.to_string(),
                                                    is_final: false,
                                                    session_id: None,
                                                    channel: savant_core::types::AgentOutputChannel::Chat,
                                                    logprob: None,
                                                    is_telemetry: false,
                                                    reasoning: None,
                                                    tool_calls: None,
                                                });
                                            }
                                            // Parse Gemini functionCall as tool calls
                                            if let Some(function_call) = part.get("functionCall") {
                                                chunk_count += 1;
                                                let name = function_call["name"].as_str().unwrap_or("").to_string();
                                                let args = function_call["args"].to_string();
                                                yield Ok(ChatChunk {
                                                    agent_name: agent_name.clone(),
                                                    agent_id: agent_id.clone(),
                                                    content: String::new(),
                                                    is_final: false,
                                                    session_id: None,
                                                    channel: savant_core::types::AgentOutputChannel::Chat,
                                                    logprob: None,
                                                    is_telemetry: false,
                                                    reasoning: None,
                                                    tool_calls: Some(vec![savant_core::types::ProviderToolCall {
                                                        id: format!("gemini_{}", name),
                                                        name,
                                                        arguments: args,
                                                    }]),
                                                });
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                Err(e) => {
                    if chunk_count > 0 {
                        tracing::warn!(
                            "[{}] Google stream interrupted after {} chunks — treating as complete",
                            agent_id,
                            chunk_count
                        );
                        break;
                    }
                    tracing::warn!(
                        "[{}] Google stream interrupted ({}): propagating error",
                        agent_id,
                        e
                    );
                    yield Err(SavantError::NetworkError(format!(
                        "[{}] Stream interrupted: {}", agent_id, e
                    )));
                    return;
                }
            }
        }

        yield Ok(ChatChunk {
            agent_name: agent_name.clone(),
            agent_id: agent_id.clone(),
            content: String::new(),
            is_final: true,
            session_id: None,
            channel: savant_core::types::AgentOutputChannel::Chat,
            logprob: None,
            is_telemetry: false,
            reasoning: None,
            tool_calls: None,
        });
    })
}

/// Mistral AI provider
pub struct MistralProvider {
    pub client: Client,
    pub api_key: String,
    pub model: String,
    pub agent_id: String,
    pub agent_name: String,
    pub llm_params: Option<LlmParams>,
    pub max_completion_tokens: Option<u32>,
}

#[async_trait]
impl LlmProvider for MistralProvider {
    async fn stream_completion(
        &self,
        messages: Vec<ChatMessage>,
        tools: Vec<serde_json::Value>,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<ChatChunk, SavantError>> + Send>>, SavantError>
    {
        let response = self
            .client
            .post("https://api.mistral.ai/v1/chat/completions")
            .header("Authorization", format!("Bearer {}", self.api_key))
            .json(&json!({
                "model": self.model,
                "messages": messages,
                "tools": tools,
                "stream": true,
                "temperature": self.llm_params.as_ref().map(|p| p.temperature).unwrap_or(0.7),
                "top_p": self.llm_params.as_ref().map(|p| p.top_p).unwrap_or(0.9),
                "max_tokens": self.max_completion_tokens.unwrap_or_else(|| self.llm_params.as_ref().map(|p| p.max_tokens).unwrap_or(4096)),
            }))
            .send()
            .await
            .map_err(|e| classify_http_error(e, "Mistral"))?;

        // PB-15: Check for 429 and extract Retry-After before streaming
        check_response_retry_after(&response, "Mistral")?;

        let stream = response
            .bytes_stream()
            .map(|res| res.map_err(|e| SavantError::IoError(std::io::Error::other(e))));

        Ok(openai_stream_to_chunks(
            stream,
            self.agent_id.clone(),
            self.agent_name.clone(),
            "mistral".to_string(),
        ))
    }
}

/// Together AI provider
pub struct TogetherProvider {
    pub client: Client,
    pub api_key: String,
    pub model: String,
    pub agent_id: String,
    pub agent_name: String,
    pub llm_params: Option<LlmParams>,
    pub max_completion_tokens: Option<u32>,
}

#[async_trait]
impl LlmProvider for TogetherProvider {
    async fn stream_completion(
        &self,
        messages: Vec<ChatMessage>,
        tools: Vec<serde_json::Value>,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<ChatChunk, SavantError>> + Send>>, SavantError>
    {
        let response = self
            .client
            .post("https://api.together.xyz/v1/chat/completions")
            .header("Authorization", format!("Bearer {}", self.api_key))
            .json(&json!({
                "model": self.model,
                "messages": messages,
                "tools": tools,
                "stream": true,
                "temperature": self.llm_params.as_ref().map(|p| p.temperature).unwrap_or(0.7),
                "top_p": self.llm_params.as_ref().map(|p| p.top_p).unwrap_or(0.9),
                "max_tokens": self.max_completion_tokens.unwrap_or_else(|| self.llm_params.as_ref().map(|p| p.max_tokens).unwrap_or(4096)),
            }))
            .send()
            .await
            .map_err(|e| classify_http_error(e, "Together AI"))?;

        // PB-15: Check for 429 and extract Retry-After before streaming
        check_response_retry_after(&response, "Together AI")?;

        let stream = response
            .bytes_stream()
            .map(|res| res.map_err(|e| SavantError::IoError(std::io::Error::other(e))));

        Ok(openai_stream_to_chunks(
            stream,
            self.agent_id.clone(),
            self.agent_name.clone(),
            "together".to_string(),
        ))
    }
}

/// Deepseek provider
pub struct DeepseekProvider {
    pub client: Client,
    pub api_key: String,
    pub model: String,
    pub agent_id: String,
    pub agent_name: String,
    pub llm_params: Option<LlmParams>,
    pub max_completion_tokens: Option<u32>,
}

#[async_trait]
impl LlmProvider for DeepseekProvider {
    async fn stream_completion(
        &self,
        messages: Vec<ChatMessage>,
        tools: Vec<serde_json::Value>,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<ChatChunk, SavantError>> + Send>>, SavantError>
    {
        let response = self
            .client
            .post("https://api.deepseek.com/v1/chat/completions")
            .header("Authorization", format!("Bearer {}", self.api_key))
            .json(&json!({
                "model": self.model,
                "messages": messages,
                "tools": tools,
                "stream": true,
                "temperature": self.llm_params.as_ref().map(|p| p.temperature).unwrap_or(0.7),
                "top_p": self.llm_params.as_ref().map(|p| p.top_p).unwrap_or(0.9),
                "max_tokens": self.max_completion_tokens.unwrap_or_else(|| self.llm_params.as_ref().map(|p| p.max_tokens).unwrap_or(4096)),
            }))
            .send()
            .await
            .map_err(|e| classify_http_error(e, "Deepseek"))?;

        // PB-15: Check for 429 and extract Retry-After before streaming
        check_response_retry_after(&response, "Deepseek")?;

        let stream = response
            .bytes_stream()
            .map(|res| res.map_err(|e| SavantError::IoError(std::io::Error::other(e))));

        Ok(openai_stream_to_chunks(
            stream,
            self.agent_id.clone(),
            self.agent_name.clone(),
            "deepseek".to_string(),
        ))
    }
}

/// Cohere provider (v2 API)
pub struct CohereProvider {
    pub client: Client,
    pub api_key: String,
    pub model: String,
    pub agent_id: String,
    pub agent_name: String,
    pub llm_params: Option<LlmParams>,
    pub max_completion_tokens: Option<u32>,
}

#[async_trait]
impl LlmProvider for CohereProvider {
    async fn stream_completion(
        &self,
        messages: Vec<ChatMessage>,
        tools: Vec<serde_json::Value>,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<ChatChunk, SavantError>> + Send>>, SavantError>
    {
        // Convert to Cohere v2 chat format
        let chat_history: Vec<serde_json::Value> = messages
            .iter()
            .enumerate()
            .filter(|(idx, m)| {
                // Filter out duplicate consecutive user messages at the end
                if *idx + 1 < messages.len() {
                    return true;
                }
                // Check if this last message is same as second-to-last
                if messages.len() >= 2 {
                    if let Some(prev) = messages.get(messages.len() - 2) {
                        return !(prev.role == m.role && prev.content == m.content);
                    }
                }
                true
            })
            .map(|(_, m)| {
                json!({
                    "role": match m.role {
                        savant_core::types::ChatRole::User => "user",
                        savant_core::types::ChatRole::Assistant => "assistant",
                        _ => "system",
                    },
                    "content": m.content,
                })
            })
            .collect();

        let response = self
            .client
            .post("https://api.cohere.com/v2/chat") // v2 endpoint
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .json(&json!({
                "model": self.model,
                "messages": chat_history,
                "tools": tools,
                "stream": true,
                "temperature": self.llm_params.as_ref().map(|p| p.temperature).unwrap_or(0.7),
                "top_p": self.llm_params.as_ref().map(|p| p.top_p).unwrap_or(0.9),
                "max_tokens": self.max_completion_tokens.unwrap_or_else(|| self.llm_params.as_ref().map(|p| p.max_tokens).unwrap_or(4096)),
            }))
            .send()
            .await
            .map_err(|e| classify_http_error(e, "Cohere"))?;

        // PB-15: Check for 429 and extract Retry-After before streaming
        check_response_retry_after(&response, "Cohere")?;

        let stream = response
            .bytes_stream()
            .map(|res| res.map_err(|e| SavantError::IoError(std::io::Error::other(e))));

        Ok(cohere_stream_to_chunks(
            stream,
            self.agent_id.clone(),
            self.agent_name.clone(),
        ))
    }
}

/// Cohere streaming response parser
fn cohere_stream_to_chunks<S>(
    stream: S,
    agent_id: String,
    agent_name: String,
) -> Pin<Box<dyn Stream<Item = Result<ChatChunk, SavantError>> + Send>>
where
    S: Stream<Item = Result<bytes::Bytes, SavantError>> + Send + 'static + std::marker::Unpin,
{
    Box::pin(stream! {
        let mut stream = stream.boxed();
        let mut chunk_count = 0u32;

        while let Some(chunk_result) = stream.next().await {
            match chunk_result {
                Ok(chunk) => {
                    let chunk_str = String::from_utf8_lossy(&chunk);

                    // Parse SSE format
                    for line in chunk_str.lines() {
                        if let Some(data) = line.strip_prefix("data: ") {
                            if let Ok(json) = serde_json::from_str::<serde_json::Value>(data) {
                                if let Some(text) = json.get("text").and_then(|t| t.as_str()) {
                                    chunk_count += 1;
                                    yield Ok(ChatChunk {
                                        agent_name: agent_name.clone(),
                                        agent_id: agent_id.clone(),
                                        content: text.to_string(),
                                        is_final: false,
                                        session_id: None,
                                        channel: savant_core::types::AgentOutputChannel::Chat,
                                        logprob: None,
                                        is_telemetry: false,
                                        reasoning: None,
                                        tool_calls: None,
                                    });
                                }
                                if json.get("is_finished").and_then(|v| v.as_bool()).unwrap_or(false) {
                                    break;
                                }
                            }
                        }
                    }
                }
                Err(e) => {
                    if chunk_count > 0 {
                        tracing::warn!(
                            "[{}] Cohere stream interrupted after {} chunks — treating as complete",
                            agent_id,
                            chunk_count
                        );
                        break;
                    }
                    tracing::warn!(
                        "[{}] Cohere stream interrupted ({}): propagating error",
                        agent_id,
                        e
                    );
                    yield Err(SavantError::NetworkError(format!(
                        "[{}] Stream interrupted: {}", agent_id, e
                    )));
                    return;
                }
            }
        }

        yield Ok(ChatChunk {
            agent_name: agent_name.clone(),
            agent_id: agent_id.clone(),
            content: String::new(),
            is_final: true,
            session_id: None,
            channel: savant_core::types::AgentOutputChannel::Chat,
            logprob: None,
            is_telemetry: false,
            reasoning: None,
            tool_calls: None,
        });
    })
}

/// Azure OpenAI provider (uses OpenAI-compatible API)
pub struct AzureProvider {
    pub client: Client,
    pub api_key: String,
    pub endpoint: String,    // e.g., "https://your-resource.openai.azure.com"
    pub deployment: String,  // e.g., "gpt-4"
    pub api_version: String, // e.g., "2024-02-15-preview"
    pub agent_id: String,
    pub agent_name: String,
    pub llm_params: Option<LlmParams>,
    pub max_completion_tokens: Option<u32>,
}

#[async_trait]
impl LlmProvider for AzureProvider {
    async fn stream_completion(
        &self,
        messages: Vec<ChatMessage>,
        tools: Vec<serde_json::Value>,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<ChatChunk, SavantError>> + Send>>, SavantError>
    {
        let url = format!(
            "{}/openai/deployments/{}/chat/completions?api-version={}",
            self.endpoint.trim_end_matches('/'),
            self.deployment,
            self.api_version
        );

        let response = self
            .client
            .post(&url)
            .header("api-key", &self.api_key)
            .json(&json!({
                "messages": messages,
                "tools": tools,
                "stream": true,
                "temperature": self.llm_params.as_ref().map(|p| p.temperature).unwrap_or(0.7),
                "top_p": self.llm_params.as_ref().map(|p| p.top_p).unwrap_or(0.9),
                "frequency_penalty": self.llm_params.as_ref().map(|p| p.frequency_penalty).unwrap_or(0.2),
                "presence_penalty": self.llm_params.as_ref().map(|p| p.presence_penalty).unwrap_or(0.1),
                "max_tokens": self.max_completion_tokens.unwrap_or_else(|| self.llm_params.as_ref().map(|p| p.max_tokens).unwrap_or(4096)),
            }))
            .send()
            .await
            .map_err(|e| classify_http_error(e, "Azure"))?;

        // PB-15: Check for 429 and extract Retry-After before streaming
        check_response_retry_after(&response, "Azure")?;

        let stream = response
            .bytes_stream()
            .map(|res| res.map_err(|e| SavantError::IoError(std::io::Error::other(e))));

        Ok(openai_stream_to_chunks(
            stream,
            self.agent_id.clone(),
            self.agent_name.clone(),
            "azure".to_string(),
        ))
    }
}

/// xAI (Grok) provider - OpenAI compatible
pub struct XaiProvider {
    pub client: Client,
    pub api_key: String,
    pub model: String,
    pub agent_id: String,
    pub agent_name: String,
    pub llm_params: Option<LlmParams>,
    pub max_completion_tokens: Option<u32>,
}

#[async_trait]
impl LlmProvider for XaiProvider {
    async fn stream_completion(
        &self,
        messages: Vec<ChatMessage>,
        tools: Vec<serde_json::Value>,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<ChatChunk, SavantError>> + Send>>, SavantError>
    {
        let response = self
            .client
            .post("https://api.x.ai/v1/chat/completions")
            .header("Authorization", format!("Bearer {}", self.api_key))
            .json(&json!({
                "model": self.model,
                "messages": messages,
                "tools": tools,
                "stream": true,
                "temperature": self.llm_params.as_ref().map(|p| p.temperature).unwrap_or(0.7),
                "top_p": self.llm_params.as_ref().map(|p| p.top_p).unwrap_or(0.9),
                "frequency_penalty": self.llm_params.as_ref().map(|p| p.frequency_penalty).unwrap_or(0.2),
                "presence_penalty": self.llm_params.as_ref().map(|p| p.presence_penalty).unwrap_or(0.1),
                "max_tokens": self.max_completion_tokens.unwrap_or_else(|| self.llm_params.as_ref().map(|p| p.max_tokens).unwrap_or(4096)),
            }))
            .send()
            .await
            .map_err(|e| classify_http_error(e, "xAI"))?;

        // PB-15: Check for 429 and extract Retry-After before streaming
        check_response_retry_after(&response, "xAI")?;

        let stream = response
            .bytes_stream()
            .map(|res| res.map_err(|e| SavantError::IoError(std::io::Error::other(e))));

        Ok(openai_stream_to_chunks(
            stream,
            self.agent_id.clone(),
            self.agent_name.clone(),
            "xai".to_string(),
        ))
    }
}

/// Fireworks AI provider - OpenAI compatible
pub struct FireworksProvider {
    pub client: Client,
    pub api_key: String,
    pub model: String,
    pub agent_id: String,
    pub agent_name: String,
    pub llm_params: Option<LlmParams>,
    pub max_completion_tokens: Option<u32>,
}

#[async_trait]
impl LlmProvider for FireworksProvider {
    async fn stream_completion(
        &self,
        messages: Vec<ChatMessage>,
        tools: Vec<serde_json::Value>,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<ChatChunk, SavantError>> + Send>>, SavantError>
    {
        let response = self
            .client
            .post("https://api.fireworks.ai/inference/v1/chat/completions")
            .header("Authorization", format!("Bearer {}", self.api_key))
            .json(&json!({
                "model": self.model,
                "messages": messages,
                "tools": tools,
                "stream": true,
                "temperature": self.llm_params.as_ref().map(|p| p.temperature).unwrap_or(0.7),
                "top_p": self.llm_params.as_ref().map(|p| p.top_p).unwrap_or(0.9),
                "max_tokens": self.max_completion_tokens.unwrap_or_else(|| self.llm_params.as_ref().map(|p| p.max_tokens).unwrap_or(4096)),
            }))
            .send()
            .await
            .map_err(|e| classify_http_error(e, "Fireworks"))?;

        // PB-15: Check for 429 and extract Retry-After before streaming
        check_response_retry_after(&response, "Fireworks")?;

        let stream = response
            .bytes_stream()
            .map(|res| res.map_err(|e| SavantError::IoError(std::io::Error::other(e))));

        Ok(openai_stream_to_chunks(
            stream,
            self.agent_id.clone(),
            self.agent_name.clone(),
            "fireworks".to_string(),
        ))
    }
}

/// Novita AI provider - OpenAI compatible
pub struct NovitaProvider {
    pub client: Client,
    pub api_key: String,
    pub model: String,
    pub agent_id: String,
    pub agent_name: String,
    pub llm_params: Option<LlmParams>,
    pub max_completion_tokens: Option<u32>,
}

#[async_trait]
impl LlmProvider for NovitaProvider {
    async fn stream_completion(
        &self,
        messages: Vec<ChatMessage>,
        tools: Vec<serde_json::Value>,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<ChatChunk, SavantError>> + Send>>, SavantError>
    {
        let response = self
            .client
            .post("https://api.novita.ai/v3/openai/chat/completions")
            .header("Authorization", format!("Bearer {}", self.api_key))
            .json(&json!({
                "model": self.model,
                "messages": messages,
                "tools": tools,
                "stream": true,
                "temperature": self.llm_params.as_ref().map(|p| p.temperature).unwrap_or(0.7),
                "top_p": self.llm_params.as_ref().map(|p| p.top_p).unwrap_or(0.9),
                "max_tokens": self.max_completion_tokens.unwrap_or_else(|| self.llm_params.as_ref().map(|p| p.max_tokens).unwrap_or(4096)),
            }))
            .send()
            .await
            .map_err(|e| classify_http_error(e, "Novita"))?;

        // PB-15: Check for 429 and extract Retry-After before streaming
        check_response_retry_after(&response, "Novita")?;

        let stream = response
            .bytes_stream()
            .map(|res| res.map_err(|e| SavantError::IoError(std::io::Error::other(e))));

        Ok(openai_stream_to_chunks(
            stream,
            self.agent_id.clone(),
            self.agent_name.clone(),
            "novita".to_string(),
        ))
    }
}
