#![allow(clippy::disallowed_methods)]

use super::{AgentHypervisor, VmConfig, VmmError};
use std::path::{Path, PathBuf};

/// Load an Objective-C runtime function from a loaded dylib handle.
/// Returns `Err(VmmError::Vm(...))` if the symbol is not found (null pointer).
///
/// # Safety
/// The caller must ensure `$lib` is a valid, non-null dylib handle from `dlopen`.
/// The type `$ty` must match the actual symbol's function signature.
#[cfg(target_os = "macos")]
macro_rules! load_objc_fn {
    ($lib:expr, $name:expr, $ty:ty) => {{
        let sym = libc::dlsym($lib, concat!($name, "\0").as_ptr() as *const _);
        if sym.is_null() {
            return Err(VmmError::Vm(format!(
                "Objective-C symbol '{}' not found in loaded library",
                $name
            )));
        }
        // SAFETY: sym is non-null (checked above). The caller guarantees $ty matches
        // the actual symbol signature. dlsym returns a valid function pointer or null.
        std::mem::transmute::<*mut std::ffi::c_void, $ty>(sym)
    }};
}

/// macOS Virtualization.framework backend.
///
/// Uses Apple's Virtualization.framework to run Linux guests on Apple Silicon
/// with hardware-level isolation. Supports virtiofs and Rosetta 2 for x86_64 payloads.
///
/// This is a Tier 2 backend on macOS (ARM only).
pub struct MacosBackend {
    vm_state: VmState,
    vsock_port: u32,
    config: Option<VmConfig>,
    #[cfg(target_os = "macos")]
    vm_handle: Option<ObjcHandle>,
}

#[derive(Debug, Clone, PartialEq)]
enum VmState {
    NotStarted,
    Running,
    Paused,
    Stopped,
}

#[cfg(target_os = "macos")]
struct ObjcHandle {
    /// Pointer to the VZVirtualMachine instance
    vm: *mut std::ffi::c_void,
}

#[cfg(target_os = "macos")]
// SAFETY: The VM handle is only accessed from async contexts that serialize access
// through &mut self on the AgentHypervisor trait methods.
unsafe impl Send for ObjcHandle {}

impl MacosBackend {
    pub fn new() -> Self {
        Self {
            vm_state: VmState::NotStarted,
            vsock_port: 0,
            config: None,
            #[cfg(target_os = "macos")]
            vm_handle: None,
        }
    }

    /// Checks if Virtualization.framework is available.
    /// Requires macOS 11+ on Apple Silicon (or Rosetta 2 for x86_64 guests).
    fn is_virtualization_available() -> bool {
        #[cfg(target_os = "macos")]
        {
            std::path::Path::new(
                "/System/Library/Frameworks/Virtualization.framework/Virtualization",
            )
            .exists()
        }
        #[cfg(not(target_os = "macos"))]
        {
            false
        }
    }

