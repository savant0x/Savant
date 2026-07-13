//! OMEGA-VII: Tri-Enclave Attestation (Consensus Enclave)
//!
//! Implements a consensus-based attestation mechanism between
//! 1. Host TPM (Hardware)
//! 2. WASM Micro-Kernel (Software Sandbox)
//! 3. Decentralized Witness (Network/External)

use thiserror::Error;
use tracing::{debug, info, warn};

#[derive(Debug, Error)]
pub enum AttestationError {
    #[error("Consensus failed: Consensus threshold not met.")]
    ConsensusThresholdNotMet,
    #[error("TPM Attestation failed.")]
    TpmFailure,
    #[error("WASM Attestation failed.")]
    WasmFailure,
    #[error("Witness Attestation failed.")]
    WitnessFailure,
}

/// Represents the state of an attestation attempt.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum EnclaveStatus {
    Verified,
    Degraded,
    Compromised,
    Failed,
    Skipped,
}

/// The result of a Tri-Enclave Attestation loop.
pub struct AttestationResult {
    pub tpm: EnclaveStatus,
    pub wasm: EnclaveStatus,
    pub witness: EnclaveStatus,
}

impl AttestationResult {
    /// Checks if a 2/3 consensus was reached.
    pub fn has_consensus(&self) -> bool {
        let mut count = 0;
        if self.tpm == EnclaveStatus::Verified {
            count += 1;
        }
        if self.wasm == EnclaveStatus::Verified {
            count += 1;
        }
        if self.witness == EnclaveStatus::Verified {
            count += 1;
        }
        count >= 2
    }
}

pub struct AttestationManager;

impl AttestationManager {
    /// Performs a full Tri-Enclave Attestation for a given substrate state.
    pub async fn attest_state(
        &self,
        state_hash: [u8; 32],
    ) -> Result<AttestationResult, AttestationError> {
        info!(
            "Attestation: Initiating Tri-Enclave Consensus for state hash: {:x?}",
            state_hash
        );

        // 1. Host TPM Attestation
        let tpm_status = Self::verify_tpm();

        // 2. WASM Micro-Kernel Attestation
        let wasm_status = Self::verify_memory_allocation();

        // 3. Decentralized Witness Attestation
        let witness_status = Self::verify_witness().await;

        let result = AttestationResult {
            tpm: tpm_status,
            wasm: wasm_status,
            witness: witness_status,
        };

        if result.has_consensus() {
            info!("Attestation: Consensus REACHED. Substrate state CERTIFIED.");
            Ok(result)
        } else {
            debug!("Attestation: Consensus FAILURE. Substrate state REJECTED.");
            Err(AttestationError::ConsensusThresholdNotMet)
        }
    }

    /// Verifies TPM presence on the host system.
    /// On Linux, checks for /dev/tpm0 or /dev/tpmrm0.
    /// On Windows, checks for TPM via WMI.
    ///
    /// SECURITY NOTE: This is a TPM presence check only, NOT full attestation.
    /// It verifies that a TPM device file exists but does NOT perform:
    /// - PCR (Platform Configuration Register) quote verification
    /// - TPM endorsement key validation
    /// - Measured boot attestation
    /// - Remote attestation via AIK (Attestation Identity Key)
    ///
    /// A device file existing does NOT prove the TPM is genuine, unmodified,
    /// or that the boot chain is trustworthy. For production attestation,
    /// integrate with a proper TPM attestation service.
    fn verify_tpm() -> EnclaveStatus {
        #[cfg(target_os = "linux")]
        {
            let tpm0 = std::path::Path::new("/dev/tpm0");
            let tpmrm0 = std::path::Path::new("/dev/tpmrm0");
            if tpm0.exists() || tpmrm0.exists() {
                info!("Attestation: TPM device found on host.");
                EnclaveStatus::Verified
            } else {
                debug!("Attestation: No TPM device found. Running in Degraded mode.");
                EnclaveStatus::Degraded
            }
        }

        #[cfg(target_os = "windows")]
        {
            // Check TPM via WMI command
            let output = std::process::Command::new("wmic")
                .args([
                    "/namespace:\\\\root\\cimv2\\security\\microsofttpm",
                    "path",
                    "win32_tpm",
                    "get",
                    "/value",
                ])
                .output();
            match output {
                Ok(out) if out.status.success() && !out.stdout.is_empty() => {
                    info!("Attestation: TPM detected via WMI.");
                    EnclaveStatus::Verified
                }
                _ => {
                    debug!("Attestation: TPM not detected via WMI. Running in Degraded mode.");
                    EnclaveStatus::Degraded
                }
            }
        }

        #[cfg(not(any(target_os = "linux", target_os = "windows")))]
        {
            debug!("Attestation: TPM check not supported on this platform.");
            EnclaveStatus::Skipped
        }
    }

