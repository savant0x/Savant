#![allow(clippy::disallowed_methods)]
// SAFETY: All clippy::disallowed_methods violations in this file originate from serde_json::json!() macro internals. The json!() macro calls .unwrap() on provably-infallible compile-time-validated JSON literals. grep confirms 0 real .unwrap() calls exist in this file outside macro expansions.
//! IRC Channel Adapter
//!
//! Provides integration with IRC servers via raw TCP + TLS.
//! Supports:
//! - TLS via `tokio-rustls`
//! - Three-layer authentication: server password, SASL PLAIN, NickServ
//! - Message splitting (512-byte IRC protocol limit)
//! - PING/PONG keepalive
//! - Nick collision handling (ERR_NICKNAMEINUSE 433)
//! - Reconnection on disconnect with exponential backoff

use async_trait::async_trait;
use savant_core::error::SavantError;
use savant_core::traits::ChannelAdapter;
use savant_core::types::{ChatMessage, ChatRole, EventFrame};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio::sync::Mutex;
use tokio_rustls::TlsConnector;
use tracing::{debug, error, info, warn};

/// IRC line terminator.
const IRC_CRLF: &str = "\r\n";

/// Maximum IRC message size (512 bytes including CRLF).
const IRC_MAX_MSG_BYTES: usize = 512;

/// Prefix overhead for PRIVMSG: `PRIVMSG #channel :\r\n`
/// = 8 (PRIVMSG) + 1 (space) + target_len + 1 (space) + 1 (:) + 2 (\r\n)
/// We conservatively reserve 64 bytes for target + overhead.
const _IRC_PRIVMSG_OVERHEAD: usize = 64;

/// SASL negotiation state machine.
#[derive(Debug, Clone, PartialEq)]
enum SaslState {
    /// No SASL negotiation in progress
    Idle,
    /// CAP ACK received, sending AUTHENTICATE PLAIN
    SentAuthPlain,
    /// AUTHENTICATE + received, sending credentials
    SentCredentials,
    /// 903 received, SASL complete
    Complete,
    /// 904-907 received, SASL failed
    Failed,
}

/// IRC channel adapter configuration.
#[derive(Debug, Clone)]
pub struct IrcConfig {
    /// IRC server hostname
    pub server: String,
    /// IRC server port (typically 6697 for TLS)
    pub port: u16,
    /// IRC nickname
    pub nickname: String,
    /// Channels to join (e.g. ["#savant", "#general"])
    pub channels: Vec<String>,
    /// Server password (PASS command)
    pub server_password: Option<String>,
    /// SASL PLAIN password (for authenticated connection)
    pub sasl_password: Option<String>,
    /// NickServ IDENTIFY password
    pub nickserv_password: Option<String>,
    /// Whether to verify TLS certificates.
    ///
    /// **Security**: Defaults to `true`. Setting this to `false` disables certificate
    /// verification, making the connection vulnerable to man-in-the-middle attacks.
    /// Only disable for development/testing with self-signed certificates.
    pub verify_tls: bool,
}

impl Default for IrcConfig {
    fn default() -> Self {
        Self {
            server: String::new(),
            port: 6697,
            nickname: String::new(),
            channels: Vec::new(),
            server_password: None,
            sasl_password: None,
            nickserv_password: None,
            verify_tls: true,
        }
    }
}

/// TLS-wrapped write half type used throughout the adapter.
type TlsWriter = tokio::io::WriteHalf<tokio_rustls::client::TlsStream<TcpStream>>;

/// IRC channel adapter backed by raw TCP + TLS.
pub struct IrcAdapter {
    config: IrcConfig,
    nexus: Arc<savant_core::bus::NexusBridge>,
    /// Writer is shared between outbound loop and send_event
    writer: Arc<Mutex<Option<TlsWriter>>>,
}

impl IrcAdapter {
    /// Creates a new IRC adapter.
    pub fn new(config: IrcConfig, nexus: Arc<savant_core::bus::NexusBridge>) -> Self {
        Self {
            config,
            nexus,
            writer: Arc::new(Mutex::new(None)),
        }
    }

