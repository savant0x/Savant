use crate::token::{AgentToken, CapabilityPayload, SignatureAlgorithm};
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use pqcrypto_dilithium::dilithium2;
use pqcrypto_traits::sign::DetachedSignature;
use std::time::{SystemTime, UNIX_EPOCH};
use thiserror::Error;
use tracing::warn;

#[derive(Debug, Error)]
pub enum SecurityError {
    #[error("Invalid signature for algorithm {0:?}")]
    InvalidSignature(SignatureAlgorithm),
    #[error("Token has expired")]
    TokenExpired,
    #[error("Unauthorized: {0} for resource {1}")]
    UnauthorizedAction(String, String),
    #[error("Zero-copy memory validation failed.")]
    MemoryCorruption,
    #[error("Unsupported signature algorithm: {0:?}")]
    UnsupportedAlgorithm(SignatureAlgorithm),
}

pub struct SecurityAuthority {
    /// The master public key of the Gateway/Orchestrator that issues tokens
    pub root_authority: VerifyingKey,
    /// OMEGA-VIII: PQC Root Authority for hybrid/quantum tokens
    pub pqc_authority: Option<dilithium2::PublicKey>,
}

impl SecurityAuthority {
    pub fn new(root_authority: VerifyingKey, pqc_authority: Option<dilithium2::PublicKey>) -> Self {
        Self {
            root_authority,
            pqc_authority,
        }
    }

    /// Helper to get current UNIX time securely
    fn current_time() -> Result<u64, SecurityError> {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .map_err(|e| {
                tracing::error!("System clock error: {}", e);
                SecurityError::UnauthorizedAction(
                    "Clock error".into(),
                    "System clock is before Unix epoch".into(),
                )
            })
    }

    /// Mints a new token with Quantum-Cognitive Entanglement.
    pub fn mint_quantum_token(
        signer: &SigningKey,
        pqc_signer: &dilithium2::SecretKey,
        assignee_hash: u64,
        resource_uri: &str,
        permitted_action: &str,
        ttl_seconds: u64,
        cadence_entropy: &[u8],
    ) -> Result<AgentToken, SecurityError> {
        // OMEGA-VII: Hybrid-Entropic mixing (System Entropy + User Cadence)
        use rand::RngCore;
        let mut rng = rand::rngs::OsRng;

        let mut entropy_hash = [0u8; 32];
        let mut system_entropy = [0u8; 16];
        rng.fill_bytes(&mut system_entropy);

        let mut combined = Vec::with_capacity(cadence_entropy.len() + system_entropy.len());
        combined.extend_from_slice(cadence_entropy);
        combined.extend_from_slice(&system_entropy);

        let h = xxhash_rust::xxh3::xxh3_128(&combined);
        let h_bytes = h.to_le_bytes();
        entropy_hash[0..16].copy_from_slice(&h_bytes);
        // Independent second hash for the remaining 16 bytes (no mirroring)
        let h2 = xxhash_rust::xxh3::xxh3_128(&system_entropy);
        entropy_hash[16..32].copy_from_slice(&h2.to_le_bytes());

        let payload = CapabilityPayload {
            assignee_hash,
            resource_uri: resource_uri.to_string(),
            permitted_action: permitted_action.to_string(),
            expires_at: Self::current_time()? + ttl_seconds,
            issued_at: Self::current_time()?,
            entropy_hash,
        };

        let payload_bytes = rkyv::to_bytes::<rkyv::rancor::Error>(&payload).map_err(|e| {
            warn!("[enclave] rkyv serialization failed (quantum token): {}", e);
            SecurityError::MemoryCorruption
        })?;

        // OMEGA-VIII: Hybrid Signature (Ed25519 + Dilithium2)
        let ed_sig = signer.sign(&payload_bytes);
        let pqc_sig = dilithium2::detached_sign(&payload_bytes, pqc_signer);

        let mut combined_sig = Vec::with_capacity(64 + pqc_sig.as_bytes().len());
        combined_sig.extend_from_slice(&ed_sig.to_bytes());
        combined_sig.extend_from_slice(pqc_sig.as_bytes());

        Ok(AgentToken {
            payload,
            algorithm: SignatureAlgorithm::QuantumCognitive,
            signature: combined_sig,
        })
    }

    /// Mints a new token for a subagent. (Executed by the Orchestrator)
    pub fn mint_token(
        signer: &SigningKey,
        assignee_hash: u64,
        resource_uri: &str,
        permitted_action: &str,
        ttl_seconds: u64,
    ) -> Result<AgentToken, SecurityError> {
        let payload = CapabilityPayload {
            assignee_hash,
            resource_uri: resource_uri.to_string(),
            permitted_action: permitted_action.to_string(),
            expires_at: Self::current_time()? + ttl_seconds,
            issued_at: Self::current_time()?,
            entropy_hash: [0u8; 32],
        };

        // Serialize the payload to bytes for signing
        let payload_bytes = rkyv::to_bytes::<rkyv::rancor::Error>(&payload).map_err(|e| {
            warn!("[enclave] rkyv serialization failed (mint_token): {}", e);
            SecurityError::MemoryCorruption
        })?;

        // Cryptographically sign the bytes using Ed25519 (Baseline Sovereignty)
        let signature = signer.sign(&payload_bytes);

        Ok(AgentToken {
            payload,
            algorithm: SignatureAlgorithm::Ed25519,
            signature: signature.to_bytes().to_vec(),
        })
    }