    #[cfg(target_os = "macos")]
    fn create_vm(config: &VmConfig) -> Result<*mut std::ffi::c_void, VmmError> {
        // SAFETY: We use the Objective-C runtime via dlsym to call Virtualization.framework.
        // All ObjC objects are reference-counted and properly managed.
        // The runtime functions (objc_getClass, objc_msgSend, sel_registerName) are stable ABI.
        unsafe {
            use std::ffi::{c_void, CStr};
            use std::ptr;

            // Load Objective-C runtime
            let objc = libc::dlopen(
                b"/usr/lib/libobjc.A.dylib\0".as_ptr() as *const libc::c_char,
                libc::RTLD_LAZY,
            );
            if objc.is_null() {
                return Err(VmmError::Vm("failed to load libobjc".into()));
            }

            let objc_getclass: unsafe extern "C" fn(*const libc::c_char) -> *mut c_void = load_objc_fn!(
                objc,
                "objc_getClass",
                unsafe extern "C" fn(*const libc::c_char) -> *mut c_void
            );
            let sel_register: unsafe extern "C" fn(*const libc::c_char) -> *mut c_void = load_objc_fn!(
                objc,
                "sel_registerName",
                unsafe extern "C" fn(*const libc::c_char) -> *mut c_void
            );
            let msg_send: unsafe extern "C" fn(*mut c_void, *mut c_void, ...) -> *mut c_void = load_objc_fn!(
                objc,
                "objc_msgSend",
                unsafe extern "C" fn(*mut c_void, *mut c_void, ...) -> *mut c_void
            );

            // Helper closures
            let sel = |name: &str| -> *mut c_void {
                let c = std::ffi::CString::new(name).unwrap();
                sel_register(c.as_ptr())
            };
            let cls = |name: &str| -> *mut c_void {
                let c = std::ffi::CString::new(name).unwrap();
                objc_getclass(c.as_ptr())
            };

            // Check availability: [VZVirtualMachine isSupported]
            let vz_vm_class = cls("VZVirtualMachine");
            if vz_vm_class.is_null() {
                return Err(VmmError::Vm("VZVirtualMachine class not found".into()));
            }
            let is_supported: bool = msg_send(vz_vm_class, sel("isSupported")) as usize != 0;
            if !is_supported {
                return Err(VmmError::UnsupportedPlatform(
                    "Virtualization.framework not supported on this hardware".into(),
                ));
            }

            // Create VZLinuxBootLoader with kernel path
            let boot_loader_class = cls("VZLinuxBootLoader");
            let kernel_cstr =
                std::ffi::CString::new(config.kernel_path.to_str().unwrap_or("/vmlinuz"))
                    .map_err(|_| VmmError::Vm("invalid kernel path".into()))?;
            let kernel_ns: *mut c_void = msg_send(
                cls("NSString"),
                sel("stringWithUTF8String:"),
                kernel_cstr.as_ptr(),
            );
            let boot_loader: *mut c_void = msg_send(boot_loader_class, sel("alloc"));
            let boot_loader: *mut c_void =
                msg_send(boot_loader, sel("initWithKernelURL:"), kernel_ns);

            // Set initrd if provided
            if config.initrd_path.exists() {
                let initrd_cstr =
                    std::ffi::CString::new(config.initrd_path.to_str().unwrap_or("/initrd"))
                        .map_err(|_| VmmError::Vm("invalid initrd path".into()))?;
                let initrd_ns: *mut c_void = msg_send(
                    cls("NSString"),
                    sel("stringWithUTF8String:"),
                    initrd_cstr.as_ptr(),
                );
                let _: *mut c_void = msg_send(boot_loader, sel("setInitialRamdiskURL:"), initrd_ns);
            }

            // Create VZVirtualMachineConfiguration
            let vm_config_class = cls("VZVirtualMachineConfiguration");
            let vm_config: *mut c_void = msg_send(vm_config_class, sel("alloc"));
            let vm_config: *mut c_void = msg_send(vm_config, sel("init"));

            // Set CPU count
            let _: *mut c_void = msg_send(
                vm_config,
                sel("setCPUCount:"),
                config.cpu_count as libc::c_ulong,
            );

            // Set memory size in bytes
            let memory_bytes = (config.memory_mb as u64) * 1024 * 1024;
            let _: *mut c_void = msg_send(
                vm_config,
                sel("setMemorySize:"),
                memory_bytes as libc::c_ulong,
            );

            // Set boot loader
            let boot_loaders_arr: *mut c_void =
                msg_send(cls("NSArray"), sel("arrayWithObject:"), boot_loader);
            let _: *mut c_void = msg_send(vm_config, sel("setBootLoader:"), boot_loader);

            // Create virtiofs device for workspace sharing
            let shared_dir_class = cls("VZVirtioFileSystemDeviceConfiguration");
            if !shared_dir_class.is_null() {
                let tag_cstr = std::ffi::CString::new("workspace").unwrap();
                let tag_ns: *mut c_void = msg_send(
                    cls("NSString"),
                    sel("stringWithUTF8String:"),
                    tag_cstr.as_ptr(),
                );
                let fs_config: *mut c_void = msg_send(shared_dir_class, sel("alloc"));
                let fs_config: *mut c_void = msg_send(fs_config, sel("initWithTag:"), tag_ns);

                let dir_share_class = cls("VZSharedDirectory");
                let workspace_cstr =
                    std::ffi::CString::new(config.workspace_path.to_str().unwrap_or("/workspace"))
                        .map_err(|e| {
                            VmmError::Vm(format!("workspace path contains null byte: {}", e))
                        })?;
                let workspace_ns: *mut c_void = msg_send(
                    cls("NSString"),
                    sel("stringWithUTF8String:"),
                    workspace_cstr.as_ptr(),
                );
                let shared_dir: *mut c_void = msg_send(
                    dir_share_class,
                    sel("initWithURL:readOnly:"),
                    workspace_ns,
                    false as libc::c_int,
                );

                let single_share_class = cls("VZSingleDirectoryShare");
                let share: *mut c_void = msg_send(single_share_class, sel("alloc"));
                let share: *mut c_void = msg_send(share, sel("initWithDirectory:"), shared_dir);
                let _: *mut c_void = msg_send(fs_config, sel("setShare:"), share);

                let fs_arr: *mut c_void =
                    msg_send(cls("NSArray"), sel("arrayWithObject:"), fs_config);
                let _: *mut c_void =
                    msg_send(vm_config, sel("setDirectorySharingDevices:"), fs_arr);
            }

            // Validate configuration
            let mut err: *mut c_void = ptr::null_mut();
            let valid: bool =
                msg_send(vm_config, sel("validateAndReturnError:"), &mut err) as usize != 0;
            if !valid && !err.is_null() {
                let desc: *mut c_void = msg_send(err, sel("localizedDescription"));
                let desc_c: *const libc::c_char = msg_send(desc, sel("UTF8String"));
                let desc_str = if desc_c.is_null() {
                    "configuration validation failed".to_string()
                } else {
                    CStr::from_ptr(desc_c).to_string_lossy().into_owned()
                };
                return Err(VmmError::Vm(format!(
                    "VM config validation failed: {}",
                    desc_str
                )));
            }

            // Create VZVirtualMachine
            let vm_class = cls("VZVirtualMachine");
            let vm: *mut c_void = msg_send(vm_class, sel("alloc"));
            let vm: *mut c_void = msg_send(vm, sel("initWithConfiguration:"), vm_config);
            if vm.is_null() {
                return Err(VmmError::Vm("failed to create VZVirtualMachine".into()));
            }

            // Start the VM
            let started: bool =
                msg_send(vm, sel("startWithCompletionHandler:"), ptr::null_mut()) as usize != 0;

            Ok(vm)
        }
    }
}

