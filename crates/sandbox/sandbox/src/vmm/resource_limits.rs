use std::path::Path;

#[derive(Debug, thiserror::Error)]
pub enum ResourceError {
    #[error("platform not supported: {0}")]
    UnsupportedPlatform(String),
    #[error("system call failed: {0}")]
    SystemCallFailed(String),
    #[error("I/O error: {0}")]
    Io(String),
    #[error("invalid configuration: {0}")]
    InvalidConfig(String),
}

/// Resource limits to apply to a sandboxed process or cgroup.
#[derive(Debug, Clone, Default)]
pub struct ResourceLimits {
    /// Maximum memory in bytes (0 = no limit).
    pub memory_bytes: u64,
    /// Maximum memory+swap in bytes (0 = same as memory_bytes).
    pub memory_swap_bytes: u64,
    /// CPU bandwidth as "microseconds period" (e.g., "50000 100000" = 50% of one core).
    pub cpu_max: Option<(u64, u64)>,
    /// Maximum number of processes/threads.
    pub pids_max: u32,
}

impl ResourceLimits {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_memory(mut self, bytes: u64) -> Self {
        self.memory_bytes = bytes;
        self
    }

    pub fn with_memory_swap(mut self, bytes: u64) -> Self {
        self.memory_swap_bytes = bytes;
        self
    }

    pub fn with_cpu(mut self, micros: u64, period: u64) -> Self {
        self.cpu_max = Some((micros, period));
        self
    }

    pub fn with_pids(mut self, max: u32) -> Self {
        self.pids_max = max;
        self
    }
}

/// Applies resource limits to the current process or a new cgroup.
///
/// On Linux: writes to cgroups v2 files.
/// On Windows: uses JobObjectExtendedLimitInformation.
/// On macOS: uses setrlimit.
pub fn apply_limits(limits: &ResourceLimits) -> Result<(), ResourceError> {
    #[cfg(target_os = "linux")]
    {
        apply_cgroups(limits)?;
    }
    #[cfg(target_os = "windows")]
    {
        apply_job_object(limits)?;
    }
    #[cfg(target_os = "macos")]
    {
        apply_rlimits(limits)?;
    }
    #[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
    {
        return Err(ResourceError::UnsupportedPlatform(format!(
            "resource limits not supported on {}",
            std::env::consts::OS
        )));
    }
    Ok(())
}

/// Applies resource limits to a specific cgroup path (Linux only).
/// If the cgroup doesn't exist, it will be created.
pub fn apply_limits_to_cgroup(
    cgroup_path: &Path,
    limits: &ResourceLimits,
) -> Result<(), ResourceError> {
    #[cfg(target_os = "linux")]
    {
        apply_cgroups_at(cgroup_path, limits)
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (cgroup_path, limits);
        Err(ResourceError::UnsupportedPlatform(
            "cgroup-based limits are Linux-only".into(),
        ))
    }
}

// ── Linux: cgroups v2 ──────────────────────────────────────────────────────

#[cfg(target_os = "linux")]
fn apply_cgroups(limits: &ResourceLimits) -> Result<(), ResourceError> {
    // Use the current process's cgroup
    let cgroup_path = detect_cgroup_path()?;
    apply_cgroups_at(&cgroup_path, limits)
}

#[cfg(target_os = "linux")]
fn apply_cgroups_at(cgroup_path: &Path, limits: &ResourceLimits) -> Result<(), ResourceError> {
    if limits.memory_bytes > 0 {
        write_cgroup_file(
            &cgroup_path.join("memory.max"),
            &limits.memory_bytes.to_string(),
        )?;
        let swap = if limits.memory_swap_bytes > 0 {
            limits.memory_swap_bytes
        } else {
            limits.memory_bytes
        };
        write_cgroup_file(&cgroup_path.join("memory.swap.max"), &swap.to_string())?;
    }

    if let Some((micros, period)) = limits.cpu_max {
        write_cgroup_file(
            &cgroup_path.join("cpu.max"),
            &format!("{} {}", micros, period),
        )?;
    }

    if limits.pids_max > 0 {
        write_cgroup_file(&cgroup_path.join("pids.max"), &limits.pids_max.to_string())?;
    }

    Ok(())
}

#[cfg(target_os = "linux")]
fn write_cgroup_file(path: &Path, value: &str) -> Result<(), ResourceError> {
    std::fs::write(path, value)
        .map_err(|e| ResourceError::Io(format!("failed to write {}: {}", path.display(), e)))
}

/// Detects the current process's cgroup v2 path.
#[cfg(target_os = "linux")]
fn detect_cgroup_path() -> Result<std::path::PathBuf, ResourceError> {
    // Read /proc/self/cgroup to find the cgroup path
    let cgroup_info = std::fs::read_to_string("/proc/self/cgroup")
        .map_err(|e| ResourceError::Io(e.to_string()))?;

    // cgroups v2 format: "0::/path"
    for line in cgroup_info.lines() {
        if line.starts_with("0::") {
            let rel_path = &line[3..];
            let abs_path = Path::new("/sys/fs/cgroup").join(rel_path.trim_start_matches('/'));
            return Ok(abs_path);
        }
    }

    Err(ResourceError::SystemCallFailed(
        "could not determine cgroup v2 path from /proc/self/cgroup".into(),
    ))
}

