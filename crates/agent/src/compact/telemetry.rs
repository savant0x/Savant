//! Telemetry — compression event emission to Nexus bus.

use crate::compact::schema::CompactionResult;
use serde::Serialize;
use std::collections::HashMap;

/// Structured compression event emitted to the Nexus bus.
#[derive(Debug, Clone, Serialize)]
pub struct CompressionEvent {
    /// Rule ID that was applied.
    pub rule_id: String,
    /// Tool family classification.
    pub family: String,
    /// Tool name.
    pub tool_name: String,
    /// Original byte count.
    pub original_bytes: usize,
    /// Compressed byte count.
    pub compressed_bytes: usize,
    /// Compression ratio (compressed / original).
    pub ratio: f32,
    /// Processing time in microseconds.
    pub processing_us: u64,
    /// Named counter values extracted from output.
    pub counters: HashMap<String, usize>,
    /// Whether the output was truncated.
    pub was_truncated: bool,
    /// Timestamp.
    pub timestamp: i64,
}

impl From<&CompactionResult> for CompressionEvent {
    fn from(result: &CompactionResult) -> Self {
        Self {
            rule_id: result.rule_id.clone(),
            family: "unknown".to_string(),
            tool_name: String::new(),
            original_bytes: result.original_bytes,
            compressed_bytes: result.compressed_bytes,
            ratio: result.ratio,
            processing_us: result.processing_us,
            counters: result.counters.clone(),
            was_truncated: result.was_truncated,
            timestamp: savant_core::utils::time::now_millis().unwrap_or(0) as i64,
        }
    }
}

impl CompressionEvent {
    /// Creates a new event with tool context.
    pub fn with_context(result: &CompactionResult, tool_name: &str, family: &str) -> Self {
        let mut event = Self::from(result);
        event.tool_name = tool_name.to_string();
        event.family = family.to_string();
        event
    }
}

/// Emits a compression event to the Nexus bus.
#[cfg(feature = "nexus")]
pub async fn emit_event(nexus: &savant_core::bus::NexusBridge, event: &CompressionEvent) {
    let payload = match serde_json::to_string(event) {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!("[telemetry] Failed to serialize compression event: {}", e);
            return;
        }
    };
    if let Err(e) = nexus.publish("system.compact.compression", &payload).await {
        tracing::warn!("[telemetry] Failed to publish compression event: {}", e);
    }
}

/// Fallback: write to local JSONL file when nexus feature is not enabled.
#[cfg(not(feature = "nexus"))]
pub async fn emit_event(_nexus: &savant_core::bus::NexusBridge, event: &CompressionEvent) {
    let telemetry_dir = std::path::PathBuf::from("data/telemetry");
    if let Err(e) = savant_core::utils::io::ensure_dir(&telemetry_dir).await {
        tracing::warn!("[telemetry] Failed to create telemetry directory: {}", e);
        return;
    }
    let file_path = telemetry_dir.join("compact_events.jsonl");
    let payload = match serde_json::to_string(event) {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!("[telemetry] Failed to serialize compression event: {}", e);
            return;
        }
    };
    use std::io::Write;
    match std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&file_path)
    {
        Ok(mut file) => {
            if let Err(e) = writeln!(file, "{}", payload) {
                tracing::warn!("[telemetry] Failed to write telemetry event: {}", e);
            }
        }
        Err(e) => {
            tracing::warn!("[telemetry] Failed to open telemetry file: {}", e);
        }
    }
}
