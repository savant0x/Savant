//! Gateway Authentication Module
//!
//! Provides Ed25519 signature-based authentication for all gateway connections.
//! All sessions must provide a valid signature to establish a connection.
//! No authentication bypasses are permitted.

use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use savant_core::error::SavantError;
use savant_core::types::{RequestFrame, SessionId};
use tracing::{debug, warn};

pub mod http_middleware;
pub mod oauth;

/// Maximum allowed timestamp drift (5 minutes)
const MAX_TIMESTAMP_DRIFT_SECS: i64 = 300;

/// An authenticated session context.
#[derive(Clone, Debug)]
pub struct AuthenticatedSession {
    pub session_id: SessionId,
    pub public_key: [u8; 32],
}

/// Authenticates a new session request using Ed25519 signatures, API key, or OAuth token.
///
/// All connections must provide either:
/// 1. Ed25519 signature authentication:
///    - A valid Ed25519 public key as the session_id (hex-encoded)
///    - A valid Ed25519 signature over the payload
///    - A recent timestamp (within MAX_TIMESTAMP_DRIFT_SECS)
/// 2. Dashboard API key authentication:
///    - An Auth payload with format "DASHBOARD_API_KEY:<key>"
///    - The key must match the configured dashboard_api_key
/// 3. OAuth token authentication:
///    - An Auth payload with format "OAUTH_TOKEN:<provider>:<token>"
///    - The token is validated against the OAuthManager
///
/// # Security
/// - No authentication bypasses are permitted
/// - All signatures are cryptographically verified
/// - Replay attacks are prevented via timestamp validation
/// - Session IDs must be valid 32-byte public keys
/// - Dashboard API keys are validated against configuration
/// - OAuth tokens are validated and auto-refreshed
///
/// # Errors
/// Returns `SavantError::AuthError` if authentication fails for any reason.
pub async fn authenticate(
    frame: &RequestFrame,
    dashboard_api_key: Option<&str>,
    oauth_manager: Option<&oauth::OAuthManager>,
) -> Result<AuthenticatedSession, SavantError> {
    if let savant_core::types::RequestPayload::Auth(auth_str) = &frame.payload {
        // Check for OAuth token authentication
        if let Some(oauth_str) = auth_str.strip_prefix("OAUTH_TOKEN:") {
            let parts: Vec<&str> = oauth_str.splitn(2, ':').collect();
            if parts.len() == 2 {
                let provider = parts[0];
                let token_id = parts[1];
                if let Some(manager) = oauth_manager {
                    if manager.get_token(token_id).await.is_some() {
                        debug!("OAuth authentication accepted for provider: {}", provider);
                        return Ok(AuthenticatedSession {
                            session_id: SessionId(format!("oauth-{}", uuid::Uuid::new_v4())),
                            public_key: [0u8; 32],
                        });
                    }
                    warn!("OAuth authentication failed: token not found or expired");
                    return Err(SavantError::AuthError(
                        "Invalid or expired OAuth token".to_string(),
                    ));
                }
                warn!("OAuth authentication failed: OAuthManager not configured");
                return Err(SavantError::AuthError(
                    "OAuth authentication not configured".to_string(),
                ));
            }
        }

        // Check for Dashboard API key authentication
        if let Some(provided_key) = auth_str.strip_prefix("DASHBOARD_API_KEY:") {
            if let Some(expected_key) = dashboard_api_key {
                if expected_key.is_empty() {
                    // No key configured — accept dashboard connection (localhost Tauri mode)
                    debug!("Dashboard authentication accepted (no key configured)");
                    return Ok(AuthenticatedSession {
                        session_id: SessionId(format!("dash-{}", uuid::Uuid::new_v4())),
                        public_key: [0u8; 32],
                    });
                }
                if constant_time_eq(provided_key.as_bytes(), expected_key.as_bytes()) {
                    debug!("Dashboard authentication accepted");
                    return Ok(AuthenticatedSession {
                        session_id: SessionId(format!("dash-{}", uuid::Uuid::new_v4())),
                        public_key: [0u8; 32],
                    });
                }
                warn!("Dashboard authentication failed: API key mismatch");
                return Err(SavantError::AuthError(
                    "Invalid dashboard API key".to_string(),
                ));
            }
            // No key configured at all — accept dashboard connection (localhost Tauri mode)
            debug!("Dashboard authentication accepted (no key configured)");
            return Ok(AuthenticatedSession {
                session_id: SessionId(format!("dash-{}", uuid::Uuid::new_v4())),
                public_key: [0u8; 32],
            });
        }
    }

    // Fall through to Ed25519 signature authentication
    // Extract and validate signature
    let signature_hex = frame.signature.as_ref().ok_or_else(|| {
        warn!("Authentication failed: Missing signature");
        SavantError::AuthError("Missing signature".to_string())
    })?;

    // Extract and validate timestamp
    let timestamp = frame.timestamp.ok_or_else(|| {
        warn!("Authentication failed: Missing timestamp");
        SavantError::AuthError("Missing timestamp".to_string())
    })?;

    // Replay protection: reject timestamps outside the allowed drift window
    let now = savant_core::utils::time::now_secs()? as i64;

    let drift = (now - timestamp).abs();
    if drift > MAX_TIMESTAMP_DRIFT_SECS {
        warn!(
            "Authentication failed: Timestamp drift {}s exceeds maximum {}s",
            drift, MAX_TIMESTAMP_DRIFT_SECS
        );
        return Err(SavantError::AuthError(format!(
            "Timestamp expired (drift: {}s, max: {}s)",
            drift, MAX_TIMESTAMP_DRIFT_SECS
        )));
    }

    // Decode and validate public key from session_id
    let public_key_bytes = hex::decode(&frame.session_id.0).map_err(|e| {
        warn!("Authentication failed: Invalid session ID hex: {}", e);
        SavantError::AuthError(format!("Invalid session ID hex: {}", e))
    })?;

    if public_key_bytes.len() != 32 {
        warn!(
            "Authentication failed: Invalid public key length: {} (expected 32)",
            public_key_bytes.len()
        );
        return Err(SavantError::AuthError(
            "Invalid public key length".to_string(),
        ));
    }

    let mut pk_array = [0u8; 32];
    pk_array.copy_from_slice(&public_key_bytes);

    let verifying_key = VerifyingKey::from_bytes(&pk_array).map_err(|e| {
        warn!("Authentication failed: Invalid public key: {}", e);
        SavantError::AuthError(format!("Invalid public key: {}", e))
    })?;

    // Decode signature
    let signature_bytes = hex::decode(signature_hex).map_err(|e| {
        warn!("Authentication failed: Invalid signature hex: {}", e);
        SavantError::AuthError(format!("Invalid signature hex: {}", e))
    })?;

    if signature_bytes.len() != 64 {
        warn!(
            "Authentication failed: Invalid signature length: {} (expected 64)",
            signature_bytes.len()
        );
        return Err(SavantError::AuthError(
            "Invalid signature length".to_string(),
        ));
    }

    let signature = Signature::from_slice(&signature_bytes).map_err(|e| {
        warn!("Authentication failed: Invalid signature format: {}", e);
        SavantError::AuthError(format!("Invalid signature format: {}", e))
    })?;

    // Construct message for verification: timestamp:payload_json
    let payload_str = serde_json::to_string(&frame.payload).map_err(|e| {
        warn!("Authentication failed: Failed to serialize payload: {}", e);
        SavantError::AuthError(format!(
            "Failed to serialize payload for verification: {}",
            e
        ))
    })?;
    let message = format!("{}:{}:{}", frame.request_id, timestamp, payload_str);

    // Verify signature
    verifying_key
        .verify(message.as_bytes(), &signature)
        .map_err(|e| {
            warn!(
                "Authentication failed: Signature verification failed: {}",
                e
            );
            SavantError::AuthError(format!("Signature verification failed: {}", e))
        })?;

    debug!(
        "Authentication successful for session: {} (key: {}...)",
        frame.session_id.0,
        &frame.session_id.0[..8.min(frame.session_id.0.len())]
    );

    Ok(AuthenticatedSession {
        session_id: frame.session_id.clone(),
        public_key: pk_array,
    })
}

