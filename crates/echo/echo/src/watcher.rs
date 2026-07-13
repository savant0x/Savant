//! ECHO Configuration Watcher
//!
//! Glues the pipeline together by listening for workspace changes,
//! triggering compilation, and performing atomic hot-swaps.

use notify::RecursiveMode;
use notify_debouncer_mini::{new_debouncer, DebouncedEvent};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tracing::{error, info};

use crate::compiler::EchoCompiler;
use crate::registry::HotSwappableRegistry;

/// Default channel capacity for the ECHO watcher.
pub const DEFAULT_CHANNEL_CAPACITY: usize = 100;

/// Spawns the ECHO watcher pipeline.
pub async fn spawn_echo_watcher(
    workspace_path: PathBuf,
    registry: Arc<HotSwappableRegistry>,
    compiler: Arc<EchoCompiler>,
) -> Result<(), savant_core::error::SavantError> {
    spawn_echo_watcher_with_capacity(workspace_path, registry, compiler, DEFAULT_CHANNEL_CAPACITY)
        .await
}

/// Spawns the ECHO watcher pipeline with a configurable channel capacity.
pub async fn spawn_echo_watcher_with_capacity(
    workspace_path: PathBuf,
    registry: Arc<HotSwappableRegistry>,
    compiler: Arc<EchoCompiler>,
    channel_capacity: usize,
) -> Result<(), savant_core::error::SavantError> {
    let (tx, mut rx) = mpsc::channel(channel_capacity);

    // Run the blocking `notify` watcher in a dedicated thread
    let workspace_path_thread = workspace_path.clone();

    // We attempt to initialize the debouncer before spawning the thread to catch errors early
    let tx_clone = tx.clone();
    let mut debouncer = new_debouncer(
        Duration::from_millis(500),
        move |res: Result<Vec<DebouncedEvent>, _>| {
            if let Ok(events) = res {
                for event in events {
                    if let Err(e) = tx_clone.blocking_send(event.path) {
                        tracing::warn!("[echo::watcher] Failed to send debounced event: {}", e);
                    }
                }
            }
        },
    )
    .map_err(|e| {
        savant_core::error::SavantError::Unknown(format!("Failed to create ECHO debouncer: {}", e))
    })?;

    debouncer
        .watcher()
        .watch(Path::new(&workspace_path_thread), RecursiveMode::Recursive)
        .map_err(|e| {
            savant_core::error::SavantError::Unknown(format!(
                "ECHO failed to watch workspace: {}",
                e
            ))
        })?;

    // Keep the debouncer alive in a dedicated thread.
    // Uses a oneshot channel receiver to block indefinitely without CPU waste.
    let (_keep_alive_tx, keep_alive_rx) = std::sync::mpsc::channel::<()>();
    std::thread::spawn(move || {
        let _debouncer = debouncer;
        // Block indefinitely — debouncer is kept alive as long as this thread runs.
        // The channel receiver blocks without consuming CPU (unlike sleep loops).
        if keep_alive_rx.recv().is_err() {
            tracing::debug!("[echo::watcher] keep-alive channel closed");
        }
    });

    // Async receiver loop handling the actual compilation and hot-swapping
    tokio::spawn(async move {
        while let Some(path) = rx.recv().await {
            // Check if the modified file is a "trigger" file (e.g., manifest.json)
            if let Some(filename) = path.file_name() {
                if filename == "manifest.json" {
                    info!(
                        "ECHO detected configuration update at {:?}. Initiating pipeline.",
                        path
                    );

                    // Extract the tool name/directory from the path
                    // We assume project structure: workspace/tool_name/manifest.json
                    if let Some(parent) = path.parent() {
                        if let Some(tool_name) = parent.file_name().and_then(|n| n.to_str()) {
                            let tool_dir = tool_name; // Relative to workspace_root

                            // 1. Compile the tool securely
                            match compiler.compile_to_wasm(tool_dir).await {
                                Ok(wasm_bytes) => {
                                    // 2. Perform Lock-Free Hot-Swap
                                    if let Err(e) =
                                        registry.hot_load_component(tool_name, wasm_bytes)
                                    {
                                        error!(
                                            "Failed to hot-swap component '{}': {}",
                                            tool_name, e
                                        );
                                    }
                                }
                                Err(e) => {
                                    error!("ECHO Compilation aborted for '{}': {}", tool_name, e)
                                }
                            }
                        }
                    }
                }
            }
        }
    });

    Ok(())
}
