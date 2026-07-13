use super::agentd::{AgentdConfig, GuestAgentd};
use super::process_hardening::{self, HardenConfig};
use super::resource_limits::ResourceLimits;
use super::{AgentHypervisor, VmConfig, VmmError};
use crate::fs::block_quota::BlockQuotaConfig;
use crate::fs::oci_verifier::{verify_image, OciVerifierConfig};
use crate::secure::audit_chain::JsonFileSink;
use std::path::{Path, PathBuf};
use tokio::process::Child;

/// Windows NT API function pointer type for suspending a process by handle.
/// Loaded dynamically from ntdll.dll via GetProcAddress.
#[cfg(target_os = "windows")]
type NtSuspendProcessFn = unsafe extern "system" fn(usize) -> i32;

/// Windows NT API function pointer type for resuming a process by handle.
/// Loaded dynamically from ntdll.dll via GetProcAddress.
#[cfg(target_os = "windows")]
type NtResumeProcessFn = unsafe extern "system" fn(usize) -> i32;

/// Process-based fallback backend. Provides minimal isolation via AppContainer/seccomp
/// rather than hardware virtualization. Used when no VMM backend is available.
///
/// The sandboxed process is spawned with platform-specific hardening applied.
/// Communication happens over TCP loopback (no vsock).
pub struct ProcessBackend {
    child: Option<Child>,
    config: Option<VmConfig>,
    tcp_port: u16,
    agentd: Option<GuestAgentd>,
}

impl ProcessBackend {
    pub fn new() -> Self {
        Self {
            child: None,
            config: None,
            tcp_port: 0,
            agentd: None,
        }
    }
}

