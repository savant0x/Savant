use crate::error::SavantError;
use crate::traits::EmbeddingProvider;
use async_trait::async_trait;
use lru::LruCache;
use std::num::NonZeroUsize;
use tracing::{error, info, warn};

#[allow(clippy::disallowed_methods)]
const CACHE_CAPACITY: NonZeroUsize = NonZeroUsize::new(1000).expect("1000 is non-zero");

const DEFAULT_MODEL: &str = "nomic-embed-text";
const DEFAULT_URL: &str = "http://localhost:11434";

/// Embedding service that uses Ollama for high-quality embeddings.
/// Falls back to NullEmbeddingProvider when SAVANT_DISABLE_EMBEDDINGS=1.
pub struct OllamaEmbeddingService {
    client: reqwest::Client,
    url: String,
    model: String,
    cache: tokio::sync::Mutex<LruCache<String, Vec<f32>>>,
}

/// No-op embedding provider for degraded mode.
/// Returns zero vectors. Semantic search is disabled.
pub struct NullEmbeddingProvider;

#[async_trait]
impl EmbeddingProvider for NullEmbeddingProvider {
    async fn embed(&self, _text: &str) -> Result<Vec<f32>, SavantError> {
        Ok(vec![0.0; 768])
    }
    async fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, SavantError> {
        Ok(texts.iter().map(|_| vec![0.0; 768]).collect())
    }
    fn dimensions(&self) -> usize {
        768
    }
}

impl OllamaEmbeddingService {
    pub fn new() -> Result<Self, SavantError> {
        let url = std::env::var("OLLAMA_URL").unwrap_or_else(|_| DEFAULT_URL.to_string());
        let model =
            std::env::var("OLLAMA_EMBED_MODEL").unwrap_or_else(|_| DEFAULT_MODEL.to_string());
        info!(
            "Initializing OllamaEmbeddingService (model={}, url={})",
            model, url
        );
        Ok(Self {
            client: crate::net::secure_client_fallible()?,
            url,
            model,
            cache: tokio::sync::Mutex::new(LruCache::new(CACHE_CAPACITY)),
        })
    }

    pub fn with_config(url: &str, model: &str) -> Result<Self, SavantError> {
        info!(
            "Initializing OllamaEmbeddingService (model={}, url={})",
            model, url
        );
        Ok(Self {
            client: crate::net::secure_client_fallible()?,
            url: url.to_string(),
            model: model.to_string(),
            cache: tokio::sync::Mutex::new(LruCache::new(CACHE_CAPACITY)),
        })
    }

    async fn call_ollama(&self, text: &str) -> Result<Vec<f32>, SavantError> {
        #[allow(clippy::disallowed_methods)]
        let body = serde_json::json!({
            "model": self.model,
            "prompt": text
        });
        let resp: serde_json::Value = self
            .client
            .post(format!("{}/api/embeddings", self.url))
            .json(&body)
            .send()
            .await
            .map_err(|e| SavantError::Unknown(format!("Ollama request failed: {}", e)))?
            .json()
            .await
            .map_err(|e| SavantError::Unknown(format!("Ollama response parse failed: {}", e)))?;

        let embedding = resp["embedding"]
            .as_array()
            .ok_or_else(|| SavantError::Unknown("No embedding in Ollama response".to_string()))?
            .iter()
            .filter_map(|v| v.as_f64())
            .map(|f| f as f32)
            .collect::<Vec<f32>>();

        if embedding.is_empty() || embedding.iter().all(|&v| v == 0.0) {
            return Err(SavantError::Unknown(
                "Ollama returned zero embedding".to_string(),
            ));
        }

        Ok(embedding)
    }

    pub fn dimensions(&self) -> usize {
        768
    }
}

#[async_trait]
impl EmbeddingProvider for OllamaEmbeddingService {
    async fn embed(&self, text: &str) -> Result<Vec<f32>, SavantError> {
        // Check cache
        {
            let mut cache = self.cache.lock().await;
            if let Some(cached) = cache.get(text) {
                return Ok(cached.clone());
            }
        }

        let embedding = self.call_ollama(text).await?;

        // Cache result
        {
            let mut cache = self.cache.lock().await;
            cache.put(text.to_string(), embedding.clone());
        }

        Ok(embedding)
    }

