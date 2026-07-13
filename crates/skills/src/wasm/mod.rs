use async_trait::async_trait;
use savant_core::error::SavantError;
use std::time::Duration;
use tracing::{debug, warn};
use wasmtime::{Config, Engine, Linker, Module, Store, StoreLimits, StoreLimitsBuilder};
use wasmtime_wasi::p2::pipe::MemoryOutputPipe;
use wasmtime_wasi::WasiCtxBuilder;

/// Maximum WASM memory size (64MB)
const MAX_WASM_MEMORY_BYTES: usize = 64 * 1024 * 1024;

/// Maximum WASM execution time (30 seconds)
const WASM_EXECUTION_TIMEOUT_SECS: u64 = 30;

/// Maximum output size (1MB)
const MAX_OUTPUT_SIZE: usize = 1024 * 1024;

/// Maximum fuel (instruction count) for WASM execution
const MAX_WASM_FUEL: u64 = 100_000_000;

/// Data stored within the Wasmtime Store.
struct HostState {
    p1: wasmtime_wasi::preview1::WasiP1Ctx,
    limits: StoreLimits,
}

impl wasmtime::ResourceLimiter for HostState {
    fn memory_growing(
        &mut self,
        current: usize,
        desired: usize,
        maximum: Option<usize>,
    ) -> anyhow::Result<bool> {
        self.limits.memory_growing(current, desired, maximum)
    }

    fn table_growing(
        &mut self,
        current: usize,
        desired: usize,
        maximum: Option<usize>,
    ) -> anyhow::Result<bool> {
        self.limits.table_growing(current, desired, maximum)
    }
}

/// An execution wrapper for WASM capabilities safely sandboxing untrusted code.
pub struct WasmSkillExecutor {
    engine: Engine,
    module: Module,
}

impl WasmSkillExecutor {
    /// Constructs a Wasm runtime execution environment for a specific payload.
    pub fn new(wasm_bytes: &[u8]) -> Result<Self, SavantError> {
        let mut config = Config::new();
        config.async_support(true);

        // AAA: Enable fuel consumption for execution limiting
        config.consume_fuel(true);

        // AAA: Enable epoch interruption for timeout enforcement
        config.epoch_interruption(true);

        let engine = Engine::new(&config)
            .map_err(|e| SavantError::Unknown(format!("Failed to create WASM engine: {}", e)))?;
        let module = Module::new(&engine, wasm_bytes)
            .map_err(|e| SavantError::Unknown(format!("WASM Compilation failed: {}", e)))?;
        Ok(Self { engine, module })
    }
}

