use super::{AgentHypervisor, VmConfig, VmmError};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

// HCS handle types (opaque pointers)
type HcsComputeSystem = isize;
type HcsOperation = isize;

// FFI function signatures from vmcompute.dll
type HcsCreateComputeSystemFn = unsafe extern "system" fn(
    id: *const u16,
    configuration: *const u16,
    security_descriptor: *mut std::ffi::c_void,
    compute_system: *mut HcsComputeSystem,
    operation: *mut HcsOperation,
) -> i32;

type HcsStartComputeSystemFn = unsafe extern "system" fn(
    compute_system: HcsComputeSystem,
    options: *const u16,
    operation: *mut HcsOperation,
) -> i32;

type HcsShutDownComputeSystemFn = unsafe extern "system" fn(
    compute_system: HcsComputeSystem,
    options: *const u16,
    operation: *mut HcsOperation,
) -> i32;

type HcsTerminateComputeSystemFn = unsafe extern "system" fn(
    compute_system: HcsComputeSystem,
    options: *const u16,
    operation: *mut HcsOperation,
) -> i32;

type HcsPauseComputeSystemFn = unsafe extern "system" fn(
    compute_system: HcsComputeSystem,
    options: *const u16,
    operation: *mut HcsOperation,
) -> i32;

type HcsResumeComputeSystemFn = unsafe extern "system" fn(
    compute_system: HcsComputeSystem,
    options: *const u16,
    operation: *mut HcsOperation,
) -> i32;

type HcsCloseComputeSystemFn = unsafe extern "system" fn(compute_system: HcsComputeSystem);

type HcsWaitForOperationResultFn = unsafe extern "system" fn(
    operation: HcsOperation,
    timeout_ms: u32,
    result_document: *mut *mut u16,
) -> i32;

type HcsCloseOperationFn = unsafe extern "system" fn(operation: HcsOperation);

type HcsModifyComputeSystemFn = unsafe extern "system" fn(
    compute_system: HcsComputeSystem,
    configuration: *const u16,
    operation: *mut HcsOperation,
) -> i32;

// HCS error codes
const S_OK: i32 = 0;

/// Dynamically loaded HCS API functions.
struct HcsApi {
    create: HcsCreateComputeSystemFn,
    start: HcsStartComputeSystemFn,
    shutdown: HcsShutDownComputeSystemFn,
    terminate: HcsTerminateComputeSystemFn,
    pause: HcsPauseComputeSystemFn,
    resume: HcsResumeComputeSystemFn,
    close: HcsCloseComputeSystemFn,
    wait: HcsWaitForOperationResultFn,
    close_op: HcsCloseOperationFn,
    modify: HcsModifyComputeSystemFn,
}

unsafe impl Send for HcsApi {}
unsafe impl Sync for HcsApi {}

