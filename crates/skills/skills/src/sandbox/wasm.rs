use super::ToolExecutor;
use async_trait::async_trait;
use savant_core::error::SavantError;
use serde_json::Value;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::{debug, info, warn};

/// PB-13: Maximum WASM output size in characters.
const MAX_WASM_OUTPUT: usize = 100_000;

/// High-performance WebAssembly executor using Wassette.
/// Fetches and executes Wasm Components from OCI registries with MCP integration.
/// Provides browser-grade isolation and capability-based security.
pub struct WassetteExecutor {
    component_url: String,
    component_dir: PathBuf,
    /// Lazy-initialized LifecycleManager for this executor
    manager: Arc<Mutex<Option<wassette::LifecycleManager>>>,
    /// Cached component ID after loading
    component_id: Arc<Mutex<Option<String>>>,
    /// Cached function name (tool name) to invoke
    function_name: Arc<Mutex<Option<String>>>,
}

impl WassetteExecutor {
    /// Creates a new WassetteExecutor for the given component URL and workspace directory.
    /// Components will be stored in `.wassette_components` subdirectory of the workspace.
    pub fn new(component_url: String, workspace_dir: PathBuf) -> Self {
        let component_dir = workspace_dir.join(".wassette_components");
        Self {
            component_url,
            component_dir,
            manager: Arc::new(Mutex::new(None)),
            component_id: Arc::new(Mutex::new(None)),
            function_name: Arc::new(Mutex::new(None)),
        }
    }

    /// Lazily initializes the Wassette LifecycleManager.
    async fn ensure_manager(&self) -> Result<wassette::LifecycleManager, SavantError> {
        // Check if already initialized
        {
            let mgr_guard = self.manager.lock().await;
            if let Some(mgr) = mgr_guard.as_ref() {
                return Ok(mgr.clone());
            }
        }

        // Not initialized yet - create a new LifecycleManager
        info!(
            "Wassette: Creating LifecycleManager in {:?}",
            self.component_dir
        );
        let manager = wassette::LifecycleManager::new_unloaded(&self.component_dir)
            .await
            .map_err(|e| {
                SavantError::Unknown(format!("Failed to create LifecycleManager: {}", e))
            })?;

        // Cache for reuse
        let mut mgr_guard = self.manager.lock().await;
        *mgr_guard = Some(manager.clone());
        Ok(manager)
    }

    /// Ensures the component is loaded and returns its component ID.
    async fn ensure_component_loaded(
        &self,
        manager: &wassette::LifecycleManager,
    ) -> Result<String, SavantError> {
        // Check cache first
        {
            let cid_guard = self.component_id.lock().await;
            if let Some(cid) = cid_guard.as_ref() {
                return Ok(cid.clone());
            }
        }

        // Load the component from the OCI URL or file path
        info!("Wassette: Loading component from {}", self.component_url);
        let outcome = manager
            .load_component(&self.component_url)
            .await
            .map_err(|e| {
                SavantError::Unknown(format!(
                    "Failed to load component {}: {}",
                    self.component_url, e
                ))
            })?;

        // SHA-256 hash verification: check for manifest.json with content_hash
        let manifest_path = self
            .component_dir
            .join(&outcome.component_id)
            .join("manifest.json");
        if manifest_path.exists() {
            let manifest_bytes = std::fs::read(&manifest_path).map_err(|e| {
                SavantError::Unknown(format!("Failed to read component manifest: {}", e))
            })?;
            let manifest: serde_json::Value =
                serde_json::from_slice(&manifest_bytes).map_err(|e| {
                    SavantError::Unknown(format!("Failed to parse component manifest: {}", e))
                })?;

            if let Some(expected_hash) = manifest.get("content_hash").and_then(|h| h.as_str()) {
                // Find and hash the WASM file in the component directory
                let component_dir = self.component_dir.join(&outcome.component_id);
                let wasm_file = component_dir.join("component.wasm");
                if wasm_file.exists() {
                    let wasm_bytes = std::fs::read(&wasm_file).map_err(|e| {
                        SavantError::Unknown(format!("Failed to read WASM file: {}", e))
                    })?;
                    use sha2::{Digest, Sha256};
                    let actual_hash = hex::encode(Sha256::digest(&wasm_bytes));
                    if actual_hash != expected_hash {
                        return Err(SavantError::Unknown(format!(
                            "Component hash mismatch for {}: expected {}, got {}",
                            self.component_url, expected_hash, actual_hash
                        )));
                    }
                    debug!("Wassette: Component hash verified: {}", actual_hash);
                }
            } else {
                return Err(SavantError::Unknown(format!(
                    "Component manifest missing required content_hash field for {}",
                    self.component_url
                )));
            }
        }

        let component_id = outcome.component_id;
        debug!("Wassette: Component loaded with ID: {}", component_id);

        // Cache the component ID
        let mut cid_guard = self.component_id.lock().await;
        *cid_guard = Some(component_id.clone());
        Ok(component_id)
    }

