//! Integration layer — wires Compact into the agent's tool execution pipeline.

use crate::compact::engine::CompactEngine;
use crate::compact::schema::{CompactionResult, ToolOutput};
use crate::compact::telemetry;
use savant_core::bus::NexusBridge;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Global compact engine instance (lazy-initialized).
static ENGINE: once_cell::sync::Lazy<Arc<RwLock<Option<CompactEngine>>>> =
    once_cell::sync::Lazy::new(|| Arc::new(RwLock::new(None)));

/// Global NexusBridge for telemetry emission (lazy-initialized, zero-arg constructor).
static NEXUS: once_cell::sync::Lazy<Arc<NexusBridge>> =
    once_cell::sync::Lazy::new(|| Arc::new(NexusBridge::new()));

/// Initializes the global compact engine.
pub async fn init(user_rules_dir: PathBuf, project_rules_dir: PathBuf) {
    let engine = CompactEngine::new(user_rules_dir, project_rules_dir);
    let mut guard = ENGINE.write().await;
    *guard = Some(engine);
    tracing::info!("[compact] Global engine initialized");
}

/// Compacts a tool output using the global engine.
/// Falls back to passthrough if the engine is not initialized.
/// NA-01: Emits a telemetry event after each compaction.
pub async fn compact_output(
    tool_name: &str,
    argv: &[String],
    exit_code: i32,
    raw_output: &str,
    working_dir: Option<&str>,
) -> CompactionResult {
    let guard = ENGINE.read().await;
    match guard.as_ref() {
        Some(engine) => {
            let output = ToolOutput {
                tool_name: tool_name.to_string(),
                argv: argv.to_vec(),
                exit_code,
                raw_output: raw_output.to_string(),
                working_dir: working_dir.map(String::from),
            };
            let (result, event) = engine.compact_with_telemetry(&output);
            // NA-01: Emit compression telemetry via dedicated emit_event function
            if event.ratio < 1.0 {
                tracing::debug!(
                    tool_patterns_count = engine.tool_patterns().len(),
                    tool_patterns = ?engine.tool_patterns(),
                    "Compact engine diagnostic: active tool patterns"
                );
                telemetry::emit_event(&NEXUS, &event).await;
            }
            result
        }
        None => CompactionResult::passthrough(raw_output),
    }
}

/// Reloads rules from disk (hot-reload).
pub async fn reload_rules() {
    let mut guard = ENGINE.write().await;
    if let Some(engine) = guard.as_mut() {
        engine.reload();
    }
}

/// Returns the number of registered rules.
pub async fn rule_count() -> usize {
    let guard = ENGINE.read().await;
    guard.as_ref().map_or(0, |e| e.rule_count())
}

/// Returns true if the compact engine has been initialized.
/// Safe to call from any context (sync or async).
pub fn is_engine_initialized() -> bool {
    if let Ok(guard) = ENGINE.try_read() {
        guard.is_some()
    } else {
        false
    }
}

/// Synchronous version for calling from non-async contexts (e.g., reactor).
/// NA-01: Emits a telemetry event after each compaction.
pub fn compact_output_sync(
    tool_name: &str,
    args: &str,
    exit_code: i32,
    raw_output: &str,
    working_dir: Option<&str>,
) -> CompactionResult {
    if let Ok(_rt) = tokio::runtime::Handle::try_current() {
        if let Ok(guard) = ENGINE.try_read() {
            if let Some(engine) = guard.as_ref() {
                let argv: Vec<String> = parse_argv_from_args(args);
                let output = ToolOutput {
                    tool_name: tool_name.to_string(),
                    argv,
                    exit_code,
                    raw_output: raw_output.to_string(),
                    working_dir: working_dir.map(String::from),
                };
                let (result, event) = engine.compact_with_telemetry(&output);
                // NA-01: Emit compression telemetry via dedicated emit_event function
                if event.ratio < 1.0 {
                    let nexus = Arc::clone(&NEXUS);
                    tokio::spawn(async move {
                        telemetry::emit_event(&nexus, &event).await;
                    });
                }
                return result;
            }
        }
    }
    if let Ok(rt) = tokio::runtime::Handle::try_current() {
        let result = rt.block_on(async {
            compact_output(
                tool_name,
                &parse_argv_from_args(args),
                exit_code,
                raw_output,
                working_dir,
            )
            .await
        });
        return result;
    }
    CompactionResult::passthrough(raw_output)
}

pub fn parse_argv_from_args(args: &str) -> Vec<String> {
    if let Ok(val) = serde_json::from_str::<serde_json::Value>(args) {
        if let Some(arr) = val.as_array() {
            return arr
                .iter()
                .filter_map(|v| v.as_str().map(String::from))
                .collect();
        }
        if let Some(obj) = val.as_object() {
            if let Some(payload) = obj.get("payload").and_then(|v| v.as_str()) {
                return vec![payload.to_string()];
            }
            return obj
                .values()
                .filter_map(|v| v.as_str().map(String::from))
                .collect();
        }
    }
    if args.is_empty() {
        vec![]
    } else {
        vec![args.to_string()]
    }
}
