use super::NetError;
use crate::secure::credential_vault::CredentialVault;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, ServerName};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use zeroize::Zeroize;

/// TLS proxy configuration.
#[derive(Debug, Clone)]
pub struct TlsProxyConfig {
    /// The listen address for the proxy (e.g., "127.0.0.1:0").
    pub listen_addr: String,
    /// Secrets to substitute in proxied traffic.
    /// Keys are placeholder names (e.g., "SECRET_API_KEY"), values are the actual secrets.
    pub secrets: HashMap<String, Vec<u8>>,
    /// Maximum secret buffer lifetime in seconds before zeroize.
    pub secret_lifetime_secs: u64,
}

impl TlsProxyConfig {
    pub fn new(listen_addr: impl Into<String>) -> Self {
        Self {
            listen_addr: listen_addr.into(),
            secrets: HashMap::new(),
            secret_lifetime_secs: 300,
        }
    }

    pub fn with_secret(mut self, placeholder: impl Into<String>, value: Vec<u8>) -> Self {
        self.secrets.insert(placeholder.into(), value);
        self
    }

    pub fn with_secret_lifetime(mut self, secs: u64) -> Self {
        self.secret_lifetime_secs = secs;
        self
    }
}

/// Represents a dynamically generated Root CA for the TLS proxy.
pub struct ProxyCertificate {
    /// The CA certificate in DER format.
    pub cert_der: Vec<u8>,
    /// The CA private key in DER format.
    pub key_der: Vec<u8>,
    /// The CA's self-signed certificate (needed for signing leaf certs).
    ca_cert: rcgen::Certificate,
    /// The CA's key pair (needed for signing leaf certs).
    ca_key: rcgen::KeyPair,
}

impl Drop for ProxyCertificate {
    fn drop(&mut self) {
        self.key_der.zeroize();
    }
}

/// A secret buffer that is automatically zeroized when dropped.
#[derive(Debug)]
pub struct SecretBuffer {
    data: Vec<u8>,
}

impl SecretBuffer {
    pub fn new(data: Vec<u8>) -> Self {
        Self { data }
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.data
    }
}

impl Drop for SecretBuffer {
    fn drop(&mut self) {
        self.data.zeroize();
    }
}

/// Generates a self-signed Root CA certificate for the TLS proxy.
///
/// The CA is injected into the guest's trust store so the proxy can
/// transparently terminate TLS connections.
pub fn generate_proxy_ca() -> Result<ProxyCertificate, NetError> {
    let key_pair = rcgen::KeyPair::generate()
        .map_err(|e| NetError::Tls(format!("failed to generate key pair: {}", e)))?;

    let mut params = rcgen::CertificateParams::new(vec!["Savant Sandbox Proxy CA".to_string()])
        .map_err(|e| NetError::Tls(format!("failed to create CA params: {}", e)))?;

    params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);

    let cert = params
        .self_signed(&key_pair)
        .map_err(|e| NetError::Tls(format!("failed to generate CA certificate: {}", e)))?;

    let cert_der = cert.der().to_vec();
    let key_der = key_pair.serialize_der();

    Ok(ProxyCertificate {
        cert_der,
        key_der,
        ca_cert: cert,
        ca_key: key_pair,
    })
}

/// Generates a leaf certificate for the given hostname, signed by the CA.
fn generate_leaf_cert(
    hostname: &str,
    ca_cert: &rcgen::Certificate,
    ca_key: &rcgen::KeyPair,
) -> Result<(CertificateDer<'static>, PrivateKeyDer<'static>), NetError> {
    let leaf_key = rcgen::KeyPair::generate()
        .map_err(|e| NetError::Tls(format!("failed to generate leaf key: {}", e)))?;

    let mut params = rcgen::CertificateParams::new(vec![hostname.to_string()])
        .map_err(|e| NetError::Tls(format!("failed to create leaf params: {}", e)))?;

    params.is_ca = rcgen::IsCa::NoCa;

    let leaf_cert = params
        .signed_by(&leaf_key, ca_cert, ca_key)
        .map_err(|e| NetError::Tls(format!("failed to sign leaf cert: {}", e)))?;

    let cert_der = CertificateDer::from(leaf_cert.der().to_vec());
    let key_der = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(leaf_key.serialize_der()));

    Ok((cert_der, key_der))
}