    /// Discovers and caches the function name to invoke within the component.
    /// Defaults to the first tool exported by the component.
    async fn ensure_function_name(
        &self,
        manager: &wassette::LifecycleManager,
        component_id: &str,
    ) -> Result<String, SavantError> {
        // Check cache first
        {
            let fn_guard = self.function_name.lock().await;
            if let Some(fn_name) = fn_guard.as_ref() {
                return Ok(fn_name.clone());
            }
        }

        // Get the tool schema for this component
        if let Some(schema) = manager.get_component_schema(component_id).await {
            if let Some(tools) = schema.get("tools").and_then(|t| t.as_array()) {
                if let Some(first_tool) = tools.first() {
                    if let Some(name) = first_tool.get("name").and_then(|n| n.as_str()) {
                        let name_str = name.to_string();
                        debug!(
                            "Wassette: Discovered tool name '{}' for component {}",
                            name_str, component_id
                        );
                        let mut fn_guard = self.function_name.lock().await;
                        *fn_guard = Some(name_str.clone());
                        return Ok(name_str);
                    }
                }
            }
        }

        // If we couldn't discover a tool name, fallback to component_id
        warn!("Wassette: Could not discover tool name for component {}, using component ID as function name", component_id);
        let fallback = component_id.to_string();
        let mut fn_guard = self.function_name.lock().await;
        *fn_guard = Some(fallback.clone());
        Ok(fallback)
    }
}

#[async_trait]
impl ToolExecutor for WassetteExecutor {
    async fn execute(&self, args: Value) -> Result<String, SavantError> {
        // Step 1: Initialize manager (lazy)
        let manager = self.ensure_manager().await?;

        // Step 2: Load component if needed
        let component_id = self.ensure_component_loaded(&manager).await?;

        // Step 3: Discover function name (tool name) to invoke
        let function_name = self.ensure_function_name(&manager, &component_id).await?;

        // Step 4: Execute the component call
        let args_str = args.to_string();
        debug!(component = %component_id, function = %function_name, "Wassette: Invoking function");

        let result = manager
            .execute_component_call(&component_id, &function_name, &args_str)
            .await
            .map_err(|e| SavantError::Unknown(format!("WASM execution failed: {}", e)))?;

        // PB-13: Truncate WASM output to prevent unbounded memory usage
        if result.len() > MAX_WASM_OUTPUT {
            warn!(
                "WASM output exceeded {} chars, truncating from {}",
                MAX_WASM_OUTPUT,
                result.len()
            );
            let mut end = MAX_WASM_OUTPUT;
            while end > 0 && !result.is_char_boundary(end) {
                end -= 1;
            }
            Ok(format!(
                "{}\n\n[... truncated at {} chars]",
                &result[..end],
                MAX_WASM_OUTPUT
            ))
        } else {
            Ok(result)
        }
    }
}