/// Loads the HCS API from vmcompute.dll. Returns None if the DLL is not available.
fn load_hcs_api() -> Option<HcsApi> {
    use windows::core::w;
    use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};

    // SAFETY: LoadLibraryW loads vmcompute.dll, a system DLL. The returned HMODULE
    // is valid until FreeLibrary is called. GetProcAddress returns function pointers
    // that remain valid for the lifetime of the loaded DLL.
    unsafe {
        let dll = LoadLibraryW(w!("vmcompute.dll")).ok()?;

        let create = GetProcAddress(dll, windows::core::s!("HcsCreateComputeSystem"));
        let start = GetProcAddress(dll, windows::core::s!("HcsStartComputeSystem"));
        let shutdown = GetProcAddress(dll, windows::core::s!("HcsShutDownComputeSystem"));
        let terminate = GetProcAddress(dll, windows::core::s!("HcsTerminateComputeSystem"));
        let pause = GetProcAddress(dll, windows::core::s!("HcsPauseComputeSystem"));
        let resume = GetProcAddress(dll, windows::core::s!("HcsResumeComputeSystem"));
        let close = GetProcAddress(dll, windows::core::s!("HcsCloseComputeSystem"));
        let wait = GetProcAddress(dll, windows::core::s!("HcsWaitForOperationResult"));
        let close_op = GetProcAddress(dll, windows::core::s!("HcsCloseOperation"));
        let modify = GetProcAddress(dll, windows::core::s!("HcsModifyComputeSystem"));

        // All functions must be present
        let create = create?;
        let start = start?;
        let shutdown = shutdown?;
        let terminate = terminate?;
        let pause = pause?;
        let resume = resume?;
        let close = close?;
        let wait = wait?;
        let close_op = close_op?;
        let modify = modify?;

        // Don't FreeLibrary — we need the DLL loaded for the process lifetime.
        // HMODULE is Copy so this intentionally ignores it without dropping.
        let _ = dll;

        Some(HcsApi {
            create: std::mem::transmute::<
                unsafe extern "system" fn() -> isize,
                HcsCreateComputeSystemFn,
            >(create),
            start: std::mem::transmute::<
                unsafe extern "system" fn() -> isize,
                HcsStartComputeSystemFn,
            >(start),
            shutdown: std::mem::transmute::<
                unsafe extern "system" fn() -> isize,
                HcsShutDownComputeSystemFn,
            >(shutdown),
            terminate: std::mem::transmute::<
                unsafe extern "system" fn() -> isize,
                HcsTerminateComputeSystemFn,
            >(terminate),
            pause: std::mem::transmute::<
                unsafe extern "system" fn() -> isize,
                HcsPauseComputeSystemFn,
            >(pause),
            resume: std::mem::transmute::<
                unsafe extern "system" fn() -> isize,
                HcsResumeComputeSystemFn,
            >(resume),
            close: std::mem::transmute::<
                unsafe extern "system" fn() -> isize,
                HcsCloseComputeSystemFn,
            >(close),
            wait: std::mem::transmute::<
                unsafe extern "system" fn() -> isize,
                HcsWaitForOperationResultFn,
            >(wait),
            close_op: std::mem::transmute::<
                unsafe extern "system" fn() -> isize,
                HcsCloseOperationFn,
            >(close_op),
            modify: std::mem::transmute::<
                unsafe extern "system" fn() -> isize,
                HcsModifyComputeSystemFn,
            >(modify),
        })
    }
}

/// Cached HCS API — loaded once on first use.
static HCS_API: OnceLock<Option<HcsApi>> = OnceLock::new();

fn get_hcs_api() -> Result<&'static HcsApi, VmmError> {
    HCS_API
        .get_or_init(load_hcs_api)
        .as_ref()
        .ok_or_else(|| VmmError::UnsupportedPlatform("vmcompute.dll not available".into()))
}

/// Windows Host Compute Service (HCS) backend.
///
/// Uses HCS Schema v2 JSON to create lightweight utility VMs with hardware-level
/// isolation via Hyper-V. Maps vsock to hvsock (AF_HYPERV) for host-guest IPC.
///
/// This is a Tier 1 backend on Windows when Hyper-V is available.
pub struct HcsBackend {
    compute_system: Option<HcsComputeSystem>,
    vsock_port: u32,
    config: Option<VmConfig>,
}

impl HcsBackend {
    pub fn new() -> Self {
        Self {
            compute_system: None,
            vsock_port: 0,
            config: None,
        }
    }

    /// Checks if Hyper-V is enabled by looking for the vmcompute service.
    fn is_hyper_v_available() -> bool {
        use std::process::Command;
        let output = Command::new("sc").args(["query", "vmcompute"]).output();
        match output {
            Ok(o) => {
                let stdout = String::from_utf8_lossy(&o.stdout);
                stdout.contains("RUNNING")
            }
            Err(_) => false,
        }
    }

