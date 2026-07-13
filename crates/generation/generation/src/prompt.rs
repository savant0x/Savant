//! Prompt Expansion
//!
//! Expands simple user prompts into detailed descriptions suitable for
//! T5-XXL and other advanced text encoders used by modern diffusion models.

use std::sync::Arc;
use tracing::debug;

use crate::GenerationError;

/// Prompt expander for diffusion models
///
/// Takes simple user prompts and expands them into detailed, natural language
/// descriptions that work well with T5-XXL and similar encoders.
pub struct PromptExpander {
    /// LLM provider for prompt expansion
    provider: Option<Arc<dyn savant_core::traits::LlmProvider>>,
}

impl PromptExpander {
    /// Create a new prompt expander
    pub fn new(provider: Option<Arc<dyn savant_core::traits::LlmProvider>>) -> Self {
        Self { provider }
    }

    /// Expand a simple prompt into a detailed description
    ///
    /// # Arguments
    /// * `prompt` - Simple user prompt (e.g., "a cat")
    /// * `style` - Art style hint
    ///
    /// # Returns
    /// Expanded prompt suitable for T5-XXL encoders
    pub async fn expand(&self, prompt: &str, style: &str) -> Result<String, GenerationError> {
        // If prompt is already detailed (> 100 chars), return as-is
        if prompt.len() > 100 {
            debug!(
                "Prompt already detailed ({} chars), skipping expansion",
                prompt.len()
            );
            return Ok(prompt.to_string());
        }

        if let Some(ref provider) = self.provider {
            self.expand_with_llm(prompt, style, provider).await
        } else {
            // Fallback: expand with template
            Ok(self.expand_with_template(prompt, style))
        }
    }

    /// Expand prompt using LLM
    async fn expand_with_llm(
        &self,
        prompt: &str,
        style: &str,
        provider: &Arc<dyn savant_core::traits::LlmProvider>,
    ) -> Result<String, GenerationError> {
        let system_prompt = format!(
            "You are a prompt expansion expert for image generation models. \
             Given a simple prompt, expand it into a detailed, natural language \
             description that will produce high-quality results with T5-XXL encoders. \
             \
             Rules:\
             - Use descriptive, spatial language (position, lighting, composition)\
             - Include details about color, texture, atmosphere\
             - Describe the scene as if narrating to someone who can't see it\
             - Do NOT use keyword lists or tags\
             - Keep it to 1-2 sentences\
             - Style: {}",
            style
        );

        let messages = vec![
            savant_core::types::ChatMessage {
                is_telemetry: false,
                role: savant_core::types::ChatRole::System,
                content: system_prompt,
                sender: None,
                recipient: None,
                agent_id: None,
                session_id: None,
                channel: savant_core::types::AgentOutputChannel::Chat,
                images: Vec::new(),
                ..Default::default()
            },
            savant_core::types::ChatMessage {
                is_telemetry: false,
                role: savant_core::types::ChatRole::User,
                content: format!("Expand this prompt: {}", prompt),
                sender: None,
                recipient: None,
                agent_id: None,
                session_id: None,
                channel: savant_core::types::AgentOutputChannel::Chat,
                images: Vec::new(),
                ..Default::default()
            },
        ];

        let stream = provider
            .stream_completion(messages, Vec::new())
            .await
            .map_err(|e| {
                GenerationError::PromptExpansionFailed(format!("LLM call failed: {}", e))
            })?;

        // Collect stream into response
        let mut response = String::new();
        use futures::StreamExt;
        let mut stream = stream;
        while let Some(chunk) = stream.next().await {
            match chunk {
                Ok(chunk) => {
                    response.push_str(&chunk.content);
                }
                Err(e) => {
                    return Err(GenerationError::PromptExpansionFailed(format!(
                        "LLM stream error: {}",
                        e
                    )));
                }
            }
        }

        let expanded = response.trim().to_string();

        debug!(
            "Expanded prompt: {} -> {} chars",
            prompt.len(),
            expanded.len()
        );

        Ok(expanded)
    }

    /// Expand prompt using template (fallback when no LLM available)
    fn expand_with_template(&self, prompt: &str, style: &str) -> String {
        let style_desc = match style {
            "photorealistic" => "photorealistic, high detail, professional photography",
            "anime" => "anime style, vibrant colors, clean lines",
            "surreal" => "surrealist, dreamlike, fantastical",
            "vector" => "vector art, clean shapes, bold colors",
            "pixel_art" => "pixel art, retro gaming style",
            "oil_painting" => "oil painting style, rich textures, classical",
            "watercolor" => "watercolor painting, soft edges, flowing colors",
            "sketch" => "pencil sketch, detailed linework",
            _ => "high quality, detailed",
        };

        format!(
            "A {} rendering of {}, with {}, warm lighting, \
             shallow depth of field, highly detailed, professional quality",
            style_desc, prompt, style_desc
        )
    }

    /// Expand prompt for video generation
    pub async fn expand_for_video(&self, prompt: &str) -> Result<String, GenerationError> {
        if let Some(ref provider) = self.provider {
            let system_prompt = "You are a prompt expansion expert for video generation models. \
                 Given a simple prompt, expand it into a detailed description of a short video clip. \
                 \
                 Rules:\
                 - Describe the motion and action in detail\
                 - Include camera movement (pan, zoom, tracking)\
                 - Describe the scene progression over time\
                 - Keep it to 2-3 sentences\
                 - Focus on visual details, not dialogue";

            let messages = vec![
                savant_core::types::ChatMessage {
                    is_telemetry: false,
                    role: savant_core::types::ChatRole::System,
                    content: system_prompt.to_string(),
                    sender: None,
                    recipient: None,
                    agent_id: None,
                    session_id: None,
                    channel: savant_core::types::AgentOutputChannel::Chat,
                    images: Vec::new(),
                    ..Default::default()
                },
                savant_core::types::ChatMessage {
                    is_telemetry: false,
                    role: savant_core::types::ChatRole::User,
                    content: format!("Expand this video prompt: {}", prompt),
                    sender: None,
                    recipient: None,
                    agent_id: None,
                    session_id: None,
                    channel: savant_core::types::AgentOutputChannel::Chat,
                    images: Vec::new(),
                    ..Default::default()
                },
            ];

            let stream = provider
                .stream_completion(messages, Vec::new())
                .await
                .map_err(|e| {
                    GenerationError::PromptExpansionFailed(format!("LLM call failed: {}", e))
                })?;

            // Collect stream into response
            let mut response = String::new();
            use futures::StreamExt;
            let mut stream = stream;
            while let Some(chunk) = stream.next().await {
                match chunk {
                    Ok(chunk) => {
                        response.push_str(&chunk.content);
                    }
                    Err(e) => {
                        return Err(GenerationError::PromptExpansionFailed(format!(
                            "LLM stream error: {}",
                            e
                        )));
                    }
                }
            }

            Ok(response.trim().to_string())
        } else {
            Ok(format!(
                "A cinematic shot of {}. Camera slowly pans across the scene. \
                 Smooth motion, high quality, professional videography.",
                prompt
            ))
        }
    }
}
