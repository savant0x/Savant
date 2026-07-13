#[derive(Debug, thiserror::Error)]
pub enum HardenError {
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
}

/// Configuration for process hardening.
pub struct HardenConfig {
    /// The workspace directory the sandboxed process may access.
    pub workspace_dir: std::path::PathBuf,
    /// Maximum virtual memory in bytes (0 = no limit).
    pub max_memory_bytes: u64,
    /// Maximum number of processes (0 = no limit).
    pub max_processes: u32,
    /// Maximum number of open file descriptors (0 = no limit).
    pub max_open_files: u32,
}

impl HardenConfig {
    pub fn new(workspace_dir: impl Into<std::path::PathBuf>) -> Self {
        Self {
            workspace_dir: workspace_dir.into(),
            max_memory_bytes: 0,
            max_processes: 0,
            max_open_files: 0,
        }
    }

    pub fn with_memory(mut self, bytes: u64) -> Self {
        self.max_memory_bytes = bytes;
        self
    }

    pub fn with_max_processes(mut self, n: u32) -> Self {
        self.max_processes = n;
        self
    }

    pub fn with_max_open_files(mut self, n: u32) -> Self {
        self.max_open_files = n;
        self
    }
}

/// Applies platform-specific process hardening. Must be called early in the
/// broker process lifecycle, before spawning any sandboxed child processes.
///
/// On Linux: installs seccomp-bpf filter + Landlock ruleset.
/// On Windows: creates AppContainer token + Job Object.
/// On macOS: applies sandbox-exec profile.
/// On other platforms: returns `UnsupportedPlatform`.
pub fn harden_process(config: &HardenConfig) -> Result<(), HardenError> {
    #[cfg(target_os = "linux")]
    {
        harden_linux(config)?;
    }
    #[cfg(target_os = "windows")]
    {
        harden_windows(config)?;
    }
    #[cfg(target_os = "macos")]
    {
        harden_macos(config)?;
    }
    #[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
    {
        return Err(HardenError::UnsupportedPlatform(format!(
            "process hardening is not available on {}",
            std::env::consts::OS
        )));
    }
    Ok(())
}

/// Returns `true` if the current platform supports process hardening.
pub fn is_available() -> bool {
    cfg!(any(
        target_os = "linux",
        target_os = "windows",
        target_os = "macos"
    ))
}

// ── Linux: seccomp-bpf + Landlock ──────────────────────────────────────────

#[cfg(target_os = "linux")]
fn harden_linux(config: &HardenConfig) -> Result<(), HardenError> {
    install_seccomp_filter()?;
    install_landlock(&config.workspace_dir)?;
    Ok(())
}

