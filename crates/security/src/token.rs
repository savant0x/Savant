use bytecheck::CheckBytes;
use rkyv::{Archive, Deserialize, Serialize};

/// The payload of the token, optimized for zero-copy deserialization.
#[derive(Archive, Serialize, Deserialize, CheckBytes, Debug, Clone)]
#[bytecheck(crate = bytecheck)]
#[repr(C)]
pub struct CapabilityPayload {
    /// The unique hash of the agent this token was issued to.
    pub assignee_hash: u64,
    /// The specific resource allowed (e.g., "/workspace/data/")
    pub resource_uri: String,
    /// The specific action allowed (e.g., "read", "append", "execute")
    pub permitted_action: String,
    /// UNIX timestamp of expiration
    pub expires_at: u64,
    /// UNIX timestamp of issuance (for rotation calculations)
    pub issued_at: u64,
    /// OMEGA-VII: Binding to Quantum-Cognitive Stream
    pub entropy_hash: [u8; 32],
}

/// Supported cryptographic algorithms for token signatures.
#[derive(
    Archive, Serialize, Deserialize, CheckBytes, rkyv::Portable, Debug, Clone, Copy, PartialEq, Eq,
)]
#[bytecheck(crate = bytecheck)]
#[repr(u8)]
pub enum SignatureAlgorithm {
    /// Standard Ed25519 (64 bytes)
    Ed25519 = 0,
    /// PQC-ready Dilithium2 (Future-proof)
    Dilithium2 = 1,
    /// Hybrid (Ed25519 + Dilithium2) for absolute sovereignty
    Hybrid = 2,
    /// OMEGA-VII: Quantum-Cognitive Entangled signature
    QuantumCognitive = 3,
}

/// The complete token, including cryptographic signatures.
///
/// Omega: Transitioned from fixed-size Ed25519 to a multi-algorithm hybrid structure.
#[derive(Archive, Serialize, Deserialize, CheckBytes, Debug, Clone)]
#[bytecheck(crate = bytecheck)]
#[repr(C)]
pub struct AgentToken {
    /// The capability payload being signed
    pub payload: CapabilityPayload,
    /// The algorithm used for signing
    pub algorithm: SignatureAlgorithm,
    /// The raw signature bytes (variable length to support PQC)
    pub signature: Vec<u8>,
}

impl AgentToken {
    /// Verifies if the token grants the requested capability.
    ///
    /// OMEGA-Tier: Checks resource URI and permitted action against the payload.
    /// On clock error, returns `false` (fail-closed) and logs the error.
    pub fn verify_capability(&self, resource: &str, action: &str) -> bool {
        // AAA: Resource URI matching (prefix-based for hierarchy support)
        let resource_match = resource.starts_with(&self.payload.resource_uri);

        // AAA: Action matching (exact match only - no wildcards permitted)
        let action_match = self.payload.permitted_action == action;

        // AAA: Expiration check — fail closed on clock error
        let now = match std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
            Ok(d) => d.as_secs(),
            Err(e) => {
                tracing::error!("Token clock error: {}", e);
                return false;
            }
        };
        let not_expired = self.payload.expires_at > now;

        resource_match && action_match && not_expired
    }

    /// Verifies that the token belongs to the specified agent.
    pub fn assignee_matches(&self, agent_id_hash: u64) -> bool {
        self.payload.assignee_hash == agent_id_hash
    }

    /// Checks if the token should be rotated based on TTL.
    ///
    /// Returns true when 80% of the token's lifetime has elapsed.
    /// This provides a safety margin before actual expiration.
    /// On clock error, returns `true` (rotate to be safe) and logs the error.
    pub fn should_rotate(&self) -> bool {
        let now = match std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
            Ok(d) => d.as_secs(),
            Err(e) => {
                tracing::error!("Token clock error during rotation check: {}", e);
                return true;
            }
        };

        let lifetime = self
            .payload
            .expires_at
            .saturating_sub(self.payload.issued_at);
        if lifetime == 0 {
            return true; // Rotate immediately if no lifetime
        }

        let elapsed = now.saturating_sub(self.payload.issued_at);
        let threshold = lifetime * 80 / 100; // 80% of lifetime
        elapsed >= threshold
    }

    /// Checks if the token has expired.
    ///
    /// On clock error, returns `true` (treat as expired) and logs the error.
    pub fn is_expired(&self) -> bool {
        let now = match std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
            Ok(d) => d.as_secs(),
            Err(e) => {
                tracing::error!("Token clock error during expiry check: {}", e);
                return true;
            }
        };
        self.payload.expires_at <= now
    }
}
