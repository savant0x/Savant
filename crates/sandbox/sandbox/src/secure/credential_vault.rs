//! Credential vault for secret isolation.
//!
//! Provides a centralized store for secrets that are injected into the sandbox
//! at runtime. Secrets are stored in secure memory and never exposed in logs.
//! The vault supports placeholder substitution ({{key}}) and redaction.

use std::collections::HashMap;
use std::sync::RwLock;
use zeroize::Zeroizing;

/// A secret value stored in secure memory that is zeroized on drop.
type SecureMemory = Zeroizing<Vec<u8>>;

/// Centralized credential vault for sandboxed environments.
///
/// Secrets are injected at startup and referenced by key. The vault provides:
/// - Placeholder substitution: replaces `{{key}}` with actual values in strings
/// - Redaction: replaces actual values with `{{key}}` for safe logging
/// - Secure storage: all values are zeroized on drop
pub struct CredentialVault {
    secrets: RwLock<HashMap<String, SecureMemory>>,
}

impl CredentialVault {
    /// Creates a new empty credential vault.
    pub fn new() -> Self {
        Self {
            secrets: RwLock::new(HashMap::new()),
        }
    }

    /// Injects a secret into the vault. Overwrites any existing secret with the same key.
    pub fn inject_secret(&self, key: &str, value: &[u8]) {
        let mut secrets = self.secrets.write().unwrap_or_else(|e| e.into_inner());
        secrets.insert(key.to_string(), Zeroizing::new(value.to_vec()));
    }

    /// Returns the placeholder string for a given key: `{{key}}`.
    pub fn get_placeholder(&self, key: &str) -> String {
        format!("{{{{{}}}}}", key)
    }

    /// Substitutes all `{{key}}` placeholders in the input with actual secret values.
    /// Returns the input unchanged if no placeholders match.
    pub fn substitute(&self, input: &str) -> String {
        let secrets = self.secrets.read().unwrap_or_else(|e| e.into_inner());
        let mut result = input.to_string();
        for (key, value) in secrets.iter() {
            let placeholder = format!("{{{{{}}}}}", key);
            if result.contains(&placeholder) {
                let value_str = String::from_utf8_lossy(value);
                result = result.replace(&placeholder, &value_str);
            }
        }
        result
    }

    /// Redacts actual secret values in the input, replacing them with `{{key}}`.
    /// Useful for sanitizing log output.
    pub fn redact(&self, input: &str) -> String {
        let secrets = self.secrets.read().unwrap_or_else(|e| e.into_inner());
        let mut result = input.to_string();
        for (key, value) in secrets.iter() {
            if let Ok(value_str) = std::str::from_utf8(value) {
                if !value_str.is_empty() && result.contains(value_str) {
                    let placeholder = format!("{{{{{}}}}}", key);
                    result = result.replace(value_str, &placeholder);
                }
            }
        }
        result
    }

    /// Returns the number of secrets stored in the vault.
    pub fn secret_count(&self) -> usize {
        self.secrets.read().unwrap_or_else(|e| e.into_inner()).len()
    }

    /// Removes all secrets from the vault.
    pub fn clear(&self) {
        self.secrets
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
    }
}

impl Default for CredentialVault {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_substitute_single_secret() {
        let vault = CredentialVault::new();
        vault.inject_secret("api_key", b"sk-abc123");
        let result = vault.substitute("Authorization: Bearer {{api_key}}");
        assert_eq!(result, "Authorization: Bearer sk-abc123");
    }

    #[test]
    fn test_substitute_multiple_secrets() {
        let vault = CredentialVault::new();
        vault.inject_secret("user", b"admin");
        vault.inject_secret("pass", b"secret123");
        let result = vault.substitute("user={{user}}&pass={{pass}}");
        assert_eq!(result, "user=admin&pass=secret123");
    }

    #[test]
    fn test_substitute_no_match() {
        let vault = CredentialVault::new();
        vault.inject_secret("api_key", b"sk-abc123");
        let result = vault.substitute("no placeholders here");
        assert_eq!(result, "no placeholders here");
    }

    #[test]
    fn test_redact() {
        let vault = CredentialVault::new();
        vault.inject_secret("api_key", b"sk-abc123");
        let result = vault.redact("Using key sk-abc123 for auth");
        assert_eq!(result, "Using key {{api_key}} for auth");
    }

    #[test]
    fn test_redact_multiple() {
        let vault = CredentialVault::new();
        vault.inject_secret("user", b"admin");
        vault.inject_secret("pass", b"secret");
        let result = vault.redact("login: admin / secret");
        assert_eq!(result, "login: {{user}} / {{pass}}");
    }

    #[test]
    fn test_get_placeholder() {
        let vault = CredentialVault::new();
        assert_eq!(vault.get_placeholder("key"), "{{key}}");
    }

    #[test]
    fn test_secret_count() {
        let vault = CredentialVault::new();
        assert_eq!(vault.secret_count(), 0);
        vault.inject_secret("a", b"1");
        vault.inject_secret("b", b"2");
        assert_eq!(vault.secret_count(), 2);
    }

    #[test]
    fn test_clear() {
        let vault = CredentialVault::new();
        vault.inject_secret("a", b"1");
        vault.clear();
        assert_eq!(vault.secret_count(), 0);
    }

    #[test]
    fn test_overwrite_secret() {
        let vault = CredentialVault::new();
        vault.inject_secret("key", b"old");
        vault.inject_secret("key", b"new");
        let result = vault.substitute("{{key}}");
        assert_eq!(result, "new");
    }
}
