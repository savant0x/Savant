use super::NetError;
use std::net::IpAddr;

/// L3 packet filter for the user-space TCP/IP stack.
///
/// Drops all traffic to loopback, RFC 1918, and link-local addresses.
/// Only allows traffic to public IPs that have an active `NetworkToken`.
pub struct PacketFilter {
    /// Allowed destination IPs (from active NetworkTokens).
    allowed_ips: Vec<AllowedIp>,
    /// Bandwidth limit in bytes per second (0 = unlimited).
    bandwidth_bps: u64,
    /// Bytes transferred in the current window.
    bytes_transferred: u64,
    /// Window start time.
    window_start: std::time::Instant,
    /// Optional SSRF guard for additional validation.
    ssrf_guard: Option<super::ssrf::SsrfGuard>,
}

#[derive(Debug, Clone)]
struct AllowedIp {
    ip: IpAddr,
    expires_at: std::time::Instant,
}

impl PacketFilter {
    pub fn new() -> Self {
        Self {
            allowed_ips: Vec::new(),
            bandwidth_bps: 0,
            bytes_transferred: 0,
            window_start: std::time::Instant::now(),
            ssrf_guard: None,
        }
    }

    /// Enables SSRF protection on this packet filter.
    pub fn with_ssrf_guard(mut self) -> Self {
        self.ssrf_guard = Some(super::ssrf::SsrfGuard::new());
        self
    }

    /// Sets the bandwidth limit in bytes per second.
    pub fn with_bandwidth_limit(mut self, bps: u64) -> Self {
        self.bandwidth_bps = bps;
        self
    }

    /// Grants temporary access to a destination IP.
    pub fn allow_ip(&mut self, ip: IpAddr, duration: std::time::Duration) {
        // Remove expired entries first
        self.cleanup_expired();
        self.allowed_ips.push(AllowedIp {
            ip,
            expires_at: std::time::Instant::now() + duration,
        });
    }

    /// Checks if a packet to the given destination IP should be allowed.
    /// Returns `Ok(())` if allowed, `Err(NetError)` if dropped.
    pub fn check_packet(&mut self, dest_ip: IpAddr, packet_size: usize) -> Result<(), NetError> {
        // Always drop loopback
        if is_loopback(dest_ip) {
            return Err(NetError::AccessDenied(format!(
                "loopback traffic blocked: {}",
                dest_ip
            )));
        }

        // Always drop RFC 1918 private addresses
        if is_private(dest_ip) {
            return Err(NetError::AccessDenied(format!(
                "private network traffic blocked: {}",
                dest_ip
            )));
        }

        // Always drop link-local
        if is_link_local(dest_ip) {
            return Err(NetError::AccessDenied(format!(
                "link-local traffic blocked: {}",
                dest_ip
            )));
        }

        // SSRF guard validation (additional ranges like CGNAT, cloud metadata, documentation)
        if let Some(ref guard) = self.ssrf_guard {
            guard.validate_ip(&dest_ip)?;
        }

        // Check if IP is in the allowed list
        self.cleanup_expired();
        let allowed = self.allowed_ips.iter().any(|a| a.ip == dest_ip);
        if !allowed {
            return Err(NetError::AccessDenied(format!(
                "no active network token for {}",
                dest_ip
            )));
        }

        // Check bandwidth
        if self.bandwidth_bps > 0 {
            self.check_bandwidth(packet_size)?;
        }

        Ok(())
    }

    fn check_bandwidth(&mut self, packet_size: usize) -> Result<(), NetError> {
        let now = std::time::Instant::now();
        let elapsed = now.duration_since(self.window_start);

        // Reset window every second
        if elapsed >= std::time::Duration::from_secs(1) {
            self.bytes_transferred = 0;
            self.window_start = now;
        }

        if self.bytes_transferred + packet_size as u64 > self.bandwidth_bps {
            return Err(NetError::AccessDenied("bandwidth limit exceeded".into()));
        }

        self.bytes_transferred += packet_size as u64;
        Ok(())
    }

    fn cleanup_expired(&mut self) {
        let now = std::time::Instant::now();
        self.allowed_ips.retain(|a| a.expires_at > now);
    }

    /// Returns the number of currently active IP allow rules.
    pub fn active_rules(&self) -> usize {
        let now = std::time::Instant::now();
        self.allowed_ips
            .iter()
            .filter(|a| a.expires_at > now)
            .count()
    }

    /// Hot-reload: replace all allowed IPs with a new set.
    pub fn update_allowed_ips(&mut self, ips: Vec<(IpAddr, std::time::Duration)>) {
        self.allowed_ips = ips
            .into_iter()
            .map(|(ip, dur)| AllowedIp {
                ip,
                expires_at: std::time::Instant::now() + dur,
            })
            .collect();
    }

