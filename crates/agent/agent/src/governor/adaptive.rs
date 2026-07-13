//! Adaptive Semaphore — adjusts agent concurrency based on resource pressure.
//!
//! Concurrency invariant: `adjust_permits()` uses a `Mutex` to serialize
//! the read-modify-write cycle on `current_max` and the semaphore permit
//! count. Without the lock, concurrent calls could double-add or double-
//! forget permits because `load()` → modify → `store()` is not atomic
//! across the two data structures (AtomicUsize and Semaphore).

use savant_core::config::ResourceGovernorConfig;
use std::sync::Arc;
use tokio::sync::{Mutex, OwnedSemaphorePermit, Semaphore, SemaphorePermit};

use super::monitor::ResourceMonitor;

/// Semaphore that adjusts its permit count based on system pressure.
pub struct AdaptiveSemaphore {
    inner: Arc<Semaphore>,
    monitor: Arc<ResourceMonitor>,
    config: ResourceGovernorConfig,
    /// Tracks the configured maximum. Protected by `adjust_lock` during
    /// modifications so the load→modify→store on both `current_max` and
    /// the semaphore permit pool is atomic.
    current_max: std::sync::atomic::AtomicUsize,
    /// Serialises `adjust_permits()` to prevent double-add/double-forget.
    adjust_lock: Mutex<()>,
}

impl AdaptiveSemaphore {
    pub fn new(monitor: Arc<ResourceMonitor>, config: ResourceGovernorConfig) -> Self {
        let max = config.max_agents_low;
        Self {
            inner: Arc::new(Semaphore::new(max)),
            monitor,
            config,
            current_max: std::sync::atomic::AtomicUsize::new(max),
            adjust_lock: Mutex::new(()),
        }
    }

    /// Try to acquire a spawn permit. Non-blocking.
    pub fn try_acquire(&self) -> Option<SemaphorePermit<'_>> {
        self.inner.try_acquire().ok()
    }

    /// Acquire a spawn permit (blocking).
    #[allow(clippy::disallowed_methods)]
    pub async fn acquire(&self) -> OwnedSemaphorePermit {
        self.inner
            .clone()
            .acquire_owned()
            .await
            .expect("semaphore closed")
    }

    /// Available permits.
    pub fn available_permits(&self) -> usize {
        self.inner.available_permits()
    }

    /// Adjust available permits based on current pressure.
    ///
    /// Serialized by `adjust_lock` so concurrent calls never double-add
    /// or double-forget permits.
    pub async fn adjust_permits(&self) {
        let _guard = self.adjust_lock.lock().await;

        let pressure = self.monitor.current_pressure();
        let target = pressure.max_agents(&self.config);
        let current = self.current_max.load(std::sync::atomic::Ordering::SeqCst);

        if target == current {
            return;
        }

        if target > current {
            // Need more permits — add them
            let diff = target - current;
            self.inner.add_permits(diff);
            self.current_max
                .store(target, std::sync::atomic::Ordering::SeqCst);
            tracing::debug!("[governor] Increased permits: {} → {}", current, target);
        } else {
            // Need fewer permits — forget excess
            let current_available = self.inner.available_permits();
            if current_available > target {
                let to_forget = current_available - target;
                self.inner.forget_permits(to_forget);
            }
            self.current_max
                .store(target, std::sync::atomic::Ordering::SeqCst);
            tracing::debug!("[governor] Decreased permits: {} → {}", current, target);
        }
    }

    /// Get the monitor reference.
    pub fn monitor(&self) -> &Arc<ResourceMonitor> {
        &self.monitor
    }
}