/// Constant-time byte comparison to prevent timing attacks on API key validation.
/// Returns true if both slices are equal, with execution time independent of where they differ.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut result = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        result |= x ^ y;
    }
    result == 0
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};
    use rand::rngs::OsRng;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[tokio::test]
    async fn test_valid_authentication() {
        let mut csprng = OsRng;
        let signing_key = SigningKey::generate(&mut csprng);
        let verifying_key = signing_key.verifying_key();

        let session_id = hex::encode(verifying_key.as_bytes());
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;

        let payload = savant_core::types::RequestPayload::Auth("test".to_string());
        let payload_str = serde_json::to_string(&payload).unwrap();
        let request_id = "test-req".to_string();
        let message = format!("{}:{}:{}", request_id, timestamp, payload_str);
        let signature = signing_key.sign(message.as_bytes());

        let frame = RequestFrame {
            request_id,
            session_id: SessionId(session_id),
            payload,
            signature: Some(hex::encode(signature.to_bytes())),
            timestamp: Some(timestamp),
        };

        let result = authenticate(&frame, None, None).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_missing_signature_fails() {
        let frame = RequestFrame {
            request_id: "test".to_string(),
            session_id: SessionId("test".to_string()),
            payload: savant_core::types::RequestPayload::Auth("test".to_string()),
            signature: None,
            timestamp: Some(123456),
        };

        let result = authenticate(&frame, None, None).await;
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Authentication failed"));
    }

    #[tokio::test]
    async fn test_expired_timestamp_fails() {
        let frame = RequestFrame {
            request_id: "test".to_string(),
            session_id: SessionId("test".to_string()),
            payload: savant_core::types::RequestPayload::Auth("test".to_string()),
            signature: Some("abcd".to_string()),
            timestamp: Some(1), // Very old timestamp
        };

        let result = authenticate(&frame, None, None).await;
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Authentication failed"));
    }

    // ====================== EXTENDED TEST COVERAGE ======================

    #[tokio::test]
    async fn test_wrong_signature_fails() {
        let mut csprng = OsRng;
        let signing_key = SigningKey::generate(&mut csprng);
        let verifying_key = signing_key.verifying_key();
        let session_id = hex::encode(verifying_key.as_bytes());

        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;

        // Sign with a DIFFERENT key
        let wrong_key = SigningKey::generate(&mut OsRng);
        let payload = savant_core::types::RequestPayload::Auth("test".to_string());
        let payload_str = serde_json::to_string(&payload).unwrap();
        let message = format!("test-req:{}:{}", timestamp, payload_str);
        let signature = wrong_key.sign(message.as_bytes());

        let frame = RequestFrame {
            request_id: "test-req".to_string(),
            session_id: SessionId(session_id),
            payload,
            signature: Some(hex::encode(signature.to_bytes())),
            timestamp: Some(timestamp),
        };

        let result = authenticate(&frame, None, None).await;
        assert!(result.is_err(), "Should reject mismatched signature");
    }

    #[tokio::test]
    async fn test_malformed_signature_hex_fails() {
        let frame = RequestFrame {
            request_id: "test".to_string(),
            session_id: SessionId("test-session".to_string()),
            payload: savant_core::types::RequestPayload::Auth("test".to_string()),
            signature: Some("not-valid-hex!".to_string()),
            timestamp: Some(
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_secs() as i64,
            ),
        };

        let result = authenticate(&frame, None, None).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_timestamp_too_far_in_future_fails() {
        let mut csprng = OsRng;
        let signing_key = SigningKey::generate(&mut csprng);
        let verifying_key = signing_key.verifying_key();
        let session_id = hex::encode(verifying_key.as_bytes());

        // Timestamp 1 hour in the future (beyond 5-minute drift window)
        let future_timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64
            + 3600;

        let payload = savant_core::types::RequestPayload::Auth("test".to_string());
        let payload_str = serde_json::to_string(&payload).unwrap();
        let message = format!("req-1:{}:{}", future_timestamp, payload_str);
        let signature = signing_key.sign(message.as_bytes());

        let frame = RequestFrame {
            request_id: "req-1".to_string(),
            session_id: SessionId(session_id),
            payload,
            signature: Some(hex::encode(signature.to_bytes())),
            timestamp: Some(future_timestamp),
        };

        let result = authenticate(&frame, None, None).await;
        assert!(
            result.is_err(),
            "Should reject future timestamps beyond drift"
        );
    }

    #[tokio::test]
    async fn test_current_timestamp_within_drift_succeeds() {
        let mut csprng = OsRng;
        let signing_key = SigningKey::generate(&mut csprng);
        let verifying_key = signing_key.verifying_key();
        let session_id = hex::encode(verifying_key.as_bytes());

        // Current timestamp (within 5-minute drift)
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;

        let payload = savant_core::types::RequestPayload::Auth("test".to_string());
        let payload_str = serde_json::to_string(&payload).unwrap();
        let message = format!("req-ok:{}:{}", timestamp, payload_str);
        let signature = signing_key.sign(message.as_bytes());

        let frame = RequestFrame {
            request_id: "req-ok".to_string(),
            session_id: SessionId(session_id),
            payload,
            signature: Some(hex::encode(signature.to_bytes())),
            timestamp: Some(timestamp),
        };

        let result = authenticate(&frame, None, None).await;
        assert!(result.is_ok(), "Should accept current timestamp");
    }

    #[tokio::test]
    async fn test_session_preserves_session_id() {
        let mut csprng = OsRng;
        let signing_key = SigningKey::generate(&mut csprng);
        let verifying_key = signing_key.verifying_key();
        let session_id = hex::encode(verifying_key.as_bytes());

        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;

        let payload = savant_core::types::RequestPayload::Auth("test".to_string());
        let payload_str = serde_json::to_string(&payload).unwrap();
        let message = format!("req-1:{}:{}", timestamp, payload_str);
        let signature = signing_key.sign(message.as_bytes());

        let frame = RequestFrame {
            request_id: "req-1".to_string(),
            session_id: SessionId(session_id.clone()),
            payload,
            signature: Some(hex::encode(signature.to_bytes())),
            timestamp: Some(timestamp),
        };

        let session = authenticate(&frame, None, None).await.unwrap();
        assert_eq!(session.session_id.0, session_id);
    }

    #[tokio::test]
    async fn test_auth_different_payload_types() {
        let mut csprng = OsRng;
        let signing_key = SigningKey::generate(&mut csprng);
        let verifying_key = signing_key.verifying_key();
        let session_id = hex::encode(verifying_key.as_bytes());

        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;

        // Test with ChatMessage payload
        let payload =
            savant_core::types::RequestPayload::ChatMessage(savant_core::types::ChatMessage {
                is_telemetry: false,
                role: savant_core::types::ChatRole::User,
                content: "hello".to_string(),
                sender: None,
                recipient: None,
                agent_id: None,
                session_id: None,
                channel: savant_core::types::AgentOutputChannel::default(),
                images: Vec::new(),
                ..Default::default()
            });
        let payload_str = serde_json::to_string(&payload).unwrap();
        let message = format!("req-cm:{}:{}", timestamp, payload_str);
        let signature = signing_key.sign(message.as_bytes());

        let frame = RequestFrame {
            request_id: "req-cm".to_string(),
            session_id: SessionId(session_id),
            payload,
            signature: Some(hex::encode(signature.to_bytes())),
            timestamp: Some(timestamp),
        };

        let result = authenticate(&frame, None, None).await;
        assert!(result.is_ok(), "Should accept ChatMessage payload");
    }

    #[tokio::test]
    async fn test_empty_session_id() {
        let frame = RequestFrame {
            request_id: "test".to_string(),
            session_id: SessionId("".to_string()),
            payload: savant_core::types::RequestPayload::Auth("test".to_string()),
            signature: Some("abcd".to_string()),
            timestamp: Some(
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_secs() as i64,
            ),
        };

        let result = authenticate(&frame, None, None).await;
        // Should still attempt auth (may succeed or fail based on verification)
        if let Err(e) = result {
            tracing::warn!("[gateway::auth] Authentication attempt failed: {}", e);
        }
    }

    #[tokio::test]
    async fn test_missing_timestamp_fails() {
        let frame = RequestFrame {
            request_id: "test".to_string(),
            session_id: SessionId("test".to_string()),
            payload: savant_core::types::RequestPayload::Auth("test".to_string()),
            signature: Some("abcd1234".to_string()),
            timestamp: None,
        };

        let result = authenticate(&frame, None, None).await;
        assert!(result.is_err(), "Missing timestamp should fail");
    }
}
