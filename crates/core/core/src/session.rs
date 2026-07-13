use crate::types::SessionId;

/// 🌀 SessionMapper: Platform-Agnostic Context Anchoring
///
/// Translates platform-specific identifiers into unified, platform-prefixed
/// session anchors to ensure high-fidelity context persistence.
pub struct SessionMapper;

impl SessionMapper {
    /// Maps a platform and ID into a sanitized OMEGA anchor.
    /// Prefixes prevent collisions across Discord, Matrix, and WebUI.
    /// Returns a hash-based session ID if the sanitized ID is empty.
    pub fn map(platform: &str, id: &str) -> SessionId {
        let sanitized = Self::sanitize(id).unwrap_or_else(|| {
            let hash = blake3::hash(id.as_bytes());
            let hash_bytes = hash.as_bytes();
            let hash64 = (hash_bytes[0] as u64)
                | ((hash_bytes[1] as u64) << 8)
                | ((hash_bytes[2] as u64) << 16)
                | ((hash_bytes[3] as u64) << 24)
                | ((hash_bytes[4] as u64) << 32)
                | ((hash_bytes[5] as u64) << 40)
                | ((hash_bytes[6] as u64) << 48)
                | ((hash_bytes[7] as u64) << 56);
            format!("hash-{:x}", hash64)
        });
        SessionId(format!("{}:{}", platform.to_lowercase(), sanitized))
    }

    /// 🛡️ Sanitizes a session ID to prevent path traversal or keyspace corruption.
    /// Returns `None` if the sanitized result is empty.
    pub fn sanitize(id: &str) -> Option<String> {
        let result: String = id
            .chars()
            .filter(|c| c.is_alphanumeric() || *c == '-' || *c == '_')
            .collect();
        if result.is_empty() {
            None
        } else {
            Some(result)
        }
    }

    /// Verifies if a session ID is well-formed within the UCH context.
    pub fn is_valid(session: &SessionId) -> bool {
        let s = &session.0;
        if !s.contains(':') {
            return false;
        }
        let parts: Vec<&str> = s.splitn(2, ':').collect();
        if parts.len() != 2 || parts[0].is_empty() || parts[1].is_empty() {
            return false;
        }
        s.chars()
            .all(|c| c.is_alphanumeric() || c == ':' || c == '-' || c == '_')
    }
}

pub fn sanitize_session_id(id: &str) -> Option<String> {
    SessionMapper::sanitize(id)
}