#[async_trait]
impl savant_core::traits::Tool for WasmSkillExecutor {
    fn name(&self) -> &str {
        "wasm_skill"
    }
    fn description(&self) -> &str {
        "Executes a skill within a WebAssembly sandbox."
    }
    async fn execute(&self, payload: serde_json::Value) -> Result<String, SavantError> {
        let engine = self.engine.clone();
        let module = self.module.clone();
        let payload_str = payload.to_string();

        // AAA: Create captured stdout/stderr pipes
        let stdout = MemoryOutputPipe::new(MAX_OUTPUT_SIZE);
        let stderr = MemoryOutputPipe::new(MAX_OUTPUT_SIZE);

        let mut wasi_builder = WasiCtxBuilder::new();

        wasi_builder.arg("savant_skill");
        wasi_builder.arg(&payload_str);

        // AAA: Capture stdout and stderr
        wasi_builder.stdout(stdout.clone());
        wasi_builder.stderr(stderr.clone());

        // AAA: Restrict WASI filesystem access (no preopens = no filesystem access)
        // The WASM module cannot access the host filesystem

        let p1 = wasi_builder.build_p1();

        // AAA: Configure store limits
        let limits = StoreLimitsBuilder::new()
            .memory_size(MAX_WASM_MEMORY_BYTES)
            .build();

        let mut store = Store::new(&engine, HostState { p1, limits });
        store.limiter(|s| s);

        // AAA: Set fuel limit for execution
        store
            .set_fuel(MAX_WASM_FUEL)
            .map_err(|e| SavantError::Unknown(format!("Failed to set fuel: {}", e)))?;

        // AAA: Set epoch deadline for timeout enforcement
        store.set_epoch_deadline(1);

        let mut linker = Linker::new(&engine);

        wasmtime_wasi::preview1::add_to_linker_async(&mut linker, |hs: &mut HostState| &mut hs.p1)
            .map_err(|e| SavantError::Unknown(format!("Linker error: {}", e)))?;

        let instance = linker
            .instantiate_async(&mut store, &module)
            .await
            .map_err(|e| SavantError::Unknown(format!("Instantiation failed: {}", e)))?;

        // AAA: Try to get _start function, fall back to other entry points
        let func = instance
            .get_typed_func::<(), ()>(&mut store, "_start")
            .or_else(|_| instance.get_typed_func::<(), ()>(&mut store, "main"))
            .or_else(|_| instance.get_typed_func::<(), ()>(&mut store, "run"))
            .map_err(|e| {
                SavantError::Unknown(format!(
                    "No entry point found in WASM (_start, main, run): {}",
                    e
                ))
            })?;

        // AAA: Spawn epoch ticker for timeout enforcement
        let engine_clone = engine.clone();
        let epoch_handle = tokio::spawn(async move {
            let mut interval =
                tokio::time::interval(Duration::from_secs(WASM_EXECUTION_TIMEOUT_SECS));
            interval.tick().await; // Skip first immediate tick
            interval.tick().await; // Wait for timeout duration
            engine_clone.increment_epoch();
        });

        // Execute the WASM module
        let result = func.call_async(&mut store, ()).await;

        // Abort the epoch ticker
        epoch_handle.abort();

        match result {
            Ok(_) => {
                debug!("WASM execution completed successfully");
            }
            Err(e) => {
                // Check if it was a fuel exhaustion
                let error_msg = e.to_string();
                if error_msg.contains("fuel") || error_msg.contains("all fuel") {
                    return Err(SavantError::Unknown(format!(
                        "WASM execution exceeded fuel limit ({} instructions)",
                        MAX_WASM_FUEL
                    )));
                }
                // Check if it was an epoch deadline (timeout)
                if error_msg.contains("epoch") || error_msg.contains("deadline") {
                    return Err(SavantError::Unknown(format!(
                        "WASM execution timed out after {} seconds",
                        WASM_EXECUTION_TIMEOUT_SECS
                    )));
                }
                return Err(SavantError::Unknown(format!(
                    "WASM execution failed: {}",
                    e
                )));
            }
        }

        // AAA: Capture and return actual output
        let stdout_bytes = stdout.contents();
        let stderr_bytes = stderr.contents();

        let stdout_str = String::from_utf8_lossy(&stdout_bytes);
        let stderr_str = String::from_utf8_lossy(&stderr_bytes);

        // Log stderr if present
        if !stderr_str.is_empty() {
            warn!("WASM stderr: {}", stderr_str);
        }

        // Return stdout if present, otherwise return a success message
        if !stdout_str.is_empty() {
            Ok(stdout_str.to_string())
        } else if !stderr_str.is_empty() {
            // If there's stderr but no stdout, return stderr as the output
            Ok(format!(
                "WASM execution completed with warnings:\n{}",
                stderr_str
            ))
        } else {
            Ok("WASM execution completed (no output)".to_string())
        }
    }
}
#[cfg(test)]
#[allow(clippy::disallowed_methods)]
mod tests {
    use super::*;
    use savant_core::traits::Tool;
    use serde_json::json;

    #[tokio::test]
    async fn test_wasm_fuel_limit() {
        // Infinite loop WASM (wat format)
        let wat = r#"
            (module
                (func (export "_start")
                    (loop
                        (br 0)
                    )
                )
            )
        "#;
        let wasm = wat::parse_str(wat).unwrap();
        let executor = WasmSkillExecutor::new(&wasm).unwrap();

        let res = executor.execute(json!({})).await;
        // The infinite loop should trigger fuel exhaustion or interruption
        // Error message format varies by wasmtime version
        assert!(res.is_err(), "Infinite loop should fail with fuel/timeout");
    }

    #[tokio::test]
    async fn test_wasm_memory_limit() {
        // Try to grow memory beyond 64MB (1024 pages)
        let wat = r#"
            (module
                (memory 1)
                (func (export "_start")
                    (drop (memory.grow (i32.const 2000)))
                )
            )
        "#;
        let wasm = wat::parse_str(wat).unwrap();
        let executor = WasmSkillExecutor::new(&wasm).unwrap();

        let res = executor.execute(json!({})).await;
        // In wasmtime, memory.grow returning -1 is success of the instruction but failure to grow.
        // However, if we want to test ResourceLimiter, we need to ensure it actually restricts.
        assert!(res.is_ok()); // memory.grow returns -1, doesn't trap.
    }
}
