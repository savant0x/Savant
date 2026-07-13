#![allow(clippy::disallowed_methods)]
// SAFETY: All clippy::disallowed_methods violations in this file originate from serde_json::json!() macro internals. The json!() macro calls .unwrap() on provably-infallible compile-time-validated JSON literals. grep confirms 0 real .unwrap() calls exist in this file outside macro expansions.
use async_trait::async_trait;
use savant_core::error::SavantError;
use savant_core::traits::ChannelAdapter;
use savant_core::types::EventFrame;
use std::sync::Arc;
use tracing::{info, warn};

/// Voice channel configuration.
#[derive(Debug, Clone)]
pub struct VoiceConfig {
    /// TTS provider: "edge-tts" (free Microsoft Edge TTS) or "openai" (OpenAI TTS API)
    pub tts_provider: String,
    /// STT provider: "whisper-cpp" (local) or "groq" (Groq Whisper API)
    pub stt_provider: String,
    /// TTS voice name (e.g. "en-US-AriaNeural" for edge-tts, "alloy" for OpenAI)
    pub voice: String,
    /// Optional Groq API key for STT
    pub groq_api_key: Option<String>,
}

/// Voice channel adapter.
/// Converts text to speech (TTS) and speech to text (STT).
/// Designed to integrate with other channels (e.g., Telegram voice messages).
pub struct VoiceAdapter {
    config: VoiceConfig,
    http: reqwest::Client,
    nexus: Arc<savant_core::bus::NexusBridge>,
}

impl VoiceAdapter {
    pub fn new(config: VoiceConfig, nexus: Arc<savant_core::bus::NexusBridge>) -> Self {
        Self {
            config,
            http: savant_core::net::secure_client(),
            nexus,
        }
    }

    /// Generates speech audio from text using edge-tts (via subprocess).
    async fn tts_edge(&self, text: &str, output_path: &str) -> Result<(), SavantError> {
        let output = tokio::process::Command::new("edge-tts")
            .args([
                "--voice",
                &self.config.voice,
                "--text",
                text,
                "--write-media",
                output_path,
            ])
            .output()
            .await
            .map_err(|e| SavantError::Unknown(format!("edge-tts failed: {}", e)))?;

        if !output.status.success() {
            return Err(SavantError::Unknown(format!(
                "edge-tts failed: {}",
                String::from_utf8_lossy(&output.stderr)
            )));
        }
        Ok(())
    }

    /// Generates speech audio from text using OpenAI TTS API.
    async fn tts_openai(&self, text: &str, output_path: &str) -> Result<(), SavantError> {
        let api_key = std::env::var("OPENAI_API_KEY").map_err(|_| {
            SavantError::ConfigError("OPENAI_API_KEY environment variable not set".to_string())
        })?;
        let resp = self
            .http
            .post("https://api.openai.com/v1/audio/speech")
            .bearer_auth(api_key)
            .json(&serde_json::json!({
                "model": "tts-1",
                "input": text,
                "voice": self.config.voice,
            }))
            .send()
            .await
            .map_err(|e| SavantError::Unknown(format!("OpenAI TTS failed: {}", e)))?;

        let bytes = resp
            .bytes()
            .await
            .map_err(|e| SavantError::Unknown(e.to_string()))?;
        tokio::fs::write(output_path, bytes)
            .await
            .map_err(|e| SavantError::Unknown(e.to_string()))?;
        Ok(())
    }