    fn build_hcs_schema(config: &VmConfig) -> String {
        format!(
            r#"{{
            "Owner": "SavantSandbox",
            "SchemaVersion": {{"Major": 2, "Minor": 0}},
            "ShouldTerminateOnLastHandleClosed": true,
            "VirtualMachine": {{
                "Chipset": {{"Uefi": {{}}}},
                "ComputeTopology": {{
                    "Processor": {{
                        "Count": {},
                        "ExposeVirtualizationExtensions": true
                    }},
                    "Memory": {{
                        "SizeInMB": {},
                        "AllowOvercommit": false,
                        "EnableEpf": true
                    }}
                }},
                "Devices": {{
                    "VirtioConsole": [{{"Name": "SavantConsole"}}],
                    "Vsock": [{{"Name": "SavantVsock", "Port": {}}}]
                }},
                "GuestState": {{
                    "GuestStateFilePath": "{}"
                }}
            }}
        }}"#,
            config.cpu_count,
            config.memory_mb,
            config.vsock_cid,
            config.initrd_path.display(),
        )
    }

    /// Waits for an HCS operation to complete and returns the result document.
    /// Closes the operation handle after completion.
    fn wait_for_operation(api: &HcsApi, operation: HcsOperation) -> Result<(), VmmError> {
        if operation == 0 {
            return Ok(());
        }

        let mut result_ptr: *mut u16 = std::ptr::null_mut();
        // SAFETY: HcsWaitForOperationResult is called with a valid operation handle
        // obtained from a prior HCS API call. The timeout of 30000ms (30s) is a
        // reasonable bound for VM operations. result_ptr is initialized to null and
        // will be set by the API on success.
        let hr = unsafe { (api.wait)(operation, 30000, &mut result_ptr) };

        // SAFETY: HcsCloseOperation closes the operation handle which is no longer needed.
        unsafe { (api.close_op)(operation) };

        if hr == S_OK {
            // The result document was allocated by the HCS runtime. We intentionally
            // do not free it — the HCS runtime manages its own memory for result
            // documents, and the sandbox process lifetime is bounded.
            let _ = result_ptr;
            Ok(())
        } else {
            Err(VmmError::HcsOperationFailed(format!(
                "HCS operation failed with HRESULT 0x{:08X}",
                hr
            )))
        }
    }
}

impl Default for HcsBackend {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl AgentHypervisor for HcsBackend {
    async fn boot(&mut self, config: &VmConfig) -> Result<(), VmmError> {
        if !Self::is_hyper_v_available() {
            return Err(VmmError::UnsupportedPlatform(
                "Hyper-V is not enabled. Enable it via Windows Features or: \
                 dism /online /enable-feature /featurename:Microsoft-Hyper-V /all"
                    .into(),
            ));
        }

        let api = get_hcs_api()?;
        let schema = Self::build_hcs_schema(config);
        let system_id = format!("SavantSandbox_{}", std::process::id());

        let id_wide: Vec<u16> = system_id.encode_utf16().chain(std::iter::once(0)).collect();
        let schema_wide: Vec<u16> = schema.encode_utf16().chain(std::iter::once(0)).collect();

        let mut compute_system: HcsComputeSystem = 0;
        let mut operation: HcsOperation = 0;

        // SAFETY: HcsCreateComputeSystem is called with valid null-terminated UTF-16
        // strings for id and configuration. Security descriptor is null (default).
        // The output pointers are valid local variables.
        let hr = unsafe {
            (api.create)(
                id_wide.as_ptr(),
                schema_wide.as_ptr(),
                std::ptr::null_mut(),
                &mut compute_system,
                &mut operation,
            )
        };

        if hr != S_OK {
            return Err(VmmError::HcsOperationFailed(format!(
                "HcsCreateComputeSystem failed with HRESULT 0x{:08X}",
                hr
            )));
        }

        Self::wait_for_operation(api, operation)?;

        // Start the compute system
        let mut start_op: HcsOperation = 0;
        // SAFETY: HcsStartComputeSystem is called with a valid compute system handle
        // obtained from HcsCreateComputeSystem. Options is null (default startup).
        let hr = unsafe { (api.start)(compute_system, std::ptr::null(), &mut start_op) };

        if hr != S_OK {
            // Clean up the compute system on start failure
            // SAFETY: HcsCloseComputeSystem releases the handle. Called before dropping.
            unsafe { (api.close)(compute_system) }
            return Err(VmmError::HcsOperationFailed(format!(
                "HcsStartComputeSystem failed with HRESULT 0x{:08X}",
                hr
            )));
        }

        Self::wait_for_operation(api, start_op)?;

        tracing::info!(
            "HCS backend: booted compute system '{}' ({} CPU, {} MB RAM)",
            system_id,
            config.cpu_count,
            config.memory_mb
        );

        self.compute_system = Some(compute_system);
        self.vsock_port = config.vsock_cid;
        self.config = Some(config.clone());

        Ok(())
    }