    /// Hot-reload: update the bandwidth limit.
    pub fn update_bandwidth_limit(&mut self, bps: u64) {
        self.bandwidth_bps = bps;
    }
}

impl Default for PacketFilter {
    fn default() -> Self {
        Self::new()
    }
}

/// Returns `true` if the IP is a loopback address (127.0.0.0/8 or ::1).
fn is_loopback(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => v4.is_loopback(),
        IpAddr::V6(v6) => v6.is_loopback(),
    }
}

/// Returns `true` if the IP is a private RFC 1918 address.
fn is_private(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => v4.is_private() || v4.is_unspecified(),
        IpAddr::V6(v6) => {
            let octets = v6.octets();
            // fc00::/7 (unique local)
            (octets[0] & 0xFE) == 0xFC
        }
    }
}

/// Returns `true` if the IP is a link-local address.
fn is_link_local(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => v4.is_link_local(),
        IpAddr::V6(v6) => {
            let octets = v6.octets();
            // fe80::/10
            octets[0] == 0xFE && (octets[1] & 0xC0) == 0x80
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, Ipv6Addr};

    #[test]
    fn test_loopback_blocked() {
        let mut filter = PacketFilter::new();
        let result = filter.check_packet(IpAddr::V4(Ipv4Addr::LOCALHOST), 64);
        assert!(result.is_err());
    }

    #[test]
    fn test_private_blocked() {
        let mut filter = PacketFilter::new();
        let result = filter.check_packet(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1)), 64);
        assert!(result.is_err());
    }

    #[test]
    fn test_link_local_blocked() {
        let mut filter = PacketFilter::new();
        let result = filter.check_packet(IpAddr::V4(Ipv4Addr::new(169, 254, 1, 1)), 64);
        assert!(result.is_err());
    }

    #[test]
    fn test_rfc1918_172_blocked() {
        let mut filter = PacketFilter::new();
        let result = filter.check_packet(IpAddr::V4(Ipv4Addr::new(172, 16, 0, 1)), 64);
        assert!(result.is_err());
    }

    #[test]
    fn test_public_ip_without_token_blocked() {
        let mut filter = PacketFilter::new();
        let result = filter.check_packet(IpAddr::V4(Ipv4Addr::new(1, 1, 1, 1)), 64);
        assert!(result.is_err());
    }

    #[test]
    fn test_public_ip_with_token_allowed() {
        let mut filter = PacketFilter::new();
        let ip = IpAddr::V4(Ipv4Addr::new(1, 1, 1, 1));
        filter.allow_ip(ip, std::time::Duration::from_secs(60));
        let result = filter.check_packet(ip, 64);
        assert!(result.is_ok());
    }

    #[test]
    fn test_expired_token_blocked() {
        let mut filter = PacketFilter::new();
        let ip = IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8));
        filter.allow_ip(ip, std::time::Duration::from_nanos(1));
        // Wait a tiny bit for the token to expire
        std::thread::sleep(std::time::Duration::from_millis(10));
        let result = filter.check_packet(ip, 64);
        assert!(result.is_err());
    }

    #[test]
    fn test_bandwidth_limit() {
        let mut filter = PacketFilter::new().with_bandwidth_limit(100);
        let ip = IpAddr::V4(Ipv4Addr::new(93, 184, 216, 34));
        filter.allow_ip(ip, std::time::Duration::from_secs(60));

        // First packet (64 bytes) should succeed
        assert!(filter.check_packet(ip, 64).is_ok());

        // Second packet (64 bytes) would exceed 100 bps limit
        assert!(filter.check_packet(ip, 64).is_err());
    }

    #[test]
    fn test_ipv6_loopback_blocked() {
        let mut filter = PacketFilter::new();
        let result = filter.check_packet(IpAddr::V6(Ipv6Addr::LOCALHOST), 64);
        assert!(result.is_err());
    }

    #[test]
    fn test_ipv6_link_local_blocked() {
        let mut filter = PacketFilter::new();
        let result =
            filter.check_packet(IpAddr::V6(Ipv6Addr::new(0xfe80, 0, 0, 0, 0, 0, 0, 1)), 64);
        assert!(result.is_err());
    }

    #[test]
    fn test_active_rules_count() {
        let mut filter = PacketFilter::new();
        assert_eq!(filter.active_rules(), 0);

        filter.allow_ip(
            IpAddr::V4(Ipv4Addr::new(1, 1, 1, 1)),
            std::time::Duration::from_secs(60),
        );
        assert_eq!(filter.active_rules(), 1);
    }
}
