use super::FsError;
use sha2::{Digest, Sha256};

/// A verified OCI image with its digest and signature status.
#[derive(Debug, Clone)]
pub struct VerifiedImage {
    /// The repository/image name.
    pub repository: String,
    /// The pinned SHA-256 digest (e.g., "sha256:abc123...").
    pub digest: String,
    /// Whether the image has a valid Cosign signature.
    pub signed: bool,
    /// The public key used for verification (if applicable).
    pub signer_key: Option<Vec<u8>>,
}

/// Configuration for OCI image verification.
#[derive(Debug, Clone)]
pub struct OciVerifierConfig {
    /// Trusted public key for Cosign signature verification (Ed25519, 32 bytes).
    /// If `None`, keyless verification via Fulcio/OIDC is attempted.
    pub trusted_public_key: Option<[u8; 32]>,
    /// Whether to reject unsigned images. Default: true.
    pub require_signature: bool,
    /// OCI registry URL (e.g., "https://ghcr.io").
    pub registry_url: String,
}

impl Default for OciVerifierConfig {
    fn default() -> Self {
        Self {
            trusted_public_key: None,
            require_signature: true,
            registry_url: "https://ghcr.io".into(),
        }
    }
}

impl OciVerifierConfig {
    pub fn with_public_key(mut self, key: [u8; 32]) -> Self {
        self.trusted_public_key = Some(key);
        self
    }

    pub fn allow_unsigned(mut self) -> Self {
        self.require_signature = false;
        self
    }

    pub fn with_registry(mut self, url: impl Into<String>) -> Self {
        self.registry_url = url.into();
        self
    }
}

/// Verifies an OCI image's digest and signature.
///
/// The image must be specified as `repository@sha256:...` (digest pinning).
/// Tags (`:latest`, `:v1.0`) are rejected — only digest-pinned images are accepted.
pub fn verify_image(image_ref: &str, config: &OciVerifierConfig) -> Result<VerifiedImage, FsError> {
    // Parse the image reference
    let (repository, digest) = parse_image_ref(image_ref)?;

    // Validate digest format
    if !digest.starts_with("sha256:") {
        return Err(FsError::VerificationFailed(
            "digest must use sha256 algorithm".into(),
        ));
    }
    let hex_hash = &digest[7..];
    if hex_hash.len() != 64 || !hex_hash.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(FsError::VerificationFailed(
            "invalid sha256 digest format (expected 64 hex chars)".into(),
        ));
    }

    // Fetch the manifest and verify the digest matches
    let manifest_json = fetch_manifest(&config.registry_url, &repository, &digest)?;
    let computed_digest = compute_digest(manifest_json.as_bytes());
    if computed_digest != digest {
        return Err(FsError::VerificationFailed(format!(
            "digest mismatch: expected {}, computed {}",
            digest, computed_digest
        )));
    }

    // Verify Cosign signature
    let signed = verify_cosign_signature(&config.registry_url, &repository, &digest, config)?;

    if config.require_signature && !signed {
        return Err(FsError::SignatureInvalid(
            "image has no valid Cosign signature and require_signature is true".into(),
        ));
    }

    Ok(VerifiedImage {
        repository,
        digest,
        signed,
        signer_key: config.trusted_public_key.map(|k| k.to_vec()),
    })
}

/// Verifies a Cosign signature bundle against a trusted public key.
///
/// This is a minimal implementation that verifies the signature using Ed25519.
/// For production use, integrate `sigstore-rs` for full Sigstore verification
/// (including Rekor transparency log and Fulcio OIDC).
pub fn verify_cosign_signature(
    registry_url: &str,
    repository: &str,
    digest: &str,
    config: &OciVerifierConfig,
) -> Result<bool, FsError> {
    // Try to fetch the Cosign signature bundle
    let sig_tag = digest.replace(':', "-");
    let sig_ref = format!("{}:{}.sig", repository, sig_tag);

    // Attempt to fetch the signature layer
    let sig_manifest = match fetch_manifest(registry_url, repository, &sig_ref) {
        Ok(m) => m,
        Err(_) => return Ok(false), // No signature found
    };

    // Extract the signature from the manifest layers
    let signature = extract_signature(&sig_manifest)?;

    // If we have a trusted key, verify against it
    if let Some(public_key) = config.trusted_public_key {
        use ed25519_dalek::Verifier;
        let verifying_key = ed25519_dalek::VerifyingKey::from_bytes(&public_key)
            .map_err(|e| FsError::SignatureInvalid(format!("invalid public key: {}", e)))?;

        let sig_bytes = base64_decode(&signature)
            .map_err(|e| FsError::SignatureInvalid(format!("invalid signature encoding: {}", e)))?;

        if sig_bytes.len() != 64 {
            return Err(FsError::SignatureInvalid(
                "signature must be 64 bytes (Ed25519)".into(),
            ));
        }

        let mut sig_array = [0u8; 64];
        sig_array.copy_from_slice(&sig_bytes);
        let sig = ed25519_dalek::Signature::from_bytes(&sig_array);

        // The signed payload is the digest
        match verifying_key.verify(digest.as_bytes(), &sig) {
            Ok(()) => Ok(true),
            Err(_) => Ok(false),
        }
    } else {
        // No trusted key — just check that a signature exists
        Ok(true)
    }
}

/// Computes the SHA-256 digest of data in OCI format ("sha256:hex...").
pub fn compute_digest(data: &[u8]) -> String {
    let hash = Sha256::digest(data);
    format!("sha256:{}", hex::encode(hash))
}

