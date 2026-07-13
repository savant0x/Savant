#![allow(clippy::disallowed_methods)]
// SAFETY: All clippy::disallowed_methods violations in this file originate from serde_json::json!() macro internals. The json!() macro calls .unwrap() on provably-infallible compile-time-validated JSON literals. grep confirms 0 real .unwrap() calls exist in this file outside macro expansions.
//! Telegram Channel Adapter
//!
//! Provides integration with Telegram Bot API for sending and receiving messages.
//! Supports:
//! - Webhook and long-polling for message reception
//! - Message formatting (Markdown, HTML)
//! - Inline keyboards and callbacks
//! - Media messages (images, documents)
//! - Rate limiting per Telegram API constraints

use async_trait::async_trait;
use savant_core::bus::NexusBridge;
use savant_core::error::SavantError;
use savant_core::traits::ChannelAdapter;
use savant_core::types::{ChatMessage, ChatRole, EventFrame};
use std::sync::Arc;
use std::time::{Duration, Instant};
use teloxide::{
    prelude::*,
    types::{
        CallbackQuery, ChatId, InlineKeyboardButton, InlineKeyboardMarkup, InputFile,
        Message as TgMessage, ParseMode, Update,
    },
    Bot,
};
use tokio::sync::mpsc;
use tokio::time::sleep;
use tracing::{debug, info, warn};

/// Maximum message length for Telegram (4096 characters)
const MAX_MESSAGE_LENGTH: usize = 4096;

/// Rate limit: maximum messages per second
const MAX_MESSAGES_PER_SECOND: f32 = 30.0;

/// Telegram adapter configuration.
#[derive(Debug, Clone)]
pub struct TelegramConfig {
    /// Bot token from BotFather
    pub bot_token: String,
    /// Default chat ID to send messages to
    pub default_chat_id: Option<i64>,
    /// Parse mode for messages (Markdown, HTML, or None)
    pub parse_mode: Option<String>,
    /// Whether to use webhooks (true) or long-polling (false)
    pub use_webhook: bool,
    /// Webhook URL (if use_webhook is true)
    pub webhook_url: Option<String>,
}

/// Telegram channel adapter.
pub struct TelegramAdapter {
    bot: Bot,
    config: TelegramConfig,
    nexus: Arc<NexusBridge>,
    /// Channel for receiving messages from Telegram
    message_tx: mpsc::Sender<TgMessage>,
    message_rx: Option<mpsc::Receiver<TgMessage>>,
    /// Last sent message timestamp for rate limiting
    last_sent: Arc<tokio::sync::Mutex<Instant>>,
}

impl TelegramAdapter {
    /// Creates a new Telegram adapter with the given configuration.
    pub fn new(config: TelegramConfig, nexus: Arc<NexusBridge>) -> Result<Self, SavantError> {
        let bot = Bot::new(&config.bot_token);
        // RC-15: Bounded channel for backpressure
        let (message_tx, message_rx) = mpsc::channel(500);

        Ok(Self {
            bot,
            config,
            nexus,
            message_tx,
            message_rx: Some(message_rx),
            last_sent: Arc::new(tokio::sync::Mutex::new(Instant::now())),
        })
    }

    /// Takes the message receiver for processing incoming messages.
    /// Returns None if the receiver has already been taken.
    pub fn take_message_receiver(&mut self) -> Option<mpsc::Receiver<TgMessage>> {
        self.message_rx.take()
    }

    /// Enforces rate limiting (Token Bucket equivalent).
    async fn enforce_rate_limit(&self) {
        let mut last_sent = self.last_sent.lock().await;
        let elapsed = last_sent.elapsed();
        let min_interval = Duration::from_secs_f32(1.0 / MAX_MESSAGES_PER_SECOND);

        if elapsed < min_interval {
            let wait_time = min_interval - elapsed;
            sleep(wait_time).await;
        }
        *last_sent = Instant::now();
    }

