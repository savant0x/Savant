#![allow(unexpected_cfgs)]
pub mod attestation;
pub mod continuous;
pub mod enclave;
pub mod pii;
pub mod prompt_defense;
#[cfg(kani)]
pub mod proofs;
pub mod token;

pub use enclave::{SecurityAuthority, SecurityError};
pub use prompt_defense::{scan_prompt, BlockedReason, ScanResult};
pub use token::{AgentToken, CapabilityPayload};