    async fn embed_batch(&self, texts: &[&str]) -> Result<Vec<Vec<f32>>, SavantError> {
        let mut results = Vec::with_capacity(texts.len());
        for text in texts {
            results.push(self.embed(text).await?);
        }
        Ok(results)
    }

    fn dimensions(&self) -> usize {
        OllamaEmbeddingService::dimensions(self)
    }
}

/// Attempts to find the Ollama executable on the system.
fn find_ollama_executable() -> Option<std::path::PathBuf> {
    // Windows common install locations
    #[cfg(target_os = "windows")]
    {
        if let Ok(local_app_data) = std::env::var("LOCALAPPDATA") {
            let candidates = [
                format!("{}\\Programs\\Ollama\\ollama.exe", local_app_data),
                format!("{}\\Ollama\\ollama.exe", local_app_data),
            ];
            for candidate in &candidates {
                let path = std::path::Path::new(candidate);
                if path.exists() {
                    return Some(path.to_path_buf());
                }
            }
        }
    }

    // Linux/macOS common locations
    #[cfg(not(target_os = "windows"))]
    {
        let candidates = ["/usr/local/bin/ollama", "/usr/bin/ollama"];
        for candidate in &candidates {
            let path = std::path::Path::new(candidate);
            if path.exists() {
                return Some(path.to_path_buf());
            }
        }
    }

    // Try PATH via spawning 'ollama --version'
    let test = std::process::Command::new("ollama")
        .arg("--version")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
    if test.is_ok() {
        return Some(std::path::PathBuf::from("ollama"));
    }

    None
}

/// Attempts to start the Ollama server process.
/// Used for self-healing: when embeddings fail mid-session, call this then retry.
pub async fn auto_start_ollama() -> Result<(), SavantError> {
    let ollama_path = find_ollama_executable().ok_or_else(|| {
        SavantError::Unknown(
            "Ollama executable not found. Install from https://ollama.com/download".to_string(),
        )
    })?;

    info!("Starting Ollama server: {}", ollama_path.display());

    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        std::process::Command::new(&ollama_path)
            .arg("serve")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .creation_flags(0x08000000) // CREATE_NO_WINDOW
            .spawn()
            .map_err(|e| SavantError::Unknown(format!("Failed to start Ollama: {}", e)))?;
    }

    #[cfg(not(target_os = "windows"))]
    {
        std::process::Command::new(&ollama_path)
            .arg("serve")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map_err(|e| SavantError::Unknown(format!("Failed to start Ollama: {}", e)))?;
    }

    // Wait for Ollama to become ready (up to 30 seconds)
    info!("Waiting for Ollama server to become ready...");
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .map_err(|e| SavantError::Unknown(format!("HTTP client error: {}", e)))?;

    let url = std::env::var("OLLAMA_URL").unwrap_or_else(|_| DEFAULT_URL.to_string());
    for i in 0..6 {
        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        match client.get(format!("{}/api/tags", url)).send().await {
            Ok(resp) if resp.status().is_success() => {
                info!("Ollama server is ready (took {}s)", (i + 1) * 5);
                return Ok(());
            }
            _ => {
                info!("Ollama not ready yet, retrying... ({}/6)", i + 1);
            }
        }
    }

    Err(SavantError::Unknown(
        "Ollama server started but did not become ready within 30 seconds".to_string(),
    ))
}

/// Checks if the required embedding model is available, pulls if needed.
async fn ensure_model(client: &reqwest::Client, url: &str, model: &str) -> Result<(), SavantError> {
    let resp = client
        .get(format!("{}/api/tags", url))
        .send()
        .await
        .map_err(|e| SavantError::Unknown(format!("Failed to query Ollama models: {}", e)))?;

    let body: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| SavantError::Unknown(format!("Failed to parse Ollama response: {}", e)))?;

    let models = body["models"].as_array().cloned().unwrap_or_default();
    let has_model = models.iter().any(|m| {
        let name = m["name"].as_str().unwrap_or("");
        name == model || name.starts_with(model)
    });

    if has_model {
        info!("Embedding model {} found in Ollama", model);
        return Ok(());
    }

    warn!("Embedding model {} not found. Pulling...", model);
    #[allow(clippy::disallowed_methods)]
    let pull_body = serde_json::json!({ "model": model });
    let pull_resp = client
        .post(format!("{}/api/pull", url))
        .json(&pull_body)
        .send()
        .await
        .map_err(|e| SavantError::Unknown(format!("Failed to pull model: {}", e)))?;

    if !pull_resp.status().is_success() {
        return Err(SavantError::Unknown(format!(
            "Failed to pull model {}: HTTP {}",
            model,
            pull_resp.status()
        )));
    }

    info!("Model {} pulled successfully", model);
    Ok(())
}