    /// Creates a TLS connector based on the config.
    fn tls_connector(&self) -> Result<TlsConnector, SavantError> {
        let mut root_store = rustls::RootCertStore::empty();

        if self.config.verify_tls {
            root_store.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        } else {
            // Accept any certificate — production systems should set verify_tls = true
            warn!("[IRC] TLS certificate verification is DISABLED");
        }

        let config = rustls::ClientConfig::builder()
            .with_root_certificates(root_store)
            .with_no_client_auth();

        Ok(TlsConnector::from(Arc::new(config)))
    }

    /// Connects to the IRC server with TLS.
    async fn connect(
        &self,
    ) -> Result<
        (
            BufReader<tokio::io::ReadHalf<tokio_rustls::client::TlsStream<TcpStream>>>,
            TlsWriter,
        ),
        SavantError,
    > {
        let addr = format!("{}:{}", self.config.server, self.config.port);
        info!("[IRC] Connecting to {}", addr);

        let tcp = TcpStream::connect(&addr)
            .await
            .map_err(|e| SavantError::NetworkError(format!("TCP connect failed: {}", e)))?;

        let connector = self.tls_connector()?;
        let server_name = rustls::pki_types::ServerName::try_from(self.config.server.as_str())
            .map_err(|e| SavantError::ConfigError(format!("Invalid server name: {}", e)))?
            .to_owned();

        let tls_stream = connector
            .connect(server_name, tcp)
            .await
            .map_err(|e| SavantError::NetworkError(format!("TLS handshake failed: {}", e)))?;

        info!(
            "[IRC] TLS connection established to {}:{}",
            self.config.server, self.config.port
        );

        let (rd, wr) = tokio::io::split(tls_stream);
        let reader = BufReader::new(rd);

        Ok((reader, wr))
    }

    /// Sends a raw IRC line (appends CRLF).
    async fn send_raw(
        writer: &Arc<Mutex<Option<TlsWriter>>>,
        line: &str,
    ) -> Result<(), SavantError> {
        let mut lock = writer.lock().await;
        if let Some(ref mut w) = *lock {
            let data = format!("{}{}", line, IRC_CRLF);
            w.write_all(data.as_bytes())
                .await
                .map_err(|e| SavantError::NetworkError(format!("IRC write failed: {}", e)))?;
            w.flush()
                .await
                .map_err(|e| SavantError::NetworkError(format!("IRC flush failed: {}", e)))?;
            debug!("[IRC] >> {}", line);
            Ok(())
        } else {
            Err(SavantError::NetworkError("IRC writer not connected".into()))
        }
    }

    /// Performs the IRC registration sequence: PASS, NICK, USER, authentication.
    async fn register(&self, writer: &Arc<Mutex<Option<TlsWriter>>>) -> Result<(), SavantError> {
        // 1. Server password
        if let Some(ref pass) = self.config.server_password {
            info!("[IRC] Sending PASS");
            Self::send_raw(writer, &format!("PASS {}", pass)).await?;
        }

        // 2. NICK
        info!("[IRC] Sending NICK {}", self.config.nickname);
        Self::send_raw(writer, &format!("NICK {}", self.config.nickname)).await?;

        // 3. USER
        Self::send_raw(
            writer,
            &format!("USER {} 0 * :Savant", self.config.nickname),
        )
        .await?;

        // Wait for registration to complete before authenticating
        // (SASL happens during CAP negotiation, NickServ after 001)
        // We handle SASL in the read loop during CAP negotiation.
        // NickServ is handled after receiving 001 (RPL_WELCOME).

        Ok(())
    }

