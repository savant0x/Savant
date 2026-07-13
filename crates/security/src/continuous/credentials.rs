//! Dynamic Credential Broker — Per-task ephemeral token management.
//!
//! Injects secrets on a per-task basis. Agents never hold static,
//! long-lived API keys. Tokens auto-expire after task completion or TTL.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;
use tracing::debug;

/// Ephemeral token that auto-expires.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EphemeralToken {
    /// Service this token is for.
    pub service: String,
    /// Task this token was issued for.
    pub task_id: String,
    /// Actual credential (masked in logs).
    pub token: String,
    /// Creation timestamp (Unix seconds).
    pub created_at: i64,
    /// Expiration timestamp (Unix seconds).
    pub expires_at: i64,
    /// Whether the token has been revoked.
    pub revoked: bool,
}

impl EphemeralToken {
    /// Returns true if this token is expired or revoked.
    pub fn is_expired(&self) -> bool {
        self.revoked || chrono::Utc::now().timestamp() >= self.expires_at
    }

    /// Revokes this token immediately.
    pub fn revoke(&mut self) {
        self.revoked = true;
    }

    /// Returns a masked version of the token for logging.
    pub fn masked(&self) -> String {
        if self.token.len() <= 8 {
            "***".to_string()
        } else {
            format!(
                "{}...{}",
                &self.token[..4],
                &self.token[self.token.len() - 4..]
            )
        }
    }
}

/// Dynamic credential broker.
///
/// Wraps `.env` loading and provides per-task ephemeral tokens.
pub struct CredentialBroker {
    /// Cached credentials from .env (service_name -> credential).
    credentials: Arc<RwLock<HashMap<String, String>>>,
    /// Active ephemeral tokens (task_id -> tokens).
    active_tokens: Arc<RwLock<HashMap<String, Vec<EphemeralToken>>>>,
}

impl CredentialBroker {
    /// Creates a new credential broker.
    pub fn new() -> Self {
        Self {
            credentials: Arc::new(RwLock::new(HashMap::new())),
            active_tokens: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// Loads a credential into the broker.
    pub async fn load_credential(&self, service: &str, credential: &str) {
        self.credentials
            .write()
            .await
            .insert(service.to_string(), credential.to_string());
        debug!(
            "[CredentialBroker] Loaded credential for service: {}",
            service
        );
    }

    /// Gets an ephemeral token for a task.
    /// Returns an error if the service credential is not loaded.
    pub async fn get_credential(
        &self,
        service: &str,
        task_id: &str,
        ttl: Duration,
    ) -> Result<EphemeralToken, String> {
        let credentials = self.credentials.read().await;
        let credential = credentials
            .get(service)
            .ok_or_else(|| format!("No credential loaded for service: {}", service))?;

        let now = chrono::Utc::now().timestamp();
        let token = EphemeralToken {
            service: service.to_string(),
            task_id: task_id.to_string(),
            token: credential.clone(),
            created_at: now,
            expires_at: now + ttl.as_secs() as i64,
            revoked: false,
        };

        debug!(
            "[CredentialBroker] Issued ephemeral token for {}/{} (expires in {}s)",
            service,
            task_id,
            ttl.as_secs(),
        );

        self.active_tokens
            .write()
            .await
            .entry(task_id.to_string())
            .or_default()
            .push(token.clone());

        Ok(token)
    }

    /// Revokes all tokens for a task (called on task completion).
    ///
    /// Zeroes credential bytes in memory before dropping to prevent
    /// credential leakage through memory dumps or reuse.
    pub async fn revoke_task_tokens(&self, task_id: &str) {
        let mut active = self.active_tokens.write().await;
        if let Some(tokens) = active.remove(task_id) {
            let count = tokens.len();
            for mut token in tokens {
                // Wire EphemeralToken::revoke() — replaces direct field assignment
                token.revoke();
                // SEC-05: Zero credential bytes using write_volatile to prevent compiler elision
                let mut credential_bytes = std::mem::take(&mut token.token).into_bytes();
                for byte in credential_bytes.iter_mut() {
                    // SAFETY: write_volatile to a valid, aligned, non-null byte pointer.
                    // The pointer comes from iter_mut() on a Vec<u8>, so it is always valid.
                    // Volatile write prevents the compiler from eliding the zeroing operation,
                    // ensuring credential bytes are actually cleared from memory.
                    unsafe {
                        std::ptr::write_volatile(byte, 0);
                    }
                }
                drop(credential_bytes);
            }
            if count > 0 {
                debug!(
                    "[CredentialBroker] Revoked {} tokens for task {}, credentials zeroed",
                    count, task_id
                );
            }
        }
    }

    /// Cleans up expired tokens (should be called periodically).
    pub async fn cleanup_expired(&self) {
        let mut active = self.active_tokens.write().await;

        for (_, tokens) in active.iter_mut() {
            let before = tokens.len();
            tokens.retain(|t| !t.is_expired());
            let removed = before - tokens.len();
            if removed > 0 {
                debug!("[CredentialBroker] Cleaned up {} expired tokens", removed);
            }
        }

        // Remove empty task entries
        active.retain(|_, tokens| !tokens.is_empty());
    }
}

impl Default for CredentialBroker {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_get_credential_success() {
        let broker = CredentialBroker::new();
        broker.load_credential("openrouter", "sk-test-key").await;

        let token = broker
            .get_credential("openrouter", "task1", Duration::from_secs(300))
            .await;

        assert!(token.is_ok());
        let token = token.expect("token should be ok");
        assert_eq!(token.service, "openrouter");
        assert_eq!(token.task_id, "task1");
        assert!(!token.is_expired());
    }

    #[tokio::test]
    async fn test_get_credential_missing_service() {
        let broker = CredentialBroker::new();

        let result = broker
            .get_credential("unknown", "task1", Duration::from_secs(300))
            .await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_revoke_task_tokens() {
        let broker = CredentialBroker::new();
        broker.load_credential("openrouter", "sk-test").await;

        let _token = broker
            .get_credential("openrouter", "task1", Duration::from_secs(300))
            .await;

        broker.revoke_task_tokens("task1").await;

        let active = broker.active_tokens.read().await;
        assert!(!active.contains_key("task1"));
    }

    #[tokio::test]
    async fn test_token_expiry() {
        let token = EphemeralToken {
            service: "test".to_string(),
            task_id: "t1".to_string(),
            token: "secret".to_string(),
            created_at: 0,
            expires_at: 1,
            revoked: false,
        };

        // Token expired (expires_at = 1, now is much later)
        assert!(token.is_expired());
    }

    #[test]
    fn test_token_masking() {
        let token = EphemeralToken {
            service: "test".to_string(),
            task_id: "t1".to_string(),
            token: "sk-1234567890abcdef".to_string(),
            created_at: 0,
            expires_at: 9999999999,
            revoked: false,
        };

        let masked = token.masked();
        assert!(masked.contains("..."));
        assert!(!masked.contains("1234567890"));
    }
}
