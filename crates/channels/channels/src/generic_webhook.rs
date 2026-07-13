#![allow(clippy::disallowed_methods)]
// SAFETY: All clippy::disallowed_methods violations in this file originate from serde_json::json!() macro internals. The json!() macro calls .unwrap() on provably-infallible compile-time-validated JSON literals. grep confirms 0 real .unwrap() calls exist in this file outside macro expansions.
use async_trait::async_trait;
use savant_core::error::SavantError;
use savant_core::traits::ChannelAdapter;
use savant_core::types::EventFrame;
use std::sync::Arc;
use tracing::{debug, info};

#[derive(Debug, Clone)]
pub struct GenericWebhookConfig {
    pub listen_port: u16,
    pub inbound_path: String,
    pub outbound_url: Option<String>,
    pub auth_token: Option<String>,
}

#[derive(Clone)]
pub struct GenericWebhookAdapter {
    config: GenericWebhookConfig,
    http: reqwest::Client,
    nexus: Arc<savant_core::bus::NexusBridge>,
}

impl GenericWebhookAdapter {
    pub fn new(config: GenericWebhookConfig, nexus: Arc<savant_core::bus::NexusBridge>) -> Self {
        Self {
            config,
            http: savant_core::net::secure_client(),
            nexus,
        }
    }

    pub fn spawn(self) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            info!(
                "[WEBHOOK] Starting generic webhook adapter on port {}",
                self.config.listen_port
            );

            // Outbound: forward events to configured URL
            if let Some(ref url) = self.config.outbound_url {
                let (mut rx, _) = self.nexus.subscribe().await;
                let http = self.http.clone();
                let url = url.clone();
                let token = self.config.auth_token.clone();
                tokio::spawn(async move {
                    while let Ok(event) = rx.recv().await {
                        if event.event_type == "chat.message" {
                            let mut req = http.post(&url).json(&serde_json::json!({
                                "event_type": event.event_type,
                                "payload": serde_json::from_str::<serde_json::Value>(&event.payload).unwrap_or_default(),
                            }));
                            if let Some(ref t) = token {
                                req = req.bearer_auth(t);
                            }
                            if let Err(e) = req.send().await {
                                debug!("[WEBHOOK] Forward failed: {}", e);
                            }
                        }
                    }
                });
            }

            // Inbound: webhook server runs via gateway at /api/channels/webhook
            info!(
                "[WEBHOOK] Inbound webhook at /api/channels/webhook/{}",
                self.config.inbound_path
            );
            futures::future::pending::<()>().await;
        })
    }
}

#[async_trait]
impl ChannelAdapter for GenericWebhookAdapter {
    fn name(&self) -> &str {
        "webhook"
    }
    async fn send_event(&self, event: EventFrame) -> Result<(), SavantError> {
        if let Some(ref url) = self.config.outbound_url {
            let mut req = self.http.post(url).json(&event);
            if let Some(ref t) = self.config.auth_token {
                req = req.bearer_auth(t);
            }
            req.send()
                .await
                .map_err(|e| SavantError::Unknown(e.to_string()))?;
        }
        Ok(())
    }
    async fn handle_event(&self, event: EventFrame) -> Result<(), SavantError> {
        debug!("[WEBHOOK] Event: {}", event.event_type);
        Ok(())
    }
}
