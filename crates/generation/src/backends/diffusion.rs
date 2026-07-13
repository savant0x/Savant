//! Diffusion Backend
//!
//! Image generation via stable-diffusion.cpp FFI bindings (diffusion-rs crate).
//! Supports FLUX.2, SD3.5, Wan2.1, LTX-2.3, Z-Image models.
//! Cross-platform: CUDA, Vulkan, Metal, SYCL, CPU.

use async_trait::async_trait;
use futures::StreamExt;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::fs::File;
use tokio::io::AsyncWriteExt;
use tracing::info;

use super::{GenerationBackend, GenerationParams};
use crate::{GenerationError, GenerationResult};

const MAX_MODEL_BYTES: u64 = 5 * 1024 * 1024 * 1024; // 5 GB
const PROGRESS_LOG_BYTES: u64 = 10 * 1024 * 1024; // 10 MB

/// Supported diffusion model formats
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum DiffusionModel {
    /// SD3.5 Medium — 8GB VRAM, GGUF Q5 quantized
    Sd35Medium,
    /// FLUX.2 Klein 4B — 12GB VRAM, FP8
    Flux2Klein4b,
    /// FLUX.2 Klein 9B — 16GB VRAM
    Flux2Klein9b,
    /// FLUX.1 Schnell — 12GB VRAM, fast inference
    Flux1Schnell,
    /// Custom model path
    Custom(PathBuf),
}

impl DiffusionModel {
    /// Returns the default GGUF filename for this model
    pub fn default_filename(&self) -> &str {
        match self {
            Self::Sd35Medium => "sd3.5_medium_q5.gguf",
            Self::Flux2Klein4b => "flux2_klein_4b_fp8.gguf",
            Self::Flux2Klein9b => "flux2_klein_9b.gguf",
            Self::Flux1Schnell => "flux1_schnell.gguf",
            Self::Custom(_) => "custom.gguf",
        }
    }

    /// Returns estimated VRAM requirement in MB
    pub fn vram_mb(&self) -> u64 {
        match self {
            Self::Sd35Medium => 5110,
            Self::Flux2Klein4b => 13000,
            Self::Flux2Klein9b => 18000,
            Self::Flux1Schnell => 13000,
            Self::Custom(_) => 8000, // assume 8GB default
        }
    }

    /// Returns the HuggingFace repo ID for download
    pub fn hf_repo(&self) -> Option<&str> {
        match self {
            Self::Sd35Medium => Some("stable-diffusion-3.5-medium-gguf"),
            Self::Flux2Klein4b => Some("FLUX.2-klein-gguf"),
            Self::Flux2Klein9b => Some("FLUX.2-klein-gguf"),
            Self::Flux1Schnell => Some("FLUX.1-schnell-gguf"),
            Self::Custom(_) => None,
        }
    }
}

/// Inference backend type
#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize)]
pub enum InferenceBackend {
    Cuda,
    Vulkan,
    Metal,
    Sycl,
    Cpu,
}

impl InferenceBackend {
    /// Detect the best available backend for this system
    #[allow(unexpected_cfgs)]
    pub fn detect() -> Self {
        // Check for CUDA
        if cfg!(feature = "cuda") || std::env::var("CUDA_VISIBLE_DEVICES").is_ok() {
            return Self::Cuda;
        }

        // Check for Vulkan (cross-platform)
        if cfg!(feature = "vulkan") {
            return Self::Vulkan;
        }

        // Check for Metal (macOS)
        if cfg!(target_os = "macos") {
            return Self::Metal;
        }

        // Check for SYCL (Intel)
        if cfg!(feature = "sycl") {
            return Self::Sycl;
        }

        // Fallback to CPU
        Self::Cpu
    }

    /// Returns whether this backend supports GPU acceleration
    pub fn is_gpu(&self) -> bool {
        !matches!(self, Self::Cpu)
    }
}

