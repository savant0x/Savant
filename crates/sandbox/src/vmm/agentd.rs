use crate::ipc::noise_crypto::NoiseTransport;
use crate::ipc::vsock_bridge::ChannelError;
use crate::net::dns_interceptor::DnsInterceptor;
use crate::net::ssrf::SsrfGuard;
use crate::secure::audit_chain::{ActionType, AuditChain, AuditSink};
use crate::secure::credential_vault::CredentialVault;
use crate::secure::shields::ShieldManager;
use std::sync::Arc;
use tokio::sync::Mutex;

/// Guest agentd configuration.
#[derive(Debug, Clone)]
pub struct AgentdConfig {
    /// The working directory for command execution.
    pub workspace_dir: std::path::PathBuf,
    /// Maximum command execution time in seconds.
    pub command_timeout_secs: u64,
    /// Maximum memory for the agent in bytes.
    pub max_memory_bytes: u64,
    /// Allowed domains for DNS interception (empty = allow all).
    pub allowed_domains: Vec<String>,
}

impl AgentdConfig {
    pub fn new(workspace_dir: impl Into<std::path::PathBuf>) -> Self {
        Self {
            workspace_dir: workspace_dir.into(),
            command_timeout_secs: 300,
            max_memory_bytes: 256 * 1024 * 1024,
            allowed_domains: Vec::new(),
        }
    }

    pub fn with_timeout(mut self, secs: u64) -> Self {
        self.command_timeout_secs = secs;
        self
    }

    pub fn with_memory(mut self, bytes: u64) -> Self {
        self.max_memory_bytes = bytes;
        self
    }

    pub fn with_allowed_domains(mut self, domains: Vec<String>) -> Self {
        self.allowed_domains = domains;
        self
    }
}

/// Command request sent from host to guest.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CommandRequest {
    /// Unique request ID for correlation.
    pub request_id: u64,
    /// The command to execute.
    pub command: String,
    /// Working directory override (optional).
    pub working_dir: Option<String>,
    /// Environment variables to set.
    pub env: std::collections::HashMap<String, String>,
}

/// Response from the guest agentd to the host.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CommandResponse {
    /// The request ID this response corresponds to.
    pub request_id: u64,
    /// Exit code of the command.
    pub exit_code: i32,
    /// Captured stdout.
    pub stdout: Vec<u8>,
    /// Captured stderr.
    pub stderr: Vec<u8>,
    /// Execution duration in milliseconds.
    pub duration_ms: u64,
}

/// Telemetry event streamed from guest to host.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TelemetryEvent {
    /// Timestamp (epoch microseconds).
    pub timestamp: u64,
    /// The action type.
    pub action_type: String,
    /// Event payload.
    pub payload: Vec<u8>,
}

/// Guest agentd process. Runs inside the microVM, receives commands over the
/// Noise-encrypted vsock channel, executes them, and streams telemetry back.
///
/// The agentd is the init process (PID 1) inside the guest. It:
/// 1. Establishes a Noise handshake with the host
/// 2. Listens for CommandRequest messages
/// 3. Executes commands with structural safety (read-only rootfs, no network)
/// 4. Streams TelemetryEvent messages back over the authenticated channel
/// 5. Maintains a Merkle-chained audit log of all actions
/// 6. Enforces network policy via DNS interception and SSRF guard
/// 7. Manages secrets via credential vault with memory locking
/// 8. Protects against threats via shield manager
pub struct GuestAgentd {
    config: AgentdConfig,
    audit: AuditChain,
    audit_sink: Box<dyn AuditSink>,
    /// Noise-encrypted transport for host-guest communication.
    transport: Option<NoiseTransport>,
    /// DNS interceptor for domain allowlisting.
    dns_interceptor: Arc<Mutex<DnsInterceptor>>,
    /// SSRF guard for URL/IP validation.
    ssrf_guard: Arc<SsrfGuard>,
    /// Credential vault for secret management with memory locking.
    vault: Arc<CredentialVault>,
    /// Shield manager for EDR-like threat protection.
    shields: ShieldManager,
}

impl GuestAgentd {
    pub fn new(config: AgentdConfig, audit_sink: Box<dyn AuditSink>) -> Self {
        let dns_interceptor = if config.allowed_domains.is_empty() {
            DnsInterceptor::new()
        } else {
            DnsInterceptor::new().with_allowed_domains(config.allowed_domains.clone())
        };

        Self {
            config,
            audit: AuditChain::new(),
            audit_sink,
            transport: None,
            dns_interceptor: Arc::new(Mutex::new(dns_interceptor)),
            ssrf_guard: Arc::new(SsrfGuard::new()),
            vault: Arc::new(CredentialVault::new()),
            shields: ShieldManager::default(),
        }
    }

    /// Sets the Noise transport for encrypted host-guest communication.
    pub fn with_transport(mut self, transport: NoiseTransport) -> Self {
        self.transport = Some(transport);
        self
    }