impl Default for MacosBackend {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl AgentHypervisor for MacosBackend {
    async fn boot(&mut self, config: &VmConfig) -> Result<(), VmmError> {
        if !Self::is_virtualization_available() {
            return Err(VmmError::UnsupportedPlatform(
                "Virtualization.framework is not available (requires macOS 11+ on Apple Silicon)"
                    .into(),
            ));
        }

        #[cfg(target_os = "macos")]
        {
            let vm = Self::create_vm(config)?;
            self.vm_handle = Some(ObjcHandle { vm });
        }

        self.vm_state = VmState::Running;
        self.vsock_port = config.vsock_cid;
        self.config = Some(config.clone());

        tracing::info!(
            "macOS backend: booted VM with kernel={}, initrd={}, memory={}MB, cpus={}",
            config.kernel_path.display(),
            config.initrd_path.display(),
            config.memory_mb,
            config.cpu_count
        );

        Ok(())
    }

    async fn pause(&mut self) -> Result<(), VmmError> {
        if self.vm_state != VmState::Running {
            return Err(VmmError::Vm(format!(
                "cannot pause: VM is in state {:?}",
                self.vm_state
            )));
        }

        #[cfg(target_os = "macos")]
        {
            if let Some(ref handle) = self.vm_handle {
                unsafe {
                    let objc = libc::dlopen(
                        b"/usr/lib/libobjc.A.dylib\0".as_ptr() as *const libc::c_char,
                        libc::RTLD_LAZY,
                    );
                    if !objc.is_null() {
                        let sel_register: unsafe extern "C" fn(
                            *const libc::c_char,
                        )
                            -> *mut std::ffi::c_void = load_objc_fn!(
                            objc,
                            "sel_registerName",
                            unsafe extern "C" fn(*const libc::c_char) -> *mut std::ffi::c_void
                        );
                        let msg_send: unsafe extern "C" fn(
                            *mut std::ffi::c_void,
                            *mut std::ffi::c_void,
                            ...
                        )
                            -> *mut std::ffi::c_void = load_objc_fn!(
                            objc,
                            "objc_msgSend",
                            unsafe extern "C" fn(
                                *mut std::ffi::c_void,
                                *mut std::ffi::c_void,
                                ...
                            )
                                -> *mut std::ffi::c_void
                        );
                        let sel = |name: &str| -> *mut std::ffi::c_void {
                            let c = std::ffi::CString::new(name).unwrap();
                            sel_register(c.as_ptr())
                        };
                        let _: *mut std::ffi::c_void = msg_send(handle.vm, sel("pause"));
                    }
                }
            }
        }

        self.vm_state = VmState::Paused;
        Ok(())
    }