/// Diffusion backend configuration
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DiffusionConfig {
    /// Model to use
    pub model: DiffusionModel,
    /// Inference backend
    pub backend: InferenceBackend,
    /// Model directory (where GGUF files are stored)
    pub model_dir: PathBuf,
    /// Enable VRAM offloading for large models
    pub vram_offload: bool,
    /// Number of CPU threads for CPU fallback
    pub cpu_threads: u32,
    /// Default number of inference steps
    pub default_steps: u32,
    /// Default guidance scale
    pub default_guidance: f32,
}

impl Default for DiffusionConfig {
    fn default() -> Self {
        Self {
            model: DiffusionModel::Sd35Medium,
            backend: InferenceBackend::detect(),
            model_dir: PathBuf::from(".savant/models/generation"),
            vram_offload: true,
            cpu_threads: num_cpus::get() as u32,
            default_steps: 30,
            default_guidance: 7.5,
        }
    }
}

/// Diffusion backend — generates images via stable-diffusion.cpp
pub struct DiffusionBackend {
    config: DiffusionConfig,
    /// Path to the loaded model file
    model_path: Option<PathBuf>,
    /// Whether the backend is initialized
    initialized: bool,
}

impl DiffusionBackend {
    /// Create a new diffusion backend
    pub fn new(config: DiffusionConfig) -> Self {
        Self {
            config,
            model_path: None,
            initialized: false,
        }
    }

    /// Initialize the backend — locate or download the model
    pub async fn initialize(&mut self) -> Result<(), GenerationError> {
        info!(
            "Diffusion backend: initializing with model {:?} on {:?}",
            self.config.model, self.config.backend
        );

        // Check if model file exists
        let model_path = self
            .config
            .model_dir
            .join(self.config.model.default_filename());

        if model_path.exists() {
            info!("Diffusion backend: model found at {}", model_path.display());
            self.model_path = Some(model_path);
            self.initialized = true;
            return Ok(());
        }

        // Model not found — try to download
        if let Some(hf_repo) = self.config.model.hf_repo() {
            info!(
                "Diffusion backend: model not found, downloading from HuggingFace: {}",
                hf_repo
            );
            self.download_model(hf_repo, &model_path, None).await?;
            self.model_path = Some(model_path);
            self.initialized = true;
            return Ok(());
        }

        Err(GenerationError::ModelNotFound(format!(
            "Model {:?} not found at {}",
            self.config.model,
            model_path.display()
        )))
    }

    /// Download model from HuggingFace with optional SHA-256 verification.
    async fn download_model(
        &self,
        hf_repo: &str,
        target_path: &Path,
        expected_sha256: Option<&str>,
    ) -> Result<(), GenerationError> {
        if target_path.exists() {
            info!(
                "Diffusion backend: model already exists at {}, skipping download",
                target_path.display()
            );
            return Ok(());
        }

        if let Some(parent) = target_path.parent() {
            tokio::fs::create_dir_all(parent).await.map_err(|e| {
                GenerationError::DownloadFailed(format!("Failed to create model dir: {}", e))
            })?;
        }

        let url = format!(
            "https://huggingface.co/{}/resolve/main/{}",
            hf_repo,
            self.config.model.default_filename()
        );

        info!("Diffusion backend: downloading from {}", url);

        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(300))
            .build()
            .map_err(|e| {
                GenerationError::DownloadFailed(format!("Failed to build HTTP client: {}", e))
            })?;

        let response = client.get(&url).send().await.map_err(|e| {
            GenerationError::DownloadFailed(format!("Download request failed: {}", e))
        })?;

        if !response.status().is_success() {
            return Err(GenerationError::DownloadFailed(format!(
                "Download failed with status: {}",
                response.status()
            )));
        }

        let content_length = response.content_length().unwrap_or(0);

        if content_length > MAX_MODEL_BYTES {
            return Err(GenerationError::DownloadFailed(format!(
                "Model too large: {} bytes exceeds {} byte limit",
                content_length, MAX_MODEL_BYTES
            )));
        }