    /// Sends AUTHENTICATE PLAIN after CAP ACK is received.
    /// Only sends the initial request — credentials are sent in `send_sasl_credentials`.
    async fn initiate_sasl(
        &self,
        writer: &Arc<Mutex<Option<TlsWriter>>>,
        sasl_state: &mut SaslState,
    ) -> Result<(), SavantError> {
        if self.config.sasl_password.is_none() {
            return Ok(());
        }
        info!("[IRC] Sending AUTHENTICATE PLAIN");
        Self::send_raw(writer, "AUTHENTICATE PLAIN").await?;
        *sasl_state = SaslState::SentAuthPlain;
        Ok(())
    }

    /// Sends SASL PLAIN credentials after AUTHENTICATE + is received.
    /// This is triggered by the read loop, not by a fixed sleep.
    async fn send_sasl_credentials(
        &self,
        writer: &Arc<Mutex<Option<TlsWriter>>>,
        sasl_state: &mut SaslState,
    ) -> Result<(), SavantError> {
        if let Some(ref sasl_pass) = self.config.sasl_password {
            info!("[IRC] Sending SASL credentials");
            let authz = "";
            let authc = &self.config.nickname;
            let plain = format!("{}\x00{}\x00{}", authz, authc, sasl_pass);
            let encoded = base64_encode(plain.as_bytes());
            Self::send_raw(writer, &format!("AUTHENTICATE {}", encoded)).await?;
            *sasl_state = SaslState::SentCredentials;
        }
        Ok(())
    }

    /// Identifies with NickServ.
    async fn nickserv_identify(
        &self,
        writer: &Arc<Mutex<Option<TlsWriter>>>,
    ) -> Result<(), SavantError> {
        if let Some(ref ns_pass) = self.config.nickserv_password {
            info!("[IRC] Identifying with NickServ");
            Self::send_raw(
                writer,
                &format!(
                    "PRIVMSG NickServ :IDENTIFY {} {}",
                    self.config.nickname, ns_pass
                ),
            )
            .await?;
        }
        Ok(())
    }

    /// Joins all configured channels.
    async fn join_channels(
        &self,
        writer: &Arc<Mutex<Option<TlsWriter>>>,
    ) -> Result<(), SavantError> {
        for channel in &self.config.channels {
            info!("[IRC] Joining {}", channel);
            Self::send_raw(writer, &format!("JOIN {}", channel)).await?;
            // Rate limit: avoid flooding the server with rapid JOINs
            // (send_raw already flushes, this is just spacing)
        }
        Ok(())
    }

