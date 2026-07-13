//! SSRF (Server-Side Request Forgery) protection.
//!
//! Validates URLs and resolved IPs against private network ranges to prevent
//! sandboxed agents from accessing internal services.

use super::NetError;
use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr};
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

/// IP network range for SSRF blocking.
#[derive(Debug, Clone)]
struct BlockedRange {
    name: &'static str,
    matches: fn(IpAddr) -> bool,
}

/// SSRF guard that validates URLs and IPs against private network ranges.
pub struct SsrfGuard {
    blocked_ranges: Vec<BlockedRange>,
    /// DNS cache with pinning: hostname -> (pinned_ip, resolved_at, ttl).
    dns_cache: Mutex<HashMap<String, PinnedDns>>,
}

#[derive(Debug, Clone)]
struct PinnedDns {
    ip: IpAddr,
    resolved_at: Instant,
    ttl: Duration,
}

impl PinnedDns {
    fn is_expired(&self) -> bool {
        self.resolved_at.elapsed() > self.ttl
    }
}

impl SsrfGuard {
    /// Creates a new SSRF guard with all standard private network ranges blocked.
    pub fn new() -> Self {
        let blocked_ranges: Vec<BlockedRange> = vec![
            BlockedRange {
                name: "loopback",
                matches: |ip| match ip {
                    IpAddr::V4(v4) => v4.is_loopback(),
                    IpAddr::V6(v6) => v6.is_loopback(),
                },
            },
            BlockedRange {
                name: "rfc1918",
                matches: |ip| match ip {
                    IpAddr::V4(v4) => v4.is_private(),
                    IpAddr::V6(v6) => (v6.octets()[0] & 0xFE) == 0xFC,
                },
            },
            BlockedRange {
                name: "link-local",
                matches: |ip| match ip {
                    IpAddr::V4(v4) => v4.is_link_local(),
                    IpAddr::V6(v6) => {
                        let octets = v6.octets();
                        octets[0] == 0xFE && (octets[1] & 0xC0) == 0x80
                    }
                },
            },
            BlockedRange {
                name: "unspecified",
                matches: |ip| match ip {
                    IpAddr::V4(v4) => v4.is_unspecified(),
                    IpAddr::V6(v6) => v6.is_unspecified(),
                },
            },
            BlockedRange {
                name: "multicast",
                matches: |ip| match ip {
                    IpAddr::V4(v4) => v4.is_multicast(),
                    IpAddr::V6(v6) => v6.is_multicast(),
                },
            },
            BlockedRange {
                name: "documentation",
                matches: |ip| match ip {
                    IpAddr::V4(v4) => {
                        // 192.0.2.0/24, 198.51.100.0/24, 203.0.113.0/24
                        let octets = v4.octets();
                        (octets[0] == 192 && octets[1] == 0 && octets[2] == 2)
                            || (octets[0] == 198 && octets[1] == 51 && octets[2] == 100)
                            || (octets[0] == 203 && octets[1] == 0 && octets[2] == 113)
                    }
                    IpAddr::V6(_) => false,
                },
            },
            BlockedRange {
                name: "cgnat",
                matches: |ip| match ip {
                    IpAddr::V4(v4) => {
                        let octets = v4.octets();
                        octets[0] == 100 && (octets[1] & 0xC0) == 64
                    }
                    IpAddr::V6(_) => false,
                },
            },
            BlockedRange {
                name: "cloud-metadata",
                matches: |ip| match ip {
                    IpAddr::V4(v4) => {
                        // 169.254.169.254 (AWS/GCP/Azure metadata)
                        v4 == Ipv4Addr::new(169, 254, 169, 254)
                    }
                    IpAddr::V6(_) => false,
                },
            },
        ];

        Self {
            blocked_ranges,
            dns_cache: Mutex::new(HashMap::new()),
        }
    }

