use super::NetError;
use std::collections::HashMap;
use std::net::IpAddr;
use std::time::{Duration, Instant};

/// DNS interceptor that prevents DNS rebinding attacks.
///
/// Intercepts all DNS queries from the sandboxed agent, resolves them via the
/// host resolver, and caches the IP. If an attacker changes the DNS record
/// after the initial resolution, the cached IP is enforced (anti-rebinding).
pub struct DnsInterceptor {
    /// DNS cache: domain -> (ip, inserted_at, ttl).
    cache: HashMap<String, CachedDnsEntry>,
    /// Allowed domains. If empty, all domains are allowed.
    allowed_domains: Vec<String>,
    /// Maximum cache entries before LRU eviction.
    max_cache_entries: usize,
}

#[derive(Debug, Clone)]
struct CachedDnsEntry {
    ip: IpAddr,
    inserted_at: Instant,
    ttl: Duration,
}

impl CachedDnsEntry {
    fn is_expired(&self) -> bool {
        self.inserted_at.elapsed() > self.ttl
    }
}

impl DnsInterceptor {
    pub fn new() -> Self {
        Self {
            cache: HashMap::new(),
            allowed_domains: Vec::new(),
            max_cache_entries: 1024,
        }
    }

    /// Sets the allowed domain list. If empty, all domains are allowed.
    pub fn with_allowed_domains(mut self, domains: Vec<String>) -> Self {
        self.allowed_domains = domains;
        self
    }

    /// Hot-reload the allowed domain list at runtime.
    pub fn update_allowed_domains(&mut self, domains: Vec<String>) {
        self.allowed_domains = domains;
    }

    /// Sets the maximum cache size.
    pub fn with_max_cache_entries(mut self, max: usize) -> Self {
        self.max_cache_entries = max;
        self
    }

    /// Resolves a domain name, using the cache if available.
    /// Returns the IP address and a `NetworkToken` duration hint.
    ///
    /// SAN-02: This method performs blocking DNS resolution. When called from
    /// async context, wrap in `tokio::task::spawn_blocking`.
    pub fn resolve(&mut self, domain: &str, token_duration: Duration) -> Result<IpAddr, NetError> {
        // Check domain allowlist
        // SAN-14: Require segment-boundary match to prevent evil.com matching .com
        if !self.allowed_domains.is_empty() {
            let allowed = self.allowed_domains.iter().any(|d| {
                domain == *d || domain.ends_with(&format!(".{}", d.trim_start_matches('.')))
            });
            if !allowed {
                return Err(NetError::AccessDenied(format!(
                    "domain {} is not in the allowlist",
                    domain
                )));
            }
        }

        // Check cache first (anti-rebinding: return cached IP even if DNS changed)
        if let Some(entry) = self.cache.get(domain) {
            if !entry.is_expired() {
                return Ok(entry.ip);
            }
        }

        // Resolve via host resolver
        let ip = self.resolve_via_host(domain)?;

        // Cache the result
        self.cache_insert(domain.to_string(), ip, token_duration);

        Ok(ip)
    }

    /// Returns the cached IP for a domain, if it exists and hasn't expired.
    pub fn get_cached(&self, domain: &str) -> Option<IpAddr> {
        self.cache.get(domain).and_then(|entry| {
            if entry.is_expired() {
                None
            } else {
                Some(entry.ip)
            }
        })
    }

    /// Clears expired entries from the cache.
    pub fn cleanup(&mut self) {
        self.cache.retain(|_, entry| !entry.is_expired());
    }

    /// Returns the number of cached entries.
    pub fn cache_size(&self) -> usize {
        self.cache.len()
    }