/// Substitutes secret placeholders in plaintext data.
///
/// Scans for `{{PLACEHOLDER}}` patterns and replaces them with the actual secret values.
/// Returns the substituted data and the number of substitutions made.
pub fn substitute_secrets(data: &[u8], secrets: &HashMap<String, Vec<u8>>) -> (Vec<u8>, usize) {
    if secrets.is_empty() {
        return (data.to_vec(), 0);
    }

    let mut result = Vec::with_capacity(data.len());
    let mut count = 0;
    let mut i = 0;

    while i < data.len() {
        // Look for "{{" pattern
        if i + 1 < data.len() && data[i] == b'{' && data[i + 1] == b'{' {
            // Find the closing "}}"
            if let Some(end) = find_closing(&data[i..]) {
                let placeholder = &data[i + 2..i + end - 2];
                if let Ok(key) = std::str::from_utf8(placeholder) {
                    if let Some(value) = secrets.get(key) {
                        result.extend_from_slice(value);
                        count += 1;
                        i += end;
                        continue;
                    }
                }
            }
        }
        result.push(data[i]);
        i += 1;
    }

    (result, count)
}

/// Finds the closing "}}" in a byte slice starting with "{{".
/// Returns the index of the byte after "}}".
/// SAN-06: Guard against underflow on short input.
fn find_closing(data: &[u8]) -> Option<usize> {
    if data.len() < 4 {
        return None;
    }
    for i in 2..data.len() - 1 {
        if data[i] == b'}' && data[i + 1] == b'}' {
            return Some(i + 2);
        }
    }
    None
}

/// TLS proxy that performs transparent MitM with secret substitution.
///
/// Architecture:
/// 1. Generate a Root CA and inject it into the guest trust store
/// 2. Intercept TLS connections from the guest
/// 3. Terminate TLS with a per-hostname cert signed by the CA
/// 4. Scan plaintext for `{{SECRET_*}}` placeholders
/// 5. Substitute with actual secrets
/// 6. Re-encrypt and forward to the real destination
/// 7. Zeroize all buffers after substitution
pub struct TlsProxy {
    config: TlsProxyConfig,
    ca: ProxyCertificate,
    /// Optional credential vault for centralized secret management.
    vault: Option<Arc<CredentialVault>>,
}

impl TlsProxy {
    pub fn new(config: TlsProxyConfig) -> Result<Self, NetError> {
        let ca = generate_proxy_ca()?;
        Ok(Self {
            config,
            ca,
            vault: None,
        })
    }

    /// Attaches a credential vault to this proxy.
    pub fn with_vault(mut self, vault: Arc<CredentialVault>) -> Self {
        self.vault = Some(vault);
        self
    }

    /// Returns the CA certificate in DER format for injection into the guest.
    pub fn ca_certificate_der(&self) -> &[u8] {
        &self.ca.cert_der
    }

    /// Processes a plaintext request: substitutes secrets, then returns the
    /// modified data. Uses the credential vault if attached, otherwise falls
    /// back to the config's inline secrets.
    pub fn process_request(&self, plaintext: &[u8]) -> Result<Vec<u8>, NetError> {
        if let Some(ref vault) = self.vault {
            // Use vault for substitution
            let input_str = std::str::from_utf8(plaintext)
                .map_err(|e| NetError::Tls(format!("non-UTF-8 plaintext: {}", e)))?;
            let substituted = vault.substitute(input_str);
            let count = substituted.len().saturating_sub(plaintext.len());
            if count > 0 {
                tracing::debug!("vault substituted {} secret placeholders", count);
            }
            Ok(substituted.into_bytes())
        } else {
            // Fallback to inline secrets
            let (substituted, count) = substitute_secrets(plaintext, &self.config.secrets);
            if count > 0 {
                tracing::debug!("substituted {} secret placeholders", count);
            }
            Ok(substituted)
        }
    }