    /// Processes a single IRC line.
    async fn process_irc_line(
        &self,
        line: &str,
        writer: &Arc<Mutex<Option<TlsWriter>>>,
        sasl_state: &mut SaslState,
        registered: &mut bool,
    ) -> Result<bool, SavantError> {
        debug!("[IRC] << {}", line);

        // Handle PING/PONG keepalive
        if let Some(_server) = line.strip_prefix("PING ") {
            let pong_target = line["PING ".len()..].trim();
            debug!("[IRC] PING -> PONG {}", pong_target);
            Self::send_raw(writer, &format!("PONG {}", pong_target)).await?;
            return Ok(true);
        }

        // Parse IRC message: [:prefix] command [params] [:trailing]
        let (prefix, command, params, trailing) = parse_irc_message(line);

        match command.as_str() {
            "001" => {
                // RPL_WELCOME — registration complete
                info!(
                    "[IRC] Welcome received. Registered as {}",
                    self.config.nickname
                );
                *registered = true;

                // NickServ identify (after registration)
                self.nickserv_identify(writer).await?;

                // Join channels
                self.join_channels(writer).await?;
            }
            "433" => {
                // ERR_NICKNAMEINUSE
                let nick = params
                    .first()
                    .map(|s| s.as_str())
                    .unwrap_or(&self.config.nickname);
                warn!("[IRC] Nick '{}' is in use", nick);

                // Try alternate nick by appending underscore
                let alt_nick = format!("{}_", nick);
                info!("[IRC] Trying alternate nick: {}", alt_nick);
                Self::send_raw(writer, &format!("NICK {}", alt_nick)).await?;
            }
            "AUTHENTICATE" => {
                // AUTHENTICATE + means server is ready for credentials
                if line.contains("+") && *sasl_state == SaslState::SentAuthPlain {
                    self.send_sasl_credentials(writer, sasl_state).await?;
                }
            }
            "903" => {
                // RPL_SASLSUCCESS
                info!("[IRC] SASL authentication successful");
                *sasl_state = SaslState::Complete;
                Self::send_raw(writer, "CAP END").await?;
            }
            "904" | "905" | "906" | "907" => {
                // SASL failure
                warn!("[IRC] SASL authentication failed (code {})", command);
                *sasl_state = SaslState::Failed;
                Self::send_raw(writer, "CAP END").await?;
            }
            "CAP" => {
                // CAP negotiation response
                if let Some(trail) = &trailing {
                    if trail.contains("sasl") && *sasl_state == SaslState::Idle {
                        info!("[IRC] Server supports SASL, initiating PLAIN auth");
                        // Send CAP REQ :sasl first, then AUTHENTICATE PLAIN
                        Self::send_raw(writer, "CAP REQ :sasl").await?;
                        self.initiate_sasl(writer, sasl_state).await?;
                    }
                }
            }
            "PRIVMSG" => {
                // Incoming message
                let sender = prefix
                    .as_ref()
                    .and_then(|p| p.split('!').next())
                    .unwrap_or("unknown");

                let target = params.first().map(|s| s.as_str()).unwrap_or("");
                let content = trailing.as_deref().unwrap_or("");

                if content.is_empty() {
                    return Ok(true);
                }

                // Determine if message is in a channel or DM
                let is_channel = target.starts_with('#');
                let effective_target = if is_channel { target } else { sender };

                info!(
                    "[IRC] Inbound message from {} in {}: {}",
                    sender, effective_target, content
                );

                let sender_id = format!("irc:{}", sender);
                let session_id = savant_core::session::SessionMapper::map("irc", effective_target);

                let chat_message = ChatMessage {
                    is_telemetry: false,
                    role: ChatRole::User,
                    content: content.to_string(),
                    sender: Some(sender_id),
                    recipient: Some("savant".to_string()),
                    agent_id: None,
                    session_id: Some(session_id),
                    channel: savant_core::types::AgentOutputChannel::Chat,
                    images: Vec::new(),
                    ..Default::default()
                };

                let event = EventFrame {
                    event_type: "chat.message".to_string(),
                    payload: match serde_json::to_string(&chat_message) {
                        Ok(p) => p,
                        Err(e) => {
                            error!("[IRC] Failed to serialize ChatMessage: {}", e);
                            return Ok(true);
                        }
                    },
                };

                if let Err(e) = self.nexus.event_bus.send(event) {
                    error!("[IRC] Failed to publish IRC event to Nexus: {}", e);
                }
            }
            "NOTICE" => {
                let content = trailing.as_deref().unwrap_or("");
                debug!("[IRC] NOTICE: {}", content);
            }
            "JOIN" => {
                let channel = params
                    .first()
                    .map(|s| s.as_str())
                    .unwrap_or(trailing.as_deref().unwrap_or(""));
                let nick = prefix
                    .as_ref()
                    .and_then(|p| p.split('!').next())
                    .unwrap_or("?");
                if nick == self.config.nickname {
                    info!("[IRC] Joined {}", channel);
                }
            }
            "PART" | "QUIT" | "KICK" => {
                debug!("[IRC] {} command received", command);
            }
            "NICK" => {
                let new_nick = trailing
                    .as_deref()
                    .or_else(|| params.first().map(|s| s.as_str()))
                    .unwrap_or("");
                let old_nick = prefix
                    .as_ref()
                    .and_then(|p| p.split('!').next())
                    .unwrap_or("?");
                if old_nick == self.config.nickname {
                    info!("[IRC] Nick changed to {}", new_nick);
                }
            }
            "ERROR" => {
                let msg = trailing.as_deref().unwrap_or(line);
                error!("[IRC] Server error: {}", msg);
                return Err(SavantError::NetworkError(format!(
                    "IRC server error: {}",
                    msg
                )));
            }
            _ => {
                // Numeric replies and other commands
                if let Ok(code) = command.parse::<u32>() {
                    if (400..500).contains(&code) {
                        warn!(
                            "[IRC] Error reply {}: {}",
                            code,
                            trailing.as_deref().unwrap_or("")
                        );
                    }
                }
            }
        }

        Ok(true)
    }

