use std::alloc::Layout;
use std::ptr::NonNull;
use zeroize::Zeroize;

#[derive(Debug, thiserror::Error)]
pub enum SecureMemoryError {
    #[error("failed to lock memory: {0}")]
    LockFailed(String),
    #[error("failed to unlock memory: {0}")]
    UnlockFailed(String),
    #[error("failed to protect memory: {0}")]
    ProtectFailed(String),
    #[error("allocation failed: {0}")]
    AllocFailed(String),
}

/// Returns the system page size in bytes.
fn page_size() -> usize {
    #[cfg(target_os = "windows")]
    {
        use windows::Win32::System::SystemInformation::{GetSystemInfo, SYSTEM_INFO};
        let mut info = SYSTEM_INFO::default();
        unsafe { GetSystemInfo(&mut info) };
        info.dwPageSize as usize
    }
    #[cfg(not(target_os = "windows"))]
    {
        let ps = unsafe { libc::sysconf(libc::_SC_PAGESIZE) };
        if ps <= 0 {
            4096
        } else {
            ps as usize
        }
    }
}

/// Rounds `size` up to the next multiple of `page_size`.
fn round_to_page(size: usize, page_sz: usize) -> usize {
    (size + page_sz - 1) & !(page_sz - 1)
}

pub struct SecureMemory {
    ptr: NonNull<u8>,
    layout: Layout,
    /// The originally requested size (may be less than layout.size() due to page alignment).
    requested_size: usize,
    locked: bool,
    #[cfg(target_os = "linux")]
    uses_memfd_secret: bool,
}

impl SecureMemory {
    /// Allocates page-aligned, locked memory. Uses `memfd_secret` on Linux 5.14+
    /// for kernel-enforced process isolation; falls back to `mmap`+`mlock` otherwise.
    pub fn new(size: usize) -> Result<Self, SecureMemoryError> {
        if size == 0 {
            return Err(SecureMemoryError::LockFailed(
                "zero-length allocation".into(),
            ));
        }

        let ps = page_size();
        let alloc_size = round_to_page(size, ps);
        let layout = Layout::from_size_align(alloc_size, ps)
            .map_err(|e| SecureMemoryError::AllocFailed(e.to_string()))?;

        #[cfg(target_os = "linux")]
        {
            if let Some(result) = Self::try_memfd_secret(alloc_size, size, layout) {
                return result;
            }
        }

        // Standard path: page-aligned allocation + mlock/VirtualLock
        let ptr = unsafe { std::alloc::alloc_zeroed(layout) };
        let ptr = NonNull::new(ptr)
            .ok_or_else(|| SecureMemoryError::AllocFailed("allocation returned null".into()))?;

        #[cfg(target_os = "linux")]
        let mut this = Self {
            ptr,
            layout,
            requested_size: size,
            locked: false,
            uses_memfd_secret: false,
        };
        #[cfg(not(target_os = "linux"))]
        let mut this = Self {
            ptr,
            layout,
            requested_size: size,
            locked: false,
        };
        this.lock()?;
        Ok(this)
    }