    /// Redacts secrets from data for safe logging. Uses vault if attached.
    pub fn redact(&self, data: &str) -> String {
        if let Some(ref vault) = self.vault {
            vault.redact(data)
        } else {
            // Without a vault, we cannot reverse-map values to placeholder names.
            // Return the input as-is — callers should attach a vault for proper redaction.
            data.to_string()
        }
    }

    /// Returns the proxy configuration.
    pub fn config(&self) -> &TlsProxyConfig {
        &self.config
    }

    /// Returns a reference to the credential vault, if attached.
    pub fn vault(&self) -> Option<&Arc<CredentialVault>> {
        self.vault.as_ref()
    }
}

/// Runs the TLS proxy server.
///
/// Accepts incoming TLS connections, extracts SNI from the ClientHello to
/// determine the target hostname, validates against the network policy,
/// terminates TLS with the generated CA, substitutes secrets in plaintext,
/// and forwards to the real destination.
pub async fn run_proxy(
    listen_addr: std::net::SocketAddr,
    ca: &ProxyCertificate,
    policy: &super::NetworkPolicy,
    ssrf_guard: Arc<super::ssrf::SsrfGuard>,
    secrets: HashMap<String, Vec<u8>>,
) -> Result<(), NetError> {
    let listener = TcpListener::bind(listen_addr)
        .await
        .map_err(|e| NetError::Io(format!("failed to bind proxy: {}", e)))?;

    tracing::info!("[tls-proxy] Listening on {}", listen_addr);

    // Build a rustls ClientConfig for upstream connections.
    // Uses the system's default root certificates for verification.
    let mut root_store = rustls::RootCertStore::empty();
    root_store.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    let client_config = rustls::ClientConfig::builder()
        .with_root_certificates(root_store)
        .with_no_client_auth();

    let client_config = Arc::new(client_config);

    loop {
        let (client_stream, peer_addr) = listener
            .accept()
            .await
            .map_err(|e| NetError::Io(format!("accept failed: {}", e)))?;

        tracing::debug!("[tls-proxy] Connection from {}", peer_addr);

        // Peek at the initial ClientHello to extract SNI before spawning.
        let mut peek_buf = vec![0u8; 4096];
        let n = match client_stream.peek(&mut peek_buf).await {
            Ok(n) => n,
            Err(e) => {
                tracing::warn!("[tls-proxy] failed to peek from {}: {}", peer_addr, e);
                continue;
            }
        };
        peek_buf.truncate(n);

        let hostname = match extract_sni(&peek_buf) {
            Some(h) => h,
            None => {
                tracing::warn!("[tls-proxy] no SNI in ClientHello from {}", peer_addr);
                continue;
            }
        };

        // Validate hostname against allowed domains
        if !policy.allowed_domains.is_empty() {
            let allowed = policy
                .allowed_domains
                .iter()
                .any(|d| hostname == *d || hostname.ends_with(&format!(".{}", d)));
            if !allowed {
                tracing::warn!("[tls-proxy] domain {} not in allowed list", hostname);
                continue;
            }
        }

        // Generate a leaf cert for this hostname, signed by our CA.
        let (leaf_cert_der, leaf_key_der) =
            match generate_leaf_cert(&hostname, &ca.ca_cert, &ca.ca_key) {
                Ok(pair) => pair,
                Err(e) => {
                    tracing::warn!(
                        "[tls-proxy] failed to generate cert for {}: {}",
                        hostname,
                        e
                    );
                    continue;
                }
            };

        let allowed_domains: Vec<String> = policy.allowed_domains.clone();
        let guard = Arc::clone(&ssrf_guard);
        let upstream_tls = Arc::clone(&client_config);
        let conn_secrets = secrets.clone();

        tokio::spawn(async move {
            if let Err(e) = handle_connection(
                client_stream,
                leaf_cert_der,
                leaf_key_der,
                hostname,
                &allowed_domains,
                &guard,
                &upstream_tls,
                &conn_secrets,
            )
            .await
            {
                tracing::warn!("[tls-proxy] Connection error from {}: {}", peer_addr, e);
            }
        });
    }
}