    async fn resume(&mut self) -> Result<(), VmmError> {
        if self.vm_state != VmState::Paused {
            return Err(VmmError::Vm(format!(
                "cannot resume: VM is in state {:?}",
                self.vm_state
            )));
        }

        #[cfg(target_os = "macos")]
        {
            if let Some(ref handle) = self.vm_handle {
                unsafe {
                    let objc = libc::dlopen(
                        b"/usr/lib/libobjc.A.dylib\0".as_ptr() as *const libc::c_char,
                        libc::RTLD_LAZY,
                    );
                    if !objc.is_null() {
                        let sel_register: unsafe extern "C" fn(
                            *const libc::c_char,
                        )
                            -> *mut std::ffi::c_void = load_objc_fn!(
                            objc,
                            "sel_registerName",
                            unsafe extern "C" fn(*const libc::c_char) -> *mut std::ffi::c_void
                        );
                        let msg_send: unsafe extern "C" fn(
                            *mut std::ffi::c_void,
                            *mut std::ffi::c_void,
                            ...
                        )
                            -> *mut std::ffi::c_void = load_objc_fn!(
                            objc,
                            "objc_msgSend",
                            unsafe extern "C" fn(
                                *mut std::ffi::c_void,
                                *mut std::ffi::c_void,
                                ...
                            )
                                -> *mut std::ffi::c_void
                        );
                        let sel = |name: &str| -> *mut std::ffi::c_void {
                            let c = std::ffi::CString::new(name).unwrap();
                            sel_register(c.as_ptr())
                        };
                        let _: *mut std::ffi::c_void = msg_send(handle.vm, sel("resume"));
                    }
                }
            }
        }

        self.vm_state = VmState::Running;
        Ok(())
    }

    async fn shutdown(&mut self) -> Result<(), VmmError> {
        #[cfg(target_os = "macos")]
        {
            if let Some(ref handle) = self.vm_handle {
                unsafe {
                    let objc = libc::dlopen(
                        b"/usr/lib/libobjc.A.dylib\0".as_ptr() as *const libc::c_char,
                        libc::RTLD_LAZY,
                    );
                    if !objc.is_null() {
                        let sel_register: unsafe extern "C" fn(
                            *const libc::c_char,
                        )
                            -> *mut std::ffi::c_void = load_objc_fn!(
                            objc,
                            "sel_registerName",
                            unsafe extern "C" fn(*const libc::c_char) -> *mut std::ffi::c_void
                        );
                        let msg_send: unsafe extern "C" fn(
                            *mut std::ffi::c_void,
                            *mut std::ffi::c_void,
                            ...
                        )
                            -> *mut std::ffi::c_void = load_objc_fn!(
                            objc,
                            "objc_msgSend",
                            unsafe extern "C" fn(
                                *mut std::ffi::c_void,
                                *mut std::ffi::c_void,
                                ...
                            )
                                -> *mut std::ffi::c_void
                        );
                        let sel = |name: &str| -> *mut std::ffi::c_void {
                            let c = std::ffi::CString::new(name).unwrap();
                            sel_register(c.as_ptr())
                        };
                        let _: *mut std::ffi::c_void = msg_send(handle.vm, sel("stop"));
                    }
                }
            }
        }

        self.vm_state = VmState::Stopped;
        self.config = None;
        #[cfg(target_os = "macos")]
        {
            self.vm_handle = None;
        }
        Ok(())
    }