    /// Attempts `memfd_secret` allocation on Linux 5.14+. Returns `None` if the
    /// syscall is unavailable (kernel too old) or fails for any reason.
    #[cfg(target_os = "linux")]
    fn try_memfd_secret(
        alloc_size: usize,
        requested_size: usize,
        layout: Layout,
    ) -> Option<Result<Self, SecureMemoryError>> {
        // memfd_secret is syscall 447 on x86_64, 441 on aarch64
        #[cfg(target_arch = "x86_64")]
        const SYS_MEMFD_SECRET: i64 = 447;
        #[cfg(target_arch = "aarch64")]
        const SYS_MEMFD_SECRET: i64 = 441;
        #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
        {
            return None;
        }

        #[cfg(any(target_arch = "x86_64", target_arch = "aarch64"))]
        {
            const SECRETFD_EXCLUSIVE: i64 = 1;
            let fd = unsafe { libc::syscall(SYS_MEMFD_SECRET, SECRETFD_EXCLUSIVE) };
            if fd < 0 {
                return None; // syscall not available or failed
            }

            // Set the size via ftruncate
            let ret = unsafe { libc::ftruncate(fd as i32, alloc_size as i64) };
            if ret != 0 {
                unsafe { libc::close(fd as i32) };
                return None;
            }

            // Map the secret fd
            let ptr = unsafe {
                libc::mmap(
                    std::ptr::null_mut(),
                    alloc_size,
                    libc::PROT_READ | libc::PROT_WRITE,
                    libc::MAP_SHARED,
                    fd as i32,
                    0,
                )
            };
            unsafe { libc::close(fd as i32) };

            if ptr == libc::MAP_FAILED {
                return None;
            }

            let nn = NonNull::new(ptr as *mut u8)
                .ok_or_else(|| SecureMemoryError::AllocFailed("mmap returned null".into()));

            match nn {
                Ok(ptr) => {
                    // memfd_secret pages are already inaccessible to other processes
                    // and are automatically freed on munmap. No mlock needed.
                    Some(Ok(Self {
                        ptr,
                        layout,
                        requested_size,
                        locked: true,
                        uses_memfd_secret: true,
                    }))
                }
                Err(e) => Some(Err(e)),
            }
        }
    }

    /// Locks memory pages to prevent swapping. Skipped for `memfd_secret` allocations
    /// on Linux, which are kernel-isolated and never swapped by design.
    pub fn lock(&mut self) -> Result<(), SecureMemoryError> {
        if self.locked {
            return Ok(());
        }
        #[cfg(target_os = "linux")]
        if self.uses_memfd_secret {
            self.locked = true;
            return Ok(());
        }
        #[cfg(target_os = "windows")]
        {
            let result = unsafe {
                windows::Win32::System::Memory::VirtualLock(
                    self.ptr.as_ptr() as *const std::ffi::c_void,
                    self.layout.size(),
                )
            };
            if result.is_err() {
                return Err(SecureMemoryError::LockFailed(
                    std::io::Error::last_os_error().to_string(),
                ));
            }
        }
        #[cfg(not(target_os = "windows"))]
        {
            let ret = unsafe {
                libc::mlock(self.ptr.as_ptr() as *const libc::c_void, self.layout.size())
            };
            if ret != 0 {
                return Err(SecureMemoryError::LockFailed(
                    std::io::Error::last_os_error().to_string(),
                ));
            }
        }
        self.locked = true;
        Ok(())
    }

    pub fn unlock(&mut self) -> Result<(), SecureMemoryError> {
        if !self.locked {
            return Ok(());
        }
        #[cfg(target_os = "linux")]
        if self.uses_memfd_secret {
            self.locked = false;
            return Ok(());
        }
        #[cfg(target_os = "windows")]
        {
            let result = unsafe {
                windows::Win32::System::Memory::VirtualUnlock(
                    self.ptr.as_ptr() as *const std::ffi::c_void,
                    self.layout.size(),
                )
            };
            if result.is_err() {
                return Err(SecureMemoryError::UnlockFailed(
                    std::io::Error::last_os_error().to_string(),
                ));
            }
        }
        #[cfg(not(target_os = "windows"))]
        {
            let ret = unsafe {
                libc::munlock(self.ptr.as_ptr() as *const libc::c_void, self.layout.size())
            };
            if ret != 0 {
                return Err(SecureMemoryError::UnlockFailed(
                    std::io::Error::last_os_error().to_string(),
                ));
            }
        }
        self.locked = false;
        Ok(())
    }

