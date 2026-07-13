use crate::error::SavantError;
use crate::traits::VisionProvider;
use async_trait::async_trait;
use tracing::{info, warn};

const DEFAULT_MODEL: &str = "gemma4";
const DEFAULT_URL: &str = "http://localhost:11434";

/// Known vision-capable model name substrings.
/// directly for multimodal requests instead of the separate generate API.
const VISION_MODEL_PATTERNS: &[&str] = &[
    "gemma4",
    "gemma-4",
    "qwen3-vl",
    "qwen2-vl",
    "llava",
    "bakllava",
    "moondream",
    "minicpm-v",
    "phi-3-vision",
    "phi-4-multimodal",
    "pixtral",
    "internvl",
    "idefics",
    "florence",
    "mistral-3",
    "cogvlm",
    "deepseek-vl",
];

/// Returns true if the given model name is known to support vision natively
/// via the chat API (images passed inline as base64).
fn is_vision_model(model: &str) -> bool {
    let lower = model.to_lowercase();
    VISION_MODEL_PATTERNS.iter().any(|p| lower.contains(p))
}

/// Vision service that uses Ollama for image understanding.
///
/// For vision-capable models (gemma4, llava, etc.), the chat API is used
/// directly with inline base64 images — no separate vision service needed.
/// For non-vision models, falls back to the generate API with the configured
/// vision model (default: gemma4).
///
/// The vision model is loaded on-demand and unloaded after each use
/// to minimize CPU/memory consumption. Set `keep_alive: 0` in generate requests
/// tells Ollama to evict the model from memory immediately after inference.
pub struct OllamaVisionService {
    client: reqwest::Client,
    url: String,
    model: String,
    /// Whether the configured model supports vision natively via chat API.
    model_is_vision: bool,
}

impl Default for OllamaVisionService {
    fn default() -> Self {
        let url = std::env::var("OLLAMA_URL").unwrap_or_else(|_| DEFAULT_URL.to_string());
        let model =
            std::env::var("OLLAMA_VISION_MODEL").unwrap_or_else(|_| DEFAULT_MODEL.to_string());
        let model_is_vision = is_vision_model(&model);
        Self {
            client: crate::net::secure_client_fallible().unwrap_or_else(|e| {
                tracing::warn!(
                    "Failed to create secure vision client: {}, using default",
                    e
                );
                reqwest::Client::new()
            }),
            url,
            model,
            model_is_vision,
        }
    }
}

impl OllamaVisionService {
    pub fn new() -> Result<Self, SavantError> {
        let url = std::env::var("OLLAMA_URL").unwrap_or_else(|_| DEFAULT_URL.to_string());
        let model =
            std::env::var("OLLAMA_VISION_MODEL").unwrap_or_else(|_| DEFAULT_MODEL.to_string());
        let model_is_vision = is_vision_model(&model);
        info!(
            "Initializing OllamaVisionService (model={}, url={}, vision_native={})",
            model, url, model_is_vision
        );
        Ok(Self {
            client: crate::net::secure_client_fallible()?,
            url,
            model,
            model_is_vision,
        })
    }

    pub fn with_config(url: &str, model: &str) -> Result<Self, SavantError> {
        let model_is_vision = is_vision_model(model);
        info!(
            "Initializing OllamaVisionService (model={}, url={}, vision_native={})",
            model, url, model_is_vision
        );
        Ok(Self {
            client: crate::net::secure_client_fallible()?,
            url: url.to_string(),
            model: model.to_string(),
            model_is_vision,
        })
    }

    /// Returns true if the configured model supports vision natively.
    pub fn is_model_vision(&self) -> bool {
        self.model_is_vision
    }
}

#[async_trait]
impl VisionProvider for OllamaVisionService {
    async fn describe_image(
        &self,
        image_base64: &str,
        prompt: &str,
    ) -> Result<String, SavantError> {
        if self.model_is_vision {
            // For vision-capable models (gemma4, qwen3-vl, etc.), use the chat API
            // with inline base64 images — no separate generate call needed.
            self.describe_image_chat(image_base64, prompt).await
        } else {
            // Fallback: use the generate API with a dedicated vision model.
            self.describe_image_generate(image_base64, prompt).await
        }
    }

