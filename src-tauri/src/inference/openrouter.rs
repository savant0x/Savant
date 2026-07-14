//! OpenRouter chat-completions client.
//!
//! Reads the `openrouter-default` profile from the vault, POSTs a single-turn
//! `messages: [{role: "user", content: prompt}]` payload, returns the first
//! choice's `message.content`. Used by the `infer_openrouter` Tauri command
//! for Phase 1 smoke-test verification.

use serde::{Deserialize, Serialize};
use thiserror::Error;

use savant_vault::master_key;

/// Public so tests can pin the exact URL against accidental drift.
pub const OPENROUTER_CHAT_COMPLETIONS: &str =
    "https://openrouter.ai/api/v1/chat/completions";
const DEFAULT_MODEL: &str = "openai/gpt-4o-mini";
const REQUEST_TIMEOUT_SECS: u64 = 60;

#[derive(Error, Debug)]
pub enum InferenceError {
    #[error("Vault error: {0}")]
    Vault(#[from] master_key::VaultError),
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("Bad response: {0}")]
    BadResponse(String),
}

#[derive(Debug, Serialize)]
struct ChatMessage {
    role: String,
    content: String,
}

#[derive(Debug, Serialize)]
struct ChatRequest {
    model: String,
    messages: Vec<ChatMessage>,
    max_tokens: u32,
    stream: bool,
}

#[derive(Debug, Deserialize)]
struct ChatChoice {
    message: ChatMessageOut,
}

#[derive(Debug, Deserialize)]
struct ChatMessageOut {
    content: String,
}

#[derive(Debug, Deserialize)]
struct ChatResponse {
    choices: Vec<ChatChoice>,
}

/// Sends a single chat completion request and returns the first choice's content.
pub async fn chat_completion(prompt: &str) -> std::result::Result<String, InferenceError> {
    let api_key = master_key::lookup_api_key("openrouter-default").await?;

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(REQUEST_TIMEOUT_SECS))
        .build()?;

    let request = ChatRequest {
        model: DEFAULT_MODEL.to_string(),
        messages: vec![ChatMessage {
            role: "user".to_string(),
            content: prompt.to_string(),
        }],
        max_tokens: 512,
        stream: false,
    };

    let response = client
        .post(OPENROUTER_CHAT_COMPLETIONS)
        .bearer_auth(&api_key)
        .header("HTTP-Referer", "https://github.com/savant0x/Savant")
        .header("X-Title", "Savant Phase 1")
        .json(&request)
        .send()
        .await?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(InferenceError::BadResponse(format!(
            "HTTP {} — {}",
            status, body
        )));
    }

    let parsed: ChatResponse = response.json().await?;
    let first = parsed
        .choices
        .first()
        .ok_or_else(|| InferenceError::BadResponse("no choices returned".to_string()))?;
    Ok(first.message.content.clone())
}
