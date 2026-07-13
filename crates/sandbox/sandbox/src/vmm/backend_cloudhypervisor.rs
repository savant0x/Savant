use super::{AgentHypervisor, VmConfig, VmmError};
use std::path::{Path, PathBuf};
use tokio::process::{Child, Command};

/// Cloud-hypervisor backend. Spawns `cloud-hypervisor` as a child process and
/// configures it via its REST API on localhost.
///
/// Supports Linux KVM and Windows WHP/MSHV. This is the primary Tier 1 backend.
pub struct CloudHypervisorBackend {
    child: Option<Child>,
    api_port: u16,
    api_base: String,
    vsock_port: u32,
    vm_config: Option<VmConfig>,
}

impl CloudHypervisorBackend {
    pub fn new() -> Self {
        Self {
            child: None,
            api_port: 0,
            api_base: String::new(),
            vsock_port: 0,
            vm_config: None,
        }
    }

    /// Detects the cloud-hypervisor binary location.
    fn find_binary() -> Option<PathBuf> {
        // Check PATH first
        if let Ok(output) = std::process::Command::new("cloud-hypervisor")
            .arg("--version")
            .output()
        {
            if output.status.success() {
                return Some(PathBuf::from("cloud-hypervisor"));
            }
        }

        // Check common installation paths
        let candidates: Vec<PathBuf> = vec![
            PathBuf::from("/usr/local/bin/cloud-hypervisor"),
            PathBuf::from("/usr/bin/cloud-hypervisor"),
            PathBuf::from("/opt/cloud-hypervisor/bin/cloud-hypervisor"),
            // Windows paths
            PathBuf::from(r"C:\Program Files\cloud-hypervisor\cloud-hypervisor.exe"),
        ];

        for path in &candidates {
            if path.exists() {
                return Some(path.clone());
            }
        }

        None
    }

    /// Checks if KVM is available on Linux.
    #[cfg(target_os = "linux")]
    fn is_kvm_available() -> bool {
        Path::new("/dev/kvm").exists()
    }

    /// Checks if WHP/MSHV is available on Windows.
    #[cfg(target_os = "windows")]
    fn is_whp_available() -> bool {
        // Check if the vmcompute service is running (indicates Hyper-V is available)
        let output = std::process::Command::new("sc")
            .args(["query", "vmcompute"])
            .output();
        match output {
            Ok(o) => {
                let stdout = String::from_utf8_lossy(&o.stdout);
                stdout.contains("RUNNING")
            }
            Err(_) => false,
        }
    }

    async fn api_put(&self, endpoint: &str, body: &str) -> Result<String, VmmError> {
        let url = format!("{}/api/v1/{}", self.api_base, endpoint);
        let client = reqwest::Client::new();
        let response = client
            .put(&url)
            .header("Content-Type", "application/json")
            .body(body.to_string())
            .send()
            .await
            .map_err(|e| VmmError::Io(format!("API request failed: {}", e)))?;

        let status = response.status();
        let text = response
            .text()
            .await
            .map_err(|e| VmmError::Io(format!("failed to read API response: {}", e)))?;

        if !status.is_success() {
            return Err(VmmError::Vm(format!("API returned {}: {}", status, text)));
        }
        Ok(text)
    }

    fn build_vm_json(config: &VmConfig, api_port: u16) -> String {
        format!(
            r#"{{
            "cpus": {{"count": {cpu}}},
            "memory": {{"size": {mem}}},
            "kernel": {{"path": "{kernel}"}},
            "initramfs": {{"path": "{initrd}"}},
            "serial": {{"mode": "Null"}},
            "console": {{"mode": "Null"}},
            "vsock": {{"cid": {vsock_cid}, "socket": "/tmp/ch-vsock-{port}"}},
            "api_socket": {{"mode": "File", "path": "/tmp/ch-api-{port}"}}
        }}"#,
            cpu = config.cpu_count,
            mem = (config.memory_mb as u64) * 1024 * 1024,
            kernel = config.kernel_path.display(),
            initrd = config.initrd_path.display(),
            vsock_cid = config.vsock_cid,
            port = api_port,
        )
    }
}