/// Creates the embedding service.
///
/// Startup sequence:
/// 1. Check SAVANT_DISABLE_EMBEDDINGS env var — if "1", return NullEmbeddingProvider
/// 2. Check if Ollama is running
/// 3. If not, try to auto-start it
/// 4. Check if embedding model exists, pull if needed
/// 5. Return Ollama embedding service
///
/// If any step fails and SAVANT_DISABLE_EMBEDDINGS=1, returns NullEmbeddingProvider.
/// Otherwise returns a hard error.
pub async fn create_embedding_service(
    model_override: Option<&str>,
) -> Result<Box<dyn EmbeddingProvider>, SavantError> {
    // Check if embeddings are explicitly disabled
    if std::env::var("SAVANT_DISABLE_EMBEDDINGS")
        .map(|v| v == "1")
        .unwrap_or(false)
    {
        warn!("SAVANT_DISABLE_EMBEDDINGS=1 — embedding service disabled, using null provider");
        return Ok(Box::new(NullEmbeddingProvider));
    }

    let url = std::env::var("OLLAMA_URL").unwrap_or_else(|_| DEFAULT_URL.to_string());
    let model = model_override
        .map(|s| s.to_string())
        .or_else(|| std::env::var("OLLAMA_EMBED_MODEL").ok())
        .unwrap_or_else(|| DEFAULT_MODEL.to_string());
    let client = crate::net::secure_client_fallible()?;

    // Step 1: Check if Ollama is already running
    let ollama_running = match client.get(format!("{}/api/tags", url)).send().await {
        Ok(resp) => resp.status().is_success(),
        Err(_) => false,
    };

    if !ollama_running {
        // Step 2: Try to auto-start Ollama
        warn!("Ollama not available at {}. Attempting auto-start...", url);
        match auto_start_ollama().await {
            Ok(()) => info!("Ollama auto-started successfully"),
            Err(e) => {
                error!(
                    "Ollama auto-start failed: {}. Falling back to fastembed.",
                    e
                );
                return create_fastembed_fallback();
            }
        }
    }

    // Step 3: Ensure the embedding model is available
    match ensure_model(&client, &url, &model).await {
        Ok(()) => {}
        Err(e) => {
            warn!(
                "Ollama model check failed: {}. Falling back to fastembed.",
                e
            );
            return create_fastembed_fallback();
        }
    }

    // Step 4: Test the embedding service
    let ollama = OllamaEmbeddingService::with_config(&url, &model)?;
    // Verify it works by embedding a test string
    match ollama.embed("test").await {
        Ok(embedding) => {
            let dims = embedding.len();
            info!(
                "Ollama embedding service initialized (model={}, dims={})",
                model, dims
            );
            Ok(Box::new(ollama))
        }
        Err(e) => {
            warn!(
                "Ollama embedding test failed: {}. Falling back to fastembed.",
                e
            );
            create_fastembed_fallback()
        }
    }
}

/// Creates a fallback embedding service when Ollama is unavailable.
/// If SAVANT_DISABLE_EMBEDDINGS=1, returns a null provider (degraded mode).
/// Otherwise returns an error.
fn create_fastembed_fallback() -> Result<Box<dyn EmbeddingProvider>, SavantError> {
    if std::env::var("SAVANT_DISABLE_EMBEDDINGS")
        .map(|v| v == "1")
        .unwrap_or(false)
    {
        warn!("Ollama unavailable but SAVANT_DISABLE_EMBEDDINGS=1 — using null embedding provider");
        return Ok(Box::new(NullEmbeddingProvider));
    }
    Err(SavantError::Unknown(
        "Embedding service unavailable: Ollama is not running and fastembed fallback \
         is not yet integrated. Install Ollama from https://ollama.com/download \
         or set SAVANT_DISABLE_EMBEDDINGS=1 to skip embedding features."
            .to_string(),
    ))
}