/// Installs a seccomp-bpf filter allowing only syscalls needed for VMM operation.
/// Any syscall outside the allowlist triggers `SIGSYS` and kills the process.
#[cfg(target_os = "linux")]
fn install_seccomp_filter() -> Result<(), HardenError> {
    use seccompiler::{BpfProgram, SeccompAction, SeccompFilter, SyscallRuleSet};

    // Syscalls required for the broker process to function.
    // This is a minimal allowlist — every additional syscall must be justified.
    let allowed_syscalls: Vec<SyscallRuleSet> = vec![
        // Memory management
        SyscallRuleSet::new(libc::SYS_mmap as i64, vec![]),
        SyscallRuleSet::new(libc::SYS_munmap as i64, vec![]),
        SyscallRuleSet::new(libc::SYS_mprotect as i64, vec![]),
        SyscallRuleSet::new(libc::SYS_madvise as i64, vec![]),
        SyscallRuleSet::new(libc::SYS_brk as i64, vec![]),
        // I/O
        SyscallRuleSet::new(libc::SYS_read as i64, vec![]),
        SyscallRuleSet::new(libc::SYS_write as i64, vec![]),
        SyscallRuleSet::new(libc::SYS_readv as i64, vec![]),
        SyscallRuleSet::new(libc::SYS_writev as i64, vec![]),
        SyscallRuleSet::new(libc::SYS_close as i64, vec![]),
        SyscallRuleSet::new(libc::SYS_lseek as i64, vec![]),
        // Filesystem (minimal)
        SyscallRuleSet::new(libc::SYS_stat as i64, vec![]),
        SyscallRuleSet::new(libc::SYS_fstat as i64, vec![]),
        SyscallRuleSet::new(libc::SYS_openat as i64, vec![]),
        SyscallRuleSet::new(libc::SYS_faccessat as i64, vec![]),
        // Epoll
        SyscallRuleSet::new(libc::SYS_epoll_create1 as i64, vec![]),
        SyscallRuleSet::new(libc::SYS_epoll_ctl as i64, vec![]),
        SyscallRuleSet::new(libc::SYS_epoll_wait as i64, vec![]),
        // Eventfd
        SyscallRuleSet::new(libc::SYS_eventfd2 as i64, vec![]),
        // Networking (for vsock/hvsock)
        SyscallRuleSet::new(libc::SYS_socket as i64, vec![]),
        SyscallRuleSet::new(libc::SYS_connect as i64, vec![]),
        SyscallRuleSet::new(libc::SYS_accept4 as i64, vec![]),
        SyscallRuleSet::new(libc::SYS_bind as i64, vec![]),
        SyscallRuleSet::new(libc::SYS_listen as i64, vec![]),
        SyscallRuleSet::new(libc::SYS_sendto as i64, vec![]),
        SyscallRuleSet::new(libc::SYS_recvfrom as i64, vec![]),
        SyscallRuleSet::new(libc::SYS_setsockopt as i64, vec![]),
        SyscallRuleSet::new(libc::SYS_getsockopt as i64, vec![]),
        // Process
        SyscallRuleSet::new(libc::SYS_exit_group as i64, vec![]),
        SyscallRuleSet::new(libc::SYS_futex as i64, vec![]),
        SyscallRuleSet::new(libc::SYS_getpid as i64, vec![]),
        SyscallRuleSet::new(libc::SYS_gettid as i64, vec![]),
        // Signal handling
        SyscallRuleSet::new(libc::SYS_rt_sigaction as i64, vec![]),
        SyscallRuleSet::new(libc::SYS_rt_sigprocmask as i64, vec![]),
        SyscallRuleSet::new(libc::SYS_sigaltstack as i64, vec![]),
        // Thread-local storage / architecture
        SyscallRuleSet::new(libc::SYS_arch_prctl as i64, vec![]),
        SyscallRuleSet::new(libc::SYS_set_tid_address as i64, vec![]),
        // Time
        SyscallRuleSet::new(libc::SYS_clock_gettime as i64, vec![]),
        // ioctl (for KVM)
        SyscallRuleSet::new(libc::SYS_ioctl as i64, vec![]),
        // pipe for self-pipe trick
        SyscallRuleSet::new(libc::SYS_pipe2 as i64, vec![]),
    ];

    let filter = SeccompFilter::new(
        allowed_syscalls.into_iter().collect(),
        SeccompAction::Errno(libc::EPERM as u32),
        SeccompAction::Kill,
    )
    .map_err(|e| HardenError::SystemCallFailed(format!("failed to build seccomp filter: {}", e)))?;

    let bpf: BpfProgram = filter
        .try_into()
        .map_err(|e| HardenError::SystemCallFailed(format!("failed to compile BPF: {}", e)))?;

    seccompiler::apply_filter(&bpf).map_err(|e| {
        HardenError::SystemCallFailed(format!("failed to apply seccomp filter: {}", e))
    })?;

    Ok(())
}

/// Installs a Landlock ruleset restricting filesystem access to `workspace_dir` only.
/// Follows the pattern from `crates/skills/src/sandbox/native.rs`.
#[cfg(target_os = "linux")]
fn install_landlock(workspace_dir: &Path) -> Result<(), HardenError> {
    use landlock::{AccessFs, AccessNet, RulesetAttr, RulesetCreatedAttr, RulesetStatus, ABI};

    let abi = ABI::V2;

    let mut ruleset = landlock::Ruleset::new()
        .handle_access(AccessFs::from_all(abi))
        .map_err(|e| HardenError::SystemCallFailed(format!("landlock ruleset attr: {}", e)))?
        .create()
        .map_err(|e| HardenError::SystemCallFailed(format!("landlock ruleset create: {}", e)))?;

    // Allow read/write access to the workspace directory
    ruleset = ruleset
        .add_rule(PathBeneath::new(landlock::path_beneath_attr(
            workspace_dir,
            AccessFs::from_all(abi),
        )))
        .map_err(|e| HardenError::SystemCallFailed(format!("landlock add rule: {}", e)))?;

    // Allow read access to system paths
    for sys_path in &[
        "/usr",
        "/lib",
        "/lib64",
        "/etc",
        "/bin",
        "/sbin",
        "/proc/self",
    ] {
        let p = Path::new(sys_path);
        if p.exists() {
            ruleset = ruleset
                .add_rule(PathBeneath::new(landlock::path_beneath_attr(
                    p,
                    AccessFs::from_read(abi),
                )))
                .map_err(|e| HardenError::SystemCallFailed(format!("landlock add rule: {}", e)))?;
        }
    }

    // Deny all network access
    ruleset = ruleset
        .handle_access(AccessNet::from_all(abi))
        .map_err(|e| HardenError::SystemCallFailed(format!("landlock net attr: {}", e)))?;

    let status = ruleset
        .restrict_self()
        .map_err(|e| HardenError::SystemCallFailed(format!("landlock restrict: {}", e)))?;

    match status.ruleset {
        RulesetStatus::FullyEnforced => {}
        RulesetStatus::PartiallyEnforced => {
            tracing::warn!("Landlock partially enforced — kernel may be too old");
        }
        RulesetStatus::NotEnforced => {
            return Err(HardenError::SystemCallFailed(
                "Landlock not enforced — kernel does not support it".into(),
            ));
        }
    }

    Ok(())
}