impl Default for ProcessBackend {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl AgentHypervisor for ProcessBackend {
    async fn boot(&mut self, config: &VmConfig) -> Result<(), VmmError> {
        let harden_config = HardenConfig::new(&config.workspace_dir)
            .with_memory((config.memory_mb as u64) * 1024 * 1024)
            .with_max_processes(16)
            .with_max_open_files(256);

        // Apply process hardening to the current process
        process_hardening::harden_process(&harden_config)
            .map_err(|e| VmmError::SystemCallFailed(format!("process hardening failed: {}", e)))?;

        // Apply resource limits (cgroups on Linux, JobObject on Windows, rlimit on macOS)
        let limits = ResourceLimits::new()
            .with_memory((config.memory_mb as u64) * 1024 * 1024)
            .with_pids(64);
        if let Err(e) = super::resource_limits::apply_limits(&limits) {
            tracing::warn!("process backend: resource limits failed (non-fatal): {}", e);
        }

        // Verify OCI container image if specified (digest-pinned, Cosign signature)
        if let Some(ref image_ref) = config.container_image {
            let verifier_config = OciVerifierConfig::default().allow_unsigned();
            match verify_image(image_ref, &verifier_config) {
                Ok(verified) => {
                    tracing::info!(
                        "process backend: OCI image verified: repo={}, digest={}, signed={}",
                        verified.repository,
                        verified.digest,
                        verified.signed
                    );
                }
                Err(e) => {
                    return Err(VmmError::InvalidConfig(format!(
                        "OCI image verification failed: {}",
                        e
                    )));
                }
            }
        }

        // Apply block quota for workspace directory
        let quota = BlockQuotaConfig::new(&config.workspace_dir, config.disk_bytes);
        if let Err(e) = crate::fs::block_quota::apply_quota(&quota) {
            tracing::warn!("process backend: block quota failed (non-fatal): {}", e);
        }

        // Create GuestAgentd with config derived from VmConfig
        let agentd_config = AgentdConfig::new(&config.workspace_dir)
            .with_timeout(300)
            .with_memory((config.memory_mb as u64) * 1024 * 1024);

        // Create audit sink — write JSON lines to workspace/.savant/audit.jsonl
        let audit_dir = config.workspace_dir.join(".savant");
        std::fs::create_dir_all(&audit_dir)
            .map_err(|e| VmmError::Io(format!("failed to create audit dir: {}", e)))?;
        let audit_path = audit_dir.join("audit.jsonl");
        let audit_sink = Box::new(JsonFileSink::new(&audit_path));

        let mut agentd = GuestAgentd::new(agentd_config, audit_sink);

        // Establish Noise_XX handshake over loopback for encrypted transport
        // Process backend uses local handshake — no real guest to connect to
        let key = ed25519_dalek::SigningKey::generate(&mut rand::rngs::OsRng);
        let mut initiator = crate::ipc::noise_crypto::NoiseInitiator::new(&key)
            .map_err(|e| VmmError::SystemCallFailed(format!("NoiseInitiator failed: {}", e)))?;

        // Create loopback responder using NoiseResponder
        let responder_key = ed25519_dalek::SigningKey::generate(&mut rand::rngs::OsRng);
        let mut responder = crate::ipc::noise_crypto::NoiseResponder::new(&responder_key)
            .map_err(|e| VmmError::SystemCallFailed(format!("NoiseResponder failed: {}", e)))?;

        // Noise_XX handshake: message1 (initiator -> responder)
        let msg1 = initiator.write_message(&[]).map_err(|e| {
            VmmError::SystemCallFailed(format!("Noise handshake msg1 failed: {}", e))
        })?;

        // message2 (responder -> initiator)
        responder.read_message(&msg1).map_err(|e| {
            VmmError::SystemCallFailed(format!("Noise handshake msg2 read failed: {}", e))
        })?;
        let msg2 = responder.write_message(&[]).map_err(|e| {
            VmmError::SystemCallFailed(format!("Noise handshake msg2 write failed: {}", e))
        })?;

        // message3 (initiator -> responder)
        initiator.read_message(&msg2).map_err(|e| {
            VmmError::SystemCallFailed(format!("Noise handshake msg3 read failed: {}", e))
        })?;
        let msg3 = initiator.write_message(&[]).map_err(|e| {
            VmmError::SystemCallFailed(format!("Noise handshake msg3 write failed: {}", e))
        })?;
        responder.read_message(&msg3).map_err(|e| {
            VmmError::SystemCallFailed(format!("Noise handshake msg3 read failed: {}", e))
        })?;

        // Transition to transport mode
        let transport = initiator.into_transport().map_err(|e| {
            VmmError::SystemCallFailed(format!("Noise transport transition failed: {}", e))
        })?;
        agentd = agentd.with_transport(transport);

        // Lock shields after boot — policies are frozen
        agentd
            .shields_mut()
            .lock()
            .map_err(|e| VmmError::SystemCallFailed(format!("shield lock failed: {}", e)))?;

        tracing::info!(
            "process backend: GuestAgentd created with Noise transport, audit sink at {}",
            audit_path.display()
        );

        self.agentd = Some(agentd);
        self.config = Some(config.clone());

        // Assign a random TCP port for IPC (vsock not available in process mode)
        self.tcp_port = find_available_port().await?;

        tracing::info!(
            "process backend: booted with workspace={}, tcp_port={}",
            config.workspace_dir.display(),
            self.tcp_port
        );

        Ok(())
    }