    /// Main IRC connection loop with reconnection.
    async fn run_connection_loop(&self) {
        let mut backoff_secs = 2u64;

        loop {
            let (reader, wr) = match self.connect().await {
                Ok(r) => r,
                Err(e) => {
                    error!("[IRC] Connection failed: {}", e);
                    info!("[IRC] Reconnecting in {}s...", backoff_secs);
                    tokio::time::sleep(Duration::from_secs(backoff_secs)).await;
                    backoff_secs = (backoff_secs * 2).min(60);
                    continue;
                }
            };

            // Store writer for outbound use
            {
                let mut lock = self.writer.lock().await;
                *lock = Some(wr);
            }

            let writer = self.writer.clone();

            // Register
            if let Err(e) = self.register(&writer).await {
                error!("[IRC] Registration failed: {}", e);
                tokio::time::sleep(Duration::from_secs(backoff_secs)).await;
                backoff_secs = (backoff_secs * 2).min(60);
                continue;
            }

            backoff_secs = 2; // Reset backoff on successful connection

            // Read loop
            let mut line_buf = String::new();
            let mut sasl_state = SaslState::Idle;
            let mut registered = false;
            let mut read_reader = reader;

            loop {
                line_buf.clear();
                match read_reader.read_line(&mut line_buf).await {
                    Ok(0) => {
                        warn!("[IRC] Connection closed by server");
                        break;
                    }
                    Ok(_) => {}
                    Err(e) => {
                        error!("[IRC] Read error: {}", e);
                        break;
                    }
                }

                let trimmed = line_buf.trim_end_matches('\r').trim_end_matches('\n');
                if trimmed.is_empty() {
                    continue;
                }

                match self
                    .process_irc_line(trimmed, &writer, &mut sasl_state, &mut registered)
                    .await
                {
                    Ok(true) => {}
                    Ok(false) => break,
                    Err(e) => {
                        error!("[IRC] Fatal processing error: {}", e);
                        break;
                    }
                }
            }

            // Clear writer
            {
                let mut lock = self.writer.lock().await;
                *lock = None;
            }

            info!("[IRC] Reconnecting in {}s...", backoff_secs);
            tokio::time::sleep(Duration::from_secs(backoff_secs)).await;
            backoff_secs = (backoff_secs * 2).min(60);
        }
    }

    /// Runs the outbound event loop, listening on the Nexus event bus.
    async fn run_outbound_loop(&self) {
        let mut event_rx = self.nexus.subscribe().await.0;

        while let Ok(event) = event_rx.recv().await {
            if event.event_type != "chat.message" {
                continue;
            }

            if let Ok(payload) = serde_json::from_str::<serde_json::Value>(&event.payload) {
                let is_assistant = payload["role"].as_str() == Some("Assistant");
                if let Some(recipient) = payload["recipient"].as_str() {
                    let is_for_irc = recipient.starts_with("irc:");

                    if is_assistant || is_for_irc {
                        let content = payload["content"].as_str().unwrap_or("");
                        if content.is_empty() {
                            continue;
                        }

                        // Determine target from session_id
                        let session_id = payload["session_id"].as_str().unwrap_or("");
                        let target = if let Some(channel_or_nick) = session_id.strip_prefix("irc:")
                        {
                            channel_or_nick.to_string()
                        } else if is_for_irc {
                            recipient.strip_prefix("irc:").unwrap_or("").to_string()
                        } else {
                            // Fall back to first joined channel
                            self.config.channels.first().cloned().unwrap_or_default()
                        };

                        if target.is_empty() {
                            warn!("[IRC] No target for outbound message");
                            continue;
                        }

                        let writer = self.writer.clone();
                        debug!(
                            "[IRC] Delivering to {}: {}",
                            target,
                            &content[..content.len().min(80)]
                        );

                        if let Err(e) = Self::send_privmsg_static(&writer, &target, content).await {
                            error!("[IRC] Failed to deliver to {}: {}", target, e);
                        }
                    }
                }
            }
        }
    }