    fn resolve_via_host(&self, domain: &str) -> Result<IpAddr, NetError> {
        // Use std::net for DNS resolution (goes through the host resolver)
        use std::net::ToSocketAddrs;
        let addr = format!("{}:0", domain);
        let mut addrs = addr
            .to_socket_addrs()
            .map_err(|e| NetError::Dns(format!("failed to resolve {}: {}", domain, e)))?;

        addrs
            .next()
            .map(|a| a.ip())
            .ok_or_else(|| NetError::Dns(format!("no addresses found for {}", domain)))
    }

    fn cache_insert(&mut self, domain: String, ip: IpAddr, ttl: Duration) {
        // Evict oldest entry if cache is full
        if self.cache.len() >= self.max_cache_entries {
            if let Some(oldest_key) = self
                .cache
                .iter()
                .min_by_key(|(_, e)| e.inserted_at)
                .map(|(k, _)| k.clone())
            {
                self.cache.remove(&oldest_key);
            }
        }

        self.cache.insert(
            domain,
            CachedDnsEntry {
                ip,
                inserted_at: Instant::now(),
                ttl,
            },
        );
    }
}

impl Default for DnsInterceptor {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
#[allow(clippy::disallowed_methods)]
mod tests {
    use super::*;

    #[test]
    fn test_resolve_caches_result() {
        let mut interceptor = DnsInterceptor::new();
        let ip1 = interceptor
            .resolve("localhost", Duration::from_secs(60))
            .expect("resolve failed");
        let ip2 = interceptor.get_cached("localhost").expect("not cached");
        assert_eq!(ip1, ip2);
    }

    #[test]
    fn test_anti_rebinding() {
        let mut interceptor = DnsInterceptor::new();
        // First resolution caches the IP
        let ip1 = interceptor
            .resolve("localhost", Duration::from_secs(60))
            .expect("resolve failed");
        // Second resolution returns the cached IP (even if DNS changed)
        let ip2 = interceptor
            .resolve("localhost", Duration::from_secs(60))
            .expect("resolve failed");
        assert_eq!(ip1, ip2);
    }

    #[test]
    fn test_domain_allowlist() {
        let mut interceptor =
            DnsInterceptor::new().with_allowed_domains(vec!["example.com".to_string()]);
        // Allowed domain should resolve
        let result = interceptor.resolve("sub.example.com", Duration::from_secs(60));
        // This will fail because example.com doesn't resolve in test env,
        // but the allowlist check should pass
        match result {
            Ok(_) => {}
            Err(NetError::Dns(_)) => {} // DNS resolution failure is OK in tests
            Err(NetError::AccessDenied(_)) => panic!("should not be denied by allowlist"),
            Err(_) => {}
        }
    }

    #[test]
    fn test_domain_not_in_allowlist() {
        let mut interceptor =
            DnsInterceptor::new().with_allowed_domains(vec!["example.com".to_string()]);
        let result = interceptor.resolve("evil.com", Duration::from_secs(60));
        assert!(result.is_err());
    }

    #[test]
    fn test_cache_size() {
        let mut interceptor = DnsInterceptor::new();
        assert_eq!(interceptor.cache_size(), 0);

        interceptor.cache.insert(
            "test.com".to_string(),
            CachedDnsEntry {
                ip: IpAddr::V4(std::net::Ipv4Addr::new(1, 2, 3, 4)),
                inserted_at: Instant::now(),
                ttl: Duration::from_secs(60),
            },
        );
        assert_eq!(interceptor.cache_size(), 1);
    }

    #[test]
    fn test_cleanup_removes_expired() {
        let mut interceptor = DnsInterceptor::new();
        interceptor.cache.insert(
            "expired.com".to_string(),
            CachedDnsEntry {
                ip: IpAddr::V4(std::net::Ipv4Addr::new(1, 2, 3, 4)),
                inserted_at: Instant::now() - Duration::from_secs(300),
                ttl: Duration::from_secs(60),
            },
        );
        assert_eq!(interceptor.cache_size(), 1);
        interceptor.cleanup();
        assert_eq!(interceptor.cache_size(), 0);
    }
}
