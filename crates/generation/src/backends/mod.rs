//! Generation backends
//!
//! Each backend implements the `GenerationBackend` trait for a specific
//! inference engine (SVG, diffusion-rs, Ollama, etc.)

pub mod diffusion;
pub mod svg;

use crate::{GenerationError, GenerationResult};
use async_trait::async_trait;

/// Trait for generation backends
#[async_trait]
pub trait GenerationBackend: Send + Sync {
    /// Returns the backend name
    fn name(&self) -> &str;

    /// Returns whether this backend requires GPU
    fn requires_gpu(&self) -> bool;

    /// Returns estimated VRAM requirement in MB
    fn vram_requirement_mb(&self) -> u64;

    /// Generate an image from a prompt
    async fn generate(
        &self,
        prompt: &str,
        params: &GenerationParams,
    ) -> Result<GenerationResult, GenerationError>;

    /// Check if this backend is available on this system
    async fn is_available(&self) -> bool;
}

/// Parameters for generation
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct GenerationParams {
    /// Art style
    pub art_style: ArtStyle,
    /// Aspect ratio
    pub aspect_ratio: AspectRatio,
    /// Quality tier
    pub quality_tier: QualityTier,
    /// Negative prompt (what to avoid)
    pub negative_prompt: Option<String>,
    /// Seed for reproducibility
    pub seed: Option<u64>,
    /// Number of inference steps
    pub steps: Option<u32>,
    /// Guidance scale
    pub guidance_scale: Option<f32>,
}

impl Default for GenerationParams {
    fn default() -> Self {
        Self {
            art_style: ArtStyle::Photorealistic,
            aspect_ratio: AspectRatio::Square,
            quality_tier: QualityTier::Balanced,
            negative_prompt: None,
            seed: None,
            steps: None,
            guidance_scale: None,
        }
    }
}

/// Art style for generation
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum ArtStyle {
    Photorealistic,
    Anime,
    Surreal,
    Vector,
    PixelArt,
    OilPainting,
    Watercolor,
    Sketch,
}

impl std::fmt::Display for ArtStyle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Photorealistic => write!(f, "photorealistic"),
            Self::Anime => write!(f, "anime"),
            Self::Surreal => write!(f, "surreal"),
            Self::Vector => write!(f, "vector"),
            Self::PixelArt => write!(f, "pixel_art"),
            Self::OilPainting => write!(f, "oil_painting"),
            Self::Watercolor => write!(f, "watercolor"),
            Self::Sketch => write!(f, "sketch"),
        }
    }
}

/// Aspect ratio for generation
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum AspectRatio {
    Square,    // 1:1 (1024x1024)
    Landscape, // 16:9 (1280x720)
    Portrait,  // 9:16 (720x1280)
    Classic,   // 4:3 (1024x768)
}

impl AspectRatio {
    /// Returns (width, height) for this aspect ratio
    pub fn dimensions(&self) -> (u32, u32) {
        match self {
            Self::Square => (1024, 1024),
            Self::Landscape => (1280, 720),
            Self::Portrait => (720, 1280),
            Self::Classic => (1024, 768),
        }
    }
}

impl std::fmt::Display for AspectRatio {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Square => write!(f, "1:1"),
            Self::Landscape => write!(f, "16:9"),
            Self::Portrait => write!(f, "9:16"),
            Self::Classic => write!(f, "4:3"),
        }
    }
}

/// Quality tier for generation
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum QualityTier {
    Fast,     // Fewer steps, lower quality
    Balanced, // Default steps
    Quality,  // More steps, higher quality
}

impl QualityTier {
    /// Returns default step count for this tier
    pub fn default_steps(&self) -> u32 {
        match self {
            Self::Fast => 20,
            Self::Balanced => 30,
            Self::Quality => 50,
        }
    }
}

impl std::fmt::Display for QualityTier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Fast => write!(f, "fast"),
            Self::Balanced => write!(f, "balanced"),
            Self::Quality => write!(f, "quality"),
        }
    }
}
