// SAFETY: All clippy::disallowed_methods violations in this file originate from serde_json::json!() macro internals. The json!() macro calls .unwrap() on provably-infallible compile-time-validated JSON literals. grep confirms 0 real .unwrap() calls exist in this file outside macro expansions.
#![allow(clippy::disallowed_methods)]
// SAFETY: All `clippy::disallowed_methods` violations in this file originate from
// the `serde_json::json!()` macro, which internally uses `.unwrap()` on
// compile-time-validated JSON literals. A malformed JSON literal would be a
// compile error, making the panic path statically unreachable.

use crate::server::GatewayState;
use axum::{extract::State, http::StatusCode, Json};
use ed25519_dalek::{Signature, Signer, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Pairing Request from a Companion Node
#[derive(Debug, Deserialize)]
pub struct PairingRequest {
    pub device_name: String,
    pub device_type: String, // "macos", "ios", "android"
    /// Hex-encoded Ed25519 public key of the requesting device
    pub public_key: String,
    /// Hex-encoded Ed25519 signature over the device_name + device_type + timestamp
    pub signature: String,
    /// UNIX timestamp of the request (for replay protection)
    pub timestamp: u64,
}

/// Pairing Response from the Gateway
#[derive(Debug, Serialize)]
pub struct PairingResponse {
    /// Hex-encoded session token (derived from shared secret)
    pub session_token: String,
    /// Hex-encoded gateway Ed25519 public key
    pub gateway_public_key: String,
    /// UNIX timestamp of token expiration
    pub expires_at: u64,
}

/// Pairing Error Response
#[derive(Debug, Serialize)]
pub struct PairingErrorResponse {
    pub error: String,
    pub code: String,
}

/// Maximum allowed timestamp drift for pairing requests (5 minutes)
const MAX_TIMESTAMP_DRIFT_SECS: i64 = 300;

/// Session token TTL (24 hours)
const SESSION_TOKEN_TTL_SECS: u64 = 86400;

/// Handles node pairing requests through a secure Ed25519 handshake.
///
/// # Security
/// - Validates device public key format
/// - Verifies Ed25519 signature over request payload
/// - Checks timestamp for replay protection
/// - Generates ephemeral session token
/// - Stores device public key for future authentication
pub async fn pairing_handler(
    State(state): State<Arc<GatewayState>>,
    Json(payload): Json<PairingRequest>,
) -> Result<Json<PairingResponse>, (StatusCode, Json<PairingErrorResponse>)> {
    tracing::info!(
        "Node pairing initiated from {} ({})",
        payload.device_name,
        payload.device_type
    );

    // 1. Validate timestamp (replay protection)
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;

    let drift = (now - payload.timestamp as i64).abs();
    if drift > MAX_TIMESTAMP_DRIFT_SECS {
        tracing::warn!(
            "Pairing rejected: timestamp drift {}s exceeds maximum {}s",
            drift,
            MAX_TIMESTAMP_DRIFT_SECS
        );
        return Err((
            StatusCode::BAD_REQUEST,
            Json(PairingErrorResponse {
                error: "Timestamp expired or too far in the future".to_string(),
                code: "TIMESTAMP_DRIFT".to_string(),
            }),
        ));
    }

    // 2. Parse and validate device public key
    let device_pubkey_bytes = match hex::decode(&payload.public_key) {
        Ok(bytes) if bytes.len() == 32 => {
            let mut arr = [0u8; 32];
            arr.copy_from_slice(&bytes);
            arr
        }
        _ => {
            tracing::warn!("Pairing rejected: invalid public key format");
            return Err((
                StatusCode::BAD_REQUEST,
                Json(PairingErrorResponse {
                    error: "Invalid public key format (must be 32 bytes hex-encoded)".to_string(),
                    code: "INVALID_PUBLIC_KEY".to_string(),
                }),
            ));
        }
    };

    let device_verifying_key = match VerifyingKey::from_bytes(&device_pubkey_bytes) {
        Ok(key) => key,
        Err(e) => {
            tracing::warn!("Pairing rejected: invalid Ed25519 public key: {}", e);
            return Err((
                StatusCode::BAD_REQUEST,
                Json(PairingErrorResponse {
                    error: format!("Invalid Ed25519 public key: {}", e),
                    code: "INVALID_ED25519_KEY".to_string(),
                }),
            ));
        }
    };

    // 3. Parse and verify signature
    let signature_bytes = match hex::decode(&payload.signature) {
        Ok(bytes) if bytes.len() == 64 => {
            let mut arr = [0u8; 64];
            arr.copy_from_slice(&bytes);
            arr
        }
        _ => {
            tracing::warn!("Pairing rejected: invalid signature format");
            return Err((
                StatusCode::BAD_REQUEST,
                Json(PairingErrorResponse {
                    error: "Invalid signature format (must be 64 bytes hex-encoded)".to_string(),
                    code: "INVALID_SIGNATURE".to_string(),
                }),
            ));
        }
    };

    let signature = Signature::from_bytes(&signature_bytes);

    // 4. Construct signed message: device_name + device_type + timestamp
    let signed_message = format!(
        "{}:{}:{}",
        payload.device_name, payload.device_type, payload.timestamp
    );

    // 5. Verify signature
    if device_verifying_key
        .verify(signed_message.as_bytes(), &signature)
        .is_err()
    {
        tracing::warn!("Pairing rejected: signature verification failed");
        return Err((
            StatusCode::UNAUTHORIZED,
            Json(PairingErrorResponse {
                error: "Signature verification failed".to_string(),
                code: "SIGNATURE_INVALID".to_string(),
            }),
        ));
    }

    // 6. Use the gateway's persistent signing key (not deterministic)
    let gateway_signing_key = &state.gateway_signing_key;
    let gateway_verifying_key = gateway_signing_key.verifying_key();

    // 7. Generate session token (signed by gateway)
    let expires_at = now as u64 + SESSION_TOKEN_TTL_SECS;
    let token_payload = format!(
        "{}:{}:{}",
        hex::encode(device_pubkey_bytes),
        payload.timestamp,
        expires_at
    );
    let token_signature = gateway_signing_key.sign(token_payload.as_bytes());

    let session_token = format!("{}:{}", hex::encode(token_signature.to_bytes()), expires_at);

    // 8. Store device public key in shared memory for future authentication
    let device_key = format!("paired_device:{}", hex::encode(device_pubkey_bytes));
    let device_info = serde_json::json!({
        "device_name": payload.device_name,
        "device_type": payload.device_type,
        "public_key": payload.public_key,
        "paired_at": now,
        "expires_at": expires_at,
    });
    state
        .nexus
        .update_state(device_key, device_info.to_string())
        .await;

    tracing::info!(
        "Pairing successful for {} ({})",
        payload.device_name,
        payload.device_type
    );

    Ok(Json(PairingResponse {
        session_token,
        gateway_public_key: hex::encode(gateway_verifying_key.as_bytes()),
        expires_at,
    }))
}