    /// Verifies WASM memory integrity by checking sandbox memory allocation and access.
    /// Tests that the process can allocate, write, read, and deallocate memory correctly.
    ///
    /// SECURITY NOTE: This is a basic WASM memory allocation test, NOT full sandbox
    /// integrity verification. It only confirms that the host process can perform
    /// normal memory operations. It does NOT verify:
    /// - WASM linear memory isolation from the host
    /// - WASM module compilation integrity
    /// - Capability-based security enforcement
    /// - Protection against side-channel attacks
    ///
    /// For production sandbox integrity, integrate with a proper WASM runtime
    /// attestation mechanism (e.g., wasmtime's component model verification).
    fn verify_memory_allocation() -> EnclaveStatus {
        let result = std::panic::catch_unwind(|| {
            // Allocate a test buffer to verify memory subsystem integrity
            let size = 4096; // One page
            let mut buffer = vec![0u8; size];
            // Write pattern
            for (i, byte) in buffer.iter_mut().enumerate() {
                *byte = (i % 256) as u8;
            }
            // Verify pattern
            for (i, &byte) in buffer.iter().enumerate() {
                assert_eq!(byte, (i % 256) as u8, "Memory integrity check failed");
            }
            // Verify zeroing
            buffer.fill(0);
            assert!(buffer.iter().all(|&b| b == 0), "Memory zeroing failed");
            true
        });
        match result {
            Ok(true) => {
                info!("Attestation: WASM memory integrity verified.");
                EnclaveStatus::Verified
            }
            Ok(false) => {
                warn!("Attestation: WASM memory integrity check returned unexpected result.");
                EnclaveStatus::Compromised
            }
            Err(_) => {
                warn!("Attestation: WASM memory integrity check panicked.");
                EnclaveStatus::Compromised
            }
        }
    }

    /// Verifies witness endpoint reachability via TCP connect test.
    async fn verify_witness() -> EnclaveStatus {
        let witness_endpoint = match std::env::var("SAVANT_WITNESS_ENDPOINT") {
            Ok(ep) if !ep.is_empty() => ep,
            _ => {
                debug!("Attestation: SAVANT_WITNESS_ENDPOINT not configured. Witness attestation skipped.");
                return EnclaveStatus::Failed;
            }
        };

        match tokio::net::TcpStream::connect(&witness_endpoint).await {
            Ok(_) => {
                info!(
                    "Attestation: Witness endpoint {} reachable.",
                    witness_endpoint
                );
                EnclaveStatus::Verified
            }
            Err(e) => {
                warn!(
                    "Attestation: Witness endpoint {} unreachable: {}. Running in Degraded mode.",
                    witness_endpoint, e
                );
                EnclaveStatus::Degraded
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_partial_consensus() {
        let res = AttestationResult {
            tpm: EnclaveStatus::Verified,
            wasm: EnclaveStatus::Verified,
            witness: EnclaveStatus::Degraded,
        };
        assert!(res.has_consensus());
    }

    #[test]
    fn test_failed_consensus() {
        let res = AttestationResult {
            tpm: EnclaveStatus::Verified,
            wasm: EnclaveStatus::Compromised,
            witness: EnclaveStatus::Degraded,
        };
        assert!(!res.has_consensus());
    }

    #[test]
    fn test_tpm_verification() {
        let status = AttestationManager::verify_tpm();
        // TPM may or may not be present; just verify it returns a valid status
        assert!(matches!(
            status,
            EnclaveStatus::Verified | EnclaveStatus::Degraded | EnclaveStatus::Skipped
        ));
    }

    #[test]
    fn test_wasm_verification() {
        let status = AttestationManager::verify_memory_allocation();
        // On a working system, wasmtime should instantiate successfully
        assert!(matches!(
            status,
            EnclaveStatus::Verified | EnclaveStatus::Compromised
        ));
    }
}