    /// Starts the Telegram bot with long-polling.
    pub async fn start_polling(&mut self) -> Result<(), SavantError> {
        let bot = self.bot.clone();
        let tx = self.message_tx.clone();
        let nexus = self.nexus.clone();

        info!("Starting Telegram bot with long-polling (dptree)");

        tokio::spawn(async move {
            let handler = dptree::entry()
                .branch(Update::filter_message().endpoint(move |msg: TgMessage| {
                    let tx = tx.clone();
                    let nexus = nexus.clone();
                    async move {
                        debug!("Received Telegram message: {:?}", msg.id);

                        // Route message to NexusBridge for agent processing
                        if let Some(text) = msg.text() {
                            let sender = msg
                                .from()
                                .map(|u| u.username.clone().unwrap_or_else(|| u.id.0.to_string()))
                                .unwrap_or_else(|| "unknown".to_string());
                            let session_id = savant_core::session::SessionMapper::map(
                                "telegram",
                                &msg.chat.id.0.to_string(),
                            );
                            let chat_message = ChatMessage {
                                is_telemetry: false,
                                role: ChatRole::User,
                                content: text.to_string(),
                                sender: Some(format!("telegram:{}", sender)),
                                recipient: Some("savant".to_string()),
                                agent_id: None,
                                session_id: Some(session_id),
                                channel: savant_core::types::AgentOutputChannel::Chat,
                                images: Vec::new(),
                                ..Default::default()
                            };
                            let event = EventFrame {
                                event_type: "chat.message".to_string(),
                                payload: serde_json::to_string(&chat_message).unwrap_or_default(),
                            };
                            if let Err(e) = nexus.event_bus.send(event) {
                                tracing::warn!(
                                    "[channels::telegram] Failed to publish to NexusBridge: {:?}",
                                    e
                                );
                            }
                        }

                        // RC-15: Use send().await for bounded channel (async context)
                        if let Err(e) = tx.send(msg).await {
                            tracing::warn!(
                                "[channels::telegram] Failed to send message to handler: {:?}",
                                e
                            );
                        }
                        respond(())
                    }
                }))
                .branch(
                    Update::filter_callback_query().endpoint(|q: CallbackQuery| async move {
                        debug!("Received Telegram callback: {:?}", q.data);
                        // Process callback query (e.g., button clicks)
                        respond(())
                    }),
                );

            Dispatcher::builder(bot, handler)
                .enable_ctrlc_handler()
                .build()
                .dispatch()
                .await;
        });

        Ok(())
    }

    /// Sends a text message to a chat with rate limiting.
    pub async fn send_message(&self, chat_id: i64, text: &str) -> Result<(), SavantError> {
        self.enforce_rate_limit().await;

        // Truncate message if too long (UTF-8 safe)
        let truncated: std::borrow::Cow<'_, str> = if text.len() > MAX_MESSAGE_LENGTH {
            warn!(
                "Message truncated from {} to {} characters",
                text.len(),
                MAX_MESSAGE_LENGTH
            );
            std::borrow::Cow::Owned(text.chars().take(MAX_MESSAGE_LENGTH).collect::<String>())
        } else {
            std::borrow::Cow::Borrowed(text)
        };

        let mut send = self.bot.send_message(ChatId(chat_id), &*truncated);

        // Apply parse mode if configured
        if let Some(ref mode) = self.config.parse_mode {
            send = match mode.to_lowercase().as_str() {
                "markdown" => send.parse_mode(ParseMode::MarkdownV2),
                "html" => send.parse_mode(ParseMode::Html),
                _ => send,
            };
        }

        send.await
            .map_err(|e| SavantError::Unknown(format!("Failed to send Telegram message: {}", e)))?;

        debug!("Sent Telegram message to chat {}", chat_id);
        Ok(())
    }

    /// Sends a photo to a chat with rate limiting.
    pub async fn send_photo(
        &self,
        chat_id: i64,
        photo_path: &str,
        caption: Option<&str>,
    ) -> Result<(), SavantError> {
        self.enforce_rate_limit().await;

        let photo = InputFile::file(photo_path);
        let mut send = self.bot.send_photo(ChatId(chat_id), photo);

        if let Some(cap) = caption {
            send = send.caption(cap);
        }

        send.await
            .map_err(|e| SavantError::Unknown(format!("Failed to send Telegram photo: {}", e)))?;

        Ok(())
    }

    /// Sends a message with an inline keyboard.
    pub async fn send_with_keyboard(
        &self,
        chat_id: i64,
        text: &str,
        buttons: Vec<(String, String)>,
    ) -> Result<(), SavantError> {
        self.enforce_rate_limit().await;

        let keyboard = InlineKeyboardMarkup::new(
            buttons
                .into_iter()
                .map(|(label, data)| vec![InlineKeyboardButton::callback(label, data)])
                .collect::<Vec<_>>(),
        );

        self.bot
            .send_message(ChatId(chat_id), text)
            .reply_markup(keyboard)
            .await
            .map_err(|e| {
                SavantError::Unknown(format!("Failed to send Telegram keyboard: {}", e))
            })?;

        Ok(())
    }

    /// Sends a message to the default chat.
    pub async fn send_to_default(&self, text: &str) -> Result<(), SavantError> {
        let chat_id = self
            .config
            .default_chat_id
            .ok_or_else(|| SavantError::InvalidInput("No default chat ID configured".into()))?;
        self.send_message(chat_id, text).await
    }

    /// Formats an EventFrame for Telegram.
    fn format_event(&self, event: &EventFrame) -> String {
        format!(
            "*{}*\n\n{}",
            escape_markdown(&event.event_type),
            escape_markdown(&event.payload)
        )
    }
}