    /// Mathematically verifies a token presented by a subagent.
    /// Executed by the Wassette sandbox BEFORE running any tool.
    pub fn verify_token_and_action(
        &self,
        token: &AgentToken,
        agent_id: u64,
        requested_resource: &str,
        requested_action: &str,
    ) -> Result<(), SecurityError> {
        // 1. Time-to-Live Check
        let now = Self::current_time()?;
        if now > token.payload.expires_at {
            return Err(SecurityError::TokenExpired);
        }

        // 2. Identity Check (Prevent token theft)
        if token.payload.assignee_hash != agent_id {
            return Err(SecurityError::UnauthorizedAction(
                "Identity Mismatch".into(),
                "Token belongs to another agent".into(),
            ));
        }

        // 3. Action / Resource Scope Check (Fixes OpenClaw Issue #11102)
        // SEC-01: Use segment-boundary matching to prevent /workspace/ matching /workspace-admin/
        let resource_matches = requested_resource == token.payload.resource_uri
            || requested_resource.starts_with(&format!(
                "{}/",
                token.payload.resource_uri.trim_end_matches('/')
            ));
        if token.payload.permitted_action != requested_action || !resource_matches {
            return Err(SecurityError::UnauthorizedAction(
                requested_action.to_string(),
                token.payload.resource_uri.clone(),
            ));
        }

        // 4. Cryptographic Integrity Check
        let payload_bytes = rkyv::to_bytes::<rkyv::rancor::Error>(&token.payload).map_err(|e| {
            warn!("[enclave] rkyv serialization failed (verify_token): {}", e);
            SecurityError::MemoryCorruption
        })?;

        match token.algorithm {
            SignatureAlgorithm::Ed25519 => {
                let sig_bytes: [u8; 64] =
                    token.signature.as_slice().try_into().map_err(|_| {
                        SecurityError::InvalidSignature(SignatureAlgorithm::Ed25519)
                    })?;

                let signature = Signature::from_bytes(&sig_bytes);

                self.root_authority
                    .verify(&payload_bytes, &signature)
                    .map_err(|_| SecurityError::InvalidSignature(SignatureAlgorithm::Ed25519))
            }
            SignatureAlgorithm::QuantumCognitive => {
                let pqc_key = self
                    .pqc_authority
                    .as_ref()
                    .ok_or(SecurityError::UnsupportedAlgorithm(token.algorithm))?;

                if token.signature.len() < 64 {
                    return Err(SecurityError::InvalidSignature(token.algorithm));
                }

                // 1. Verify Ed25519 component
                let ed_sig_bytes: [u8; 64] = token.signature[0..64]
                    .try_into()
                    .map_err(|_| SecurityError::InvalidSignature(token.algorithm))?;
                let ed_sig = Signature::from_bytes(&ed_sig_bytes);
                self.root_authority
                    .verify(&payload_bytes, &ed_sig)
                    .map_err(|_| SecurityError::InvalidSignature(token.algorithm))?;

                // 2. Verify Dilithium2 component
                let pqc_sig_bytes = &token.signature[64..];
                let pqc_sig = dilithium2::DetachedSignature::from_bytes(pqc_sig_bytes)
                    .map_err(|_| SecurityError::InvalidSignature(token.algorithm))?;

                dilithium2::verify_detached_signature(&pqc_sig, &payload_bytes, pqc_key)
                    .map_err(|_| SecurityError::InvalidSignature(token.algorithm))?;

                // 🛡️ Quantum-Cognitive Defense: Entropy Hash Validation
                if token.payload.entropy_hash == [0u8; 32] {
                    return Err(SecurityError::InvalidSignature(token.algorithm));
                }
                Ok(())
            }
            SignatureAlgorithm::Dilithium2 => {
                // SEC-08: Implement Dilithium2 signature verification
                let pqc_key = self
                    .pqc_authority
                    .as_ref()
                    .ok_or(SecurityError::UnsupportedAlgorithm(token.algorithm))?;

                let pqc_sig = dilithium2::DetachedSignature::from_bytes(&token.signature)
                    .map_err(|_| SecurityError::InvalidSignature(token.algorithm))?;

                dilithium2::verify_detached_signature(&pqc_sig, &payload_bytes, pqc_key)
                    .map_err(|_| SecurityError::InvalidSignature(token.algorithm))
            }
            SignatureAlgorithm::Hybrid => {
                // SEC-08: Hybrid verification — both Ed25519 AND Dilithium2 must pass
                let pqc_key = self
                    .pqc_authority
                    .as_ref()
                    .ok_or(SecurityError::UnsupportedAlgorithm(token.algorithm))?;

                if token.signature.len() < 64 {
                    return Err(SecurityError::InvalidSignature(token.algorithm));
                }

                // 1. Verify Ed25519 component (first 64 bytes)
                let ed_sig_bytes: [u8; 64] = token.signature[0..64]
                    .try_into()
                    .map_err(|_| SecurityError::InvalidSignature(token.algorithm))?;
                let ed_sig = Signature::from_bytes(&ed_sig_bytes);
                self.root_authority
                    .verify(&payload_bytes, &ed_sig)
                    .map_err(|_| SecurityError::InvalidSignature(token.algorithm))?;

                // 2. Verify Dilithium2 component (remaining bytes)
                let pqc_sig_bytes = &token.signature[64..];
                let pqc_sig = dilithium2::DetachedSignature::from_bytes(pqc_sig_bytes)
                    .map_err(|_| SecurityError::InvalidSignature(token.algorithm))?;

                dilithium2::verify_detached_signature(&pqc_sig, &payload_bytes, pqc_key)
                    .map_err(|_| SecurityError::InvalidSignature(token.algorithm))
            }
        }
    }