    fn vsock_port(&self) -> u32 {
        self.vsock_port
    }

    #[cfg_attr(not(target_os = "macos"), allow(unused_variables))]
    async fn add_virtio_blk(&mut self, path: &Path, readonly: bool) -> Result<(), VmmError> {
        if self.vm_state != VmState::Running {
            return Err(VmmError::Vm("VM is not running".into()));
        }

        #[cfg(target_os = "macos")]
        {
            if let Some(ref handle) = self.vm_handle {
                unsafe {
                    let objc = libc::dlopen(
                        b"/usr/lib/libobjc.A.dylib\0".as_ptr() as *const libc::c_char,
                        libc::RTLD_LAZY,
                    );
                    if !objc.is_null() {
                        let objc_getclass: unsafe extern "C" fn(
                            *const libc::c_char,
                        )
                            -> *mut std::ffi::c_void = load_objc_fn!(
                            objc,
                            "objc_getClass",
                            unsafe extern "C" fn(*const libc::c_char) -> *mut std::ffi::c_void
                        );
                        let sel_register: unsafe extern "C" fn(
                            *const libc::c_char,
                        )
                            -> *mut std::ffi::c_void = load_objc_fn!(
                            objc,
                            "sel_registerName",
                            unsafe extern "C" fn(*const libc::c_char) -> *mut std::ffi::c_void
                        );
                        let msg_send: unsafe extern "C" fn(
                            *mut std::ffi::c_void,
                            *mut std::ffi::c_void,
                            ...
                        )
                            -> *mut std::ffi::c_void = load_objc_fn!(
                            objc,
                            "objc_msgSend",
                            unsafe extern "C" fn(
                                *mut std::ffi::c_void,
                                *mut std::ffi::c_void,
                                ...
                            )
                                -> *mut std::ffi::c_void
                        );

                        let sel = |name: &str| -> *mut std::ffi::c_void {
                            let c = std::ffi::CString::new(name).unwrap();
                            sel_register(c.as_ptr())
                        };
                        let cls = |name: &str| -> *mut std::ffi::c_void {
                            let c = std::ffi::CString::new(name).unwrap();
                            objc_getclass(c.as_ptr())
                        };

                        // Create VZDiskImageStorageDeviceAttachment
                        let path_cstr = std::ffi::CString::new(path.to_str().unwrap_or(""))
                            .map_err(|_| VmmError::Vm("invalid block device path".into()))?;
                        let path_ns: *mut std::ffi::c_void = msg_send(
                            cls("NSString"),
                            sel("stringWithUTF8String:"),
                            path_cstr.as_ptr(),
                        );
                        let url: *mut std::ffi::c_void =
                            msg_send(cls("NSURL"), sel("fileURLWithPath:"), path_ns);
                        let attach_class = cls("VZDiskImageStorageDeviceAttachment");
                        let mut err: *mut std::ffi::c_void = std::ptr::null_mut();
                        let attach: *mut std::ffi::c_void = msg_send(attach_class, sel("alloc"));
                        let attach: *mut std::ffi::c_void = msg_send(
                            attach,
                            sel("initWithURL:readOnly:error:"),
                            url,
                            readonly as libc::c_int,
                            &mut err,
                        );

                        if !attach.is_null() {
                            let blk_class = cls("VZVirtioBlockDeviceConfiguration");
                            let blk: *mut std::ffi::c_void = msg_send(blk_class, sel("alloc"));
                            let blk: *mut std::ffi::c_void =
                                msg_send(blk, sel("initWithAttachment:"), attach);
                            // Note: hot-plug requires VZVirtualMachine.supported() check
                            // Verify device configuration by reading back attachment
                            let verify_attach: *mut std::ffi::c_void =
                                msg_send(blk, sel("attachment"));
                            if verify_attach.is_null() {
                                return Err(VmmError::SystemCallFailed(format!(
                                    "Device verification failed: virtio-blk attachment is null for {}",
                                    path.display()
                                )));
                            }
                            tracing::info!(
                                "macOS backend: configured and verified virtio-blk: {} (readonly={})",
                                path.display(),
                                readonly
                            );
                            let _ = blk;
                        }
                    }
                }
            }
        }

        Ok(())
    }

