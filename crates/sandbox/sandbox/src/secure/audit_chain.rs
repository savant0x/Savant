use sha2::{Digest, Sha256};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, thiserror::Error)]
pub enum AuditError {
    #[error("chain integrity violation at index {index}: {detail}")]
    IntegrityViolation { index: u64, detail: String },
    #[error("serialization failed: {0}")]
    SerializationFailed(String),
    #[error("I/O error: {0}")]
    Io(String),
}

/// The type of action recorded in an audit entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ActionType {
    Exec,
    FsWrite,
    NetConnect,
    SecretAccess,
    ProcessSpawn,
    ResourceExhausted,
}

/// A single entry in the Merkle-chained audit log.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AuditEntry {
    pub index: u64,
    pub timestamp: u64,
    pub action_type: ActionType,
    pub payload: Vec<u8>,
    pub prev_hash: [u8; 32],
    pub hash: [u8; 32],
}

/// Append-only, hash-chained audit log for tamper-proof telemetry.
///
/// Each entry's hash covers `index || timestamp || action_type || payload || prev_hash`,
/// forming a Merkle chain where tampering with any entry invalidates all subsequent hashes.
pub struct AuditChain {
    entries: Vec<AuditEntry>,
    current_hash: [u8; 32],
}

impl AuditChain {
    /// Creates a new chain with a genesis entry (index 0).
    pub fn new() -> Self {
        let genesis = Self::compute_entry(0, ActionType::Exec, &[], [0u8; 32]);
        let current_hash = genesis.hash;
        Self {
            entries: vec![genesis],
            current_hash,
        }
    }

    /// Appends a new entry to the chain and returns a reference to it.
    pub fn append(
        &mut self,
        action: ActionType,
        payload: &[u8],
    ) -> Result<&AuditEntry, AuditError> {
        let index = self.entries.len() as u64;
        let entry = Self::compute_entry(index, action, payload, self.current_hash);
        self.current_hash = entry.hash;
        self.entries.push(entry);
        Ok(&self.entries[self.entries.len() - 1])
    }

    /// Verifies the entire chain integrity. Returns `Ok(())` if valid,
    /// or `Err` with the index and detail of the first violation found.
    pub fn verify(&self) -> Result<(), AuditError> {
        let mut expected_prev = [0u8; 32];
        for (i, entry) in self.entries.iter().enumerate() {
            // Verify prev_hash linkage
            if entry.prev_hash != expected_prev {
                return Err(AuditError::IntegrityViolation {
                    index: i as u64,
                    detail: format!(
                        "prev_hash mismatch: expected {}, got {}",
                        hex::encode(expected_prev),
                        hex::encode(entry.prev_hash)
                    ),
                });
            }
            // Verify hash computation
            let recomputed = Self::compute_entry(
                entry.index,
                entry.action_type,
                &entry.payload,
                entry.prev_hash,
            );
            if entry.hash != recomputed.hash {
                return Err(AuditError::IntegrityViolation {
                    index: i as u64,
                    detail: format!(
                        "hash mismatch: expected {}, got {}",
                        hex::encode(recomputed.hash),
                        hex::encode(entry.hash)
                    ),
                });
            }
            expected_prev = entry.hash;
        }
        Ok(())
    }

    /// Returns a slice of all entries.
    pub fn entries(&self) -> &[AuditEntry] {
        &self.entries
    }

    /// Returns the number of entries (including genesis).
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Returns `true` if the chain contains only the genesis entry.
    pub fn is_genesis_only(&self) -> bool {
        self.entries.len() == 1
    }

    /// Returns `true` if the chain is empty (should never happen after `new()`).
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Serializes the entire chain to JSON.
    pub fn to_json(&self) -> Result<String, AuditError> {
        serde_json::to_string_pretty(&self.entries)
            .map_err(|e| AuditError::SerializationFailed(e.to_string()))
    }

    /// Deserializes a chain from JSON and verifies its integrity.
    pub fn from_json(json: &str) -> Result<Self, AuditError> {
        let entries: Vec<AuditEntry> = serde_json::from_str(json)
            .map_err(|e| AuditError::SerializationFailed(e.to_string()))?;
        if entries.is_empty() {
            return Err(AuditError::SerializationFailed(
                "chain must contain at least a genesis entry".into(),
            ));
        }
        let current_hash = match entries.last() {
            Some(e) => e.hash,
            None => {
                return Err(AuditError::SerializationFailed(
                    "chain must contain at least a genesis entry".into(),
                ))
            }
        };
        let chain = Self {
            entries,
            current_hash,
        };
        chain.verify()?;
        Ok(chain)
    }

    fn compute_entry(
        index: u64,
        action_type: ActionType,
        payload: &[u8],
        prev_hash: [u8; 32],
    ) -> AuditEntry {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let mut hasher = Sha256::new();
        hasher.update(index.to_le_bytes());
        hasher.update(timestamp.to_le_bytes());
        hasher.update([action_type as u8]);
        hasher.update(payload);
        hasher.update(prev_hash);
        let hash: [u8; 32] = hasher.finalize().into();

        AuditEntry {
            index,
            timestamp,
            action_type,
            payload: payload.to_vec(),
            prev_hash,
            hash,
        }
    }
}

impl Default for AuditChain {
    fn default() -> Self {
        Self::new()
    }
}