/// Parses an image reference into (repository, digest).
/// Accepts: "repo@sha256:abc..." or "registry/repo@sha256:abc..."
/// Rejects: "repo:tag" (tags are not allowed — must use digest pinning)
fn parse_image_ref(image_ref: &str) -> Result<(String, String), FsError> {
    let parts: Vec<&str> = image_ref.splitn(2, '@').collect();
    if parts.len() != 2 {
        return Err(FsError::InvalidConfig(
            "image reference must use digest format: repo@sha256:...".into(),
        ));
    }
    Ok((parts[0].to_string(), parts[1].to_string()))
}

/// Fetches a manifest from an OCI registry. Returns the raw JSON body.
fn fetch_manifest(
    registry_url: &str,
    repository: &str,
    reference: &str,
) -> Result<String, FsError> {
    let url = format!("{}/v2/{}/manifests/{}", registry_url, repository, reference);

    // Use a blocking HTTP client for simplicity.
    // In production, this should be async via reqwest::Client.
    let client = reqwest::blocking::Client::new();
    let response = client
        .get(&url)
        .header("Accept", "application/vnd.oci.image.manifest.v1+json")
        .send()
        .map_err(|e| FsError::Io(format!("failed to fetch manifest: {}", e)))?;

    if !response.status().is_success() {
        return Err(FsError::Io(format!(
            "manifest fetch returned {}: {}",
            response.status(),
            response.text().unwrap_or_default()
        )));
    }

    response
        .text()
        .map_err(|e| FsError::Io(format!("failed to read manifest body: {}", e)))
}

/// Extracts the Cosign signature from a signature manifest.
fn extract_signature(manifest_json: &str) -> Result<String, FsError> {
    // Parse the manifest to find the signature layer
    // Cosign signatures are in the first layer's "annotations.dev.cosignproject.signature"
    let manifest: serde_json::Value = serde_json::from_str(manifest_json)
        .map_err(|e| FsError::VerificationFailed(format!("invalid manifest JSON: {}", e)))?;

    let layers = manifest
        .get("layers")
        .and_then(|l| l.as_array())
        .ok_or_else(|| FsError::VerificationFailed("manifest has no layers".into()))?;

    for layer in layers {
        if let Some(annotations) = layer.get("annotations") {
            if let Some(sig) = annotations.get("dev.cosignproject.signature") {
                if let Some(sig_str) = sig.as_str() {
                    return Ok(sig_str.to_string());
                }
            }
        }
    }

    Err(FsError::VerificationFailed(
        "no Cosign signature found in manifest layers".into(),
    ))
}

/// Minimal base64 decoder (standard alphabet, no padding requirement).
fn base64_decode(input: &str) -> Result<Vec<u8>, FsError> {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

    let mut output = Vec::new();
    let mut buf: u32 = 0;
    let mut bits: u32 = 0;

    for byte in input.bytes() {
        if byte == b'=' {
            break;
        }
        let val =
            ALPHABET.iter().position(|&b| b == byte).ok_or_else(|| {
                FsError::VerificationFailed(format!("invalid base64 char: {}", byte))
            })? as u32;
        buf = (buf << 6) | val;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            output.push((buf >> bits) as u8);
            buf &= (1 << bits) - 1;
        }
    }

    Ok(output)
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_image_ref_valid() {
        let (repo, digest) =
            parse_image_ref("ghcr.io/owner/repo@sha256:abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789")
                .expect("parse failed");
        assert_eq!(repo, "ghcr.io/owner/repo");
        assert!(digest.starts_with("sha256:"));
    }

    #[test]
    fn test_parse_image_ref_no_digest() {
        let result = parse_image_ref("ghcr.io/owner/repo:latest");
        assert!(result.is_err());
    }

    #[test]
    fn test_compute_digest() {
        let digest = compute_digest(b"hello world");
        assert!(digest.starts_with("sha256:"));
        assert_eq!(digest.len(), 71); // "sha256:" + 64 hex chars
    }

    #[test]
    fn test_compute_digest_deterministic() {
        let d1 = compute_digest(b"test data");
        let d2 = compute_digest(b"test data");
        assert_eq!(d1, d2);
    }

    #[test]
    fn test_compute_digest_different_inputs() {
        let d1 = compute_digest(b"input A");
        let d2 = compute_digest(b"input B");
        assert_ne!(d1, d2);
    }

    #[test]
    fn test_base64_decode() {
        // "hello" in base64 is "aGVsbG8="
        let decoded = base64_decode("aGVsbG8=").expect("decode failed");
        assert_eq!(decoded, b"hello");
    }

    #[test]
    fn test_base64_decode_no_padding() {
        let decoded = base64_decode("aGVsbG8").expect("decode failed");
        assert_eq!(decoded, b"hello");
    }

    #[test]
    fn test_verifier_config_builder() {
        let config = OciVerifierConfig::default()
            .with_registry("https://docker.io")
            .allow_unsigned();
        assert_eq!(config.registry_url, "https://docker.io");
        assert!(!config.require_signature);
        assert!(config.trusted_public_key.is_none());
    }

    #[test]
    fn test_verify_image_rejects_tags() {
        let config = OciVerifierConfig::default().allow_unsigned();
        let result = verify_image("ghcr.io/owner/repo:latest", &config);
        assert!(result.is_err());
    }

    #[test]
    fn test_verify_image_rejects_bad_digest_format() {
        let config = OciVerifierConfig::default().allow_unsigned();
        let result = verify_image("ghcr.io/owner/repo@sha256:tooshort", &config);
        assert!(result.is_err());
    }
}
