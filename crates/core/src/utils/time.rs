/// Reliable time utilities — return Result to handle clock errors gracefully.
///
/// The system clock should NEVER return a time before Unix epoch.
/// If it does, the system is misconfigured and we return an error
/// rather than panicking or silently treating the error as epoch 0.
use std::time::{SystemTime, UNIX_EPOCH};

use crate::error::SavantError;

/// Returns the current time as seconds since Unix epoch.
/// Returns Err if the system clock is before Unix epoch.
pub fn now_secs() -> Result<u64, SavantError> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .map_err(|e| SavantError::Unknown(format!("System clock before UNIX epoch: {}", e)))
}

/// Returns the current time as milliseconds since Unix epoch.
/// Returns Err if the system clock is before Unix epoch.
pub fn now_millis() -> Result<u64, SavantError> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .map_err(|e| SavantError::Unknown(format!("System clock before UNIX epoch: {}", e)))
}

/// Returns the current time as nanoseconds since Unix epoch.
/// Returns Err if the system clock is before Unix epoch.
pub fn now_nanos() -> Result<u128, SavantError> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .map_err(|e| SavantError::Unknown(format!("System clock before UNIX epoch: {}", e)))
}
