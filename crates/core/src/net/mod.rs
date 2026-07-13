use crate::error::SavantError;
use std::net::IpAddr;
use std::time::Duration;

/// URL schemes that must never be fetched.
const BLOCKED_SCHEMES: &[&str] = &[
    "file",
    "ftp",
    "sftp",
    "data",
    "javascript",
    "vbscript",
    "about",
];

/// Known cloud metadata hostnames.
const BLOCKED_HOSTNAMES: &[&str] = &[
    "169.254.169.254",          // AWS
    "100.100.100.200",          // Alibaba Cloud
    "metadata.google.internal", // GCP
];

/// Shared SSRF validation for all HTTP fetch paths (web tool, browser tool, etc.).
///
/// Blocks:
/// - Dangerous schemes (file://, javascript:, data:, etc.)
/// - Cloud metadata endpoints
/// - Loopback, private RFC1918, link-local, and unspecified IPs
/// - Localhost and .local hostnames
///
/// Returns `Ok(())` if the URL is safe, `Err(...)` if blocked.
pub fn validate_url(url_str: &str) -> Result<(), SavantError> {
    let parsed = reqwest::Url::parse(url_str)
        .map_err(|e| SavantError::Unknown(format!("Invalid URL: {e}")))?;

    // Block dangerous schemes
    if BLOCKED_SCHEMES.contains(&parsed.scheme()) {
        return Err(SavantError::Unknown(format!(
            "Blocked URL scheme: {}",
            parsed.scheme()
        )));
    }

    let host = match parsed.host_str() {
        Some(h) => h.to_ascii_lowercase(),
        None => return Err(SavantError::Unknown("URL has no host".to_string())),
    };

    // Block known metadata hostnames
    for blocked in BLOCKED_HOSTNAMES {
        if host.as_str() == *blocked {
            return Err(SavantError::Unknown(format!(
                "Blocked internal host: {host}"
            )));
        }
    }

    // Block RFC1918 / loopback / link-local / unspecified IPs
    if let Ok(ip) = host.parse::<IpAddr>() {
        let is_restricted = match ip {
            IpAddr::V4(v4) => {
                v4.is_loopback() || v4.is_private() || v4.is_unspecified() || v4.is_link_local()
            }
            IpAddr::V6(v6) => v6.is_loopback() || v6.is_unspecified(),
        };
        if is_restricted {
            return Err(SavantError::Unknown(format!(
                "Blocked private/loopback IP: {ip}"
            )));
        }
    }

    // Block localhost and .local hostnames
    if host == "localhost" || host.ends_with(".local") {
        return Err(SavantError::Unknown(format!(
            "Blocked local hostname: {host}"
        )));
    }

    Ok(())
}

/// Creates a secure `reqwest::Client` with read timeout, connection pool, and redirect limits.
///
/// PB-05: Uses `read_timeout(30s)` instead of `timeout(12s)` so that long LLM streams
/// don't get killed mid-flight. The read timeout requires data to keep flowing but
/// doesn't cap total duration. Connect timeout stays at 5s.
///
/// COR-05: Falls back to a default client instead of panicking on build failure.
/// Prefer `secure_client_fallible()` for new code.
#[allow(clippy::disallowed_methods)]
pub fn secure_client() -> reqwest::Client {
    secure_client_fallible().unwrap_or_else(|e| {
        tracing::error!("Failed to build secure HTTP client: {}, using default", e);
        reqwest::Client::new()
    })
}

/// Fallible version of `secure_client()` that returns a `Result` instead of panicking.
///
/// Preferred for new code. Use `?` to propagate errors.
#[allow(clippy::disallowed_methods)]
pub fn secure_client_fallible() -> Result<reqwest::Client, SavantError> {
    reqwest::Client::builder()
        .read_timeout(Duration::from_secs(30))
        .connect_timeout(Duration::from_secs(5))
        .pool_max_idle_per_host(4)
        .redirect(reqwest::redirect::Policy::limited(10))
        .build()
        .map_err(|e| SavantError::Unknown(format!("Failed to build HTTP client: {}", e)))
}

/// PB-19: Creates an HTTP client with custom timeout values.
/// Use this when the caller needs different timeout behavior than the defaults.
#[allow(clippy::disallowed_methods)]
pub fn secure_client_with_timeout(
    read_timeout_secs: u64,
    connect_timeout_secs: u64,
) -> Result<reqwest::Client, SavantError> {
    reqwest::Client::builder()
        .read_timeout(Duration::from_secs(read_timeout_secs))
        .connect_timeout(Duration::from_secs(connect_timeout_secs))
        .pool_max_idle_per_host(4)
        .redirect(reqwest::redirect::Policy::limited(10))
        .build()
        .map_err(|e| SavantError::Unknown(format!("Failed to build HTTP client: {}", e)))
}
