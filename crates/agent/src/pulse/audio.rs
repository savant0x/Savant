//! Voice/audio event pipeline for heartbeat integration.
//!
//! Defines the `AudioPipeline` trait for voice integration (Whisper STT, TTS playback).
//! The `NoopAudioPipeline` provides a no-op implementation that returns clear errors
//! when transcription is attempted. Replace with a real implementation when Whisper/TTS
//! is available.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AudioEvent {
    VoiceReady,
    TranscriptReceived(String),
    PlaybackComplete,
}

pub trait AudioPipeline: Send + Sync {
    /// Start the audio pipeline (spawn background tasks, initialize hardware).
    fn start(&self);

    /// Subscribe to audio events. Returns a receiver that gets all future events.
    fn subscribe(&self) -> tokio::sync::broadcast::Receiver<AudioEvent>;

    /// Transcribe audio bytes to text. Returns an error if no STT backend is configured.
    fn transcribe(&self, _audio: &[u8]) -> Result<String, savant_core::error::SavantError> {
        Err(savant_core::error::SavantError::OperationFailed(
            "Audio pipeline not configured — install Whisper/TTS".into(),
        ))
    }
}

pub struct NoopAudioPipeline {
    event_tx: tokio::sync::broadcast::Sender<AudioEvent>,
}

impl Default for NoopAudioPipeline {
    fn default() -> Self {
        Self::new()
    }
}

impl NoopAudioPipeline {
    pub fn new() -> Self {
        let (event_tx, _) = tokio::sync::broadcast::channel(16);
        Self { event_tx }
    }
}

impl AudioPipeline for NoopAudioPipeline {
    fn start(&self) {
        let tx = self.event_tx.clone();
        tokio::spawn(async move {
            let _ = tx.send(AudioEvent::VoiceReady);
        });
    }

    fn subscribe(&self) -> tokio::sync::broadcast::Receiver<AudioEvent> {
        self.event_tx.subscribe()
    }
}