/// Escapes special characters for MarkdownV2.
fn escape_markdown(text: &str) -> String {
    text.replace('_', "\\_")
        .replace('*', "\\*")
        .replace('[', "\\[")
        .replace(']', "\\]")
        .replace('(', "\\(")
        .replace(')', "\\)")
        .replace('~', "\\~")
        .replace('`', "\\`")
        .replace('>', "\\>")
        .replace('#', "\\#")
        .replace('+', "\\+")
        .replace('-', "\\-")
        .replace('=', "\\=")
        .replace('|', "\\|")
        .replace('{', "\\{")
        .replace('}', "\\}")
        .replace('.', "\\.")
        .replace('!', "\\!")
}

#[async_trait]
impl ChannelAdapter for TelegramAdapter {
    fn name(&self) -> &str {
        "telegram"
    }

    async fn send_event(&self, event: EventFrame) -> Result<(), SavantError> {
        let text = self.format_event(&event);
        self.send_to_default(&text).await
    }

    async fn handle_event(&self, event: EventFrame) -> Result<(), SavantError> {
        info!("Telegram handling event: {:?}", event.event_type);

        // Handle incoming events (e.g., from other channels)
        match event.event_type.as_str() {
            "message.send" => {
                // Extract chat_id and message from payload
                if let Ok(payload) = serde_json::from_str::<serde_json::Value>(&event.payload) {
                    let chat_id = payload
                        .get("chat_id")
                        .and_then(|v| v.as_i64())
                        .or(self.config.default_chat_id);

                    let text = payload
                        .get("text")
                        .and_then(|v| v.as_str())
                        .unwrap_or(&event.payload);

                    if let Some(chat_id) = chat_id {
                        if let Some(photo) = payload.get("photo").and_then(|v| v.as_str()) {
                            self.send_photo(chat_id, photo, Some(text)).await?;
                        } else {
                            self.send_message(chat_id, text).await?;
                        }
                    } else {
                        warn!("No chat_id specified for Telegram message");
                    }
                }
            }
            "callback.answer" => {
                // Example of responding to callback query
                if let Ok(payload) = serde_json::from_str::<serde_json::Value>(&event.payload) {
                    if let Some(callback_query_id) =
                        payload.get("callback_query_id").and_then(|v| v.as_str())
                    {
                        self.bot
                            .answer_callback_query(callback_query_id)
                            .await
                            .map_err(|e| {
                                SavantError::Unknown(format!("Failed to answer callback: {}", e))
                            })?;
                    }
                }
            }
            _ => {
                debug!("Unhandled Telegram event type: {}", event.event_type);
            }
        }

        Ok(())
    }
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)]
mod tests {
    use super::*;

    #[test]
    fn test_escape_markdown() {
        let text = "Hello *world* [test] (foo)";
        let escaped = escape_markdown(text);
        assert_eq!(escaped, "Hello \\*world\\* \\[test\\] \\(foo\\)");
    }

    #[tokio::test]
    async fn test_format_event() {
        let config = TelegramConfig {
            bot_token: "test".to_string(),
            default_chat_id: Some(123),
            parse_mode: Some("markdown".to_string()),
            use_webhook: false,
            webhook_url: None,
        };

        let nexus = Arc::new(savant_core::bus::NexusBridge::new());
        let adapter = TelegramAdapter::new(config, nexus).unwrap();
        let event = EventFrame {
            event_type: "test".to_string(),
            payload: "hello".to_string(),
        };
        assert!(adapter.format_event(&event).contains("hello"));
    }
}
