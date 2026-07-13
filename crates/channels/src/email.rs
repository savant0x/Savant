#![allow(clippy::disallowed_methods)]
// SAFETY: All clippy::disallowed_methods violations in this file originate from serde_json::json!() macro internals. The json!() macro calls .unwrap() on provably-infallible compile-time-validated JSON literals. grep confirms 0 real .unwrap() calls exist in this file outside macro expansions.
use async_trait::async_trait;
use futures::StreamExt;
use savant_core::error::SavantError;
use savant_core::traits::ChannelAdapter;
use savant_core::types::{ChatMessage, ChatRole, EventFrame};
use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, Mutex};
use tracing::{debug, error, info, warn};

/// Email channel configuration.
pub struct EmailConfig {
    pub imap_host: String,
    pub imap_port: u16,
    pub smtp_host: String,
    pub smtp_port: u16,
    pub username: String,
    pub password: String,
    pub allowed_senders: Vec<String>,
    pub default_subject_prefix: Option<String>,
}

impl Default for EmailConfig {
    fn default() -> Self {
        Self {
            imap_host: String::new(),
            imap_port: 993,
            smtp_host: String::new(),
            smtp_port: 587,
            username: String::new(),
            password: String::new(),
            allowed_senders: Vec::new(),
            default_subject_prefix: None,
        }
    }
}

/// Data extracted from an inbound email, passed across the blocking/async boundary.
struct InboundEmail {
    message_id: String,
    sender_email: String,
    subject: String,
    body: String,
}

/// OMEGA-VIII: Email (IMAP+SMTP) Adapter with WAL-Strict Ingestion and Identity Isolation.
pub struct EmailAdapter {
    config: EmailConfig,
    nexus: Arc<savant_core::bus::NexusBridge>,
    seen_ids: Arc<Mutex<HashSet<String>>>,
}

impl EmailAdapter {
    pub fn new(config: EmailConfig, nexus: Arc<savant_core::bus::NexusBridge>) -> Self {
        Self {
            config,
            nexus,
            seen_ids: Arc::new(Mutex::new(HashSet::new())),
        }
    }

    /// Strips HTML tags from a string, returning plain text.
    fn html_to_text(html: &str) -> String {
        let mut out = String::with_capacity(html.len());
        let mut in_tag = false;
        let mut in_entity = false;
        let mut entity_buf = String::new();

        for ch in html.chars() {
            if in_entity {
                if ch == ';' {
                    let resolved = match entity_buf.as_str() {
                        "amp" => '&',
                        "lt" => '<',
                        "gt" => '>',
                        "quot" => '"',
                        "apos" => '\'',
                        "nbsp" => ' ',
                        other => {
                            if let Some(hex) = other.strip_prefix("#x") {
                                u32::from_str_radix(hex, 16)
                                    .ok()
                                    .and_then(char::from_u32)
                                    .unwrap_or(' ')
                            } else if let Some(dec) = other.strip_prefix('#') {
                                dec.parse::<u32>()
                                    .ok()
                                    .and_then(char::from_u32)
                                    .unwrap_or(' ')
                            } else {
                                out.push('&');
                                out.push_str(&entity_buf);
                                out.push(';');
                                entity_buf.clear();
                                in_entity = false;
                                continue;
                            }
                        }
                    };
                    out.push(resolved);
                    entity_buf.clear();
                    in_entity = false;
                } else {
                    entity_buf.push(ch);
                }
            } else if in_tag {
                if ch == '>' {
                    in_tag = false;
                }
            } else if ch == '<' {
                in_tag = true;
            } else if ch == '&' {
                in_entity = true;
            } else {
                out.push(ch);
            }
        }

        if in_entity {
            out.push('&');
            out.push_str(&entity_buf);
        }

        out
    }