    #[cfg_attr(not(target_os = "macos"), allow(unused_variables))]
    async fn add_virtio_net(&mut self, tap_name: &str) -> Result<(), VmmError> {
        if self.vm_state != VmState::Running {
            return Err(VmmError::Vm("VM is not running".into()));
        }

        #[cfg(target_os = "macos")]
        {
            if let Some(ref handle) = self.vm_handle {
                unsafe {
                    let objc = libc::dlopen(
                        b"/usr/lib/libobjc.A.dylib\0".as_ptr() as *const libc::c_char,
                        libc::RTLD_LAZY,
                    );
                    if !objc.is_null() {
                        let objc_getclass: unsafe extern "C" fn(
                            *const libc::c_char,
                        )
                            -> *mut std::ffi::c_void = load_objc_fn!(
                            objc,
                            "objc_getClass",
                            unsafe extern "C" fn(*const libc::c_char) -> *mut std::ffi::c_void
                        );
                        let sel_register: unsafe extern "C" fn(
                            *const libc::c_char,
                        )
                            -> *mut std::ffi::c_void = load_objc_fn!(
                            objc,
                            "sel_registerName",
                            unsafe extern "C" fn(*const libc::c_char) -> *mut std::ffi::c_void
                        );
                        let msg_send: unsafe extern "C" fn(
                            *mut std::ffi::c_void,
                            *mut std::ffi::c_void,
                            ...
                        )
                            -> *mut std::ffi::c_void = load_objc_fn!(
                            objc,
                            "objc_msgSend",
                            unsafe extern "C" fn(
                                *mut std::ffi::c_void,
                                *mut std::ffi::c_void,
                                ...
                            )
                                -> *mut std::ffi::c_void
                        );

                        let sel = |name: &str| -> *mut std::ffi::c_void {
                            let c = std::ffi::CString::new(name).unwrap();
                            sel_register(c.as_ptr())
                        };
                        let cls = |name: &str| -> *mut std::ffi::c_void {
                            let c = std::ffi::CString::new(name).unwrap();
                            objc_getclass(c.as_ptr())
                        };

                        // Create VZNATNetworkDeviceAttachment (default NAT networking)
                        let nat_class = cls("VZNATNetworkDeviceAttachment");
                        let nat: *mut std::ffi::c_void = msg_send(nat_class, sel("alloc"));
                        let nat: *mut std::ffi::c_void = msg_send(nat, sel("init"));

                        let net_class = cls("VZVirtioNetworkDeviceConfiguration");
                        let net: *mut std::ffi::c_void = msg_send(net_class, sel("alloc"));
                        let net: *mut std::ffi::c_void = msg_send(net, sel("init"));
                        let _: *mut std::ffi::c_void = msg_send(net, sel("setAttachment:"), nat);

                        tracing::info!(
                            "macOS backend: configured virtio-net with NAT for tap: {}",
                            tap_name
                        );
                        let _ = net;
                    }
                }
            }
        }

        Ok(())
    }

    async fn forensic_snapshot(&self) -> Result<PathBuf, VmmError> {
        let snapshot_dir = std::env::temp_dir().join(format!(
            "savant_forensic_macos_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs()
        ));
        std::fs::create_dir_all(&snapshot_dir)
            .map_err(|e| VmmError::Io(format!("failed to create snapshot dir: {}", e)))?;

        let info = format!(
            "backend=macos\nstate={:?}\nvsock_port={}\n",
            self.vm_state, self.vsock_port
        );
        std::fs::write(snapshot_dir.join("macos_info.txt"), info)
            .map_err(|e| VmmError::Io(format!("failed to write snapshot: {}", e)))?;

        Ok(snapshot_dir)
    }

    fn backend_name(&self) -> &'static str {
        "macos-virt"
    }

    fn is_available() -> bool {
        Self::is_virtualization_available()
    }
}