    /// Returns a mutable reference to the shield manager.
    pub fn shields_mut(&mut self) -> &mut ShieldManager {
        &mut self.shields
    }

    /// Returns a reference to the credential vault.
    pub fn vault(&self) -> &CredentialVault {
        &self.vault
    }

    /// Returns a reference to the SSRF guard.
    pub fn ssrf_guard(&self) -> &SsrfGuard {
        &self.ssrf_guard
    }

    /// Validates a URL through the SSRF guard before execution.
    /// Returns the resolved IP address if valid.
    pub async fn validate_url(&self, url: &str) -> Result<std::net::IpAddr, AgentdError> {
        self.ssrf_guard
            .validate_url(url)
            .map_err(|e| AgentdError::Execution(format!("SSRF validation failed: {}", e)))
    }

    /// Resolves a domain through the DNS interceptor.
    pub async fn resolve_domain(&self, domain: &str) -> Result<std::net::IpAddr, AgentdError> {
        let mut dns = self.dns_interceptor.lock().await;
        dns.resolve(domain, std::time::Duration::from_secs(300))
            .map_err(|e| AgentdError::Execution(format!("DNS resolution failed: {}", e)))
    }

    /// Updates the allowed domains list on the DNS interceptor.
    pub async fn update_allowed_domains(&self, domains: Vec<String>) {
        let mut dns = self.dns_interceptor.lock().await;
        dns.update_allowed_domains(domains);
    }

    /// Injects a secret into the credential vault.
    pub fn inject_secret(&self, key: &str, value: &[u8]) {
        self.vault.inject_secret(key, value);
    }

    /// Substitutes secret placeholders in a string using the vault.
    pub fn substitute_secrets(&self, input: &str) -> String {
        self.vault.substitute(input)
    }