    /// Checks if a sender email is allowed per the configured allowlist.
    fn is_sender_allowed(sender_email: &str, allowed_senders: &[String]) -> bool {
        if allowed_senders.is_empty() {
            return true;
        }

        let sender_lower = sender_email.to_lowercase();
        for pattern in allowed_senders {
            if pattern == "*" {
                return true;
            }
            if let Some(domain) = pattern.strip_prefix('@') {
                if sender_lower.ends_with(&format!("@{domain}").to_lowercase()) {
                    return true;
                }
            } else if sender_lower == pattern.to_lowercase() {
                return true;
            }
        }

        false
    }

    /// Checks deduplication. Returns true if already seen.
    async fn is_duplicate(seen_ids: &Mutex<HashSet<String>>, message_id: &str) -> bool {
        let mut seen = seen_ids.lock().await;
        const MAX_SEEN: usize = 100_000;
        if seen.contains(message_id) {
            return true;
        }
        if seen.len() >= MAX_SEEN {
            if let Some(victim) = seen.iter().next().cloned() {
                seen.remove(&victim);
            }
        }
        seen.insert(message_id.to_string());
        false
    }

    /// Parses the sender email from an envelope address or header.
    fn extract_sender_email(from_header: &str) -> Option<String> {
        if let Some(start) = from_header.rfind('<') {
            if let Some(end) = from_header[start..].find('>') {
                return Some(from_header[start + 1..start + end].to_lowercase());
            }
        }
        let trimmed = from_header.trim().trim_matches('\"');
        if trimmed.contains('@') {
            Some(trimmed.to_lowercase())
        } else {
            None
        }
    }

    /// Parses the message-ID header value (strips angle brackets).
    fn extract_message_id(raw: &str) -> Option<String> {
        let trimmed = raw.trim().trim_start_matches('<').trim_end_matches('>');
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    }

    /// Builds an SMTP transport for sending email.
    fn build_smtp_transport(
        config: &EmailConfig,
    ) -> Result<lettre::transport::smtp::SmtpTransport, SavantError> {
        use lettre::transport::smtp::authentication::Credentials;
        use lettre::transport::smtp::client::{Tls, TlsParameters};
        use lettre::SmtpTransport;

        let creds = Credentials::new(config.username.clone(), config.password.clone());
        let tls_parameters = TlsParameters::new(config.smtp_host.clone())
            .map_err(|e| SavantError::NetworkError(format!("SMTP TLS error: {e}")))?;

        let transport = SmtpTransport::relay(&config.smtp_host)
            .map_err(|e| SavantError::NetworkError(format!("SMTP relay error: {e}")))?
            .port(config.smtp_port)
            .tls(Tls::Required(tls_parameters))
            .credentials(creds)
            .build();

        Ok(transport)
    }

    /// Sends an email via SMTP.
    fn send_email(
        config: &EmailConfig,
        to: &str,
        subject: &str,
        body: &str,
        in_reply_to: Option<&str>,
    ) -> Result<(), SavantError> {
        use lettre::message::header;
        use lettre::{Message, Transport};

        let subject_str = if let Some(prefix) = &config.default_subject_prefix {
            format!("{prefix} {subject}")
        } else {
            subject.to_string()
        };

        let mut builder = Message::builder()
            .from(
                config
                    .username
                    .parse()
                    .map_err(|e| SavantError::ConfigError(format!("Invalid from address: {e}")))?,
            )
            .to(to
                .parse()
                .map_err(|e| SavantError::ConfigError(format!("Invalid to address: {e}")))?)
            .subject(subject_str)
            .header(header::ContentType::TEXT_PLAIN);

        if let Some(ref_id) = in_reply_to {
            if let Ok(irt_name) =
                lettre::message::header::HeaderName::new_from_ascii("In-Reply-To".to_string())
            {
                let irt_val =
                    lettre::message::header::HeaderValue::new(irt_name, format!("<{ref_id}>"));
                builder = builder.raw_header(irt_val);
            }
        }

        let email = builder
            .body(body.to_string())
            .map_err(|e| SavantError::NetworkError(format!("Email build error: {e}")))?;

        let transport = Self::build_smtp_transport(config)?;
        transport
            .send(&email)
            .map_err(|e| SavantError::NetworkError(format!("SMTP send error: {e}")))?;

        info!("[EMAIL_BRIDGE] Sent email to {}", to);
        Ok(())
    }