    /// Validates that an IP address is not in any blocked range.
    pub fn validate_ip(&self, ip: &IpAddr) -> Result<(), NetError> {
        for range in &self.blocked_ranges {
            if (range.matches)(*ip) {
                return Err(NetError::AccessDenied(format!(
                    "SSRF blocked: {} is in {} range",
                    ip, range.name
                )));
            }
        }
        Ok(())
    }

    /// Validates a URL's hostname resolves to a non-private IP.
    /// Returns the resolved IP if valid.
    pub fn validate_url(&self, url: &str) -> Result<IpAddr, NetError> {
        let hostname = extract_hostname(url)?;
        let ip = resolve_hostname(&hostname)?;
        self.validate_ip(&ip)?;
        Ok(ip)
    }

    /// Validates a URL and pins the DNS resolution to prevent TOCTOU rebinding.
    /// The pinned IP is cached and enforced on subsequent lookups.
    pub async fn validate_and_pin(&self, url: &str, ttl: Duration) -> Result<IpAddr, NetError> {
        let hostname = extract_hostname(url)?;

        // Check pinned cache first (anti-rebinding)
        {
            let cache = self.dns_cache.lock().await;
            if let Some(pinned) = cache.get(&hostname) {
                if !pinned.is_expired() {
                    return Ok(pinned.ip);
                }
            }
        }

        // Resolve and validate
        let ip = resolve_hostname(&hostname)?;
        self.validate_ip(&ip)?;

        // Pin the resolution
        {
            let mut cache = self.dns_cache.lock().await;
            cache.insert(
                hostname,
                PinnedDns {
                    ip,
                    resolved_at: Instant::now(),
                    ttl,
                },
            );
        }

        Ok(ip)
    }

    /// Clears expired DNS pins.
    pub async fn cleanup_pins(&self) {
        let mut cache = self.dns_cache.lock().await;
        cache.retain(|_, pinned| !pinned.is_expired());
    }
}

impl Default for SsrfGuard {
    fn default() -> Self {
        Self::new()
    }
}

/// Extracts the hostname from a URL string.
/// SAN-07: Handles IPv6 literals like `http://[::1]/` and `http://[::1]:8080/`.
fn extract_hostname(url: &str) -> Result<String, NetError> {
    // Simple hostname extraction without pulling in a URL parser
    let without_scheme = url
        .strip_prefix("http://")
        .or_else(|| url.strip_prefix("https://"))
        .unwrap_or(url);

    // SAN-07: Handle IPv6 literal addresses in brackets
    if let Some(rest) = without_scheme.strip_prefix('[') {
        // IPv6 literal: extract until ']'
        let end = rest.find(']').ok_or_else(|| {
            NetError::InvalidConfig(format!("unclosed IPv6 bracket in URL: {}", url))
        })?;
        let host = &rest[..end];
        if host.is_empty() {
            return Err(NetError::InvalidConfig(format!(
                "empty IPv6 address in URL: {}",
                url
            )));
        }
        return Ok(host.to_string());
    }

    let host = without_scheme
        .split('/')
        .next()
        .unwrap_or(without_scheme)
        .split(':')
        .next()
        .unwrap_or(without_scheme)
        .split('?')
        .next()
        .unwrap_or(without_scheme);

    if host.is_empty() {
        return Err(NetError::InvalidConfig(format!(
            "cannot extract hostname from URL: {}",
            url
        )));
    }

    Ok(host.to_string())
}

/// Resolves a hostname to an IP address using the system resolver.
fn resolve_hostname(hostname: &str) -> Result<IpAddr, NetError> {
    use std::net::ToSocketAddrs;
    let addr = format!("{}:0", hostname);
    let mut addrs = addr
        .to_socket_addrs()
        .map_err(|e| NetError::Dns(format!("failed to resolve {}: {}", hostname, e)))?;

    addrs
        .next()
        .map(|a| a.ip())
        .ok_or_else(|| NetError::Dns(format!("no addresses found for {}", hostname)))
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)]
mod tests {
    use super::*;
    use std::net::Ipv6Addr;