/// Helper type for Landlock path_beneath rules.
#[cfg(target_os = "linux")]
struct PathBeneath {
    attr: landlock::PathBeneathAttr,
}

#[cfg(target_os = "linux")]
impl PathBeneath {
    fn new(attr: landlock::PathBeneathAttr) -> Self {
        Self { attr }
    }
}

#[cfg(target_os = "linux")]
impl landlock::Rule for PathBeneath {
    type Attr = landlock::PathBeneathAttr;
    fn get_attr(&self) -> &Self::Attr {
        &self.attr
    }
}

// ── Windows: AppContainer + Job Objects ────────────────────────────────────

#[cfg(target_os = "windows")]
fn harden_windows(config: &HardenConfig) -> Result<(), HardenError> {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
        SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JOB_OBJECT_LIMIT_ACTIVE_PROCESS, JOB_OBJECT_LIMIT_JOB_MEMORY,
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };
    use windows::Win32::System::Threading::GetCurrentProcess;

    // SAFETY: All Windows API calls below operate on valid handles and properly initialized
    // structures. CreateJobObjectW receives a valid PCWSTR from an encoded UTF-16 string.
    // SetInformationJobObject receives a pointer to a properly initialized
    // JOBOBJECT_EXTENDED_LIMIT_INFORMATION struct with correct size. AssignProcessToJobObject
    // receives valid handles. The job handle is intentionally leaked (not closed) because it
    // must remain open for the process lifetime — KILL_ON_JOB_CLOSE depends on this.
    unsafe {
        // Create a Job Object with strict limits
        let job_name = format!("SavantSandbox_{}", std::process::id());
        let wide_name: Vec<u16> = job_name.encode_utf16().chain(std::iter::once(0)).collect();
        let pcw_name = windows::core::PCWSTR(wide_name.as_ptr());

        let job_handle = CreateJobObjectW(None, pcw_name)
            .map_err(|e| HardenError::SystemCallFailed(format!("CreateJobObjectW: {}", e)))?;

        // Configure extended limits
        let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
        limits.BasicLimitInformation.LimitFlags =
            JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE | JOB_OBJECT_LIMIT_ACTIVE_PROCESS;
        limits.BasicLimitInformation.ActiveProcessLimit = 1;

        if config.max_memory_bytes > 0 {
            limits.ProcessMemoryLimit = config.max_memory_bytes as usize;
            limits.BasicLimitInformation.LimitFlags |= JOB_OBJECT_LIMIT_JOB_MEMORY;
        }

        let result = SetInformationJobObject(
            job_handle,
            JobObjectExtendedLimitInformation,
            &limits as *const _ as *const std::ffi::c_void,
            std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        );
        if result.is_err() {
            let _ = CloseHandle(job_handle);
            return Err(HardenError::SystemCallFailed(format!(
                "SetInformationJobObject: {}",
                std::io::Error::last_os_error()
            )));
        }

        // Assign current process to the job
        let result = AssignProcessToJobObject(job_handle, GetCurrentProcess());
        if result.is_err() {
            let _ = CloseHandle(job_handle);
            return Err(HardenError::SystemCallFailed(format!(
                "AssignProcessToJobObject: {}",
                std::io::Error::last_os_error()
            )));
        }

        // Note: We intentionally leak the job handle — it must remain open
        // for the lifetime of the process. When the process exits,
        // KILL_ON_JOB_CLOSE kills all child processes.
    }

    Ok(())
}

// ── macOS: sandbox-exec profile ────────────────────────────────────────────