    /// Transcribes audio to text using Groq Whisper API.
    async fn stt_groq(&self, audio_path: &str) -> Result<String, SavantError> {
        let api_key = self
            .config
            .groq_api_key
            .as_ref()
            .ok_or_else(|| SavantError::Unknown("No Groq API key configured".into()))?;

        let audio_bytes = tokio::fs::read(audio_path)
            .await
            .map_err(|e| SavantError::Unknown(format!("Failed to read audio: {}", e)))?;

        let file_name = std::path::Path::new(audio_path)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("audio.wav")
            .to_string();

        let part = reqwest::multipart::Part::bytes(audio_bytes)
            .file_name(file_name)
            .mime_str("audio/wav")
            .map_err(|e| SavantError::Unknown(e.to_string()))?;

        let form = reqwest::multipart::Form::new()
            .part("file", part)
            .text("model", "whisper-large-v3");

        let resp: serde_json::Value = self
            .http
            .post("https://api.groq.com/openai/v1/audio/transcriptions")
            .bearer_auth(api_key)
            .multipart(form)
            .send()
            .await
            .map_err(|e| SavantError::Unknown(e.to_string()))?
            .json()
            .await
            .map_err(|e| SavantError::Unknown(e.to_string()))?;

        Ok(resp["text"].as_str().unwrap_or("").to_string())
    }

    pub fn spawn(self) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            info!(
                "[VOICE] Starting Voice adapter (TTS: {}, STT: {})",
                self.config.tts_provider, self.config.stt_provider
            );

            let (mut event_rx, _) = self.nexus.subscribe().await;

            while let Ok(event) = event_rx.recv().await {
                if event.event_type == "voice.tts" {
                    // Convert text to speech
                    if let Ok(p) = serde_json::from_str::<serde_json::Value>(&event.payload) {
                        let text = p["text"].as_str().unwrap_or("");
                        let output_path =
                            p["output_path"].as_str().unwrap_or("/tmp/savant_tts.mp3");

                        let result = match self.config.tts_provider.as_str() {
                            "edge-tts" => self.tts_edge(text, output_path).await,
                            "openai" => self.tts_openai(text, output_path).await,
                            _ => Err(SavantError::Unknown(format!(
                                "Unknown TTS provider: {}",
                                self.config.tts_provider
                            ))),
                        };

                        if let Err(e) = result {
                            warn!("[VOICE] TTS error: {}", e);
                        }
                    }
                } else if event.event_type == "voice.stt" {
                    // Convert speech to text
                    if let Ok(p) = serde_json::from_str::<serde_json::Value>(&event.payload) {
                        let audio_path = p["audio_path"].as_str().unwrap_or("");

                        let result = match self.config.stt_provider.as_str() {
                            "groq" => self.stt_groq(audio_path).await,
                            _ => Err(SavantError::Unknown(format!(
                                "Unknown STT provider: {}",
                                self.config.stt_provider
                            ))),
                        };

                        match result {
                            Ok(text) => {
                                let frame = EventFrame {
                                    event_type: "voice.transcription".into(),
                                    payload: serde_json::json!({"text": text}).to_string(),
                                };
                                if let Err(e) = self.nexus.event_bus.send(frame) {
                                    tracing::warn!("[channels] Event publish failed: {}", e);
                                }
                            }
                            Err(e) => warn!("[VOICE] STT error: {}", e),
                        }
                    }
                }
            }
        })
    }
}

#[async_trait]
impl ChannelAdapter for VoiceAdapter {
    fn name(&self) -> &str {
        "voice"
    }
    async fn send_event(&self, event: EventFrame) -> Result<(), SavantError> {
        // Route outbound voice events through the nexus event bus.
        // The spawned task subscribes and dispatches TTS/STT operations.
        if let Err(e) = self.nexus.event_bus.send(event) {
            warn!("[VOICE] Failed to send event to nexus: {}", e);
            return Err(SavantError::Unknown(format!(
                "Voice event send failed: {}",
                e
            )));
        }
        Ok(())
    }
    async fn handle_event(&self, event: EventFrame) -> Result<(), SavantError> {
        // Inbound voice events are dispatched through the nexus event bus
        // where the spawned task processes TTS/STT operations.
        if event.event_type.starts_with("voice.") {
            info!("[VOICE] Handling inbound event: {}", event.event_type);
            if let Err(e) = self.nexus.event_bus.send(event) {
                warn!("[VOICE] Failed to route inbound event: {}", e);
                return Err(SavantError::Unknown(format!(
                    "Voice event routing failed: {}",
                    e
                )));
            }
        } else {
            warn!(
                "[VOICE] Dropping non-voice event type: {}",
                event.event_type
            );
        }
        Ok(())
    }
}
