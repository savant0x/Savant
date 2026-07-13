//! SVG Generation Backend
//!
//! Generates images via LLM-generated SVG markup rendered through resvg.
//! This is the zero-VRAM fast path — no GPU required.

use async_trait::async_trait;
use sha2::{Digest, Sha256};
use tracing::{debug, info};

use super::{ArtStyle, GenerationBackend, GenerationParams};
use crate::{GenerationError, GenerationResult};

/// SVG generation backend
///
/// Generates SVG markup via the LLM, then renders to PNG using resvg.
/// This is the fastest path — no GPU required, no model loading.
pub struct SvgBackend {
    /// LLM provider for SVG generation
    provider: Option<Arc<dyn savant_core::traits::LlmProvider>>,
}

use std::sync::Arc;

impl SvgBackend {
    /// Create a new SVG backend
    pub fn new(provider: Option<Arc<dyn savant_core::traits::LlmProvider>>) -> Self {
        Self { provider }
    }

    /// Generate SVG markup from a prompt using the LLM
    async fn generate_svg_markup(
        &self,
        prompt: &str,
        params: &GenerationParams,
    ) -> Result<String, GenerationError> {
        let style_hint = match params.art_style {
            ArtStyle::Vector => "Use clean geometric shapes, bold colors, and simple lines.",
            _ => "Create a visually appealing illustration.",
        };

        let (width, height) = params.aspect_ratio.dimensions();

        let system_prompt = format!(
            "You are an SVG generator. Generate ONLY valid SVG markup. \
             No explanations, no markdown code blocks, just raw SVG. \
             The SVG must be {}x{} pixels. {} \
             Use clean, well-formed XML.",
            width, height, style_hint
        );

        let user_prompt = format!("Generate an SVG image of: {}", prompt);

        if let Some(ref provider) = self.provider {
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
                    content: user_prompt,
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
                .map_err(|e| GenerationError::Backend(format!("LLM call failed: {}", e)))?;

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
                        return Err(GenerationError::Backend(format!("LLM stream error: {}", e)));
                    }
                }
            }

            // Extract SVG from response
            let svg = Self::extract_svg(&response)?;
            Ok(svg)
        } else {
            // Fallback: generate a simple placeholder SVG
            Ok(Self::generate_placeholder_svg(prompt, width, height))
        }
    }

    /// Extract SVG markup from LLM response
    fn extract_svg(response: &str) -> Result<String, GenerationError> {
        // Try to find SVG in the response
        let trimmed = response.trim();

        // If response starts with <svg, it's raw SVG
        if trimmed.starts_with("<svg") {
            return Ok(trimmed.to_string());
        }

        // Try to extract from markdown code block
        if let Some(start) = trimmed.find("```svg") {
            let svg_start = start + 6;
            if let Some(end) = trimmed[svg_start..].find("```") {
                return Ok(trimmed[svg_start..svg_start + end].trim().to_string());
            }
        }

        // Try to extract from any code block
        if let Some(start) = trimmed.find("```") {
            let code_start = start + 3;
            // Skip language identifier
            let code_start = if let Some(newline) = trimmed[code_start..].find('\n') {
                code_start + newline + 1
            } else {
                code_start
            };
            if let Some(end) = trimmed[code_start..].find("```") {
                let code = trimmed[code_start..code_start + end].trim();
                if code.starts_with("<svg") {
                    return Ok(code.to_string());
                }
            }
        }

        // Try to find <svg> anywhere in the response
        if let Some(start) = trimmed.find("<svg") {
            if let Some(end) = trimmed[start..].find("</svg>") {
                return Ok(trimmed[start..start + end + 6].to_string());
            }
        }

        Err(GenerationError::SvgRenderFailed(
            "Could not extract SVG from LLM response".to_string(),
        ))
    }

    /// Generate a simple placeholder SVG
    fn generate_placeholder_svg(prompt: &str, width: u32, height: u32) -> String {
        let cx = width / 2;
        let cy = height / 2;
        format!(
            "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{w}\" height=\"{h}\" viewBox=\"0 0 {w} {h}\">\
            <rect width=\"{w}\" height=\"{h}\" fill=\"#f0f0f0\"/>\
            <text x=\"{cx}\" y=\"{cy}\" text-anchor=\"middle\" font-family=\"Arial\" font-size=\"16\" fill=\"#333\">\
            {p}\
            </text>\
            </svg>",
            w = width,
            h = height,
            cx = cx,
            cy = cy,
            p = prompt
        )
    }

    /// Render SVG to PNG using resvg
    fn render_svg_to_png(svg_str: &str) -> Result<Vec<u8>, GenerationError> {
        // Parse SVG
        let opts = usvg::Options::default();
        let tree = usvg::Tree::from_str(svg_str, &opts)
            .map_err(|e| GenerationError::SvgRenderFailed(format!("SVG parse error: {}", e)))?;

        // Get dimensions
        let size = tree.size();
        let width = size.width() as u32;
        let height = size.height() as u32;

        // Create pixmap
        let mut pixmap = tiny_skia::Pixmap::new(width, height).ok_or_else(|| {
            GenerationError::SvgRenderFailed("Failed to create pixmap".to_string())
        })?;

        // Render SVG to pixmap
        resvg::render(&tree, tiny_skia::Transform::default(), &mut pixmap.as_mut());

        // Encode to PNG
        let png_data = pixmap
            .encode_png()
            .map_err(|e| GenerationError::ImageEncodeFailed(format!("PNG encode error: {}", e)))?;

        Ok(png_data)
    }
}

#[async_trait]
impl GenerationBackend for SvgBackend {
    fn name(&self) -> &str {
        "svg"
    }

    fn requires_gpu(&self) -> bool {
        false
    }

    fn vram_requirement_mb(&self) -> u64 {
        0
    }

    async fn generate(
        &self,
        prompt: &str,
        params: &GenerationParams,
    ) -> Result<GenerationResult, GenerationError> {
        let start = std::time::Instant::now();

        info!("SVG backend: generating image for prompt: {}", prompt);

        // Generate SVG markup via LLM
        let svg_markup = self.generate_svg_markup(prompt, params).await?;

        debug!("SVG markup generated ({} bytes)", svg_markup.len());

        // Render SVG to PNG
        let png_data = Self::render_svg_to_png(&svg_markup)?;

        let duration_ms = start.elapsed().as_millis() as u64;

        // Generate hash for caching
        let mut hasher = Sha256::new();
        hasher.update(prompt.as_bytes());
        hasher.update(params.art_style.to_string().as_bytes());
        hasher.update(params.aspect_ratio.to_string().as_bytes());
        let hash = format!("{:x}", hasher.finalize());

        let (width, height) = params.aspect_ratio.dimensions();

        info!(
            "SVG backend: generated {}x{} image in {}ms",
            width, height, duration_ms
        );

        Ok(GenerationResult {
            id: format!("svg-{}", &hash[..12]),
            prompt: prompt.to_string(),
            expanded_prompt: prompt.to_string(), // SVG doesn't need expansion
            data: png_data,
            mime_type: "image/png".to_string(),
            width,
            height,
            duration_ms,
            model: "svg-llm".to_string(),
            backend: "svg".to_string(),
            cached: false,
        })
    }

    async fn is_available(&self) -> bool {
        // SVG backend is always available (no GPU required)
        true
    }
}