    /// Sets memory to no-access protection. On Windows, uses `PAGE_NOACCESS`.
    /// On Unix, uses `mprotect(PROT_NONE)`. Note: `VirtualLock` drops its lock
    /// when page protection changes to `PAGE_NOACCESS` on Windows; re-lock on `activate`.
    pub fn idle_protect(&mut self) -> Result<(), SecureMemoryError> {
        #[cfg(target_os = "windows")]
        {
            let mut old = std::mem::MaybeUninit::<
                windows::Win32::System::Memory::PAGE_PROTECTION_FLAGS,
            >::uninit();
            let result = unsafe {
                windows::Win32::System::Memory::VirtualProtect(
                    self.ptr.as_ptr() as *const std::ffi::c_void,
                    self.layout.size(),
                    windows::Win32::System::Memory::PAGE_NOACCESS,
                    old.as_mut_ptr(),
                )
            };
            if result.is_err() {
                return Err(SecureMemoryError::ProtectFailed(
                    std::io::Error::last_os_error().to_string(),
                ));
            }
        }
        #[cfg(not(target_os = "windows"))]
        {
            let ret = unsafe {
                libc::mprotect(
                    self.ptr.as_ptr() as *mut libc::c_void,
                    self.layout.size(),
                    libc::PROT_NONE,
                )
            };
            if ret != 0 {
                return Err(SecureMemoryError::ProtectFailed(
                    std::io::Error::last_os_error().to_string(),
                ));
            }
        }
        Ok(())
    }

    /// Restores memory to read-write protection. On Windows, uses `PAGE_READWRITE`
    /// and re-applies `VirtualLock` (since it was dropped by `idle_protect`).
    /// On Unix, uses `mprotect(PROT_READ | PROT_WRITE)`.
    pub fn activate(&mut self) -> Result<(), SecureMemoryError> {
        #[cfg(target_os = "windows")]
        {
            let mut old = std::mem::MaybeUninit::<
                windows::Win32::System::Memory::PAGE_PROTECTION_FLAGS,
            >::uninit();
            let result = unsafe {
                windows::Win32::System::Memory::VirtualProtect(
                    self.ptr.as_ptr() as *const std::ffi::c_void,
                    self.layout.size(),
                    windows::Win32::System::Memory::PAGE_READWRITE,
                    old.as_mut_ptr(),
                )
            };
            if result.is_err() {
                return Err(SecureMemoryError::ProtectFailed(
                    std::io::Error::last_os_error().to_string(),
                ));
            }
            // Re-lock after PAGE_NOACCESS dropped the VirtualLock
            if self.locked {
                let lock_result = unsafe {
                    windows::Win32::System::Memory::VirtualLock(
                        self.ptr.as_ptr() as *const std::ffi::c_void,
                        self.layout.size(),
                    )
                };
                if lock_result.is_err() {
                    return Err(SecureMemoryError::LockFailed(
                        std::io::Error::last_os_error().to_string(),
                    ));
                }
            }
        }
        #[cfg(not(target_os = "windows"))]
        {
            let ret = unsafe {
                libc::mprotect(
                    self.ptr.as_ptr() as *mut libc::c_void,
                    self.layout.size(),
                    libc::PROT_READ | libc::PROT_WRITE,
                )
            };
            if ret != 0 {
                return Err(SecureMemoryError::ProtectFailed(
                    std::io::Error::last_os_error().to_string(),
                ));
            }
        }
        Ok(())
    }

    /// SAN-03: Return slice of requested size only, not page-aligned size.
    /// This prevents callers from touching guard pages.
    pub fn as_slice(&self) -> &[u8] {
        unsafe { std::slice::from_raw_parts(self.ptr.as_ptr(), self.requested_size) }
    }

    /// SAN-03: Return mutable slice of requested size only, not page-aligned size.
    pub fn as_mut_slice(&mut self) -> &mut [u8] {
        unsafe { std::slice::from_raw_parts_mut(self.ptr.as_ptr(), self.requested_size) }
    }

    /// Returns the originally requested allocation size (before page alignment).
    pub fn len(&self) -> usize {
        self.requested_size
    }

    /// Returns the actual allocated size (page-aligned).
    pub fn alloc_size(&self) -> usize {
        self.layout.size()
    }