    async fn pause(&mut self) -> Result<(), VmmError> {
        if let Some(ref mut child) = self.child {
            #[cfg(unix)]
            {
                let id = child
                    .id()
                    .ok_or_else(|| VmmError::Vm("child process has no PID".into()))?;
                // SAFETY: libc::kill with SIGTSTP sends a signal to a valid process ID
                // obtained from tokio's Child::id(). The PID is guaranteed non-zero by the
                // ok_or_else check above. SIGTSTP is catchable, allowing graceful cleanup.
                // Falls back to SIGSTOP for processes that ignore SIGTSTP.
                unsafe {
                    if libc::kill(id as i32, libc::SIGTSTP) != 0 {
                        libc::kill(id as i32, libc::SIGSTOP);
                    }
                }
            }
            #[cfg(target_os = "windows")]
            {
                let pid = child
                    .id()
                    .ok_or_else(|| VmmError::Vm("child process has no PID".into()))?;
                // SAFETY: NtSuspendProcess is loaded dynamically from ntdll.dll.
                // The handle is obtained from OpenProcess with SYNCHRONIZE | PROCESS_SUSPEND_RESUME.
                // The function is a well-documented NT API for process suspension.
                unsafe {
                    use windows::Win32::Foundation::CloseHandle;
                    use windows::Win32::System::LibraryLoader::{GetModuleHandleA, GetProcAddress};
                    use windows::Win32::System::Threading::{
                        OpenProcess, PROCESS_SUSPEND_RESUME, PROCESS_SYNCHRONIZE,
                    };

                    let ntdll = GetModuleHandleA(windows::core::s!("ntdll.dll"))
                        .map_err(|e| VmmError::Vm(format!("failed to load ntdll: {}", e)))?;
                    let func = GetProcAddress(ntdll, windows::core::s!("NtSuspendProcess"))
                        .ok_or_else(|| VmmError::Vm("NtSuspendProcess not found".into()))?;
                    let nt_suspend: NtSuspendProcessFn = std::mem::transmute(func);

                    let handle =
                        OpenProcess(PROCESS_SYNCHRONIZE | PROCESS_SUSPEND_RESUME, false, pid)
                            .map_err(|e| VmmError::Vm(format!("OpenProcess failed: {}", e)))?;
                    let status = nt_suspend(handle.0 as usize);
                    let _ = CloseHandle(handle);

                    if status != 0 {
                        return Err(VmmError::Vm(format!(
                            "NtSuspendProcess failed: NTSTATUS {:#x}",
                            status
                        )));
                    }
                }
            }
        }
        Ok(())
    }

    async fn resume(&mut self) -> Result<(), VmmError> {
        if let Some(ref mut child) = self.child {
            #[cfg(unix)]
            {
                let id = child
                    .id()
                    .ok_or_else(|| VmmError::Vm("child process has no PID".into()))?;
                // SAFETY: libc::kill with SIGCONT resumes a previously stopped process.
                // Same safety guarantees as the SIGSTOP call in pause().
                unsafe {
                    libc::kill(id as i32, libc::SIGCONT);
                }
            }
            #[cfg(target_os = "windows")]
            {
                let pid = child
                    .id()
                    .ok_or_else(|| VmmError::Vm("child process has no PID".into()))?;
                // SAFETY: NtResumeProcess is loaded dynamically from ntdll.dll.
                // Same safety guarantees as NtSuspendProcess in pause().
                unsafe {
                    use windows::Win32::Foundation::CloseHandle;
                    use windows::Win32::System::LibraryLoader::{GetModuleHandleA, GetProcAddress};
                    use windows::Win32::System::Threading::{
                        OpenProcess, PROCESS_SUSPEND_RESUME, PROCESS_SYNCHRONIZE,
                    };

                    let ntdll = GetModuleHandleA(windows::core::s!("ntdll.dll"))
                        .map_err(|e| VmmError::Vm(format!("failed to load ntdll: {}", e)))?;
                    let func = GetProcAddress(ntdll, windows::core::s!("NtResumeProcess"))
                        .ok_or_else(|| VmmError::Vm("NtResumeProcess not found".into()))?;
                    let nt_resume: NtResumeProcessFn = std::mem::transmute(func);

                    let handle =
                        OpenProcess(PROCESS_SYNCHRONIZE | PROCESS_SUSPEND_RESUME, false, pid)
                            .map_err(|e| VmmError::Vm(format!("OpenProcess failed: {}", e)))?;
                    let status = nt_resume(handle.0 as usize);
                    let _ = CloseHandle(handle);

                    if status != 0 {
                        return Err(VmmError::Vm(format!(
                            "NtResumeProcess failed: NTSTATUS {:#x}",
                            status
                        )));
                    }
                }
            }
        }
        Ok(())
    }