    async fn pause(&mut self) -> Result<(), VmmError> {
        let cs = self
            .compute_system
            .ok_or_else(|| VmmError::Vm("no compute system running".into()))?;

        let api = get_hcs_api()?;
        let mut operation: HcsOperation = 0;
        // SAFETY: HcsPauseComputeSystem is called with a valid compute system handle.
        let hr = unsafe { (api.pause)(cs, std::ptr::null(), &mut operation) };

        if hr != S_OK {
            return Err(VmmError::HcsOperationFailed(format!(
                "HcsPauseComputeSystem failed with HRESULT 0x{:08X}",
                hr
            )));
        }

        Self::wait_for_operation(api, operation)?;
        tracing::info!("HCS backend: paused compute system");
        Ok(())
    }

    async fn resume(&mut self) -> Result<(), VmmError> {
        let cs = self
            .compute_system
            .ok_or_else(|| VmmError::Vm("no compute system running".into()))?;

        let api = get_hcs_api()?;
        let mut operation: HcsOperation = 0;
        // SAFETY: HcsResumeComputeSystem is called with a valid compute system handle.
        let hr = unsafe { (api.resume)(cs, std::ptr::null(), &mut operation) };

        if hr != S_OK {
            return Err(VmmError::HcsOperationFailed(format!(
                "HcsResumeComputeSystem failed with HRESULT 0x{:08X}",
                hr
            )));
        }

        Self::wait_for_operation(api, operation)?;
        tracing::info!("HCS backend: resumed compute system");
        Ok(())
    }

    async fn shutdown(&mut self) -> Result<(), VmmError> {
        if let Some(cs) = self.compute_system {
            let api = get_hcs_api()?;

            // Try graceful shutdown first
            let mut operation: HcsOperation = 0;
            // SAFETY: HcsShutDownComputeSystem initiates a graceful shutdown.
            let hr = unsafe { (api.shutdown)(cs, std::ptr::null(), &mut operation) };

            if hr == S_OK {
                if let Err(e) = Self::wait_for_operation(api, operation) {
                    tracing::warn!("HCS graceful shutdown failed, forcing terminate: {}", e);
                    // Force terminate if graceful shutdown fails
                    let mut term_op: HcsOperation = 0;
                    // SAFETY: HcsTerminateComputeSystem force-kills the VM.
                    let term_hr = unsafe { (api.terminate)(cs, std::ptr::null(), &mut term_op) };
                    if term_hr == S_OK {
                        let _ = Self::wait_for_operation(api, term_op);
                    }
                }
            } else {
                // Graceful shutdown couldn't be initiated, force terminate
                tracing::warn!(
                    "HcsShutDownComputeSystem failed (0x{:08X}), forcing terminate",
                    hr
                );
                let mut term_op: HcsOperation = 0;
                // SAFETY: HcsTerminateComputeSystem force-kills the VM.
                let term_hr = unsafe { (api.terminate)(cs, std::ptr::null(), &mut term_op) };
                if term_hr == S_OK {
                    let _ = Self::wait_for_operation(api, term_op);
                }
            }

            // Close the handle
            // SAFETY: HcsCloseComputeSystem releases the compute system handle.
            unsafe { (api.close)(cs) }
            tracing::info!("HCS backend: compute system shut down");
        }

        self.compute_system = None;
        self.config = None;
        Ok(())
    }

    fn vsock_port(&self) -> u32 {
        self.vsock_port
    }

    async fn add_virtio_blk(&mut self, path: &Path, readonly: bool) -> Result<(), VmmError> {
        let cs = self
            .compute_system
            .ok_or_else(|| VmmError::Vm("no compute system running".into()))?;

        let api = get_hcs_api()?;

        // HCS ModifyComputeSystem JSON for adding a virtio-blk device
        let modify_json = format!(
            r#"{{
            "ResourcePath": "VirtualMachine/Devices/VirtioBlk",
            "RequestType": "Add",
            "Settings": {{
                "Path": "{}",
                "ReadOnly": {}
            }}
        }}"#,
            path.display().to_string().replace('\\', "\\\\"),
            readonly
        );