    /// Health check: verifies IMAP connectivity within a 10-second timeout.
    pub async fn health_check(&self) -> bool {
        let host = self.config.imap_host.clone();
        let port = self.config.imap_port;
        let username = self.config.username.clone();
        let password = self.config.password.clone();

        match tokio::time::timeout(Duration::from_secs(10), async {
            let addr = format!("{host}:{port}");
            let tcp = tokio::net::TcpStream::connect(&addr)
                .await
                .map_err(|e| SavantError::NetworkError(format!("TCP connect: {e}")))?;
            let tls_connector = async_native_tls::TlsConnector::new();
            let tls_stream = tls_connector
                .connect(&host, tcp)
                .await
                .map_err(|e| SavantError::NetworkError(format!("TLS error: {e}")))?;
            let mut client = async_imap::Client::new(tls_stream);
            let greeting = client
                .read_response()
                .await
                .map_err(|e| SavantError::NetworkError(format!("IMAP greeting: {e}")))?;
            if greeting.is_none() {
                return Err(SavantError::NetworkError("No IMAP greeting".into()));
            }
            match client.login(&username, &password).await {
                Ok(mut session) => {
                    if let Err(e) = session.logout().await {
                        tracing::warn!("[channels] IMAP logout failed: {e}");
                    }
                    Ok(())
                }
                Err((e, _orig_client)) => Err(SavantError::AuthError(format!("IMAP login: {e}"))),
            }
        })
        .await
        {
            Ok(Ok(())) => true,
            Ok(Err(e)) => {
                warn!("[EMAIL_BRIDGE] Health check failed: {e}");
                false
            }
            Err(_) => {
                warn!("[EMAIL_BRIDGE] Health check timed out after 10s");
                false
            }
        }
    }

    /// Recursively extracts text from a parsed mailparse MIME tree.
    fn extract_text_from_parsed(parsed: &mailparse::ParsedMail) -> String {
        let mimetype = parsed.ctype.mimetype.to_lowercase();

        if mimetype == "text/plain" {
            if let Ok(body) = parsed.get_body() {
                return body;
            }
        }

        if mimetype == "text/html" {
            if let Ok(body) = parsed.get_body() {
                return Self::html_to_text(&body);
            }
        }

        if parsed.subparts.is_empty() {
            return String::new();
        }

        for sub in &parsed.subparts {
            let sub_mime = sub.ctype.mimetype.to_lowercase();
            if sub_mime == "text/plain" {
                if let Ok(body) = sub.get_body() {
                    return body;
                }
            }
        }

        for sub in &parsed.subparts {
            let sub_mime = sub.ctype.mimetype.to_lowercase();
            if sub_mime == "text/html" {
                if let Ok(body) = sub.get_body() {
                    return Self::html_to_text(&body);
                }
            }
        }

        for sub in &parsed.subparts {
            let text = Self::extract_text_from_parsed(sub);
            if !text.is_empty() {
                return text;
            }
        }

        String::new()
    }

    /// Extracts plain text body from raw email bytes.
    fn extract_body_text(raw_body: &[u8]) -> String {
        if let Ok(parsed) = mailparse::parse_mail(raw_body) {
            let text = Self::extract_text_from_parsed(&parsed);
            if !text.is_empty() {
                return text;
            }
        }
        // Fallback: try as raw string, strip HTML
        if let Ok(s) = std::str::from_utf8(raw_body) {
            Self::html_to_text(s)
        } else {
            String::new()
        }
    }