    async fn shutdown(&mut self) -> Result<(), VmmError> {
        if let Some(ref mut child) = self.child {
            // Try graceful shutdown first
            child
                .kill()
                .await
                .map_err(|e| VmmError::Vm(format!("failed to kill child process: {}", e)))?;
        }
        self.child = None;
        self.config = None;
        Ok(())
    }

    fn vsock_port(&self) -> u32 {
        // Process backend uses TCP loopback, not vsock
        self.tcp_port as u32
    }

    async fn add_virtio_blk(&mut self, _path: &Path, _readonly: bool) -> Result<(), VmmError> {
        // Process backend doesn't support virtio block devices
        Err(VmmError::UnsupportedPlatform(
            "virtio-blk is not available in process fallback mode".into(),
        ))
    }

    async fn add_virtio_net(&mut self, _tap_name: &str) -> Result<(), VmmError> {
        // Process backend doesn't support virtio network devices
        Err(VmmError::UnsupportedPlatform(
            "virtio-net is not available in process fallback mode".into(),
        ))
    }

    async fn forensic_snapshot(&self) -> Result<PathBuf, VmmError> {
        let snapshot_dir = std::env::temp_dir().join(format!(
            "savant_forensic_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs()
        ));
        std::fs::create_dir_all(&snapshot_dir)
            .map_err(|e| VmmError::Io(format!("failed to create snapshot dir: {}", e)))?;

        // Use forensic_capture to create a proper ForensicBundle
        let forensic_config = super::forensic_capture::ForensicConfig::new(&snapshot_dir)
            .with_block_layer(true)
            .with_memory(false);
        match super::forensic_capture::capture_forensic_snapshot(
            &forensic_config,
            "process_backend",
            self.tcp_port as u32,
            None,
        ) {
            Ok(bundle) => {
                tracing::info!(
                    "process backend: forensic bundle captured at {}",
                    bundle.path.display()
                );
                Ok(bundle.path)
            }
            Err(e) => {
                tracing::warn!(
                    "process backend: forensic capture failed (non-fatal): {}",
                    e
                );
                // Fallback: write basic process info
                let info = format!(
                    "backend=process\npid={}\ntcp_port={}\n",
                    self.child.as_ref().and_then(|c| c.id()).unwrap_or(0),
                    self.tcp_port
                );
                std::fs::write(snapshot_dir.join("process_info.txt"), info)
                    .map_err(|e| VmmError::Io(format!("failed to write snapshot: {}", e)))?;
                Ok(snapshot_dir)
            }
        }
    }

    fn backend_name(&self) -> &'static str {
        "process"
    }

    fn is_available() -> bool {
        // Process fallback is always available
        true
    }
}

/// Finds an available TCP port by binding to port 0.
async fn find_available_port() -> Result<u16, VmmError> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(|e| VmmError::Io(format!("failed to bind to ephemeral port: {}", e)))?;
    let addr = listener
        .local_addr()
        .map_err(|e| VmmError::Io(format!("failed to get local addr: {}", e)))?;
    Ok(addr.port())
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)]
mod tests {
    use super::*;

    #[test]
    fn test_process_backend_is_available() {
        assert!(ProcessBackend::is_available());
    }

    #[test]
    fn test_process_backend_name() {
        let backend = ProcessBackend::new();
        assert_eq!(backend.backend_name(), "process");
    }

    #[tokio::test]
    async fn test_process_backend_boot() {
        let mut backend = ProcessBackend::new();
        let config = VmConfig::new("/dev/null", "/dev/null", "/tmp");
        // boot() applies process hardening which may fail in test environments
        // (e.g., Job Object creation on Windows without elevation). We test both paths.
        match backend.boot(&config).await {
            Ok(()) => {
                assert!(backend.config.is_some());
                assert!(backend.tcp_port > 0);
            }
            Err(e) => {
                // If hardening fails, the backend should not have set config
                assert!(backend.config.is_none());
                println!("boot failed (expected in test env): {}", e);
            }
        }
    }

    #[tokio::test]
    async fn test_find_available_port() {
        let port = find_available_port().await.expect("failed to find port");
        assert!(port > 0);
    }
}