    #[test]
    fn test_loopback_blocked() {
        let guard = SsrfGuard::new();
        assert!(guard.validate_ip(&IpAddr::V4(Ipv4Addr::LOCALHOST)).is_err());
        assert!(guard.validate_ip(&IpAddr::V6(Ipv6Addr::LOCALHOST)).is_err());
    }

    #[test]
    fn test_rfc1918_blocked() {
        let guard = SsrfGuard::new();
        assert!(guard
            .validate_ip(&IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)))
            .is_err());
        assert!(guard
            .validate_ip(&IpAddr::V4(Ipv4Addr::new(172, 16, 0, 1)))
            .is_err());
        assert!(guard
            .validate_ip(&IpAddr::V4(Ipv4Addr::new(192, 168, 1, 1)))
            .is_err());
    }

    #[test]
    fn test_link_local_blocked() {
        let guard = SsrfGuard::new();
        assert!(guard
            .validate_ip(&IpAddr::V4(Ipv4Addr::new(169, 254, 1, 1)))
            .is_err());
    }

    #[test]
    fn test_cloud_metadata_blocked() {
        let guard = SsrfGuard::new();
        assert!(guard
            .validate_ip(&IpAddr::V4(Ipv4Addr::new(169, 254, 169, 254)))
            .is_err());
    }

    #[test]
    fn test_cgnat_blocked() {
        let guard = SsrfGuard::new();
        assert!(guard
            .validate_ip(&IpAddr::V4(Ipv4Addr::new(100, 64, 0, 1)))
            .is_err());
    }

    #[test]
    fn test_documentation_blocked() {
        let guard = SsrfGuard::new();
        assert!(guard
            .validate_ip(&IpAddr::V4(Ipv4Addr::new(192, 0, 2, 1)))
            .is_err());
        assert!(guard
            .validate_ip(&IpAddr::V4(Ipv4Addr::new(198, 51, 100, 1)))
            .is_err());
        assert!(guard
            .validate_ip(&IpAddr::V4(Ipv4Addr::new(203, 0, 113, 1)))
            .is_err());
    }

    #[test]
    fn test_public_ip_allowed() {
        let guard = SsrfGuard::new();
        assert!(guard
            .validate_ip(&IpAddr::V4(Ipv4Addr::new(1, 1, 1, 1)))
            .is_ok());
        assert!(guard
            .validate_ip(&IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8)))
            .is_ok());
        assert!(guard
            .validate_ip(&IpAddr::V4(Ipv4Addr::new(93, 184, 216, 34)))
            .is_ok());
    }

    #[test]
    fn test_extract_hostname() {
        assert_eq!(
            extract_hostname("https://example.com/path").unwrap(),
            "example.com"
        );
        assert_eq!(
            extract_hostname("http://example.com:8080/path").unwrap(),
            "example.com"
        );
        assert_eq!(extract_hostname("example.com/path").unwrap(), "example.com");
        assert_eq!(
            extract_hostname("https://sub.example.com").unwrap(),
            "sub.example.com"
        );
    }

    #[test]
    fn test_validate_url_blocks_private() {
        let guard = SsrfGuard::new();
        // These should fail because localhost resolves to 127.0.0.1
        let result = guard.validate_url("http://127.0.0.1");
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_dns_pinning() {
        let guard = SsrfGuard::new();
        // First resolution should pin
        let ip1 = guard
            .validate_and_pin("https://example.com", Duration::from_secs(60))
            .await;
        // Should succeed (example.com resolves to a public IP in most envs)
        // Second call should return the pinned IP
        if let Ok(ip) = ip1 {
            let ip2 = guard
                .validate_and_pin("https://example.com", Duration::from_secs(60))
                .await;
            assert_eq!(ip2.unwrap(), ip);
        }
    }
}