    /// Rotates the root authority key. (Administrative only)
    pub fn rotate_root_authority(&mut self, next_authority: VerifyingKey) {
        tracing::info!(
            "🔄 SecurityAuthority: Rotating root authority to {:?}",
            next_authority
        );
        self.root_authority = next_authority;
    }

    /// Derives a new signing key from a base key and hybrid entropy.
    /// 🧬 OMEGA-VIII: Combines system entropy with deterministic seed.
    pub fn derive_entropic_key(base: &SigningKey, entropy: &[u8]) -> SigningKey {
        let mut hasher = blake3::Hasher::new();
        hasher.update(base.as_bytes());
        hasher.update(entropy);
        let hash = hasher.finalize();
        let mut key_bytes = [0u8; 32];
        key_bytes.copy_from_slice(hash.as_bytes());
        SigningKey::from_bytes(&key_bytes)
    }
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)]
mod tests {
    use super::*;
    use ed25519_dalek::SigningKey;
    use rand::thread_rng;

    #[test]
    fn test_token_minting_and_verification() {
        let mut rng = thread_rng();
        let signing_key = SigningKey::generate(&mut rng);
        let enclave = SecurityAuthority::new(signing_key.verifying_key(), None);

        let token =
            SecurityAuthority::mint_token(&signing_key, 12345, "/workspace/data/", "read", 3600)
                .expect("Failed to mint token");

        assert!(enclave
            .verify_token_and_action(&token, 12345, "/workspace/data/file.txt", "read")
            .is_ok());
    }

    #[test]
    fn test_token_expiration() {
        let mut rng = thread_rng();
        let signing_key = SigningKey::generate(&mut rng);
        let enclave = SecurityAuthority::new(signing_key.verifying_key(), None);

        let mut token = SecurityAuthority::mint_token(
            &signing_key,
            12345,
            "/",
            "read",
            100, // Not yet expired
        )
        .expect("mint_token should succeed");

        // Force expiration by setting time to the past
        token.payload.expires_at = SecurityAuthority::current_time()
            .expect("current_time should succeed")
            .saturating_sub(1);

        assert!(matches!(
            enclave.verify_token_and_action(&token, 12345, "/file", "read"),
            Err(SecurityError::TokenExpired)
        ));
    }

    #[test]
    fn test_identity_mismatch() {
        let mut rng = thread_rng();
        let signing_key = SigningKey::generate(&mut rng);
        let enclave = SecurityAuthority::new(signing_key.verifying_key(), None);

        let token = SecurityAuthority::mint_token(&signing_key, 111, "/", "read", 100)
            .expect("mint_token should succeed");

        assert!(enclave
            .verify_token_and_action(&token, 222, "/file", "read")
            .is_err());
    }

    #[test]
    fn test_signature_forgery() {
        let mut rng = thread_rng();
        let signing_key = SigningKey::generate(&mut rng);
        let enclave = SecurityAuthority::new(signing_key.verifying_key(), None);

        let mut token = SecurityAuthority::mint_token(&signing_key, 123, "/", "read", 100)
            .expect("mint_token should succeed");

        // Tamper with payload action
        token.payload.permitted_action = "write".to_string();

        assert!(matches!(
            enclave.verify_token_and_action(&token, 123, "/file", "write"),
            Err(SecurityError::InvalidSignature(SignatureAlgorithm::Ed25519))
        ));
    }

    #[test]
    fn test_quantum_token() {
        let mut rng = thread_rng();
        let signing_key = SigningKey::generate(&mut rng);
        let (pqc_pk, pqc_sk) = dilithium2::keypair();
        let enclave = SecurityAuthority::new(signing_key.verifying_key(), Some(pqc_pk));

        let cadence = b"user_typing_pattern_123";
        let token = SecurityAuthority::mint_quantum_token(
            &signing_key,
            &pqc_sk,
            555,
            "/secure",
            "execute",
            3600,
            cadence,
        )
        .expect("Failed to mint quantum token");

        assert_eq!(token.algorithm, SignatureAlgorithm::QuantumCognitive);
        assert_ne!(token.payload.entropy_hash, [0u8; 32]);

        assert!(enclave
            .verify_token_and_action(&token, 555, "/secure/tool", "execute")
            .is_ok());
    }
}
