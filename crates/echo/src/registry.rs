//! Zero-Downtime Epoch-Based Tool Registry
//!
//! Utilizes `ArcSwap` to provide lock-free reads on the hot path while allowing
//! the ECHO subsystem to atomically swap in newly compiled WASM components.

use arc_swap::ArcSwap;
use std::collections::HashMap;
use std::sync::Arc;
use tracing::{info, instrument};
use wasmtime::component::Component;
use wasmtime::Engine;

/// Represents an executable capability within the Swarm
pub struct WasmCapability {
    /// Public name of the tool
    pub name: String,
    /// Monotonically increasing version for this specific tool
    pub version: u64,
    /// Pre-JIT compiled Wasmtime Component
    pub module: Component,
    /// Raw WASM bytes for persistence and recovery
    pub raw_bytes: Vec<u8>,
}

/// The immutable routing table for a specific Epoch in time.
#[derive(Default, Clone)]
pub struct RegistryEpoch {
    /// O(1) lookup map for active tools
    pub tools: HashMap<String, Arc<WasmCapability>>,
    /// The monotonically increasing version of the entire Swarm state
    pub epoch_id: u64,
}

/// A highly-scalable, wait-free tool registry.
pub struct HotSwappableRegistry {
    /// The core `arc-swap` pointer. Reads are wait-free and perfectly scalable.
    active_state: ArcSwap<RegistryEpoch>,
    /// We keep the previous epoch in memory to allow instant Circuit Breaker rollbacks.
    previous_state: ArcSwap<Option<Arc<RegistryEpoch>>>,
    /// Shared Wasmtime engine to prevent recompilation overhead
    engine: Engine,
}

impl HotSwappableRegistry {
    /// Creates a new registry with the provided Wasmtime engine.
    pub fn new(engine: Engine) -> Self {
        Self {
            active_state: ArcSwap::from_pointee(RegistryEpoch::default()),
            previous_state: ArcSwap::from_pointee(None),
            engine,
        }
    }

    /// Retrieves a tool from the current epoch.
    ///
    /// This is wait-free and highly scalable. Returns a strong Arc reference
    /// so the caller can safely execute the tool even if a hot-swap occurs.
    #[inline]
    pub fn get_tool(&self, tool_name: &str) -> Option<Arc<WasmCapability>> {
        let guard = self.active_state.load();
        guard.tools.get(tool_name).cloned()
    }

    /// Atomically injects a new WASM component into the registry.
    ///
    /// 1. Performs JIT compilation off the hot path.
    /// 2. Clones the existing routing table.
    /// 3. Injects the new capability.
    /// 4. Performs an atomic pointer swap.
    #[instrument(skip(self, new_tool_bytes))]
    pub fn hot_load_component(
        &self,
        tool_name: &str,
        new_tool_bytes: Vec<u8>,
    ) -> Result<(), String> {
        info!("JIT Compiling new ECHO component: {}", tool_name);

        let new_component = Component::new(&self.engine, &new_tool_bytes)
            .map_err(|e| format!("WASM compilation failed: {}", e))?;

        let current_guard = self.active_state.load();

        let new_capability = Arc::new(WasmCapability {
            name: tool_name.to_string(),
            version: current_guard.epoch_id + 1,
            module: new_component,
            raw_bytes: new_tool_bytes,
        });

        let mut new_epoch_map = current_guard.tools.clone();
        new_epoch_map.insert(tool_name.to_string(), new_capability);

        let new_epoch = Arc::new(RegistryEpoch {
            tools: new_epoch_map,
            epoch_id: current_guard.epoch_id + 1,
        });

        // Store for potential rollback
        self.previous_state
            .store(Arc::new(Some(current_guard.clone())));
        // The Atomic Swap
        self.active_state.store(new_epoch);

        info!(
            "Hot-swap complete for '{}'. Swarm Epoch advanced to {}.",
            tool_name,
            current_guard.epoch_id + 1
        );
        Ok(())
    }

    /// Rollback to the previous epoch (Statistical Circuit Breaker).
    pub fn rollback_epoch(&self) -> Result<(), &'static str> {
        let prev = self.previous_state.load();
        if let Some(rollback_state) = prev.as_ref() {
            let old_epoch = rollback_state.epoch_id;
            self.active_state.store(rollback_state.clone());
            self.previous_state.store(Arc::new(None));
            info!(
                "CRITICAL: Circuit breaker triggered. Rolled back to Epoch {}.",
                old_epoch
            );
            Ok(())
        } else {
            Err("No previous epoch available for rollback")
        }
    }

    /// Returns the current epoch ID.
    pub fn current_epoch(&self) -> u64 {
        self.active_state.load().epoch_id
    }
}