/// Handles a single proxy connection: terminates TLS with a pre-generated cert,
/// substitutes secrets in plaintext, and forwards to upstream over TLS.
#[allow(clippy::too_many_arguments)]
async fn handle_connection(
    client_stream: TcpStream,
    leaf_cert_der: CertificateDer<'static>,
    leaf_key_der: PrivateKeyDer<'static>,
    hostname: String,
    allowed_domains: &[String],
    ssrf_guard: &super::ssrf::SsrfGuard,
    upstream_tls: &Arc<rustls::ClientConfig>,
    secrets: &HashMap<String, Vec<u8>>,
) -> Result<(), NetError> {
    tracing::info!("[tls-proxy] SNI: {}", hostname);

    // Validate hostname against allowed domains (already checked in run_proxy,
    // but double-check for safety).
    if !allowed_domains.is_empty() {
        let allowed = allowed_domains
            .iter()
            .any(|d| hostname == *d || hostname.ends_with(&format!(".{}", d)));
        if !allowed {
            return Err(NetError::AccessDenied(format!(
                "domain {} not in allowed list",
                hostname
            )));
        }
    }

    // Validate against SSRF guard
    let resolved_ip = tokio::net::lookup_host(format!("{}:443", hostname))
        .await
        .map_err(|e| NetError::Dns(format!("DNS lookup failed for {}: {}", hostname, e)))?
        .next()
        .ok_or_else(|| NetError::Dns(format!("no addresses for {}", hostname)))?
        .ip();

    ssrf_guard.validate_ip(&resolved_ip)?;

    tracing::info!(
        "[tls-proxy] Forwarding {} -> {} ({})",
        hostname,
        resolved_ip,
        "validated"
    );

    // Build a rustls ServerConfig with the generated cert.
    let server_config = rustls::ServerConfig::builder()
        .with_no_client_auth()
        .with_single_cert(vec![leaf_cert_der], leaf_key_der)
        .map_err(|e| NetError::Tls(format!("failed to build server TLS config: {}", e)))?;

    // Perform TLS handshake with the client (terminate TLS).
    let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(server_config));
    let mut client_tls = acceptor
        .accept(client_stream)
        .await
        .map_err(|e| NetError::Tls(format!("client TLS handshake failed: {}", e)))?;

    // Read plaintext from the client (decrypted).
    let mut plaintext = Vec::with_capacity(8192);
    client_tls
        .read_buf(&mut plaintext)
        .await
        .map_err(|e| NetError::Io(format!("failed to read client plaintext: {}", e)))?;

    if plaintext.is_empty() {
        return Ok(());
    }

    // Apply secret substitution on the plaintext.
    let substituted = substitute_secrets(&plaintext, secrets);
    let data_to_send = if substituted.1 > 0 {
        substituted.0
    } else {
        plaintext.clone()
    };

    // Zeroize the plaintext buffer (may contain secrets).
    plaintext.zeroize();

    // Connect to the upstream server over TLS.
    let upstream_addr = format!("{}:443", hostname);
    let upstream_tcp = TcpStream::connect(&upstream_addr)
        .await
        .map_err(|e| NetError::Io(format!("upstream connect failed: {}", e)))?;

    let server_name = ServerName::try_from(hostname.clone())
        .map_err(|e| NetError::Tls(format!("invalid server name: {}", e)))?;

    let connector = tokio_rustls::TlsConnector::from(Arc::clone(upstream_tls));
    let mut upstream_tls_stream = connector
        .connect(server_name, upstream_tcp)
        .await
        .map_err(|e| NetError::Tls(format!("upstream TLS handshake failed: {}", e)))?;

    // Forward the (substituted) plaintext to upstream.
    upstream_tls_stream
        .write_all(&data_to_send)
        .await
        .map_err(|e| NetError::Io(format!("upstream write failed: {}", e)))?;

    // Read response from upstream and forward back to client.
    let mut response = Vec::with_capacity(8192);
    upstream_tls_stream
        .read_buf(&mut response)
        .await
        .map_err(|e| NetError::Io(format!("upstream read failed: {}", e)))?;

    if !response.is_empty() {
        client_tls
            .write_all(&response)
            .await
            .map_err(|e| NetError::Io(format!("client write failed: {}", e)))?;
    }

    // Zeroize response buffer (may contain sensitive data).
    response.zeroize();

    // Bidirectional relay for remaining data (e.g., streaming responses).
    let (mut client_read, mut client_write) = tokio::io::split(client_tls);
    let (mut upstream_read, mut upstream_write) = tokio::io::split(upstream_tls_stream);

    let client_to_upstream = tokio::io::copy(&mut client_read, &mut upstream_write);
    let upstream_to_client = tokio::io::copy(&mut upstream_read, &mut client_write);

    tokio::select! {
        r = client_to_upstream => {
            if let Err(e) = r {
                tracing::debug!("[tls-proxy] client->upstream ended: {}", e);
            }
        }
        r = upstream_to_client => {
            if let Err(e) = r {
                tracing::debug!("[tls-proxy] upstream->client ended: {}", e);
            }
        }
    }

    Ok(())
}

