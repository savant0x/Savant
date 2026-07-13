//! Generation Tools
//!
//! MCP tools for image/video/SVG generation via the savant_generation crate.
// SAFETY: All `clippy::disallowed_methods` violations in this file originate from
// the `serde_json::json!()` macro, which internally uses `.unwrap()` on
// compile-time-validated JSON literals. A malformed JSON literal would be a
// compile error, making the panic path statically unreachable.
#![allow(clippy::disallowed_methods)]

use async_trait::async_trait;
use savant_core::error::SavantError;
use savant_core::traits::Tool;
use savant_generation::backends::GenerationBackend;
use std::sync::Arc;

use base64::Engine as _;

/// Tool for generating images via local models
pub struct GenerateImageTool {
    orchestrator: Arc<savant_generation::orchestrator::GenerationOrchestrator>,
}

impl GenerateImageTool {
    pub fn new(orchestrator: Arc<savant_generation::orchestrator::GenerationOrchestrator>) -> Self {
        Self { orchestrator }
    }
}

#[async_trait]
impl Tool for GenerateImageTool {
    fn name(&self) -> &str {
        "generate_image"
    }

    fn description(&self) -> &str {
        "Generate an image from a text description using local models (no cloud API). \
         Supports various art styles, aspect ratios, and quality tiers. \
         Returns the generated image as base64-encoded data."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        savant_generation::tools::generate_image_schema()
            .get("input_schema")
            .cloned()
            .unwrap_or_default()
    }

    async fn execute(&self, payload: serde_json::Value) -> Result<String, SavantError> {
        let subject = payload["subject"]
            .as_str()
            .ok_or_else(|| SavantError::Unknown("Missing 'subject' parameter".to_string()))?;

        let art_style = match payload["art_style"].as_str().unwrap_or("photorealistic") {
            "anime" => savant_generation::backends::ArtStyle::Anime,
            "surreal" => savant_generation::backends::ArtStyle::Surreal,
            "vector" => savant_generation::backends::ArtStyle::Vector,
            "pixel_art" => savant_generation::backends::ArtStyle::PixelArt,
            "oil_painting" => savant_generation::backends::ArtStyle::OilPainting,
            "watercolor" => savant_generation::backends::ArtStyle::Watercolor,
            "sketch" => savant_generation::backends::ArtStyle::Sketch,
            _ => savant_generation::backends::ArtStyle::Photorealistic,
        };

        let aspect_ratio = match payload["aspect_ratio"].as_str().unwrap_or("1:1") {
            "16:9" => savant_generation::backends::AspectRatio::Landscape,
            "9:16" => savant_generation::backends::AspectRatio::Portrait,
            "4:3" => savant_generation::backends::AspectRatio::Classic,
            _ => savant_generation::backends::AspectRatio::Square,
        };

        let quality_tier = match payload["quality_tier"].as_str().unwrap_or("balanced") {
            "fast" => savant_generation::backends::QualityTier::Fast,
            "quality" => savant_generation::backends::QualityTier::Quality,
            _ => savant_generation::backends::QualityTier::Balanced,
        };

        let params = savant_generation::backends::GenerationParams {
            art_style,
            aspect_ratio,
            quality_tier,
            negative_prompt: payload["negative_prompt"].as_str().map(|s| s.to_string()),
            seed: payload["seed"].as_u64(),
            steps: payload["steps"].as_u64().map(|s| s as u32),
            guidance_scale: payload["guidance_scale"].as_f64().map(|f| f as f32),
        };

        let result = self
            .orchestrator
            .generate_image(subject, &params)
            .await
            .map_err(|e| SavantError::Unknown(format!("Image generation failed: {}", e)))?;

        let output = serde_json::json!({
            "id": result.id,
            "prompt": result.prompt,
            "expanded_prompt": result.expanded_prompt,
            "image_base64": base64::engine::general_purpose::STANDARD.encode(&result.data),
            "mime_type": result.mime_type,
            "width": result.width,
            "height": result.height,
            "duration_ms": result.duration_ms,
            "model": result.model,
            "backend": result.backend,
            "cached": result.cached,
        });

        Ok(serde_json::to_string(&output).unwrap_or_else(|_| "{}".to_string()))
    }
}

/// Tool for generating SVG images via LLM (zero VRAM)
pub struct GenerateSvgTool {
    backend: Arc<savant_generation::backends::svg::SvgBackend>,
}

impl GenerateSvgTool {
    pub fn new(backend: Arc<savant_generation::backends::svg::SvgBackend>) -> Self {
        Self { backend }
    }
}

#[async_trait]
impl Tool for GenerateSvgTool {
    fn name(&self) -> &str {
        "generate_svg"
    }

    fn description(&self) -> &str {
        "Generate an SVG image via LLM. Zero VRAM required. \
         Best for diagrams, icons, charts, and simple illustrations. \
         Returns the generated image as base64-encoded PNG or raw SVG."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        savant_generation::tools::generate_svg_schema()
            .get("input_schema")
            .cloned()
            .unwrap_or_default()
    }

    async fn execute(&self, payload: serde_json::Value) -> Result<String, SavantError> {
        let subject = payload["subject"]
            .as_str()
            .ok_or_else(|| SavantError::Unknown("Missing 'subject' parameter".to_string()))?;

        let style = match payload["style"].as_str().unwrap_or("illustration") {
            "diagram" => savant_generation::backends::ArtStyle::Vector,
            "icon" => savant_generation::backends::ArtStyle::Vector,
            "chart" => savant_generation::backends::ArtStyle::Vector,
            _ => savant_generation::backends::ArtStyle::Vector,
        };

        let params = savant_generation::backends::GenerationParams {
            art_style: style,
            aspect_ratio: savant_generation::backends::AspectRatio::Square,
            quality_tier: savant_generation::backends::QualityTier::Fast,
            ..Default::default()
        };

        let result = self
            .backend
            .generate(subject, &params)
            .await
            .map_err(|e| SavantError::Unknown(format!("SVG generation failed: {}", e)))?;

        let format = payload["format"].as_str().unwrap_or("png");

        let output = if format == "svg" {
            serde_json::json!({
                "id": result.id,
                "prompt": result.prompt,
                "svg": String::from_utf8_lossy(&result.data),
                "format": "svg",
            })
        } else {
            serde_json::json!({
                "id": result.id,
                "prompt": result.prompt,
                "image_base64": base64::engine::general_purpose::STANDARD.encode(&result.data),
                "mime_type": "image/png",
                "width": result.width,
                "height": result.height,
                "format": "png",
            })
        };

        Ok(serde_json::to_string(&output).unwrap_or_else(|_| "{}".to_string()))
    }
}