    /// Executes a command and returns the response.
    /// This is the core of the agentd — it runs commands inside the guest
    /// with structural safety constraints.
    pub async fn execute_command(
        &mut self,
        request: &CommandRequest,
    ) -> Result<CommandResponse, AgentdError> {
        let start = std::time::Instant::now();

        // Audit: record the command execution
        self.audit
            .append(ActionType::Exec, request.command.as_bytes())
            .map_err(|e| AgentdError::Audit(e.to_string()))?;
        let last_idx = self.audit.entries().len() - 1;
        let entry = &self.audit.entries()[last_idx];
        self.audit_sink
            .emit(entry)
            .map_err(|e| AgentdError::Audit(e.to_string()))?;

        // Subsystem 1: Credential vault — substitute secret placeholders before execution
        let command = self.vault.substitute(&request.command);

        // Subsystem 2: SSRF guard — validate any URLs in the command
        static URL_RE: once_cell::sync::Lazy<regex_lite::Regex> =
            once_cell::sync::Lazy::new(|| {
                regex_lite::Regex::new(r#"https?://[^\s'"`]+"#)
                    .unwrap_or_else(|e| panic!("invalid URL regex pattern: {e}"))
            });
        for url_match in URL_RE.find_iter(&command) {
            if let Err(e) = self.ssrf_guard.validate_url(url_match.as_str()) {
                return Err(AgentdError::Execution(format!(
                    "SSRF validation failed for URL '{}': {}",
                    url_match.as_str(),
                    e
                )));
            }
        }

        // Subsystem 3: DNS interceptor — resolve domains in the command
        static DOMAIN_RE: once_cell::sync::Lazy<regex_lite::Regex> =
            once_cell::sync::Lazy::new(|| {
                regex_lite::Regex::new(
                    r"\b([a-zA-Z0-9]([a-zA-Z0-9-]{0,61}[a-zA-Z0-9])?\.)+[a-zA-Z]{2,}\b",
                )
                .unwrap_or_else(|e| panic!("invalid domain regex pattern: {e}"))
            });
        for domain_match in DOMAIN_RE.find_iter(&command) {
            let domain = domain_match.as_str();
            let mut dns = self.dns_interceptor.lock().await;
            if let Err(e) = dns.resolve(domain, std::time::Duration::from_secs(300)) {
                return Err(AgentdError::Execution(format!(
                    "DNS resolution failed for domain '{}': {}",
                    domain, e
                )));
            }
        }

        // Subsystem 4: Shield manager — check if policies allow execution
        if let Err(e) = self.shields.check_mutable() {
            return Err(AgentdError::Execution(format!(
                "Shield policy blocks execution: {}",
                e
            )));
        }

        // Execute the command with timeout
        let mut cmd = tokio::process::Command::new("sh");
        cmd.arg("-c").arg(&command);

        // Set working directory
        if let Some(ref dir) = request.working_dir {
            cmd.current_dir(dir);
        } else {
            cmd.current_dir(&self.config.workspace_dir);
        }

        // Set environment variables
        for (key, value) in &request.env {
            cmd.env(key, value);
        }

        // Capture stdout and stderr
        cmd.stdout(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::piped());

        // Apply timeout
        let timeout = std::time::Duration::from_secs(self.config.command_timeout_secs);

        let child = cmd
            .spawn()
            .map_err(|e| AgentdError::Execution(format!("failed to spawn command: {}", e)))?;

        let output = tokio::time::timeout(timeout, child.wait_with_output())
            .await
            .map_err(|_| AgentdError::Timeout(request.request_id))?
            .map_err(|e| AgentdError::Execution(format!("command failed: {}", e)))?;

        let duration_ms = start.elapsed().as_millis() as u64;

        let response = CommandResponse {
            request_id: request.request_id,
            exit_code: output.status.code().unwrap_or(-1),
            stdout: output.stdout,
            stderr: output.stderr,
            duration_ms,
        };

        // Audit: record the result
        let result_payload = format!(
            "exit={} duration={}ms",
            response.exit_code, response.duration_ms
        );
        if let Err(e) = self
            .audit
            .append(ActionType::Exec, result_payload.as_bytes())
        {
            tracing::warn!("[agentd] failed to append result audit entry: {}", e);
        }

        Ok(response)
    }

    /// Returns a reference to the audit chain.
    pub fn audit(&self) -> &AuditChain {
        &self.audit
    }

    /// Returns the workspace directory.
    pub fn workspace_dir(&self) -> &std::path::Path {
        &self.config.workspace_dir
    }
}

#[derive(Debug, thiserror::Error)]
pub enum AgentdError {
    #[error("execution failed: {0}")]
    Execution(String),
    #[error("timeout for request {0}")]
    Timeout(u64),
    #[error("audit error: {0}")]
    Audit(String),
    #[error("channel error: {0}")]
    Channel(#[from] ChannelError),
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_agentd_config_builder() {
        let config = AgentdConfig::new("/workspace")
            .with_timeout(600)
            .with_memory(512 * 1024 * 1024);
        assert_eq!(config.workspace_dir, PathBuf::from("/workspace"));
        assert_eq!(config.command_timeout_secs, 600);
        assert_eq!(config.max_memory_bytes, 512 * 1024 * 1024);
    }

    // The following tests require `sh` (Unix shell) — agentd is guest-VM code
    // designed to run as PID 1 inside a Linux microVM.
    #[cfg(unix)]
    mod unix_tests {
        use super::*;
        use crate::secure::audit_chain::VecSink;

        #[tokio::test]
        async fn test_execute_simple_command() {
            let config = AgentdConfig::new("/tmp");
            let sink = Box::new(VecSink::new());
            let mut agentd = GuestAgentd::new(config, sink);

            let request = CommandRequest {
                request_id: 1,
                command: "echo hello".to_string(),
                working_dir: None,
                env: std::collections::HashMap::new(),
            };

            let response = agentd
                .execute_command(&request)
                .await
                .expect("execution failed");
            assert_eq!(response.exit_code, 0);
            assert_eq!(response.stdout, b"hello\n");
            assert!(response.stderr.is_empty());
        }

        #[tokio::test]
        async fn test_execute_command_with_exit_code() {
            let config = AgentdConfig::new("/tmp");
            let sink = Box::new(VecSink::new());
            let mut agentd = GuestAgentd::new(config, sink);

            let request = CommandRequest {
                request_id: 2,
                command: "exit 42".to_string(),
                working_dir: None,
                env: std::collections::HashMap::new(),
            };

            let response = agentd
                .execute_command(&request)
                .await
                .expect("execution failed");
            assert_eq!(response.exit_code, 42);
        }

        #[tokio::test]
        async fn test_execute_command_timeout() {
            let config = AgentdConfig::new("/tmp").with_timeout(1);
            let sink = Box::new(VecSink::new());
            let mut agentd = GuestAgentd::new(config, sink);

            let request = CommandRequest {
                request_id: 3,
                command: "sleep 10".to_string(),
                working_dir: None,
                env: std::collections::HashMap::new(),
            };

            let result = agentd.execute_command(&request).await;
            assert!(result.is_err());
            match result.unwrap_err() {
                AgentdError::Timeout(id) => assert_eq!(id, 3),
                other => panic!("expected Timeout, got: {}", other),
            }
        }

        #[tokio::test]
        async fn test_command_audit_trail() {
            let config = AgentdConfig::new("/tmp");
            let sink = Box::new(VecSink::new());
            let mut agentd = GuestAgentd::new(config, sink);

            let request = CommandRequest {
                request_id: 1,
                command: "echo test".to_string(),
                working_dir: None,
                env: std::collections::HashMap::new(),
            };

            agentd
                .execute_command(&request)
                .await
                .expect("execution failed");

            // Should have: genesis + command + result = 3 entries
            assert_eq!(agentd.audit().len(), 3);
            agentd
                .audit()
                .verify()
                .expect("audit chain integrity failed");
        }
    }
}
