use savant_core::types::{RequestFrame, ResponseFrame};
use std::sync::Arc;
use tokio::sync::{mpsc, Semaphore};
use tokio::time::{timeout, Duration};

/// A session lane for queuing tasks and messages per session.
pub struct SessionLane {
    pub tx: mpsc::Sender<RequestFrame>,
    pub response_tx: mpsc::Sender<ResponseFrame>,
}

impl SessionLane {
    /// Creates a new SessionLane with specified capacity and concurrency limits.
    #[must_use]
    pub fn new(
        capacity: usize,
        max_concurrent: usize,
    ) -> (
        Self,
        mpsc::Receiver<RequestFrame>,
        mpsc::Receiver<ResponseFrame>,
        Arc<Semaphore>,
    ) {
        let (tx, rx) = mpsc::channel(capacity);
        let (res_tx, res_rx) = mpsc::channel(capacity);
        (
            Self {
                tx,
                response_tx: res_tx,
            },
            rx,
            res_rx,
            Arc::new(Semaphore::new(max_concurrent)),
        )
    }

    /// Spawns a consumer task that processes messages from the lane.
    pub fn spawn_consumer(
        mut rx: mpsc::Receiver<RequestFrame>,
        response_tx: mpsc::Sender<ResponseFrame>,
        concurrency_limit: Arc<Semaphore>,
        nexus: Arc<savant_core::bus::NexusBridge>,
    ) {
        tokio::spawn(async move {
            while let Some(frame) = rx.recv().await {
                // 🏰 Lane Backpressure: 30s timeout on concurrency acquisition
                let _permit = match timeout(Duration::from_secs(30), concurrency_limit.acquire())
                    .await
                {
                    Ok(Ok(p)) => p,
                    _ => {
                        tracing::error!(
                            "Lane timeout: Failed to acquire concurrency permit for session {}",
                            frame.session_id.0
                        );
                        // A2: Send error response to client (FID-20260529)
                        let error_response = savant_core::types::ResponseFrame {
                            request_id: frame.request_id.clone(),
                            payload:
                                "Error: Server busy, message could not be processed. Please retry."
                                    .to_string(),
                        };
                        if let Err(e) = response_tx.send(error_response).await {
                            tracing::warn!("[gateway] Failed to send lane timeout error: {}", e);
                        }
                        continue;
                    }
                };

                tracing::debug!(
                    "[LANE:ACTUATOR] Processing frame for session: {}",
                    frame.session_id.0
                );

                // 1. Process Payloads
                let response_payload = match frame.payload {
                    savant_core::types::RequestPayload::Auth(ref s)
                        if s.starts_with("DIRECTIVE:") =>
                    {
                        let directive = s.trim_start_matches("DIRECTIVE:").trim().to_string();

                        // Validate directive content: reject control characters, enforce length limit
                        if directive.len() > 2048 {
                            tracing::warn!(
                                "Directive rejected: exceeds maximum length (2048 chars)"
                            );
                            "Directive rejected: too long (max 2048 chars).".to_string()
                        } else if directive
                            .chars()
                            .any(|c| c.is_control() && c != '\n' && c != '\r' && c != '\t')
                        {
                            tracing::warn!("Directive rejected: contains control characters");
                            "Directive rejected: invalid characters.".to_string()
                        } else if directive.is_empty() {
                            "Directive rejected: empty content.".to_string()
                        } else {
                            tracing::info!(
                                "Global directive received: {}",
                                &directive[..directive.len().min(100)]
                            );
                            nexus
                                .update_state("GLOBAL_DIRECTIVE".to_string(), directive)
                                .await;
                            "Global directive broadcasted to swarm.".to_string()
                        }
                    }
                    savant_core::types::RequestPayload::ChatMessage(ref m) => {
                        format!(
                            "Accepted message from {}",
                            m.sender.as_deref().unwrap_or("User")
                        )
                    }
                    savant_core::types::RequestPayload::ControlFrame(_) => {
                        "Control frame acknowledged.".to_string()
                    }
                    savant_core::types::RequestPayload::Auth(ref s) => {
                        format!("Auth string acknowledged: {}", s)
                    }
                };

                let response = ResponseFrame {
                    request_id: frame.request_id.clone(),
                    payload: response_payload,
                };
                if let Err(e) = response_tx.send(response).await {
                    tracing::warn!("[gateway] Failed to send lane response: {}", e);
                }
            }
        });
    }
}