        let modify_wide: Vec<u16> = modify_json
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        let mut operation: HcsOperation = 0;
        // SAFETY: HcsModifyComputeSystem is called with a valid handle and JSON config.
        let hr = unsafe { (api.modify)(cs, modify_wide.as_ptr(), &mut operation) };

        if hr != S_OK {
            return Err(VmmError::HcsOperationFailed(format!(
                "HcsModifyComputeSystem (virtio-blk) failed with HRESULT 0x{:08X}",
                hr
            )));
        }

        Self::wait_for_operation(api, operation)?;
        tracing::info!(
            "HCS backend: added virtio-blk device: {} (readonly={})",
            path.display(),
            readonly
        );
        Ok(())
    }

    async fn add_virtio_net(&mut self, tap_name: &str) -> Result<(), VmmError> {
        let cs = self
            .compute_system
            .ok_or_else(|| VmmError::Vm("no compute system running".into()))?;

        let api = get_hcs_api()?;

        // HCS ModifyComputeSystem JSON for adding a network endpoint
        let modify_json = format!(
            r#"{{
            "ResourcePath": "VirtualMachine/Devices/Network",
            "RequestType": "Add",
            "Settings": {{
                "EndpointId": "{}",
                "MacAddress": ""
            }}
        }}"#,
            tap_name
        );

        let modify_wide: Vec<u16> = modify_json
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        let mut operation: HcsOperation = 0;
        // SAFETY: HcsModifyComputeSystem is called with a valid handle and JSON config.
        let hr = unsafe { (api.modify)(cs, modify_wide.as_ptr(), &mut operation) };

        if hr != S_OK {
            return Err(VmmError::HcsOperationFailed(format!(
                "HcsModifyComputeSystem (network) failed with HRESULT 0x{:08X}",
                hr
            )));
        }

        Self::wait_for_operation(api, operation)?;
        tracing::info!("HCS backend: added virtio-net device: {}", tap_name);
        Ok(())
    }

    async fn forensic_snapshot(&self) -> Result<PathBuf, VmmError> {
        let snapshot_dir = std::env::temp_dir().join(format!(
            "savant_forensic_hcs_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs()
        ));
        std::fs::create_dir_all(&snapshot_dir)
            .map_err(|e| VmmError::Io(format!("failed to create snapshot dir: {}", e)))?;

        // Write configuration state
        let info = format!("backend=hcs\nvsock_port={}\n", self.vsock_port);
        std::fs::write(snapshot_dir.join("hcs_info.txt"), info)
            .map_err(|e| VmmError::Io(format!("failed to write snapshot: {}", e)))?;

        if let Some(ref config) = self.config {
            let config_info = format!(
                "cpu_count={}\nmemory_mb={}\nvsock_cid={}\ninitrd={}\n",
                config.cpu_count,
                config.memory_mb,
                config.vsock_cid,
                config.initrd_path.display()
            );
            std::fs::write(snapshot_dir.join("vm_config.txt"), config_info)
                .map_err(|e| VmmError::Io(format!("failed to write config: {}", e)))?;
        }

        Ok(snapshot_dir)
    }

    fn backend_name(&self) -> &'static str {
        "hcs"
    }

    fn is_available() -> bool {
        Self::is_hyper_v_available() && load_hcs_api().is_some()
    }
}

impl Drop for HcsBackend {
    fn drop(&mut self) {
        if let Some(cs) = self.compute_system {
            if let Some(api) = HCS_API.get().and_then(|a| a.as_ref()) {
                // SAFETY: HcsCloseComputeSystem releases the handle on drop.
                // This is a best-effort cleanup — errors are logged but not propagated
                // since we're in a Drop impl.
                unsafe { (api.close)(cs) }
                tracing::debug!("HCS backend: closed compute system on drop");
            }
        }
    }
}