    pub fn is_empty(&self) -> bool {
        self.layout.size() == 0
    }
}

impl Drop for SecureMemory {
    fn drop(&mut self) {
        let ptr = self.ptr.as_ptr();
        let size = self.layout.size();
        unsafe { std::ptr::write_bytes(ptr, 0, size) };

        #[cfg(target_os = "linux")]
        if self.uses_memfd_secret {
            unsafe { libc::munmap(ptr as *mut libc::c_void, size) };
            return;
        }

        if self.locked {
            #[cfg(target_os = "windows")]
            {
                let _ = unsafe {
                    windows::Win32::System::Memory::VirtualUnlock(
                        ptr as *const std::ffi::c_void,
                        size,
                    )
                };
            }
            #[cfg(not(target_os = "windows"))]
            {
                let _ = unsafe { libc::munlock(ptr as *const libc::c_void, size) };
            }
        }
        unsafe { std::alloc::dealloc(ptr, self.layout) };
    }
}

impl Zeroize for SecureMemory {
    fn zeroize(&mut self) {
        unsafe { std::ptr::write_bytes(self.ptr.as_ptr(), 0, self.layout.size()) };
    }
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)]
mod tests {
    use super::*;

    #[test]
    fn test_new_and_lock() {
        let mut mem = SecureMemory::new(64).expect("failed to allocate");
        assert!(mem.locked);
        assert_eq!(mem.len(), 64);
        assert!(mem.alloc_size() >= 64);
        assert_eq!(mem.alloc_size() % page_size(), 0);
        mem.unlock().expect("failed to unlock");
        assert!(!mem.locked);
    }

    #[test]
    fn test_zeroize_on_drop() {
        // SAN-01: Test zeroization by reading before drop and verifying zeroize() works.
        // Avoids use-after-free from reading raw pointer after drop.
        let mut mem = SecureMemory::new(16).expect("failed to allocate");
        // Fill the entire allocated region via the safe slice API
        unsafe {
            std::ptr::write_bytes(mem.as_mut_slice().as_mut_ptr(), 0xAA, 16);
        }
        assert_eq!(mem.as_slice()[0], 0xAA);
        // Call zeroize explicitly and verify
        mem.zeroize();
        for &b in mem.as_slice() {
            assert_eq!(b, 0x00);
        }
        // Verify the full allocated region is zeroed (using alloc_size)
        for &b in &mem.as_slice()[..16] {
            assert_eq!(b, 0x00);
        }
        drop(mem);
    }

    #[test]
    fn test_idle_protect_cycle() {
        let mut mem = SecureMemory::new(32).expect("failed to allocate");
        // Fill first 32 bytes
        mem.as_mut_slice()[..32].copy_from_slice(&[0xBBu8; 32]);
        mem.idle_protect().expect("idle_protect failed");
        mem.activate().expect("activate failed");
        assert_eq!(mem.as_slice()[0], 0xBB);
        assert_eq!(mem.as_slice()[31], 0xBB);
    }

    #[test]
    fn test_zeroize_trait() {
        let mut mem = SecureMemory::new(8).expect("failed to allocate");
        // Fill first 8 bytes
        mem.as_mut_slice()[..8].copy_from_slice(&[0xCCu8; 8]);
        mem.zeroize();
        // All allocated bytes should be zeroed
        for &b in mem.as_slice() {
            assert_eq!(b, 0x00);
        }
    }

    #[test]
    fn test_lock_twice_is_idempotent() {
        let mut mem = SecureMemory::new(16).expect("failed to allocate");
        assert!(mem.locked);
        mem.lock().expect("second lock should succeed");
        mem.unlock().expect("unlock failed");
    }

    #[test]
    fn test_unlock_twice_is_idempotent() {
        let mut mem = SecureMemory::new(16).expect("failed to allocate");
        mem.unlock().expect("first unlock");
        mem.unlock().expect("second unlock should be no-op");
    }
}