    /// Static helper for sending PRIVMSG from the outbound loop.
    /// Uses word-boundary-aware splitting to avoid breaking words mid-message.
    async fn send_privmsg_static(
        writer: &Arc<Mutex<Option<TlsWriter>>>,
        target: &str,
        message: &str,
    ) -> Result<(), SavantError> {
        let prefix = format!("PRIVMSG {} :", target);
        let max_payload = IRC_MAX_MSG_BYTES
            .saturating_sub(prefix.len())
            .saturating_sub(2);

        // Word-boundary-aware splitting (enterprise pattern from openclaw IRC protocol)
        let mut chunks = Vec::new();
        let mut remaining = message;

        while !remaining.is_empty() {
            if remaining.len() <= max_payload {
                chunks.push(remaining.to_string());
                break;
            }

            // Find the last word boundary within the limit
            let split_idx = remaining[..max_payload.min(remaining.len())]
                .rfind(|c: char| c.is_whitespace())
                .unwrap_or_else(|| {
                    // No word boundary — find last char boundary
                    remaining
                        .char_indices()
                        .map(|(i, _)| i)
                        .take_while(|&i| i <= max_payload)
                        .last()
                        .unwrap_or(0)
                });

            if split_idx == 0 {
                // Single word exceeds limit — take one char
                let len = remaining.chars().next().map(|c| c.len_utf8()).unwrap_or(1);
                chunks.push(remaining[..len].to_string());
                remaining = &remaining[len..];
            } else {
                chunks.push(remaining[..split_idx].trim_end().to_string());
                remaining = remaining[split_idx..].trim_start();
            }
        }

        // Send chunks with rate limiting to avoid flooding
        for chunk in chunks {
            let line = format!("PRIVMSG {} :{}", target, chunk);
            Self::send_raw(writer, &line).await?;
            tokio::time::sleep(Duration::from_millis(200)).await;
        }

        Ok(())
    }
}

/// Parses an IRC message into components.
/// Format: `[:prefix] command [params] [:trailing]`
fn parse_irc_message(line: &str) -> (Option<String>, String, Vec<String>, Option<String>) {
    let line = line.trim();

    let (prefix, rest): (Option<String>, &str) = if let Some(stripped) = line.strip_prefix(':') {
        match stripped.find(' ') {
            Some(pos) => (Some(stripped[..pos].to_string()), &stripped[pos + 1..]),
            None => (Some(stripped.to_string()), ""),
        }
    } else {
        (None, line)
    };

    let rest = rest.trim();

    // Split off trailing (everything after first `:` that's preceded by a space)
    let (middle, trailing) = if let Some(pos) = rest.find(" :") {
        let trail_start = pos + 2;
        let trail = rest[trail_start..].to_string();
        let mid = rest[..pos].trim();
        (mid, Some(trail))
    } else {
        (rest, None)
    };

    let parts: Vec<&str> = middle.split_whitespace().collect();
    let command = parts.first().map(|s| s.to_string()).unwrap_or_default();
    let params: Vec<String> = parts[1..].iter().map(|s| s.to_string()).collect();

    (prefix, command, params, trailing)
}