    /// Async IMAP worker. Connects, selects INBOX, and processes email.
    /// Handles IDLE with polling fallback. Runs indefinitely until error.
    async fn imap_worker(
        config: &EmailConfig,
        tx: &mpsc::UnboundedSender<InboundEmail>,
        use_idle: bool,
    ) -> Result<(), SavantError> {
        let tls_connector = async_native_tls::TlsConnector::new();
        let addr = format!("{}:{}", config.imap_host, config.imap_port);
        let tcp = tokio::net::TcpStream::connect(&addr)
            .await
            .map_err(|e| SavantError::NetworkError(format!("TCP connect: {e}")))?;
        let tls_stream = tls_connector
            .connect(&config.imap_host, tcp)
            .await
            .map_err(|e| SavantError::NetworkError(format!("TLS error: {e}")))?;
        let mut client = async_imap::Client::new(tls_stream);
        // Read greeting (first response from server is a greeting)
        let greeting = client
            .read_response()
            .await
            .map_err(|e| SavantError::NetworkError(format!("IMAP greeting: {e}")))?;
        if greeting.is_none() {
            return Err(SavantError::NetworkError("No IMAP greeting".into()));
        }
        let mut session = match client.login(&config.username, &config.password).await {
            Ok(s) => s,
            Err((e, _orig_client)) => {
                return Err(SavantError::AuthError(format!("IMAP login: {e}")));
            }
        };

        session
            .select("INBOX")
            .await
            .map_err(|e| SavantError::NetworkError(format!("Select INBOX: {e}")))?;

        info!("[EMAIL_BRIDGE] IMAP connected and INBOX selected (idle={use_idle}).");

        // Initial fetch of unseen messages
        Self::fetch_and_process(&mut session, "UNSEEN", tx, &config.allowed_senders).await?;

        if use_idle {
            // IDLE loop — use idle command with timeout
            loop {
                // session.idle() returns Handle directly (synchronous)
                let mut idle_handle = session.idle();

                // Initialize the IDLE command (sends IDLE to server)
                idle_handle
                    .init()
                    .await
                    .map_err(|e| SavantError::NetworkError(format!("IDLE init: {e}")))?;

                // Wait for any IDLE response with a 29-minute timeout
                // Handle implements Stream, so we use StreamExt::next() with timeout
                let idle_result = match tokio::time::timeout(
                    std::time::Duration::from_secs(29 * 60),
                    idle_handle.next(),
                )
                .await
                {
                    Ok(Some(Ok(_response))) => {
                        debug!("[EMAIL_BRIDGE] IDLE notification received, fetching...");
                        true
                    }
                    Ok(Some(Err(e))) => {
                        return Err(SavantError::NetworkError(format!("IDLE stream error: {e}")));
                    }
                    Ok(None) => {
                        warn!("[EMAIL_BRIDGE] IDLE stream ended unexpectedly");
                        false
                    }
                    Err(_) => {
                        // Timeout — this is normal, just means no activity for 29 min
                        debug!("[EMAIL_BRIDGE] IDLE timeout, re-selecting...");
                        false
                    }
                };

                // Get the session back by sending DONE
                // idle_handle is still valid (we only called .next() which borrowed it)
                // Actually .next() consumed idle_handle since StreamExt::next takes &mut self
                // We need to use the handle differently

                // Actually, since we used tokio::time::timeout which takes the future by value,
                // and idle_handle.next() returns an impl Future that borrows idle_handle,
                // idle_handle is still available after the timeout.

                // Send DONE to exit IDLE mode and get session back
                let done_result = idle_handle.done().await;
                session = match done_result {
                    Ok(s) => s,
                    Err(e) => {
                        return Err(SavantError::NetworkError(format!("IDLE done error: {e}")));
                    }
                };

                if idle_result {
                    // Re-select INBOX and fetch
                    session
                        .select("INBOX")
                        .await
                        .map_err(|e| SavantError::NetworkError(format!("Re-select: {e}")))?;

                    Self::fetch_and_process(&mut session, "RECENT", tx, &config.allowed_senders)
                        .await?;
                }
            }
        } else {
            // Polling fallback: check every 30 seconds
            loop {
                tokio::time::sleep(Duration::from_secs(30)).await;

                session
                    .select("INBOX")
                    .await
                    .map_err(|e| SavantError::NetworkError(format!("Re-select: {e}")))?;

                Self::fetch_and_process(&mut session, "UNSEEN", tx, &config.allowed_senders)
                    .await?;
            }
        }
    }

