pub mod agentd;
pub mod backend_cloudhypervisor;
pub mod backend_hcs;
pub mod backend_macos;
pub mod backend_process;
pub mod forensic_capture;
pub mod process_hardening;
pub mod resource_limits;

use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum VmmError {
    #[error("platform not supported: {0}")]
    UnsupportedPlatform(String),
    #[error("permission denied: {0}")]
    PermissionDenied(String),
    #[error("system call failed: {0}")]
    SystemCallFailed(String),
    #[error("configuration invalid: {0}")]
    InvalidConfig(String),
    #[error("I/O error: {0}")]
    Io(String),
    #[error("VM error: {0}")]
    Vm(String),
    #[error("timeout: {0}")]
    Timeout(String),
    #[error("HCS operation failed: {0}")]
    HcsOperationFailed(String),
}

/// Configuration for a virtual machine instance.
#[derive(Debug, Clone)]
pub struct VmConfig {
    /// Path to the kernel image.
    pub kernel_path: PathBuf,
    /// Path to the initrd image.
    pub initrd_path: PathBuf,
    /// vsock context ID for host-guest communication.
    pub vsock_cid: u32,
    /// Memory in megabytes.
    pub memory_mb: u32,
    /// Number of vCPUs.
    pub cpu_count: u32,
    /// Workspace directory to mount into the guest.
    pub workspace_dir: PathBuf,
    /// Maximum disk space in bytes for the writable overlay.
    pub disk_bytes: u64,
    /// Optional OCI container image reference (digest-pinned) for verification.
    pub container_image: Option<String>,
}

impl VmConfig {
    pub fn new(
        kernel: impl Into<PathBuf>,
        initrd: impl Into<PathBuf>,
        workspace: impl Into<PathBuf>,
    ) -> Self {
        Self {
            kernel_path: kernel.into(),
            initrd_path: initrd.into(),
            vsock_cid: 3,
            memory_mb: 256,
            cpu_count: 1,
            workspace_dir: workspace.into(),
            disk_bytes: 1024 * 1024 * 1024, // 1 GB
            container_image: None,
        }
    }

    pub fn with_memory(mut self, mb: u32) -> Self {
        self.memory_mb = mb;
        self
    }

    pub fn with_cpus(mut self, count: u32) -> Self {
        self.cpu_count = count;
        self
    }

    pub fn with_vsock_cid(mut self, cid: u32) -> Self {
        self.vsock_cid = cid;
        self
    }

    pub fn with_disk_bytes(mut self, bytes: u64) -> Self {
        self.disk_bytes = bytes;
        self
    }
}

/// Abstract trait for hypervisor backends. Each backend provides hardware-level
/// isolation by running agent code inside a virtual machine or sandboxed process.
///
/// Implementations:
/// - `CloudHypervisorBackend` — Linux KVM / Windows WHP (Tier 1)
/// - `HcsBackend` — Windows Host Compute Service (Tier 1)
/// - `MacosBackend` — Virtualization.framework (Tier 2)
/// - `ProcessBackend` — AppContainer/seccomp fallback (Tier 3)
#[async_trait::async_trait]
pub trait AgentHypervisor: Send + Sync {
    /// Boots a new VM with the given configuration.
    async fn boot(&mut self, config: &VmConfig) -> Result<(), VmmError>;

    /// Pauses the VM (all vCPUs stop). Guest memory is preserved.
    async fn pause(&mut self) -> Result<(), VmmError>;

    /// Resumes a paused VM.
    async fn resume(&mut self) -> Result<(), VmmError>;

    /// Shuts down the VM gracefully, then forcefully after timeout.
    async fn shutdown(&mut self) -> Result<(), VmmError>;

    /// Returns the vsock port for host-guest IPC.
    fn vsock_port(&self) -> u32;

    /// Adds a virtio block device to the VM.
    async fn add_virtio_blk(
        &mut self,
        path: &std::path::Path,
        readonly: bool,
    ) -> Result<(), VmmError>;

    /// Adds a virtio network device to the VM.
    async fn add_virtio_net(&mut self, tap_name: &str) -> Result<(), VmmError>;

    /// Captures a forensic snapshot of the VM state for debugging.
    async fn forensic_snapshot(&self) -> Result<PathBuf, VmmError>;

    /// Returns the name of this backend for logging.
    fn backend_name(&self) -> &'static str;

    /// Returns `true` if this backend is available on the current platform.
    fn is_available() -> bool
    where
        Self: Sized;
}

/// Selects the best available hypervisor backend for the current platform.
/// Tries backends in priority order: cloud-hypervisor → HCS → macOS → process fallback.
pub async fn select_backend() -> Box<dyn AgentHypervisor> {
    if backend_cloudhypervisor::CloudHypervisorBackend::is_available() {
        tracing::info!("selected cloud-hypervisor backend");
        return Box::new(backend_cloudhypervisor::CloudHypervisorBackend::new());
    }

    #[cfg(target_os = "windows")]
    if backend_hcs::HcsBackend::is_available() {
        tracing::info!("selected HCS backend");
        return Box::new(backend_hcs::HcsBackend::new());
    }

    #[cfg(target_os = "macos")]
    if backend_macos::MacosBackend::is_available() {
        tracing::info!("selected macOS Virtualization.framework backend");
        return Box::new(backend_macos::MacosBackend::new());
    }

    tracing::warn!("no VMM backend available, falling back to process isolation");
    Box::new(backend_process::ProcessBackend::new())
}