        info!(
            "Diffusion backend: content-length = {} bytes",
            content_length
        );

        let tmp_path = target_path.with_extension("part");
        let mut file = File::create(&tmp_path).await.map_err(|e| {
            GenerationError::DownloadFailed(format!("Failed to create temp file: {}", e))
        })?;

        let mut stream = response.bytes_stream();
        let mut downloaded: u64 = 0;
        let mut next_log_threshold: u64 = PROGRESS_LOG_BYTES;
        let mut hasher = Sha256::new();

        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| {
                GenerationError::DownloadFailed(format!("Failed to read response chunk: {}", e))
            })?;

            hasher.update(&chunk);
            file.write_all(&chunk).await.map_err(|e| {
                GenerationError::DownloadFailed(format!("Failed to write chunk: {}", e))
            })?;

            downloaded += chunk.len() as u64;

            if downloaded >= next_log_threshold {
                info!(
                    "Diffusion backend: downloaded {} / {} bytes ({:.1}%)",
                    downloaded,
                    if content_length > 0 {
                        content_length.to_string()
                    } else {
                        "unknown".to_string()
                    },
                    if content_length > 0 {
                        (downloaded as f64 / content_length as f64) * 100.0
                    } else {
                        0.0
                    }
                );
                next_log_threshold += PROGRESS_LOG_BYTES;
            }
        }

        file.flush()
            .await
            .map_err(|e| GenerationError::DownloadFailed(format!("Failed to flush file: {}", e)))?;
        drop(file);

        if content_length > 0 && downloaded != content_length {
            let _ = tokio::fs::remove_file(&tmp_path).await;
            return Err(GenerationError::DownloadFailed(format!(
                "Size mismatch: expected {} bytes, got {}",
                content_length, downloaded
            )));
        }

        if let Some(expected) = expected_sha256 {
            let actual = format!("{:x}", hasher.finalize());
            if actual != expected {
                let _ = tokio::fs::remove_file(&tmp_path).await;
                return Err(GenerationError::DownloadFailed(format!(
                    "SHA-256 mismatch: expected {}, got {}",
                    expected, actual
                )));
            }
            info!("Diffusion backend: SHA-256 verified ({})", actual);
        } else {
            tracing::warn!("No checksum provided for model download — integrity not verified");
        }

        tokio::fs::rename(&tmp_path, target_path)
            .await
            .map_err(|e| {
                GenerationError::DownloadFailed(format!("Failed to rename temp file: {}", e))
            })?;

        info!(
            "Diffusion backend: downloaded {} bytes to {}",
            downloaded,
            target_path.display()
        );

        Ok(())
    }

    /// Generate image using stable-diffusion.cpp
    fn generate_sync(
        &self,
        prompt: &str,
        params: &GenerationParams,
    ) -> Result<GenerationResult, GenerationError> {
        let model_path = self.model_path.as_ref().ok_or_else(|| {
            GenerationError::Backend("Model not loaded — call initialize() first".to_string())
        })?;

        let (width, height) = params.aspect_ratio.dimensions();
        let steps = params.steps.unwrap_or(self.config.default_steps);
        let guidance = params
            .guidance_scale
            .unwrap_or(self.config.default_guidance);
        let seed = params.seed.unwrap_or_else(|| {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs()
        });

        info!(
            "Diffusion backend: generating {}x{} image with {} steps, guidance={}, seed={}",
            width, height, steps, guidance, seed
        );

        // Build the sd-cli command
        let output_path = std::env::temp_dir().join(format!("savant_gen_{}.png", seed));

        let mut cmd = std::process::Command::new("sd-cli");
        cmd.arg("-m")
            .arg(model_path.to_str().unwrap_or_default())
            .arg("-p")
            .arg(prompt)
            .arg("-o")
            .arg(output_path.to_str().unwrap_or_default())
            .arg("-W")
            .arg(width.to_string())
            .arg("-H")
            .arg(height.to_string())
            .arg("-s")
            .arg(steps.to_string())
            .arg("-g")
            .arg(guidance.to_string())
            .arg("--seed")
            .arg(seed.to_string());

        // Add negative prompt if provided
        if let Some(ref negative) = params.negative_prompt {
            cmd.arg("-n").arg(negative);
        }

        // Set backend-specific flags
        match self.config.backend {
            InferenceBackend::Cuda => {
                cmd.arg("--cuda");
            }
            InferenceBackend::Vulkan => {
                cmd.arg("--vulkan");
            }
            InferenceBackend::Metal => {
                cmd.arg("--metal");
            }
            InferenceBackend::Sycl => {
                cmd.arg("--sycl");
            }
            InferenceBackend::Cpu => {
                cmd.arg("--cpu")
                    .arg("--threads")
                    .arg(self.config.cpu_threads.to_string());
            }
        }

        // Enable VRAM offloading if configured
        if self.config.vram_offload {
            cmd.arg("--offload");
        }

        let start = std::time::Instant::now();

        // Execute generation
        let output = cmd
            .output()
            .map_err(|e| GenerationError::Backend(format!("Failed to execute sd-cli: {}", e)))?;

        let duration_ms = start.elapsed().as_millis() as u64;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(GenerationError::Backend(format!(
                "sd-cli failed with status {}: {}",
                output.status,
                stderr.chars().take(500).collect::<String>()
            )));
        }

        // Read generated image
        let image_data = std::fs::read(&output_path).map_err(|e| {
            GenerationError::Backend(format!("Failed to read generated image: {}", e))
        })?;

        // Clean up temp file
        let _ = std::fs::remove_file(&output_path);

        // Generate hash for caching
        let mut hasher = Sha256::new();
        hasher.update(prompt.as_bytes());
        hasher.update(params.art_style.to_string().as_bytes());
        hasher.update(width.to_string().as_bytes());
        hasher.update(height.to_string().as_bytes());
        hasher.update(seed.to_string().as_bytes());
        let hash = format!("{:x}", hasher.finalize());

        info!(
            "Diffusion backend: generated {}x{} image in {}ms ({} bytes)",
            width,
            height,
            duration_ms,
            image_data.len()
        );

        Ok(GenerationResult {
            id: format!("diffusion-{}", &hash[..12]),
            prompt: prompt.to_string(),
            expanded_prompt: prompt.to_string(),
            data: image_data,
            mime_type: "image/png".to_string(),
            width,
            height,
            duration_ms,
            model: format!("{:?}", self.config.model),
            backend: format!("{:?}", self.config.backend),
            cached: false,
        })
    }
}