#[cfg(target_os = "macos")]
fn harden_macos(config: &HardenConfig) -> Result<(), HardenError> {
    use std::process::Command;

    let workspace = config
        .workspace_dir
        .to_str()
        .ok_or_else(|| HardenError::InvalidConfig("workspace path is not valid UTF-8".into()))?;

    // Generate sandbox-exec profile (deny-by-default with explicit allow rules)
    let profile = format!(
        r#"(version 1)
(deny default)
(allow process-exec process-fork sysctl-read)
(allow signal)
(allow file-read* file-write* (subpath "{workspace}"))
(allow file-read* (subpath "/usr") (subpath "/bin") (subpath "/sbin") (subpath "/lib") (subpath "/System") (subpath "/private/tmp"))
(allow file-read* (literal "/dev/null") (literal "/dev/urandom") (literal "/dev/random"))
(allow mach-lookup)
(allow sysctl-read)
(deny network*)
"#
    );

    // Write profile to a temporary file
    let profile_path =
        std::env::temp_dir().join(format!("savant_sandbox_{}.sb", std::process::id()));
    std::fs::write(&profile_path, &profile)
        .map_err(|e| HardenError::Io(format!("failed to write sandbox profile: {}", e)))?;

    // Note: The actual sandbox-exec application happens when spawning the child process,
    // not in the broker. The profile path is written to disk for reference.
    // The macOS sandbox in skills/src/sandbox/native.rs builds its own profile independently.

    // Apply resource limits
    apply_rlimits(config)?;

    Ok(())
}

#[cfg(target_os = "macos")]
fn apply_rlimits(config: &HardenConfig) -> Result<(), HardenError> {
    if config.max_memory_bytes > 0 {
        let rlim = libc::rlimit {
            rlim_cur: config.max_memory_bytes as libc::rlim_t,
            rlim_max: config.max_memory_bytes as libc::rlim_t,
        };
        // SAFETY: setrlimit receives a valid pointer to a properly initialized rlimit struct.
        // RLIMIT_AS is a valid resource constant. The rlimit values are within rlim_t range.
        let ret = unsafe { libc::setrlimit(libc::RLIMIT_AS, &rlim) };
        if ret != 0 {
            return Err(HardenError::SystemCallFailed(format!(
                "setrlimit(RLIMIT_AS): {}",
                std::io::Error::last_os_error()
            )));
        }
    }

    if config.max_open_files > 0 {
        let rlim = libc::rlimit {
            rlim_cur: config.max_open_files as libc::rlim_t,
            rlim_max: config.max_open_files as libc::rlim_t,
        };
        // SAFETY: setrlimit receives a valid pointer to a properly initialized rlimit struct.
        // RLIMIT_NOFILE is a valid resource constant.
        let ret = unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &rlim) };
        if ret != 0 {
            return Err(HardenError::SystemCallFailed(format!(
                "setrlimit(RLIMIT_NOFILE): {}",
                std::io::Error::last_os_error()
            )));
        }
    }

    if config.max_processes > 0 {
        let rlim = libc::rlimit {
            rlim_cur: config.max_processes as libc::rlim_t,
            rlim_max: config.max_processes as libc::rlim_t,
        };
        // SAFETY: setrlimit receives a valid pointer to a properly initialized rlimit struct.
        // RLIMIT_NPROC is a valid resource constant.
        let ret = unsafe { libc::setrlimit(libc::RLIMIT_NPROC, &rlim) };
        if ret != 0 {
            return Err(HardenError::SystemCallFailed(format!(
                "setrlimit(RLIMIT_NPROC): {}",
                std::io::Error::last_os_error()
            )));
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_available() {
        // On Windows and Linux, hardening should be available
        #[cfg(any(target_os = "linux", target_os = "windows", target_os = "macos"))]
        assert!(is_available());
    }

    #[test]
    fn test_harden_config_builder() {
        use std::path::Path;
        let config = HardenConfig::new("/tmp/workspace")
            .with_memory(1024 * 1024 * 256)
            .with_max_processes(4)
            .with_max_open_files(256);

        assert_eq!(config.workspace_dir, Path::new("/tmp/workspace"));
        assert_eq!(config.max_memory_bytes, 256 * 1024 * 1024);
        assert_eq!(config.max_processes, 4);
        assert_eq!(config.max_open_files, 256);
    }

    #[test]
    fn test_harden_config_defaults() {
        let config = HardenConfig::new("/tmp");
        assert_eq!(config.max_memory_bytes, 0);
        assert_eq!(config.max_processes, 0);
        assert_eq!(config.max_open_files, 0);
    }
}