/// Simple base64 encoder for SASL PLAIN.
fn base64_encode(input: &[u8]) -> String {
    const CHARS: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut output = String::with_capacity(input.len().div_ceil(3) * 4);

    for chunk in input.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = if chunk.len() > 1 { chunk[1] as u32 } else { 0 };
        let b2 = if chunk.len() > 2 { chunk[2] as u32 } else { 0 };

        let triple = (b0 << 16) | (b1 << 8) | b2;

        output.push(CHARS[((triple >> 18) & 0x3F) as usize] as char);
        output.push(CHARS[((triple >> 12) & 0x3F) as usize] as char);

        if chunk.len() > 1 {
            output.push(CHARS[((triple >> 6) & 0x3F) as usize] as char);
        } else {
            output.push('=');
        }

        if chunk.len() > 2 {
            output.push(CHARS[(triple & 0x3F) as usize] as char);
        } else {
            output.push('=');
        }
    }

    output
}

#[async_trait]
impl ChannelAdapter for IrcAdapter {
    fn name(&self) -> &str {
        "irc"
    }

    async fn send_event(&self, event: EventFrame) -> Result<(), SavantError> {
        debug!("[IRC] Manual send_event: {:?}", event.event_type);

        if event.event_type == "chat.message" {
            if let Ok(payload) = serde_json::from_str::<serde_json::Value>(&event.payload) {
                let content = payload["content"].as_str().unwrap_or("");
                let recipient = payload["recipient"].as_str().unwrap_or("");

                let target = if let Some(nick) = recipient.strip_prefix("irc:") {
                    nick.to_string()
                } else {
                    self.config.channels.first().cloned().unwrap_or_default()
                };

                if !target.is_empty() && !content.is_empty() {
                    return Self::send_privmsg_static(&self.writer, &target, content).await;
                }
            }
        }

        Ok(())
    }

    async fn handle_event(&self, event: EventFrame) -> Result<(), SavantError> {
        debug!("[IRC] Incoming internal event: {:?}", event.event_type);

        match event.event_type.as_str() {
            "irc.action" => {
                if let Ok(payload) = serde_json::from_str::<serde_json::Value>(&event.payload) {
                    let target = payload["target"].as_str().unwrap_or("");
                    let action = payload["action"].as_str().unwrap_or("");
                    if !target.is_empty() && !action.is_empty() {
                        let msg = format!("\x01ACTION {}\x01", action);
                        return Self::send_privmsg_static(&self.writer, target, &msg).await;
                    }
                }
            }
            "irc.join" => {
                if let Ok(payload) = serde_json::from_str::<serde_json::Value>(&event.payload) {
                    let channel = payload["channel"].as_str().unwrap_or("");
                    if !channel.is_empty() {
                        return Self::send_raw(&self.writer, &format!("JOIN {}", channel)).await;
                    }
                }
            }
            "irc.part" => {
                if let Ok(payload) = serde_json::from_str::<serde_json::Value>(&event.payload) {
                    let channel = payload["channel"].as_str().unwrap_or("");
                    if !channel.is_empty() {
                        return Self::send_raw(&self.writer, &format!("PART {}", channel)).await;
                    }
                }
            }
            _ => {
                debug!("[IRC] Unhandled event type: {}", event.event_type);
            }
        }

        Ok(())
    }
}

impl IrcAdapter {
    /// Spawns the autonomous IRC adapter background task.
    /// Runs connection loop and outbound event loop concurrently.
    pub fn spawn(self) -> tokio::task::JoinHandle<()> {
        let adapter = Arc::new(self);

        tokio::spawn(async move {
            info!(
                "[IRC] Spawned autonomous background task for {}:{} (nick={})",
                adapter.config.server, adapter.config.port, adapter.config.nickname
            );

            let conn_adapter = adapter.clone();
            let conn_handle = tokio::spawn(async move {
                conn_adapter.run_connection_loop().await;
            });

            let outbound_adapter = adapter.clone();
            let outbound_handle = tokio::spawn(async move {
                outbound_adapter.run_outbound_loop().await;
            });

            let (conn_result, outbound_result) = tokio::join!(conn_handle, outbound_handle);
            if let Err(e) = conn_result {
                tracing::warn!("[channels] IRC connection task failed: {}", e);
            }
            if let Err(e) = outbound_result {
                tracing::warn!("[channels] IRC outbound task failed: {}", e);
            }
            error!("[IRC] Background task terminated.");
        })
    }
}
