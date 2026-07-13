//! Savant Generation — Local multimodal generation (images, videos, SVG)
//!
//! This crate provides zero-cost, cross-platform generation capabilities
//! for the Savant agent framework. It supports:
//! - SVG generation via LLM (zero VRAM)
//! - Image generation via stable-diffusion.cpp FFI
//! - Video generation via Wan2.1/LTX-2.3
//! - VRAM ping-pong orchestration with Ollama
//! - Prompt expansion for T5-XXL encoders
//! - Image caching and memory integration

pub mod backends;
pub mod cache;
pub mod orchestrator;
pub mod prompt;
pub mod tools;

/// Generation error types
#[derive(Debug, thiserror::Error)]
pub enum GenerationError {
    #[error("Backend error: {0}")]
    Backend(String),

    #[error("VRAM insufficient: need {needed}MB, have {available}MB")]
    VramInsufficient { needed: u64, available: u64 },

    #[error("Model not found: {0}")]
    ModelNotFound(String),

    #[error("Model download failed: {0}")]
    DownloadFailed(String),

    #[error("Prompt expansion failed: {0}")]
    PromptExpansionFailed(String),

    #[error("SVG render failed: {0}")]
    SvgRenderFailed(String),

    #[error("Image encode failed: {0}")]
    ImageEncodeFailed(String),

    #[error("Cache error: {0}")]
    Cache(String),

    #[error("Ollama not available: {0}")]
    OllamaUnavailable(String),

    #[error("Generation timeout after {0}s")]
    Timeout(u64),

    #[error("Internal error: {0}")]
    Internal(String),

    #[error("Generation is disabled in configuration")]
    Disabled,

    #[error("Generation cooldown active — try again in {0}ms")]
    CooldownActive(u64),
}

/// Generation result with metadata
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct GenerationResult {
    /// Unique ID for this generation
    pub id: String,
    /// Original prompt
    pub prompt: String,
    /// Expanded prompt (after LLM expansion)
    pub expanded_prompt: String,
    /// Image/video bytes
    pub data: Vec<u8>,
    /// MIME type (image/png, image/webp, video/mp4, etc.)
    pub mime_type: String,
    /// Width in pixels
    pub width: u32,
    /// Height in pixels
    pub height: u32,
    /// Generation duration in milliseconds
    pub duration_ms: u64,
    /// Model used
    pub model: String,
    /// Backend used (svg, diffusion, ollama)
    pub backend: String,
    /// Whether this was served from cache
    pub cached: bool,
}

/// Video generation result
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct VideoResult {
    /// Unique ID for this generation
    pub id: String,
    /// Original prompt
    pub prompt: String,
    /// Video bytes
    pub data: Vec<u8>,
    /// MIME type (video/mp4, video/webm, image/gif)
    pub mime_type: String,
    /// Width in pixels
    pub width: u32,
    /// Height in pixels
    pub height: u32,
    /// Duration in seconds
    pub duration_secs: f32,
    /// Frame count
    pub frame_count: u32,
    /// Generation duration in milliseconds
    pub generation_duration_ms: u64,
    /// Model used
    pub model: String,
}

/// Generation configuration
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct GenerationConfig {
    /// Enable generation
    pub enabled: bool,
    /// Default backend (auto, svg, diffusion, ollama)
    pub default_backend: String,
    /// Default model name
    pub default_model: String,
    /// Max concurrent generations
    pub max_concurrent: u32,
    /// Cooldown between generations (ms)
    pub cooldown_ms: u64,
    /// Output directory for generated images
    pub output_dir: String,
    /// Enable image caching
    pub cache_enabled: bool,
    /// Max cache size in MB
    pub cache_max_size_mb: u64,
}

impl Default for GenerationConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            default_backend: "auto".to_string(),
            default_model: "sd3.5-medium-q5".to_string(),
            max_concurrent: 1,
            cooldown_ms: 5000,
            output_dir: ".savant/generated/images".to_string(),
            cache_enabled: true,
            cache_max_size_mb: 1024,
        }
    }
}