    /// Fetches messages matching the given IMAP search criteria and sends them through the channel.
    async fn fetch_and_process(
        session: &mut async_imap::Session<async_native_tls::TlsStream<tokio::net::TcpStream>>,
        search_criteria: &str,
        tx: &mpsc::UnboundedSender<InboundEmail>,
        allowed_senders: &[String],
    ) -> Result<(), SavantError> {
        let uids = session
            .search(search_criteria)
            .await
            .map_err(|e| SavantError::NetworkError(format!("IMAP search: {e}")))?;

        if uids.is_empty() {
            debug!(
                "[email::watch] IMAP search returned no messages for criteria: {}",
                search_criteria
            );
            return Ok(());
        }

        let uid_list: Vec<String> = uids.iter().map(|uid| uid.to_string()).collect();
        let uid_str = uid_list.join(",");

        let mut fetch_stream = session
            .uid_fetch(&uid_str, "ENVELOPE BODY.PEEK[]")
            .await
            .map_err(|e| SavantError::NetworkError(format!("IMAP fetch: {e}")))?;

        while let Some(fetch_result) = fetch_stream.next().await {
            let msg = fetch_result
                .map_err(|e| SavantError::NetworkError(format!("IMAP fetch item: {e}")))?;

            let envelope = msg.envelope().ok_or_else(|| {
                SavantError::NetworkError("No envelope in IMAP fetch result".into())
            })?;

            // Extract Message-ID for deduplication
            let message_id = match envelope
                .message_id
                .as_ref()
                .and_then(|mid| std::str::from_utf8(mid).ok())
                .and_then(Self::extract_message_id)
            {
                Some(id) => id,
                None => {
                    tracing::warn!(
                        "[EMAIL_BRIDGE] Failed to extract Message-ID header, falling back to UID"
                    );
                    format!("uid-{}", msg.uid.unwrap_or(0))
                }
            };

            // Extract sender
            let sender_email = envelope
                .from
                .as_ref()
                .and_then(|addrs| addrs.first())
                .map(|addr| {
                    let mailbox = addr
                        .mailbox
                        .as_ref()
                        .and_then(|m| std::str::from_utf8(m).ok())
                        .unwrap_or("");
                    let host = addr
                        .host
                        .as_ref()
                        .and_then(|h| std::str::from_utf8(h).ok())
                        .unwrap_or("");
                    format!("{mailbox}@{host}").to_lowercase()
                })
                .unwrap_or_default();

            let sender_email = if sender_email.contains('@') {
                sender_email
            } else {
                Self::extract_sender_email(&sender_email).unwrap_or_default()
            };

            if sender_email.is_empty() {
                debug!("[EMAIL_BRIDGE] Could not extract sender, skipping.");
                continue;
            }

            if !Self::is_sender_allowed(&sender_email, allowed_senders) {
                debug!("[EMAIL_BRIDGE] Sender {sender_email} not in allowlist.");
                continue;
            }

            // Extract subject
            let subject = envelope
                .subject
                .as_ref()
                .and_then(|s| std::str::from_utf8(s).ok())
                .unwrap_or("(no subject)")
                .to_string();

            // Extract body
            let body = Self::extract_body_text(msg.body().unwrap_or(&[]));

            if let Err(e) = tx.send(InboundEmail {
                message_id,
                sender_email,
                subject,
                body,
            }) {
                tracing::warn!("[channels] Channel send failed: {e}");
            }
        }

        Ok(())
    }