impl Default for CloudHypervisorBackend {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl AgentHypervisor for CloudHypervisorBackend {
    async fn boot(&mut self, config: &VmConfig) -> Result<(), VmmError> {
        let binary = Self::find_binary().ok_or_else(|| {
            VmmError::UnsupportedPlatform(
                "cloud-hypervisor binary not found in PATH or common locations".into(),
            )
        })?;

        // Find an available port for the API
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .map_err(|e| VmmError::Io(format!("failed to bind API port: {}", e)))?;
        self.api_port = listener
            .local_addr()
            .map_err(|e| VmmError::Io(e.to_string()))?
            .port();
        drop(listener);

        self.api_base = format!("http://127.0.0.1:{}", self.api_port);

        // Spawn cloud-hypervisor with API socket
        let api_socket = format!("/tmp/ch-api-{}", self.api_port);
        let child = Command::new(binary)
            .args(["--api-socket", &api_socket])
            .spawn()
            .map_err(|e| VmmError::Io(format!("failed to spawn cloud-hypervisor: {}", e)))?;

        self.child = Some(child);

        // Wait for the API to become available (up to 5 seconds)
        let start = std::time::Instant::now();
        let mut api_ready = false;
        while start.elapsed() < std::time::Duration::from_secs(5) {
            if self.api_put("vm.info", "{}").await.is_ok() {
                api_ready = true;
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
        if !api_ready {
            return Err(VmmError::Timeout(
                "cloud-hypervisor API did not become available within 5s".into(),
            ));
        }

        // Create the VM
        let vm_json = Self::build_vm_json(config, self.api_port);
        self.api_put("vm.create", &vm_json).await?;

        // Boot the VM
        self.api_put("vm.boot", "{}").await?;

        self.vm_config = Some(config.clone());
        self.vsock_port = config.vsock_cid;

        tracing::info!(
            "cloud-hypervisor backend: booted VM with cid={}, memory={}MB, cpus={}",
            config.vsock_cid,
            config.memory_mb,
            config.cpu_count
        );

        Ok(())
    }

    async fn pause(&mut self) -> Result<(), VmmError> {
        self.api_put("vm.pause", "{}").await?;
        Ok(())
    }

    async fn resume(&mut self) -> Result<(), VmmError> {
        self.api_put("vm.resume", "{}").await?;
        Ok(())
    }

    async fn shutdown(&mut self) -> Result<(), VmmError> {
        if self.child.is_some() {
            if let Err(e) = self.api_put("vm.shutdown", "{}").await {
                tracing::warn!("[cloud-hypervisor] vm.shutdown API call failed: {}", e);
            }

            // Give it a moment to shut down gracefully
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;

            if let Some(ref mut child) = self.child {
                if let Err(e) = child.kill().await {
                    tracing::warn!("[cloud-hypervisor] failed to kill child process: {}", e);
                }
            }
        }
        self.child = None;
        self.vm_config = None;
        Ok(())
    }

    fn vsock_port(&self) -> u32 {
        self.vsock_port
    }

    async fn add_virtio_blk(&mut self, path: &Path, readonly: bool) -> Result<(), VmmError> {
        let body = format!(
            r#"{{"path": "{}", "readonly": {}}}"#,
            path.display(),
            readonly
        );
        self.api_put("vm.add-disk", &body).await?;
        Ok(())
    }

    async fn add_virtio_net(&mut self, tap_name: &str) -> Result<(), VmmError> {
        let body = format!(r#"{{"tap": "{}"}}"#, tap_name);
        self.api_put("vm.add-net", &body).await?;
        Ok(())
    }

    async fn forensic_snapshot(&self) -> Result<PathBuf, VmmError> {
        let snapshot_dir = std::env::temp_dir().join(format!(
            "savant_forensic_ch_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs()
        ));
        std::fs::create_dir_all(&snapshot_dir)
            .map_err(|e| VmmError::Io(format!("failed to create snapshot dir: {}", e)))?;

        // Try to dump VM state via the API
        if let Ok(snapshot_json) = self.api_put("vm.snapshot", "{}").await {
            std::fs::write(snapshot_dir.join("vm_snapshot.json"), snapshot_json)
                .map_err(|e| VmmError::Io(format!("failed to write snapshot: {}", e)))?;
        }

        Ok(snapshot_dir)
    }

    fn backend_name(&self) -> &'static str {
        "cloud-hypervisor"
    }

    fn is_available() -> bool {
        Self::find_binary().is_some() && {
            #[cfg(target_os = "linux")]
            {
                Self::is_kvm_available()
            }
            #[cfg(target_os = "windows")]
            {
                Self::is_whp_available()
            }
            #[cfg(not(any(target_os = "linux", target_os = "windows")))]
            {
                false
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cloudhypervisor_backend_name() {
        let backend = CloudHypervisorBackend::new();
        assert_eq!(backend.backend_name(), "cloud-hypervisor");
    }

    #[test]
    fn test_find_binary() {
        // This test just verifies the function runs without panicking.
        // On most CI systems, cloud-hypervisor won't be installed.
        let _ = CloudHypervisorBackend::find_binary();
    }

    #[test]
    fn test_build_vm_json() {
        let config = VmConfig::new("/tmp/kernel", "/tmp/initrd", "/tmp/ws")
            .with_memory(512)
            .with_cpus(2)
            .with_vsock_cid(10);
        let json = CloudHypervisorBackend::build_vm_json(&config, 8000);
        assert!(json.contains("\"count\": 2"));
        assert!(json.contains("536870912")); // 512 MB
        assert!(json.contains("10")); // vsock cid
    }
}