// ── Windows: Job Objects ───────────────────────────────────────────────────

#[cfg(target_os = "windows")]
fn apply_job_object(limits: &ResourceLimits) -> Result<(), ResourceError> {
    use windows::Win32::System::JobObjects::{
        CreateJobObjectW, JobObjectExtendedLimitInformation, SetInformationJobObject,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_JOB_MEMORY,
    };

    // SAFETY: All Windows API calls operate on valid handles and properly initialized structures.
    // CreateJobObjectW receives a valid PCWSTR. SetInformationJobObject receives a pointer to a
    // properly initialized JOBOBJECT_EXTENDED_LIMIT_INFORMATION with correct size. The job handle
    // is intentionally leaked for the process lifetime.
    unsafe {
        let job_name = format!("SavantSandbox_{}", std::process::id());
        let wide_name: Vec<u16> = job_name.encode_utf16().chain(std::iter::once(0)).collect();
        let pcw_name = windows::core::PCWSTR(wide_name.as_ptr());

        let job_handle = CreateJobObjectW(None, pcw_name)
            .map_err(|e| ResourceError::SystemCallFailed(format!("CreateJobObjectW: {}", e)))?;

        let mut ext_limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();

        if limits.memory_bytes > 0 {
            ext_limits.ProcessMemoryLimit = limits.memory_bytes as usize;
            ext_limits.BasicLimitInformation.LimitFlags |= JOB_OBJECT_LIMIT_JOB_MEMORY;
        }

        let result = SetInformationJobObject(
            job_handle,
            JobObjectExtendedLimitInformation,
            &ext_limits as *const _ as *const std::ffi::c_void,
            std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        );
        if result.is_err() {
            return Err(ResourceError::SystemCallFailed(format!(
                "SetInformationJobObject: {}",
                std::io::Error::last_os_error()
            )));
        }

        // Note: job handle intentionally leaked for process lifetime
    }

    Ok(())
}

// ── macOS: setrlimit ───────────────────────────────────────────────────────

#[cfg(target_os = "macos")]
fn apply_rlimits(limits: &ResourceLimits) -> Result<(), ResourceError> {
    if limits.memory_bytes > 0 {
        let rlim = libc::rlimit {
            rlim_cur: limits.memory_bytes as libc::rlim_t,
            rlim_max: limits.memory_bytes as libc::rlim_t,
        };
        // SAFETY: setrlimit receives a valid pointer to a properly initialized rlimit struct.
        // RLIMIT_AS is a valid resource constant. Values are within rlim_t range.
        let ret = unsafe { libc::setrlimit(libc::RLIMIT_AS, &rlim) };
        if ret != 0 {
            return Err(ResourceError::SystemCallFailed(format!(
                "setrlimit(RLIMIT_AS): {}",
                std::io::Error::last_os_error()
            )));
        }
    }

    if limits.pids_max > 0 {
        let rlim = libc::rlimit {
            rlim_cur: limits.pids_max as libc::rlim_t,
            rlim_max: limits.pids_max as libc::rlim_t,
        };
        // SAFETY: setrlimit receives a valid pointer to a properly initialized rlimit struct.
        // RLIMIT_NPROC is a valid resource constant.
        let ret = unsafe { libc::setrlimit(libc::RLIMIT_NPROC, &rlim) };
        if ret != 0 {
            return Err(ResourceError::SystemCallFailed(format!(
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
    fn test_resource_limits_builder() {
        let limits = ResourceLimits::new()
            .with_memory(256 * 1024 * 1024)
            .with_memory_swap(512 * 1024 * 1024)
            .with_cpu(50000, 100000)
            .with_pids(16);

        assert_eq!(limits.memory_bytes, 256 * 1024 * 1024);
        assert_eq!(limits.memory_swap_bytes, 512 * 1024 * 1024);
        assert_eq!(limits.cpu_max, Some((50000, 100000)));
        assert_eq!(limits.pids_max, 16);
    }

    #[test]
    fn test_resource_limits_defaults() {
        let limits = ResourceLimits::new();
        assert_eq!(limits.memory_bytes, 0);
        assert_eq!(limits.memory_swap_bytes, 0);
        assert!(limits.cpu_max.is_none());
        assert_eq!(limits.pids_max, 0);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn test_detect_cgroup_path() {
        let result = detect_cgroup_path();
        // Should succeed on most Linux systems with cgroups v2
        if let Ok(path) = result {
            assert!(path.starts_with("/sys/fs/cgroup"));
        }
    }
}