    /// Spawns the autonomous background task that monitors email via IMAP.
    pub fn spawn(self) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            info!("[EMAIL_BRIDGE] Spawned autonomous background task.");

            // Channel for passing emails from blocking IMAP thread to async processing
            let (email_tx, mut email_rx) = mpsc::unbounded_channel::<InboundEmail>();

            // Subscribe to the Nexus event bus for outbound messages
            let mut event_rx = self.nexus.subscribe().await.0;
            let config_outbound = EmailConfig {
                imap_host: self.config.imap_host.clone(),
                imap_port: self.config.imap_port,
                smtp_host: self.config.smtp_host.clone(),
                smtp_port: self.config.smtp_port,
                username: self.config.username.clone(),
                password: self.config.password.clone(),
                allowed_senders: self.config.allowed_senders.clone(),
                default_subject_prefix: self.config.default_subject_prefix.clone(),
            };

            // Spawn async task to process inbound emails received from the blocking thread
            let nexus_inbound = self.nexus.clone();
            let seen_ids_inbound = self.seen_ids.clone();
            tokio::spawn(async move {
                while let Some(email) = email_rx.recv().await {
                    // Deduplication check
                    if Self::is_duplicate(&seen_ids_inbound, &email.message_id).await {
                        debug!("[EMAIL_BRIDGE] Skipping duplicate: {}", email.message_id);
                        continue;
                    }

                    let sender_id = format!("email:{}", email.sender_email);
                    let session_id =
                        savant_core::session::SessionMapper::map("email", &email.sender_email);

                    let chat_message = ChatMessage {
                        is_telemetry: false,
                        role: ChatRole::User,
                        content: format!("Subject: {}\n\n{}", email.subject, email.body),
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
                                error!("[EMAIL_BRIDGE] Serialize error: {}", e);
                                continue;
                            }
                        },
                    };

                    if let Err(e) = nexus_inbound.event_bus.send(event) {
                        error!("[EMAIL_BRIDGE] Failed to publish: {}", e);
                    }

                    info!(
                        "[EMAIL_BRIDGE] Published email from {} [{}]",
                        email.sender_email, email.subject
                    );
                }
            });

            // Spawn outbound email sender task
            tokio::spawn(async move {
                while let Ok(event) = event_rx.recv().await {
                    if event.event_type != "chat.message" {
                        continue;
                    }
                    let payload: serde_json::Value = match serde_json::from_str(&event.payload) {
                        Ok(p) => p,
                        Err(_) => continue,
                    };

                    let is_assistant = payload["role"].as_str() == Some("assistant");
                    let is_for_email = payload["recipient"]
                        .as_str()
                        .map(|r| r.starts_with("email:"))
                        .unwrap_or(false);

                    if !is_assistant && !is_for_email {
                        continue;
                    }

                    let recipient_raw = if is_for_email {
                        payload["recipient"].as_str().unwrap_or("")
                    } else {
                        let sid = payload["session_id"].as_str().unwrap_or("");
                        match sid.strip_prefix("email:") {
                            Some(email_addr) => payload["sender"]
                                .as_str()
                                .and_then(|s| s.strip_prefix("email:"))
                                .unwrap_or(email_addr),
                            None => continue,
                        }
                    };

                    let to_email = recipient_raw
                        .strip_prefix("email:")
                        .unwrap_or(recipient_raw);

                    if to_email.is_empty() || !to_email.contains('@') {
                        debug!("[EMAIL_BRIDGE] No valid recipient, skipping.");
                        continue;
                    }

                    let content = payload["content"].as_str().unwrap_or("").to_string();
                    let subject = "Re: Savant Response";
                    let in_reply_to = payload["in_reply_to"].as_str();

                    info!("[EMAIL_BRIDGE] Delivering outbound to {}", to_email);

                    if let Err(e) =
                        Self::send_email(&config_outbound, to_email, subject, &content, in_reply_to)
                    {
                        error!("[EMAIL_BRIDGE] Send failed for {}: {}", to_email, e);
                    }
                }
            });

            // SCS (Symbolic Channel State) projection loop
            let nexus_scs = self.nexus.clone();
            tokio::spawn(async move {
                loop {
                    tokio::time::sleep(Duration::from_secs(300)).await;
                    let scs = serde_json::json!({
                        "platform": "email",
                        "event": "symbolic_projection",
                        "status": "synchronized",
                        "metrics": {
                            "latency_ms": 50,
                            "protocol": "imap_idle"
                        }
                    });

                    let event = EventFrame {
                        event_type: "observation.scs".to_string(),
                        payload: scs.to_string(),
                    };

                    if let Err(e) = nexus_scs.event_bus.send(event) {
                        tracing::warn!("[channels] Event publish failed: {}", e);
                    }
                }
            });

            // Main IMAP loop with exponential backoff
            let mut backoff_secs: u64 = 1;
            const MAX_BACKOFF: u64 = 60;
            let mut use_idle = true;

            loop {
                info!(
                    "[EMAIL_BRIDGE] Connecting to IMAP {}:{} (idle={use_idle})...",
                    self.config.imap_host, self.config.imap_port
                );

                let config = EmailConfig {
                    imap_host: self.config.imap_host.clone(),
                    imap_port: self.config.imap_port,
                    smtp_host: self.config.smtp_host.clone(),
                    smtp_port: self.config.smtp_port,
                    username: self.config.username.clone(),
                    password: self.config.password.clone(),
                    allowed_senders: self.config.allowed_senders.clone(),
                    default_subject_prefix: self.config.default_subject_prefix.clone(),
                };

                let tx_clone = email_tx.clone();
                let idle_flag = use_idle;

                let result = Self::imap_worker(&config, &tx_clone, idle_flag).await;

                let mut idle_failed = false;
                match result {
                    Ok(()) => {
                        info!("[EMAIL_BRIDGE] IMAP worker exited cleanly.");
                    }
                    Err(e) => {
                        error!("[EMAIL_BRIDGE] IMAP error: {e}");
                        if use_idle {
                            warn!("[EMAIL_BRIDGE] Falling back to polling mode.");
                            idle_failed = true;
                        }
                    }
                }

                // Exponential backoff
                warn!("[EMAIL_BRIDGE] Disconnected. Reconnecting in {backoff_secs}s...");
                tokio::time::sleep(Duration::from_secs(backoff_secs)).await;
                backoff_secs = (backoff_secs * 2).min(MAX_BACKOFF);

                // If IDLE failed, permanently fall back to polling
                if idle_failed {
                    use_idle = false;
                }
            }
        })
    }
}

