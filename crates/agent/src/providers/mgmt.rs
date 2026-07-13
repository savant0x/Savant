use savant_core::error::SavantError;
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
struct KeyCreateRequest {
    name: String,
}

pub struct OpenRouterMgmt {
    master_key: String,
}

impl OpenRouterMgmt {
    pub fn new(master_key: String) -> Self {
        Self { master_key }
    }

    pub async fn create_key(&self, agent_name: &str) -> Result<String, SavantError> {
        // Use extended timeout for key creation (API can be slow)
        let client = savant_core::net::secure_client_with_timeout(30, 10)
            .map_err(|e| SavantError::Unknown(format!("HTTP client error: {}", e)))?;
        let name = format!("Savant Agent: {}", agent_name);

        let response = client
            .post("https://openrouter.ai/api/v1/keys")
            .header("Authorization", format!("Bearer {}", self.master_key))
            .json(&KeyCreateRequest { name })
            .send()
            .await
            .map_err(|e| SavantError::Unknown(format!("OpenRouter Keygen Error: {}", e)))?;

        if !response.status().is_success() {
            let err_text = response.text().await.unwrap_or_default();
            return Err(SavantError::Unknown(format!(
                "OpenRouter Keygen Failed: {}",
                err_text
            )));
        }

        // Try to get raw response text for debugging
        let status = response.status();
        let raw_body = response.text().await.unwrap_or_default();
        tracing::debug!(
            "OpenRouter key creation: status={}, body_len={}",
            status,
            raw_body.len()
        );

        // Parse JSON flexibly: try top-level "key" first, then nested "data.key"
        let json: serde_json::Value = serde_json::from_str(&raw_body).map_err(|e| {
            SavantError::Unknown(format!(
                "OpenRouter JSON Parse Error: {}. Body: {}",
                e, raw_body
            ))
        })?;

        // Try top-level key
        if let Some(key) = json.get("key").and_then(|v| v.as_str()) {
            return Ok(key.to_string());
        }

        // Try nested data.key
        if let Some(key) = json
            .get("data")
            .and_then(|d| d.get("key"))
            .and_then(|v| v.as_str())
        {
            return Ok(key.to_string());
        }

        // Try array format or other variations
        if let Some(key) = json
            .get("data")
            .and_then(|d| d.get("keys"))
            .and_then(|arr| arr.as_array())
            .and_then(|arr| arr.first())
            .and_then(|v| v.get("key"))
            .and_then(|v| v.as_str())
        {
            return Ok(key.to_string());
        }

        Err(SavantError::Unknown(format!(
            "No key found in OpenRouter response. JSON: {}",
            json
        )))
    }
}