/// Extracts the SNI hostname from a raw TLS ClientHello message.
///
/// Returns `None` if the ClientHello doesn't contain an SNI extension.
fn extract_sni(data: &[u8]) -> Option<String> {
    // Minimal TLS ClientHello parser
    // TLS record: ContentType(1) Version(2) Length(2) Handshake(1) ...
    if data.len() < 5 {
        return None;
    }

    // ContentType must be 22 (Handshake)
    if data[0] != 0x16 {
        return None;
    }

    // Skip record header (5 bytes), then handshake header (4 bytes)
    let mut pos = 5;
    if data.len() < pos + 4 {
        return None;
    }

    // HandshakeType must be 1 (ClientHello)
    if data[pos] != 0x01 {
        return None;
    }
    pos += 4; // Skip handshake type + length

    // Skip client version (2 bytes)
    pos += 2;
    // Skip client random (32 bytes)
    pos += 32;

    if data.len() < pos + 1 {
        return None;
    }

    // Session ID length
    let session_id_len = data[pos] as usize;
    pos += 1 + session_id_len;

    if data.len() < pos + 2 {
        return None;
    }

    // Cipher suites length
    let cipher_suites_len = u16::from_be_bytes([data[pos], data[pos + 1]]) as usize;
    pos += 2 + cipher_suites_len;

    if data.len() < pos + 1 {
        return None;
    }

    // Compression methods length
    let compression_len = data[pos] as usize;
    pos += 1 + compression_len;

    if data.len() < pos + 2 {
        return None;
    }

    // Extensions length
    let extensions_len = u16::from_be_bytes([data[pos], data[pos + 1]]) as usize;
    pos += 2;

    let extensions_end = pos + extensions_len;
    if data.len() < extensions_end {
        return None;
    }

    // Parse extensions looking for SNI (type 0)
    while pos + 4 <= extensions_end {
        let ext_type = u16::from_be_bytes([data[pos], data[pos + 1]]);
        let ext_len = u16::from_be_bytes([data[pos + 2], data[pos + 3]]) as usize;
        pos += 4;

        if pos + ext_len > extensions_end {
            return None;
        }

        if ext_type == 0 {
            // SNI extension
            let sni_data = &data[pos..pos + ext_len];
            if sni_data.len() < 2 {
                return None;
            }
            // SNI list length (we skip it)
            let mut sni_pos = 2;
            if sni_pos + 3 > sni_data.len() {
                return None;
            }
            // Name type (must be 0 for hostname)
            if sni_data[sni_pos] != 0 {
                return None;
            }
            sni_pos += 1;
            // Name length
            let name_len = u16::from_be_bytes([sni_data[sni_pos], sni_data[sni_pos + 1]]) as usize;
            sni_pos += 2;

            if sni_pos + name_len > sni_data.len() {
                return None;
            }

            return String::from_utf8(sni_data[sni_pos..sni_pos + name_len].to_vec()).ok();
        }

        pos += ext_len;
    }

    None
}
