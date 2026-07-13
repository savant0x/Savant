use backoff::future::retry;
use backoff::ExponentialBackoff;
use reqwest::Client;
use savant_core::error::SavantError;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{debug, info};

/// Represents an OAuth token with refresh capabilities.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OAuthToken {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_at: Option<i64>,
    pub provider: String,
}

impl OAuthToken {
    /// Checks if the token is expired or will expire soon (within 60s).
    pub fn is_expired(&self) -> bool {
        if let Some(expiry) = self.expires_at {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs() as i64;
            return expiry <= now + 60; // 60s buffer
        }
        false
    }
}

/// The OAuth Manager handles token storage and autonomous rotation.
pub struct OAuthManager {
    tokens: Arc<RwLock<HashMap<String, OAuthToken>>>,
    client: Client,
}

impl Default for OAuthManager {
    fn default() -> Self {
        Self::new()
    }
}

impl OAuthManager {
    pub fn new() -> Self {
        Self {
            tokens: Arc::new(RwLock::new(HashMap::new())),
            client: Client::new(),
        }
    }

    /// Stores a new token for a specific provider/user.
    pub async fn store_token(&self, id: String, token: OAuthToken) {
        let mut lock = self.tokens.write().await;
        lock.insert(id, token);
    }

    /// Retrieves a valid token, triggering refresh if necessary.
    /// GTW-07: Releases write lock before network I/O to prevent deadlock.
    pub async fn get_token(&self, id: &str) -> Option<String> {
        // Check if refresh is needed (read-only, quick)
        let needs_refresh = {
            let lock = self.tokens.read().await;
            if let Some(token) = lock.get(id) {
                token.is_expired() && token.refresh_token.is_some()
            } else {
                return None;
            }
        };

        // Perform refresh without holding the lock
        if needs_refresh {
            tracing::info!("Refreshing OAuth token for {}", id);
            let token_snapshot = {
                let lock = self.tokens.read().await;
                lock.get(id).cloned()
            };
            if let Some(token) = token_snapshot {
                match self.perform_refresh(&token).await {
                    Ok(new_token) => {
                        let mut lock = self.tokens.write().await;
                        lock.insert(id.to_string(), new_token);
                    }
                    Err(_) => {
                        tracing::error!("Failed to refresh OAuth token for {}", id);
                        return None;
                    }
                }
            }
        }

        // Return the current token
        let lock = self.tokens.read().await;
        lock.get(id).map(|t| t.access_token.clone())
    }

    /// Internal logic for performing the refresh request.
    async fn perform_refresh(&self, token: &OAuthToken) -> Result<OAuthToken, SavantError> {
        let refresh_token = token
            .refresh_token
            .as_ref()
            .ok_or_else(|| SavantError::AuthError("No refresh token available".into()))?;

        info!(
            "OMEGA-III: Performing autonomous OAuth refresh for provider: {}",
            token.provider
        );

        // 🏰 AAA: Production-Grade Exponential Backoff
        let backoff = ExponentialBackoff::default();

        let operation = || {
            let client = self.client.clone();
            let provider = token.provider.clone();
            let refresh_val = refresh_token.clone();

            async move {
                debug!("Attempting OAuth refresh request to {}...", provider);

                // Logic for different providers
                let url = match provider.as_str() {
                    "google" => "https://oauth2.googleapis.com/token",
                    "github" => "https://github.com/login/oauth/access_token",
                    _ => {
                        return Err(backoff::Error::permanent(SavantError::AuthError(format!(
                            "Unsupported provider: {}",
                            provider
                        ))))
                    }
                };

                let res = client
                    .post(url)
                    .form(&[
                        ("grant_type", "refresh_token"),
                        ("refresh_token", &refresh_val),
                    ])
                    .send()
                    .await
                    .map_err(|e| {
                        backoff::Error::transient(SavantError::NetworkError(e.to_string()))
                    })?;

                if !res.status().is_success() {
                    let status = res.status();
                    return Err(backoff::Error::transient(SavantError::AuthError(format!(
                        "Provider returned {}",
                        status
                    ))));
                }

                let new_token_data: OAuthToken = res
                    .json()
                    .await
                    .map_err(|e| backoff::Error::Permanent(SavantError::Unknown(e.to_string())))?;

                Ok(new_token_data)
            }
        };

        retry(backoff, operation).await
    }
}
