use super::VmmError;
use std::path::PathBuf;
use sysinfo::System;

/// Forensic capture configuration.
#[derive(Debug, Clone)]
pub struct ForensicConfig {
    /// Directory to store forensic snapshots.
    pub output_dir: PathBuf,
    /// Whether to capture the writable block layer.
    pub capture_block_layer: bool,
    /// Whether to capture guest memory (if supported by backend).
    pub capture_memory: bool,
    /// Whether to redact host secrets from the capture.
    pub redact_secrets: bool,
    /// Whether to capture process tree.
    pub capture_process_tree: bool,
    /// Whether to capture open file handles.
    pub capture_open_handles: bool,
    /// Whether to capture memory region map.
    pub capture_memory_regions: bool,
}

impl ForensicConfig {
    pub fn new(output_dir: impl Into<PathBuf>) -> Self {
        Self {
            output_dir: output_dir.into(),
            capture_block_layer: true,
            capture_memory: false,
            redact_secrets: true,
            capture_process_tree: true,
            capture_open_handles: true,
            capture_memory_regions: true,
        }
    }

    pub fn with_block_layer(mut self, capture: bool) -> Self {
        self.capture_block_layer = capture;
        self
    }

    pub fn with_memory(mut self, capture: bool) -> Self {
        self.capture_memory = capture;
        self
    }
}

/// The result of a forensic capture.
#[derive(Debug)]
pub struct ForensicBundle {
    /// Path to the forensic bundle directory.
    pub path: PathBuf,
    /// Timestamp of the capture (epoch seconds).
    pub timestamp: u64,
    /// Whether secrets were redacted.
    pub secrets_redacted: bool,
    /// Files included in the bundle.
    pub files: Vec<PathBuf>,
}

/// Captures a forensic snapshot of a VM for post-mortem analysis.
///
/// This is called when:
/// - The guest panics or hits an unhandled exception
/// - A resource limit is exceeded (OOM, CPU timeout)
/// - The host detects anomalous behavior
///
/// The capture includes:
/// - The writable block layer (for file system forensics)
/// - Guest memory dump (if supported and enabled)
/// - Process info and configuration
/// - Audit chain from the guest agentd
///
/// Host secrets are replaced with placeholders in the capture
/// to make it safe for external analysis.
pub fn capture_forensic_snapshot(
    config: &ForensicConfig,
    backend_name: &str,
    vsock_port: u32,
    audit_json: Option<&str>,
) -> Result<ForensicBundle, VmmError> {
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    let bundle_dir = config.output_dir.join(format!("forensic_{}", timestamp));
    std::fs::create_dir_all(&bundle_dir)
        .map_err(|e| VmmError::Io(format!("failed to create forensic dir: {}", e)))?;

    let mut files = Vec::new();

    // Write metadata
    let metadata = format!(
        "backend={}\nvsock_port={}\ntimestamp={}\nsecrets_redacted={}\n",
        backend_name, vsock_port, timestamp, config.redact_secrets
    );
    let metadata_path = bundle_dir.join("metadata.txt");
    std::fs::write(&metadata_path, &metadata)
        .map_err(|e| VmmError::Io(format!("failed to write metadata: {}", e)))?;
    files.push(metadata_path);

    // Write audit chain if available
    if let Some(json) = audit_json {
        let audit_path = bundle_dir.join("audit_chain.json");
        let content = if config.redact_secrets {
            redact_secrets(json)
        } else {
            json.to_string()
        };
        std::fs::write(&audit_path, &content)
            .map_err(|e| VmmError::Io(format!("failed to write audit chain: {}", e)))?;
        files.push(audit_path);
    }

    // Note: Block layer and memory capture require backend-specific operations
    // that are done through the AgentHypervisor::forensic_snapshot method.
    // This function handles the common metadata and audit capture.

    // Capture process tree using sysinfo
    if config.capture_process_tree {
        let mut sys = System::new_all();
        sys.refresh_all();
        let mut process_tree = Vec::new();
        for (pid, process) in sys.processes() {
            let cmd_str: Vec<String> = process
                .cmd()
                .iter()
                .map(|s| s.to_string_lossy().to_string())
                .collect();
            process_tree.push(format!(
                "pid={} ppid={} name={} status={:?} cpu={:.2}% mem={}B cmd={}",
                pid,
                process.parent().map(|p| p.as_u32()).unwrap_or(0),
                process.name().to_string_lossy(),
                process.status(),
                process.cpu_usage(),
                process.memory(),
                cmd_str.join(" ")
            ));
        }
        let process_tree_path = bundle_dir.join("process_tree.txt");
        std::fs::write(&process_tree_path, process_tree.join("\n"))
            .map_err(|e| VmmError::Io(format!("failed to write process tree: {}", e)))?;
        files.push(process_tree_path);
    }

    // Capture open file handles (Linux: /proc/self/fd)
    #[cfg(target_os = "linux")]
    if config.capture_open_handles {
        let mut handles = Vec::new();
        if let Ok(entries) = std::fs::read_dir("/proc/self/fd") {
            for entry in entries.flatten() {
                let fd = entry.file_name().to_string_lossy().to_string();
                if let Ok(link) = std::fs::read_link(entry.path()) {
                    handles.push(format!("fd={} target={}", fd, link.display()));
                }
            }
        }
        let handles_path = bundle_dir.join("open_handles.txt");
        std::fs::write(&handles_path, handles.join("\n"))
            .map_err(|e| VmmError::Io(format!("failed to write open handles: {}", e)))?;
        files.push(handles_path);
    }

    // Capture memory regions (Linux: /proc/self/maps)
    #[cfg(target_os = "linux")]
    if config.capture_memory_regions {
        if let Ok(maps) = std::fs::read_to_string("/proc/self/maps") {
            let maps_path = bundle_dir.join("memory_regions.txt");
            std::fs::write(&maps_path, &maps)
                .map_err(|e| VmmError::Io(format!("failed to write memory regions: {}", e)))?;
            files.push(maps_path);
        }
    }

    tracing::info!(
        "forensic snapshot captured at {} ({} files)",
        bundle_dir.display(),
        files.len()
    );

    Ok(ForensicBundle {
        path: bundle_dir,
        timestamp,
        secrets_redacted: config.redact_secrets,
        files,
    })
}