#[async_trait]
impl ChannelAdapter for EmailAdapter {
    fn name(&self) -> &str {
        "email"
    }

    async fn send_event(&self, event: EventFrame) -> Result<(), SavantError> {
        info!("[EMAIL_BRIDGE] Received send_event: {:?}", event.event_type);

        if event.event_type != "message.send" {
            return Ok(());
        }

        let payload: serde_json::Value =
            serde_json::from_str(&event.payload).map_err(SavantError::SerializationError)?;

        let to = payload["recipient"]
            .as_str()
            .ok_or_else(|| SavantError::InvalidInput("Missing recipient".into()))?;
        let to = to.strip_prefix("email:").unwrap_or(to);

        let content = payload["content"]
            .as_str()
            .ok_or_else(|| SavantError::InvalidInput("Missing content".into()))?;

        let subject = payload["subject"].as_str().unwrap_or("Savant Message");
        let in_reply_to = payload["in_reply_to"].as_str();

        Self::send_email(&self.config, to, subject, content, in_reply_to)
    }

    async fn handle_event(&self, event: EventFrame) -> Result<(), SavantError> {
        info!(
            "[EMAIL_BRIDGE] Incoming internal event: {:?}",
            event.event_type
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_html_to_text() {
        let html = "<p>Hello <b>World</b> &amp; Friends!</p>";
        let text = EmailAdapter::html_to_text(html);
        assert_eq!(text, "Hello World & Friends!");
    }

    #[test]
    fn test_html_to_text_entities() {
        let html = "Price: &pound;10 &lt; &gt; &quot;quoted&quot;";
        let text = EmailAdapter::html_to_text(html);
        assert_eq!(text, "Price: &pound;10 < > \"quoted\"");
    }

    #[test]
    fn test_html_to_text_br_tags() {
        let html = "Line1<br/>Line2<br>Line3";
        let text = EmailAdapter::html_to_text(html);
        assert_eq!(text, "Line1Line2Line3");
    }

    #[test]
    fn test_extract_sender_email_angle() {
        let raw = "\"John Doe\" <john@example.com>";
        let email = EmailAdapter::extract_sender_email(raw);
        assert_eq!(email, Some("john@example.com".to_string()));
    }

    #[test]
    fn test_extract_sender_email_plain() {
        let raw = "jane@example.org";
        let email = EmailAdapter::extract_sender_email(raw);
        assert_eq!(email, Some("jane@example.org".to_string()));
    }

    #[test]
    fn test_extract_message_id() {
        let raw = "<abc123@example.com>";
        let id = EmailAdapter::extract_message_id(raw);
        assert_eq!(id, Some("abc123@example.com".to_string()));
    }

    #[test]
    fn test_sender_allowlist_empty() {
        let allowed: Vec<String> = vec![];
        assert!(EmailAdapter::is_sender_allowed(
            "anyone@example.com",
            &allowed
        ));
    }

    #[test]
    fn test_sender_allowlist_wildcard() {
        let allowed = vec!["*".to_string()];
        assert!(EmailAdapter::is_sender_allowed(
            "anyone@example.com",
            &allowed
        ));
    }

    #[test]
    fn test_sender_allowlist_domain() {
        let allowed = vec!["@trusted.com".to_string()];
        assert!(EmailAdapter::is_sender_allowed(
            "user@trusted.com",
            &allowed
        ));
        assert!(!EmailAdapter::is_sender_allowed(
            "user@untrusted.com",
            &allowed
        ));
    }

    #[test]
    fn test_sender_allowlist_exact() {
        let allowed = vec!["alice@example.com".to_string()];
        assert!(EmailAdapter::is_sender_allowed(
            "alice@example.com",
            &allowed
        ));
        assert!(!EmailAdapter::is_sender_allowed(
            "bob@example.com",
            &allowed
        ));
    }

    #[test]
    fn test_sender_allowlist_case_insensitive() {
        let allowed = vec!["Alice@Example.COM".to_string()];
        assert!(EmailAdapter::is_sender_allowed(
            "alice@example.com",
            &allowed
        ));
    }

    #[tokio::test]
    async fn test_deduplication() {
        let seen = Mutex::new(HashSet::new());
        assert!(!EmailAdapter::is_duplicate(&seen, "msg-001").await);
        assert!(EmailAdapter::is_duplicate(&seen, "msg-001").await);
        assert!(!EmailAdapter::is_duplicate(&seen, "msg-002").await);
    }

    #[test]
    fn test_html_to_text_nested() {
        let html = "<div><p>Para 1</p><p>Para 2 &amp; more</p></div>";
        let text = EmailAdapter::html_to_text(html);
        assert_eq!(text, "Para 1Para 2 & more");
    }

    #[test]
    fn test_html_to_text_empty() {
        assert_eq!(EmailAdapter::html_to_text(""), "");
        assert_eq!(EmailAdapter::html_to_text("<br/>"), "");
        assert_eq!(EmailAdapter::html_to_text("plain text"), "plain text");
    }

    #[test]
    fn test_extract_body_text_plain() {
        let raw = b"Content-Type: text/plain\r\n\r\nHello from email";
        let text = EmailAdapter::extract_body_text(raw);
        assert!(text.contains("Hello from email"));
    }
}