/// Trait for pluggable audit output sinks.
pub trait AuditSink: Send + Sync {
    fn emit(&self, entry: &AuditEntry) -> Result<(), AuditError>;
}

/// Sink that writes entries as JSON lines to a file.
pub struct JsonFileSink {
    path: std::path::PathBuf,
}

impl JsonFileSink {
    pub fn new(path: impl Into<std::path::PathBuf>) -> Self {
        Self { path: path.into() }
    }
}

impl AuditSink for JsonFileSink {
    fn emit(&self, entry: &AuditEntry) -> Result<(), AuditError> {
        let line = serde_json::to_string(entry)
            .map_err(|e| AuditError::SerializationFailed(e.to_string()))?;
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .map_err(|e| AuditError::Io(e.to_string()))?;
        writeln!(file, "{}", line).map_err(|e| AuditError::Io(e.to_string()))?;
        Ok(())
    }
}

/// In-memory sink for testing.
pub struct VecSink {
    pub entries: std::sync::Mutex<Vec<AuditEntry>>,
}

impl VecSink {
    pub fn new() -> Self {
        Self {
            entries: std::sync::Mutex::new(Vec::new()),
        }
    }
}

impl Default for VecSink {
    fn default() -> Self {
        Self::new()
    }
}

impl AuditSink for VecSink {
    fn emit(&self, entry: &AuditEntry) -> Result<(), AuditError> {
        self.entries
            .lock()
            .map_err(|e| AuditError::Io(format!("mutex poisoned: {}", e)))?
            .push(entry.clone());
        Ok(())
    }
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)]
mod tests {
    use super::*;

    #[test]
    fn test_genesis_entry() {
        let chain = AuditChain::new();
        assert_eq!(chain.len(), 1);
        assert!(chain.is_genesis_only());
        let genesis = &chain.entries()[0];
        assert_eq!(genesis.index, 0);
        assert_eq!(genesis.prev_hash, [0u8; 32]);
        assert_ne!(genesis.hash, [0u8; 32]);
    }

    #[test]
    fn test_append_and_verify() -> Result<(), AuditError> {
        let mut chain = AuditChain::new();
        chain.append(ActionType::Exec, b"ls -la")?;
        chain.append(ActionType::FsWrite, b"/tmp/out.txt")?;
        chain.append(ActionType::NetConnect, b"10.0.0.1:443")?;

        assert_eq!(chain.len(), 4); // genesis + 3
        chain.verify()?;
        Ok(())
    }

    #[test]
    fn test_tampered_payload_detected() -> Result<(), AuditError> {
        let mut chain = AuditChain::new();
        chain.append(ActionType::Exec, b"safe-cmd")?;

        // Tamper with entry 1's payload
        chain.entries[1].payload = b"malicious-cmd".to_vec();

        match chain.verify() {
            Err(AuditError::IntegrityViolation { index, .. }) => assert_eq!(index, 1),
            other => panic!("expected IntegrityViolation at index 1, got: {:?}", other),
        }
        Ok(())
    }

    #[test]
    fn test_tampered_prev_hash_detected() -> Result<(), AuditError> {
        let mut chain = AuditChain::new();
        chain.append(ActionType::Exec, b"cmd1")?;
        chain.append(ActionType::FsWrite, b"/tmp/f")?;

        // Tamper with entry 2's prev_hash
        chain.entries[2].prev_hash = [0xFF; 32];

        match chain.verify() {
            Err(AuditError::IntegrityViolation { index, .. }) => assert_eq!(index, 2),
            other => panic!("expected IntegrityViolation at index 2, got: {:?}", other),
        }
        Ok(())
    }

    #[test]
    fn test_json_round_trip() -> Result<(), AuditError> {
        let mut chain = AuditChain::new();
        chain.append(ActionType::Exec, b"test-cmd")?;
        chain.append(ActionType::SecretAccess, b"api-key")?;

        let json = chain.to_json()?;
        let restored = AuditChain::from_json(&json)?;

        assert_eq!(restored.len(), chain.len());
        for (a, b) in chain.entries().iter().zip(restored.entries().iter()) {
            assert_eq!(a.index, b.index);
            assert_eq!(a.hash, b.hash);
            assert_eq!(a.payload, b.payload);
        }
        Ok(())
    }

    #[test]
    fn test_vec_sink() -> Result<(), AuditError> {
        let mut chain = AuditChain::new();
        let sink = VecSink::new();

        let entry = chain.append(ActionType::Exec, b"echo hi")?;
        sink.emit(entry)?;

        assert_eq!(
            sink.entries
                .lock()
                .map_err(|e| AuditError::Io(format!("mutex poisoned: {}", e)))?
                .len(),
            1
        );
        assert_eq!(
            sink.entries
                .lock()
                .map_err(|e| AuditError::Io(format!("mutex poisoned: {}", e)))?[0]
                .action_type,
            ActionType::Exec
        );
        Ok(())
    }

    #[test]
    fn test_action_type_serialization() -> Result<(), AuditError> {
        let mut chain = AuditChain::new();
        chain.append(ActionType::ProcessSpawn, b"pid=42")?;
        chain.append(ActionType::ResourceExhausted, b"OOM")?;

        let json = chain.to_json()?;
        assert!(json.contains("\"ProcessSpawn\""));
        assert!(json.contains("\"ResourceExhausted\""));
        Ok(())
    }
}
