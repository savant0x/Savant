use async_trait::async_trait;
use savant_core::error::SavantError;
use savant_core::types::{CapabilityGrants, ExecutionMode};
use serde_json::Value;

pub mod native;
pub mod wasm;

/// Trait for executing a skill in a sandboxed environment.
#[async_trait]
pub trait ToolExecutor: Send + Sync {
    /// Executes the tool with the provided JSON arguments.
    /// Returns the standard output or an error describing the failure.
    async fn execute(&self, args: Value) -> Result<String, SavantError>;
}

/// Dispatches execution to the appropriate sandbox engine.
pub struct SandboxDispatcher;

impl SandboxDispatcher {
    /// Creates a boxed ToolExecutor based on the execution mode.
    ///
    /// Routes to:
    /// - `WasmComponent` → WASM executor (wasmtime-based)
    /// - `LegacyNative` → Native executor (Landlock-sandboxed)
    /// - `DockerContainer` → Docker executor (bollard-based, full isolation)
    pub fn create_executor(
        mode: &ExecutionMode,
        workspace_dir: std::path::PathBuf,
        capabilities: CapabilityGrants,
    ) -> Box<dyn ToolExecutor> {
        match mode {
            ExecutionMode::WasmComponent(url) => {
                Box::new(wasm::WassetteExecutor::new(url.clone(), workspace_dir))
            }
            ExecutionMode::LegacyNative(script) => Box::new(native::LegacyNativeExecutor::new(
                script.clone(),
                workspace_dir,
                capabilities,
            )),
            ExecutionMode::DockerContainer(image) => {
                match crate::docker::DockerToolExecutor::new(image.clone()) {
                    Ok(executor) => Box::new(executor),
                    Err(e) => {
                        tracing::error!(
                            "Failed to create Docker executor for image {}: {}",
                            image,
                            e
                        );
                        // Fall back to a no-op executor that returns the error
                        Box::new(FallbackExecutor {
                            error: format!("Docker executor init failed: {}", e),
                        })
                    }
                }
            }
            ExecutionMode::NixFlake(flake_ref) => {
                match crate::nix::NixSkillExecutor::new(flake_ref.clone()) {
                    Ok(executor) => Box::new(ToolExecutorAdapter::new(executor)),
                    Err(e) => {
                        tracing::error!("Failed to create NixSkillExecutor: {}", e);
                        Box::new(FallbackExecutor {
                            error: format!("NixSkillExecutor init failed: {}", e),
                        })
                    }
                }
            }
            ExecutionMode::Lambda(function_name) => {
                // LambdaTool requires tool_name, description, function_name, region
                // For SandboxDispatcher, we use function_name as tool_name and default region
                match crate::lambda::LambdaTool::new(
                    function_name.clone(),
                    format!("AWS Lambda function: {}", function_name),
                    function_name,
                    "us-east-1",
                ) {
                    Ok(executor) => Box::new(ToolExecutorAdapter::new(executor)),
                    Err(e) => {
                        tracing::error!("Failed to create LambdaTool: {}", e);
                        Box::new(FallbackExecutor {
                            error: format!("LambdaTool init failed: {}", e),
                        })
                    }
                }
            }
            ExecutionMode::StandaloneWasm(wasm_bytes) => {
                match crate::wasm::WasmSkillExecutor::new(wasm_bytes) {
                    Ok(executor) => Box::new(ToolExecutorAdapter::new(executor)),
                    Err(e) => {
                        tracing::error!("Failed to create WasmSkillExecutor: {}", e);
                        Box::new(FallbackExecutor {
                            error: format!("WasmSkillExecutor init failed: {}", e),
                        })
                    }
                }
            }
            ExecutionMode::Reference => Box::new(FallbackExecutor {
                error: "ExecutionMode::Reference is documentation-only and cannot be executed"
                    .to_string(),
            }),
        }
    }
}

/// Wrapper to adapt Tool trait implementations to ToolExecutor trait.
/// This allows using types that implement Tool (like NixSkillExecutor, LambdaTool, WasmSkillExecutor)
/// in the SandboxDispatcher which expects ToolExecutor.
struct ToolExecutorAdapter<T: savant_core::traits::Tool> {
    tool: T,
}

impl<T: savant_core::traits::Tool> ToolExecutorAdapter<T> {
    fn new(tool: T) -> Self {
        Self { tool }
    }
}

#[async_trait]
impl<T: savant_core::traits::Tool + Send + Sync> ToolExecutor for ToolExecutorAdapter<T> {
    async fn execute(&self, args: Value) -> Result<String, SavantError> {
        self.tool.execute(args).await
    }
}

/// Fallback executor that returns an error when the primary executor fails to initialize.
struct FallbackExecutor {
    error: String,
}

#[async_trait]
impl ToolExecutor for FallbackExecutor {
    async fn execute(&self, _args: Value) -> Result<String, SavantError> {
        Err(SavantError::Unknown(self.error.clone()))
    }
}