#[async_trait]
impl GenerationBackend for DiffusionBackend {
    fn name(&self) -> &str {
        "diffusion"
    }

    fn requires_gpu(&self) -> bool {
        self.config.backend.is_gpu()
    }

    fn vram_requirement_mb(&self) -> u64 {
        self.config.model.vram_mb()
    }

    async fn generate(
        &self,
        prompt: &str,
        params: &GenerationParams,
    ) -> Result<GenerationResult, GenerationError> {
        if !self.initialized {
            return Err(GenerationError::Backend(
                "Diffusion backend not initialized — call initialize() first".to_string(),
            ));
        }

        // Run generation in a blocking task to avoid blocking the async runtime
        let prompt = prompt.to_string();
        let params = params.clone();
        let backend_config = self.config.clone();
        let model_path = self.model_path.clone();

        tokio::task::spawn_blocking(move || {
            let backend = DiffusionBackend {
                config: backend_config,
                model_path,
                initialized: true,
            };
            backend.generate_sync(&prompt, &params)
        })
        .await
        .map_err(|e| GenerationError::Backend(format!("Generation task panicked: {}", e)))?
    }

    async fn is_available(&self) -> bool {
        // Check if sd-cli is available
        std::process::Command::new("sd-cli")
            .arg("--version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }
}
