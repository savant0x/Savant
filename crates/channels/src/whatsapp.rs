#![allow(clippy::disallowed_methods)]
// SAFETY: All clippy::disallowed_methods violations in this file originate from serde_json::json!() macro internals. The json!() macro calls .unwrap() on provably-infallible compile-time-validated JSON literals. grep confirms 0 real .unwrap() calls exist in this file outside macro expansions.
use async_trait::async_trait;
use savant_core::error::SavantError;
use savant_core::traits::ChannelAdapter;
use savant_core::types::EventFrame;
use serde::{Deserialize, Serialize};
use std::process::Stdio;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{ChildStdin, Command};
use tokio::sync::{mpsc, Mutex};
use tracing::{debug, info, warn};

/// Config for WhatsApp Adapter
#[derive(Debug, Clone)]
pub struct WhatsAppConfig {
    /// Path to Node.js sidecar script
    pub script_path: String,
    /// Session data storage path
    pub session_path: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum WhatsAppMessage {
    Command {
        action: String,
        chat_id: String,
        text: String,
    },
    Event {
        event: String,
        chat_id: String,
        text: String,
        from: String,
    },
    Status {
        state: String,
    },
}

pub struct WhatsAppAdapter {
    config: WhatsAppConfig,
    sidecar_stdin: Arc<Mutex<Option<ChildStdin>>>,
    events_tx: mpsc::Sender<EventFrame>,
    /// Handle to the child process for cleanup
    child_process: Arc<Mutex<Option<tokio::process::Child>>>,
    /// Handle to the reader task for cleanup
    reader_task: Arc<Mutex<Option<tokio::task::JoinHandle<()>>>>,
}

impl WhatsAppAdapter {
    pub fn new(config: WhatsAppConfig, events_tx: mpsc::Sender<EventFrame>) -> Self {
        Self {
            config,
            sidecar_stdin: Arc::new(Mutex::new(None)),
            events_tx,
            child_process: Arc::new(Mutex::new(None)),
            reader_task: Arc::new(Mutex::new(None)),
        }
    }

    pub async fn start_sidecar(&self) -> Result<(), SavantError> {
        info!(
            "Starting WhatsApp sidecar: node {}",
            self.config.script_path
        );

        let mut child = Command::new("node")
            .arg(&self.config.script_path)
            .arg(&self.config.session_path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|e| {
                SavantError::Unknown(format!("Failed to spawn WhatsApp sidecar: {}", e))
            })?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| SavantError::Unknown("WhatsApp sidecar: stdin already taken".into()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| SavantError::Unknown("WhatsApp sidecar: stdout already taken".into()))?;
        let tx = self.events_tx.clone();

        {
            let mut lock = self.sidecar_stdin.lock().await;
            *lock = Some(stdin);
        }

        // Store child process handle for cleanup
        {
            let mut lock = self.child_process.lock().await;
            *lock = Some(child);
        }

        let reader_handle = tokio::spawn(async move {
            let mut reader = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = reader.next_line().await {
                if let Ok(msg) = serde_json::from_str::<WhatsAppMessage>(&line) {
                    match msg {
                        WhatsAppMessage::Event {
                            event,
                            chat_id,
                            text,
                            from,
                        } => {
                            debug!("WhatsApp event: {} from {}", event, from);
                            let frame = EventFrame {
                                event_type: format!("whatsapp.{}", event),
                                payload: serde_json::json!({
                                    "chat_id": chat_id,
                                    "text": text,
                                    "from": from
                                })
                                .to_string(),
                            };
                            if let Err(e) = tx.send(frame).await {
                                tracing::warn!("[channels] Channel send failed: {}", e);
                            }
                        }
                        WhatsAppMessage::Status { state } => {
                            info!("WhatsApp sidecar status: {}", state);
                        }
                        _ => {}
                    }
                }
            }
            info!("WhatsApp sidecar stdout closed");
        });

        // Store reader task handle for cleanup
        {
            let mut lock = self.reader_task.lock().await;
            *lock = Some(reader_handle);
        }

        Ok(())
    }
}

impl Drop for WhatsAppAdapter {
    fn drop(&mut self) {
        // Abort reader task if still running
        if let Ok(mut lock) = self.reader_task.try_lock() {
            if let Some(handle) = lock.take() {
                handle.abort();
            }
        }
        // Kill child process if still running
        if let Ok(mut lock) = self.child_process.try_lock() {
            if let Some(mut child) = lock.take() {
                if let Err(e) = child.start_kill() {
                    tracing::warn!("[channels] Process kill failed: {}", e);
                }
            }
        }
    }
}

#[async_trait]
impl ChannelAdapter for WhatsAppAdapter {
    fn name(&self) -> &str {
        "whatsapp"
    }

    async fn send_event(&self, event: EventFrame) -> Result<(), SavantError> {
        match event.event_type.as_str() {
            "message.send" => {
                if let Ok(payload) = serde_json::from_str::<serde_json::Value>(&event.payload) {
                    let chat_id = payload
                        .get("chat_id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    let text = payload.get("text").and_then(|v| v.as_str()).unwrap_or("");

                    let cmd = WhatsAppMessage::Command {
                        action: "send".into(),
                        chat_id: chat_id.into(),
                        text: text.into(),
                    };

                    let mut lock = self.sidecar_stdin.lock().await;
                    if let Some(ref mut stdin) = *lock {
                        let json =
                            serde_json::to_string(&cmd).unwrap_or_else(|_| "{}".to_string()) + "\n";
                        stdin.write_all(json.as_bytes()).await.map_err(|e| {
                            SavantError::Unknown(format!(
                                "Failed to write to WhatsApp sidecar: {}",
                                e
                            ))
                        })?;
                        stdin.flush().await.map_err(|e| {
                            SavantError::Unknown(format!("Failed to flush WhatsApp sidecar: {}", e))
                        })?;
                        debug!("Sent message to WhatsApp sidecar for {}", chat_id);
                    } else {
                        warn!("WhatsApp sidecar not running");
                    }
                }
            }
            _ => {
                warn!("WhatsApp: Unhandled event type: {}", event.event_type);
            }
        }
        Ok(())
    }

    async fn handle_event(&self, event: EventFrame) -> Result<(), SavantError> {
        self.send_event(event).await
    }
}
