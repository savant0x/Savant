//! Shield state machine for sandbox policy enforcement.
//!
//! Controls whether sandbox policies (network, DNS, bandwidth) are mutable
//! or locked. Supports timed unlocks with automatic re-lock.

use std::time::Duration;
use tokio::task::JoinHandle;

/// Shield states for the sandbox policy layer.
#[derive(Debug, Clone, PartialEq)]
pub enum ShieldState {
    /// Default state — policies can be changed.
    Mutable,
    /// Policies frozen, config read-only. Network rules, DNS allowlist,
    /// and bandwidth limits cannot be modified.
    Locked,
    /// Temporarily unlocked with an auto-restore timer.
    /// Contains the timestamp when the shield will auto-lock.
    TemporarilyUnlocked {
        /// Unix timestamp when auto-lock fires.
        lock_at: u64,
    },
}

/// Manages the shield state machine for sandbox policies.
///
/// Note: This controls the *runtime policy layer* (network rules, DNS allowlist,
/// bandwidth limits). Kernel-level filters (Landlock, seccomp) are applied once
/// at process start and cannot be dynamically modified.
pub struct ShieldManager {
    state: ShieldState,
    auto_restore_handle: Option<JoinHandle<()>>,
}

impl ShieldManager {
    /// Creates a new shield manager in the Mutable state.
    pub fn new() -> Self {
        Self {
            state: ShieldState::Mutable,
            auto_restore_handle: None,
        }
    }

    /// Returns the current shield state.
    pub fn state(&self) -> &ShieldState {
        &self.state
    }

    /// Returns `true` if policies can be modified (state is Mutable or TemporarilyUnlocked).
    /// SAN-17: Consistent with check_mutable() which allows both states.
    pub fn is_mutable(&self) -> bool {
        match &self.state {
            ShieldState::Mutable => true,
            ShieldState::TemporarilyUnlocked { lock_at } => {
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs();
                now < *lock_at
            }
            ShieldState::Locked => false,
        }
    }

    /// Returns `true` if the shield is locked (Locked or TemporarilyUnlocked).
    pub fn is_locked(&self) -> bool {
        matches!(
            self.state,
            ShieldState::Locked | ShieldState::TemporarilyUnlocked { .. }
        )
    }

    /// Locks the shield. Policies become read-only.
    /// Cancels any pending auto-restore timer.
    pub fn lock(&mut self) -> Result<(), ShieldError> {
        // Cancel any pending auto-restore
        if let Some(handle) = self.auto_restore_handle.take() {
            handle.abort();
        }

        self.state = ShieldState::Locked;
        tracing::info!("[shields] Policies LOCKED");
        Ok(())
    }

    /// Temporarily unlocks the shield with an auto-restore timer.
    /// After `timeout` elapses, the shield automatically re-locks.
    pub fn unlock(&mut self, timeout: Duration) -> Result<(), ShieldError> {
        // Cancel any existing timer
        if let Some(handle) = self.auto_restore_handle.take() {
            handle.abort();
        }

        let lock_at = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
            + timeout.as_secs();

        self.state = ShieldState::TemporarilyUnlocked { lock_at };

        tracing::info!(
            "[shields] Policies TEMPORARILY UNLOCKED for {:?} (auto-lock at {})",
            timeout,
            lock_at
        );

        // Spawn auto-restore timer (abort-able via JoinHandle)
        let handle = tokio::spawn(async move {
            tokio::time::sleep(timeout).await;
            tracing::info!("[shields] Auto-lock timer fired — policies should re-lock");
        });

        self.auto_restore_handle = Some(handle);

        Ok(())
    }

    /// Unlocks the shield permanently (no auto-restore).
    pub fn unlock_permanent(&mut self) -> Result<(), ShieldError> {
        if let Some(handle) = self.auto_restore_handle.take() {
            handle.abort();
        }

        self.state = ShieldState::Mutable;
        tracing::info!("[shields] Policies PERMANENTLY UNLOCKED");
        Ok(())
    }

    /// Checks if a policy modification is allowed given the current shield state.
    pub fn check_mutable(&self) -> Result<(), ShieldError> {
        match &self.state {
            ShieldState::Mutable => Ok(()),
            ShieldState::Locked => Err(ShieldError::Locked),
            ShieldState::TemporarilyUnlocked { lock_at } => {
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs();
                if now >= *lock_at {
                    Err(ShieldError::LockExpired)
                } else {
                    Ok(())
                }
            }
        }
    }
}

impl Default for ShieldManager {
    fn default() -> Self {
        Self::new()
    }
}

/// Errors from shield operations.
#[derive(Debug, thiserror::Error)]
pub enum ShieldError {
    #[error("policies are locked — cannot modify")]
    Locked,
    #[error("temporary unlock window has expired")]
    LockExpired,
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)]
mod tests {
    use super::*;

    #[test]
    fn test_initial_state_mutable() {
        let mgr = ShieldManager::new();
        assert_eq!(*mgr.state(), ShieldState::Mutable);
        assert!(mgr.is_mutable());
        assert!(!mgr.is_locked());
    }

    #[test]
    fn test_lock() {
        let mut mgr = ShieldManager::new();
        mgr.lock().expect("lock should succeed");
        assert_eq!(*mgr.state(), ShieldState::Locked);
        assert!(mgr.is_locked());
        assert!(mgr.check_mutable().is_err());
    }

    #[test]
    fn test_unlock_permanent() {
        let mut mgr = ShieldManager::new();
        mgr.lock().expect("lock should succeed");
        mgr.unlock_permanent().expect("unlock should succeed");
        assert!(mgr.is_mutable());
        assert!(mgr.check_mutable().is_ok());
    }

    #[tokio::test]
    async fn test_temporary_unlock() {
        let mut mgr = ShieldManager::new();
        mgr.lock().expect("lock should succeed");
        assert!(mgr.check_mutable().is_err());
        mgr.unlock(Duration::from_secs(30))
            .expect("unlock should succeed");
        // TemporarilyUnlocked allows modifications until the timer fires
        assert!(mgr.check_mutable().is_ok());
        assert!(mgr.is_locked()); // Still "locked" in the shield sense
    }
}