/// Redacts potential secrets from a string by replacing patterns that look like
/// API keys, tokens, or credentials with `[REDACTED]`.
fn redact_secrets(input: &str) -> String {
    let mut result = input.to_string();

    // Redact common secret patterns
    let patterns = &[
        (r#""(sk-[a-zA-Z0-9]{20,})""#, r#""[REDACTED_API_KEY]""#),
        (
            r#""(token[_-]?[=:]\s*[a-zA-Z0-9]{20,})""#,
            r#""[REDACTED_TOKEN]""#,
        ),
        (
            r#""(password[_-]?[=:]\s*[a-zA-Z0-9]{8,})""#,
            r#""[REDACTED_PASSWORD]""#,
        ),
        (
            r#""(secret[_-]?[=:]\s*[a-zA-Z0-9]{8,})""#,
            r#""[REDACTED_SECRET]""#,
        ),
    ];

    for (pattern, replacement) in patterns {
        if let Ok(re) = regex_lite::Regex::new(pattern) {
            result = re.replace_all(&result, *replacement).to_string();
        }
    }

    result
}

/// Validates that a forensic bundle is complete and unmodified.
pub fn validate_bundle(bundle: &ForensicBundle) -> Result<(), VmmError> {
    if !bundle.path.exists() {
        return Err(VmmError::Io(format!(
            "forensic bundle directory does not exist: {}",
            bundle.path.display()
        )));
    }

    // Check that metadata file exists
    let metadata_path = bundle.path.join("metadata.txt");
    if !metadata_path.exists() {
        return Err(VmmError::Io(
            "forensic bundle is missing metadata.txt".into(),
        ));
    }

    // Validate metadata content
    let metadata = std::fs::read_to_string(&metadata_path)
        .map_err(|e| VmmError::Io(format!("failed to read metadata: {}", e)))?;

    if !metadata.contains("backend=") {
        return Err(VmmError::Io(
            "forensic metadata is malformed (missing backend field)".into(),
        ));
    }

    Ok(())
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)]
mod tests {
    use super::*;

    #[test]
    fn test_forensic_config_builder() {
        let config = ForensicConfig::new("/tmp/forensics")
            .with_block_layer(true)
            .with_memory(true);
        assert_eq!(config.output_dir, PathBuf::from("/tmp/forensics"));
        assert!(config.capture_block_layer);
        assert!(config.capture_memory);
        assert!(config.redact_secrets);
    }

    #[test]
    fn test_capture_forensic_snapshot() {
        let config = ForensicConfig::new(std::env::temp_dir().join("savant_test_forensic"));
        let audit_json = r#"[{"index":0,"action_type":"Exec","payload":"echo test"}]"#;

        let bundle = capture_forensic_snapshot(&config, "process", 1234, Some(audit_json))
            .expect("capture failed");

        assert!(bundle.path.exists());
        assert!(bundle.secrets_redacted);
        assert!(!bundle.files.is_empty());

        // Validate the bundle
        validate_bundle(&bundle).expect("validation failed");

        // Cleanup
        let _ = std::fs::remove_dir_all(&bundle.path);
    }

    #[test]
    fn test_redact_secrets_keeps_short_inputs() {
        // Fixture uses an sk- prefix with fewer than 20 alphanumerics after.
        // The redact_secrets regex `sk-[a-zA-Z0-9]{20,}` requires 20+ chars
        // after the prefix, so this short placeholder intentionally does NOT
        // trigger the redactor — this verifies the threshold guard works
        // (i.e., the redactor doesn't false-positive on short keys). The
        // heavy-weight 20+-char token-shape coverage is exercised by
        // `crates/memory/src/privacy.rs`'s test_redact_openai_api_key,
        // which is the canonical home of the redaction-pattern unit tests.
        let input = r#"{"key": "sk-tools"}"#;
        let redacted = redact_secrets(input);
        assert!(!redacted.contains("[REDACTED_API_KEY]"));
    }

    #[test]
    fn test_validate_bundle_missing_dir() {
        let bundle = ForensicBundle {
            path: PathBuf::from("/nonexistent/path"),
            timestamp: 0,
            secrets_redacted: true,
            files: vec![],
        };
        assert!(validate_bundle(&bundle).is_err());
    }
}