    async fn is_available(&self) -> bool {
        match self
            .client
            .get(format!("{}/api/tags", self.url))
            .send()
            .await
        {
            Ok(resp) if resp.status().is_success() => {
                if let Ok(body) = resp.json::<serde_json::Value>().await {
                    let models = body["models"].as_array().cloned().unwrap_or_default();
                    let model_base = self.model.split(':').next().unwrap_or(&self.model);
                    return models.iter().any(|m| {
                        let name = m["name"].as_str().unwrap_or("");
                        name == self.model || name.starts_with(model_base)
                    });
                }
                false
            }
            _ => false,
        }
    }

    async fn unload_model(&self) -> Result<(), SavantError> {
        #[allow(clippy::disallowed_methods)]
        let unload_body = serde_json::json!({
            "model": self.model,
            "keep_alive": 0,
            "prompt": "",
            "stream": false
        });
        self.client
            .post(format!("{}/api/generate", self.url))
            .json(&unload_body)
            .send()
            .await
            .map_err(|e| SavantError::Unknown(format!("Failed to unload vision model: {}", e)))?;

        info!("Vision model {} unloaded from memory", self.model);
        Ok(())
    }
}

impl OllamaVisionService {
    /// Describes an image using the chat API (for vision-capable models like gemma4).
    async fn describe_image_chat(
        &self,
        image_base64: &str,
        prompt: &str,
    ) -> Result<String, SavantError> {
        #[allow(clippy::disallowed_methods)]
        let chat_body = serde_json::json!({
            "model": self.model,
            "messages": [{
                "role": "user",
                "content": prompt,
                "images": [image_base64]
            }],
            "stream": false,
            "keep_alive": 0
        });
        let resp: serde_json::Value = self
            .client
            .post(format!("{}/api/chat", self.url))
            .json(&chat_body)
            .send()
            .await
            .map_err(|e| SavantError::Unknown(format!("Ollama vision chat request failed: {}", e)))?
            .json()
            .await
            .map_err(|e| {
                SavantError::Unknown(format!("Ollama vision chat response parse failed: {}", e))
            })?;

        let response = resp["message"]["content"].as_str().ok_or_else(|| {
            SavantError::Unknown("No content in Ollama vision chat result".to_string())
        })?;

        Ok(response.to_string())
    }

    /// Describes an image using the generate API (fallback for non-vision models).
    async fn describe_image_generate(
        &self,
        image_base64: &str,
        prompt: &str,
    ) -> Result<String, SavantError> {
        #[allow(clippy::disallowed_methods)]
        let gen_body = serde_json::json!({
            "model": self.model,
            "prompt": prompt,
            "images": [image_base64],
            "stream": false,
            "keep_alive": 0
        });
        let resp: serde_json::Value = self
            .client
            .post(format!("{}/api/generate", self.url))
            .json(&gen_body)
            .send()
            .await
            .map_err(|e| SavantError::Unknown(format!("Ollama vision request failed: {}", e)))?
            .json()
            .await
            .map_err(|e| {
                SavantError::Unknown(format!("Ollama vision response parse failed: {}", e))
            })?;

        let response = resp["response"].as_str().ok_or_else(|| {
            SavantError::Unknown("No response in Ollama vision result".to_string())
        })?;

        Ok(response.to_string())
    }
}

/// Create a vision service. The model is loaded lazily on first `describe_image()` call
/// and unloaded from memory after each use to minimize resource consumption.
pub async fn create_vision_service() -> Option<Box<dyn VisionProvider>> {
    let svc = match OllamaVisionService::new() {
        Ok(s) => s,
        Err(e) => {
            warn!("Failed to create Ollama vision service: {}", e);
            return None;
        }
    };
    // Check if Ollama is running (not whether the model exists — model loads on-demand)
    match svc.client.get(format!("{}/api/tags", svc.url)).send().await {
        Ok(resp) if resp.status().is_success() => {
            info!("Ollama vision service ready (model will load on-demand)");
            Some(Box::new(svc))
        }
        _ => {
            warn!(
                "Ollama not running at {}. Vision service unavailable.",
                svc.url
            );
            None
        }
    }
}
